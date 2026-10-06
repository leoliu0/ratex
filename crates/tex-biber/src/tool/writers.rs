use super::{enabled,option};
use crate::{bib, model::*};
use std::collections::{BTreeMap,BTreeSet};
use std::fmt::Write;
use std::path::Path;

#[derive(Default)]
pub(super) struct Metadata {macros:Vec<(String,String)>,comments:Vec<String>}
impl Metadata {
    pub(super) fn parse(input:&str,nostdmacros:bool)->Result<Self,String> {
        let mut result=Self::default();let mut prefix=String::new();let mut position=0;
        while let Some(offset)=input[position..].find('@') {
            let start=position+offset;let mut p=start+1;
            while input.as_bytes().get(p).is_some_and(u8::is_ascii_alphabetic){p+=1;}
            let kind=input[start+1..p].to_ascii_lowercase();
            while input.as_bytes().get(p).is_some_and(u8::is_ascii_whitespace){p+=1;}
            let Some(&open)=input.as_bytes().get(p)else{break};
            if open!=b'{'&&open!=b'(' {position=p+1;continue;}
            let body=p+1;let close=if open==b'{'{b'}'}else{b')'};
            let mut depth=1;let mut brace=0usize;let mut quote=false;let mut escape=false;p+=1;
            while p<input.len() {
                let c=input.as_bytes()[p];
                if escape {escape=false;}else if c==b'\\' {escape=true;}
                else if c==b'"'&&brace==0 {quote=!quote;}
                else if !quote {
                    if open==b'{' {if c==open{depth+=1;}else if c==close{depth-=1;}}
                    else if c==b'{' {brace+=1;}else if c==b'}'{brace=brace.saturating_sub(1);}
                    else if brace==0 {if c==open{depth+=1;}else if c==close{depth-=1;}}
                }
                p+=1;if depth==0{break;}
            }
            if depth!=0{return Err("Unclosed BibTeX record".into());}
            if kind=="comment" {result.comments.push(input[body..p-1].into());}
            if kind=="string" {
                // Text::BibTeX MACRODEF fieldlist in file order; values are the
                // expanded, not LaTeX-decoded, macro text.
                let content=&input[body..p-1];
                for definition in top_level_split(content) {
                    let Some((name,value))=definition.split_once('=')else{continue};
                    let mut database=bib::Database::default();
                    database.standard_macros(nostdmacros);
                    let synthetic=format!("{prefix}\n@misc{{toolmacro,value={}}}",value.trim());
                    database.add(&synthetic,"<tool macro>")?;
                    if let Some(value)=database.entries.first().and_then(|e|e.fields.get("value")) {result.macros.push((name.trim().to_ascii_lowercase(),value.clone()));}
                }
                prefix.push_str(&input[start..p]);prefix.push('\n');
            }
            position=p;
        }
        Ok(result)
    }
}
fn top_level_split(content:&str)->Vec<&str> {
    let mut parts=Vec::new();let (mut depth,mut quote,mut begin)=(0usize,false,0);
    for (i,c) in content.char_indices() {match c {'{'=>depth+=1,'}'=>depth=depth.saturating_sub(1),'"' if depth==0=>quote=!quote,',' if depth==0&&!quote=>{parts.push(&content[begin..i]);begin=i+1;},_=>{}}}
    parts.push(&content[begin..]);parts.retain(|s|!s.trim().is_empty());parts
}
fn casing(c:&Control,value:&str)->String {
    match option(c,"output_fieldcase","upper") {"lower"=>value.to_lowercase(),"title"=>{let mut chars=value.chars();chars.next().map(|ch|ch.to_uppercase().collect::<String>()+chars.as_str()).unwrap_or_default()},_=>value.to_uppercase()}
}
fn outfield(c:&Control,field:&str)->String {
    let replacements=option(c,"output_field_replace","");
    let replacement=replacements.split(',').filter_map(|s|s.split_once(':')).find(|(from,_)|*from==field).map(|(_,to)|to).unwrap_or(field);
    casing(c,replacement)
}
fn part(name:&Name,key:&str,protect:bool)->String {
    let value=name.part(key).replace('~'," ");
    if protect&&name.strip.contains(key){format!("{{{value}}}")}else{value}
}
fn xdata_reference<'a>(c:&Control,value:&'a str)->Option<&'a str> {
    value.strip_prefix(option(c,"xdatamarker","xdata")).and_then(|s|s.strip_prefix(option(c,"xnamesep","=")))
}
fn xdata(c:&Control,value:&str)->String {
    if let Some(reference)=xdata_reference(c,value) {
        format!("{}{}{}",option(c,"output_xdatamarker","xdata"),option(c,"output_xnamesep","="),reference.replace(option(c,"xdatasep","-"),option(c,"output_xdatasep","-")))
    }else{value.into()}
}
fn name(c:&Control,name:&Name)->String {
    if let Some(value)=name.options.get("xdata"){return xdata(c,value);}
    if enabled(c,"output_xname") {
        let mut parts=if c.nameparts.is_empty(){vec!["family","given","prefix","suffix"]}else{c.nameparts.iter().map(String::as_str).collect()};parts.sort_unstable();
        let sep=option(c,"output_xnamesep","=");
        let mut out=parts.into_iter().filter(|p|!name.part(p).is_empty()).map(|p|format!("{p}{sep}{}",part(name,p,false))).collect::<Vec<_>>();
        for key in ["useprefix","sortingnamekeytemplatename"] {if let Some(value)=name.options.get(key){out.push(format!("{key}{sep}{}",boolean(value)));}}
        out.join(", ")
    }else{
        let mut out=String::new();
        if !name.prefix.is_empty(){out.push_str(&part(name,"prefix",true));out.push(' ');}
        out.push_str(&part(name,"family",true));
        if !name.suffix.is_empty(){out.push_str(", ");out.push_str(&part(name,"suffix",true));}
        if !name.given.is_empty(){out.push_str(", ");out.push_str(&part(name,"given",true));}
        out
    }
}
fn boolean(s:&str)->&str {match s {"1"=>"true","0"=>"false",_=>s}}
fn range(value:&str)->String {
    bib::split_list(value,",").into_iter().map(|v|{let parts=v.split('-').filter(|s|!s.is_empty()).collect::<Vec<_>>();if parts.len()>1{format!("{}--{}",parts[0],parts[1])}else if v.ends_with('-'){format!("{}--",parts.first().copied().unwrap_or(""))}else{v}}).collect::<Vec<_>>().join(",")
}
// Perl truthiness of a scalar field value.
fn truthy(value:&str)->bool {!value.is_empty()&&value!="0"}
fn division(value:&str)->Option<String> {
    // Output::bibtex/biblatexml spell the last southern season `WinterS`.
    ["spring","summer","autumn","winter","springN","summerN","autumnN","winterN","springS","summerS","autumnS","WinterS","Q1","Q2","Q3","Q4","QD1","QD2","QD3","S1","S2"].iter().position(|s|*s==value).map(|n|(n+21).to_string())
}
/// sprintf('%.2d', $value) after Perl numification.
fn number(value:&str)->String {
    let value=value.trim_start();let (negative,digits)=match value.as_bytes().first(){Some(b'-')=>(true,&value[1..]),Some(b'+')=>(false,&value[1..]),_=>(false,value)};
    let n=digits[..digits.find(|c:char|!c.is_ascii_digit()).unwrap_or(digits.len())].parse::<i64>().unwrap_or(0);
    if negative&&n!=0{format!("-{n:02}")}else{format!("{n:02}")}
}
/// `s/\\bibtzminsep\s+/:/`
fn timezone(zone:&str)->String {
    for (i,_) in zone.match_indices("\\bibtzminsep") {
        let rest=&zone[i+12..];let trimmed=rest.trim_start_matches(char::is_whitespace);
        if trimmed.len()<rest.len(){return format!("{}:{trimmed}",&zone[..i]);}
    }
    zone.into()
}
fn get(fields:&BTreeMap<String,String>,d:&str,part:&str)->Option<String> {fields.get(&format!("{d}{part}")).filter(|v|truthy(v)).cloned()}
/// Approximate/uncertain markers of the `${d}` or `${d}end` point.
fn markers(flags:&BTreeSet<String>,p:&str)->&'static str {
    let (uncertain,approximate)=(flags.contains(&format!("{p}dateuncertain")),flags.contains(&format!("{p}datecirca")));
    if uncertain&&approximate{"%"}else if uncertain{"?"}else if approximate{"~"}else{""}
}
fn clock(fields:&BTreeMap<String,String>,p:&str)->Option<String> {
    let hour=get(fields,p,"hour")?;let part=|part:&str|number(fields.get(&format!("{p}{part}")).map(String::as_str).unwrap_or(""));
    Some(format!("T{}:{}:{}",number(&hour),part("minute"),part("second")))
}
/// Reverse of Utils::parse_date_unspecified: [year, month, day] overrides; deletes consumed end parts.
fn unspecified(fields:&mut BTreeMap<String,String>,d:&str)->[Option<String>;3] {
    let mut overrides:[Option<String>;3]=Default::default();
    let Some(unspec)=get(fields,d,"dateunspecified")else{return overrides};
    let year=fields.get(&format!("{d}year")).cloned().unwrap_or_default();
    // m/^(\d+)\d$/ and m/^(\d+)\d\d$/; a failed match leaves the capture undef.
    let leading=|n:usize|{let count=year.chars().count();if count>n&&year.chars().all(|c|crate::perl_unicode::property_contains("Nd",c).unwrap_or(false)){year.chars().take(count-n).collect()}else{String::new()}};
    let deleted:&[&str]=match unspec.as_str() {
        "yearindecade"=>{overrides[0]=Some(format!("{}X",leading(1)));&["endyear"]},
        "yearincentury"=>{overrides[0]=Some(format!("{}XX",leading(2)));&["endyear"]},
        "monthinyear"=>{overrides[1]=Some("XX".into());&["endyear","endmonth"]},
        "dayinmonth"=>{overrides[2]=Some("XX".into());&["endyear","endmonth","endday"]},
        "dayinyear"=>{overrides[1]=Some("XX".into());overrides[2]=Some("XX".into());&["endyear","endmonth","endday"]},
        _=>&[],
    };
    for part in deleted {fields.remove(&format!("{d}{part}"));}
    overrides
}
/// Output::bibtex::construct_datetime; consumed date parts are deleted from `fields`.
fn construct_datetime(fields:&mut BTreeMap<String,String>,flags:&BTreeSet<String>,d:&str)->String {
    let [year,mut month,day]=unspecified(fields,d);
    if let Some(s)=get(fields,d,"yeardivision"){month=division(&s);}
    let endmonth=get(fields,d,"endyeardivision").and_then(|s|division(&s));
    let mut out=String::new();
    let remove=|fields:&mut BTreeMap<String,String>,p:&str,parts:&[&str]|for part in parts{fields.remove(&format!("{p}{part}"));};
    let Some(year)=year.or_else(||get(fields,d,"year"))else{return out};
    out.push_str(&year);remove(fields,d,&["year"]);
    if let Some(month)=month.or_else(||get(fields,d,"month")){out.push('-');out.push_str(&number(&month));remove(fields,d,&["month"]);}
    if let Some(day)=day.or_else(||get(fields,d,"day")){out.push('-');out.push_str(&number(&day));remove(fields,d,&["day"]);}
    out.push_str(markers(flags,d));
    if let Some(time)=clock(fields,d){out.push_str(&time);remove(fields,d,&["hour","minute","second"]);}
    if let Some(zone)=get(fields,d,"timezone"){out.push_str(&timezone(&zone));remove(fields,d,&["timezone"]);}
    if fields.contains_key(&format!("{d}endyear")){out.push('/');}
    let e=format!("{d}end");
    if let Some(year)=get(fields,&e,"year") {
        out.push_str(&year);remove(fields,&e,&["year"]);
        if let Some(month)=endmonth.or_else(||get(fields,&e,"month")){out.push('-');out.push_str(&number(&month));remove(fields,&e,&["month"]);}
        if let Some(day)=get(fields,&e,"day"){out.push('-');out.push_str(&number(&day));remove(fields,&e,&["day"]);}
        out.push_str(markers(flags,&e));
        if let Some(time)=clock(fields,&e){out.push_str(&time);remove(fields,&e,&["hour","minute","second"]);}
        if let Some(zone)=get(fields,&e,"timezone"){out.push_str(&timezone(&zone));remove(fields,&e,&["timezone"]);}
    }
    out
}
/// Output::biblatexml date element: (simple date or range start, range end).
fn xml_date(fields:&mut BTreeMap<String,String>,flags:&BTreeSet<String>,d:&str)->Option<(String,Option<String>)> {
    let mut start=get(fields,d,"year")?;
    start.push_str(markers(flags,d));
    if flags.contains(&format!("{d}dateunknown")){start="unknown".into();}
    let [year,mut month,day]=unspecified(fields,d);
    if let Some(s)=get(fields,d,"yeardivision"){month=division(&s);}
    let endmonth=get(fields,d,"endyeardivision").and_then(|s|division(&s));
    let monthday=|value:Option<String>|value.filter(|v|truthy(v)).map(|v|number(&v));
    let e=format!("{d}end");
    let point=[year.or(Some(start)),monthday(month.or_else(||fields.get(&format!("{d}month")).cloned())),monthday(day.or_else(||fields.get(&format!("{d}day")).cloned()))];
    let mut start=point.into_iter().flatten().filter(|v|truthy(v)).collect::<Vec<_>>().join("-");
    let end=[fields.get(&format!("{e}year")).cloned(),monthday(endmonth.or_else(||fields.get(&format!("{e}month")).cloned())),monthday(fields.get(&format!("{e}day")).cloned())].into_iter().flatten().collect::<Vec<_>>();
    let suffix=|p:&str|clock(fields,p).unwrap_or_default()+&get(fields,p,"timezone").map(|z|timezone(&z)).unwrap_or_default();
    start.push_str(&suffix(d));
    if end.is_empty(){return Some((start,None));}
    Some((start,Some(end.join("-")+&suffix(&e))))
}
fn values(c:&Control,entry:&Entry,warnings:&mut Vec<String>)->BTreeMap<String,String> {
    let mut values=BTreeMap::new();let mut fields=entry.fields.clone();
    for (field,spec) in &c.fields {if spec.datatype!="date"{continue;}let d=field.strip_suffix("date").unwrap_or(field);
        let Some(year)=get(&fields,d,"year")else{continue};
        if d.is_empty()&&enabled(c,"output_legacy_dates") {
            if get(&fields,"","day").is_none()&&get(&fields,"","endyear").is_none() {
                values.insert(outfield(c,"year"),year);
                if let Some(month)=get(&fields,"","month"){let name=if enabled(c,"nostdmacros"){None}else{["1","2","3","4","5","6","7","8","9","10","11","12"].iter().position(|m|*m==month).map(|m|["jan","feb","mar","apr","may","jun","jul","aug","sep","oct","nov","dec"][m].to_owned())};values.insert(outfield(c,"month"),name.unwrap_or(month));}
                continue;
            }
            warnings.push(format!("Date in entry '{}' has DAY or ENDYEAR, cannot be output in legacy format.",entry.key));
        }
        values.insert(outfield(c,field),construct_datetime(&mut fields,&entry.flags,d));
    }
    for (field,value) in &fields {
        let Some(spec)=c.fields.get(field)else{continue};
        if spec.datatype=="date"||(spec.skip_output&&!matches!(field.as_str(),"ids"|"options"))||entry.names.contains_key(field)||entry.lists.contains_key(field)||!truthy(value) {continue;}
        if field=="crossref"&&enabled(c,"output_resolve_crossrefs")||field=="xdata"&&enabled(c,"output_resolve_xdata"){continue;}
        let value=if spec.datatype=="range"&&xdata_reference(c,value).is_none(){range(value)}else{xdata(c,value)};
        values.entry(outfield(c,field)).or_insert(value);
    }
    for (field,list) in &entry.names {
        if c.fields.get(field).is_some_and(|spec|spec.skip_output){continue;}
        let mut names=Vec::new();for key in ["useprefix","sortingnamekeytemplatename"]{if let Some(value)=list.options.get(key){names.push(format!("{key}={}",boolean(value)));}}
        names.extend(list.names.iter().map(|n|name(c,n)));if list.others{names.push("others".into());}
        values.insert(outfield(c,field),names.join(&format!(" {} ",option(c,"output_namesep","and"))));
    }
    for (field,list) in &entry.lists {if c.fields.get(field).is_some_and(|spec|spec.skip_output){continue;}values.insert(outfield(c,field),list.iter().map(|s|xdata(c,s)).collect::<Vec<_>>().join(&format!(" {} ",option(c,"output_listsep","and"))));}
    let mut annotations=BTreeMap::<String,Vec<&Annotation>>::new();
    for annotation in &entry.annotations {if values.contains_key(&outfield(c,&annotation.field)){annotations.entry(format!("{}{}{}{}",outfield(c,&annotation.field),option(c,"output_annotation_marker","+an"),option(c,"output_named_annotation_marker",":"),annotation.name)).or_default().push(annotation);}}
    for (field,mut annotations) in annotations {annotations.sort_by(|a,b|(&a.item,&a.part).cmp(&(&b.item,&b.part)));values.insert(field,annotations.into_iter().map(|a|format!("{}{}={}",a.item,if a.part.is_empty(){String::new()}else{format!(":{}",a.part)},if a.literal{format!("\"{}\"",a.value)}else{a.value.clone()})).collect::<Vec<_>>().join(";"));}
    values
}
pub(super) fn bibtex(c:&Control,entries:&[Entry],order:&[usize],metadata:&Metadata,warnings:&mut Vec<String>)->Result<String,String> {
    // %RSTRINGS: macro value -> name, later definitions overwrite earlier ones.
    let mut reverse=BTreeMap::new();for (key,value) in &metadata.macros {reverse.insert(crate::collation::normalization::normalize(value,"NFD"),key.clone());}
    let mut used=BTreeSet::new();let mut body=String::new();
    let indent=option(c,"output_indent","2");let tabs=indent.ends_with('t');let count=indent.trim_end_matches('t').parse::<usize>().unwrap_or(2);let indent=if tabs{"\t"}else{" "}.repeat(count);
    for &index in order {
        let start=body.len();
        let entry=&entries[index];let mut fields=values(c,entry,warnings);let _=writeln!(body,"@{}{{{},",outfield(c,&entry.kind),entry.key);
        let width=if enabled(c,"output_align"){fields.keys().map(|s|crate::gcstring::length(s)).max().unwrap_or(0)}else{0};
        let mut keys=Vec::with_capacity(fields.len());
        for selector in option(c,"output_field_order","options,abstract,names,lists,dates").split(',').map(str::trim) {
            if matches!(selector,"names"|"lists"|"dates"){
                // The oracle date-output loop mutates datefield names into
                // prefixes. Thus `dates` selects URI `url` via `urldate`, not
                // reconstructed DATE/EVENTDATE values.
                keys.extend(fields.keys().filter(|key|{
                    let base=key.split(option(c,"output_annotation_marker","+an")).next().unwrap_or(key).to_lowercase();
                    if selector=="dates"{c.fields.get(&format!("{base}date")).is_some_and(|spec|spec.datatype=="date")}
                    else{c.fields.get(&base).is_some_and(|spec|match selector{"names"=>spec.fieldtype=="list"&&spec.datatype=="name","lists"=>spec.fieldtype=="list"&&!matches!(spec.datatype.as_str(),"name"|"uri"|"verbatim"),_=>false})}
                }).cloned());
            }else{keys.push(outfield(c,selector));}
        }
        keys.extend(fields.keys().cloned());
        for key in keys {let Some(mut value)=fields.remove(&key)else{continue};
            if let Some(mac)=reverse.get(&crate::collation::normalization::normalize(&value,"NFD")){used.insert(mac.clone());value=casing(c,mac);}
            let macro_value=metadata.macros.iter().any(|(m,_)|m.eq_ignore_ascii_case(&value))||(!enabled(c,"nostdmacros")&&["jan","feb","mar","apr","may","jun","jul","aug","sep","oct","nov","dec"].contains(&value.to_ascii_lowercase().as_str()));
            let field=format!("{indent}{key}{} = {},\n"," ".repeat(width.saturating_sub(crate::gcstring::length(&key))),if macro_value{value}else{format!("{{{value}}}")});
            let columns=option(c,"wraplines","0").parse::<usize>().unwrap_or(0);
            if columns==0{body.push_str(&field);}else{
                let inum=count+if enabled(c,"output_align"){width}else{crate::gcstring::length(&key)}+4;
                body.push_str(&wrap_field(&field,&if tabs{"\t"}else{" "}.repeat(inum),columns));
            }
        }
        body.push_str("}\n\n");
        let mut recode=enabled(c,"output_safechars");
        let encoding=option(c,"output_encoding","UTF-8");
        if !recode&&encoding!="UTF-8"&&crate::encoding::encode(&crate::collation::normalization::normalize(&body[start..],"NFC"),encoding)?.contains(&b'?'){
            recode=true;
            warnings.push(format!("The entry '{}' has characters which cannot be encoded in '{encoding}'. Recoding problematic characters into macros.",entry.key));
        }
        if recode{let encoded=super::recode::encode(&body[start..],option(c,"output_safecharsset","base"));body.truncate(start);body.push_str(&encoded);}
    }
    let mut result=String::new();
    if !enabled(c,"output_no_macrodefs") {
        let mut definitions=BTreeMap::new();for (key,value) in &metadata.macros {definitions.insert(key,value);}
        let mut defs=definitions.into_iter().filter(|(key,_)|used.contains(*key)||enabled(c,"output_all_macrodefs")).map(|(key,value)|format!("@{}{{{} = \"{}\"}}\n",casing(c,"string"),casing(c,key),crate::collation::normalization::normalize(value,"NFD"))).collect::<Vec<_>>();
        defs.sort();
        for def in &defs {result.push_str(def);}
        if !defs.is_empty(){result.push('\n');}
    }
    result.push_str(&body);
    if !enabled(c,"strip_comments") {for comment in &metadata.comments{let _=writeln!(result,"@{}{{{comment}}}",casing(c,"comment"));}}
    Ok(result)
}
fn escaped(value:&str)->String {crate::collation::normalization::normalize(value,"NFC").replace('&',"&amp;").replace('<',"&lt;").replace('>',"&gt;").replace('"',"&quot;")}
struct Xml {text:String,indent:usize,level:usize,empty:bool}
impl Xml {
    fn line(&mut self,value:&str){self.empty=false;self.text.push_str(&" ".repeat(self.indent*self.level));self.text.push_str(value);self.text.push('\n');}
    fn attrs(attrs:&[(&str,String)])->String {attrs.iter().map(|(k,v)|format!(" {k}=\"{}\"",escaped(v))).collect()}
    fn open(&mut self,tag:&str,attrs:&[(&str,String)]){self.line(&format!("<bltx:{tag}{}>",Self::attrs(attrs)));self.level+=1;self.empty=true;}
    // XML::Writer DATA_MODE closes a childless element on its start-tag line.
    fn close(&mut self,tag:&str){self.level-=1;if self.empty{self.text.pop();self.text.push_str(&format!("</bltx:{tag}>\n"));self.empty=false;}else{self.line(&format!("</bltx:{tag}>"));}}
    fn data(&mut self,tag:&str,value:&str,attrs:&[(&str,String)]){self.line(&format!("<bltx:{tag}{}>{}</bltx:{tag}>",Self::attrs(attrs),escaped(value)));}
    fn value(&mut self,c:&Control,tag:&str,value:&str,attrs:&[(&str,String)]){
        if let Some(reference)=xdata_reference(c,value){let mut attrs=attrs.to_vec();attrs.push(("xdata",reference.replace(option(c,"xdatasep","-"),option(c,"output_xdatasep","-"))));self.line(&format!("<bltx:{tag}{} />",Self::attrs(&attrs)));}else{self.data(tag,value,attrs);}
    }
}
pub(super) fn xml(c:&Control,entries:&[Entry],order:&[usize],source:&Path)->String {
    let mut xml=Xml {text:format!("<?xml version=\"1.0\" encoding=\"{}\"?>\n<?xml-model href=\"{}\" type=\"application/xml\" schematypens=\"http://relaxng.org/ns/structure/1.0\"?>\n<!-- Auto-generated by Biber::Output::biblatexml -->\n\n",option(c,"output_encoding","UTF-8"),escaped(&source.with_extension("rng").to_string_lossy())),indent:option(c,"output_indent","2").parse().unwrap_or(2),level:0,empty:false};
    xml.open("entries",&[("xmlns:bltx","http://biblatex-biber.sourceforge.net/biblatexml".into())]);
    for &index in order {
        let entry=&entries[index];xml.open("entry",&[("id",entry.key.clone()),("entrytype",entry.kind.clone())]);
        for key in ["ids","xdata"] {if key=="xdata"&&enabled(c,"output_resolve_xdata"){continue;}if let Some(value)=entry.fields.get(key){xml.open(key,&[]);xml.open("list",&[]);let mut items=bib::split_list(value,",");if key=="ids"{items.sort();}for item in items{xml.data("item",&item,&[]);}xml.close("list");xml.close(key);}}
        for key in ["crossref","options"] {if key=="crossref"&&enabled(c,"output_resolve_crossrefs"){continue;}if let Some(value)=entry.fields.get(key){xml.data(key,value,&[]);}}
        let names=if c.field_order.is_empty(){entry.names.keys().cloned().collect::<Vec<_>>()}else{c.field_order.clone()};
        for field in names {let Some(list)=entry.names.get(&field)else{continue};let mut attrs=vec![("type",field.clone())];if list.others{attrs.push(("morenames","1".into()));}for (key,value) in &list.options {if crate::names::output_option(c,"NAMELIST",key){attrs.push((key,boolean(value).into()));}}
            xml.open("names",&attrs);
            for name in &list.names {
                if let Some(reference)=name.options.get("xdata"){xml.value(c,"name",reference,&[]);continue;}
                let attrs=name.options.iter().filter(|(key,_)|key.as_str()=="gender"||crate::names::output_option(c,"NAME",key)).map(|(key,value)|(key.as_str(),boolean(value).into())).collect::<Vec<_>>();xml.open("name",&attrs);
                let parts=if c.nameparts.is_empty(){vec!["family","given","prefix","suffix"]}else{c.nameparts.iter().map(String::as_str).collect()};
                for part in parts {let value=name.part(part);if value.is_empty(){continue;}let words=value.split([' ','~']).collect::<Vec<_>>();let initials=name.initial_tokens.get(part);
                    if words.len()>1 {xml.open("namepart",&[("type",part.into())]);for (index,word) in words.iter().enumerate(){let attrs=initials.and_then(|v|v.get(index)).filter(|v|!v.is_empty()).map(|v|vec![("initial",v.clone())]).unwrap_or_default();xml.data("namepart",word,&attrs);}xml.close("namepart");}
                    else{let mut attrs=vec![("type",part.into())];if let Some(initial)=initials.and_then(|v|v.first()).filter(|v|!v.is_empty()){attrs.push(("initial",initial.clone()));}xml.data("namepart",value,&attrs);}
                }
                xml.close("name");
            }
            xml.close("names");
        }
        for (field,list) in &entry.lists {let more=list.last().is_some_and(|s|s.eq_ignore_ascii_case(option(c,"others_string","others")));let attrs=if more{vec![("morelist","1".into())]}else{vec![]};xml.open(field,&attrs);xml.open("list",&[]);for item in list.iter().take(list.len()-usize::from(more)){xml.value(c,"item",item,&[]);}xml.close("list");xml.close(field);}
        for (field,value) in &entry.fields {let Some(spec)=c.fields.get(field)else{continue};if entry.names.contains_key(field)||entry.lists.contains_key(field)||matches!(field.as_str(),"ids"|"xdata"|"crossref"|"options")||spec.datatype=="date"||spec.datatype=="range"||spec.format=="xsv"||!matches!(spec.datatype.as_str(),"entrykey"|"key"|"literal"|"code"|"integer"|"verbatim"|"uri"){continue;}if !value.is_empty()||spec.nullok{xml.value(c,field,value,&[]);}}
        for field in &c.field_order {let Some(spec)=c.fields.get(field)else{continue};if spec.datatype!="xsv"||matches!(field.as_str(),"ids"|"xdata"){continue;}if let Some(value)=entry.fields.get(field){xml.value(c,field,value,&[]);}}
        for (field,spec) in &c.fields {if spec.datatype!="range"{continue;}if let Some(value)=entry.fields.get(field){if xdata_reference(c,value).is_some(){xml.value(c,field,value,&[]);continue;}xml.open(field,&[]);xml.open("list",&[]);for item in range(value).split(','){if let Some((start,end))=item.split_once("--"){xml.open("item",&[]);xml.data("start",start,&[]);xml.data("end",end,&[]);xml.close("item");}else{xml.data("item",item,&[]);}}xml.close("list");xml.close(field);}}
        let mut dates=entry.fields.clone();
        for (field,spec) in &c.fields {if spec.datatype!="date"{continue;}let prefix=field.strip_suffix("date").unwrap_or("");if let Some((start,end))=xml_date(&mut dates,&entry.flags,prefix){let attrs=if prefix.is_empty(){vec![]}else{vec![("type",prefix.into())]};if let Some(end)=end{xml.open("date",&attrs);xml.data("start",&start,&[]);xml.data("end",&end,&[]);xml.close("date");}else{xml.data("date",&start,&attrs);}}}
        let mut annotations=entry.annotations.iter().collect::<Vec<_>>();annotations.sort_by(|a,b|{let scope=|a:&Annotation|if a.item.is_empty(){0}else if a.part.is_empty(){1}else{2};(scope(a),&a.field,&a.name,&a.item,&a.part).cmp(&(scope(b),&b.field,&b.name,&b.item,&b.part))});
        for annotation in annotations {let mut attrs=vec![("field",annotation.field.clone()),("name",annotation.name.clone())];if !annotation.item.is_empty(){attrs.push(("item",annotation.item.clone()));}if !annotation.part.is_empty(){attrs.push(("part",annotation.part.clone()));}attrs.push(("literal",if annotation.literal{"1"}else{"0"}.into()));xml.data("annotation",&annotation.value,&attrs);}
        xml.close("entry");
    }
    xml.close("entries");xml.text
}
fn is_mark(c:char)->bool{crate::perl_unicode::property_contains("M",c).unwrap_or(false)}
fn expand_tabs(text:&str)->String{
    let mut out=String::with_capacity(text.len());let mut column=0;
    for c in text.chars(){if c=='\t'{let n=8-column%8;out.push_str(&" ".repeat(n));column=0;}else{out.push(c);if c=='\n'{column=0;}else if !is_mark(c){column+=1;}}}
    out
}
fn clusters(text:&str)->Vec<&str>{
    let mut result=Vec::new();let mut start=0;
    for (i,c) in text.char_indices(){if i>start&&!is_mark(c){result.push(&text[start..i]);start=i;}}
    if start<text.len(){result.push(&text[start..]);}result
}
fn unexpand_chunk(text:&str)->String {
    let mut out=String::with_capacity(text.len());
    for (i,line) in text.split('\n').enumerate(){if i>0{out.push('\n');}let expanded=expand_tabs(line);let parts=clusters(&expanded);
        for block in parts.chunks(8){let start=out.len();for part in block{out.push_str(part);}if block.len()==8{let end=out.len();let trimmed=out[start..end].trim_end_matches(' ').len();if end-start-trimmed>=2{out.truncate(start+trimmed);out.push('\t');}}}
    }
    out
}
pub(crate) fn wrap_field(text:&str,continuation:&str,columns:usize)->String{
    wrap(text,"",continuation,columns,false)
}
pub(crate) fn wrap(text:&str,initial:&str,continuation:&str,mut columns:usize,unexpand:bool)->String{
    let expanded=expand_tabs(text);let chars=clusters(&expanded);let indent=expand_tabs(continuation).chars().filter(|c|!is_mark(*c)).count();
    if columns<=indent+1&&!continuation.is_empty(){columns=indent+2;}
    if columns<2{return text.into();}
    let initial_indent=expand_tabs(initial).chars().filter(|c|!is_mark(*c)).count();let mut width=columns.saturating_sub(initial_indent+1);let next=columns-indent-1;let mut position=0;let mut result=String::new();let mut remainder=String::new();let mut subsequent=false;
    while position<chars.len()&&!chars[position..].iter().all(|s|s.chars().next().unwrap().is_whitespace()){
        let limit=(position+width).min(chars.len());let mut boundary=None;
        for end in position..=limit{
            if chars[position..end].iter().any(|s|s.starts_with('\n')){break;}
            if end==chars.len()||chars[end].chars().next().unwrap().is_whitespace(){boundary=Some(end);}
        }
        let end=boundary.unwrap_or(limit);let mut consumed=end;
        remainder.clear();
        if end<chars.len(){if chars[end].starts_with('\n'){while consumed<chars.len()&&chars[consumed].starts_with('\n'){remainder.push_str(chars[consumed]);consumed+=1;}}else if boundary.is_some(){remainder.push_str(chars[end]);consumed+=1;}else{remainder.push('\n');}}
        let lead=if subsequent{continuation}else{initial};
        if unexpand{let mut chunk=String::new();if subsequent{chunk.push('\n');}chunk.push_str(lead);for cluster in &chars[position..end]{chunk.push_str(cluster);}result.push_str(&unexpand_chunk(&chunk));}
        else{if subsequent{result.push('\n');}result.push_str(lead);for cluster in &chars[position..end]{result.push_str(cluster);}}
        position=consumed;subsequent=true;width=next;
    }
    result.push_str(&remainder);
    if position<chars.len(){result.push_str(continuation);result.push_str(&chars[position..].concat());}
    result
}
