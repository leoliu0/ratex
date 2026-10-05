use crate::model::*;
use crate::names;
use std::collections::{BTreeMap,BTreeSet};

pub struct Database {pub entries:Vec<Entry>,pub preamble:Vec<String>,pub warnings:Vec<String>,macros:BTreeMap<String,String>}
impl Default for Database {fn default()->Self {Self {entries:vec![],preamble:vec![],warnings:vec![],macros:[("jan","1"),("feb","2"),("mar","3"),("apr","4"),("may","5"),("jun","6"),("jul","7"),("aug","8"),("sep","9"),("oct","10"),("nov","11"),("dec","12")].into_iter().map(|(a,b)|(a.into(),b.into())).collect()}}}
struct Parser<'a> {input:&'a str,pos:usize}
impl<'a> Parser<'a> {
    fn peek(&self)->Option<char>{self.input[self.pos..].chars().next()}
    fn next(&mut self)->Option<char>{let c=self.peek()?;self.pos+=c.len_utf8();Some(c)}
    fn ws(&mut self){loop {while self.peek().is_some_and(char::is_whitespace){self.next();} if self.peek()==Some('%') {while self.next().is_some_and(|c|c!='\n') {}} else {break}}}
    fn ident(&mut self)->String {self.ws();let start=self.pos;while self.peek().is_some_and(|c|!c.is_whitespace()&&!"=,#{}()\"".contains(c)){self.next();}self.input[start..self.pos].into()}
    fn balanced(&mut self,open:char,close:char)->Result<String,String>{
        if self.next()!=Some(open){return Err("Expected opening BibTeX delimiter".into())}let start=self.pos;let mut depth=1usize;let mut escaped=false;
        while let Some(c)=self.next(){if escaped{escaped=false;continue}if c=='\\'{escaped=true;continue}if c==open {depth+=1;}else if c==close {depth-=1;if depth==0{return Ok(self.input[start..self.pos-c.len_utf8()].into())}}}
        Err("Unclosed BibTeX value".into())
    }
    fn quoted(&mut self)->Result<String,String>{self.next();let start=self.pos;let mut depth=0usize;let mut escaped=false;while let Some(c)=self.next(){if escaped{escaped=false;continue}if c=='\\'{escaped=true;}else if c=='{'{depth+=1;}else if c=='}' {depth=depth.saturating_sub(1);}else if c=='\"'&&depth==0{return Ok(self.input[start..self.pos-1].into())}}Err("Unclosed quoted BibTeX value".into())}
    fn value(&mut self,db:&mut Database)->Result<String,String>{let mut value=String::new();loop {self.ws();let part=match self.peek(){Some('{')=>self.balanced('{','}')?,Some('"')=>self.quoted()?,Some(_)=>{let atom=self.ident();if atom.is_empty(){return Err(format!("Expected BibTeX value at byte {}",self.pos))}if atom.chars().all(|c|c.is_ascii_digit()){atom}else if let Some(s)=db.macros.get(&atom.to_lowercase()){s.clone()}else{db.warnings.push(format!("Undefined string macro {atom}"));atom}},None=>return Err("Unexpected end of BibTeX value".into())};value.push_str(&part);self.ws();if self.peek()==Some('#'){self.next();}else{break}}Ok(value)}
}
impl Database {
    pub fn add(&mut self,input:&str)->Result<(),String>{let mut p=Parser {input,pos:0};while let Some(c)=p.next(){if c!='@'{continue}let kind=p.ident().to_lowercase();p.ws();let Some(open)=p.peek() else {break};if open!='{'&&open!='(' {continue}let close=if open=='{'{'}'}else{')'};
        if kind=="comment" {p.balanced(open,close)?;continue}p.next();p.ws();
        if kind=="preamble" {let value=p.value(self)?;self.preamble.push(value);p.ws();if p.peek()==Some(','){p.next();p.ws();}if p.next()!=Some(close){return Err("Unclosed BibTeX preamble".into())}continue}
        if kind=="string" {let key=p.ident().to_lowercase();p.ws();if p.next()!=Some('='){return Err("Invalid @string definition".into())}let value=p.value(self)?;self.macros.insert(key,value);p.ws();if p.peek()==Some(','){p.next();p.ws();}if p.next()!=Some(close){return Err("Unclosed @string".into())}continue}
        let key=p.ident();if key.is_empty(){return Err("BibTeX entry has an empty key".into())}let mut entry=Entry {key,kind,..Default::default()};p.ws();if p.peek()==Some(','){p.next();}
        loop {p.ws();if p.peek()==Some(close){p.next();break}let field=p.ident().to_lowercase();if field.is_empty(){return Err(format!("Invalid field in entry {} at byte {}",entry.key,p.pos))}p.ws();if p.next()!=Some('='){return Err(format!("Missing '=' after field {field} in {}",entry.key))}let value=p.value(self)?;entry.fields.entry(field).or_insert(value);p.ws();if p.peek()==Some(','){p.next();}else if p.peek()!=Some(close){return Err(format!("Missing comma in entry {}",entry.key))}}
        if self.entries.iter().any(|e|e.key==entry.key){self.warnings.push(format!("Duplicate entry key {}",entry.key));}else{self.entries.push(entry)}
    }Ok(())}
}

pub fn split_list(value:&str,sep:&str)->Vec<String>{let mut result=vec![];let mut depth=0usize;let mut escaped=false;let mut start=0;let mut p=0;while p<value.len(){let c=value[p..].chars().next().unwrap();if escaped{escaped=false;}else if c=='\\'{escaped=true;}else if c=='{'{depth+=1;}else if c=='}'{depth=depth.saturating_sub(1);}else if depth==0&&value[p..].starts_with(sep){result.push(value[start..p].trim().into());p+=sep.len();start=p;continue}p+=c.len_utf8();}result.push(value[start..].trim().into());result}

pub fn sourcemaps(control:&Control,entry:&mut Entry,source:&str){
    for map in &control.sourcemaps {
        if !map.per_type.is_empty()&&!map.per_type.contains(&entry.kind){continue}if !map.per_source.is_empty()&&!map.per_source.iter().any(|s|s==source){continue}
        let mut original=String::new();let mut original_field=String::new();let mut original_type=entry.kind.clone();let mut captures=Vec::<String>::new();
        for step in &map.steps {
            let get=|k:&str|step.get(k).map(String::as_str);
            let final_=get("map_final")==Some("1");
            if let Some(kind)=get("map_type_source"){if kind!=entry.kind {if final_{break}else{continue}}original_type=entry.kind.clone();if let Some(target)=get("map_type_target"){entry.kind=target.into();}}
            if let Some(field)=get("map_field_source"){
                let value=if field=="entrykey"{Some(entry.key.clone())}else{entry.fields.get(field).cloned()};
                let Some(value)=value else {if final_{break}else{continue}};original=value.clone();original_field=field.into();
                let mut replacement=value.clone();
                if let Some((name,pattern))=["map_match","map_matchi","map_notmatch","map_notmatchi"].into_iter().find_map(|name|get(name).map(|pattern|(name,pattern))){
                    let Some(re)=map.patterns.get(&format!("{name}:{pattern}"))else{continue};
                    let matched=re.is_match(&value);let negative=name.starts_with("map_not");
                    if matched==negative {if final_{break}else{continue}}
                    if !negative {captures=re.captures(&value).map(|m|m.iter().skip(1).map(|s|s.map_or("",|m|m.as_str()).to_owned()).collect()).unwrap_or_default();}
                    if let Some(replace)=get("map_replace"){replacement=re.replace_all(&value,replace).into_owned();}
                }
                if let Some(target)=get("map_field_target"){if map.overwrite||!entry.fields.contains_key(target){entry.fields.insert(target.into(),replacement);entry.fields.remove(field);}}
                else if get("map_replace").is_some()&&field!="entrykey"{entry.fields.insert(field.into(),replacement);}
            }
            if let Some(field)=get("map_field_set"){
                if get("map_null")==Some("1"){entry.fields.remove(field);continue}
                if !map.overwrite&&entry.fields.contains_key(field){if final_{break}else{continue}}
                let mut value=if get("map_origfieldval")==Some("1"){original.clone()}else if get("map_origfield")==Some("1"){original_field.clone()}else if get("map_origentrytype")==Some("1"){original_type.clone()}else{get("map_field_value").unwrap_or("").into()};
                for (i,capture) in captures.iter().enumerate(){value=value.replace(&format!("${}",i+1),capture);}
                if get("map_append")==Some("1"){entry.fields.entry(field.into()).or_default().push_str(&value);}else{entry.fields.insert(field.into(),value);}
            }
        }
    }
}

pub fn inherit(control:&Control,entries:&mut [Entry],warnings:&mut Vec<String>){
    let index=entries.iter().enumerate().map(|(i,e)|(e.key.clone(),i)).collect::<BTreeMap<_,_>>();let mut done=BTreeSet::new();
    fn visit(i:usize,c:&Control,entries:&mut [Entry],index:&BTreeMap<String,usize>,active:&mut BTreeSet<usize>,done:&mut BTreeSet<usize>,warnings:&mut Vec<String>){
        if done.contains(&i){return}if !active.insert(i){warnings.push(format!("Cyclic inheritance at {}",entries[i].key));return}
        let refs=["xdata","crossref","xref"].into_iter().flat_map(|field|entries[i].fields.get(field).map(|v|split_list(v,",")).unwrap_or_default().into_iter().map(move |key|(field,key))).collect::<Vec<_>>();
        for (field,key) in refs {let Some(&j)=index.get(&key)else{warnings.push(format!("Missing {field} entry {key} referenced by {}",entries[i].key));continue};visit(j,c,entries,index,active,done,warnings);let source=entries[j].clone();
            if field=="xref" {continue}
            let mut mapping:BTreeMap<String,Vec<(Option<String>,bool)>>=BTreeMap::new();
            if field!="xdata" {for rule in &c.inheritance {if rule.types.iter().any(|(s,t)|(s=="*"||s==&source.kind)&&(t=="*"||t==&entries[i].kind)){for (src,target) in &rule.fields{mapping.entry(src.clone()).or_default().push((target.clone(),rule.override_target));}}}}
            if field=="xdata" && source.kind!="xdata" {warnings.push(format!("Entry {} references non-XDATA entry {key}",entries[i].key));continue}
            let own_dates=c.fields.iter().filter(|(_,s)|s.datatype=="date").map(|(d,_)|d.strip_suffix("date").unwrap_or(d).to_owned()).filter(|p|entries[i].computed.contains_key(&format!("{p}datesplit"))).collect::<Vec<_>>();
            for (src,value) in &source.fields {
                if src=="xdata" || src=="ids" {continue}
                if field!="xdata" && c.fields.get(src).is_some_and(|s|s.datatype=="datepart") && own_dates.iter().any(|p|src.starts_with(p)&&["year","month","day","hour","minute","second","timezone","endyear","endmonth","endday","endhour","endminute","endsecond","endtimezone"].contains(&src[p.len()..].as_ref())) {continue}
                let targets=mapping.get(src).cloned().unwrap_or_else(||vec![(Some(src.clone()),field=="xdata")]);
                for (target,override_) in targets {if let Some(target)=target {if override_||!entries[i].fields.contains_key(&target){
                    entries[i].fields.insert(target.clone(),value.clone());
                    if let Some(names)=source.names.get(src){entries[i].names.insert(target.clone(),names.clone());}
                    if let Some(list)=source.lists.get(src){entries[i].lists.insert(target,list.clone());}
                }}}
            }
            if field=="crossref" {
                for (d,spec) in &c.fields {if spec.datatype!="date" {continue}let p=d.strip_suffix("date").unwrap_or(d);if own_dates.iter().any(|own|own==p){continue}
                    for (k,v) in &source.computed {if k.starts_with(p)&&["datesplit","era","endera","dayofyear","enddayofyear"].contains(&k[p.len()..].as_ref()){entries[i].computed.insert(k.clone(),v.clone());}}
                    for flag in &source.flags {if flag.starts_with(p)&&flag[p.len()..].contains("date"){entries[i].flags.insert(flag.clone());}}
                }
            }
        }
        entries[i].fields.remove("xdata");active.remove(&i);done.insert(i);
    }
    for i in 0..entries.len(){visit(i,control,entries,&index,&mut BTreeSet::new(),&mut done,warnings)}
}

pub fn normalize(control:&Control,entry:&mut Entry){
    let raw=entry.fields.clone();
    for (field,value) in &raw {if field!="date"&&control.fields.get(field).is_some_and(|s|s.datatype=="date"){crate::dates::parse(control,entry,field,value);}}
    for (field,value) in &raw {
        let Some(spec)=control.fields.get(field)else{continue};
        if spec.datatype=="name" {entry.names.insert(field.clone(),names::parse(&value));}
        else if spec.fieldtype=="list"&&spec.format!="xsv" {entry.lists.insert(field.clone(),split_list(&value," and ").into_iter().map(|s|names::decode_latex(&s)).collect());}
        else if spec.datatype=="date" {continue;}
        else if spec.datatype=="range" {entry.fields.insert(field.clone(),names::decode_latex(&value));}
        else if spec.format=="xsv" || spec.datatype=="keyword" {entry.fields.insert(field.clone(),split_list(&value,",").into_iter().map(|s|names::decode_latex(&s)).collect::<Vec<_>>().join(","));}
        else if !["verbatim","uri","entrykey"].contains(&spec.datatype.as_str()) {entry.fields.insert(field.clone(),names::decode_latex(&value));}
    }
    if let Some(value)=raw.get("date"){crate::dates::parse(control,entry,"date",value);}
    if let Some(month)=entry.fields.get("month").cloned(){if let Ok(n)=month.parse::<u32>() {if (1..=12).contains(&n){entry.fields.insert("month".into(),n.to_string());}else{entry.fields.remove("month");}}}
}
