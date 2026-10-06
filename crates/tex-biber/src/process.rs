use crate::model::{Control, DataList, Entry, Name, NameList, SortElement, SortItem};
use regex::Regex;
use std::collections::{BTreeMap,BTreeSet};
use std::fmt::Write;
use std::sync::LazyLock;

pub(crate) fn digest(value: &str) -> String { crate::md5_hex(value.as_bytes()) }
pub(crate) fn enabled(c: &Control, e: &Entry, key: &str) -> bool { matches!(option(c,e,key), "1" | "true" | "yes") }
pub(crate) fn option<'a>(c: &'a Control, e: &'a Entry, key: &str) -> &'a str {
    let mut value=None;
    if let Some(opts) = e.fields.get("options") {
        for p in opts.split(',') {
            let (k,v)=p.trim().split_once('=').unwrap_or((p.trim(),"true"));
            if k==key {value=Some(v);}
            else if k=="dataonly" {let enabled=matches!(v,"true"|"1"|"yes");if matches!(key,"skipbib"|"skipbiblist"|"skiplab"){value=Some(if enabled{"true"}else{"false"});}else if enabled&&matches!(key,"uniquename"|"uniquelist"){value=Some("false");}}
        }
    }
    value.unwrap_or_else(||c.option(&e.kind,key))
}
pub(crate) fn entry_option<'a>(e:&'a Entry,key:&str)->Option<&'a str> {
    e.fields.get("options").and_then(|options|options.split(',').filter_map(|p| {let (k,v)=p.trim().split_once('=').unwrap_or((p.trim(),"true"));(k==key).then_some(v)}).next_back())
}
fn number(c: &Control, e: &Entry, key: &str, fallback: usize) -> usize { option(c,e,key).parse().unwrap_or(fallback) }
pub(crate) fn hash_normalize(value: &str) -> String {
    static LETTERS:LazyLock<&'static [(u32,u32)]>=LazyLock::new(||crate::perl_unicode::property_ranges("L").expect("bundled Perl letter property"));
    let is_letter=|c:char|crate::perl_unicode::in_ranges(&LETTERS,c);
    // Perl performs the letter-macro and nonletter-macro substitutions in
    // separate passes: collapsing them changes adjacent backslash semantics.
    let letters=if value.contains('\\'){
        let mut expanded=String::with_capacity(value.len());
        let mut chars=value.chars().peekable();
        while let Some(c)=chars.next(){
            if c=='\\'&&chars.peek().copied().is_some_and(is_letter){
                while chars.peek().copied().is_some_and(is_letter){expanded.push(chars.next().unwrap());}
                expanded.push(':');
                while chars.peek().copied().is_some_and(crate::perl_unicode::is_space){chars.next();}
            }else{expanded.push(c);}
        }
        std::borrow::Cow::Owned(expanded)
    }else{std::borrow::Cow::Borrowed(value)};
    let mut stripped=String::with_capacity(letters.len());
    let mut chars=letters.chars().peekable();
    while let Some(c)=chars.next(){
        if c=='\\'&&chars.peek().copied().is_some_and(|next|!is_letter(next)){
            write!(stripped,"{}:",chars.next().unwrap() as u32).unwrap();
            while chars.peek().copied().is_some_and(crate::perl_unicode::is_space){chars.next();}
        }else if !matches!(c,'{'|'}'|'~'|'.')&&!crate::perl_unicode::is_space(c){stripped.push(c);}
    }
    match crate::collation::normalization::normalize(&stripped,"NFC"){
        std::borrow::Cow::Borrowed(_)=>stripped,
        std::borrow::Cow::Owned(normalized)=>normalized,
    }
}
fn norm(value: &str) -> String {
    static MACRO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\[A-Za-z]+").unwrap());
    static TIES: LazyLock<Regex> = LazyLock::new(||Regex::new(r"([^\\])~").unwrap());
    let tied=TIES.replace_all(value,"$1 ");
    let s=MACRO.replace_all(&tied, "");
    s.replace(['{','}'],"").split(crate::perl_unicode::is_space).filter(|part|!part.is_empty()).collect::<Vec<_>>().join(" ")
}
fn part<'a>(n: &'a Name, p: &str) -> &'a str {n.part(p)}
fn nameparts(c:&Control)->impl Iterator<Item=&str> {
    ["family","given","prefix","suffix"].into_iter().take(if c.nameparts.is_empty(){4}else{0}).chain(c.nameparts.iter().map(String::as_str))
}
fn hash_part<'a>(c:&Control,n:&'a Name,p:&str,template:&str)->std::borrow::Cow<'a,str> {
    let Some(parts)=c.name_templates.get("namehashtemplate").and_then(|t|t.get(template))else{return std::borrow::Cow::Borrowed(part(n,p))};
    let Some(spec)=parts.iter().flatten().find(|s|s.name==p)else{return std::borrow::Cow::Borrowed("")};
    match spec.attributes.get("hashscope").map(String::as_str).unwrap_or("full"){
        "full"=>std::borrow::Cow::Borrowed(part(n,p)),
        "init"=>std::borrow::Cow::Owned(n.initial_tokens.get(p).map(|tokens|tokens.concat()).unwrap_or_default()),
        _=>std::borrow::Cow::Borrowed(""),
    }
}
fn hash_name(c: &Control,n: &Name,template: &str)->String {
    if !n.hashid.is_empty() {return n.hashid.clone();}
    let template=n.options.get("namehashtemplatename").map(String::as_str).unwrap_or(template);
    let mut out=String::new();
    for p in nameparts(c){out.push_str(&hash_part(c,n,p,template));}
    out
}
fn name_hash(c: &Control,n: &Name,template: &str) -> String {digest(&hash_normalize(&hash_name(c,n,template)))}
fn list_hash(c: &Control,n: &NameList,visible: usize,raw: bool,noothers: bool,template: &str,strip_field:Option<&str>) -> String {
    let mut s=String::new();
    let mut template=template;
    let rules=strip_field.map(|field|{
        let mut rules=vec![format!("nonamestring.{field}")];
        for (key,fields) in c.options.iter().filter(|(key,_)|key.starts_with("datafieldset.")){
            if fields.split(',').any(|candidate|candidate==field){rules.push(format!("nonamestring.{}",&key["datafieldset.".len()..]));}
        }
        rules
    }).unwrap_or_default();
    for name in n.names.iter().take(visible) {
        if !raw&&!name.hashid.is_empty(){s.push_str(&name.hashid);continue;}
        if !raw {if let Some(local)=name.options.get("namehashtemplatename"){template=local;}}
        for p in nameparts(c){
            let mut value=if raw{std::borrow::Cow::Borrowed(part(name,p))}else{hash_part(c,name,p,template)};
            for rule in &rules {
                if let std::borrow::Cow::Owned(stripped)=crate::names::strip_rules(c,rule,&value){value=std::borrow::Cow::Owned(stripped);}
            }
            s.push_str(&value);
        }
    }
    if !noothers && (n.others || visible<n.names.len()) {s.push('+');} digest(&hash_normalize(&s))
}
const VISIBILITY_SCOPES:[&str;4]=["cite","bib","sort","alpha"];
/// One entry's process_visible_names result for a list: past max*names, visibility is the
/// list's uniquelist if any, else min*names; alpha ignores uniquelist, and global
/// pluralothers makes "et al" replace at least two cited names.
fn visibility(c: &Control,e: &Entry,n: &NameList)->[usize;4] {
    let count=n.names.len();
    VISIBILITY_SCOPES.map(|scope| {
        let max=number(c,e,&format!("max{scope}names"),3);
        let min=number(c,e,&format!("min{scope}names"),1).min(count);
        if count<=max {return count;}
        if scope=="alpha" {return min;}
        let v=n.uniquelist.unwrap_or(min);
        if scope=="cite"&&count.checked_sub(v)==Some(1)&&c.options.get("pluralothers").is_some_and(|s|matches!(s.as_str(),"1"|"true"|"yes")) {count} else {v}
    })
}
/// DataList visibility keyed by Names object: inherited/cloned lists take the value set by the
/// last citekey that holds them.
pub(crate) fn assign_visibility(c: &Control,entries: &mut [Entry]) {
    let mut shared=BTreeMap::new();
    for e in entries.iter() {for list in e.names.values() {shared.insert(list.id,visibility(c,e,list));}}
    for e in entries.iter_mut() {for list in e.names.values_mut() {list.visible=shared.get(&list.id).copied();}}
}
pub(crate) fn visible_final(c: &Control,e: &Entry,n: &NameList,scope: &str) -> usize {
    let index=VISIBILITY_SCOPES.iter().position(|s|*s==scope).expect("visibility scope");
    n.visible.map_or_else(||visibility(c,e,n)[index],|v|v[index])
}
/// Internals::_getnamehash_u: the visible labelname parts the uniquename template keeps, with
/// non-base parts at the level of each name's uniquename result.
fn namehash_u(c: &Control,e: &Entry,list: &NameList,template: &str) -> String {
    let mut s=String::new();
    let mut untname=list.options.get("uniquenametemplatename").map(String::as_str).unwrap_or_else(||entry_option(e,"uniquenametemplatename").unwrap_or(template));
    let visible=visible_final(c,e,list,"cite");
    for name in list.names.iter().take(visible) {
        if let Some(value)=name.options.get("uniquenametemplatename"){untname=value;}
        let Some(schema)=c.name_templates.get("uniquenametemplate").and_then(|t|t.get(untname)) else {continue};
        for p in schema.iter().flatten() {
            if p.attributes.get("disambiguation").is_some_and(|s|s=="none"){continue;}
            let value=name.part(&p.name);if !perl_true(value){continue;}
            if p.attributes.get("base").is_some_and(|s|matches!(s.as_str(),"1"|"true")){s.push_str(value);continue;}
            match name.uniquename.as_ref().and_then(|u|u.level) {
                Some(crate::model::UniqueLevel::Init)=>{for token in name.initial_tokens.get(&p.name).into_iter().flatten(){s.push_str(token);}}
                Some(_)=>s.push_str(value),
                None=>{}
            }
        }
    }
    let noothers=list.options.get("nohashothers").map(|s|matches!(s.as_str(),"1"|"true")).unwrap_or_else(||enabled(c,e,"nohashothers"));
    if !noothers&&(list.others||visible<list.names.len()){s.push('+');}
    digest(&hash_normalize(&s))
}
fn rehash(c: &Control,e: &mut Entry,context_template: &str,unique_template: &str) {
    let fields:Vec<String>=e.names.keys().cloned().collect();
    for f in fields {
        let list=&e.names[&f];
        let entry=entry_option(e,"namehashtemplatename").unwrap_or(if context_template.is_empty(){"global"}else{context_template});
        let template=list.options.get("namehashtemplatename").map(String::as_str).unwrap_or(entry).to_string();
        let nh=list.options.get("nohashothers").map(|s|matches!(s.as_str(),"1"|"true")).unwrap_or_else(||enabled(c,e,"nohashothers"));
        let hash=list_hash(c,list,visible_final(c,e,list,"cite"),false,nh,&template,None);
        let full=list_hash(c,list,list.names.len(),false,false,&template,Some(&f));
        let raw=list_hash(c,list,list.names.len(),true,false,&template,Some(&f));
        let bib=list_hash(c,list,visible_final(c,e,list,"bib"),false,nh,&template,None);
        for (suffix,value) in [("namehash",&hash),("fullhash",&full),("fullhashraw",&raw),("bibnamehash",&bib)]{e.computed.insert(format!("{f}{suffix}"),value.clone());}
        let list=e.names.get_mut(&f).unwrap();list.hash=hash;list.fullhash=full;list.bibnamehash=bib;
        for name in &mut list.names{name.hash=name_hash(c,name,&template);}
    }
    if let Some(f)=e.computed.get("labelnamesource").cloned() {
        for suffix in ["namehash","bibnamehash"]{if let Some(v)=e.computed.get(&format!("{f}{suffix}")).cloned(){e.computed.insert(suffix.into(),v);}}
        let key=namehash_u(c,e,&e.names[&f],unique_template);e.computed.insert("nameuniqueness".into(),key);
    }
    if let Some(f)=e.computed.get("labelnamefullsource").cloned(){for suffix in ["fullhash","fullhashraw"]{if let Some(v)=e.computed.get(&format!("{f}{suffix}")).cloned(){e.computed.insert(suffix.into(),v);}}}
}

pub fn prepare(c: &Control, entries: &mut [Entry]) -> Result<(),String> {
    // Section::set_np_length: the longest NFD part string, and joined initials, per namepart.
    let mut widths=BTreeMap::<String,usize>::new();
    let nfd_length=|value:&str|crate::collation::normalization::normalize(value,"NFD").chars().count();
    for name in entries.iter().flat_map(|e|e.names.values()).flat_map(|l|&l.names) {
        for p in nameparts(c) {
            let value=part(name,p);if value.is_empty()||value=="0"{continue;}
            let width=widths.entry(format!("sortwidth-{p}")).or_default();*width=(*width).max(nfd_length(value));
            if let Some(tokens)=name.initial_tokens.get(p){let width=widths.entry(format!("sortwidth-{p}-i")).or_default();*width=(*width).max(nfd_length(&tokens.concat()));}
        }
    }
    for e in entries.iter_mut() {for (key,width) in &widths {e.computed.insert(key.clone(),width.to_string());}}
    for e in entries.iter_mut() {
        let candidates=c.label_spec(&e.kind,"labelnamespec",&c.labelname);
        for field in &candidates {let k=field.strip_prefix("short").unwrap_or(field); if option(c,e,&format!("use{k}"))=="0" || option(c,e,&format!("use{k}"))=="false" {continue;} if e.names.contains_key(*field) {e.computed.insert("labelnamesource".into(),field.to_string());break;}}
        for field in &candidates { if field.starts_with("short") {continue;} let k=format!("use{field}"); if matches!(option(c,e,&k),"0"|"false") {continue;} if e.names.contains_key(*field) {e.computed.insert("labelnamefullsource".into(),field.to_string());break;} }
        for f in c.label_spec(&e.kind,"labeltitlespec",&c.labeltitle) {if let Some(v)=e.fields.get(f).filter(|v| perl_true(v)) {e.computed.insert("labeltitlesource".into(),f.into()); e.computed.insert("labeltitle".into(),v.clone());break;}}
        if enabled(c,e,"labeldateparts") {
            let specs:Vec<(&str,bool)>=match c.labeldate_specs.get(&e.kind).or_else(||c.labeldate_specs.get("global")) {Some(specs)=>specs.iter().map(|s|(s.name.as_str(),s.literal)).collect(),None=>c.labeldate.iter().map(|s|(s.as_str(),s=="nodate")).collect()};
            for (f,literal) in specs {if literal {e.computed.insert("labeldatesource".into(),f.into());break;}
                let prefix=f.strip_suffix("date"); let y=prefix.map(|p|format!("{p}year")).unwrap_or_else(||f.into());
                if !e.fields.contains_key(&y) && !prefix.is_some_and(|p|e.fields.contains_key(&format!("{p}endyear"))) {continue;}
                e.computed.insert("labeldatesource".into(),prefix.unwrap_or(f).into());
                // process_labeldate: labelyear is the (possibly empty) year; end parts append when they differ.
                let ytype=y.strip_suffix("year").unwrap_or("");
                if let Some(v)=e.fields.get(&y).cloned(){e.computed.insert("labelyear".into(),v);}
                if let Some(end)=e.fields.get(&format!("{ytype}endyear")).cloned() {
                    if e.fields.get(&y).map_or("",String::as_str)!=end {let mut v=e.computed.get("labelyear").cloned().unwrap_or_default();v.push_str("\\bibdatedash ");v.push_str(&end);e.computed.insert("labelyear".into(),v);}
                }
                if let Some(prefix)=prefix {
                    for p in ["month","day","hour","minute","second","timezone"] {if let Some(v)=e.fields.get(&format!("{prefix}{p}")).cloned(){e.computed.insert(format!("label{p}"),v);}}
                    for p in ["month","day","hour","minute","second"] {
                        let Some(end)=e.fields.get(&format!("{ytype}end{p}")).filter(|v|perl_true(v)).cloned() else {continue};
                        let start=e.fields.get(&format!("{prefix}{p}")).cloned().unwrap_or_default();
                        if start!=end {e.computed.insert(format!("label{p}"),format!("{start}\\bibdatedash {end}"));}
                    }
                }
                break;
            }
            if e.computed.contains_key("labelyear") {e.computed.insert("extradatescope".into(),"labelyear".into());}
        }
    }
    crate::uniqueness::uniqueness(c,entries,"global")?;assign_visibility(c,entries);
    for e in entries.iter_mut(){rehash(c,e,"global","global");}
    crate::labels::prepare(c,entries,"global");
    let global=DataList{sorting:c.options.get("sortingtemplatename").cloned().unwrap_or_else(||"nyt".into()),..DataList::default()};
    let order=sort(c,&global,entries); contextualize(c,&global,entries,&order)
}
fn sort_rules<'a>(c:&Control,value:&'a str,field:&str)->std::borrow::Cow<'a,str> {
    let mut value=std::borrow::Cow::Borrowed(value);
    for key in c.options.keys().filter(|k|k.starts_with("nosort.")) {
        let target=key.trim_start_matches("nosort.");
        if target.eq_ignore_ascii_case(field)||c.options.get(&format!("datafieldset.{target}")).is_some_and(|set|set.split(',').any(|f|f.eq_ignore_ascii_case(field))) {
            let stripped=crate::names::strip_rules(c,key,&value);
            if let std::borrow::Cow::Owned(stripped)=stripped{value=std::borrow::Cow::Owned(stripped);}
        }
    }
    value
}
fn norm_sort(c:&Control,value:&str,field:&str)->String {norm(&sort_rules(c,value,field))}
fn sort_name(c: &Control,e: &Entry,n: &NameList,namekey: &str,field:&str)->String {
    let mut s=String::new();
    let mut template_name=n.options.get("sortingnamekeytemplatename").map(String::as_str).unwrap_or_else(||{let local=option(c,e,"sortingnamekeytemplatename");if !local.is_empty(){local}else if namekey.is_empty(){"global"}else{namekey}});
    let visibility=if c.options.get(&format!("sortingnamekeytemplate.visibility.{template_name}")).is_some_and(|value|value=="cite"){"cite"}else{"sort"};
    let count=visible_final(c,e,n,visibility);
    let mut useprefix=n.options.get("useprefix").map(|value|matches!(value.as_str(),"1"|"true"|"yes")).unwrap_or_else(||enabled(c,e,"useprefix"));
    for name in n.names.iter().take(count) {
        if let Some(local)=name.options.get("sortingnamekeytemplatename"){template_name=local;}
        if let Some(local)=name.options.get("useprefix"){useprefix=matches!(local.as_str(),"1"|"true"|"yes");}
        let template=c.name_templates.get("sortingnamekeytemplate").and_then(|templates|templates.get(template_name));
        if let Some(groups)=template {
            for group in groups {
                for p in group {
                    if let Some(use_)=p.attributes.get("use") {
                        let enabled_=if p.name=="prefix"{useprefix}else{enabled(c,e,&format!("use{}",p.name))};
                        if enabled_!=(use_=="1"){continue;}
                    }
                    if p.attributes.get("type").is_some_and(|s|s=="literal") {s.push_str(&p.name);continue;}
                    // Internals::_namestring: a true namepart is padded to the section's
                    // NFD length with sprintf over its NFC form, even if it normalises away.
                    let raw=part(name,&p.name);if raw.is_empty()||raw=="0"{continue;}
                    let inits=p.attributes.get("inits").is_some_and(|value|value=="1");
                    let value=if inits{norm_sort(c,&name.initial_tokens.get(&p.name).map(|tokens|tokens.concat()).unwrap_or_default(),field)}else{norm_sort(c,raw,field)};
                    s.push_str(&value);
                    let width=e.computed.get(&format!("sortwidth-{}{}",p.name,if inits{"-i"}else{""})).and_then(|v|v.parse::<usize>().ok()).unwrap_or(0);
                    for _ in crate::collation::normalization::normalize(&value,"NFC").chars().count()..width {s.push(' ');}
                }
            }
        } else {
            if useprefix&&!name.prefix.is_empty(){s.push_str(&norm_sort(c,&name.prefix,field));s.push(' ');}
            s.push_str(&norm_sort(c,&name.family,field));s.push(' ');s.push_str(&norm_sort(c,&name.given,field));s.push(' ');s.push_str(&norm_sort(c,&name.suffix,field));s.push(' ');
            if !useprefix{s.push_str(&norm_sort(c,&name.prefix,field));}
        }
    }
    let nosortothers=n.options.get("nosortothers").map(|value|matches!(value.as_str(),"1"|"true"|"yes")).unwrap_or_else(||enabled(c,e,"nosortothers"));
    if !nosortothers&&count<n.names.len(){s.push('\u{10fffd}');}s
}
fn sort_allowed(c:&Control,kind:&str,field:&str)->bool {
    let contains=|kind:&str,key:&str|c.type_options.get(kind).and_then(|options|options.get(key)).is_some_and(|fields|fields.split(',').any(|candidate|candidate==field));
    !contains(kind,"sortexclusion")&&(!contains("*","sortexclusion")||contains(kind,"sortinclusion"))
}
/// Internals::_process_sort_attributes turns a real zero into this marker.
const BIBERZERO:&str="BIBERZERO";
fn perl_true(value:&str)->bool {!value.is_empty()&&value!="0"}
/// Perl numification of an attribute string (leading integer, else 0).
fn perl_int(value:&str)->isize {let value=value.trim_start();let digits=value.strip_prefix(['+','-']).unwrap_or(value);let end=digits.find(|c:char|!c.is_ascii_digit()).unwrap_or(digits.len());let n=digits[..end].parse::<isize>().unwrap_or(0);if value.starts_with('-'){-n}else{n}}
/// Internals::_process_sort_attributes: substring then padding, both in GCString clusters.
fn sort_attributes(value:String,el:&SortElement)->String {
    if value=="0"{return BIBERZERO.into();}
    if value.is_empty(){return value;}
    fn attr(v:&Option<String>)->Option<&str>{v.as_deref().filter(|v|perl_true(v))}
    let mut value=value;
    if attr(&el.substring_width).is_some()||attr(&el.substring_side).is_some() {
        let width=attr(&el.substring_width).map_or(4,perl_int);
        let offset=if attr(&el.substring_side).unwrap_or("left")=="right"{-width}else{0};
        value=crate::gcstring::substr(&value,offset,width).to_owned();
    }
    if attr(&el.pad_side).is_some()||attr(&el.pad_width).is_some()||attr(&el.pad_char).is_some() {
        let width=attr(&el.pad_width).map_or(4,perl_int);let side=attr(&el.pad_side).unwrap_or("left");let pad=attr(&el.pad_char).unwrap_or("0");
        let length=width-crate::gcstring::length(&value) as isize;
        if length>0 {let padding=pad.repeat(length as usize);if side=="left"{value=padding+&value;}else if side=="right"{value.push_str(&padding);}}
    }
    value
}
/// Internals::_dispatch_sorting for one sort element; `""` is Perl's false result.
fn sort_value(c: &Control,e: &Entry,f: &str,el: &SortElement,namekey: &str)->String {
    if !sort_allowed(c,&e.kind,f){return String::new();}
    if el.literal {return sort_attributes(f.to_owned(),el);}
    let translit=|field:&str,value:String|crate::transliteration::apply(c,e,field,value);
    match f {
        "presort"=>{let p=e.fields.get("presort").map(String::as_str).filter(|p|perl_true(p)).unwrap_or_else(||option(c,e,f));return sort_attributes(if p.is_empty(){"mm".into()}else{p.into()},el);}
        "citeorder"|"intciteorder"=>return e.computed.get(f).cloned().unwrap_or_default(),
        "citecount"=>return if e.cite_count<0{String::new()}else{e.cite_count.to_string()},
        "entrykey"=>return sort_attributes(e.key.clone(),el),
        "entrytype"=>return sort_attributes(e.kind.clone(),el),
        "labelalpha"=>return sort_attributes(e.computed.get("sortlabelalpha").cloned().unwrap_or_default(),el),
        "labelname"|"labeltitle"=>{
            let source=e.computed.get(if f=="labelname"{"labelnamesource"}else{"labeltitlesource"}).map(String::as_str).unwrap_or("");
            return if source.is_empty(){String::new()}else{sort_value(c,e,source,el,namekey)};
        }
        "labelyear"|"labelmonth"|"labelday"=>{
            let Some(source)=e.computed.get("labeldatesource").filter(|source|source.as_str()!="nodate")else{return String::new()};
            let component=&f["label".len()..];
            if source.ends_with("year"){
                if component!="year"{return String::new();}
                return sort_value(c,e,source,el,namekey);
            }
            return sort_value(c,e,&format!("{}{component}",source.strip_suffix("date").unwrap_or(source)),el,namekey);
        }
        "editoratype"|"editorbtype"|"editorctype"=>return match e.fields.get(f).filter(|v|perl_true(v)) {Some(v) if enabled(c,e,"useeditor")=>translit(f,sort_attributes(v.clone(),el)),_=>String::new()},
        _=>{}
    }
    let Some(spec)=c.fields.get(f) else{return String::new()};
    if spec.datatype=="name" {
        let Some(n)=e.names.get(f) else{return String::new()};
        if matches!(option(c,e,&format!("use{f}")),"0"|"false"){return String::new();}
        return translit(f,sort_attributes(sort_name(c,e,n,namekey,f),el));
    }
    if spec.fieldtype=="list" {
        let Some(l)=e.lists.get(f) else{return String::new()};
        let verbatim=matches!(spec.datatype.as_str(),"verbatim"|"uri");
        if !verbatim&&!matches!(spec.datatype.as_str(),"literal"|"key"){return String::new();}
        let max=number(c,e,"maxitems",3);let take=if l.len()>max{number(c,e,"minitems",1)}else{l.len()};
        let mut s=l.iter().take(take).map(|value|if verbatim{sort_rules(c,value,f).into_owned()}else{norm_sort(c,value,f)}).collect::<Vec<_>>().join("!");
        let length=s.trim_end_matches(crate::perl_unicode::is_space).len();s.truncate(length);
        if take<l.len(){s.push('\u{10fffd}');}
        let s=sort_attributes(s,el);
        return if verbatim{s}else{translit(f,s)};
    }
    let v=e.fields.get(f).map(String::as_str).unwrap_or("");
    if !perl_true(v){return String::new();}
    match spec.datatype.as_str() {
        "integer"|"datepart"=>sort_attributes(crate::collation::integer_sort_value(v,enabled(c,e,"noroman")),el),
        "verbatim"|"uri"=>sort_attributes(norm_sort(c,v,f),el),
        "literal"|"key"=>translit(f,sort_attributes(norm_sort(c,v,f),el)),
        _=>String::new(),
    }
}
/// Biber::generate_sortdataschema: the named template, else the global default one.
fn sorting_template<'a>(c:&'a Control,d:&DataList)->Option<&'a Vec<SortItem>> {
    c.sorting.get(&d.sorting).or_else(||c.sorting.get(c.options.get("sortingtemplatename").map(String::as_str).unwrap_or("nyt")))
}
/// Internals::_generatesortinfo: the sort object and whether a real zero was seen.
pub(crate) fn sort_values(c: &Control,d: &DataList,e: &Entry)->(Vec<String>,bool) {
    let Some(template)=sorting_template(c,d) else{return (Vec::new(),false)};
    let mut final_value:Option<String>=None;let mut suppress=true;let mut zero=false;
    let mut out=Vec::with_capacity(template.len());
    for s in template {
        let mut value=String::new();
        for el in &s.elements {
            let v=sort_value(c,e,&el.name,el,&d.namekey);
            if perl_true(&v) {if s.final_{final_value=Some(v);}else{value=v;}break;}
        }
        if value==BIBERZERO{zero=true;value="0".into();}
        if let Some(f)=&final_value {out.push(if suppress{String::new()}else{f.clone()});suppress=false;}else{out.push(value);}
    }
    (out,zero)
}
pub fn sort(c: &Control,d: &DataList,entries: &[Entry])->Vec<usize> {
    // Tool mode never runs process_sets: sets neither inherit sort data nor group members.
    let tool=c.options.get("tool").is_some_and(|v|matches!(v.as_str(),"1"|"true"|"yes"));
    let values:Vec<Vec<String>>=entries.iter().map(|e|{
        let source=if e.kind=="set"&&!tool{e.fields.get("entryset").and_then(|s|s.split(',').next()).and_then(|key|entries.iter().find(|e|e.key==key)).unwrap_or(e)}else{e};
        sort_values(c,d,source).0
    }).collect();
    let template=sorting_template(c,d);
    let base=crate::collation::Collator::sorting(c,d,None);let mut collator=base.clone();
    // Sort::Key int keys: citeorder-like fields and integer/datepart fields, by the first sortitem name.
    let numeric:Vec<bool>=sorting_template(c,d).map(|t|t.iter().map(|s|s.elements.first().is_some_and(|el|matches!(el.name.as_str(),"citeorder"|"intciteorder"|"citecount")||c.fields.get(&el.name).is_some_and(|s|s.fieldtype=="field"&&matches!(s.datatype.as_str(),"integer"|"datepart")))).collect()).unwrap_or_default();
    let mut unique_keys=Vec::new();let mut cache=BTreeMap::new();
    let keys:Vec<Vec<usize>>=values.iter().map(|v|v.iter().enumerate().map(|(i,s)| {
        if numeric.get(i).copied().unwrap_or(false){return usize::MAX;}
        let item=template.and_then(|t|t.get(i));
        let signature=(crate::collation::Collator::signature(c,d,item),s.as_str());
        if let Some(&key)=cache.get(&signature){return key;}
        let key=unique_keys.len();unique_keys.push(collator.item_key(&base,c,d,item,s));cache.insert(signature,key);key
    }).collect()).collect();
    let nums:Vec<Vec<Option<i64>>>=values.iter().map(|v|v.iter().enumerate().map(|(i,s)|if numeric.get(i).copied().unwrap_or(false){Some(crate::collation::integer_sort_key(s))}else{None}).collect()).collect();
    let mut order:Vec<usize>=(0..entries.len()).collect();
    order.sort_by(|&a,&b| {
        for (i,(x,y)) in keys[a].iter().zip(&keys[b]).enumerate() {
            let cmp=match (nums[a][i],nums[b][i]){(Some(x),Some(y))=>x.cmp(&y),_=>unique_keys[*x].cmp(&unique_keys[*y])};
            if !cmp.is_eq(){return if template.and_then(|t|t.get(i)).is_some_and(|s|s.descending){cmp.reverse()}else{cmp};}
        }
        a.cmp(&b)
    });
    if tool{return order;}
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
/// Utils::normalise_string with Perl 5.38 Unicode classes.
fn normalise_string(value:&str)->String {
    static MACRO:LazyLock<Regex>=LazyLock::new(||Regex::new(r"\\[A-Za-z]+").unwrap());
    static TIES:LazyLock<Regex>=LazyLock::new(||Regex::new(r"([^\\])~").unwrap());
    static CLASSES:LazyLock<[&'static [(u32,u32)];3]>=LazyLock::new(||["P","S","C"].map(|name|crate::perl_unicode::property_ranges(name).expect("Perl general category")));
    if value.is_empty()||value=="0"{return String::new();}
    let tied=TIES.replace_all(value,"$1 ");let unmacroed=MACRO.replace_all(&tied,"");
    let kept=unmacroed.chars().filter(|&ch|!CLASSES.iter().any(|ranges|crate::perl_unicode::in_ranges(ranges,ch))).collect::<String>();
    kept.split(crate::perl_unicode::is_space).filter(|part|!part.is_empty()).collect::<Vec<_>>().join(" ")
}
/// Internals::_generatesortinfo sortinit: none unless the sort string is true or
/// held a real zero; the `presort` option is interpolated as a regex.
pub(crate) fn initial(c: &Control,d: &DataList,e: &Entry,collator:&crate::collation::Collator)->Result<Option<(String,String)>,String> {
    let (values,zero)=sort_values(c,d,e);let raw=values.join(",");
    if !perl_true(&raw)&&!zero{return Ok(None);}
    // Biber::process_presort makes a true presort field the entry-scope option.
    let presort=e.fields.get("presort").map(String::as_str).filter(|p|!p.is_empty()&&*p!="0").unwrap_or_else(||option(c,e,"presort"));let presort=if presort.is_empty(){"mm"}else{presort};
    let pattern=crate::perl_regex::Regex::compile(&format!(r"\A{presort},+"),false).map_err(|error|format!("Invalid presort regex '{presort}': {error}"))?;
    let found=pattern.inner.find_iter(&raw).flatten().next().map(|found|found.start()..found.end());
    let stripped=match found {Some(range)=>format!("{}{}",&raw[..range.start],&raw[range.end..]),None=>raw};
    let s=crate::gcstring::prefix(&normalise_string(&stripped),1).to_owned();
    let hash=digest(&collator.view(&s));Ok(Some((s,hash)))
}
fn extra_date(c: &Control,e: &Entry,unique_template: &str)->(String,Option<String>) {
    let fallback=["labelname","labeltitle"];
    let fields:Vec<&str>=if c.extradate_context.is_empty(){fallback.to_vec()}else{c.extradate_context.iter().map(String::as_str).collect()};
    let mut context=String::new();
    for f in fields {
        let f=if f=="labelname"{e.computed.get("labelnamesource").map(String::as_str).unwrap_or("")}else if f=="labeltitle"{e.computed.get("labeltitlesource").map(String::as_str).unwrap_or("")}else{f};
        if let Some(n)=e.names.get(f){context=namehash_u(c,e,n,unique_template);}
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
pub(crate) fn contextualize(c: &Control,d: &DataList,entries: &mut [Entry],order: &[usize]) -> Result<(),String> {
    let context_hash=if d.hash_template.is_empty(){d.name.rsplit('/').next().filter(|s|!s.is_empty()).unwrap_or("global")}else{&d.hash_template};
    let unique_template=if d.unique_template.is_empty(){"global"}else{&d.unique_template};
    if unique_template!="global" {crate::uniqueness::uniqueness(c,entries,unique_template)?;}
    assign_visibility(c,entries);
    for e in entries.iter_mut(){rehash(c,e,context_hash,unique_template);}
    crate::labels::prepare(c,entries,if d.alpha_template.is_empty(){"global"}else{&d.alpha_template});
    let initial_collator=crate::collation::Collator::initial(c,d);
    let initials=entries.iter().map(|e| {
        let source=if e.kind=="set"{e.fields.get("entryset").and_then(|s|s.split(',').next()).and_then(|key|entries.iter().find(|e|e.key==key)).unwrap_or(e)}else{e};
        initial(c,d,source,&initial_collator)
    }).collect::<Result<Vec<_>,_>>()?;
    let mut counts:BTreeMap<(&str,String),usize>=BTreeMap::new();
    let mut all:Vec<Vec<(&str,String)>>=Vec::with_capacity(entries.len());
    let mut primary:BTreeMap<String,BTreeSet<String>>=BTreeMap::new();
    for (i,e) in entries.iter_mut().enumerate() {
        e.computed.remove("sortinit");e.computed.remove("sortinithash");
        if let Some((s,h))=&initials[i]{e.computed.insert("sortinit".into(),s.clone());e.computed.insert("sortinithash".into(),h.clone());}
        for k in ["extraname","extradate","extraalpha","extratitle","extratitleyear","extradatescope"]{e.computed.remove(k);}
        for k in ["singletitle","uniquetitle","uniquebaretitle","uniquework","uniqueprimaryauthor"]{e.flags.remove(k);}
        let mut keys=Vec::new();
        if e.kind=="set"||e.kind=="missing"{all.push(keys);continue;}
        let n=e.computed.get("nameuniqueness").cloned().unwrap_or_default();
        let full=e.computed.get("fullhash").cloned().unwrap_or_default();
        let title=e.computed.get("labeltitle").cloned().unwrap_or_default();
        if !enabled(c,e,"skiplab") {
            if !n.is_empty(){keys.push(("extraname",n));}
            if enabled(c,e,"labeldateparts") {let (key,scope)=extra_date(c,e,unique_template);if let Some(scope)=scope{e.computed.insert("extradatescope".into(),scope);}if !key.is_empty(){keys.push(("extradate",key));}}
            if enabled(c,e,"labelalpha"){if let Some(a)=e.computed.get("labelalpha").filter(|a|perl_true(a)){keys.push(("extraalpha",a.clone()));}}
            if enabled(c,e,"labeltitle")&&!title.is_empty(){keys.push(("extratitle",format!("{full},{title}")));}
            if enabled(c,e,"labeltitleyear")&&!title.is_empty(){keys.push(("extratitleyear",format!("{title},{}",e.computed.get("labelyear").or_else(||e.fields.get("year")).map(String::as_str).unwrap_or(""))));}
        }
        if enabled(c,e,"singletitle")&&!full.is_empty(){keys.push(("singletitle",full.clone()));}
        if enabled(c,e,"uniquetitle")&&!title.is_empty(){keys.push(("uniquetitle",title.clone()));}
        if enabled(c,e,"uniquebaretitle")&&full.is_empty()&&!title.is_empty(){keys.push(("uniquebaretitle",title.clone()));}
        if enabled(c,e,"uniquework")&&!full.is_empty()&&!title.is_empty(){keys.push(("uniquework",format!("{full}{title}")));}
        if enabled(c,e,"uniqueprimaryauthor"){if let Some(base)=crate::uniqueness::primary_author_base(c,e,unique_template)?.filter(|base|perl_true(base)){let hash=e.names[&e.computed["labelnamesource"]].names[0].hash.clone();primary.entry(base.clone()).or_default().insert(hash);keys.push(("uniqueprimaryauthor",base));}}
        for k in &keys{*counts.entry(k.clone()).or_default()+=1;}all.push(keys);
    }
    let mut seen:BTreeMap<(&str,String),usize>=BTreeMap::new();
    for &i in order {for (field,key) in &all[i] {
        let count=if *field=="uniqueprimaryauthor"{primary.get(key).map_or(0,|s|s.len())}else{counts.get(&(*field,key.clone())).copied().unwrap_or(0)};
        if matches!(*field,"singletitle"|"uniquetitle"|"uniquebaretitle"|"uniquework"|"uniqueprimaryauthor") {
            if count==1{entries[i].flags.insert((*field).into());}
        } else if count>1 {let value=seen.entry((*field,key.clone())).or_default();*value+=1;entries[i].computed.insert((*field).into(),value.to_string());}
    }}
    Ok(())
}


