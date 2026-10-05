use crate::model::{Control, DataList, Entry, LabelField, Name, NameList, SortItem};
use regex::Regex;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

pub(crate) fn digest(value: &str) -> String { crate::md5_hex(value.as_bytes()) }
pub(crate) fn enabled(c: &Control, e: &Entry, key: &str) -> bool { matches!(option(c,e,key), "1" | "true" | "yes") }
pub(crate) fn option<'a>(c: &'a Control, e: &'a Entry, key: &str) -> &'a str {
    if let Some(opts) = e.fields.get("options") {
        for p in opts.split(',') { let (k,v) = p.trim().split_once('=').unwrap_or((p.trim(),"true")); if k == key { return v; } }
    }
    c.option(&e.kind,key)
}
fn number(c: &Control, e: &Entry, key: &str, fallback: usize) -> usize { option(c,e,key).parse().unwrap_or(fallback) }
fn hash_normalize(value: &str) -> String {
    static MACRO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\(\p{L}+|[^\p{L}])\s*").unwrap());
    let s = MACRO.replace_all(value, |cap: &regex::Captures<'_>| { let x=&cap[1]; if x.chars().all(char::is_alphabetic) { format!("{x}:") } else { format!("{}:",x.chars().next().unwrap() as u32) } });
    s.chars().filter(|x| !matches!(x,'{'|'}'|'~'|'.') && !x.is_whitespace()).collect::<String>().nfc().collect()
}
fn norm(value: &str) -> String {
    static MACRO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\[A-Za-z]+").unwrap());
    let s=MACRO.replace_all(value, "");
    s.replace(['{','}'],"").replace('~'," ").split_whitespace().collect::<Vec<_>>().join(" ")
}
fn part<'a>(n: &'a Name, p: &str) -> &'a str { match p {"family"=>&n.family,"given"=>&n.given,"prefix"=>&n.prefix,"suffix"=>&n.suffix,_=>""} }
fn raw_name(c: &Control,n: &Name) -> String {
    let mut s=String::new();
    if c.nameparts.is_empty() { for p in ["family","given","prefix","suffix"] { s.push_str(part(n,p)); } }
    else { for p in &c.nameparts { s.push_str(part(n,p)); } } s
}
fn hash_name(c: &Control,n: &Name,template: &str)->String {
    if !n.hashid.is_empty() {return n.hashid.clone();}
    let name=n.options.get("namehashtemplatename").map(String::as_str).unwrap_or(template);
    let Some(parts)=c.name_templates.get("namehashtemplate").and_then(|t|t.get(name)) else {return raw_name(c,n);};
    let mut out=String::new();
    for p in if c.nameparts.is_empty(){vec!["family","given","prefix","suffix"]}else{c.nameparts.iter().map(String::as_str).collect()} {
        if let Some(spec)=parts.iter().flatten().find(|s|s.name==p) {
            match spec.attributes.get("hashscope").map(String::as_str).unwrap_or("full") {
                "full"=>out.push_str(part(n,p)),
                "init"=>{for token in n.initial_tokens.get(p).into_iter().flatten(){out.push_str(token);}},
                _=>{}
            }
        }
    }
    out
}
fn name_hash(c: &Control,n: &Name,template: &str) -> String {digest(&hash_normalize(&hash_name(c,n,template)))}
fn list_hash(c: &Control,n: &NameList,visible: usize,raw: bool,noothers: bool,template: &str) -> String {
    let mut s=String::new();
    for name in n.names.iter().take(visible) {s.push_str(&if raw{raw_name(c,name)}else{hash_name(c,name,template)});}
    if !noothers && (n.others || visible<n.names.len()) {s.push('+');} digest(&hash_normalize(&s))
}
fn visible(c: &Control,e: &Entry,n: &NameList,scope: &str) -> usize {
    let local=|key: &str,fallback: usize|n.options.get(key).or_else(||n.options.get(if key.starts_with("max"){"maxnames"}else{"minnames"})).and_then(|s|s.parse().ok()).unwrap_or(fallback);
    let max=local(&format!("max{scope}names"),number(c,e,&format!("max{scope}names"),3));
    let min=local(&format!("min{scope}names"),number(c,e,&format!("min{scope}names"),1));
    let v=if n.names.len()>max {min} else {n.names.len()}; v.max(n.uniquelist).min(n.names.len())
}
#[derive(Clone)]
struct NameDis { base:String, baseparts:Vec<String>, parts:Vec<(String,String,String,String)> }
fn name_dis(c: &Control,e: &Entry,list: &NameList,name: &Name)->NameDis {
    let local=option(c,e,"uniquenametemplatename");
    let template=name.options.get("uniquenametemplatename").or_else(||list.options.get("uniquenametemplatename")).map(String::as_str).unwrap_or(if local.is_empty(){"global"}else{local});
    let schema=c.name_templates.get("uniquenametemplate").and_then(|t|t.get(template));
    let useprefix=name.options.get("useprefix").or_else(||list.options.get("useprefix")).map(|s|matches!(s.as_str(),"1"|"true")).unwrap_or_else(||enabled(c,e,"useprefix"));
    let mut dis=NameDis{base:String::new(),baseparts:Vec::new(),parts:Vec::new()};
    if let Some(schema)=schema {
        for p in schema.iter().flatten() {
            if p.attributes.get("use").is_some_and(|s|s=="1")&&p.name=="prefix"&&!useprefix {continue;}
            let value=part(name,&p.name);if value.is_empty(){continue;}
            if p.attributes.get("base").is_some_and(|s|s=="1") {dis.base.push_str(value);dis.baseparts.push(p.name.clone());}
            else {
                let mode=p.attributes.get("disambiguation").map(String::as_str).unwrap_or("full");
                if mode!="none"{dis.parts.push((p.name.clone(),value.to_string(),name.initial_tokens.get(&p.name).map(|s|s.join("")).unwrap_or_default(),mode.into()));}
            }
        }
    } else {
        if useprefix{dis.base.push_str(&name.prefix);dis.baseparts.push("prefix".into());}dis.base.push_str(&name.family);dis.baseparts.push("family".into());
        if !name.given.is_empty(){dis.parts.push(("given".into(),name.given.clone(),name.initial_tokens.get("given").map(|s|s.join("")).unwrap_or_default(),"full".into()));}
    }
    dis.base=hash_normalize(&dis.base);
    for (_,full,initial,_) in &mut dis.parts{*full=hash_normalize(full);*initial=hash_normalize(initial);}
    dis
}
fn dis_identity(dis: &NameDis)->String {dis.parts.iter().map(|p|p.1.as_str()).collect::<Vec<_>>().join("\0")}
fn uniqueness_key(c: &Control,e: &Entry,list: &NameList) -> String {
    let mut s=String::new();
    for name in list.names.iter().take(visible(c,e,list,"cite")) {
        let dis=name_dis(c,e,list,name);s.push_str(&dis.base);
        for (p,full,init,_) in dis.parts {
            let level=name.options.get(&format!("biber-un-{p}")).and_then(|v|v.parse::<u8>().ok()).unwrap_or(if p=="given"{name.un}else{0});
            if level==1{s.push_str(&init);}else if level==2{s.push_str(&full);}
        }
    }
    let noothers=list.options.get("nohashothers").map(|s|matches!(s.as_str(),"1"|"true")).unwrap_or_else(||enabled(c,e,"nohashothers"));
    if !noothers&&(list.others||visible(c,e,list,"cite")<list.names.len()){s.push('+');}
    digest(&hash_normalize(&s))
}
fn rehash(c: &Control,e: &mut Entry,context_template: &str) {
    let fields:Vec<String>=e.names.keys().cloned().collect();
    for f in fields {
        let list=&e.names[&f];
        let entry=option(c,e,"namehashtemplatename");
        let entry=if context_template!="global"&&(entry.is_empty()||entry=="global"){context_template}else if entry.is_empty(){"global"}else{entry};
        let template=list.options.get("namehashtemplatename").map(String::as_str).unwrap_or(entry).to_string();
        let nh=list.options.get("nohashothers").map(|s|matches!(s.as_str(),"1"|"true")).unwrap_or_else(||enabled(c,e,"nohashothers"));
        let hash=list_hash(c,list,visible(c,e,list,"cite"),false,nh,&template);
        let full=list_hash(c,list,list.names.len(),false,false,&template);
        let raw=list_hash(c,list,list.names.len(),true,false,&template);
        let bib=list_hash(c,list,visible(c,e,list,"bib"),false,nh,&template);
        for (suffix,value) in [("namehash",&hash),("fullhash",&full),("fullhashraw",&raw),("bibnamehash",&bib)]{e.computed.insert(format!("{f}{suffix}"),value.clone());}
        let list=e.names.get_mut(&f).unwrap();list.hash=hash;list.fullhash=full;list.bibnamehash=bib;
        for name in &mut list.names{name.hash=name_hash(c,name,&template);}
    }
    if let Some(f)=e.computed.get("labelnamesource").cloned() {
        for suffix in ["namehash","bibnamehash"]{if let Some(v)=e.computed.get(&format!("{f}{suffix}")).cloned(){e.computed.insert(suffix.into(),v);}}
        let key=uniqueness_key(c,e,&e.names[&f]);e.computed.insert("nameuniqueness".into(),key);
    }
    if let Some(f)=e.computed.get("labelnamefullsource").cloned(){for suffix in ["fullhash","fullhashraw"]{if let Some(v)=e.computed.get(&format!("{f}{suffix}")).cloned(){e.computed.insert(suffix.into(),v);}}}
}

pub fn prepare(c: &Control, entries: &mut [Entry]) {
    for p in ["family","given","prefix","suffix"] {
        let width=entries.iter().flat_map(|e|e.names.values()).flat_map(|l|&l.names).map(|n|part(n,p).graphemes(true).count()).max().unwrap_or(0);
        for e in entries.iter_mut() {e.computed.insert(format!("sortwidth-{p}"),width.to_string());}
    }
    for e in entries.iter_mut() {
        let candidates: Vec<&str>=if c.labelname.is_empty(){vec!["shortauthor","author","shorteditor","editor","translator"]}else{c.labelname.iter().map(String::as_str).collect()};
        for field in &candidates {let k=field.strip_prefix("short").unwrap_or(field); if option(c,e,&format!("use{k}"))=="0" || option(c,e,&format!("use{k}"))=="false" {continue;} if e.names.contains_key(*field) {e.computed.insert("labelnamesource".into(),field.to_string());break;}}
        for field in &candidates { if c.fields.get(*field).is_some_and(|s|s.label)||field.starts_with("short") {continue;} let k=format!("use{field}"); if matches!(option(c,e,&k),"0"|"false") {continue;} if e.names.contains_key(*field) {e.computed.insert("labelnamefullsource".into(),field.to_string());break;} }
        let ts:Vec<&str>=if c.labeltitle.is_empty(){vec!["shorttitle","title"]}else{c.labeltitle.iter().map(String::as_str).collect()};
        for f in ts {if let Some(v)=e.fields.get(f).filter(|v| !v.is_empty()) {e.computed.insert("labeltitlesource".into(),f.into()); e.computed.insert("labeltitle".into(),v.clone());break;}}
        if enabled(c,e,"labeldateparts") {
            let specs:Vec<(&str,bool)>=if !c.labeldate_specs.is_empty(){c.labeldate_specs.iter().map(|s|(s.name.as_str(),s.literal)).collect()}else if !c.labeldate.is_empty(){c.labeldate.iter().map(|s|(s.as_str(),s=="nodate")).collect()}else{vec![("date",false),("year",false),("eventdate",false),("origdate",false),("urldate",false),("nodate",true)]};
            for (f,literal) in specs {if literal {e.computed.insert("labeldatesource".into(),f.into());break;}
                let prefix=f.strip_suffix("date"); let y=prefix.map(|p|format!("{p}year")).unwrap_or_else(||f.into());
                if !e.fields.contains_key(&y) && !prefix.is_some_and(|p|e.fields.contains_key(&format!("{p}endyear"))) {continue;}
                e.computed.insert("labeldatesource".into(),prefix.unwrap_or(f).into());
                for p in ["year","month","day","hour","minute","second","timezone"] {let source=prefix.map(|x|format!("{x}{p}")).unwrap_or_else(||y.clone()); if prefix.is_none() && p!="year" {continue;}
                    let mut v=e.fields.get(&source).cloned().unwrap_or_default(); let end=prefix.map(|x|format!("{x}end{p}")); if let Some(ev)=end.and_then(|x|e.fields.get(&x)) {if ev!=&v {v.push_str("\\bibdatedash ");v.push_str(ev);}} if !v.is_empty(){e.computed.insert(format!("label{p}"),v);}}
                break;
            }
            if e.computed.contains_key("labelyear") {e.computed.insert("extradatescope".into(),"labelyear".into());}
        }
    }
    // Uniquename compares distinct representations, not repeated citations.
    let mut people:BTreeMap<String,BTreeMap<String,NameDis>>=BTreeMap::new();
    for e in entries.iter() {
        if enabled(c,e,"skiplab"){continue;}
        if let Some(list)=e.computed.get("labelnamesource").and_then(|f|e.names.get(f)) {
            for name in &list.names {let dis=name_dis(c,e,list,name);people.entry(dis.base.clone()).or_default().insert(dis_identity(&dis),dis);}
        }
    }
    for e in entries.iter_mut() {
        let Some(source)=e.computed.get("labelnamesource").cloned() else{continue;};
        if enabled(c,e,"skiplab"){continue;}
        let list=&e.names[&source];
        let un=list.options.get("uniquename").map(String::as_str).unwrap_or_else(||option(c,e,"uniquename")).to_string();
        if matches!(un.as_str(),""|"0"|"false"){continue;}
        let resolutions:Vec<_>=list.names.iter().map(|name| {
            let dis=name_dis(c,e,list,name);let peers=people.get(&dis.base);
            let mut levels=BTreeMap::new();let mut summary=0;let mut unique="base".to_string();let mut prefix=String::new();
            if peers.is_some_and(|p|p.len()>1) {
                for (i,(p,full,init,mode)) in dis.parts.iter().enumerate() {
                    let distinct=|value: &str,initial: bool|peers.unwrap().values().filter(|other| {
                        let before=other.parts.iter().take(i).flat_map(|p|p.1.chars());
                        let current=other.parts.get(i).map(|p|if initial{p.2.as_str()}else{p.1.as_str()}).unwrap_or("");
                        before.chain(current.chars()).eq(prefix.chars().chain(value.chars()))
                    }).count();
                    if mode!="fullonly"&&distinct(init,true)==1 {levels.insert(p.clone(),1);summary=summary.max(1);unique=p.clone();break;}
                    if mode!="init"&&un!="init"&&distinct(full,false)==1 {levels.insert(p.clone(),2);summary=2;unique=p.clone();break;}
                    if mode=="init"||un=="init"{levels.insert(p.clone(),1);summary=summary.max(1);unique=p.clone();break;}
                    levels.insert(p.clone(),2);summary=2;unique=p.clone();prefix.push_str(full);
                }
            }
            (summary,unique,levels,dis.baseparts)
        }).collect();
        for (name,(summary,unique,levels,baseparts)) in e.names.get_mut(&source).unwrap().names.iter_mut().zip(resolutions) {
            name.un=summary;name.uniquepart=unique;
            for p in ["family","given","prefix","suffix"]{name.options.insert(format!("biber-base-{p}"),u8::from(baseparts.iter().any(|s|s==p)).to_string());}
            for (p,level) in levels{name.options.insert(format!("biber-un-{p}"),level.to_string());}
        }
    }
    let signature=|list: &NameList|list.names.iter().map(|n|raw_name(c,n)).collect::<Vec<_>>().join("\0");
    let mut list_uniqueness=BTreeMap::new();
    for e in entries.iter(){for list in e.names.values(){if list.names.iter().any(|n|!n.uniquepart.is_empty()){list_uniqueness.insert(signature(list),list.names.iter().map(|n|(n.un,n.uniquepart.clone(),n.options.iter().filter(|(k,_)|k.starts_with("biber-")).map(|(k,v)|(k.clone(),v.clone())).collect::<BTreeMap<_,_>>())).collect::<Vec<_>>());}}}
    for e in entries.iter_mut(){for list in e.names.values_mut(){if let Some(values)=list_uniqueness.get(&signature(list)){for (n,(un,unique,parts)) in list.names.iter_mut().zip(values){n.un=*un;n.uniquepart=unique.clone();n.options.extend(parts.clone());}}}}
    let lists:Vec<Option<NameList>>=entries.iter().map(|e|e.computed.get("labelnamesource").and_then(|f|e.names.get(f)).cloned()).collect();
    for (i,e) in entries.iter_mut().enumerate() {
        let Some(a)=&lists[i] else{continue;};
        if matches!(a.options.get("uniquelist").map(String::as_str).unwrap_or_else(||option(c,e,"uniquelist")),""|"0"|"false"){continue;}
        let initial=visible(c,e,a,"cite");let mut needed=initial;
        for (j,b) in lists.iter().enumerate() {
            if i==j{continue;}let Some(b)=b else{continue;};
            let start=needed.min(b.names.len());
            if !a.names.iter().take(start).map(|n|&n.family).eq(b.names.iter().take(start).map(|n|&n.family)){continue;}
            if let Some(k)=a.names.iter().zip(&b.names).position(|(x,y)|x.family!=y.family||x.given!=y.given){needed=needed.max(k+1);}
            else if a.names.len()!=b.names.len(){needed=needed.max(a.names.len().min(b.names.len())+1).min(a.names.len());}
        }
        if needed>initial{let source=e.computed["labelnamesource"].clone();e.names.get_mut(&source).unwrap().uniquelist=needed;}
    }
    for e in entries.iter_mut() {
        rehash(c,e,"global");
        if enabled(c,e,"labelalpha") {let alpha=labelalpha(c,e);e.computed.insert("labelalpha".into(),alpha);}
    }
    let global=DataList{sorting:c.options.get("sortingtemplatename").cloned().unwrap_or_else(||"nyt".into()),..DataList::default()};
    let order=sort(c,&global,entries); contextualize(c,&global,entries,&order);
}
fn substring(s: &str,width: usize,right: bool) -> String {let g:Vec<&str>=s.graphemes(true).collect();let start=if right{g.len().saturating_sub(width)}else{0};g[start..(start+width).min(g.len())].concat()}
fn alpha_field(c: &Control,e: &Entry,f: &LabelField) -> String {
    if f.literal {return f.name.clone();}
    let key=if f.name=="labelname" {e.computed.get("labelnamesource").map(String::as_str).unwrap_or("")}else{&f.name};
    if let Some(n)=e.names.get(key) {let max=number(c,e,"maxalphanames",3);let count=if n.names.len()>max{number(c,e,"minalphanames",1)}else{n.names.len()};if f.ifnames.is_some_and(|i|i!=count){return String::new();}let count=f.names.unwrap_or(count).min(n.names.len());let width=f.strwidth.unwrap_or(if count==1{3}else{1});let mut s=String::new();for name in n.names.iter().take(count) {if enabled(c,e,"useprefix")&&!name.prefix.is_empty(){s.push_str(&substring(&norm(&name.prefix),1,false));}s.push_str(&substring(&norm(&name.family),width,f.strside=="right"));}if n.others || count<n.names.len(){s.push_str(if option(c,e,"alphaothers").is_empty(){"+"}else{option(c,e,"alphaothers")});}return s;}
    let v=match f.name.as_str(){"citekey"|"entrykey"=>e.key.as_str(),_=>e.computed.get(&f.name).or_else(||e.fields.get(&f.name)).map(String::as_str).unwrap_or("")};let s=norm(v);f.strwidth.map(|w|substring(&s,w,f.strside=="right")).unwrap_or(s)
}
fn labelalpha(c: &Control,e: &Entry)->String {if c.labelalpha.is_empty(){let source=e.computed.get("labelnamesource").and_then(|f|e.names.get(f));let mut a=source.map(|n|n.names.iter().take(3).map(|x|substring(&norm(&x.family),if n.names.len()==1{3}else{1},false)).collect::<String>()).unwrap_or_default();a.push_str(&substring(e.fields.get("year").map(String::as_str).unwrap_or(""),2,true));return a;}let mut s=String::new();for p in &c.labelalpha {for f in &p.fields {let v=alpha_field(c,e,f);if !v.is_empty(){s.push_str(&v);if f.final_ {return s;}break;}}}s}

fn sort_name(c: &Control,e: &Entry,n: &NameList,namekey: &str)->String {
    let mut s=String::new();
    let name=n.options.get("sortingnamekeytemplatename").map(String::as_str).unwrap_or_else(||{let local=option(c,e,"sortingnamekeytemplatename");if !local.is_empty(){local}else if namekey.is_empty(){"global"}else{namekey}});
    let template=c.name_templates.get("sortingnamekeytemplate").and_then(|t|t.get(name));
    for name in n.names.iter().take(visible(c,e,n,"sort")) {
        if let Some(groups)=template {
            for group in groups {
                for p in group {
                    if let Some(use_)=p.attributes.get("use") {
                        let enabled_=if p.name=="prefix"{enabled(c,e,"useprefix")}else{enabled(c,e,&format!("use{}",p.name))};
                        if enabled_!=(use_=="1"){continue;}
                    }
                    if p.attributes.get("type").is_some_and(|s|s=="literal") {s.push_str(&p.name);continue;}
                    let value=if p.attributes.get("inits").is_some_and(|s|s=="1"){name.initial_tokens.get(&p.name).map(|v|v.join("")).unwrap_or_default()}else{norm(part(name,&p.name))};
                    if value.is_empty(){continue;}
                    s.push_str(&value);
                    let width=e.computed.get(&format!("sortwidth-{}",p.name)).and_then(|v|v.parse::<usize>().ok()).unwrap_or(value.graphemes(true).count());
                    for _ in value.graphemes(true).count()..width {s.push(' ');}
                }
            }
        } else {
            if enabled(c,e,"useprefix")&&!name.prefix.is_empty(){s.push_str(&norm(&name.prefix));s.push(' ');}
            s.push_str(&norm(&name.family));s.push(' ');s.push_str(&norm(&name.given));s.push(' ');s.push_str(&norm(&name.suffix));s.push(' ');
            if !enabled(c,e,"useprefix"){s.push_str(&norm(&name.prefix));}
        }
    }
    if !enabled(c,e,"nosortothers")&&visible(c,e,n,"sort")<n.names.len(){s.push('\u{10fffd}');}s
}
fn sort_value(c: &Control,e: &Entry,f: &str,index: usize,namekey: &str)->String {match f {"presort"=>return e.fields.get(f).cloned().unwrap_or_else(||{let p=option(c,e,f);if p.is_empty(){"mm".into()}else{p.into()}}),"citeorder"|"intciteorder"=>return e.computed.get(f).cloned().unwrap_or_else(||(index+1).to_string()),"citecount"=>return e.cite_count.max(0).to_string(),"entrykey"|"citekey"=>return e.key.clone(),"entrytype"=>return e.kind.clone(),_=>{}}
    let f=if f=="labelname"{e.computed.get("labelnamesource").map(String::as_str).unwrap_or("")}else{f};
    if let Some(n)=e.names.get(f) {if matches!(option(c,e,&format!("use{f}")),"0"|"false"){return String::new();}return sort_name(c,e,n,namekey);}
    if let Some(l)=e.lists.get(f){let max=number(c,e,"maxitems",3);let take=if l.len()>max{number(c,e,"minitems",1)}else{l.len()};let mut s=l.iter().take(take).map(|s|norm(s)).collect::<Vec<_>>().join("!");if take<l.len(){s.push('\u{10fffd}');}return s;}
    let v=e.computed.get(f).or_else(||e.fields.get(f)).map(String::as_str).unwrap_or("");norm(v)
}
fn transform(v: String,s: &SortItem)->String {let mut v=if let Some(w)=s.substring_width{substring(&v,w,s.substring_side.as_deref()==Some("right"))}else{v};if let Some(w)=s.pad_width{let n=w.saturating_sub(v.graphemes(true).count());let p=s.pad_char.as_deref().unwrap_or("0").repeat(n);if s.pad_side.as_deref()==Some("right"){v.push_str(&p)}else{v=p+&v}}v}
pub(crate) fn sort_values(c: &Control,d: &DataList,e: &Entry,index: usize)->Vec<String> {
    if let Some(template)=c.sorting.get(&d.sorting) {
        let mut final_value:Option<String>=None;
        let mut out=Vec::with_capacity(template.len());
        for s in template {
            if let Some(v)=&final_value {out.push(v.clone());continue;}
            let v=s.fields.iter().map(|f|sort_value(c,e,f,index,&d.namekey)).find(|x|!x.is_empty()).or_else(||s.literal.clone()).unwrap_or_default();
            let v=transform(v,s);
            if s.final_&&!v.is_empty(){final_value=Some(v);out.push(String::new());}else{out.push(v)}
        }
        out
    } else if d.sorting=="none" {vec![(index+1).to_string()]}
    else {vec!["mm".into(),String::new(),["sortname","author","editor","translator","sorttitle","title"].iter().map(|f|sort_value(c,e,f,index,&d.namekey)).find(|x|!x.is_empty()).unwrap_or_default(),sort_value(c,e,"year",index,&d.namekey),sort_value(c,e,"title",index,&d.namekey)]}
}
pub fn sort(c: &Control,d: &DataList,entries: &[Entry])->Vec<usize> {
    let values:Vec<Vec<String>>=entries.iter().enumerate().map(|(i,e)|{
        let source=if e.kind=="set"{e.fields.get("entryset").and_then(|s|s.split(',').next()).and_then(|key|entries.iter().find(|e|e.key==key)).unwrap_or(e)}else{e};
        sort_values(c,d,source,i)
    }).collect();
    let keys:Vec<Vec<Vec<u16>>>=values.iter().zip(entries).map(|(v,e)|v.iter().map(|s|collation_key(s,enabled(c,e,"sortcase"))).collect()).collect();
    let numeric:Vec<bool>=c.sorting.get(&d.sorting).map(|t|t.iter().map(|s|s.fields.first().is_some_and(|f|matches!(f.as_str(),"citeorder"|"intciteorder"|"citecount"|"labelyear"|"labelmonth"|"labelday")||c.fields.get(f).is_some_and(|s|matches!(s.datatype.as_str(),"integer"|"datepart")))).collect()).unwrap_or_else(||if d.sorting=="none"{vec![true]}else{vec![false,false,false,true,false]});
    let nums:Vec<Vec<Option<i64>>>=values.iter().map(|v|v.iter().enumerate().map(|(i,s)|if numeric.get(i).copied().unwrap_or(false){s.parse().ok()}else{None}).collect()).collect();
    let mut order:Vec<usize>=(0..entries.len()).collect();
    order.sort_by(|&a,&b| {
        for (i,(x,y)) in keys[a].iter().zip(&keys[b]).enumerate() {
            let cmp=match (nums[a][i],nums[b][i]){(Some(x),Some(y))=>x.cmp(&y),_=>x.cmp(y)};
            if !cmp.is_eq(){return if c.sorting.get(&d.sorting).and_then(|t|t.get(i)).is_some_and(|s|s.descending){cmp.reverse()}else{cmp};}
        }
        a.cmp(&b)
    });
    // Set members immediately follow their set parent, in the declared order.
    let mut result=Vec::with_capacity(order.len());
    for i in order.iter().copied() {
        if entries[i].computed.contains_key("inset"){continue;}
        result.push(i);
        if entries[i].kind=="set" {
            if let Some(members)=entries[i].fields.get("entryset") {for key in members.split(','){if let Some(j)=entries.iter().position(|e|e.key==key){result.push(j);}}}
        }
    }
    for i in order{if !result.contains(&i){result.push(i);}}
    result
}
pub(crate) fn initial(c: &Control,d: &DataList,e: &Entry,index: usize)->(String,String) {let values=sort_values(c,d,e,index);let raw=values.iter().skip(if c.sorting.get(&d.sorting).is_some_and(|s|s.first().is_some_and(|s|s.fields.iter().any(|f|f=="presort"))){1}else{0}).cloned().collect::<Vec<_>>().join(",");let s=norm(&raw);let s=s.chars().filter(|c|c.is_alphanumeric()||c.is_whitespace()).collect::<String>();let s=s.trim().graphemes(true).next().unwrap_or("").to_string();let weights=uca(&s);let mut view=String::from("[");let mut first=true;for w in weights {if w[0]!=0{if !first{view.push(' ');}first=false;let _=write!(view,"{:04X}",w[0]);}}view.push_str(" | | |]");let hash=digest(&view);(s,hash)}
fn extra_date(c: &Control,e: &Entry)->(String,Option<String>) {
    let fallback=["labelname","labeltitle"];
    let fields:Vec<&str>=if c.extradate_context.is_empty(){fallback.to_vec()}else{c.extradate_context.iter().map(String::as_str).collect()};
    let mut context=String::new();
    for f in fields {
        let f=if f=="labelname"{e.computed.get("labelnamesource").map(String::as_str).unwrap_or("")}else if f=="labeltitle"{e.computed.get("labeltitlesource").map(String::as_str).unwrap_or("")}else{f};
        if let Some(n)=e.names.get(f){context=uniqueness_key(c,e,n);}
        else if let Some(l)=e.lists.get(f){context=digest(&hash_normalize(&l.join("")));}
        else if let Some(v)=e.fields.get(f).or_else(||e.computed.get(f)){context=digest(&hash_normalize(v));}
        if !context.is_empty(){break;}
    }
    let default=vec![vec!["labelyear".to_string(),"year".to_string()]];
    let spec=if c.extradate_spec.is_empty(){&default}else{&c.extradate_spec};
    let mut date=String::new();let mut scope=None;
    for group in spec{for f in group{if let Some(v)=e.computed.get(f).or_else(||e.fields.get(f)){date.push_str(v);scope=Some(f.clone());break;}}}
    (if context.is_empty(){String::new()}else{format!("{context},{date}")},scope)
}
pub(crate) fn contextualize(c: &Control,d: &DataList,entries: &mut [Entry],order: &[usize]) {
    let context_hash=d.name.rsplit('/').next().filter(|s|!s.is_empty()).unwrap_or("global");
    if context_hash!="global" {for e in entries.iter_mut(){rehash(c,e,context_hash);}}
    let initials:Vec<_>=entries.iter().enumerate().map(|(i,e)| {
        let source=if e.kind=="set"{e.fields.get("entryset").and_then(|s|s.split(',').next()).and_then(|key|entries.iter().find(|e|e.key==key)).unwrap_or(e)}else{e};
        initial(c,d,source,i)
    }).collect();
    let mut counts:BTreeMap<(&str,String),usize>=BTreeMap::new();
    let mut all:Vec<Vec<(&str,String)>>=Vec::with_capacity(entries.len());
    for (i,e) in entries.iter_mut().enumerate() {
        let (s,h)=&initials[i];e.computed.insert("sortinit".into(),s.clone());e.computed.insert("sortinithash".into(),h.clone());
        for k in ["extraname","extradate","extraalpha","extratitle","extratitleyear","extradatescope"]{e.computed.remove(k);}
        for k in ["singletitle","uniquetitle","uniquebaretitle","uniquework","uniqueprimaryauthor"]{e.flags.remove(k);}
        let mut keys=Vec::new();
        if e.kind=="set"||e.kind=="missing"||enabled(c,e,"skiplab"){all.push(keys);continue;}
        let n=e.computed.get("nameuniqueness").cloned().unwrap_or_default();
        let full=e.computed.get("fullhash").cloned().unwrap_or_default();
        let title=e.computed.get("labeltitle").cloned().unwrap_or_default();
        if !n.is_empty(){keys.push(("extraname",n));}
        if enabled(c,e,"labeldateparts") {let (key,scope)=extra_date(c,e);if let Some(scope)=scope{e.computed.insert("extradatescope".into(),scope);}if !key.is_empty(){keys.push(("extradate",key));}}
        if enabled(c,e,"labelalpha"){if let Some(a)=e.computed.get("labelalpha"){keys.push(("extraalpha",a.clone()));}}
        if enabled(c,e,"labeltitle")&&!title.is_empty(){keys.push(("extratitle",format!("{full},{title}")));}
        if enabled(c,e,"labeltitleyear")&&!title.is_empty(){keys.push(("extratitleyear",format!("{title},{}",e.computed.get("labelyear").or_else(||e.fields.get("year")).map(String::as_str).unwrap_or(""))));}
        if enabled(c,e,"singletitle")&&!full.is_empty(){keys.push(("singletitle",full.clone()));}
        if enabled(c,e,"uniquetitle")&&!title.is_empty(){keys.push(("uniquetitle",title.clone()));}
        if enabled(c,e,"uniquebaretitle")&&full.is_empty()&&!title.is_empty(){keys.push(("uniquebaretitle",title.clone()));}
        if enabled(c,e,"uniquework")&&!full.is_empty()&&!title.is_empty(){keys.push(("uniquework",format!("{full}{title}")));}
        if enabled(c,e,"uniqueprimaryauthor"){if let Some(name)=e.computed.get("labelnamesource").and_then(|f|e.names.get(f)).and_then(|l|l.names.first()){keys.push(("uniqueprimaryauthor",name.family.clone()));}}
        for k in &keys{*counts.entry(k.clone()).or_default()+=1;}all.push(keys);
    }
    let mut seen:BTreeMap<(&str,String),usize>=BTreeMap::new();
    for &i in order {for (field,key) in &all[i] {
        let count=counts.get(&(*field,key.clone())).copied().unwrap_or(0);
        if matches!(*field,"singletitle"|"uniquetitle"|"uniquebaretitle"|"uniquework"|"uniqueprimaryauthor") {
            if count==1{entries[i].flags.insert((*field).into());}
        } else if count>1 {let value=seen.entry((*field,key.clone())).or_default();*value+=1;entries[i].computed.insert((*field).into(),value.to_string());}
    }}
}

fn collation_key(s: &str,case: bool)->Vec<u16> {let w=uca(s);let mut key=Vec::with_capacity(w.len()*3+3);for level in 0..if case{3}else{2}{key.extend(w.iter().map(|x|x[level]).filter(|&x|x!=0));key.push(0);}key}
fn uca(s: &str)->Vec<[u16;3]> {let chars:Vec<u32>=s.nfd().map(|c|c as u32).collect();let mut weights=Vec::with_capacity(chars.len());let mut i=0;while i<chars.len(){let mut found=None;for len in (1..=3.min(chars.len()-i)).rev(){let mut key=0u64;for &c in &chars[i..i+len]{key=(key<<21)|(u64::from(c)+1);}if let Ok(k)=UCA_KEYS.binary_search_by_key(&key,|x|x.0){found=Some((len,UCA_KEYS[k]));break;}}if let Some((len,(_,off,n)))=found{weights.extend_from_slice(&UCA_WEIGHTS[off as usize..off as usize+n as usize]);i+=len;}else{let c=chars[i];let lead=if (0x4e00..=0x9fff).contains(&c)||(0xf900..=0xfaff).contains(&c){0xfb40}else if (0x3400..=0x4dbf).contains(&c)||(0x20000..=0x2ffff).contains(&c){0xfb80}else{0xfbc0};weights.push([(lead+(c>>15))as u16,0x20,2]);weights.push([((c&0x7fff)|0x8000)as u16,0,0]);i+=1;}}weights}

// Unicode DUCET 13.0.0, copyright Unicode, Inc.; Unicode data-file license.
// Static collation elements match Unicode::Collate shipped with Biber 2.22.
include!("uca_table.rs");

