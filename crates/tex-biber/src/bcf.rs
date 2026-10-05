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
    for n in root.children().filter(Node::is_element) {
        match n.tag_name().name() {
            "options" => {
                let mut opts=BTreeMap::new();
                for o in elements(n,"option") {
                    if let Some(k)=elements(o,"key").next() {
                        let vals=elements(o,"value").map(text).collect::<Vec<_>>();
                        if text(k)=="labeldatespec" && n.attribute("type")==Some("global") { c.labeldate_specs=elements(o,"value").map(|v|LabelDateSpec {name:text(v),literal:v.attribute("type")==Some("string")}).collect(); }
                        opts.insert(text(k),vals.join(","));
                    }
                }
                if n.attribute("type")==Some("global") { c.options.extend(opts); }
                else if let Some(kind)=n.attribute("type") { c.type_options.entry(kind.to_owned()).or_default().extend(opts); }
            }
            "datamodel" => {
                for d in n.descendants().filter(Node::is_element) {
                    if d.tag_name().name()=="field" && d.attribute("datatype").is_some() {
                        c.field_order.push(text(d));
                        c.fields.insert(text(d),FieldSpec {datatype:d.attribute("datatype").unwrap_or("").into(),fieldtype:d.attribute("fieldtype").unwrap_or("field").into(),format:d.attribute("format").unwrap_or("").into(),label:yes(d,"label"),skip_output:yes(d,"skip_output"),nullok:yes(d,"nullok")});
                    } else if d.tag_name().name()=="entrytype" && yes(d,"skip_output") { c.skip_types.insert(text(d)); }
                    else if d.tag_name().name()=="constant" && d.attribute("name")==Some("nameparts") { c.nameparts=text(d).split(',').map(str::to_owned).collect(); }
                }
            }
            "bibdata" => {
                let s=section(&mut c,number(n,"section").unwrap_or(0));
                for d in elements(n,"datasource") { let name=text(d); if !s.sources.contains(&name) {s.sources.push(name)} }
            }
            "section" => {
                let s=section(&mut c,number(n,"number").unwrap_or(0));
                for key in elements(n,"citekey") {
                    let keytext=text(key);
                    if !s.citekeys.contains(&keytext) {s.citekeys.push(keytext.clone());}
                    s.cite_orders.entry(keytext.clone()).or_insert((number(key,"order").unwrap_or(1),number(key,"intorder").unwrap_or(1)));
                    if yes(key,"nocite") || key.attribute("type")==Some("nocite") {s.nocite.insert(keytext);}
                }
                for count in elements(n,"citekeycount") {s.cite_counts.insert(text(count),count.attribute("count").and_then(|s|s.parse().ok()).unwrap_or(-1));}
            }
            "sortingtemplate" => {
                let sorts=elements(n,"sort").map(|s| {
                    let mut item=SortItem {descending:s.attribute("direction")==Some("descending"),final_:yes(s,"final"),..Default::default()};
                    for f in elements(s,"sortitem") {
                        if yes(f,"literal") {item.literal=Some(text(f));} else {item.fields.push(text(f));}
                        item.substring_side=f.attribute("substring_side").map(str::to_owned);
                        item.substring_width=number(f,"substring_width");
                        item.pad_side=f.attribute("pad_side").map(str::to_owned);
                        item.pad_width=number(f,"pad_width");
                        item.pad_char=f.attribute("pad_char").map(str::to_owned);
                    }
                    item
                }).collect();
                c.sorting.insert(n.attribute("name").unwrap_or("global").into(),sorts);
            }
            "datalist" => {
                section(&mut c,number(n,"section").unwrap_or(0)).lists.push(DataList {name:n.attribute("name").unwrap_or("").into(),sorting:n.attribute("sortingtemplatename").unwrap_or("nyt").into(),kind:n.attribute("type").unwrap_or("entry").into(),labelprefix:n.attribute("labelprefix").unwrap_or("").into(),namekey:n.attribute("sortingnamekeytemplatename").unwrap_or("global").into()});
            }
            "labelalphatemplate" if n.attribute("type")==Some("global") => {
                c.labelalpha=elements(n,"labelelement").map(|e|LabelPart {fields:elements(e,"labelpart").map(|p|LabelField {name:text(p),literal:yes(p,"literal"),strwidth:number(p,"substring_width"),strside:p.attribute("substring_side").unwrap_or("left").into(),ifnames:number(p,"ifnames"),names:number(p,"names"),final_:yes(p,"final")}).collect()}).collect();
            }
            "sourcemap" => {
                for maps in elements(n,"maps") {
                    if maps.attribute("datatype").is_some_and(|s|s!="bibtex") { c.warnings.push("Only BibTeX sourcemaps are supported".into()); continue; }
                    for map in elements(maps,"map") {
                        let mut m=SourceMap {overwrite:yes(map,"map_overwrite") || yes(maps,"map_overwrite"),..Default::default()};
                        for step in map.children().filter(Node::is_element) {
                            match step.tag_name().name() {
                                "per_type" => m.per_type.push(text(step)),
                                "per_datasource" => m.per_source.push(text(step)),
                                "map_step" => m.steps.push(step.attributes().map(|a|(a.name().to_owned(),a.value().to_owned())).collect()),
                                other => c.warnings.push(format!("Unsupported sourcemap element {other}")),
                            }
                        }
                        for step in &m.steps {for name in ["map_match","map_matchi","map_notmatch","map_notmatchi"] {if let Some(pattern)=step.get(name){let insensitive=name.ends_with('i');match regex::RegexBuilder::new(pattern).case_insensitive(insensitive).build(){Ok(re)=>{m.patterns.insert(format!("{name}:{pattern}"),re);},Err(e)=>c.warnings.push(format!("Unsupported sourcemap pattern '{pattern}': {e}"))}}}}
                        for step in &m.steps {for key in step.keys(){if !["map_field_source","map_field_target","map_field_set","map_field_value","map_type_source","map_type_target","map_final","map_null","map_match","map_matchi","map_notmatch","map_notmatchi","map_replace","map_origfieldval","map_origfield","map_origentrytype","map_append"].contains(&key.as_str()){c.warnings.push(format!("Unsupported sourcemap step {key}"));}}}
                        c.sourcemaps.push(m);
                    }
                }
            }
            "inheritance" => {
                for rule in elements(n,"inherit") {
                    c.inheritance.push(Inheritance {types:elements(rule,"type_pair").map(|p|(p.attribute("source").unwrap_or("*").into(),p.attribute("target").unwrap_or("*").into())).collect(),fields:elements(rule,"field").map(|f|(f.attribute("source").unwrap_or("").into(),if yes(f,"skip") {None} else {Some(f.attribute("target").or_else(||f.attribute("source")).unwrap_or("").into())})).collect(),override_target:yes(rule,"override_target")});
                }
            }
            "extradatespec" => { c.extradate_spec=elements(n,"scope").map(|s|elements(s,"field").map(text).collect()).collect(); }
            "presort" => {c.options.insert("presort".into(),text(n));}
            "namehashtemplate" | "uniquenametemplate" | "labelalphanametemplate" | "sortingnamekeytemplate" => {
                let part=|p:Node<'_, '_>|NameTemplatePart {name:text(p),attributes:p.attributes().map(|a|(a.name().into(),a.value().into())).collect()};
                let groups=if n.tag_name().name()=="sortingnamekeytemplate" {elements(n,"keypart").map(|g|elements(g,"part").map(part).collect()).collect()}else{vec![elements(n,"namepart").map(part).collect()]};
                c.name_templates.entry(n.tag_name().name().into()).or_default().insert(n.attribute("name").unwrap_or("global").into(),groups);
            }
            _=>{}
        }
    }
    c.labelname=c.options.get("labelnamespec").map(|s|s.split(',').map(str::to_owned).collect()).unwrap_or_else(||vec!["author".into(),"editor".into(),"translator".into()]);
    c.labeltitle=c.options.get("labeltitlespec").map(|s|s.split(',').map(str::to_owned).collect()).unwrap_or_else(||vec!["shorttitle".into(),"title".into()]);
    c.labeldate=c.options.get("labeldatespec").map(|s|s.split(',').map(str::to_owned).collect()).unwrap_or_else(||vec!["date".into(),"year".into(),"nodate".into()]);
    c.extradate_context=c.options.get("extradatecontext").map(|s|s.split(',').map(str::to_owned).collect()).unwrap_or_default();
    c.sections.sort_by_key(|s|s.number);
    if c.sections.is_empty() {return Err("BCF contains no bibliography sections".into())}
    Ok(c)
}
