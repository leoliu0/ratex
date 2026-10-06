use crate::model::{Control, DataList, Entry, FieldSpec, Name, Section};
use crate::process;
use std::fmt::Write;
use crate::collation::normalization::normalize;
use std::borrow::Cow;

const HEADER: &str = "% $ biblatex auxiliary file $\n% $ biblatex bbl format version 3.3 $\n% Do not modify the above lines!\n%\n% This is an auxiliary file used by the 'biblatex' package.\n% This file may safely be deleted. It will be recreated by\n% biber as required.\n%\n\\begingroup\n\\makeatletter\n\\@ifundefined{ver@biblatex.sty}\n  {\\@latex@error\n     {Missing 'biblatex' package}\n     {The bibliography requires the 'biblatex' package.}\n      \\aftergroup\\endinput}\n  {}\n\\endgroup\n\n";
fn field(out: &mut String,command: &str,key: &str,value: &str) {let _=writeln!(out,"      \\{command}{{{key}}}{{{value}}}");}
fn print_field(out:&mut String,c:&Control,e:&Entry,command:&str,key:&str,value:&str) {
    let value=if command!="strng"&&e.computed.get("sourcemap_datatype").is_some_and(|s|s.starts_with("biblatexml")){
        let mut escaped=String::with_capacity(value.len());let mut previous='\0';for ch in value.chars(){if matches!(ch,'#'|'&'|'%')&&previous!='\\'{escaped.push('\\');}escaped.push(ch);previous=ch;}Cow::Owned(escaped)
    }else{Cow::Borrowed(value)};
    let columns=c.options.get("wraplines").and_then(|s|s.parse::<usize>().ok()).unwrap_or(0);
    if columns==0{field(out,command,key,&value);return;}
    let length=16+crate::gcstring::length(key)+crate::gcstring::length(&value);
    if length>2*columns{
        let _=writeln!(out,"      \\{command}{{{key}}}{{%");
        out.push_str(&crate::tool::writers::wrap(&value,"      ","      ",columns,true));out.push_str("%\n      }\n");
    }else if length>columns{
        out.push_str(&crate::tool::writers::wrap(&format!("\\{command}{{{key}}}{{{value}}}"),"      ","      ",columns,true));out.push('\n');
    }else{field(out,command,key,&value);}
}
fn recode_text<'a>(c:&Control,text:&'a str,key:Option<&str>,warnings:&mut Vec<String>)->Result<Cow<'a,str>,String>{
    let encoding=c.options.get("output_encoding").map(String::as_str).unwrap_or("UTF-8");
    let safe=c.options.get("output_safechars").is_some_and(|s|matches!(s.as_str(),"1"|"true"|"yes"))||encoding.to_ascii_lowercase().ends_with("ascii")&&!c.options.get("input_encoding").is_some_and(|s|s.to_ascii_lowercase().ends_with("ascii"));
    let automatic=!safe&&encoding!="UTF-8"&&crate::encoding::encode_probe(&normalize(text,"NFC"),encoding)?.contains(&0);
    if automatic{if let Some(key)=key{warnings.push(format!("The entry '{key}' has characters which cannot be encoded in '{encoding}'. Recoding problematic characters into macros."));}}
    if safe||automatic{Ok(Cow::Owned(crate::tool::recode::encode(text,c.options.get("output_safecharsset").map(String::as_str).unwrap_or("base"))))}else{Ok(Cow::Borrowed(text))}
}
fn computed(out: &mut String,e: &Entry,command: &str,key: &str) {if let Some(v)=e.computed.get(key).filter(|s|!s.is_empty()){field(out,command,key,v);}}
/// DataList::instantiate_entry: sortinit and its hash are written whenever defined, even empty.
fn sortinit(out: &mut String,e: &Entry) {if let Some(v)=e.computed.get("sortinit"){field(out,"field","sortinit",v);if let Some(h)=e.computed.get("sortinithash"){field(out,"field","sortinithash",h);}}}
fn boolean(out: &mut String,key: &str) {let _=writeln!(out,"      \\true{{{key}}}");}
fn namepart<'a>(n: &'a Name,key: &str)-> &'a str {n.part(key)}
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
/// Name::name_to_bbl with DataList::instantiate_entry's UNS/UNP substitution: placeholders exist
/// when the uniquename option is not false, and are filled when the name has uniquename state.
fn render_name(out: &mut String,c: &Control,n: &Name,unique: bool){
    let state=n.uniquename.as_ref().filter(|_|unique);
    let mut opts=Vec::new();if let Some(u)=state {opts.push(format!("un={}",u.summary));opts.push(format!("uniquepart={}",u.part));}
    for (k,v) in &n.options {if crate::names::output_option(c,"NAME",k)||k=="gender" {opts.push(format!("{k}={v}"));}}
    if !n.hash.is_empty(){opts.push(format!("hash={}",n.hash));}let _=writeln!(out,"        {{{{{}}}{{%",opts.join(","));
    let parts:Vec<&str>=if c.nameparts.is_empty(){vec!["family","given","prefix","suffix"]}else{c.nameparts.iter().map(String::as_str).collect()};let mut rows=Vec::new();
    for p in parts {
        let v=namepart(n,p);if v.is_empty(){continue;}
        rows.push(format!("           {p}={{{}}}",joined_name(v,n.strip.contains(p))));
        let initial=n.initials.get(p).map(String::as_str).unwrap_or("");rows.push(format!("           {p}i={{{initial}}}"));
        if let Some(u)=state.filter(|u|!u.base_parts.iter().any(|b|b==p)) {rows.push(format!("           {p}un={}",u.parts.get(p).copied().unwrap_or(0)));}
    }
    out.push_str(&rows.join(",\n"));out.push_str("}}%\n");
}
fn spec<'a>(c: &'a Control,key: &str)->Option<&'a FieldSpec>{c.fields.get(key).filter(|s|!s.skip_output)}
fn is_name(c: &Control,key: &str)->bool{spec(c,key).is_some_and(|s|s.datatype=="name"&&s.fieldtype=="list")}
fn entry_options(c:&Control,value: &str)->String {
    crate::names::expand_options(c,"ENTRY",value).into_iter().filter(|(key,_)|crate::names::output_option(c,"ENTRY",key)).map(|(k,v)|if v.is_empty(){k}else{format!("{k}={v}")}).collect::<Vec<_>>().join(",")
}
fn url(value: &str)->String {let mut out=String::with_capacity(value.len());for c in normalize(value,"NFC").chars(){if c.is_ascii()&&!c.is_ascii_control()&&!matches!(c,' '| '"'|'<'|'>'|'\\'|'^'|'`'|'{'|'|'|'}'){out.push(c);}else{let mut buf=[0;4];for b in c.encode_utf8(&mut buf).bytes(){let _=write!(out,"%{b:02X}");}}}out}
fn page_number(value: &str)->Option<i64> {
    let text=normalize(value,"NFKD");if let Ok(n)=text.parse(){return Some(n);}
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
    if c.skip_types.contains(&e.kind){return;}let count=s.cite_counts.get(&e.key).copied().unwrap_or(e.cite_count);let options=entry_options(c,e.fields.get("options").map(String::as_str).unwrap_or(""));let _=writeln!(out,"    \\entry{{{}}}{{{}}}{{{options}}}{{{}}}",e.key,e.kind,if count<0{String::new()}else{count.to_string()});
    if e.kind=="set" {
        if let Some(v)=e.fields.get("entryset").or_else(||e.computed.get("entryset")) {let _=writeln!(out,"      \\set{{{v}}}");}
        if process::enabled(c,e,"labelalpha") {computed(out,e,"field","labelalpha");computed(out,e,"field","extraalpha");}
        sortinit(out,e);
        if !d.labelprefix.is_empty() {field(out,"field","labelprefix",&d.labelprefix);}
        for k in ["label","annotation","shorthand"] {if let Some(v)=e.fields.get(k){field(out,"field",k,v);}}
        keywords(out,e);out.push_str("    \\endentry\n");return;
    }
    if let Some(v)=e.computed.get("inset"){let _=writeln!(out,"      \\inset{{{v}}}");}
    let label=e.computed.get("labelnamesource").map(String::as_str).unwrap_or("");
    let labelname=e.names.get(label);let truthy=|v:&&String|!v.is_empty()&&v.as_str()!="0";
    let ul=labelname.and_then(|n|n.options.get("uniquelist")).filter(truthy).map_or_else(||crate::uniqueness::uniquelist_mode(process::option(c,e,"uniquelist")),|v|crate::uniqueness::uniquelist_mode(v));
    let mut un=labelname.and_then(|n|n.options.get("uniquename")).filter(truthy).map_or_else(||crate::uniqueness::uniquename_mode(process::option(c,e,"uniquename")),|v|crate::uniqueness::uniquename_mode(v));
    for (k,n) in &e.names {
        if !is_name(c,k){continue;}
        if n.others{boolean(out,&format!("more{k}"));if k==label{boolean(out,"morelabelname");}}
        if k==label{if let Some(v)=n.options.get("uniquename"){un=crate::uniqueness::uniquename_mode(v);}}
        let mut opts=Vec::new();
        if k==label&&ul!="false"{if let Some(width)=n.uniquelist{opts.push(format!("ul={width}"));}}
        if k==label{for (key,value) in &n.options{if crate::names::output_option(c,"NAMELIST",key){opts.push(format!("{key}={value}"));}}}
        let _=writeln!(out,"      \\name{{{k}}}{{{}}}{{{}}}{{%",n.names.len(),opts.join(","));
        for name in &n.names{render_name(out,c,name,un!="false");}
        out.push_str("      }\n");
    }
    for (k,l) in &e.lists {if !spec(c,k).is_some_and(|s|s.fieldtype=="list"&&matches!(s.datatype.as_str(),"literal"|"key")){continue;}let more=l.last().is_some_and(|v|v.to_lowercase()==c.options.get("others_string").map(String::as_str).unwrap_or("others"));if more{boolean(out,&format!("more{k}"));}let n=l.len()-usize::from(more);let _=writeln!(out,"      \\list{{{k}}}{{{n}}}{{%");for v in &l[..n]{let _=writeln!(out,"        {{{v}}}%");}out.push_str("      }\n");}
    for k in ["namehash","fullhash","fullhashraw","bibnamehash"]{computed(out,e,"strng",k);}for k in e.names.keys().filter(|k|is_name(c,k)){for suffix in ["bibnamehash","namehash","fullhash","fullhashraw"]{computed(out,e,"strng",&format!("{k}{suffix}"));}}
    computed(out,e,"field","extraname");if process::enabled(c,e,"labelalpha"){computed(out,e,"field","labelalpha");}sortinit(out,e);
    if process::enabled(c,e,"labeldateparts"){computed(out,e,"field","extradate");computed(out,e,"field","extradatescope");if let Some(v)=e.computed.get("labeldatesource"){field(out,"field","labeldatesource",v);}}
    if !e.fields.contains_key("shorthand")&&!d.labelprefix.is_empty(){field(out,"field","labelprefix",&d.labelprefix);}for k in ["extratitle","extratitleyear","extraalpha"]{computed(out,e,"field",k);}for k in ["crossrefsource","xrefsource","singletitle","uniquetitle","uniquebaretitle","uniquework","uniqueprimaryauthor"]{if e.flags.contains(k){boolean(out,k);}}
    computed(out,e,"field","labelnamesource");computed(out,e,"field","labeltitlesource");if let Some(v)=e.fields.get("clonesourcekey"){field(out,"field","clonesourcekey",v);}
    for (k,v) in &e.fields {let Some(fs)=spec(c,k) else {continue;};if fs.fieldtype!="field"||!matches!(fs.format.as_str(),""|"default")||matches!(fs.datatype.as_str(),"name"|"date"|"range"|"verbatim"|"uri"|"keyword"|"option"){continue;}if v.is_empty()&&!fs.nullok{continue;}let val=if k.ends_with("year"){v.strip_prefix('-').unwrap_or(v)}else{v};print_field(out,c,e,if matches!(k.as_str(),"crossref"|"xref"){"strng"}else{"field"},k,val);}
    date_metadata(out,c,e);
    for (k,v) in &e.fields {if k!="keywords"&&spec(c,k).is_some_and(|s|s.format=="xsv"){print_field(out,c,e,"field",k,v);}}for (k,v) in &e.lists{if k!="keywords"&&spec(c,k).is_some_and(|s|s.format=="xsv"){print_field(out,c,e,"field",k,&v.join(","));}}
    if e.flags.contains("nocite")||e.fields.get("nocite").is_some_and(|v|matches!(v.as_str(),"1"|"true")){boolean(out,"nocite");}
    for (k,v) in &e.fields {if spec(c,k).is_some_and(|s|s.datatype=="range"){
        let (v,n)=if let Some(ranges)=e.ranges.get(k){
            // BibLaTeXML _parse_range_list always defines the end ('' when absent): bbl.pm emits `\bibrangedash`; Utils::rangelen gives -1 unless both ends are true numerals.
            let truthy=|v:&str|!v.is_empty()&&v!="0";let mut count=Some(0i64);let mut parts=Vec::new();
            for (start,end) in ranges {parts.push(format!("{start}\\bibrangedash{}",if truthy(end){format!(" {end}")}else{String::new()}));count=count.and_then(|n|{if !truthy(start)||!truthy(end){return None;}let (a,mut b)=(page_number(start)?,page_number(end)?);if b<a{let m=a.to_string().chars().rev().collect::<Vec<_>>();let mut o=b.to_string().chars().rev().collect::<Vec<_>>();for i in 0..m.len(){if i==o.len(){o.push(m[i]);}else if o[i]=='0'{o[i]=m[i];}}b=o.iter().rev().collect::<String>().parse().ok()?;}Some(n+b-a+1)});}
            (parts.join("\\bibrangessep "),count.unwrap_or(-1))
        }else{range(v)};field(out,"field",k,&v);field(out,"range",k,&n.to_string());
    }}
    for (k,v) in &e.fields {let Some(fs)=spec(c,k).filter(|s|s.fieldtype=="field"&&matches!(s.datatype.as_str(),"uri"|"verbatim")) else {continue;};if v.is_empty(){continue;}if fs.datatype=="uri"{let _=writeln!(out,"      \\verb{{{k}raw}}\n      \\verb {v}\n      \\endverb");}let encoded;if fs.datatype=="uri"{encoded=url(v);}else{encoded=v.clone();}let _=writeln!(out,"      \\verb{{{k}}}\n      \\verb {encoded}\n      \\endverb");}
    for (k,l) in &e.lists{let Some(fs)=spec(c,k).filter(|s|s.fieldtype=="list"&&matches!(s.datatype.as_str(),"uri"|"verbatim")) else {continue;};let more=l.last().is_some_and(|v|v.to_lowercase()==c.options.get("others_string").map(String::as_str).unwrap_or("others"));if more{boolean(out,&format!("more{k}"));}let n=l.len()-usize::from(more);if fs.datatype=="uri"{let _=writeln!(out,"      \\lverb{{{k}raw}}{{{n}}}");for v in &l[..n]{let _=writeln!(out,"      \\lverb {v}");}out.push_str("      \\endlverb\n");}let _=writeln!(out,"      \\lverb{{{k}}}{{{n}}}");for v in &l[..n]{let _=writeln!(out,"      \\lverb {}",if fs.datatype=="uri"{url(v)}else{v.clone()});}out.push_str("      \\endlverb\n");}
    keywords(out,e);annotations(out,e);
    for w in &e.warnings{let _=writeln!(out,"      \\warn{{\\item {w}}}");}out.push_str("    \\endentry\n");
}
fn keywords(out: &mut String,e: &Entry){if let Some(v)=e.lists.get("keywords"){let _=writeln!(out,"      \\keyw{{{}}}",v.join(","));}else if let Some(v)=e.fields.get("keywords"){let _=writeln!(out,"      \\keyw{{{v}}}");}}
fn annotations(out: &mut String,e: &Entry) {
    let mut annotations=e.annotations.iter().collect::<Vec<_>>();
    annotations.sort_by(|a,b| {
        let scope=|a:&crate::model::Annotation|if !a.part.is_empty(){2}else if !a.item.is_empty(){1}else{0};
        scope(a).cmp(&scope(b)).then_with(||a.field.cmp(&b.field)).then_with(||a.name.cmp(&b.name)).then_with(||a.item.cmp(&b.item)).then_with(||a.part.cmp(&b.part))
    });
    for a in annotations {
        let scope=if !a.part.is_empty(){"part"}else if !a.item.is_empty(){"item"}else{"field"};
        let _=writeln!(out,"      \\annotation{{{scope}}}{{{}}}{{{}}}{{{}}}{{{}}}{{{}}}{{{}}}",a.field,a.name,a.item,a.part,u8::from(a.literal),a.value);
    }
}

pub(crate) struct Rendered {pub text:String,pub warnings:Vec<String>,pub infos:Vec<String>}
pub(crate) fn render(c: &Control,sections: &[(Section,Vec<Entry>)],preamble: &str)->Result<Rendered,String>{
    let mut warnings=Vec::new();let mut infos=Vec::new();let mut out=String::from(HEADER);if !preamble.is_empty(){let preamble=recode_text(c,preamble,None,&mut warnings)?;let _=write!(out,"\\preamble{{%\n{preamble}%\n}}\n\n");}
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
            // Skipout entrytypes (XDATA) stay citekeys in Perl and take part in extra*/unique* tracking; only bbl.pm's set_output_entry drops them.
            order.retain(|&i|!matches!(contextual[i].kind.as_str(),"missing"|"alias"));
            if d.kind=="shorthand"{order.retain(|&i|contextual[i].fields.contains_key("shorthand"));}
            let output=order.iter().copied().filter(|&i|!c.skip_types.contains(&contextual[i].kind)).collect::<Vec<_>>();
            if output.is_empty(){continue;}
            process::contextualize(c,d,&mut contextual,&order)?;
            let mut collation_info=crate::collation::tool_log_info(c,d);
            let tailoring=collation_info.last().is_some_and(|s|s.starts_with("No sort tailoring")).then(||collation_info.pop().unwrap());
            infos.extend(collation_info);
            let locale=c.options.get("sortlocale_override").map(String::as_str).or_else(||c.sorting_locales.get(&d.sorting).map(|s|crate::collation::language_locale(s))).or_else(||c.options.get("sortlocale").map(|s|crate::collation::language_locale(s))).unwrap_or("en_US");
            infos.push(format!("Sorting list '{}' of type '{}' with template '{}' and locale '{locale}'",d.name,if d.kind.is_empty(){"entry"}else{&d.kind},d.sorting));
            if let Some(tailoring)=tailoring{infos.push(tailoring);}
            let _=writeln!(out,"  \\datalist[{}]{{{}}}",if d.kind.is_empty(){"entry"}else{&d.kind},d.name);
            for i in output{
                let start=out.len();render_entry(&mut out,c,d,s,&contextual[i]);
                if let Cow::Owned(text)=recode_text(c,&out[start..],Some(&contextual[i].key),&mut warnings)?{out.truncate(start);out.push_str(&text);}
            }out.push_str("  \\enddatalist\n");
        }
        let mut aliases:Vec<_>=entries.iter().filter(|e|e.kind=="alias").collect();
        aliases.sort_by(|a,b|a.key.cmp(&b.key));
        for e in aliases{if let Some(target)=e.computed.get("target"){let _=writeln!(out,"  \\keyalias{{{}}}{{{target}}}",e.key);}}
        let allkeys=s.cite_orders.contains_key("*");
        let mut missing:Vec<_>=s.citekeys.iter().filter(|k|!allkeys&&k.as_str()!="*"&&!entries.iter().any(|e|&e.key==*k&&e.kind!="missing")).collect();
        missing.extend(entries.iter().filter(|e|e.kind=="missing").map(|e|&e.key));
        missing.sort();missing.dedup();for k in missing{let _=writeln!(out,"  \\missing{{{k}}}");}
        out.push_str("\\endrefsection\n");
    }out.push_str("\\endinput\n\n");Ok(Rendered{text:normalize(&out,"NFC").into_owned(),warnings,infos})
}
