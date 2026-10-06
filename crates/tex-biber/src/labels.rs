use crate::model::{Control, Entry, LabelField, Name, NameList, NameTemplatePart};
use crate::{gcstring, names, process};
use std::collections::BTreeMap;

fn truth(value: Option<&String>) -> bool { value.is_some_and(|s|matches!(s.as_str(),"1"|"true")) }
fn norm(c:&Control,value:&str)->String {
    static MACRO:std::sync::LazyLock<regex::Regex>=std::sync::LazyLock::new(||regex::Regex::new(r"\\[A-Za-z]+").unwrap());
    static TIES:std::sync::LazyLock<regex::Regex>=std::sync::LazyLock::new(||regex::Regex::new(r"([^\\])~").unwrap());
    static DEFAULT:std::sync::LazyLock<regex::Regex>=std::sync::LazyLock::new(||regex::Regex::new(r"[\p{Pc}\p{Ps}\p{Pe}\p{Pi}\p{Pf}\p{Po}\p{S}\p{C}]+").unwrap());
    let macros=MACRO.replace_all(value,"");let ties=TIES.replace_all(&macros,"$1 ");
    let clean=if c.options.contains_key("nolabel"){names::strip_rules(c,"nolabel",&ties)}else{DEFAULT.replace_all(&ties,"")};
    clean.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn substring(s:&str,width:usize,right:bool)->String {if right{gcstring::suffix(s,width)}else{gcstring::prefix(s,width)}.to_owned()}
fn visible(c:&Control,e:&Entry,n:&NameList)->usize {process::visible_final(c,e,n,"alpha")}
fn range_matches(raw:&str,count:usize)->bool {
    if let Ok(n)=raw.parse::<usize>(){return count==n;}
    let Some((start,end))=raw.split_once('-')else{return false};
    start.parse::<usize>().is_ok_and(|n|count<n)==false && end.parse::<usize>().is_ok_and(|n|count>n)==false
}
fn name_range(raw:Option<&String>,visible:usize,total:usize)->(usize,usize) {
    let Some(raw)=raw else{return (0,visible)};
    if let Ok(n)=raw.parse::<usize>() {return(n.saturating_sub(1).min(total),n.min(total));}
    let (start,end)=raw.split_once('-').unwrap_or((raw.as_str(),""));
    let start=start.parse::<usize>().unwrap_or(1).saturating_sub(1).min(total);
    let end=if end=="+"{visible}else{end.parse().unwrap_or(total)};
    (start,end.min(total).max(start))
}
fn template_name<'a>(e:&'a Entry,list:&'a NameList,context:&'a str)->&'a str {
    list.options.get("labelalphanametemplatename").map(String::as_str).unwrap_or_else(||process::entry_option(e,"labelalphanametemplatename").unwrap_or(if context.is_empty(){"global"}else{context}))
}
fn name_template<'a>(c:&'a Control,template:&str)->Option<&'a Vec<Vec<NameTemplatePart>>> {
    c.name_templates.get("labelalphanametemplate").and_then(|t|t.get(template))
}
fn main_string(c:&Control,name:&Name,template:&str,prefix:bool,normalized:bool)->String {
    let mut out=String::new();
    if let Some(schema)=name_template(c,template) {for p in schema.iter().flatten().filter(|p|!truth(p.attributes.get("pre"))&&(!truth(p.attributes.get("use"))||p.name!="prefix"||prefix)) {
        let value=name.part(&p.name);if normalized{out.push_str(&norm(c,value));}else{out.push_str(value);}
    }}else if normalized{out=norm(c,&name.family);}else{out.push_str(&name.family);}
    out
}
fn dynamic_rows(c:&Control,entries:&[Entry],field:&str,context:&str,normalized:bool)->Vec<Vec<String>> {
    entries.iter().map(|e| {
        let key=if field=="labelname"{e.computed.get("labelnamesource").map(String::as_str).unwrap_or("")}else{field};
        if let Some(n)=e.names.get(key){
            let mut template=template_name(e,n,context);let mut prefix=n.options.get("useprefix").map(|v|matches!(v.as_str(),"1"|"true")).unwrap_or_else(||process::enabled(c,e,"useprefix"));
            n.names.iter().take(visible(c,e,n)).map(|name|{
                if let Some(value)=name.options.get("labelalphanametemplatename"){template=value;}
                if let Some(value)=name.options.get("useprefix"){prefix=matches!(value.as_str(),"1"|"true");}
                main_string(c,name,template,prefix,normalized)
            }).collect()
        }
        else{let value=e.computed.get(key).or_else(||e.fields.get(key)).map(String::as_str).unwrap_or("");vec![if normalized{norm(c,value)}else{value.into()}]}
    }).collect()
}
fn variable_widths(rows:&[Vec<String>],max:Option<usize>,fixed:bool,threshold:usize)->BTreeMap<String,usize> {
    let mut unique=BTreeMap::<String,usize>::new();for row in rows {for (i,s) in row.iter().enumerate(){unique.insert(s.clone(),i);}}
    let longest=max.unwrap_or_else(||unique.keys().map(|s|gcstring::length(s)).max().unwrap_or(1)).max(1);
    let mut widths=BTreeMap::new();
    for width in 1..=longest {let mut counts=BTreeMap::new();for s in unique.keys(){*counts.entry(substring(s,width,false)).or_insert(0)+=1;}
        for s in unique.keys(){if !widths.contains_key(s)&&(counts[&substring(s,width,false)]==1||width==longest){widths.insert(s.clone(),width.min(gcstring::length(s)));}}
    }
    if fixed {let mut counts=BTreeMap::<(usize,usize),usize>::new();for (s,index) in &unique{*counts.entry((*index,widths[s])).or_default()+=1;}let mut fixed_width=BTreeMap::<usize,usize>::new();for ((index,width),count) in counts{if count>=threshold{let w=fixed_width.entry(index).or_default();*w=(*w).max(width);}}for (s,width) in &mut widths{if let Some(w)=fixed_width.get(&unique[s]){if *w>0{*width=*w;}}}}
    widths
}
fn list_substrings(rows:&[Vec<String>])->Vec<Vec<String>> {
    let mut widths=rows.iter().map(|r|vec![1;r.len()]).collect::<Vec<_>>();let mut result=vec![None;rows.len()];
    loop {let mut groups=BTreeMap::<String,Vec<usize>>::new();for (i,row) in rows.iter().enumerate(){if result[i].is_none(){let key=row.iter().zip(&widths[i]).map(|(s,w)|substring(s,*w,false)).collect::<String>();groups.entry(key).or_default().push(i);}}
        if groups.is_empty(){break;}
        for group in groups.values(){let i=group[0];if group.len()==1{result[i]=Some(rows[i].iter().zip(&widths[i]).map(|(s,w)|substring(s,*w,false)).collect());continue;}
            if group.iter().all(|&j|rows[j]==rows[i]){result[i]=Some(rows[i].iter().map(|s|substring(s,1,false)).collect());continue;}
            let peers=group.iter().copied().filter(|&j|rows[j]!=rows[i]).collect::<Vec<_>>();
            let differing=(0..rows[i].len()).find(|&p|peers.iter().all(|&j|rows[j].get(p)!=rows[i].get(p))).or_else(||(!rows[i].is_empty()).then_some(0));
            if let Some(p)=differing {if widths[i][p]<gcstring::length(&rows[i][p]){widths[i][p]+=1;}else{result[i]=Some(rows[i].iter().zip(&widths[i]).map(|(s,w)|substring(s,*w,false)).collect());}}else{result[i]=Some(rows[i].iter().zip(&widths[i]).map(|(s,w)|substring(s,*w,false)).collect());}
        }
    }
    result.into_iter().map(Option::unwrap).collect()
}
fn static_part(c:&Control,value:&str,attrs:&BTreeMap<String,String>,overrides:Option<&BTreeMap<String,String>>)->String {
    if !attrs.contains_key("substring_width"){return value.into();}
    let get=|key:&str|overrides.and_then(|a|a.get(key)).or_else(||attrs.get(key));
    let Some(raw_width)=get("substring_width")else{return value.into()};let width=raw_width.parse::<usize>().unwrap_or(1).max(1);
    let right=get("substring_side").is_some_and(|s|s=="right");
    // Utils::match_indices: character match offsets index GCString clusters,
    // shifted by the cluster length of each earlier regex's final match.
    let mut excluded=Vec::<(String,isize)>::new();let mut relen=0isize;
    for pattern in names::text_rules(c,"nolabelwidthcount"){if pattern.is_empty(){continue;}let mut len=0isize;names::with_text_rule(pattern,|rule|{for found in rule.inner.find_iter(value).flatten(){let start=value[..found.start()].chars().count() as isize;let found=gcstring::substr(value,start,found.as_str().chars().count() as isize);excluded.push((found.to_owned(),start-relen));len=gcstring::length(found) as isize;}});relen+=len;}
    let clean=names::strip_rules(c,"nolabelwidthcount",value);
    static COMPOUND:std::sync::LazyLock<regex::Regex>=std::sync::LazyLock::new(||regex::Regex::new(r"[\s\p{Dash}]+").unwrap());
    let mut result=if get("substring_compound").is_some_and(|s|matches!(s.as_str(),"1"|"true")){COMPOUND.split(&clean).filter(|s|!s.is_empty()).map(|s|substring(s,width,right)).collect()}else{substring(&clean,width,right)};
    if let Some(pad)=get("pad_char"){let pad=unescape(pad);let padding=pad.repeat(width.saturating_sub(gcstring::length(&result)));if get("pad_side").is_some_and(|s|s=="left"){result=padding+&result}else{result.push_str(&padding);}result=escape(&result);}
    // Reinstated last index first. An index below -2*length makes sombok croak; that case leaves the label unchanged.
    for (text,index) in excluded.into_iter().rev() {if index+1<=gcstring::length(&result) as isize{if let Some(replaced)=gcstring::replace(&result,index,0,&text){result=replaced;}}}
    result
}
struct Dynamic { widths:BTreeMap<String,usize>, lists:Option<Vec<Vec<String>>> }
fn unescape(value:&str)->String {
    static ESCAPE:std::sync::LazyLock<regex::Regex>=std::sync::LazyLock::new(||regex::Regex::new(r"\\([_\^$~#%&])").unwrap());
    ESCAPE.replace_all(value,"$1").into_owned()
}
fn escape(value:&str)->String {let mut out=String::with_capacity(value.len());for ch in value.chars(){if matches!(ch,'_'|'^'|'$'|'#'|'%'|'&'){out.push('\\');}out.push(ch);}out}
fn case(attrs:&BTreeMap<String,String>,value:String)->String {let upper=truth(attrs.get("uppercase"));let lower=truth(attrs.get("lowercase"));if upper&&!lower{value.to_uppercase()}else if lower&&!upper{value.to_lowercase()}else{value}}
fn field(c:&Control,e:&Entry,index:usize,f:&LabelField,context:&str,dynamic:Option<&Dynamic>,sort:bool)->String {
    if f.literal || (!c.fields.contains_key(&f.name)&&!matches!(f.name.as_str(),"labelname"|"labeltitle"|"labelyear"|"labelmonth"|"labelday"|"citekey"|"entrykey")){let raw=unescape(&f.name);return if sort{raw}else{escape(&raw)};}
    let key=if f.name=="labelname"{e.computed.get("labelnamesource").map(String::as_str).unwrap_or("")}else{&f.name};
    let mut value=String::new();
    if let Some(list)=e.names.get(key) {
        let visible=visible(c,e,list);if f.attributes.get("ifnames").is_some_and(|s|!range_matches(s,visible)){return value;}
        let (start,end)=name_range(f.attributes.get("names"),visible,list.names.len());
        let mut template=template_name(e,list,context);
        let mut useprefix=list.options.get("useprefix").map(|s|matches!(s.as_str(),"1"|"true")).unwrap_or_else(||process::enabled(c,e,"useprefix"));
        for (i,name) in list.names.iter().enumerate().take(end) {
            if let Some(value)=name.options.get("labelalphanametemplatename"){template=value;}
            if let Some(value)=name.options.get("useprefix"){useprefix=matches!(value.as_str(),"1"|"true");}
            if i<start{continue;}
            if i>start {if let Some(sep)=f.attributes.get("namessep"){value.push_str(sep);}}
            let mut main=String::new();
            if let Some(schema)=name_template(c,template) {for p in schema.iter().flatten(){if truth(p.attributes.get("use"))&&p.name=="prefix"&&!useprefix{continue;}let raw=norm(c,name.part(&p.name));if raw.is_empty(){continue;}
                if truth(p.attributes.get("pre")){let mut attrs=p.attributes.clone();attrs.entry("substring_width".into()).or_insert_with(||"1".into());value.push_str(&static_part(c,&raw,&attrs,None));}
                else if let Some(dynamic)=dynamic{if let Some(lists)=&dynamic.lists{main.push_str(lists.get(index).and_then(|r|r.get(i)).map(String::as_str).unwrap_or(""));break;}main.push_str(&substring(&raw,dynamic.widths.get(&main_string(c,name,template,useprefix,true)).copied().unwrap_or(1),false));}
                else{main.push_str(&static_part(c,&raw,&f.attributes,Some(&p.attributes)));}
            }}else{if useprefix&&!name.prefix.is_empty(){value.push_str(&substring(&norm(c,&name.prefix),1,false));}let raw=norm(c,&name.family);main.push_str(&if let Some(dynamic)=dynamic{if let Some(lists)=&dynamic.lists{lists.get(index).and_then(|r|r.get(i)).cloned().unwrap_or_default()}else{substring(&raw,dynamic.widths.get(&raw).copied().unwrap_or(1),false)}}else{static_part(c,&raw,&f.attributes,None)});}
            value.push_str(&case(&f.attributes,main));
        }
        if !truth(f.attributes.get("noalphaothers"))&&(list.others||end<list.names.len()){value.push_str(process::option(c,e,if sort{"sortalphaothers"}else{"alphaothers"}));}
    }else{let raw=match f.name.as_str(){"entrykey"|"citekey"=>e.key.as_str(),_=>e.computed.get(&f.name).or_else(||e.fields.get(&f.name)).map(String::as_str).unwrap_or("")};let raw=if matches!(f.name.as_str(),"entrykey"|"citekey"|"label"|"shorthand"|"sortkey"){raw.into()}else{norm(c,raw)};value=if let Some(dynamic)=dynamic{if let Some(lists)=&dynamic.lists{lists.get(index).and_then(|r|r.first()).cloned().unwrap_or_default()}else{substring(&raw,dynamic.widths.get(&raw).copied().unwrap_or(1),false)}}else{static_part(c,&raw,&f.attributes,None)};}
    if !e.names.contains_key(key){value=case(&f.attributes,value);}
    if sort{unescape(&value)}else{value}
}
pub(crate) fn prepare(c:&Control,entries:&mut [Entry],context:&str) {
    let mut cache=BTreeMap::<(String,String),Dynamic>::new();
    for e in entries.iter(){for f in c.labelalpha_template(&e.kind).iter().flat_map(|p|&p.fields){if let Some(mode)=f.attributes.get("substring_width").filter(|s|s.contains(['v','l'])){let key=(f.name.clone(),mode.clone());if !cache.contains_key(&key){let list_mode=mode.contains('l')&&!mode.contains('v');let rows=dynamic_rows(c,entries,&f.name,context,!list_mode);let lists=list_mode.then(||list_substrings(&rows));let widths=variable_widths(&rows,f.attributes.get("substring_width_max").and_then(|s|s.parse().ok()),mode.contains('f'),f.attributes.get("substring_fixed_threshold").and_then(|s|s.parse().ok()).unwrap_or(1));cache.insert(key,Dynamic{widths,lists});}}}}
    let results=entries.iter().enumerate().map(|(index,e)|{if !process::enabled(c,e,"labelalpha")||process::enabled(c,e,"skiplab"){return(None,None)}let mut alpha=String::new();let mut sortalpha=String::new();let mut final_=false;for p in c.labelalpha_template(&e.kind){for f in &p.fields{let d=f.attributes.get("substring_width").and_then(|mode|cache.get(&(f.name.clone(),mode.clone())));let v=field(c,e,index,f,context,d,false);alpha.push_str(&v);sortalpha.push_str(&field(c,e,index,f,context,d,true));if !v.is_empty(){final_=f.final_;break;}}if final_{break;}}(Some(alpha),Some(sortalpha))}).collect::<Vec<_>>();
    for (e,(alpha,sortalpha)) in entries.iter_mut().zip(results){if let Some(alpha)=alpha{e.computed.insert("labelalpha".into(),alpha);}if let Some(alpha)=sortalpha{e.computed.insert("sortlabelalpha".into(),alpha);}}
}
