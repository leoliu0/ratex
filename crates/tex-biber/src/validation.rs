//! Biber 2.22's declared datamodel validation, in its observable stage order.
use crate::model::{Control, Entry, FieldSpec};
use roxmltree::Node;
use std::collections::{BTreeMap, BTreeSet};
#[path = "validation_unicode.rs"]
mod unicode;

#[derive(Clone, Debug, Default)]
pub struct Datamodel {
    pub entrytypes: BTreeSet<String>,
    pub legal_fields: BTreeMap<String, BTreeSet<String>>,
    pub constraints: BTreeMap<String, Vec<Constraint>>,
}
#[derive(Clone, Debug)]
pub enum Constraint {
    Mandatory(String),
    Xor(Vec<String>),
    Or(Vec<String>),
    Conditional { antecedent_quant: String, antecedent: Vec<String>, consequent_quant: String, consequent: Vec<String> },
    Data { datatype: String, fields: Vec<String>, min: Option<String>, max: Option<String>, pattern: Option<String> },
}
fn children<'a, 'b>(node: Node<'a, 'b>, name: &'static str) -> impl Iterator<Item = Node<'a, 'b>> {
    node.children().filter(move |n| n.is_element() && n.tag_name().name() == name)
}
fn text(node: Node<'_, '_>) -> String { node.text().unwrap_or("").trim().to_owned() }
fn fields(node: Node<'_, '_>) -> Vec<String> { children(node, "field").map(text).collect() }
fn yes(node: Node<'_, '_>, name: &str) -> bool { matches!(node.attribute(name), Some("1" | "true")) }

pub fn parse(node: Node<'_, '_>, control: &mut Control) -> Result<(), String> {
    for section in children(node, "fields") {
        for field in children(section, "field") {
            let name = text(field);
            if !control.fields.contains_key(&name) { control.field_order.push(name.clone()); }
            control.fields.insert(name, FieldSpec {
                datatype: field.attribute("datatype").unwrap_or("").into(),
                fieldtype: field.attribute("fieldtype").unwrap_or("field").into(),
                format: field.attribute("format").unwrap_or("default").into(),
                skip_output: yes(field, "skip_output"), nullok: yes(field, "nullok"),
            });
        }
    }
    for (name, spec) in &control.fields {
        if spec.datatype == "date" && !name.ends_with("date") {
            return Err(format!("Fatal datamodel error: date field '{name}' must end with string 'date'"));
        }
    }
    for section in children(node, "entrytypes") {
        for entrytype in children(section, "entrytype") {
            let name = text(entrytype);
            control.datamodel.entrytypes.insert(name.clone());
            if yes(entrytype, "skip_output") { control.skip_types.insert(name); }
        }
    }
    for constant in children(node, "constants").flat_map(|n| children(n, "constant")) {
        if constant.attribute("name") == Some("nameparts") { control.nameparts = text(constant).split(',').map(str::to_owned).collect(); }
    }
    for kind in &control.datamodel.entrytypes {
        for section in children(node, "entryfields") {
            let types = children(section, "entrytype").map(text).collect::<Vec<_>>();
            if types.is_empty() || types.contains(kind) { control.datamodel.legal_fields.entry(kind.clone()).or_default().extend(fields(section)); }
        }
        let constraints = control.datamodel.constraints.entry(kind.clone()).or_default();
        for section in children(node, "constraints") {
            let types = children(section, "entrytype").map(text).collect::<Vec<_>>();
            if !types.is_empty() && !types.contains(kind) { continue; }
            for constraint in children(section, "constraint") {
                match constraint.attribute("type").unwrap_or("") {
                    "mandatory" => {
                        for field in fields(constraint) {
                            if !constraints.iter().any(|c| matches!(c, Constraint::Mandatory(f) if f == &field)) { constraints.push(Constraint::Mandatory(field)); }
                        }
                        constraints.extend(children(constraint, "fieldxor").map(|n| Constraint::Xor(fields(n))));
                        constraints.extend(children(constraint, "fieldor").map(|n| Constraint::Or(fields(n))));
                    }
                    "conditional" => {
                        if let (Some(a), Some(c)) = (children(constraint, "antecedent").next(), children(constraint, "consequent").next()) {
                            constraints.push(Constraint::Conditional { antecedent_quant: a.attribute("quant").unwrap_or("").into(), antecedent: fields(a), consequent_quant: c.attribute("quant").unwrap_or("").into(), consequent: fields(c) });
                        }
                    }
                    "data" => {
                        let pattern = constraint.attribute("pattern").filter(|s| !s.is_empty()).map(str::to_owned);
                        constraints.push(Constraint::Data { datatype: constraint.attribute("datatype").unwrap_or("").into(), fields: fields(constraint), min: constraint.attribute("rangemin").map(str::to_owned), max: constraint.attribute("rangemax").map(str::to_owned), pattern });
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn enabled(control: &Control) -> bool { matches!(control.options.get("validate_datamodel").map(String::as_str), Some("1" | "true")) }
fn prefix(entry: &Entry) -> String { format!("Datamodel: {} entry '{}' ({})", entry.kind, entry.key, entry.source) }
fn exists(entry: &Entry, field: &str) -> bool {
    matches!(field, "entrytype" | "citekey" | "datatype") || entry.fields.contains_key(field) || entry.names.contains_key(field) || entry.lists.contains_key(field) || entry.computed.contains_key(field)
}
fn value<'a>(entry: &'a Entry, field: &str) -> Option<&'a str> {
    if field == "entrytype" { return Some(&entry.kind); }
    if field == "citekey" { return Some(&entry.key); }
    entry.fields.get(field).or_else(|| entry.computed.get(field)).map(String::as_str)
}
fn remove(entry: &mut Entry, field: &str) {
    entry.fields.remove(field); entry.names.remove(field); entry.lists.remove(field); entry.ranges.remove(field); entry.computed.remove(field);
    entry.computed.remove(&format!("derivedfield:{field}")); entry.computed.remove(&format!("shadowderivedfield:{field}"));
}
fn truth(value: &str) -> bool { !value.is_empty() && value != "0" }

/// Run after datasource sourcemapping, before handlers discard unknown fields.
pub fn validate_input(control: &Control, entry: &mut Entry) {
    if !enabled(control) { return; }
    for field in entry.fields.keys() {
        if !control.fields.contains_key(field) && !field.contains('+') {
            entry.warnings.push(format!("{}: Field '{field}' invalid in data model - ignoring", prefix(entry)));
        }
    }
}

/// Run after field handlers and inheritance, before labels and uniqueness.
pub fn validate(control: &Control, entries: &mut [Entry]) -> Result<(), String> {
    if !enabled(control) { return Ok(()); }
    let mut patterns = BTreeMap::new();
    for entry in entries {
        if matches!(entry.kind.as_str(), "alias" | "missing") { continue; }
        let original_prefix = prefix(entry);
        if !control.datamodel.entrytypes.contains(&entry.kind) {
            entry.warnings.push(format!("{original_prefix}: Invalid entry type '{}' - defaulting to 'misc'", entry.kind));
            entry.kind = "misc".into();
        }
        let p = prefix(entry);
        let datafields = entry.fields.keys().chain(entry.names.keys()).chain(entry.lists.keys()).filter(|f| control.fields.contains_key(*f) && !entry.computed.contains_key(&format!("derivedfield:{f}"))).cloned().collect::<BTreeSet<_>>();
        if !matches!(entry.kind.as_str(), "xdata" | "set") {
            for field in &datafields {
                if !control.datamodel.legal_fields.get(&entry.kind).is_some_and(|fs| fs.contains(field)) {
                    entry.warnings.push(format!("{original_prefix}: Invalid field '{field}' for entrytype '{}'", entry.kind));
                }
            }
        }
        let constraints = control.datamodel.constraints.get(&entry.kind).map(Vec::as_slice).unwrap_or(&[]);
        for constraint in constraints {
            match constraint {
                Constraint::Mandatory(field) if !exists(entry, field) => entry.warnings.push(format!("{p}: Missing mandatory field '{field}'")),
                Constraint::Xor(fs) | Constraint::Or(fs) => {
                    let xor = matches!(constraint, Constraint::Xor(_));
                    let mut found = false;
                    for field in fs {
                        if exists(entry, field) && !(xor && field == "date" && truth(entry.computed.get("datesplit").map(String::as_str).unwrap_or(""))) {
                            if xor && found { entry.warnings.push(format!("{p}: Mandatory fields - only one of '{}' must be defined - ignoring field '{field}'", fs.join(", "))); remove(entry, field); }
                            found = true;
                        }
                    }
                    if !found { entry.warnings.push(format!("{p}: Missing mandatory field - one of '{}' must be defined", fs.join(", "))); }
                }
                _ => {}
            }
        }
        for constraint in constraints {
            if let Constraint::Conditional { antecedent_quant: aq, antecedent: afs, consequent_quant: cq, consequent: cfs } = constraint {
                let count = afs.iter().filter(|f| exists(entry, f)).count();
                if (aq == "all" && count != afs.len()) || (aq == "none" && count != 0) || (aq == "one" && count == 0) { continue; }
                let actual = cfs.iter().filter(|f| exists(entry, f)).collect::<Vec<_>>();
                if cq == "none" && !actual.is_empty() {
                    entry.warnings.push(format!("{p}: Constraint violation - {cq} of fields ({}) must exist when {aq} of fields ({}) exist. Ignoring them.", actual.iter().map(|f| f.as_str()).collect::<Vec<_>>().join(", "), afs.join(", ")));
                    for field in actual { remove(entry, field); }
                } else if (cq == "all" && actual.len() != cfs.len()) || (cq == "one" && actual.is_empty()) {
                    entry.warnings.push(format!("{p}: Constraint violation - {cq} of fields ({}) must exist when {aq} of fields ({}) exist", cfs.join(", "), afs.join(", ")));
                }
            }
        }
        for field in &datafields {
            if !exists(entry, field) { continue; }
            let Some(spec) = control.fields.get(field) else { continue; };
            let fv = value(entry, field).unwrap_or("");
            if spec.nullok && fv.is_empty() { continue; }
            let valid = if (spec.fieldtype == "list" && spec.datatype != "name") || spec.format == "xsv" {
                entry.lists.contains_key(field) || spec.format == "xsv"
            } else {
                match spec.datatype.as_str() {
                    "integer" => numeric(fv),
                    "datepart" => datepart(field, fv),
                    "name" => entry.names.contains_key(field),
                    "range" => !entry.names.contains_key(field),
                    "isbn" => identifier("isbn", fv),
                    // Biber's scalar ISSN/ISMN predicates use $_ rather than the argument.
                    "issn" | "ismn" => false,
                    _ => !entry.names.contains_key(field) && !entry.lists.contains_key(field),
                }
            };
            if !valid { entry.warnings.push(format!("{p}: Invalid value of field '{field}' must be datatype '{}' - ignoring field", spec.datatype)); remove(entry, field); }
        }
        for constraint in constraints {
            if let Constraint::Data { datatype, fields, min, max, pattern } = constraint {
                if datatype == "pattern" && pattern.is_none() { entry.warnings.push("Datamodel: Pattern constraint has no pattern!".into()); }
                for field in fields {
                    let Some(fv) = value(entry, field) else { continue; };
                    if !truth(fv) { continue; }
                    match datatype.as_str() {
                        "isbn" | "issn" | "ismn" => {
                            let invalid = if let Some(values) = entry.lists.get(field) {
                                values.iter().filter(|fv| !identifier(datatype, fv)).count()
                            } else { usize::from(!identifier(datatype, fv)) };
                            for _ in 0..invalid { entry.warnings.push(format!("{p}: Invalid {} in value of field '{field}'", datatype.to_uppercase())); }
                        }
                        "integer" | "datepart" => {
                            let n = perl_number(fv);
                            let invalid = min.as_ref().filter(|s| truth(s)).filter(|s| n < perl_number(s)).map(|s| (">=", s)).or_else(|| max.as_ref().filter(|s| truth(s)).filter(|s| n > perl_number(s)).map(|s| ("<=", s)));
                            if let Some((op, limit)) = invalid { entry.warnings.push(format!("{p}: Invalid value of field '{field}' must be '{op}{limit}' - ignoring field")); remove(entry, field); }
                        }
                        "pattern" => {
                            let matched = if let Some(pattern) = pattern {
                                if !patterns.contains_key(pattern.as_str()) { patterns.insert(pattern.as_str(), crate::perl_regex::Regex::compile(pattern, false)?); }
                                patterns[pattern.as_str()].inner.is_match(fv)?
                            } else { false };
                            if !matched { entry.warnings.push(format!("{p}: Invalid value (pattern match fails) for field '{field}'")); }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn numeric(value: &str) -> bool {
    let value = value.strip_prefix('-').unwrap_or(value);
    let mut chars = value.chars();
    let Some(first) = chars.next().map(u32::from) else { return false; };
    if chars.clone().next().is_none() {
        let i = unicode::NUMERIC_RANGES.partition_point(|(_, end)| *end < first);
        return unicode::NUMERIC_RANGES.get(i).is_some_and(|(start, _)| *start <= first);
    }
    let i = unicode::DECIMAL_ZEROS.partition_point(|zero| *zero <= first);
    let Some(&zero) = i.checked_sub(1).and_then(|i| unicode::DECIMAL_ZEROS.get(i)) else { return false; };
    first < zero + 10 && chars.all(|c| (zero..zero + 10).contains(&u32::from(c)))
}
fn perl_number(value: &str) -> f64 {
    let value = value.trim_start();
    for end in (1..=value.len()).rev().filter(|i| value.is_char_boundary(*i)) { if let Ok(n) = value[..end].parse::<f64>() { return n; } }
    0.0
}
fn datepart(field: &str, value: &str) -> bool {
    if field.ends_with("timezone") {
        if value == "Z" { return true; }
        let Some(rest) = value.strip_prefix('+').or_else(|| value.strip_prefix('-')) else { return false; };
        let mut digits = rest.chars();
        if !digits.next().is_some_and(decimal_digit) || !digits.next().is_some_and(decimal_digit) { return false; }
        let rest = digits.as_str();
        let rest = if let Some(minutes) = rest.strip_prefix("\\bibtzminsep") {
            let Some(first) = minutes.chars().next() else { return false; };
            if !first.is_whitespace() { return false; }
            &minutes[first.len_utf8()..]
        } else { rest };
        rest.is_empty() || (rest.chars().count() == 2 && rest.chars().all(decimal_digit))
    } else if field.ends_with("season") { ["winter", "spring", "summer", "autumn"].iter().any(|s| value.contains(s)) }
    else if field.ends_with("yeardivision") { ["spring", "summer", "autumn", "winter", "springN", "summerN", "autumnN", "winterN", "springS", "summerS", "autumnS", "winterS", "Q1", "Q2", "Q3", "Q4", "QD1", "QD2", "QD3", "S1", "S2"].contains(&value) }
    else { numeric(value) }
}
fn decimal_digit(c: char) -> bool {
    let code = u32::from(c);
    let i = unicode::DECIMAL_ZEROS.partition_point(|zero| *zero <= code);
    i.checked_sub(1).and_then(|i| unicode::DECIMAL_ZEROS.get(i)).is_some_and(|zero| code < zero + 10)
}
fn identifier(datatype: &str, value: &str) -> bool {
    let mut digits = [0u8; 13];
    let mut len = 0;
    for c in value.bytes().map(|c| c.to_ascii_uppercase()) {
        if c.is_ascii_digit() || (c == b'X' && datatype != "ismn") || (c == b'M' && datatype == "ismn") {
            if len == digits.len() { return false; }
            digits[len] = c; len += 1;
        }
    }
    let digits = &digits[..len];
    match datatype {
        // Business::ISBN->new creates an object even for invalid prefix/group/checksum.
        "isbn" => matches!(len, 10 | 13) && digits[..len-1].iter().all(u8::is_ascii_digit) && (digits[len-1].is_ascii_digit() || digits[len-1] == b'X'),
        "issn" => len == 8 && digits[..7].iter().all(u8::is_ascii_digit) && (digits[7].is_ascii_digit() || digits[7] == b'X') && digits.iter().enumerate().map(|(i,c)| u32::from(if *c == b'X' { 10 } else { *c - b'0' }) * (8 - i as u32)).sum::<u32>() % 11 == 0,
        // Business::ISMN uses negative, truthy codes for bad publisher/checksum.
        // Biber consequently accepts every old-style object, but not EAN-13.
        "ismn" => len == 10 && digits[0] == b'M' && digits[1..].iter().all(u8::is_ascii_digit),
        _ => true,
    }
}

