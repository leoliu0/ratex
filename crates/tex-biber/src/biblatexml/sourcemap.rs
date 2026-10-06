//! XML sourcemaps operate on a mutable datasource DOM before field ingestion.
use super::{dtd, NAMESPACE};
use crate::bib::{map_created_key,map_csv,map_truth,maploop_value};
use crate::model::{Control,Section};
use crate::perl_regex::{Regex,saved_captures};
use roxmltree::Node as ReadNode;
use std::collections::{BTreeMap,BTreeSet};
use sxd_document::{dom,Package,QName};
use sxd_xpath::{Context,Factory,Value,nodeset::Node};
use unicode_normalization::UnicodeNormalization;

fn copy_read<'d>(document:dom::Document<'d>,node:ReadNode<'_, '_>)->dom::Element<'d> {
    let element=document.create_element(QName::with_namespace_uri(node.tag_name().namespace(),node.tag_name().name()));
    let input=node.document().input_text();
    let lexical=input[node.range()].trim_start_matches('<').split(|c:char|c.is_whitespace()||c=='>'||c=='/').next().unwrap_or("");
    if let Some((prefix,_))=lexical.split_once(':'){element.set_preferred_prefix(Some(prefix));}
    for namespace in node.namespaces(){if let Some(prefix)=namespace.name(){element.register_prefix(prefix,namespace.uri());}else{element.set_default_namespace_uri(Some(namespace.uri()));}}
    for attribute in node.attributes(){element.set_attribute_value(QName::with_namespace_uri(attribute.namespace(),attribute.name()),attribute.value());}
    for child in node.children(){
        if child.is_text(){element.append_child(document.create_text(child.text().unwrap_or("")));}
        else if child.is_element()&&child.range().start>=node.document().root_element().range().start{element.append_child(copy_read(document,child));}
        else if child.is_comment(){element.append_child(document.create_comment(child.text().unwrap_or("")));}
    }
    element
}
fn clone_element<'d>(element:dom::Element<'d>)->dom::Element<'d> {
    let document=element.document();let copy=document.create_element(element.name());
    copy.set_preferred_prefix(element.preferred_prefix());copy.set_default_namespace_uri(element.default_namespace_uri());
    for namespace in element.namespaces_in_scope(){copy.register_prefix(namespace.prefix(),namespace.uri());}
    for attribute in element.attributes(){let a=copy.set_attribute_value(attribute.name(),attribute.value());a.set_preferred_prefix(attribute.preferred_prefix());}
    for child in element.children(){
        let node = match child {
            dom::ChildOfElement::Element(n) => Node::from(n),
            dom::ChildOfElement::Text(n) => Node::from(n),
            dom::ChildOfElement::Comment(n) => Node::from(n),
            dom::ChildOfElement::ProcessingInstruction(n) => Node::from(n),
        };
        copy_child(copy,node);
    }
    copy
}
fn copy_child<'d>(target:dom::Element<'d>,source:Node<'d>){
    let document=target.document();
    match source {
        Node::Element(element)=>target.append_child(clone_element(element)),
        Node::Text(text)=>target.append_child(document.create_text(text.text())),
        Node::Comment(comment)=>target.append_child(document.create_comment(comment.text())),
        Node::ProcessingInstruction(pi)=>target.append_child(document.create_processing_instruction(pi.target(),pi.value())),
        _=>{}
    }
}
fn value<'d>(entry:dom::Element<'d>,path:&str)->Result<Value<'d>,String>{
    let expression=Factory::new().build(path).map_err(|e|format!("Invalid XML sourcemap XPath '{path}': {e}"))?.ok_or_else(||format!("Empty XML sourcemap XPath '{path}'"))?;
    let mut context=Context::new();
    for namespace in entry.namespaces_in_scope(){context.set_namespace(namespace.prefix(),namespace.uri());}
    context.set_namespace("bltx",NAMESPACE);
    expression.evaluate(&context,entry).map_err(|e|format!("XML sourcemap XPath '{path}' failed: {e}"))
}
fn first<'d>(entry:dom::Element<'d>,path:&str)->Result<Option<Node<'d>>,String>{
    match value(entry,path)?{Value::Nodeset(nodes)=>Ok(nodes.document_order_first()),_=>Err(format!("XML sourcemap XPath '{path}' is not a node selector"))}
}
fn exists(entry:dom::Element<'_>,path:&str)->Result<bool,String>{Ok(value(entry,path)?.boolean())}
fn path(control:&Control,field:&str)->String{
    if field.contains('/') {return field.to_owned();}
    if let Some(spec)=control.fields.get(field){
        if spec.fieldtype=="list"&&spec.datatype=="name"{format!("./bltx:names[@type='{field}']")}
        else{format!("./bltx:{field}")}
    }else{field.to_owned()}
}
fn detach(node:Node<'_>){match node{Node::Element(n)=>n.remove_from_parent(),Node::Attribute(n)=>n.remove_from_parent(),Node::Text(n)=>n.remove_from_parent(),Node::Comment(n)=>n.remove_from_parent(),Node::ProcessingInstruction(n)=>n.remove_from_parent(),_=>{}}}
fn set_attribute<'d>(parent:dom::Element<'d>,name:&str,text:&str)->dom::Attribute<'d>{
    if let Some((prefix,local))=name.split_once(':'){
        parent.set_attribute_value(QName::with_namespace_uri(parent.namespace_uri_for_prefix(prefix),local),text)
    }else{parent.set_attribute_value(name,text)}
}
fn set_string(node:Node<'_>,text:&str)->Result<(),String>{
    let text=text.nfc().collect::<String>();
    match node{
        Node::Attribute(attribute)=>{let parent=attribute.parent().ok_or("Detached XML attribute")?;parent.set_attribute_value(attribute.name(),&text);},
        Node::Text(node)=>node.set_text(&text),
        Node::Element(element)=>{
            let node=element.children().into_iter().find_map(|n|n.text()).ok_or("XML sourcemap target element has no text node")?;
            node.set_text(&text);
        },
        _=>return Err("XML sourcemap target is not an attribute, element or text node".into())
    }
    Ok(())
}
fn create_target<'d>(entry:dom::Element<'d>,selector:&str,complex:bool)->Result<Node<'d>,String>{
    let mut parent=entry;
    let mut cumulative=String::from(".");
    let components=selector.strip_prefix("./").unwrap_or(selector).split('/').collect::<Vec<_>>();
    for (i,component) in components.iter().enumerate(){
        cumulative.push('/');cumulative.push_str(component);
        if let Some(node)=first(entry,&cumulative)? {
            if i+1==components.len(){return Ok(node);}
            parent=node.element().ok_or("XML sourcemap creation path has a non-element parent")?;continue;
        }
        if let Some(attribute)=component.strip_prefix('@'){
            if i+1!=components.len(){return Err("XML sourcemap attribute is not the final path component".into());}
            return Ok(set_attribute(parent,attribute,"").into());
        }
        if *component=="text()"{
            if i+1!=components.len(){return Err("XML sourcemap text() is not the final path component".into());}
            let text=entry.document().create_text("");parent.append_child(text);return Ok(text.into());
        }
        let (local,nametype)=if let Some(rest)=component.strip_prefix("bltx:names[@type="){
            let value=rest.strip_suffix(']').and_then(|s|s.strip_prefix('\'').and_then(|s|s.strip_suffix('\'')).or_else(||s.strip_prefix('"').and_then(|s|s.strip_suffix('"')))).ok_or("Invalid name-list creation selector")?;
            ("names",Some(value))
        }else{
            let local=component.strip_prefix("bltx:").ok_or_else(||format!("Cannot create XML sourcemap path component '{component}'"))?;
            if local.contains(['[',']','(',')','@',':']){return Err(format!("Cannot create XML sourcemap element '{local}'"));}
            (local,None)
        };
        let element=entry.document().create_element((NAMESPACE,local));element.set_preferred_prefix(Some("bltx"));element.register_prefix("bltx",NAMESPACE);
        if let Some(kind)=nametype{element.set_attribute_value("type",kind);if i+1==components.len()&&!complex{return Err(format!("Tried to map to complex target '{selector}' with string value"));}}
        parent.append_child(element);parent=element;
        if i+1==components.len(){return Ok(element.into());}
    }
    Err(format!("Cannot create XML sourcemap target '{selector}'"))
}
fn change(control:&Control,entry:dom::Element<'_>,target:&str,input:&str)->Result<(),String>{
    let normalized=if control.fields.get(input).is_some_and(|s|s.fieldtype=="list"&&s.datatype=="name"){path(control,input)}else{input.to_owned()};
    let source=if normalized.contains('/') {Some(first(entry,&normalized)?.ok_or_else(||format!("XML sourcemap source '{normalized}' does not exist"))?)}else{None};
    let existing=first(entry,target)?;
    let node=match existing{Some(node)=>node,None=>create_target(entry,target,source.is_some())?};
    if let Some(source)=source{
        let Some(element)=node.element()else{return Err(format!("Tried to replace '{target}' scalar node with complex data"));};
        let children=source.children();
        // Clone before removal, including when source and target are identical.
        let staging=entry.document().create_element("staging");
        for child in children{copy_child(staging,child);}
        element.clear_children();for child in staging.children(){element.append_child(child);}
    }else if existing.is_none(){
        if let Some(element)=node.element(){element.set_text(&normalized.nfc().collect::<String>());}
        else{set_string(node,&normalized)?;}
    }else{set_string(node,&normalized)?;}
    Ok(())
}

fn attribute<'a>(entry:dom::Element<'a>,defaults:&'a dtd::Defaults,name:&str)->Option<&'a str>{
    entry.attribute_value(name).or_else(|| {
        defaults.iter().find(|(lexical,_)| {
            let (prefix,local)=lexical.split_once(':').map(|(p,l)|(Some(p),l)).unwrap_or((None,lexical.as_str()));
            local==entry.name().local_part() && prefix==entry.preferred_prefix()
        }).and_then(|(_,attrs)|attrs.get(name)).map(String::as_str)
    })
}

pub fn apply(control:&Control,section:&mut Section,document:&roxmltree::Document<'_>,defaults:&dtd::Defaults,source:&str,remote:bool,warnings:&mut Vec<String>)->Result<Option<String>,String>{
    if !control.sourcemaps.iter().any(|m|m.datatype=="biblatexml"){return Ok(None);}
    let package=Package::new();let output=package.as_document();let root=copy_read(output,document.root_element());output.root().append_child(root);
    let entries=root.children().into_iter().filter_map(|n|n.element()).filter(|e|e.name()==QName::with_namespace_uri(Some(NAMESPACE),"entry")).collect::<Vec<_>>();
    let mut wanted=section.citekeys.iter().cloned().collect::<BTreeSet<_>>();let allkeys=wanted.contains("*");let mut done=BTreeSet::new();let mut unique=String::new();
    loop{
        let next=entries.iter().enumerate().find(|(i,e)|!done.contains(i)&&(allkeys||e.attribute_value("id").is_some_and(|k|wanted.contains(k))));
        let Some((index,entry))=next else{break};done.insert(index);let entry=*entry;
        let key=attribute(entry,defaults,"id").unwrap_or("").to_owned();
        let mut created=BTreeMap::new();let mut keep=true;
        'maps: for map in &control.sourcemaps{
            let kind=attribute(entry,defaults,"entrytype").unwrap_or("");
            // The XML driver's per_type check uses the DOM node's type (1).
            if map.datatype!="biblatexml"||map.refsection.is_some_and(|n|n!=section.number)||(!map.per_type.is_empty()&&!map.per_type.iter().any(|v|v.to_lowercase()=="1"))||map.per_not_type.iter().any(|v|v.to_lowercase()==kind)||(!map.per_source.is_empty()&&(remote||!map.per_source.iter().any(|v|v==source))){continue;}
            let mut last_type=kind.to_owned();let mut last_field=String::new();let mut last_value=String::new();let mut captures=Vec::new();
            let loops=if let Some(foreach)=&map.foreach{
                let selector=if foreach.contains('/') {foreach.clone()}else{format!("./bltx:{foreach}")};
                let selected=value(entry,&selector)?;
                if selected.boolean(){map_csv(&selected.string())}else{vec![String::new()]}
            }else{vec![String::new()]};
            for maploop in loops{
                for step in &map.steps{
                    let get=|name:&str|step.get(name).map(String::as_str);let yes=|name:&str|get(name).is_some_and(map_truth);
                    macro_rules! fail{()=>{if yes("map_final"){continue 'maps;}else{continue;}}}
                    if yes("map_entry_null"){keep=false;break 'maps;}
                    if let Some(newkey)=maploop_value(get("map_entry_new"),&maploop,&mut unique).filter(|v|map_truth(v)){
                        let Some(kind)=maploop_value(get("map_entry_newtype"),&maploop,&mut unique).filter(|v|map_truth(v))else{warnings.push(format!("Source mapping (type={}, key={key}): Missing type for new entry '{newkey}', skipping step ...",map.level));continue;};
                        let new=output.create_element((NAMESPACE,"entry"));new.set_preferred_prefix(Some("bltx"));new.register_prefix("bltx",NAMESPACE);new.set_attribute_value("id",&newkey.nfc().collect::<String>());new.set_attribute_value("entrytype",&kind.nfc().collect::<String>());created.insert(newkey.clone(),new);if allkeys{map_created_key(section,source,&newkey);}
                    }
                    if let Some(prefix)=maploop_value(get("map_entry_clone"),&maploop,&mut unique).filter(|v|map_truth(v)){
                        // 2.22 reinserts the original DOM id; only the requested prefixed key is added.
                        if allkeys{map_created_key(section,source,&format!("{prefix}{key}"));}
                    }
                    let targetkey=maploop_value(get("map_entrytarget"),&maploop,&mut unique).filter(|v|map_truth(v));
                    let target=if let Some(targetkey)=targetkey{let Some(target)=created.get(&targetkey).copied()else{warnings.push(format!("Source mapping (type={}, key={key}): Dynamically created entry target '{targetkey}' does not exist skipping step ...",map.level));continue;};target}else{entry};
                    if let Some(kind)=maploop_value(get("map_type_source"),&maploop,&mut unique).filter(|v|map_truth(v)){
                        if attribute(target,defaults,"entrytype")!=Some(kind.to_lowercase().as_str()){fail!();}
                        last_type=attribute(target,defaults,"entrytype").unwrap_or("").to_owned();target.set_attribute_value("entrytype",&maploop_value(get("map_type_target"),&maploop,&mut unique).unwrap_or_default().to_lowercase().nfc().collect::<String>());
                    }
                    let absent=maploop_value(get("map_notfield"),&maploop,&mut unique).filter(|v|map_truth(v)).map(|v|path(control,&v));
                    if let Some(absent)=&absent{if exists(target,absent)?{fail!();}}
                    let cited=section.citekeys.contains(&key)&&!section.nocite.contains(&key);let nocited=section.nocite.contains(&key);let allnocited=section.nocite.contains("*");
                    if (yes("map_entrykey_citedornocited")&&!section.citekeys.contains(&key))||(yes("map_entrykey_cited")&&!cited)||(yes("map_entrykey_nocited")&&(cited||(!nocited&&!allnocited)))||(yes("map_entrykey_allnocited")&&!allnocited)||(yes("map_entrykey_starnocited")&&allnocited&&(cited||nocited)){fail!();}
                    let field=maploop_value(get("map_field_source"),&maploop,&mut unique).filter(|v|map_truth(v)).map(|v|path(control,&v));
                    if let Some(field)=&field{if !exists(target,field)?{fail!();}}
                    if field.is_some()||absent.is_some(){
                        let field=field.as_deref().ok_or("XML map_notfield has no map_field_source")?;
                        let node=first(target,field)?.ok_or("XML sourcemap source is not a node")?;last_field=node.prefixed_name().unwrap_or_else(||"#text".into());last_value=value(target,field)?.string();
                        if let Some((name,fixed))=["map_matchesi","map_matches"].into_iter().find_map(|name|get(name).filter(|v|map_truth(v)).map(|v|(name,v))){
                            let fixed=map_csv(fixed);let replacements=map_csv(get("map_replace").unwrap_or(""));if fixed.len()!=replacements.len(){continue;}
                            for (matched,replacement) in fixed.iter().zip(replacements){if &last_value==matched||(name.ends_with('i')&&last_value.to_lowercase()==matched.to_lowercase()){set_string(node,&replacement)?;}}
                        }
                        if let Some((name,pattern))=["map_matchi","map_notmatchi","map_notmatch","map_match"].into_iter().find_map(|name|get(name).filter(|v|map_truth(v)).map(|v|(name,v))){
                            let pattern=maploop_value(Some(pattern),&maploop,&mut unique).unwrap();let pattern=if get("map_replace").is_none(){saved_captures(&pattern,&captures,false)}else{pattern};
                            let re=Regex::compile(&pattern,name.ends_with('i'))?;
                            if let Some(replacement)=get("map_replace"){
                                if field.eq_ignore_ascii_case("./@id"){continue;}
                                let replacement=maploop_value(Some(replacement),&maploop,&mut unique).unwrap();let replacement=re.replace_all(&last_value,&replacement)?;
                                if let Err(error)=change(control,target,field,&replacement){warnings.push(format!("Source mapping (type={}, key={key}): {error}",map.level));}
                            }else{
                                // In 2.22 map_notmatchi sets insensitive matching but not negation.
                                captures=re.match_all(&last_value,name=="map_notmatch")?;if captures.is_empty(){fail!();}
                            }
                        }
                        if let Some(destination)=maploop_value(get("map_field_target"),&maploop,&mut unique).filter(|v|map_truth(v)).map(|v|path(control,&v)){
                            if destination.eq_ignore_ascii_case("./@id"){continue;}
                            if exists(target,&destination)?&&!map.overwrite{continue;}
                            if let Err(error)=change(control,target,&destination,field){warnings.push(format!("Source mapping (type={}, key={key}): {error}",map.level));}
                            if let Some(node)=first(target,field)?{detach(node);}
                        }
                    }
                    if let Some(destination)=maploop_value(get("map_field_set"),&maploop,&mut unique).filter(|v|map_truth(v)).map(|v|path(control,&v)){
                        if yes("map_null"){let node=first(target,&destination)?.ok_or("XML sourcemap cannot delete an absent node")?;detach(node);continue;}
                        if exists(target,&destination)?&&!map.overwrite{fail!();}
                        let original=if yes("map_append")||yes("map_appendstrict"){value(target,&destination)?.string()}else{String::new()};
                        let next=if yes("map_origentrytype"){if !map_truth(&last_type){continue;}last_type.clone()}
                            else if yes("map_origfieldval"){if !map_truth(&last_value){continue;}last_value.clone()}
                            else if yes("map_origfield"){if !map_truth(&last_field){continue;}last_field.clone()}
                            else{saved_captures(&maploop_value(get("map_field_value"),&maploop,&mut unique).unwrap_or_default(),&captures,false)};
                        let next=if yes("map_appendstrict")&&!map_truth(&original){String::new()}else{original+&next};
                        if let Err(error)=change(control,target,&destination,&next){warnings.push(format!("Source mapping (type={}, key={key}): {error}",map.level));}
                    }
                }
            }
        }
        if !keep{entry.remove_from_parent();section.citekeys.retain(|k|k!=&key);continue;}
        for new in created.into_values(){root.append_child(new);}
        for field in ["crossref","xref","xdata","related","entryset"]{
            if let Value::Nodeset(nodes)=value(entry,&format!("./bltx:{field}"))?{for node in nodes.document_order(){wanted.extend(map_csv(&node.string_value()));}}
        }
        if let Value::Nodeset(nodes)=value(entry,".//@xdata")?{for node in nodes.document_order(){wanted.insert(node.string_value().split('-').next().unwrap_or("").to_owned());}}
    }
    let mut bytes=Vec::new();sxd_document::writer::format_document(&output,&mut bytes).map_err(|e|e.to_string())?;
    let mut text=String::from_utf8(bytes).map_err(|e|e.to_string())?;
    if let Some(doctype)=dtd::doctype(document.input_text()){
        let insertion=text.find("?>").map(|i|i+2).unwrap_or(0);text.insert_str(insertion,doctype);
    }
    Ok(Some(text))
}
