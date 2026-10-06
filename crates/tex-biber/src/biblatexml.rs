//! Biber 2.22's BibLaTeXML datasource driver.
//!
//! XML values are already structured data: unlike BibTeX input, neither text
//! fields nor name parts undergo LaTeX decoding or BibTeX list/name splitting.
use crate::bib::Database;
use crate::model::{namelist_id, Annotation, Control, Entry, Name, NameList, Section};
use crate::{dates, names};
use roxmltree::{Document, Node, ParsingOptions};
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt::Write;
use unicode_normalization::{is_nfd, UnicodeNormalization};

const NAMESPACE: &str = "http://biblatex-biber.sourceforge.net/biblatexml";

#[path = "biblatexml/dtd.rs"]
mod dtd;
#[path = "biblatexml/sourcemap.rs"]
mod sourcemap;

#[derive(Clone, Copy)]
struct XmlNode<'a, 'input> {
    node: Node<'a, 'input>,
    defaults: &'a dtd::Defaults,
}
impl<'a, 'input> std::ops::Deref for XmlNode<'a, 'input> {
    type Target = Node<'a, 'input>;
    fn deref(&self) -> &Self::Target { &self.node }
}
impl<'a, 'input> XmlNode<'a, 'input> {
    fn attribute(&self, name: &str) -> Option<&'a str> {
        self.node.attribute(name).or_else(|| {
            let input = self.node.document().input_text();
            let lexical = input[self.node.range()].trim_start_matches('<')
                .split(|c: char| c.is_whitespace() || c == '>' || c == '/').next()?;
            self.defaults.get(lexical)?.get(name).map(String::as_str)
        })
    }
}

fn elements<'a, 'input>(node: XmlNode<'a, 'input>, name: &'static str) -> impl Iterator<Item = XmlNode<'a, 'input>> {
    node.children().filter(move |n| n.is_element() && n.range().start >= n.document().root_element().range().start && n.tag_name().namespace() == Some(NAMESPACE) && n.tag_name().name() == name).map(move |n| XmlNode { node: n, defaults: node.defaults })
}

fn content(node: XmlNode<'_, '_>) -> String {
    node.descendants().filter(Node::is_text).filter_map(|n| n.text()).collect()
}

// XML::LibXML does not load external entities with the driver's default parser
// settings. roxmltree skips their declarations instead, so declare them empty.
fn external_entities(input: &str) -> Cow<'_, str> {
    let Some(mut pos) = input.find("<!DOCTYPE") else { return Cow::Borrowed(input); };
    let bytes = input.as_bytes();
    let mut subset = 0usize;
    let mut quote = 0u8;
    let mut replacements = Vec::new();
    while pos < bytes.len() {
        if quote != 0 {
            if bytes[pos] == quote { quote = 0; }
            pos += 1;
            continue;
        }
        if bytes[pos..].starts_with(b"<!--") {
            let Some(end) = input[pos + 4..].find("-->") else { break; };
            pos += end + 7;
            continue;
        }
        if bytes[pos..].starts_with(b"<!ENTITY") {
            let start = pos;
            pos += 8;
            let mut entity_quote = 0u8;
            while pos < bytes.len() {
                let byte = bytes[pos];
                if entity_quote != 0 { if byte == entity_quote { entity_quote = 0; } }
                else if matches!(byte, b'\'' | b'"') { entity_quote = byte; }
                else if byte == b'>' { break; }
                pos += 1;
            }
            if pos == bytes.len() { break; }
            let declaration = input[start + 8..pos].trim_start();
            if let Some(split) = declaration.find(char::is_whitespace) {
                let name = &declaration[..split];
                let definition = declaration[split..].trim_start();
                let (kind, mut tail) = definition.split_once(char::is_whitespace).unwrap_or(("", ""));
                let count = match kind { "SYSTEM" => 1, "PUBLIC" => 2, _ => 0 };
                let mut valid = count != 0;
                for _ in 0..count {
                    tail = tail.trim_start();
                    let Some(delimiter) = tail.chars().next().filter(|c| matches!(c, '\'' | '"')) else { valid = false; break; };
                    let Some(end) = tail[1..].find(delimiter) else { valid = false; break; };
                    tail = &tail[end + 2..];
                }
                if name != "%" && valid && tail.trim().is_empty() {
                    replacements.push((start, pos + 1, format!("<!ENTITY {name} \"\">")));
                }
            }
            pos += 1;
            continue;
        }
        match bytes[pos] {
            b'\'' | b'"' => quote = bytes[pos],
            b'[' => subset += 1,
            b']' => subset = subset.saturating_sub(1),
            b'>' if subset == 0 => break,
            _ => {}
        }
        pos += 1;
    }
    if replacements.is_empty() { return Cow::Borrowed(input); }
    let mut result = String::with_capacity(input.len());
    let mut previous = 0;
    for (start, end, replacement) in replacements {
        result.push_str(&input[previous..start]);
        result.push_str(&replacement);
        previous = end;
    }
    result.push_str(&input[previous..]);
    Cow::Owned(result)
}

fn xdata(control: &Control, node: XmlNode<'_, '_>) -> Option<String> {
    node.attribute("xdata").filter(|s| !s.is_empty() && *s != "0").map(|value| {
        let marker = control.options.get("xdatamarker").map(String::as_str).unwrap_or("xdata");
        let sep = control.options.get("xnamesep").map(String::as_str).unwrap_or("=");
        format!("{marker}{sep}{value}")
    })
}

fn list(control: &Control, node: XmlNode<'_, '_>) -> Vec<String> {
    let items = elements(node, "list").flat_map(|n| elements(n, "item")).collect::<Vec<_>>();
    if items.is_empty() { vec![content(node)] }
    else { items.into_iter().map(|n| xdata(control, n).unwrap_or_else(|| content(n))).collect() }
}

fn store_option(options: &mut std::collections::BTreeMap<String, String>, key: &str, value: &str) {
    if key == "nametemplates" {
        for name in ["sortingnamekeytemplatename", "uniquenametemplatename", "labelalphanametemplatename", "namehashtemplatename"] {
            options.insert(name.to_owned(), value.to_owned());
        }
    } else { options.insert(key.to_owned(), value.to_owned()); }
}

fn name_options(node: XmlNode<'_, '_>, list: bool) -> std::collections::BTreeMap<String, String> {
    let mut options = std::collections::BTreeMap::new();
    for key in ["useprefix", "nametemplates", "sortingnamekeytemplatename", "uniquenametemplatename", "labelalphanametemplatename", "namehashtemplatename", "uniquename", "familyinits", "giveninits", "prefixinits", "suffixinits", "terseinits", "uniquelist", "nohashothers", "nosortothers"] {
        if !list && matches!(key, "uniquelist" | "nohashothers" | "nosortothers") { continue; }
        if let Some(value) = node.attribute(key) { store_option(&mut options, key, value); }
    }
    options
}

fn parse_name(control: &Control, node: XmlNode<'_, '_>) -> Name {
    let mut name = Name { hashid: node.attribute("id").unwrap_or("").to_owned(), options: name_options(node, false), ..Default::default() };
    if let Some(reference) = xdata(control, node) {
        name.options.insert("xdata".to_owned(), reference);
        return name;
    }
    let parts: Vec<&str> = if control.nameparts.is_empty() { vec!["family", "given", "prefix", "suffix"] }
        else { control.nameparts.iter().map(String::as_str).collect() };
    for part in parts {
        let Some(np) = elements(node, "namepart").find(|n| n.attribute("type") == Some(part)) else { continue };
        let children = elements(np, "namepart").collect::<Vec<_>>();
        let (value, tokens) = if children.is_empty() {
            let value = content(np);
            if value.is_empty() { continue; }
            // The 2.22 driver reads this attribute from the *name*, not namepart.
            let tokens = node.attribute("initial").filter(|v| !v.is_empty() && *v != "0")
                .map(|v| vec![v.to_owned()]).unwrap_or_else(|| names::initial_tokens_xml(control, &value));
            (value, tokens)
        } else {
            let values = children.iter().map(|n| content(*n)).collect::<Vec<_>>();
            let value = names::join_name_parts(&values.iter().map(String::as_str).collect::<Vec<_>>());
            let tokens = children.iter().zip(&values).flat_map(|(n, value)| {
                n.attribute("initial").filter(|v| !v.is_empty() && *v != "0").map(|v| vec![v.to_owned()])
                    .unwrap_or_else(|| names::initial_tokens_xml(control, value))
            }).collect();
            (value, tokens)
        };
        name.set_part(part, value);
        name.initials.insert(part.to_owned(), names::render_initials(&tokens));
        name.initial_tokens.insert(part.to_owned(), tokens);
    }
    name
}

fn parse_names(control: &Control, node: XmlNode<'_, '_>) -> NameList {
    NameList {
        id: namelist_id(),
        names: elements(node, "name").map(|n| parse_name(control, n)).collect(),
        // Perl truth, not XML Schema boolean: the string "false" is truthy.
        others: node.attribute("morenames").is_some_and(|v| !v.is_empty() && v != "0"),
        options: name_options(node, true),
        ..Default::default()
    }
}

fn uri(value: String) -> String {
    if value.contains('%') { return value; }
    let mut result = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~:/?#[]@!$&'()*+,;=".contains(&byte) { result.push(byte as char); }
        else { let _ = write!(result, "%{byte:02X}"); }
    }
    result
}

fn annotation(entry: &mut Entry, node: XmlNode<'_, '_>) {
    let Some(field) = node.attribute("field").filter(|v| !v.is_empty()) else {
        entry.fields.insert("annotation".to_owned(), content(node));
        return;
    };
    let part = node.attribute("part").unwrap_or("");
    let item = node.attribute("item").unwrap_or("");
    let value = Annotation {
        field: field.to_owned(), name: node.attribute("name").filter(|v| !v.is_empty() && *v != "0").unwrap_or("default").to_owned(),
        item: item.to_owned(), part: part.to_owned(),
        literal: node.attribute("literal").is_some_and(|v| !v.is_empty() && v != "0"), value: content(node),
    };
    if let Some(old) = entry.annotations.iter_mut().find(|a| a.field == value.field && a.name == value.name && a.item == value.item && a.part == value.part) {
        *old = value;
    } else { entry.annotations.push(value); }
}

fn related(entry: &mut Entry, node: XmlNode<'_, '_>) {
    for item in elements(node, "list").flat_map(|n| elements(n, "item")) {
        entry.fields.insert("related".to_owned(), item.attribute("ids").unwrap_or("").to_owned());
        entry.fields.insert("relatedtype".to_owned(), item.attribute("type").unwrap_or("").to_owned());
        if let Some(value) = item.attribute("string").filter(|v| !v.is_empty() && *v != "0") {
            entry.fields.insert("relatedstring".to_owned(), value.to_owned());
        }
        // Keep the driver's distinction between the trigger and value attribute.
        if item.attribute("options").is_some_and(|v| !v.is_empty() && v != "0") {
            entry.fields.insert("relatedoptions".to_owned(), item.attribute("relatedoptions").unwrap_or("").to_owned());
        }
    }
}

/// Resolve the internal subset and unloaded external entities without adding
/// getter-only default attributes to the document's attribute axis.
pub(crate) fn preprocess(input: &str) -> Result<Cow<'_, str>, String> {
    match dtd::prepare(input)? {
        Cow::Borrowed(text) => Ok(external_entities(text)),
        Cow::Owned(text) => Ok(Cow::Owned(external_entities(&text).into_owned())),
    }
}

/// Append datasource entries in their original order. Callers must not run
/// `bib::normalize` on these entries; names, lists and dates are already parsed.
/// XML XPath sourcemaps run here before typed ingestion and may mutate citation
/// keys in `section`; the datatype marker prevents the shared mapper replaying
/// them. Internal DTD defaults are getter-only, not synthetic XPath attributes.
/// `remote` describes an actual fetch, not a URL-shaped callback-resolved path;
/// remote maps cannot match the original source because Biber uses a temp name.
/// Shared inheritance, processing and output still apply.
pub fn add(control: &Control, section: &mut Section, db: &mut Database, input: &str, source: &str, remote: bool) -> Result<(), String> {
    if input.trim().is_empty() {
        db.warnings.push(format!("Data source '{source}' is empty, ignoring"));
        return Ok(());
    }
    let normalized: Cow<'_, str> = if is_nfd(input) { Cow::Borrowed(input) } else { Cow::Owned(input.nfd().collect()) };
    let normalized = preprocess(&normalized)?;
    let document = Document::parse_with_options(&normalized, ParsingOptions { allow_dtd: true, ..Default::default() })
        .map_err(|e| format!("Invalid BibLaTeXML in '{source}': {e}"))?;
    let defaults = dtd::defaults(&normalized)?;
    let mapped = sourcemap::apply(control, section, &document, &defaults, source, remote, &mut db.warnings)?;
    let normalized = mapped.as_deref().unwrap_or(&normalized);
    let mapped_document = mapped.as_deref().map(|text| Document::parse_with_options(text, ParsingOptions { allow_dtd: true, ..Default::default() })
        .map_err(|e| format!("Invalid mapped BibLaTeXML in '{source}': {e}"))).transpose()?;
    let document = mapped_document.as_ref().unwrap_or(&document);
    let root = XmlNode { node: document.root_element(), defaults: &defaults };
    if root.tag_name().namespace() != Some(NAMESPACE) || root.tag_name().name() != "entries" { return Ok(()); }
    // XML::LibXML's field XPath lookups require the literal bltx prefix binding.
    if root.lookup_namespace_uri(Some("bltx")) != Some(NAMESPACE) {
        return Err(format!("BibLaTeXML in '{source}' has no bltx namespace binding"));
    }
    for node in elements(root, "entry") {
        let Some(key) = node.attribute("id") else {
            db.warnings.push(format!("Invalid or undefined BibLaTeXML entry key in file '{source}', skipping ..."));
            continue;
        };
        // The 2.22 allkeys driver leaves earlier aliases registered when a
        // later real key collides; that later object is hidden by the alias.
        let shadowed = section.citekeys.iter().any(|key| key == "*")
            && db.entries.iter().any(|e| e.fields.get("ids").is_some_and(|ids| ids.split(',').any(|alias| alias == key)));
        if shadowed { db.warnings.push(format!("Citekey alias '{key}' is also a real entry key, skipping ...")); }
        if db.entries.iter().any(|e| e.key == key) {
            db.warnings.push(format!("Duplicate entry key: '{key}' in file '{source}', skipping ..."));
            continue;
        }
        let mut entry = Entry { key: key.to_owned(), kind: node.attribute("entrytype").unwrap_or("").to_owned(), source: source.to_owned(), ..Default::default() };
        entry.computed.insert("sourcemap_datatype".to_owned(), "biblatexml-preprocessed".to_owned());
        let mut seen = BTreeSet::new();
        for field_node in node.children().filter(|n| n.is_element() && n.tag_name().namespace() == Some(NAMESPACE)).map(|n| XmlNode { node: n, defaults: &defaults }) {
            if field_node.range().start < root.range().start { continue; }
            let tag = field_node.tag_name().name().to_lowercase();
            let lexical = normalized[field_node.range()].trim_start_matches('<')
                .split(|c: char| c.is_whitespace() || c == '>' || c == '/').next().unwrap_or("");
            if lexical.split_once(':').is_some_and(|(prefix, _)| prefix != "bltx") { continue; }
            let field = if tag == "names" { field_node.attribute("type").unwrap_or("").to_owned() }
                else if tag == "date" { format!("{}date", field_node.attribute("type").unwrap_or("")) }
                else { tag.clone() };
            if tag == "annotation" {
                if control.fields.contains_key("annotation") { annotation(&mut entry, field_node); }
                continue;
            }
            let Some(spec) = control.fields.get(&field) else { continue; };
            if !lexical.contains(':') && tag != "names" {
                return Err(format!("BibLaTeXML field '{field}' in '{source}' has no bltx prefix"));
            }
            if tag != "date" && !seen.insert(field.clone()) { continue; }
            if tag == "related" { related(&mut entry, field_node); continue; }
            if tag == "names" && spec.datatype == "name" {
                entry.fields.insert(field.clone(), content(field_node));
                entry.names.insert(field, parse_names(control, field_node));
            } else if spec.datatype == "date" {
                let start = elements(field_node, "start").next();
                if let Some(start) = start {
                    let start = content(start);
                    let end = elements(field_node, "end").next().map(content).unwrap_or_default();
                    dates::parse_xml(control, &mut entry, &field, &start, Some(&end))?;
                } else { dates::parse_xml(control, &mut entry, &field, &content(field_node), None)?; }
            } else if spec.datatype == "range" {
                if let Some(reference) = xdata(control, field_node) { entry.fields.insert(field, reference); continue; }
                let ranges = elements(field_node, "list").flat_map(|n| elements(n, "item")).map(|n| {
                    let start = elements(n, "start").next().map(content).unwrap_or_default();
                    let end = elements(n, "end").next().map(content).unwrap_or_default();
                    (start,end)
                }).collect::<Vec<_>>();
                if !ranges.is_empty() {
                    entry.fields.insert(field.clone(), ranges.iter().map(|(start,end)| format!("{start}--{end}")).collect::<Vec<_>>().join(","));
                    entry.ranges.insert(field, ranges);
                }
            } else if field == "ids" {
                if !section.citekeys.iter().any(|key| key == "*") { continue; }
                let mut ids = Vec::new();
                for id in elements(field_node, "key").map(content) {
                    if db.entries.iter().any(|e| e.key == id) {
                        db.warnings.push(format!("Citekey alias '{id}' is also a real entry key, skipping ..."));
                        continue;
                    }
                    if let Some(other) = db.entries.iter().find(|e| e.fields.get("ids").is_some_and(|v| v.split(',').any(|alias| alias == id))) {
                        if other.key != key { db.warnings.push(format!("Citekey alias '{id}' already has an alias '{}', skipping ...", other.key)); }
                        continue;
                    }
                    ids.push(id);
                }
                entry.fields.insert(field, ids.join(","));
            } else if spec.fieldtype == "list" || spec.format == "xsv" {
                let values = list(control, field_node);
                entry.fields.insert(field.clone(), values.join(if spec.format == "xsv" { "," } else { " and " }));
                entry.lists.insert(field, values);
            } else {
                let mut value = xdata(control, field_node).unwrap_or_else(|| content(field_node));
                if spec.datatype == "uri" && field_node.attribute("xdata").is_none() { value = uri(value); }
                dates::authored_field(&mut entry, &field);
                entry.fields.insert(field, value);
            }
        }
        if !shadowed { db.entries.push(entry); }
    }
    Ok(())
}
