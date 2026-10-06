use crate::model::*;
use roxmltree::{Document, Node};
use std::collections::BTreeMap;

fn text(n: Node<'_, '_>) -> String { n.text().unwrap_or("").trim().to_owned() }
fn yes(n: Node<'_, '_>, a: &str) -> bool { matches!(n.attribute(a), Some("1" | "true")) }
fn number(n: Node<'_, '_>, a: &str) -> Option<usize> { n.attribute(a).and_then(|s|s.parse().ok()) }
fn elements<'a,'b>(n: Node<'a,'b>, name: &'static str) -> impl Iterator<Item=Node<'a,'b>> { n.children().filter(move |c|c.is_element() && c.tag_name().name()==name) }
fn section(c: &mut Control, number: usize) -> &mut Section {
    if let Some(i)=c.sections.iter().position(|s|s.number==number) { return &mut c.sections[i] }
    c.sections.push(Section {number,..Default::default()}); c.sections.last_mut().unwrap()
}
pub fn parse(xml: &str) -> Result<Control,String> {
    let doc=Document::parse(xml).map_err(|e|format!("Invalid BCF XML: {e}"))?;
    let root=doc.root_element();
    if root.tag_name().name()!="controlfile" { return Err("BCF root is not a controlfile".into()) }
    let mut c=Control::default();
    if root.attribute("version")!=Some("3.11") { c.warnings.push(format!("BCF version {} differs from supported 3.11",root.attribute("version").unwrap_or("unknown"))); }
    let mut option_datatypes=BTreeMap::<String,String>::new();
    for n in root.children().filter(Node::is_element) {
        match n.tag_name().name() {
            "options" => {
                let mut opts=BTreeMap::new();
                for o in elements(n,"option") {
                    if let Some(k)=elements(o,"key").next() {
                        let vals=elements(o,"value").map(text).collect::<Vec<_>>();
                        if text(k)=="labeldatespec" { c.labeldate_specs.insert(n.attribute("type").unwrap_or("global").to_owned(),elements(o,"value").map(|v|LabelDateSpec {name:text(v),literal:v.attribute("type")==Some("string")}).collect()); }
                        opts.insert(text(k),vals.join(","));
                    }
                }
                if n.attribute("type")==Some("global") { c.options.extend(opts); }
                else if let Some(kind)=n.attribute("type") { c.type_options.entry(kind.to_owned()).or_default().extend(opts); }
            }
            "optionscope" => {
                let scope=n.attribute("type").unwrap_or("ENTRY");
                for option in elements(n,"option") {
                    let name=text(option);
                    let datatype=option.attribute("datatype").unwrap_or("").to_lowercase();
                    if let Some(previous)=option_datatypes.get(&name).filter(|previous|**previous!=datatype) {
                        c.warnings.push(format!("Warning: Datatype for biblatex option '{name}' has conflicting values, probably at different scopes. This is not supported."));
                        c.options.insert(format!("optionscope.{scope}.{name}.datatype"),previous.clone());
                    }else{
                        option_datatypes.insert(name.clone(),datatype.clone());
                        c.options.insert(format!("optionscope.{scope}.{name}.datatype"),datatype);
                    }
                    for key in ["backendin","backendout"]{if let Some(value)=option.attribute(key){c.options.insert(format!("optionscope.{scope}.{name}.{key}"),value.into());}}
                }
            }
            "datamodel" => { crate::validation::parse(n, &mut c)?; }
            "bibdata" => {
                let s=section(&mut c,number(n,"section").unwrap_or(0));
                for d in elements(n,"datasource") {
                    let name=text(d);
                    s.datasource_formats.insert(name.clone(),d.attribute("datatype").unwrap_or("bibtex").to_owned());
                    if let Some(encoding)=d.attribute("encoding") {s.datasource_encodings.insert(name.clone(),encoding.to_owned());}
                    if !s.sources.contains(&name) {s.sources.push(name)}
                }
            }
            "section" => {
                let s=section(&mut c,number(n,"number").unwrap_or(0));
                for key in elements(n,"citekey") {
                    let keytext=text(key);
                    if !s.citekeys.contains(&keytext) {s.citekeys.push(keytext.clone());}
                    // Biber.pm's @prekeys keeps only the first occurrence of a key, "*" included;
                    // a later \cite of a \nocite'd key only clears its nocite flag.
                    let first=!s.cite_orders.contains_key(&keytext);
                    s.cite_orders.entry(keytext.clone()).or_insert((number(key,"order").unwrap_or(1),number(key,"intorder").unwrap_or(1)));
                    if yes(key,"nocite") || key.attribute("type")==Some("nocite") {if first{s.nocite.insert(keytext);}} else {s.nocite.remove(&keytext);}
                }
                for count in elements(n,"citekeycount") {s.cite_counts.insert(text(count),count.attribute("count").and_then(|s|s.parse().ok()).unwrap_or(-1));}
            }
            "sortingtemplate" => {
                // Biber::_parse_sort: sorts and sortitems ordered by `order`; empty sorts dropped.
                let mut sort_nodes=elements(n,"sort").collect::<Vec<_>>();sort_nodes.sort_by_key(|s|number(*s,"order").unwrap_or(0));
                let sorts=sort_nodes.into_iter().filter_map(|s| {
                    let mut item=SortItem {descending:s.attribute("sort_direction")==Some("descending"),final_:yes(s,"final"),locale:s.attribute("locale").map(str::to_owned),sortcase:s.attribute("sortcase").map(|v|matches!(v,"1"|"true")),sortupper:s.attribute("sortupper").map(|v|matches!(v,"1"|"true")),..Default::default()};
                    let mut items=elements(s,"sortitem").collect::<Vec<_>>();items.sort_by_key(|f|number(*f,"order").unwrap_or(0));
                    let attribute=|f:Node<'_,'_>,name:&str|f.attribute(name).map(str::to_owned);
                    item.elements=items.into_iter().map(|f|SortElement {name:text(f),literal:yes(f,"literal"),substring_side:attribute(f,"substring_side"),substring_width:attribute(f,"substring_width"),pad_side:attribute(f,"pad_side"),pad_width:attribute(f,"pad_width"),pad_char:attribute(f,"pad_char")}).collect();
                    (!item.elements.is_empty()).then_some(item)
                }).collect();
                let locale=n.attribute("locale").or_else(||c.options.get("sortlocale").map(String::as_str)).unwrap_or("en_US");
                c.sorting_locales.insert(n.attribute("name").unwrap_or("global").into(),locale.into());
                c.sorting.insert(n.attribute("name").unwrap_or("global").into(),sorts);
            }
            "datalist" => {
                section(&mut c,number(n,"section").unwrap_or(0)).lists.push(DataList {name:n.attribute("name").unwrap_or("").into(),sorting:n.attribute("sortingtemplatename").unwrap_or("nyt").into(),kind:n.attribute("type").unwrap_or("entry").into(),labelprefix:n.attribute("labelprefix").unwrap_or("").into(),namekey:n.attribute("sortingnamekeytemplatename").unwrap_or("global").into(),unique_template:n.attribute("uniquenametemplatename").unwrap_or("global").into(),alpha_template:n.attribute("labelalphanametemplatename").unwrap_or("global").into(),hash_template:n.attribute("namehashtemplatename").unwrap_or("global").into()});
            }
            "transliteration" => {
                let kind=n.attribute("entrytype").or_else(||n.attribute("type")).unwrap_or("*");
                let opts=if matches!(kind,"*"|"global"){&mut c.options}else{c.type_options.entry(kind.into()).or_default()};
                for rule in elements(n,"translit") {
                    let value=format!("{}\t{}\t{}\t{}",rule.attribute("target").unwrap_or(""),rule.attribute("from").unwrap_or(""),rule.attribute("to").unwrap_or(""),rule.attribute("langids").unwrap_or("\0"));
                    let rules=opts.entry("translit_rules".into()).or_default();if !rules.is_empty(){rules.push('\n');}rules.push_str(&value);
                }
            }
            "labelalphatemplate" => {
                let mut parts=elements(n,"labelelement").collect::<Vec<_>>();parts.sort_by_key(|e|number(*e,"order").unwrap_or(0));
                let template=parts.into_iter().map(|e| {let mut fields=elements(e,"labelpart").collect::<Vec<_>>();fields.sort_by_key(|p|number(*p,"order").unwrap_or(0));LabelPart {fields:fields.into_iter().map(|p|LabelField {name:text(p),literal:yes(p,"literal"),final_:yes(p,"final"),attributes:p.attributes().map(|a|(a.name().into(),a.value().into())).collect()}).collect()}}).collect();
                if n.attribute("type").unwrap_or("global")=="global" {c.labelalpha=template;}else {c.label_templates.by_type.insert(n.attribute("type").unwrap_or("").into(),template);}
            }
            "sourcemap" => {
                let mut fieldsets = BTreeMap::new();
                for set in elements(root,"datafieldset") {
                    let mut fields = Vec::new();
                    for member in elements(set,"member") {
                        if let Some(field) = member.attribute("field") {fields.push(field.to_owned());}
                        else {
                            for field in root.descendants().filter(|f|f.is_element() && f.tag_name().name()=="field" && f.attribute("datatype").is_some()) {
                                if member.attribute("datatype").is_none_or(|s|field.attribute("datatype")==Some(s)) && member.attribute("fieldtype").is_none_or(|s|field.attribute("fieldtype")==Some(s)) {fields.push(text(field));}
                            }
                        }
                    }
                    fieldsets.insert(set.attribute("name").unwrap_or("").to_lowercase(),fields);
                }
                let mut groups=elements(n,"maps").collect::<Vec<_>>();
                groups.sort_by_key(|maps|match maps.attribute("level").unwrap_or("user") {"user"=>0,"style"=>1,"driver"=>2,_=>3});
                let mut seen_driver=std::collections::BTreeSet::new();
                for maps in groups {
                    let datatype=maps.attribute("datatype").unwrap_or("bibtex");
                    let level=maps.attribute("level").unwrap_or("user");
                    if level=="driver" && !seen_driver.insert(datatype.to_owned()) {continue;}
                    for map in elements(maps,"map") {
                        let mut m=SourceMap {overwrite:map.attribute("map_overwrite").map(|s|matches!(s,"1"|"true")).unwrap_or_else(||yes(maps,"map_overwrite")),foreach:map.attribute("map_foreach").map(str::to_owned),refsection:number(map,"refsection"),level:level.into(),datatype:datatype.into(),fieldsets:fieldsets.clone(),..Default::default()};
                        for step in map.children().filter(Node::is_element) {
                            match step.tag_name().name() {
                                "per_type" => m.per_type.push(text(step).to_lowercase()),
                                "per_nottype" => m.per_not_type.push(text(step).to_lowercase()),
                                "per_datasource" => m.per_source.push(text(step)),
                                "map_step" => m.steps.push(step.attributes().map(|a|(a.name().to_owned(),a.value().to_owned())).collect()),
                                other => c.warnings.push(format!("Unsupported sourcemap element {other}")),
                            }
                        }
                        for step in &m.steps {
                            for name in ["map_match","map_matchi","map_notmatch","map_notmatchi"] {
                                if let Some(pattern)=step.get(name) {
                                    if pattern.contains("$MAP") || crate::perl_regex::has_saved_captures(pattern) {continue;}
                                    if let Ok(re)=crate::perl_regex::Regex::compile(pattern,name.ends_with('i')) {m.patterns.insert(format!("{name}:{pattern}"),re);}
                                }
                            }
                            for key in step.keys() {
                                if !["map_field_source","map_field_target","map_field_set","map_field_value","map_type_source","map_type_target","map_final","map_null","map_match","map_matchi","map_notmatch","map_notmatchi","map_matches","map_matchesi","map_replace","map_origfieldval","map_origfield","map_origentrytype","map_append","map_appendstrict","map_notfield","map_entry_null","map_entry_new","map_entry_newtype","map_entry_clone","map_entry_nocite","map_entrytarget","map_entrykey_citedornocited","map_entrykey_cited","map_entrykey_nocited","map_entrykey_allnocited","map_entrykey_starnocited"].contains(&key.as_str()) {c.warnings.push(format!("Unsupported sourcemap step {key}"));}
                            }
                        }
                        c.sourcemaps.push(m);
                    }
                }
            }
            "inheritance" => {
                for defaults in elements(n,"defaults") {
                    for key in ["inherit_all","override_target"]{if let Some(value)=defaults.attribute(key){c.options.insert(format!("inheritance.{key}.*.*"),value.into());}}
                    for pair in elements(defaults,"type_pair"){for key in ["inherit_all","override_target"]{if let Some(value)=pair.attribute(key){c.options.insert(format!("inheritance.{key}.{}.{}",pair.attribute("source").unwrap_or("*"),pair.attribute("target").unwrap_or("*")),value.into());}}}
                }
                for rule in elements(n,"inherit") {
                    c.inheritance.push(Inheritance {types:elements(rule,"type_pair").map(|p|(p.attribute("source").unwrap_or("*").into(),p.attribute("target").unwrap_or("*").into())).collect(),fields:elements(rule,"field").map(|f|(f.attribute("source").unwrap_or("").into(),if yes(f,"skip") {None} else {Some(f.attribute("target").or_else(||f.attribute("source")).unwrap_or("").into())})).collect(),overrides:elements(rule,"field").filter(|f|yes(*f,"override_target")).map(|f|(f.attribute("source").unwrap_or("").into(),f.attribute("target").or_else(||f.attribute("source")).unwrap_or("").into())).collect()});
                }
            }
            "extradatespec" => { c.extradate_spec=elements(n,"scope").map(|s|elements(s,"field").map(text).collect()).collect(); }
            "presort" => {c.options.insert("presort".into(),text(n));}
            "sortexclusion" | "sortinclusion" => {
                let key=n.tag_name().name();
                let member=if key=="sortexclusion"{"exclusion"}else{"inclusion"};
                let values=elements(n,member).map(text).collect::<Vec<_>>().join(",");
                c.type_options.entry(n.attribute("type").unwrap_or("*").into()).or_default().insert(key.into(),values);
            }
            "nosorts" | "nonamestrings" => {
                let key=if n.tag_name().name()=="nosorts"{"nosort"}else{"nonamestring"};
                for rule in elements(n,key) {
                    let field=rule.attribute("field").map(str::to_owned).or_else(||elements(rule,"field").next().map(text)).unwrap_or_default();
                    let value=rule.attribute("value").map(str::to_owned).or_else(||elements(rule,"value").next().map(text)).unwrap_or_default();
                    let rules=c.options.entry(format!("{key}.{field}")).or_default();
                    if !rules.is_empty(){rules.push('\n');}
                    rules.push_str(&value);
                    c.text_rules.entry(format!("{key}.{field}")).or_default().push(value);
                }
            }
            "noinits" | "nolabels" | "nolabelwidthcounts" => {
                let key=match n.tag_name().name() {"noinits"=>"noinit","nolabels"=>"nolabel",_=>"nolabelwidthcount"};
                let values=elements(n,key).map(|p|p.attribute("value").map(str::to_owned).unwrap_or_else(||text(p))).collect::<Vec<_>>();
                if !values.is_empty(){c.options.insert(key.into(),values.join("\n"));c.text_rules.insert(key.into(),values);}
            }
            "namehashtemplate" | "uniquenametemplate" | "labelalphanametemplate" | "sortingnamekeytemplate" => {
                let part=|p:Node<'_, '_>|NameTemplatePart {name:text(p),attributes:p.attributes().map(|a|(a.name().into(),a.value().into())).collect()};
                let ordered=|tag:&'static str| {let mut nodes=elements(n,tag).collect::<Vec<_>>();nodes.sort_by_key(|p|number(*p,"order").unwrap_or(0));nodes};
                let groups=if n.tag_name().name()=="sortingnamekeytemplate" {ordered("keypart").into_iter().map(|g|{let mut parts=elements(g,"part").collect::<Vec<_>>();parts.sort_by_key(|p|number(*p,"order").unwrap_or(0));parts.into_iter().map(part).collect()}).collect()}else{vec![ordered("namepart").into_iter().map(part).collect()]};
                c.name_templates.entry(n.tag_name().name().into()).or_default().insert(n.attribute("name").unwrap_or("global").into(),groups);
                if n.tag_name().name()=="sortingnamekeytemplate" {c.options.insert(format!("sortingnamekeytemplate.visibility.{}",n.attribute("name").unwrap_or("global")),n.attribute("visibility").unwrap_or("sort").into());}
            }
            _=>{}
        }
    }
    parse_fieldsets(root,&mut c);
    c.labelname=c.options.get("labelnamespec").map(|s|s.split(',').map(str::to_owned).collect()).unwrap_or_else(||vec!["author".into(),"editor".into(),"translator".into()]);
    c.labeltitle=c.options.get("labeltitlespec").map(|s|s.split(',').map(str::to_owned).collect()).unwrap_or_else(||vec!["shorttitle".into(),"title".into()]);
    c.labeldate=c.options.get("labeldatespec").map(|s|s.split(',').map(str::to_owned).collect()).unwrap_or_else(||vec!["date".into(),"year".into(),"nodate".into()]);
    c.extradate_context=c.options.get("extradatecontext").map(|s|s.split(',').map(str::to_owned).collect()).unwrap_or_default();
    c.sections.sort_by_key(|s|s.number);
    if c.sections.is_empty() {return Err("BCF contains no bibliography sections".into())}
    Ok(c)
}
pub(crate) fn parse_fieldsets(root:Node<'_, '_>,c:&mut Control) {
    let sets=elements(root,"datafieldset").collect::<Vec<_>>();
    for _ in 0..sets.len().max(1) {for set in &sets {
        let mut fields=Vec::new();for member in elements(*set,"member") {
            if let Some(field)=member.attribute("field"){
                if let Some(set)=c.options.get(&format!("datafieldset.{field}")){fields.extend(set.split(',').map(str::to_owned));}else{fields.push(field.to_owned());}
            }else{for (field,spec) in &c.fields {if member.attribute("datatype").is_none_or(|s|s==spec.datatype)&&member.attribute("fieldtype").is_none_or(|s|s==spec.fieldtype){fields.push(field.clone());}}}
        }
        fields.sort();fields.dedup();c.options.insert(format!("datafieldset.{}",set.attribute("name").unwrap_or("")),fields.join(","));
    }}
}
