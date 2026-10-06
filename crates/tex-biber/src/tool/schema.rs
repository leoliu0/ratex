//! Datamodel-derived BibLaTeXML Relax NG and a data-only validator.
use crate::model::Control;
use std::collections::{BTreeMap,BTreeSet};
use roxmltree::Node;
const NS:&str="http://biblatex-biber.sourceforge.net/biblatexml";
fn escape(s:&str)->String{s.replace('&',"&amp;").replace('<',"&lt;").replace('"',"&quot;")}
fn values<'a>(items:impl IntoIterator<Item=&'a str>)->String{items.into_iter().map(|s|format!("<value>{}</value>",escape(s))).collect()}
fn names(c:&Control)->String{
    let doc=roxmltree::Document::parse(super::SCHEMA).expect("bundled schema");
    let node=doc.descendants().find(|n|n.tag_name().name()=="define"&&n.attribute("name")==Some("namelist")).unwrap();
    let mut text=super::SCHEMA[node.range()].to_owned();let mut edits=Vec::new();
    for attribute in node.descendants().filter(|n|n.tag_name().name()=="attribute"&&n.attribute("name")==Some("type")){
        let choice=attribute.children().find(|n|n.is_element()).unwrap();
        let fields=c.fields.iter().filter(|(_,s)|s.fieldtype=="list"&&s.datatype=="name").map(|(name,_)|name.as_str());
        let replacement=if attribute.ancestors().any(|n|n.attribute("name")==Some("bltx:namepart")){values(c.nameparts.iter().map(String::as_str))}else{values(fields)};
        let range=choice.range();edits.push((range.start-node.range().start,range.end-node.range().start,format!("<choice>{replacement}</choice>")));
    }
    for (start,end,value) in edits.into_iter().rev(){text.replace_range(start..end,&value);}text
}
pub(super) fn generate(c:&Control)->String{
    let mut groups=BTreeMap::<String,Vec<&str>>::new();
    for (name,spec) in &c.fields{if spec.datatype!="datepart"{groups.entry(format!("{}{}",spec.datatype,spec.fieldtype)).or_default().push(name);}}
    let mut text=format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- Auto-generated from .bcf Datamodel -->\n<grammar xmlns=\"http://relaxng.org/ns/structure/1.0\" xmlns:bltx=\"{NS}\" datatypeLibrary=\"http://www.w3.org/2001/XMLSchema-datatypes\"><start><element name=\"bltx:entries\"><oneOrMore><element name=\"bltx:entry\"><attribute name=\"id\"/><attribute name=\"entrytype\"><choice>{}</choice></attribute><interleave>",values(c.datamodel.entrytypes.iter().map(String::as_str)));
    for group in groups.keys(){text.push_str(&format!("<ref name=\"{}\"/>",escape(group)));}text.push_str("<ref name=\"mannotation\"/></interleave></element></oneOrMore></element></start>");
    for (group,fields) in groups{
        if group=="namelist"{text.push_str(&names(c));continue;}
        text.push_str(&format!("<define name=\"{}\">",escape(&group)));
        if group=="datefield"{
            text.push_str(&format!("<zeroOrMore><element name=\"bltx:date\"><optional><attribute name=\"type\"><choice>{}</choice></attribute></optional><choice><data type=\"string\"/><group><element name=\"bltx:start\"><data type=\"string\"/></element><element name=\"bltx:end\"><choice><data type=\"string\"/><empty/></choice></element></group></choice></element></zeroOrMore>",values(fields.iter().filter_map(|s|s.strip_suffix("date")).filter(|s|!s.is_empty()))));
        }else{
            text.push_str("<interleave>");
            for field in fields{
                text.push_str(&format!("<optional><element name=\"bltx:{}\">",escape(field)));
                if group=="entrykeyfield"&&field=="related"{
                    text.push_str("<element name=\"bltx:list\"><oneOrMore><element name=\"bltx:item\"><attribute name=\"type\"/><attribute name=\"ids\"/><optional><attribute name=\"string\"/></optional><optional><attribute name=\"options\"/></optional></element></oneOrMore></element>");
                }else{
                    text.push_str("<choice><ref name=\"xdata\"/>");
                    if group.ends_with("list")||group=="entrykeyfield"{
                        text.push_str("<choice>");if group=="entrykeyfield"{text.push_str("<list><oneOrMore><data type=\"string\"/></oneOrMore></list>");}else{text.push_str("<text/>");}
                        text.push_str("<element name=\"bltx:list\"><oneOrMore><element name=\"bltx:item\"><choice><ref name=\"xdata\"/><text/></choice></element></oneOrMore></element></choice>");
                    }else if group=="rangefield"{
                        text.push_str("<element name=\"bltx:list\"><oneOrMore><element name=\"bltx:item\"><choice><ref name=\"xdata\"/><group><element name=\"bltx:start\"><text/></element><element name=\"bltx:end\"><choice><text/><empty/></choice></element></group></choice></element></oneOrMore></element>");
                    }else if group=="urifield"{text.push_str("<data type=\"anyURI\"/>");}else{text.push_str("<text/>");}
                    text.push_str("</choice>");
                }
                text.push_str("</element></optional>");
            }
            text.push_str("</interleave>");
        }
        text.push_str("</define>");
    }
    let doc=roxmltree::Document::parse(super::SCHEMA).expect("bundled schema");
    for name in ["xdata","gender","mannotation"]{
        let n=doc.descendants().find(|n|n.tag_name().name()=="define"&&n.attribute("name")==Some(name)).unwrap();
        if name=="gender"{if let Some(genders)=c.options.get("tool.gender"){
            text.push_str(&format!("<define name=\"gender\"><optional><attribute name=\"gender\"><choice>{}</choice></attribute></optional></define>",values(genders.split(',').map(str::trim))));continue;
        }}
        text.push_str(&super::SCHEMA[n.range()]);
    }
    text.push_str("</grammar>\n");text
}
#[derive(Clone)]
enum Pattern{Empty,Text,Data(String),Value(String),Element(String,Box<Pattern>),Attribute(String,Box<Pattern>),Choice(Vec<Pattern>),Group(Vec<Pattern>,bool),Repeat(Box<Pattern>,usize),List(Box<Pattern>)}
fn children<'a,'b>(n:Node<'a,'b>)->impl Iterator<Item=Node<'a,'b>>{n.children().filter(|n|n.is_element())}
fn parse<'a,'b>(n:Node<'a,'b>,defs:&BTreeMap<String,Node<'a,'b>>)->Result<Pattern,String>{
    let name=n.tag_name().name();
    let sub=||children(n).map(|n|parse(n,defs)).collect::<Result<Vec<_>,_>>();
    let body=||sub().map(|v|Pattern::Group(v,false));
    Ok(match name{
        "empty"=>Pattern::Empty,"text"=>Pattern::Text,"data"=>Pattern::Data(n.attribute("type").unwrap_or("string").into()),"value"=>Pattern::Value(n.text().unwrap_or("").into()),
        "element"=>Pattern::Element(n.attribute("name").unwrap_or("").into(),Box::new(body()?)),
        "attribute"=>Pattern::Attribute(n.attribute("name").unwrap_or("").into(),Box::new(if children(n).next().is_none(){Pattern::Text}else{body()?})),
        "choice"=>Pattern::Choice(sub()?),"group"|"start"|"define"=>body()?,"interleave"=>Pattern::Group(sub()?,true),
        "optional"=>Pattern::Choice(vec![Pattern::Empty,body()?]),"zeroOrMore"|"oneOrMore"=>Pattern::Repeat(Box::new(body()?),usize::from(name=="oneOrMore")),
        "list"=>Pattern::List(Box::new(body()?)),"ref"=>{let key=n.attribute("name").unwrap_or("");parse(*defs.get(key).ok_or_else(||format!("Missing schema definition '{key}'"))?,defs)?},
        _=>return Err(format!("Unsupported generated Relax NG pattern '{name}'")),
    })
}
#[derive(Clone)]
enum Item<'a,'b>{Element(Node<'a,'b>),Attribute(&'a str,&'a str),Text(String)}
fn content<'a,'b>(node:Node<'a,'b>)->Vec<Item<'a,'b>>{
    let mut items=node.attributes().map(|a|Item::Attribute(a.name(),a.value())).collect::<Vec<_>>();
    let elements=node.children().any(|n|n.is_element());
    for n in node.children(){if n.is_element(){items.push(Item::Element(n));}else if n.is_text(){let text=n.text().unwrap_or("");if !elements||!text.trim().is_empty(){if let Some(Item::Text(previous))=items.last_mut(){previous.push_str(text);}else{items.push(Item::Text(text.into()));}}}}
    items
}
fn simple(pattern:&Pattern,value:&str)->bool{
    match pattern{Pattern::Text=>true,Pattern::Empty=>value.is_empty(),Pattern::Data(kind)=>match kind.as_str(){"boolean"=>matches!(value.trim(),"true"|"false"|"1"|"0"),"string"|"anyURI"=>true,_=>false},Pattern::Value(expected)=>value.split_whitespace().collect::<Vec<_>>().join(" ")==*expected,Pattern::Choice(v)=>v.iter().any(|p|simple(p,value)),Pattern::Group(v,_)=>v.iter().all(|p|simple(p,value)),Pattern::List(p)=>value.split_whitespace().all(|s|simple(p,s)),Pattern::Repeat(p,min)=>if value.is_empty(){*min==0}else{simple(p,value)},_=>false}
}
fn matches(pattern:&Pattern,items:&[Item<'_, '_>],used:&[bool],unordered:bool)->Vec<Vec<bool>>{
    let mut results=Vec::new();
    match pattern{
        Pattern::Empty=>results.push(used.to_vec()),
        Pattern::Text|Pattern::Data(_)|Pattern::Value(_)|Pattern::List(_)=>{
            if simple(pattern,""){results.push(used.to_vec());}
            for (i,item) in items.iter().enumerate(){if !used[i]{if let Item::Text(value)=item{if simple(pattern,value){let mut state=used.to_vec();state[i]=true;results.push(state);}break;}}}
        }
        Pattern::Attribute(name,p)=>for (i,item) in items.iter().enumerate(){if !used[i]{if let Item::Attribute(key,value)=item{if *key==name&&simple(p,value){let mut state=used.to_vec();state[i]=true;results.push(state);}}}},
        Pattern::Element(name,p)=>for (i,item) in items.iter().enumerate(){if !used[i]{if let Item::Element(node)=item{
            let key=name.strip_prefix("bltx:").unwrap_or(name);if node.tag_name().name()==key&&node.tag_name().namespace()==Some(NS){let nested=content(*node);if matches(p,&nested,&vec![false;nested.len()],false).iter().any(|s|s.iter().all(|b|*b)){let mut state=used.to_vec();state[i]=true;results.push(state);}}
            if !unordered{break;}
        }else if matches!(item,Item::Text(s) if !s.trim().is_empty()){break;}}},
        Pattern::Choice(v)=>for p in v{results.extend(matches(p,items,used,unordered));},
        Pattern::Group(v,interleave)=>{
            let mut states=vec![used.to_vec()];for p in v{let mut next=Vec::new();for state in states{next.extend(matches(p,items,&state,unordered||*interleave));}next.sort();next.dedup();states=next;}results=states;
        }
        Pattern::Repeat(p,min)=>{
            let mut states=vec![used.to_vec()];let mut seen=BTreeSet::new();if *min==0{results.push(used.to_vec());}
            while !states.is_empty(){let mut next=Vec::new();for state in states{for candidate in matches(p,items,&state,unordered){if candidate!=state&&seen.insert(candidate.clone()){results.push(candidate.clone());next.push(candidate);}}}states=next;}
        }
    }
    results.sort();results.dedup();results
}
pub(super) fn validate(xml:&str,schema:&str)->Result<(),String>{
    let preprocessed=crate::biblatexml::preprocess(xml)?;
    let document=roxmltree::Document::parse_with_options(&preprocessed,roxmltree::ParsingOptions {allow_dtd:true,..Default::default()}).map_err(|e|format!("Invalid BibLaTeXML: {e}"))?;
    let grammar=roxmltree::Document::parse(schema).map_err(|e|format!("Invalid BibLaTeXML schema: {e}"))?;
    let defs=grammar.root_element().children().filter(|n|n.tag_name().name()=="define").map(|n|(n.attribute("name").unwrap_or("").to_owned(),n)).collect();
    let start=grammar.root_element().children().find(|n|n.tag_name().name()=="start").ok_or("Schema has no start pattern")?;
    let pattern=parse(start,&defs)?;let items=[Item::Element(document.root_element())];
    if matches(&pattern,&items,&[false],false).iter().any(|s|s.iter().all(|b|*b)){Ok(())}else{Err("BibLaTeXML does not validate against the datamodel Relax NG schema".into())}
}
