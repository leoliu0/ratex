use crate::model::*;
use crate::names;
use std::collections::{BTreeMap,BTreeSet};

pub struct Database {pub entries:Vec<Entry>,pub preamble:Vec<String>,pub warnings:Vec<String>,macros:BTreeMap<String,String>}
impl Default for Database {fn default()->Self {Self {entries:vec![],preamble:vec![],warnings:vec![],macros:[("jan","1"),("feb","2"),("mar","3"),("apr","4"),("may","5"),("jun","6"),("jul","7"),("aug","8"),("sep","9"),("oct","10"),("nov","11"),("dec","12")].into_iter().map(|(a,b)|(a.into(),b.into())).collect()}}}
struct Parser<'a> {input:&'a str,pos:usize,line:usize}
impl<'a> Parser<'a> {
    fn peek(&self)->Option<char>{self.input[self.pos..].chars().next()}
    fn next(&mut self)->Option<char>{let c=self.peek()?;self.pos+=c.len_utf8();if c=='\n'{self.line+=1;}Some(c)}
    fn ws(&mut self){loop {while self.peek().is_some_and(char::is_whitespace){self.next();} if self.peek()==Some('%') {while self.next().is_some_and(|c|c!='\n') {}} else {break}}}
    fn ident(&mut self)->String {self.ws();let start=self.pos;while self.peek().is_some_and(|c|!c.is_whitespace()&&!"=,#{}()\"".contains(c)){self.next();}self.input[start..self.pos].into()}
    fn balanced(&mut self,open:char,close:char)->Result<String,String>{
        if self.next()!=Some(open){return Err("Expected opening BibTeX delimiter".into())}let start=self.pos;let mut depth=1usize;let mut escaped=false;
        while let Some(c)=self.next(){if escaped{escaped=false;continue}if c=='\\'{escaped=true;continue}if c==open {depth+=1;}else if c==close {depth-=1;if depth==0{return Ok(self.input[start..self.pos-c.len_utf8()].into())}}}
        Err("Unclosed BibTeX value".into())
    }
    fn quoted(&mut self)->Result<String,String>{self.next();let start=self.pos;let mut depth=0usize;let mut escaped=false;while let Some(c)=self.next(){if escaped{escaped=false;continue}if c=='\\'{escaped=true;}else if c=='{'{depth+=1;}else if c=='}' {depth=depth.saturating_sub(1);}else if c=='\"'&&depth==0{return Ok(self.input[start..self.pos-1].into())}}Err("Unclosed quoted BibTeX value".into())}
    fn value(&mut self,db:&mut Database)->Result<String,String>{let mut value=String::new();loop {self.ws();let part=match self.peek(){Some('{')=>self.balanced('{','}')?,Some('"')=>self.quoted()?,Some(_)=>{let line=self.line;let atom=self.ident();if atom.is_empty(){return Err(format!("Expected BibTeX value at byte {}",self.pos))}if atom.chars().all(|c|c.is_ascii_digit()){atom}else if let Some(s)=db.macros.get(&atom.to_lowercase()){s.clone()}else{db.warnings.push(format!("BibTeX subsystem: <converted datasource>, line {line}, warning: undefined macro \"{atom}\""));String::new()}},None=>return Err("Unexpected end of BibTeX value".into())};value.push_str(&part);self.ws();if self.peek()==Some('#'){self.next();}else{break}}Ok(value)}
}
impl Database {
    pub(crate) fn standard_macros(&mut self,disabled:bool){
        if disabled {for (key,value) in ["jan","feb","mar","apr","may","jun","jul","aug","sep","oct","nov","dec"].into_iter().zip(["January","February","March","April","May","June","July","August","September","October","November","December"]){self.macros.insert(key.into(),value.into());}}
    }
    pub fn add(&mut self,input:&str,source:&str)->Result<(),String>{let mut p=Parser {input,pos:0,line:1};while let Some(c)=p.next(){if c!='@'{continue}let kind=p.ident().to_lowercase();p.ws();let Some(open)=p.peek() else {break};if open!='{'&&open!='(' {continue}let close=if open=='{'{'}'}else{')'};
        if kind=="comment" {p.balanced(open,close)?;continue}p.next();p.ws();
        if kind=="preamble" {let value=p.value(self)?;self.preamble.push(value);p.ws();if p.peek()==Some(','){p.next();p.ws();}if p.next()!=Some(close){return Err("Unclosed BibTeX preamble".into())}continue}
        if kind=="string" {let key=p.ident().to_lowercase();p.ws();if p.next()!=Some('='){return Err("Invalid @string definition".into())}let value=p.value(self)?;self.macros.insert(key,value);p.ws();if p.peek()==Some(','){p.next();p.ws();}if p.next()!=Some(close){return Err("Unclosed @string".into())}continue}
        let key=p.ident();if key.is_empty(){return Err("BibTeX entry has an empty key".into())}let mut entry=Entry {key,kind,source:source.into(),..Default::default()};p.ws();if p.peek()==Some(','){p.next();}
        loop {p.ws();if p.peek()==Some(close){p.next();break}let field=p.ident().to_lowercase();if field.is_empty(){return Err(format!("Invalid field in entry {} at byte {}",entry.key,p.pos))}p.ws();if p.next()!=Some('='){return Err(format!("Missing '=' after field {field} in {}",entry.key))}let raw=bibtex_spaces(p.value(self)?);let normalized=crate::collation::normalization::normalize(&raw,"NFD");let value=match normalized{std::borrow::Cow::Borrowed(_)=>raw,std::borrow::Cow::Owned(value)=>value};entry.fields.entry(field).or_insert(value);p.ws();if p.peek()==Some(','){p.next();}else if p.peek()!=Some(close){return Err(format!("Missing comma in entry {}",entry.key))}}
        if self.entries.iter().any(|e|e.key==entry.key){self.warnings.push(format!("Duplicate entry key: '{}' in file '{source}', skipping ...",entry.key));}else{self.entries.push(entry)}
    }Ok(())}
}
// Text::BibTeX normalizes ASCII spaces and physical line breaks after macro
// concatenation. Tabs and Unicode spacing characters are data, not separators.
fn bibtex_spaces(value:String)->String{
    if !value.starts_with(' ')&&!value.ends_with(' ')&&!value.contains("  ")&&!value.chars().any(|c|matches!(c,'\r'|'\n'|'\x0b'|'\x0c'|'\u{85}'|'\u{2028}'|'\u{2029}')){return value;}
    let mut out=String::with_capacity(value.len());let mut pending=false;
    for c in value.chars(){
        if matches!(c,' '|'\r'|'\n'|'\x0b'|'\x0c'|'\u{85}'|'\u{2028}'|'\u{2029}'){pending=!out.is_empty();}
        else{if pending{out.push(' ');pending=false;}out.push(c);}
    }
    out
}

/// parse_decode: Biber latex_decodes every field of the .bib, except data model
/// verbatim/uri fields that are not skipout, before sourcemaps run; btparse then
/// re-reads each decoded `{value}`, collapsing whitespace again.
pub fn decode_entries(control:&Control,entries:&mut [Entry]){
    for entry in entries {for (field,value) in entry.fields.iter_mut() {
        if control.fields.get(field).is_some_and(|spec|matches!(spec.datatype.as_str(),"verbatim"|"uri")&&!spec.skip_output){continue;}
        *value=bibtex_spaces(names::decode_latex(value));
    }}
}
pub fn split_list(value:&str,sep:&str)->Vec<String>{let mut result=vec![];let mut depth=0usize;let mut escaped=false;let mut start=0;let mut p=0;while p<value.len(){let c=value[p..].chars().next().unwrap();if escaped{escaped=false;}else if c=='\\'{escaped=true;}else if c=='{'{depth+=1;}else if c=='}'{depth=depth.saturating_sub(1);}else if !sep.is_empty()&&depth==0&&value[p..].starts_with(sep){result.push(value[start..p].trim().into());p+=sep.len();start=p;continue}p+=c.len_utf8();}result.push(value[start..].trim().into());result}

/// Maps run on the datasource entries before typed field instantiation. Created
/// entries are instantiated directly, not recursively subjected to sourcemaps.
pub fn sourcemaps(control:&Control,section:&mut Section,entries:&mut Vec<Entry>,warnings:&mut Vec<String>)->Result<(),String>{
    let allkeys=section.citekeys.iter().any(|key|key=="*");
    let mut wanted=section.citekeys.iter().cloned().collect::<BTreeSet<_>>();
    let explicit=wanted.clone();
    if allkeys {section.citekeys.retain(|key|key!="*");}
    let mut done=BTreeSet::new();
    let mut deleted=BTreeSet::new();
    let mut created=Vec::new();
    let mut unique=String::new();
    loop {
        let next=(0..entries.len()).find(|i|!done.contains(i) && (allkeys || wanted.contains(&entries[*i].key) || entries[*i].fields.get("ids").is_some_and(|ids|split_list(ids,",").iter().any(|id|wanted.contains(id)))));
        let Some(i)=next else {break};
        done.insert(i);
        let (keep,mut new)=map_entry(control,section,&mut entries[i],warnings,allkeys,&mut unique)?;
        if !keep {
            deleted.insert(i);
            if wanted.contains(&entries[i].key) {warnings.push(format!("Entry with key '{}' in section '{}' is cited and found but not created (likely due to sourcemap)",entries[i].key,section.number));}
            section.citekeys.retain(|key|key!=&entries[i].key);
        } else {
            // orig_key_order holds every created entry, skipout types (XDATA) included.
            if allkeys {
                map_add_key(section,&entries[i].key);
                if !explicit.contains(&entries[i].key)&&section.nocite.contains("*") {section.nocite.insert(entries[i].key.clone());}
            }
            for e in std::iter::once(&entries[i]).chain(new.iter()) {
                if allkeys&&!explicit.contains(&e.key)&&section.nocite.contains("*") {section.nocite.insert(e.key.clone());}
                for field in ["crossref","xref","xdata","related","entryset"] {
                    if let Some(value)=e.fields.get(field) {wanted.extend(split_list(value,","));}
                }
            }
            wanted.extend(section.citekeys.iter().cloned());
            created.append(&mut new);
        }
    }
    let mut index=0;entries.retain(|_|{let keep=!deleted.contains(&index);index+=1;keep});
    if allkeys {
        // Biber clears allkeys citekeys, then per datasource adds map-created
        // keys during entry creation followed by the surviving .bib order.
        let mut sources=section.sources.clone();
        for e in entries.iter() {if !sources.contains(&e.source){sources.push(e.source.clone());}}
        let mut seen=std::collections::HashSet::new();let mut order=Vec::new();
        for source in &sources {
            for (_,key) in section.created_keys.iter().filter(|(s,_)|s==source) {if seen.insert(key.clone()){order.push(key.clone());}}
            for e in entries.iter().filter(|e|&e.source==source) {if seen.insert(e.key.clone()){order.push(e.key.clone());}}
        }
        section.allkeys_order=order;
    }
    for entry in created {
        if let Some(existing)=entries.iter_mut().find(|e|e.key==entry.key) {*existing=entry;}
        else {entries.push(entry);}
    }
    Ok(())
}

fn map_entry(control:&Control,section:&mut Section,entry:&mut Entry,warnings:&mut Vec<String>,allkeys:bool,mut unique:&mut String)->Result<(bool,Vec<Entry>),String>{
    let key=entry.key.clone();
    let source=if std::path::Path::new(&entry.source).is_absolute() {std::path::Path::new(&entry.source).file_name().and_then(|s|s.to_str()).unwrap_or(&entry.source)} else {&entry.source};
    let source=source.to_owned();
    let datatype=entry.computed.get("sourcemap_datatype").cloned().unwrap_or_else(||"bibtex".into());
    let mut newentries=BTreeMap::<String,Entry>::new();
    for map in &control.sourcemaps {
        if map.datatype!=datatype || map.refsection.is_some_and(|n|n!=section.number) || (!map.per_type.is_empty()&&!map.per_type.contains(&entry.kind.to_lowercase())) || map.per_not_type.contains(&entry.kind.to_lowercase()) || (!map.per_source.is_empty()&&(entry.computed.contains_key("sourcemap_remote")||!map.per_source.contains(&source))) {continue;}
        let mut last_type=entry.kind.clone();let mut last_field=String::new();let mut last_value=String::new();let mut captures=Vec::new();
        let loops=map.foreach.as_ref().map(|foreach|map.fieldsets.get(&foreach.to_lowercase()).cloned().unwrap_or_else(||map_csv(entry.fields.get(&foreach.to_lowercase()).filter(|v|map_truth(v)).unwrap_or(foreach)))).unwrap_or_else(||vec![String::new()]);
        'maploop: for maploop in loops {
            for (step_index,step) in map.steps.iter().enumerate() {
                let get=|k:&str|step.get(k).map(String::as_str);
                let yes=|k:&str|get(k).is_some_and(map_truth);
                let error=|e:String|format!("Source mapping (type={}, key={key}, step={}): {e}",map.level,step_index+1);
                macro_rules! fail {() => {if yes("map_final") {continue 'maploop;} else {continue;}}}
                if yes("map_entry_null") {return Ok((false,Vec::new()));}
                if let Some(newkey)=maploop_value(get("map_entry_new"),&maploop,&mut unique).filter(|v|map_truth(v)) {
                    let newkey=crate::perl_regex::saved_captures(&newkey,&captures,false);
                    let Some(kind)=maploop_value(get("map_entry_newtype"),&maploop,&mut unique).filter(|v|map_truth(v)) else {warnings.push(format!("Source mapping (type={}, key={key}): Missing type for new entry '{newkey}', skipping step ...",map.level));continue;};
                    newentries.insert(newkey.clone(),Entry {key:newkey.clone(),kind:kind.to_lowercase(),source:entry.source.clone(),..Default::default()});
                    if yes("map_entry_nocite") {map_nocite(section,&newkey);}
                    if allkeys {map_created_key(section,&entry.source,&newkey);}
                }
                if let Some(clonekey)=maploop_value(get("map_entry_clone"),&maploop,&mut unique).filter(|v|map_truth(v)) {
                    let clonekey=crate::perl_regex::saved_captures(&clonekey,&captures,false);
                    let mut clone=entry.clone();clone.computed.insert("sourcemap_entrykey".into(),entry.key.clone());clone.key=clonekey.clone();newentries.insert(clonekey.clone(),clone);
                    if yes("map_entry_nocite") {map_nocite(section,&clonekey);}
                    if allkeys {map_created_key(section,&entry.source,&clonekey);}
                }
                let targetkey=maploop_value(get("map_entrytarget"),&maploop,&mut unique).filter(|v|map_truth(v)).map(|v|crate::perl_regex::saved_captures(&v,&captures,false));
                if let Some(target)=&targetkey {
                    if !newentries.contains_key(target) {warnings.push(format!("Source mapping (type={}, key={key}): Dynamically created entry target '{target}' does not exist skipping step ...",map.level));continue;}
                }
                // BibTeX's fieldtarget operation reads from the original entry,
                // even when entrytarget selects a dynamically created entry.
                let (etarget,original_fields)=if let Some(target)=&targetkey {(newentries.get_mut(target).unwrap(),Some(&entry.fields))} else {(&mut *entry,None)};
                if let Some(kind)=maploop_value(get("map_type_source"),&maploop,&mut unique).filter(|v|map_truth(v)) {
                    if etarget.kind!=kind.to_lowercase() {fail!();}
                    last_type=etarget.kind.clone();
                    etarget.kind=maploop_value(get("map_type_target"),&maploop,&mut unique).unwrap_or_default().to_lowercase();
                }
                let notfield=maploop_value(get("map_notfield"),&maploop,&mut unique).filter(|v|map_truth(v)).map(|v|v.to_lowercase());
                if notfield.as_ref().is_some_and(|field|etarget.fields.contains_key(field)) {fail!();}
                let cited=section.citekeys.contains(&key)&&!section.nocite.contains(&key);
                let nocited=section.nocite.contains(&key);
                let allnocited=section.nocite.contains("*");
                if (yes("map_entrykey_citedornocited")&&!section.citekeys.contains(&key)) || (yes("map_entrykey_cited")&&!cited) || (yes("map_entrykey_nocited")&&(cited || (!nocited&&!allnocited))) || (yes("map_entrykey_allnocited")&&!allnocited) || (yes("map_entrykey_starnocited")&&allnocited&&(cited||nocited)) {fail!();}
                let fieldsource=maploop_value(get("map_field_source"),&maploop,&mut unique).filter(|v|map_truth(v)).map(|v|v.to_lowercase());
                if let Some(field)=&fieldsource {
                    if field!="entrykey"&&!etarget.fields.contains_key(field) {fail!();}
                }
                if fieldsource.is_some() || notfield.is_some() {
                    last_field=fieldsource.clone().unwrap_or_default();
                    // Text::BibTeX::Entry::get re-normalizes to the entry's NFD even after an NFC set.
                    last_value=if last_field=="entrykey" {etarget.computed.get("sourcemap_entrykey").unwrap_or(&etarget.key).clone()} else {etarget.fields.get(&last_field).map(|v|crate::collation::normalization::normalize(v,"NFD").into_owned()).unwrap_or_default()};
                    if let Some((name,matches))=["map_matchesi","map_matches"].into_iter().find_map(|name|get(name).filter(|v|map_truth(v)).map(|v|(name,v))) {
                        let matches=map_csv(matches);let replacements=map_csv(get("map_replace").unwrap_or(""));
                        if matches.len()!=replacements.len() {continue;}
                        for (m,r) in matches.iter().zip(replacements) {if &last_value==m || (name.ends_with('i')&&last_value.to_lowercase()==m.to_lowercase()) {etarget.fields.insert(last_field.clone(),r);}}
                    }
                    let matchspec=["map_matchi","map_notmatchi","map_notmatch","map_match"].into_iter().find_map(|name|get(name).filter(|v|map_truth(v)).map(|v|(name,v)));
                    if let Some((name,pattern))=matchspec {
                        let pattern=maploop_value(Some(pattern),&maploop,&mut unique).unwrap();
                        let pattern=if get("map_replace").is_none() {crate::perl_regex::saved_captures(&pattern,&captures,true)} else {pattern};
                        let compiled;
                        let re=if let Some(re)=map.patterns.get(&format!("{name}:{pattern}")) {re} else {
                            compiled=crate::perl_regex::Regex::compile(&pattern,name.ends_with('i')).map_err(&error)?;&compiled
                        };
                        if let Some(replacement)=get("map_replace") {
                            if last_field=="entrykey" {continue;}
                            let replacement=maploop_value(Some(replacement),&maploop,&mut unique).unwrap();
                            let value=re.replace_all(&last_value,&replacement).map_err(&error)?;
                            etarget.fields.insert(last_field.clone(),crate::collation::normalization::normalize(&value,"NFC").into_owned());
                        } else {
                            captures=re.match_all(&last_value,yes("map_notmatch")||yes("map_notmatchi")).map_err(&error)?;
                            if captures.is_empty() {fail!();}
                        }
                    }
                    if let Some(target)=maploop_value(get("map_field_target"),&maploop,&mut unique).filter(|v|map_truth(v)).map(|v|v.to_lowercase()) {
                        if last_field=="entrykey" || (!map.overwrite&&etarget.fields.contains_key(&target)) {continue;}
                        let value=original_fields.unwrap_or(&etarget.fields).get(&last_field).cloned().unwrap_or_default();
                        etarget.fields.insert(target.clone(),value);
                        etarget.fields.remove(&last_field);
                        if let Some(names)=etarget.names.remove(&last_field) {etarget.names.insert(target.clone(),names);}
                        if let Some(list)=etarget.lists.remove(&last_field) {etarget.lists.insert(target,list);}
                    }
                }
                if let Some(field)=maploop_value(get("map_field_set"),&maploop,&mut unique).filter(|v|map_truth(v)).map(|v|v.to_lowercase()) {
                    if yes("map_null") {etarget.fields.remove(&field);etarget.names.remove(&field);etarget.lists.remove(&field);continue;}
                    if !map.overwrite&&etarget.fields.contains_key(&field) {fail!();}
                    let orig=if yes("map_append")||yes("map_appendstrict") {etarget.fields.get(&field).cloned().unwrap_or_default()} else {String::new()};
                    let value=if yes("map_origentrytype") {if !map_truth(&last_type) {continue;}last_type.clone()}
                        else if yes("map_origfieldval") {if !map_truth(&last_value) {continue;}last_value.clone()}
                        else if yes("map_origfield") {if !map_truth(&last_field) {continue;}last_field.clone()}
                        else {crate::perl_regex::saved_captures(&maploop_value(get("map_field_value"),&maploop,&mut unique).unwrap_or_default(),&captures,false)};
                    let value=if yes("map_appendstrict")&&!map_truth(&orig) {String::new()} else {orig+&value};
                    etarget.fields.insert(field,crate::collation::normalization::normalize(&value,"NFC").into_owned());
                }
            }
        }
    }
    Ok((true,newentries.into_values().collect()))
}
pub(crate) fn map_truth(value:&str)->bool {!value.is_empty()&&value!="0"}
pub(crate) fn map_csv(value:&str)->Vec<String>{
    let mut values=value.split(',').enumerate().map(|(i,v)|if i==0 {v.trim_end().to_owned()} else {v.trim().to_owned()}).collect::<Vec<_>>();
    while values.last().is_some_and(String::is_empty) {values.pop();}
    values
}
pub(crate) fn map_nocite(section:&mut Section,key:&str){
    section.nocite.insert(key.into());map_add_key(section,key);
}
/// `Section::add_citekeys`: no keyorder is recorded for keys that were not cited.
pub(crate) fn map_add_key(section:&mut Section,key:&str){
    if !section.citekeys.iter().any(|k|k==key) {section.citekeys.push(key.into());}
}
/// A key created by an allkeys sourcemap in `source`.
pub(crate) fn map_created_key(section:&mut Section,source:&str,key:&str){
    map_add_key(section,key);
    if !section.created_keys.iter().any(|(_,k)|k==key) {section.created_keys.push((source.into(),key.into()));}
}
thread_local! {static MAP_UNIQUE:std::cell::RefCell<String>=const {std::cell::RefCell::new(String::new())};}
pub(crate) fn reset_sourcemap_unique(){MAP_UNIQUE.with(|unique|unique.borrow_mut().clear());}
pub(crate) fn maploop_value(value:Option<&str>,maploop:&str,unique:&mut String)->Option<String>{
    let value=value?;
    if !map_truth(maploop) {return Some(value.into());}
    let mut value=MAP_UNIQUE.with(|latest|value.replace("$MAPLOOP",maploop).replace("$MAPUNIQVAL",&latest.borrow()));
    if value.contains("$MAPUNIQ") {
        static NEXT:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
        let time=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64;
        let mut old=NEXT.load(std::sync::atomic::Ordering::Relaxed);
        while let Err(current)=NEXT.compare_exchange_weak(old,old.max(time).wrapping_add(1),std::sync::atomic::Ordering::Relaxed,std::sync::atomic::Ordering::Relaxed) {old=current;}
        let mut number=old.max(time).wrapping_add(1);
        let alphabet=b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        let mut bytes=[b'0';10];
        for byte in bytes.iter_mut().rev() {*byte=alphabet[(number%62) as usize];number/=62;}
        *unique=String::from_utf8(bytes.to_vec()).unwrap();
        MAP_UNIQUE.with(|latest|latest.borrow_mut().clone_from(unique));
        value=value.replace("$MAPUNIQ",unique);
    }
    Some(value)
}

pub fn inherit(control:&Control,entries:&mut [Entry],warnings:&mut Vec<String>){
    let mut index=entries.iter().enumerate().map(|(i,e)|(e.key.clone(),i)).collect::<BTreeMap<_,_>>();
    for (i,e) in entries.iter().enumerate(){if let Some(ids)=e.fields.get("ids"){for id in split_list(ids,","){index.entry(id).or_insert(i);}}}
    let mut done=BTreeSet::new();
    fn visit(i:usize,c:&Control,entries:&mut [Entry],index:&BTreeMap<String,usize>,active:&mut BTreeSet<usize>,done:&mut BTreeSet<usize>,warnings:&mut Vec<String>){
        if done.contains(&i){return}if !active.insert(i){warnings.push(format!("Cyclic inheritance at {}",entries[i].key));return}
        let refs=["xdata","crossref","xref"].into_iter().flat_map(|field|entries[i].fields.get(field).map(|v|split_list(v,",")).unwrap_or_default().into_iter().map(move |key|(field,key))).collect::<Vec<_>>();
        for (field,key) in refs {
            let Some(&j)=index.get(&key)else{if field=="xdata"{xdata_warning(&mut entries[i],&key,"which does not exist");}else{warnings.push(format!("Missing {field} entry {key} referenced by {}",entries[i].key));}continue};
            if active.contains(&j){warnings.push(format!("Cyclic inheritance at {}",entries[i].key));continue;}
            visit(j,c,entries,index,active,done,warnings);let source=entries[j].clone();
            if field=="xref" {continue}
            if field=="xdata"&&source.kind!="xdata"{xdata_warning(&mut entries[i],&key,"which is not an XDATA entry");continue;}
            let default=|name:&str,fallback:bool| {
                [(source.kind.as_str(),entries[i].kind.as_str()),(source.kind.as_str(),"*"),("*",entries[i].kind.as_str()),("*","*")].into_iter().find_map(|(s,t)|c.options.get(&format!("inheritance.{name}.{s}.{t}"))).map(|s|matches!(s.as_str(),"1"|"true")).unwrap_or(fallback)
            };
            let inherit_all=field=="xdata"||default("inherit_all",true);let default_override=field=="xdata"||default("override_target",false);
            let mut mapping:BTreeMap<String,Vec<(Option<String>,bool)>>=BTreeMap::new();
            if field!="xdata" {for rule in &c.inheritance {if rule.types.iter().any(|(s,t)|(s=="*"||s==&source.kind)&&(t=="*"||t==&entries[i].kind)){for (src,target) in &rule.fields{mapping.entry(src.clone()).or_default().push((target.clone(),target.as_ref().is_some_and(|target|rule.overrides.contains(&(src.clone(),target.clone())))));}}}}
            let dateparts=["year","month","day","hour","minute","second","timezone","yeardivision","endyear","endmonth","endday","endhour","endminute","endsecond","endtimezone","endyeardivision"];
            let noinherit=crate::process::option(c,&entries[i],"noinherit");
            let noinherit=c.options.get(&format!("datafieldset.{noinherit}")).map(String::as_str).unwrap_or("");
            let noinherit=noinherit.split(',').map(str::to_owned).collect::<BTreeSet<_>>();
            let mut processed=BTreeSet::new();
            for (src,targets) in &mapping {
                if !source.fields.contains_key(src){continue;}
                for (target,override_) in targets {
                    if target.as_ref().is_some_and(|target|noinherit.contains(target)){continue;}
                    processed.insert(src.clone());
                    if let Some(target)=target {if *override_||!entries[i].fields.contains_key(target){inherit_datafield(&source,&mut entries[i],src,target);}}
                }
            }
            let own_dates=if source.computed.contains_key("datesplit")&&entries[i].computed.contains_key("datesplit"){
                c.fields.iter().filter(|(_,s)|s.datatype=="date").map(|(d,_)|d.strip_suffix("date").unwrap_or(d).to_owned()).filter(|p|{
                    let has_parts=|entry:&Entry|dateparts.iter().any(|part|{let name=format!("{p}{part}");entry.fields.contains_key(&name)&&!entry.computed.contains_key(&format!("derivedfield:{name}"))});
                    has_parts(&source)&&has_parts(&entries[i])
                }).collect::<Vec<_>>()
            }else{Vec::new()};
            if field!="xdata"&&inherit_all&&default_override {
                for prefix in &own_dates {for part in dateparts {
                    let name=format!("{prefix}{part}");
                    if !entries[i].computed.contains_key(&format!("derivedfield:{name}")){
                        entries[i].fields.remove(&name);
                        if let Some(value)=entries[i].computed.remove(&format!("shadowderivedfield:{name}")){entries[i].fields.insert(name.clone(),value);entries[i].computed.insert(format!("derivedfield:{name}"),"1".into());}
                    }
                }}
            }
            // CROSSREF copies derived date scopes before copying remaining datafields; XDATA never does.
            if field!="xdata"&&inherit_all {for (d,spec) in &c.fields {if spec.datatype!="date"{continue;}let p=d.strip_suffix("date").unwrap_or(d);
                for (k,v) in &source.computed {if k.starts_with(p)&&["datesplit","era","endera"].contains(&k[p.len()..].as_ref()){entries[i].computed.insert(k.clone(),v.clone());}}
                for flag in &source.flags {if flag.starts_with(p)&&flag[p.len()..].contains("date"){entries[i].flags.insert(flag.clone());}}
                for part in ["dateunspecified","yeardivision","endyeardivision"] {
                    let name=format!("{p}{part}");let marker=format!("derivedfield:{name}");let shadow=format!("shadowderivedfield:{name}");
                    let value=if source.computed.contains_key(&marker){source.fields.get(&name)}else{source.computed.get(&shadow)};
                    if let Some(value)=value {crate::dates::derived_field(&mut entries[i],name,value.clone());}
                }
            }}
            if inherit_all {for src in source.fields.keys() {
                if src=="xdata"||src=="ids"||!c.fields.contains_key(src)||processed.contains(src)||source.computed.contains_key(&format!("derivedfield:{src}")){continue;}
                if field!="xdata"&&!default_override&&c.fields.get(src).is_some_and(|s|s.datatype=="datepart")&&own_dates.iter().any(|p|src.starts_with(p)&&dateparts.contains(&src[p.len()..].as_ref())){continue;}
                if !noinherit.contains(src)&&(default_override||!entries[i].fields.contains_key(src)){inherit_datafield(&source,&mut entries[i],src,src);}
            }}
            if field!="xdata" {for (d,spec) in &c.fields {if spec.datatype=="date"{let p=d.strip_suffix("date").unwrap_or(d);let key=format!("{p}datesplit");if let Some(value)=source.computed.get(&key){entries[i].computed.insert(key,value.clone());}}}}
        }
        let granular=granular_refs(c,&entries[i]);
        let mut offsets=BTreeMap::<String,isize>::new();
        for (field,position,key,sourcefield,sourceposition) in granular {
            let Some(&j)=index.get(&key)else{xdata_warning(&mut entries[i],&key,"which does not exist");continue};
            if active.contains(&j){warnings.push(format!("Cyclic inheritance at {}",entries[i].key));continue;}
            visit(j,c,entries,index,active,done,warnings);let source=entries[j].clone();
            if source.kind!="xdata"{xdata_warning(&mut entries[i],&key,"which is not an XDATA entry");continue;}
            let section=entries[i].computed.get("refsection").map(String::as_str).unwrap_or("0");
            if c.fields.get(&field).zip(c.fields.get(&sourcefield)).is_none_or(|(a,b)|a.fieldtype!=b.fieldtype||a.datatype!=b.datatype) {
                entries[i].warnings.push(format!("Field '{field}' in {} entry '{}' which xdata references field '{sourcefield}' in entry '{key}' are not the same types, not resolving (section {section})",entries[i].kind,entries[i].key));continue;
            }
            if !source.fields.contains_key(&sourcefield){entries[i].warnings.push(format!("Field '{field}' in {} entry '{}' references XDATA field '{sourcefield}' in entry '{key}' and this field does not exist, not resolving (section {section})",entries[i].kind,entries[i].key));continue;}
            let targetposition=(position as isize+offsets.get(&field).copied().unwrap_or(0)).max(0) as usize;
            let mut count=1;
            if let Some(list)=source.names.get(&sourcefield){
                let values=if sourceposition=="*"{list.names.clone()}else{list.names.get(sourceposition.parse::<usize>().unwrap_or(1).saturating_sub(1)).cloned().into_iter().collect()};
                count=values.len();if count>0 {if let Some(target)=entries[i].names.get_mut(&field){target.names.splice(targetposition..targetposition+1,values);}}
            }else if let Some(list)=source.lists.get(&sourcefield){
                let values=if sourceposition=="*"{list.clone()}else{list.get(sourceposition.parse::<usize>().unwrap_or(1).saturating_sub(1)).cloned().into_iter().collect()};
                count=values.len();if count>0 {if let Some(target)=entries[i].lists.get_mut(&field){target.splice(targetposition..targetposition+1,values);}}
            }else{
                entries[i].fields.insert(field.clone(),source.fields[&sourcefield].clone());
                if let Some(range)=source.ranges.get(&sourcefield){entries[i].ranges.insert(field.clone(),range.clone());}
            }
            if count==0 {let section=entries[i].computed.get("refsection").map(String::as_str).unwrap_or("0");entries[i].warnings.push(format!("Field '{field}' in {} entry '{}' references field '{sourcefield}' position {sourceposition} in entry '{key}' and this position does not exist, not resolving (section {section})",entries[i].kind,entries[i].key));continue;}
            if source.names.contains_key(&sourcefield)||source.lists.contains_key(&sourcefield){for n in 0..count {let sourceindex=if sourceposition=="*"{n+1}else{sourceposition.parse().unwrap_or(1)};inherit_annotations(&source,&mut entries[i],&sourcefield,&field,Some((sourceindex,targetposition+n+1)));}*offsets.entry(field).or_default()+=count as isize-1;}else{inherit_annotations(&source,&mut entries[i],&sourcefield,&field,None);}
        }
        entries[i].fields.remove("xdata");active.remove(&i);done.insert(i);
    }
    for i in 0..entries.len(){visit(i,control,entries,&index,&mut BTreeSet::new(),&mut done,warnings)}
}
fn xdata_warning(entry:&mut Entry,key:&str,reason:&str) {
    let section=entry.computed.get("refsection").map(String::as_str).unwrap_or("0");
    entry.warnings.push(format!("{} entry '{}' references XDATA entry '{key}' {reason}, not resolving (section {section})",entry.kind,entry.key));
}
fn inherit_datafield(source:&Entry,target:&mut Entry,sourcefield:&str,targetfield:&str){
    crate::dates::authored_field(target,targetfield);
    target.fields.insert(targetfield.to_owned(),source.fields[sourcefield].clone());
    if let Some(names)=source.names.get(sourcefield){target.names.insert(targetfield.to_owned(),names.clone());}
    if let Some(list)=source.lists.get(sourcefield){target.lists.insert(targetfield.to_owned(),list.clone());}
    if let Some(range)=source.ranges.get(sourcefield){target.ranges.insert(targetfield.to_owned(),range.clone());}
    inherit_annotations(source,target,sourcefield,targetfield,None);
}
fn inherit_annotations(source:&Entry,target:&mut Entry,sourcefield:&str,targetfield:&str,indices:Option<(usize,usize)>) {
    let scope=|a:&Annotation|if !a.part.is_empty(){2}else if !a.item.is_empty(){1}else{0};
    // Perl copies the field/item/part maps separately, replacing only scopes present in the source.
    target.annotations.retain(|a|{
        if a.field!=targetfield{return true;}
        !source.annotations.iter().any(|s|{
            s.field==sourcefield&&scope(s)==scope(a)&&match indices {
                None=>true,
                Some((sourceitem,targetitem))=>s.item.parse::<usize>().ok()==Some(sourceitem)&&a.item.parse::<usize>().ok()==Some(targetitem)&&s.name==a.name,
            }
        })
    });
    for annotation in source.annotations.iter().filter(|a|a.field==sourcefield) {
        if indices.is_some_and(|(source,_)|annotation.item.parse::<usize>().ok()!=Some(source)){continue;}
        let mut annotation=annotation.clone();annotation.field=targetfield.into();if let Some((_,item))=indices{annotation.item=item.to_string();}
        target.annotations.retain(|a|!(a.field==annotation.field&&a.name==annotation.name&&a.item==annotation.item&&a.part==annotation.part));target.annotations.push(annotation);
    }
}
fn granular_refs(c:&Control,entry:&Entry)->Vec<(String,usize,String,String,String)> {
    let marker=c.options.get("xdatamarker").map(String::as_str).unwrap_or("xdata");let separator=c.options.get("xdatasep").map(String::as_str).unwrap_or("-");let name_separator=c.options.get("xnamesep").map(String::as_str).unwrap_or("=");
    let parse=|field:&str,position:usize,value:&str| {
        let (prefix,value)=value.trim().split_once(name_separator)?;if !prefix.eq_ignore_ascii_case(marker){return None;}
        let mut parts=value.split(separator);let key=parts.next()?;let sourcefield=parts.next()?;let sourceposition=parts.next().unwrap_or("*");if parts.next().is_some(){return None;}
        Some((field.into(),position,key.into(),sourcefield.into(),sourceposition.into()))
    };
    let mut refs=Vec::new();for (field,value) in &entry.fields {
        if let Some(names)=entry.names.get(field){for (i,name) in names.names.iter().enumerate(){if let Some(value)=name.options.get("xdata"){if let Some(r)=parse(field,i,value){refs.push(r);}}}}
        else if let Some(list)=entry.lists.get(field){for (i,value) in list.iter().enumerate(){if let Some(r)=parse(field,i,value){refs.push(r);}}}
        else if c.fields.get(field).is_some_and(|s|s.format!="xsv"){if let Some(r)=parse(field,0,value){refs.push(r);}}
    }
    refs
}

pub fn normalize(control:&Control,entry:&mut Entry){
    let raw=entry.fields.clone();
    parse_annotations(control,entry,&raw);
    // create_entry runs no field handler for an empty Text::BibTeX value (e.g. `title = {}`).
    entry.fields.retain(|field,value|control.fields.contains_key(field)&&(field=="options"||!value.is_empty()));
    let raw:std::collections::BTreeMap<String,String>=raw.into_iter().filter(|(field,value)|field=="options"||!value.is_empty()).collect();
    if let Some(options)=raw.get("options"){entry.fields.insert("options".into(),names::expand_options(control,"ENTRY",options).into_iter().map(|(k,v)|format!("{k}={v}")).collect::<Vec<_>>().join(","));}
    for (field,value) in &raw {if control.fields.get(field).is_some_and(|s|s.datatype=="date"){crate::dates::parse(control,entry,field,value);}}
    // _literal returns before warning when a split date already set a true year/month.
    let mut split_wins=Vec::new();
    for (field,value) in &raw {
        let Some(spec)=control.fields.get(field)else{continue};
        if field=="options"{continue;}
        crate::dates::authored_field(entry,field);
        if matches!(field.as_str(),"year"|"month")&&entry.computed.contains_key("datesplit")&&entry.fields.get(field).is_some_and(|s|!s.is_empty()&&s!="0"){split_wins.push(field.as_str());continue;}
        if spec.datatype=="name" {entry.names.insert(field.clone(),names::parse(control,value));}
        else if spec.fieldtype=="list"&&spec.format!="xsv" {entry.lists.insert(field.clone(),names::split_words(value,control.options.get("listsep").filter(|s|!s.is_empty()).map(String::as_str).unwrap_or("and")).into_iter().map(|s|if names::has_outer(s){&s[1..s.len()-1]}else{s}.to_owned()).collect());}
        else if spec.datatype=="date" {continue;}
        else if spec.format=="xsv" || spec.datatype=="keyword" {entry.fields.insert(field.clone(),xsv(control,value).join(","));}
        else if !["verbatim","uri","entrykey"].contains(&spec.datatype.as_str()) {entry.fields.insert(field.clone(),value.clone());}
    }
    if let Some(month)=entry.fields.get("month").cloned(){
        let value=match month.to_ascii_lowercase().as_str(){"jan"|"january"=>"1","feb"|"february"=>"2","mar"|"march"=>"3","apr"|"april"=>"4","may"=>"5","jun"|"june"=>"6","jul"|"july"=>"7","aug"|"august"=>"8","sep"|"september"=>"9","oct"|"october"=>"10","nov"|"november"=>"11","dec"|"december"=>"12",_=>&month};
        entry.fields.insert("month".into(),value.parse::<u32>().map(|n|n.to_string()).unwrap_or_else(|_|value.into()));
    }
    for field in ["year","month"]{if split_wins.contains(&field){continue;}if let Some(value)=raw.get(field){if !value.is_empty()&&value!="0"&&!crate::validation::numeric(value){entry.input_warnings.push(format!("legacy {field} field '{value}' in entry '{}' is not an integer - this will probably not sort properly.",entry.key));}}}
    if let Some(value)=entry.fields.get("isbn"){if !valid_isbn(value){entry.input_warnings.push(format!("ISBN '{value}' in entry '{}' is invalid - run biber with '--validate_datamodel' for details.",entry.key));}}
}
fn valid_isbn(value:&str)->bool {
    let digits=value.chars().filter(|c|c.is_ascii_digit()||matches!(c,'X'|'x')).collect::<String>();
    if digits.len()==10 {let mut sum=0;for (i,c) in digits.chars().enumerate(){let Some(n)=c.to_digit(10).or_else(||(i==9&&matches!(c,'X'|'x')).then_some(10))else{return false};sum+=(10-i as u32)*n;}sum%11==0}
    else if digits.len()==13 {let mut sum=0;for (i,c) in digits.chars().enumerate(){let Some(n)=c.to_digit(10)else{return false};sum+=n*if i%2==0{1}else{3};}matches!(&digits[..3],"978"|"979")&&sum%10==0}else{false}
}
pub(crate) fn xsv(control:&Control,value:&str)->Vec<String> {
    let pattern=control.options.get("xsvsep").map(String::as_str).unwrap_or(r"\s*,\s*");
    let mut result=names::with_text_rule(pattern,|rule|{let mut result=Vec::new();let mut start=0;for found in rule.inner.find_iter(value).flatten(){result.push(value[start..found.start()].into());start=found.end();}result.push(value[start..].into());result}).unwrap_or_else(||vec![value.into()]);
    while result.last().is_some_and(String::is_empty){result.pop();}result
}
fn parse_annotations(control:&Control,entry:&mut Entry,fields:&BTreeMap<String,String>) {
    let marker=control.options.get("annotation_marker").map(String::as_str).unwrap_or("+an");let named=control.options.get("named_annotation_marker").map(String::as_str).unwrap_or(":");
    for (field,raw) in fields {
        let Some((field,suffix))=field.split_once(marker)else{continue};
        let name=suffix.strip_prefix(named).filter(|s|!s.is_empty()).unwrap_or("default");
        for raw in raw.split(';') {
            let Some((target,value))=raw.trim_start().split_once('=')else{continue};if value.is_empty(){continue;}
            let digits=target.chars().take_while(char::is_ascii_digit).count();let item=target[..digits].to_owned();let part=target[digits..].strip_prefix(':').unwrap_or(&target[digits..]).to_owned();
            let literal=value.trim().strip_prefix('"').and_then(|s|s.strip_suffix('"'));
            let annotation=Annotation {field:field.into(),name:name.into(),item,part,literal:literal.is_some(),value:literal.unwrap_or(value).into()};
            entry.annotations.retain(|a|!(a.field==annotation.field&&a.name==annotation.name&&a.item==annotation.item&&a.part==annotation.part));entry.annotations.push(annotation);
        }
    }
}
