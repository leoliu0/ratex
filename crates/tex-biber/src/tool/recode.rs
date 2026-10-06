//! Biber 2.22 LaTeX::Recode output tables and ordered encoding passes.
use std::collections::{BTreeMap,BTreeSet};
use std::sync::LazyLock;
use crate::collation::normalization::normalize;
struct Tables {maps:BTreeMap<String,BTreeMap<String,String>>,raw:BTreeSet<String>}
fn tables(set:&str)->Tables {
    let doc=roxmltree::Document::parse(include_str!("recode_data.xml")).expect("bundled recode XML");
    let mut maps=BTreeMap::<String,BTreeMap<String,String>>::new();let mut raw=BTreeSet::new();
    for group in doc.root_element().children().filter(|n|n.is_element()&&n.tag_name().name()=="maps"&&n.attribute("set").unwrap_or("").split(',').any(|s|s.trim()==set)){
        let map=maps.entry(group.attribute("type").unwrap_or("").into()).or_default();
        for preferred in [false,true]{for record in group.children().filter(|n|n.is_element()){
            let from=record.children().find(|n|n.is_element()&&n.tag_name().name()=="from").unwrap();
            if preferred&&!from.has_attribute("preferred"){continue;}
            let to=record.children().find(|n|n.is_element()&&n.tag_name().name()=="to").unwrap();
            let key=normalize(to.text().unwrap_or(""),"NFD").into_owned();
            if from.has_attribute("raw"){raw.insert(key.clone());}
            map.insert(key,normalize(from.text().unwrap_or(""),"NFD").into_owned());
        }}
    }
    for excluded in doc.descendants().filter(|n|n.is_element()&&n.tag_name().name()=="encode_exclude").flat_map(|n|n.children().filter(|n|n.is_element())){
        let key=normalize(excluded.text().unwrap_or(""),"NFD");for map in maps.values_mut(){map.remove(key.as_ref());}
    }
    Tables {maps,raw}
}
static BASE:LazyLock<Tables>=LazyLock::new(||tables("base"));
static FULL:LazyLock<Tables>=LazyLock::new(||tables("full"));
fn replace(text:&str,map:&BTreeMap<String,String>,mut replacement:impl FnMut(&str,&str)->String)->String{
    let mut out=String::with_capacity(text.len());let mut p=0;
    while p<text.len(){if let Some((key,value))=map.iter().find(|(key,_)|text[p..].starts_with(key.as_str())){out.push_str(&replacement(key,value));p+=key.len();}else{let c=text[p..].chars().next().unwrap();out.push(c);p+=c.len_utf8();}}
    out
}
fn mark(c:char)->bool{crate::perl_unicode::property_contains("M",c).unwrap_or(false)}
fn letter(c:char)->bool{crate::perl_unicode::property_contains("L",c).unwrap_or(false)}
fn accent<'a>(map:&'a BTreeMap<String,String>,text:&str)->Option<(usize,&'a str)>{map.iter().find(|(key,_)|text.starts_with(key.as_str())).map(|(key,value)|(key.len(),value.as_str()))}
fn accents(text:&str,map:&BTreeMap<String,String>)->String{
    // The first three Perl substitutions accent dotless i, brace-protected
    // letter clusters, and letter clusters (greedy marks with backtracking).
    let mut out=String::with_capacity(text.len());let mut p=0;
    while p<text.len(){let c=text[p..].chars().next().unwrap();if c=='i'{if let Some((len,command))=accent(map,&text[p+1..]){out.push_str(&format!("\\{command}{{\\i}}"));p+=1+len;continue;}}out.push(c);p+=c.len_utf8();}
    let text=out;let mut out=String::with_capacity(text.len());let mut p=0;
    while p<text.len(){
        if text[p..].starts_with('{'){
            let start=p+1;let mut end=start;let mut chars=text[start..].chars();
            if chars.next().is_some_and(letter){end+=text[start..].chars().next().unwrap().len_utf8();for c in text[end..].chars(){if !mark(c){break;}end+=c.len_utf8();}
                if text[end..].starts_with('}') {if let Some((len,command))=accent(map,&text[end+1..]){out.push_str(&format!("\\{command}{{{}}}",&text[start..end]));p=end+1+len;continue;}}
            }
        }
        let c=text[p..].chars().next().unwrap();out.push(c);p+=c.len_utf8();
    }
    let text=out;let mut out=String::with_capacity(text.len());let mut p=0;
    while p<text.len(){let c=text[p..].chars().next().unwrap();
        if letter(c){let start=p;let mut end=p+c.len_utf8();let mut positions=Vec::new();for m in text[end..].chars(){if !mark(m){break;}positions.push(end);end+=m.len_utf8();}
            if let Some((at,len,command))=positions.into_iter().rev().find_map(|at|accent(map,&text[at..]).filter(|(len,_)|at+len==end).map(|(len,command)|(at,len,command))){out.push_str(&format!("\\{command}{{{}}}",&text[start..at]));p=at+len;continue;}
        }
        out.push(c);p+=c.len_utf8();
    }
    // Remaining marks attach to any non-mark, in runs of three, two, then one.
    for count in [3,2,1]{let text=out;out=String::with_capacity(text.len());let mut p=0;
        while p<text.len(){let c=text[p..].chars().next().unwrap();let mut end=p+c.len_utf8();let mut commands=Vec::new();
            if !mark(c){for _ in 0..count{if let Some((len,command))=accent(map,&text[end..]){commands.push(command);end+=len;}else{break;}}}
            if commands.len()==count{for command in commands.iter().rev(){out.push('\\');out.push_str(command);out.push('{');}out.push(c);for _ in 0..count{out.push('}');}p=end;}else{out.push(c);p+=c.len_utf8();}
        }
    }
    out
}
pub(crate) fn encode(text:&str,set:&str)->String{
    let tables=match set{"base"=>&*BASE,"full"=>&*FULL,_=>return text.into()};
    let mut text=normalize(text,"NFD").into_owned();
    for kind in ["greek","dings","negatedsymbols","superscripts","cmdsuperscripts","diacritics","letters","punctuation","symbols"]{
        let Some(map)=tables.maps.get(kind)else{continue;};
        if kind=="diacritics"{text=accents(&text,map);continue;}
        text=replace(&text,map,|key,value|match kind{
            "negatedsymbols"=>format!("{{$\\not\\{value}$}}"),"superscripts"=>format!("\\textsuperscript{{{value}}}"),"cmdsuperscripts"=>format!("\\textsuperscript{{\\{value}}}"),"dings"=>format!("\\ding{{{value}}}"),
            "letters"=>if tables.raw.contains(key){value.into()}else{format!("\\{value}{{}}")},
            _=>if value.starts_with("text")||value.starts_with("guil"){format!("\\{value}{{}}")}else if tables.raw.contains(key){value.into()}else{format!("{{$\\{value}$}}")},
        });
    }
    text
}
