use crate::model::{Control, DataList, Entry, FieldSpec, Name, Section};
use crate::process;
use std::fmt::Write;
use unicode_normalization::UnicodeNormalization;

const HEADER: &str = "% $ biblatex auxiliary file $\n% $ biblatex bbl format version 3.3 $\n% Do not modify the above lines!\n%\n% This is an auxiliary file used by the 'biblatex' package.\n% This file may safely be deleted. It will be recreated by\n% biber as required.\n%\n\\begingroup\n\\makeatletter\n\\@ifundefined{ver@biblatex.sty}\n  {\\@latex@error\n     {Missing 'biblatex' package}\n     {The bibliography requires the 'biblatex' package.}\n      \\aftergroup\\endinput}\n  {}\n\\endgroup\n\n";
fn field(out: &mut String,command: &str,key: &str,value: &str) {let _=writeln!(out,"      \\{command}{{{key}}}{{{value}}}");}
fn computed(out: &mut String,e: &Entry,command: &str,key: &str) {if let Some(v)=e.computed.get(key).filter(|s|!s.is_empty()){field(out,command,key,v);}}
fn boolean(out: &mut String,key: &str) {let _=writeln!(out,"      \\true{{{key}}}");}
fn namepart<'a>(n: &'a Name,key: &str)-> &'a str {match key{"family"=>&n.family,"given"=>&n.given,"prefix"=>&n.prefix,"suffix"=>&n.suffix,_=>""}}
fn joined_name(value: &str,protected: bool)->String {
    if protected{return format!("{{{value}}}");}
    let mut out=String::new();let mut previous='\0';let mut whitespace=false;
    for ch in value.chars() {
        if ch=='~'||ch.is_whitespace() {
            if !whitespace {out.push_str(if previous=='.'{"\\bibnamedelimi "}else if ch=='~'{"\\bibnamedelima "}else{"\\bibnamedelimb "});}
            whitespace=true;
        } else {out.push(ch);previous=ch;whitespace=false;}
    }
    out
}
fn render_name(out: &mut String,c: &Control,n: &Name,unique: bool,prefixbase: bool){
    let mut opts=Vec::new();if unique {opts.push(format!("un={}",n.un));opts.push(format!("uniquepart={}",if n.uniquepart.is_empty(){"base"}else{&n.uniquepart}));}
    for (k,v) in &n.options {if matches!(k.as_str(),"useprefix"|"gender"|"giveninits"|"familyinits"|"prefixinits"|"suffixinits") {opts.push(format!("{k}={v}"));}}
    if !n.hash.is_empty(){opts.push(format!("hash={}",n.hash));}let _=writeln!(out,"        {{{{{}}}{{%",opts.join(","));
    let parts:Vec<&str>=if c.nameparts.is_empty(){vec!["family","given","prefix","suffix"]}else{c.nameparts.iter().map(String::as_str).collect()};let mut rows=Vec::new();
    for p in parts {
        let v=namepart(n,p);if v.is_empty(){continue;}
        rows.push(format!("           {p}={{{}}}",joined_name(v,n.strip.contains(p))));
        let initial=n.initials.get(p).map(String::as_str).unwrap_or("");rows.push(format!("           {p}i={{{initial}}}"));
        let base=n.options.get(&format!("biber-base-{p}")).map(|s|s=="1").unwrap_or(p=="family"||(p=="prefix"&&prefixbase));
        if unique&&!base {
            let un=n.options.get(&format!("biber-un-{p}")).and_then(|s|s.parse::<u8>().ok()).unwrap_or(if p=="given"{n.un}else{0});
            rows.push(format!("           {p}un={un}"));
        }
    }
    out.push_str(&rows.join(",\n"));out.push_str("}}%\n");
}
fn spec<'a>(c: &'a Control,key: &str)->Option<&'a FieldSpec>{c.fields.get(key).filter(|s|!s.skip_output)}
fn is_name(c: &Control,key: &str)->bool{spec(c,key).is_some_and(|s|s.datatype=="name"&&s.fieldtype=="list")}
fn entry_options(value: &str)->String {let mut opts=Vec::new();for p in value.split(','){let p=p.trim();let k=p.split('=').next().unwrap_or("");if matches!(k,"skipbib"|"skipbiblist"|"skiplab"|"useauthor"|"useeditor"|"usetranslator"|"useprefix"|"giveninits"|"uniquename"|"uniquelist"|"labelalpha"|"labeldateparts"|"labeltitle"|"labeltitleyear"|"singletitle"|"uniquetitle"|"uniquebaretitle"|"uniquework"|"uniqueprimaryauthor"|"maxbibnames"|"minbibnames"|"maxcitenames"|"mincitenames"|"maxsortnames"|"minsortnames"|"maxitems"|"minitems"|"maxalphanames"|"minalphanames"|"dataonly"|"bibtexcaseprotection"){opts.push(p)}}opts.join(",")}
fn url(value: &str)->String {let mut out=String::with_capacity(value.len());for c in value.nfc(){if c.is_ascii()&&!c.is_ascii_control()&&!matches!(c,' '| '"'|'<'|'>'|'\\'|'^'|'`'|'{'|'|'|'}'){out.push(c);}else{let mut buf=[0;4];for b in c.encode_utf8(&mut buf).bytes(){let _=write!(out,"%{b:02X}");}}}out}
fn page_number(value: &str)->Option<i64> {
    let text=value.nfkd().collect::<String>();if let Ok(n)=text.parse(){return Some(n);}
    let mut total=0;let mut previous=0;
    for ch in text.chars().rev(){let n=match ch.to_ascii_uppercase(){'I'=>1,'V'=>5,'X'=>10,'L'=>50,'C'=>100,'D'=>500,'M'=>1000,_=>return None};if n<previous{total-=n;}else{total+=n;}previous=n;}
    (!text.is_empty()).then_some(total)
}
fn range(value: &str)->(String,i64) {
    let mut parts=Vec::new();let mut count=Some(0i64);
    for raw in value.split([',',';']) {
        let raw=raw.trim();let mut depth=0;let mut split=None;
        for (i,ch) in raw.char_indices(){match ch{'{'=>depth+=1,'}'=>depth-=1,'-'|'–'|'—' if depth==0=>{if raw[i..].starts_with("--"){split=Some((i,2));break;}if split.is_none(){split=Some((i,ch.len_utf8()));}},_=>{}}}
        let clean=|v: &str| {let v=v.trim();v.strip_prefix('{').and_then(|s|s.strip_suffix('}')).unwrap_or(v).to_string()};
        if let Some((i,width))=split {
            let start=clean(&raw[..i]);let end=clean(raw[i+width..].trim_start_matches(['-','–','—']));
            parts.push(format!("{start}\\bibrangedash{}{}",if end.is_empty(){""}else{" "},end));
            if let (Some(a),Some(mut b))=(page_number(&start),page_number(&end)) {
                if b<a&&end.chars().all(|c|c.is_ascii_digit())&&start.chars().all(|c|c.is_ascii_digit())&&end.len()<start.len(){b=page_number(&format!("{}{}",&start[..start.len()-end.len()],end)).unwrap_or(b);}
                count=count.map(|n|n+b-a+1);
            }else{count=None;}
        }else{let start=clean(raw);parts.push(start.clone());count=if page_number(&start).is_some(){count.map(|n|n+1)}else{None};}
    }
    (parts.join("\\bibrangessep "),count.unwrap_or(-1))
}
fn date_metadata(out: &mut String,c: &Control,e: &Entry) {
    for (date,_) in c.fields.iter().filter(|(_,s)|s.datatype=="date") {
        let p=date.strip_suffix("date").unwrap_or(date);
        if let Some(v)=e.fields.get(&format!("{p}dateunspecified")) {field(out,"field",&format!("{p}dateunspecified"),v);}
        for suffix in ["datejulian","enddatejulian","datecirca","enddatecirca","dateuncertain","enddateuncertain","dateunknown","enddateunknown"] {
            if e.flags.contains(&format!("{p}{suffix}")) {boolean(out,&format!("{p}{suffix}"));}
        }
        if e.fields.contains_key(&format!("{p}endyear")) {
            if let Some(v)=e.computed.get(&format!("{p}endera")) {field(out,"field",&format!("{p}enddateera"),v);}
        }
        if e.fields.contains_key(&format!("{p}year"))&&e.computed.contains_key(&format!("{p}datesplit")) {
            if let Some(v)=e.computed.get(&format!("{p}era")) {field(out,"field",&format!("{p}dateera"),v);}
        }
    }
}
fn render_entry(out: &mut String,c: &Control,d: &DataList,s: &Section,e: &Entry){
    if c.skip_types.contains(&e.kind){return;}let count=s.cite_counts.get(&e.key).copied().unwrap_or(e.cite_count);let options=entry_options(e.fields.get("options").map(String::as_str).unwrap_or(""));let _=writeln!(out,"    \\entry{{{}}}{{{}}}{{{options}}}{{{}}}",e.key,e.kind,if count<0{String::new()}else{count.to_string()});
    if e.kind=="set" {
        if let Some(v)=e.fields.get("entryset").or_else(||e.computed.get("entryset")) {let _=writeln!(out,"      \\set{{{v}}}");}
        if process::enabled(c,e,"labelalpha") {computed(out,e,"field","labelalpha");computed(out,e,"field","extraalpha");}
        computed(out,e,"field","sortinit");computed(out,e,"field","sortinithash");
        if !d.labelprefix.is_empty() {field(out,"field","labelprefix",&d.labelprefix);}
        for k in ["label","annotation","shorthand"] {if let Some(v)=e.fields.get(k){field(out,"field",k,v);}}
        keywords(out,e);out.push_str("    \\endentry\n");return;
    }
    if let Some(v)=e.computed.get("inset"){let _=writeln!(out,"      \\inset{{{v}}}");}
    let label=e.computed.get("labelnamesource").map(String::as_str).unwrap_or("");let unique=!matches!(process::option(c,e,"uniquename"),""|"0"|"false")&&!e.computed.contains_key("relatedclone");
    for (k,n) in &e.names {
        if !is_name(c,k){continue;}
        if n.others{boolean(out,&format!("more{k}"));if k==label{boolean(out,"morelabelname");}}
        let mut opts=Vec::new();
        if k==label&&n.uniquelist>0&&!matches!(process::option(c,e,"uniquelist"),""|"0"|"false"){opts.push(format!("ul={}",n.uniquelist));}
        if k==label{for (key,value) in &n.options{if matches!(key.as_str(),"useprefix"|"uniquename"|"uniquelist"|"giveninits"|"maxbibnames"|"minbibnames"|"maxcitenames"|"mincitenames"|"maxalphanames"|"minalphanames"){opts.push(format!("{key}={value}"));}}}
        let _=writeln!(out,"      \\name{{{k}}}{{{}}}{{{}}}{{%",n.names.len(),opts.join(","));
        let unique=unique&&!n.options.get("uniquename").is_some_and(|v|matches!(v.as_str(),"false"|"0"));
        let prefixbase=n.options.get("useprefix").map(|s|matches!(s.as_str(),"1"|"true")).unwrap_or_else(||process::enabled(c,e,"useprefix"));
        for name in &n.names{render_name(out,c,name,unique&&(k==label||!name.uniquepart.is_empty()),prefixbase);}
        out.push_str("      }\n");
    }
    for (k,l) in &e.lists {if !spec(c,k).is_some_and(|s|s.fieldtype=="list"&&matches!(s.datatype.as_str(),"literal"|"key")){continue;}let more=l.last().is_some_and(|v|v.eq_ignore_ascii_case("others"));if more{boolean(out,&format!("more{k}"));}let n=l.len()-usize::from(more);let _=writeln!(out,"      \\list{{{k}}}{{{n}}}{{%");for v in &l[..n]{let _=writeln!(out,"        {{{v}}}%");}out.push_str("      }\n");}
    for k in ["namehash","fullhash","fullhashraw","bibnamehash"]{computed(out,e,"strng",k);}for k in e.names.keys().filter(|k|is_name(c,k)){for suffix in ["bibnamehash","namehash","fullhash","fullhashraw"]{computed(out,e,"strng",&format!("{k}{suffix}"));}}
    computed(out,e,"field","extraname");if process::enabled(c,e,"labelalpha"){computed(out,e,"field","labelalpha");}computed(out,e,"field","sortinit");computed(out,e,"field","sortinithash");
    if process::enabled(c,e,"labeldateparts"){computed(out,e,"field","extradate");computed(out,e,"field","extradatescope");if let Some(v)=e.computed.get("labeldatesource"){field(out,"field","labeldatesource",v);}}
    if !e.fields.contains_key("shorthand")&&!d.labelprefix.is_empty(){field(out,"field","labelprefix",&d.labelprefix);}for k in ["extratitle","extratitleyear","extraalpha"]{computed(out,e,"field",k);}for k in ["crossrefsource","xrefsource","singletitle","uniquetitle","uniquebaretitle","uniquework","uniqueprimaryauthor"]{if e.flags.contains(k){boolean(out,k);}}
    computed(out,e,"field","labelnamesource");computed(out,e,"field","labeltitlesource");if let Some(v)=e.fields.get("clonesourcekey"){field(out,"field","clonesourcekey",v);}
    for (k,v) in &e.fields {let Some(fs)=spec(c,k) else {continue;};if fs.fieldtype!="field"||!fs.format.is_empty()||matches!(fs.datatype.as_str(),"name"|"date"|"range"|"verbatim"|"uri"|"keyword"|"option"){continue;}if v.is_empty()&&!fs.nullok{continue;}let val=if k.ends_with("year"){v.strip_prefix('-').unwrap_or(v)}else{v};field(out,if matches!(k.as_str(),"crossref"|"xref"){"strng"}else{"field"},k,val);}
    date_metadata(out,c,e);
    for (k,v) in &e.fields {if k!="keywords"&&spec(c,k).is_some_and(|s|s.format=="xsv"){field(out,"field",k,v);}}for (k,v) in &e.lists{if k!="keywords"&&spec(c,k).is_some_and(|s|s.format=="xsv"){field(out,"field",k,&v.join(","));}}
    if e.flags.contains("nocite")||e.fields.get("nocite").is_some_and(|v|matches!(v.as_str(),"1"|"true")){boolean(out,"nocite");}
    for (k,v) in &e.fields {if spec(c,k).is_some_and(|s|s.datatype=="range"){let (v,n)=range(v);field(out,"field",k,&v);field(out,"range",k,&n.to_string());}}
    for (k,v) in &e.fields {let Some(fs)=spec(c,k).filter(|s|s.fieldtype=="field"&&matches!(s.datatype.as_str(),"uri"|"verbatim")) else {continue;};if v.is_empty(){continue;}if fs.datatype=="uri"{let _=writeln!(out,"      \\verb{{{k}raw}}\n      \\verb {v}\n      \\endverb");}let encoded;if fs.datatype=="uri"{encoded=url(v);}else{encoded=v.clone();}let _=writeln!(out,"      \\verb{{{k}}}\n      \\verb {encoded}\n      \\endverb");}
    for (k,l) in &e.lists{let Some(fs)=spec(c,k).filter(|s|s.fieldtype=="list"&&matches!(s.datatype.as_str(),"uri"|"verbatim")) else {continue;};let more=l.last().is_some_and(|v|v.eq_ignore_ascii_case("others"));if more{boolean(out,&format!("more{k}"));}let n=l.len()-usize::from(more);if fs.datatype=="uri"{let _=writeln!(out,"      \\lverb{{{k}raw}}{{{n}}}");for v in &l[..n]{let _=writeln!(out,"      \\lverb {v}");}out.push_str("      \\endlverb\n");}let _=writeln!(out,"      \\lverb{{{k}}}{{{n}}}");for v in &l[..n]{let _=writeln!(out,"      \\lverb {}",if fs.datatype=="uri"{url(v)}else{v.clone()});}out.push_str("      \\endlverb\n");}
    keywords(out,e);annotations(out,e);
    for w in &e.warnings{let _=writeln!(out,"      \\warn{{\\item {w}}}");}out.push_str("    \\endentry\n");
}
fn keywords(out: &mut String,e: &Entry){if let Some(v)=e.lists.get("keywords"){let _=writeln!(out,"      \\keyw{{{}}}",v.join(","));}else if let Some(v)=e.fields.get("keywords"){let _=writeln!(out,"      \\keyw{{{v}}}");}}
fn annotations(out: &mut String,e: &Entry) {
    let mut annotations=std::collections::BTreeMap::new();
    for (field,raw) in &e.fields {
        let Some((field,name))=field.split_once("+an") else{continue;};
        let name=name.strip_prefix(':').filter(|s|!s.is_empty()).unwrap_or("default");
        for value in raw.split(';') {
            let Some((target,value))=value.trim().split_once('=') else{continue;};
            let target=target.trim();
            let (item,part)=if let Some((item,part))=target.split_once(':'){(item,part)}else if target.chars().all(|c|c.is_ascii_digit()){(target,"")}else{("",target)};
            let scope=if !part.is_empty(){2}else if !item.is_empty(){1}else{0};
            let literal=value.trim().strip_prefix('"').and_then(|s|s.strip_suffix('"'));
            annotations.insert((scope,field,name,item,part),(u8::from(literal.is_some()),literal.unwrap_or(value)));
        }
    }
    for ((scope,field,name,item,part),(literal,value)) in annotations {
        let scope=match scope{0=>"field",1=>"item",_=>"part"};
        let _=writeln!(out,"      \\annotation{{{scope}}}{{{field}}}{{{name}}}{{{item}}}{{{part}}}{{{literal}}}{{{value}}}");
    }
}

pub fn render(c: &Control,sections: &[(Section,Vec<Entry>)],preamble: &str)->String{
    let mut out=String::from(HEADER);if !preamble.is_empty(){let _=write!(out,"\\preamble{{%\n{preamble}%\n}}\n\n");}
    let mut sections:Vec<_>=sections.iter().collect();sections.sort_by(|a,b|a.0.number.to_string().cmp(&b.0.number.to_string()));
    for (s,entries) in sections {
        if !entries.iter().any(|e|!matches!(e.kind.as_str(),"missing"|"alias")&&!c.skip_types.contains(&e.kind)){continue;}
        let _=write!(out,"\n\\refsection{{{}}}\n",s.number);
        let default_sort=c.options.get("sortingtemplatename").map(String::as_str).unwrap_or("nyt");
        let default_list=DataList{name:format!("{default_sort}/global//global/global/global"),sorting:default_sort.into(),kind:"entry".into(),..DataList::default()};
        let mut lists:Vec<&DataList>=s.lists.iter().collect();
        if !lists.iter().any(|d|d.name==default_list.name){lists.push(&default_list);}
        lists.sort_by(|a,b|{let ag=a.sorting==default_sort&&a.kind=="entry";let bg=b.sorting==default_sort&&b.kind=="entry";ag.cmp(&bg).then_with(||a.sorting.cmp(&b.sorting))});
        for d in lists {
            let mut contextual=entries.clone();let mut order=process::sort(c,d,&contextual);
            order.retain(|&i|!matches!(contextual[i].kind.as_str(),"missing"|"alias")&&!c.skip_types.contains(&contextual[i].kind));
            if d.kind=="shorthand"{order.retain(|&i|contextual[i].fields.contains_key("shorthand"));}
            if order.is_empty(){continue;}
            process::contextualize(c,d,&mut contextual,&order);
            let _=writeln!(out,"  \\datalist[{}]{{{}}}",if d.kind.is_empty(){"entry"}else{&d.kind},d.name);
            for i in order{render_entry(&mut out,c,d,s,&contextual[i]);}out.push_str("  \\enddatalist\n");
        }
        let mut aliases:Vec<_>=entries.iter().filter(|e|e.kind=="alias").collect();
        aliases.sort_by(|a,b|a.key.cmp(&b.key));
        for e in aliases{if let Some(target)=e.computed.get("target"){let _=writeln!(out,"  \\keyalias{{{}}}{{{target}}}",e.key);}}
        let mut missing:Vec<_>=s.citekeys.iter().filter(|k|k.as_str()!="*"&&!entries.iter().any(|e|&e.key==*k&&e.kind!="missing")).collect();
        missing.extend(entries.iter().filter(|e|e.kind=="missing").map(|e|&e.key));
        missing.sort();missing.dedup();for k in missing{let _=writeln!(out,"  \\missing{{{k}}}");}
        out.push_str("\\endrefsection\n");
    }out.push_str("\\endinput\n\n");out
}
