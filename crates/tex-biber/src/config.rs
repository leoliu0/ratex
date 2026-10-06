use crate::model::Control;
use roxmltree::{Document, Node};

/// Biber's configuration file is XML, not an INI file. Keep regular-expression
/// lists intact: commas and whitespace inside a pattern are significant.
pub(crate) fn apply(control: &mut Control, xml: &str) -> Result<(), String> {
    let document = Document::parse(xml).map_err(|error| format!("Invalid Biber configuration XML: {error}"))?;
    let root = document.root_element();
    if root.tag_name().name() != "config" {
        return Err("Biber configuration root is not config".into());
    }
    let namespaces=root.namespaces().map(|namespace|{
        let name=namespace.name().map(|name|format!("xmlns:{name}")).unwrap_or_else(||"xmlns".into());
        let value=namespace.uri().replace('&',"&amp;").replace('"',"&quot;");
        format!(" {name}=\"{value}\"")
    }).collect::<String>();
    for node in root.children().filter(Node::is_element) {
        let key = node.tag_name().name();
        match key {
            "noinit" | "nolabel" | "nolabelwidthcount" => {
                let values = node.children().filter(Node::is_element)
                    .filter(|child| child.tag_name().name() == "option")
                    .map(|child| child.attribute("value").unwrap_or("").to_owned())
                    .collect::<Vec<_>>();
                control.options.insert(key.into(), values.join("\n"));
                control.text_rules.insert(key.into(),values);
            }
            "nosort" | "nonamestring" => {
                let mut values = std::collections::BTreeMap::<String, Vec<String>>::new();
                for option in node.children().filter(Node::is_element) {
                    if option.tag_name().name() != "option" { continue; }
                    let Some(name) = option.attribute("name") else { continue; };
                    values.entry(name.into()).or_default().push(option.attribute("value").unwrap_or("").into());
                }
                for (name, patterns) in values {
                    control.options.insert(format!("{key}.{name}"), patterns.join("\n"));
                    control.text_rules.insert(format!("{key}.{name}"),patterns);
                }
            }
            "collate_options" => {
                for option in node.children().filter(Node::is_element) {
                    if option.tag_name().name() != "option" { continue; }
                    if let Some(name)=option.attribute("name") {
                        control.options.insert(format!("collate_options.{name}"), option.attribute("value").unwrap_or("\0").into());
                    }
                }
            }
            "transliteration" => {
                let kind = node.attribute("entrytype").unwrap_or("*");
                for rule in node.children().filter(Node::is_element) {
                    if rule.tag_name().name() != "translit" { continue; }
                    let (Some(target), Some(from), Some(to)) = (rule.attribute("target"), rule.attribute("from"), rule.attribute("to")) else { continue; };
                    let options = if kind == "*" { &mut control.options } else { control.type_options.entry(kind.into()).or_default() };
                    let rules = options.entry("translit_rules".into()).or_default();
                    if !rules.is_empty() { rules.push('\n'); }
                    rules.push_str(target);rules.push('\t');rules.push_str(from);rules.push('\t');rules.push_str(to);rules.push('\t');rules.push_str(rule.attribute("langids").unwrap_or("\0"));
                }
            }
            "presort" => {
                let value=node.text().unwrap_or("").trim().to_owned();
                if let Some(kind)=node.attribute("type"){control.type_options.entry(kind.into()).or_default().insert("presort".into(),value);}
                else{control.options.insert("presort".into(),value);}
            }
            "sortexclusion" | "sortinclusion" => {
                let member=if key=="sortexclusion"{"exclusion"}else{"inclusion"};
                let values=node.children().filter(Node::is_element).filter(|child|child.tag_name().name()==member)
                    .map(|child|child.text().unwrap_or("").trim()).collect::<Vec<_>>().join(",");
                control.type_options.entry(node.attribute("type").unwrap_or("*").into()).or_default().insert(key.into(),values);
            }
            "datafieldset"=>{},
            "sourcemap" | "inheritance" | "labelalphatemplate" | "labelalphanametemplate" | "namehashtemplate" | "uniquenametemplate" | "sortingnamekeytemplate" | "sortingtemplate" | "optionscope" | "datamodel" => {
                let fragment=&xml[node.range()];
                let wrapped=format!("<controlfile version=\"3.11\"{namespaces}><section number=\"0\"/>{fragment}</controlfile>");
                let parsed=crate::bcf::parse(&wrapped)?;
                control.warnings.extend(parsed.warnings);
                match key {
                    "sourcemap"=>{
                        let insertion=control.sourcemaps.iter().position(|map|map.level!="user").unwrap_or(control.sourcemaps.len());
                        let maps=parsed.sourcemaps.into_iter().filter(|map|map.level=="user");
                        control.sourcemaps.splice(insertion..insertion,maps);
                    },
                    "inheritance"=>control.inheritance.extend(parsed.inheritance),
                    "labelalphatemplate"=>control.labelalpha=parsed.labelalpha,
                    "sortingtemplate"=>control.sorting.extend(parsed.sorting),
                    "optionscope"=>control.options.extend(parsed.options),
                    "datamodel"=>{
                        control.fields.extend(parsed.fields);
                        for field in parsed.field_order{if !control.field_order.contains(&field){control.field_order.push(field);}}
                        control.skip_types.extend(parsed.skip_types);
                        if !parsed.nameparts.is_empty(){control.nameparts=parsed.nameparts;}
                        control.datamodel=parsed.datamodel;
                    },
                    _=>{
                        control.options.extend(parsed.options);
                        for (kind,templates) in parsed.name_templates{control.name_templates.entry(kind).or_default().extend(templates);}
                    },
                }
            }
            _ if !node.children().any(|child| child.is_element()) => {
                control.options.insert(key.into(), node.text().unwrap_or("").trim().into());
                if key=="sortlocale"{control.options.insert("sortlocale_override".into(),node.text().unwrap_or("").trim().into());}
            }
            _ => return Err(format!("Unsupported Biber configuration element '{key}'")),
        }
    }
    crate::bcf::parse_fieldsets(root,control);
    Ok(())
}
