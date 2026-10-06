//! Port of Biber's uniquename/uniquelist machinery: process_namedis,
//! uniqueness (create/generate_uniquename, create/generate_uniquelist),
//! DataList::set_uniquelist and namelist_differs_*, and the per-namepart
//! uniquename values of generate_contextdata.
//!
//! DataList state is keyed by Names object (`NameList::id`) and name position,
//! so an inherited or cloned list shares it and the last citekey to set it wins.
use crate::model::{Control, Entry, NameList, NameUnique, UniqueLevel};
use crate::process::{entry_option, option};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const SEP: char = '\u{10FFFD}';

/// A namedisschema element: `['base', parts]` or `[namepart, level]`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Schema {
    Base(Option<Vec<String>>),
    Part(String, UniqueLevel),
}

/// One name's process_namedis result.
struct NameDis {
    nameun: String,
    namelistul: String,
    namestring: String,
    namestrings: Vec<String>,
    schema: Vec<Schema>,
}

#[derive(Default)]
struct NameState {
    namestring: String,
    namestrings: Vec<String>,
    schema: Vec<Schema>,
    base: Option<String>,
    base_parts: Option<Vec<String>>,
    un: Option<Schema>,
    unall: Option<Schema>,
    unmininfo: Option<String>,
}

pub(crate) fn uniquename_mode(value: &str) -> &str {
    match value {
        "0" | "false" | "" => "false",
        "1" => "init",
        "2" | "true" => "full",
        "3" => "allinit",
        "4" => "allfull",
        "5" => "mininit",
        "6" => "minfull",
        "7" => "minyearinit",
        "8" => "minyearfull",
        _ => value,
    }
}
pub(crate) fn uniquelist_mode(value: &str) -> &str {
    match value {
        "0" | "false" | "" => "false",
        "1" | "true" => "true",
        _ => value,
    }
}
fn truthy(value: &str) -> bool { !value.is_empty() && value != "0" }
fn enabled(value: &str) -> bool { !matches!(value, "" | "0" | "false") }
fn is_min(un: &str) -> bool { matches!(un, "mininit" | "minfull" | "minyearinit" | "minyearfull") }
fn join(list: &[String]) -> String {
    let mut out = String::new();
    for (i, item) in list.iter().enumerate() {
        if i > 0 {
            out.push(SEP);
        }
        out.push_str(item);
    }
    out
}
/// Perl `split("\x{10FFFD}", $s)`: trailing empty fields are dropped.
fn split(value: &str) -> Vec<&str> {
    let mut fields: Vec<&str> = value.split(SEP).collect();
    while fields.last().is_some_and(|f| f.is_empty()) {
        fields.pop();
    }
    fields
}

/// Utils::strip_nonamestring for a name list field.
pub(crate) fn strip_nonamestring(c: &Control, value: &str, field: &str) -> String {
    if !truthy(value) {
        return String::new();
    }
    let folded = crate::perl_unicode::full_fold(field);
    let mut value = value.to_owned();
    for key in c.options.keys().filter(|key| key.starts_with("nonamestring.")) {
        let target = &key["nonamestring.".len()..];
        let applies = crate::perl_unicode::full_fold(target) == folded
            || c.options.get(&format!("datafieldset.{}", target.to_lowercase()))
                .is_some_and(|set| set.split(',').any(|f| crate::perl_unicode::full_fold(f) == folded));
        if applies {
            if let std::borrow::Cow::Owned(stripped) = crate::names::strip_rules(c, key, &value) {
                value = stripped;
            }
        }
    }
    value
}

/// Biber::process_namedis for one entry: every name list, with the
/// uniquename/uniquelist/template options carried from list to list.
fn process_namedis(c: &Control, e: &Entry, template: &str) -> Result<Vec<(usize, usize, NameDis)>, String> {
    let mut un = uniquename_mode(option(c, e, "uniquename")).to_owned();
    let mut ul = uniquelist_mode(option(c, e, "uniquelist")).to_owned();
    let mut untname = entry_option(e, "uniquenametemplatename").unwrap_or(template).to_owned();
    let mut out = Vec::new();
    for (field, list) in &e.names {
        if let Some(v) = list.options.get("uniquenametemplatename") {
            untname = v.clone();
        }
        if let Some(v) = list.options.get("uniquelist") {
            ul = uniquelist_mode(v).to_owned();
        }
        if let Some(v) = list.options.get("uniquename") {
            un = uniquename_mode(v).to_owned();
        }
        for (index, name) in list.names.iter().enumerate() {
            if let Some(v) = name.options.get("uniquenametemplatename") {
                untname = v.clone();
            }
            let Some(parts) = c.name_templates.get("uniquenametemplate").and_then(|t| t.get(&untname)) else {
                return Err(format!("No uniquenametemplate called '{untname}' found, cannot continue."));
            };
            if let Some(v) = name.options.get("uniquename") {
                un = uniquename_mode(v).to_owned();
            }
            let attr = |p: &crate::model::NameTemplatePart, key: &str| p.attributes.get(key).is_some_and(|v| matches!(v.as_str(), "1" | "true"));
            let useok = |npn: &str| {
                let key = format!("use{npn}");
                let value = name.options.get(&key).or_else(|| list.options.get(&key)).map(String::as_str).unwrap_or_else(|| option(c, e, &key));
                enabled(value)
            };
            let mut base = String::new();
            let mut baseparts: Option<Vec<String>> = None;
            for np in parts.iter().flatten().filter(|np| attr(np, "base")) {
                let p = name.part(&np.name);
                if !truthy(p) {
                    continue;
                }
                if attr(np, "use") && !useok(&np.name) {
                    continue;
                }
                base.push_str(p);
                baseparts.get_or_insert_with(Vec::new).push(np.name.clone());
            }
            let mut namestring = base.clone();
            let mut namestrings = vec![base];
            let mut schema = Vec::new();
            if baseparts.is_some() {
                schema.push(Schema::Base(baseparts));
            }
            for np in parts.iter().flatten() {
                if attr(np, "base") {
                    continue;
                }
                let disambiguation = np.attributes.get("disambiguation").map(String::as_str);
                if disambiguation == Some("none") {
                    continue;
                }
                let level = disambiguation.or(match un.as_str() {
                    "false" => Some("none"),
                    "init" | "allinit" | "mininit" | "minyearinit" => Some("init"),
                    "full" | "allfull" | "minfull" | "minyearfull" => Some("initorfull"),
                    _ => None,
                }).map(str::to_lowercase).unwrap_or_default();
                let lastns = namestrings.last().cloned().unwrap_or_default();
                let p = name.part(&np.name);
                if !truthy(p) {
                    continue;
                }
                if attr(np, "use") && !useok(&np.name) {
                    continue;
                }
                let initials = name.initial_tokens.get(&np.name).map(|t| t.concat()).unwrap_or_default();
                namestring.push_str(p);
                if level == "full" {
                    namestrings.push(format!("{lastns}{p}"));
                    schema.push(Schema::Part(np.name.clone(), UniqueLevel::FullOnly));
                }
                if level == "initorfull" {
                    namestrings.push(format!("{lastns}{initials}"));
                    schema.push(Schema::Part(np.name.clone(), UniqueLevel::Init));
                    namestrings.push(format!("{lastns}{p}"));
                    schema.push(Schema::Part(np.name.clone(), UniqueLevel::Full));
                } else if level == "init" {
                    namestrings.push(format!("{lastns}{initials}"));
                    schema.push(Schema::Part(np.name.clone(), UniqueLevel::Init));
                }
            }
            out.push((list.id, index, NameDis {
                nameun: un.clone(),
                namelistul: ul.clone(),
                namestring: strip_nonamestring(c, &namestring, field),
                namestrings: namestrings.iter().map(|s| strip_nonamestring(c, s, field)).collect(),
                schema,
            }));
        }
    }
    Ok(out)
}

/// process_uniqueprimaryauthor: the base namestring of the labelname's first name.
pub(crate) fn primary_author_base(c: &Control, e: &Entry, template: &str) -> Result<Option<String>, String> {
    let Some(list) = e.computed.get("labelnamesource").and_then(|f| e.names.get(f)) else { return Ok(None) };
    let dis = process_namedis(c, e, template)?;
    let Some((_, _, first)) = dis.iter().find(|(id, index, _)| *id == list.id && *index == 0) else { return Ok(None) };
    Ok(first.schema.iter().zip(&first.namestrings).filter(|(s, _)| matches!(s, Schema::Base(_))).map(|(_, ns)| ns.clone()).last())
}

/// One citekey with a labelname, and the options Biber reads for it.
struct Cite<'a> {
    list: &'a NameList,
    /// uniquename with a defined per-namelist override.
    un: String,
    /// uniquelist with a true per-namelist override (create_uniquename_info).
    ul_truthy: String,
    /// uniquelist with a defined per-namelist override.
    ul: String,
    maxcn: usize,
    mincn: usize,
    labelyear: Option<&'a str>,
}

#[derive(Default)]
struct State {
    names: HashMap<(usize, usize), NameState>,
    ul: HashMap<usize, usize>,
    changed: bool,
    uncount: HashMap<(String, String), BTreeSet<String>>,
    uncount_all: HashMap<(String, String), BTreeSet<String>>,
    ulcount: HashMap<String, usize>,
    ulfinal: HashMap<String, usize>,
    ulfinal_year: HashMap<String, HashMap<String, usize>>,
    ulminyear: HashMap<(String, String), BTreeSet<String>>,
}

impl State {
    fn name(&self, nlid: usize, nid: usize) -> Option<&NameState> { self.names.get(&(nlid, nid)) }
    fn name_mut(&mut self, nlid: usize, nid: usize) -> &mut NameState { self.names.entry((nlid, nid)).or_default() }
    fn set_uniquename(&mut self, nlid: usize, nid: usize, value: Option<Schema>) {
        let state = self.name_mut(nlid, nid);
        let changed = state.un.is_none() || state.un != value;
        state.un = value;
        self.changed |= changed;
    }
    fn namedis(&self, nlid: usize, nid: usize) -> (Vec<String>, Vec<Schema>) {
        self.name(nlid, nid).map(|s| (s.namestrings.clone(), s.schema.clone())).unwrap_or_default()
    }
    /// The labelname list prefix used for uniquelist tracking.
    fn push_ul_names(&self, nlid: usize, nid: usize, unall: Option<&Schema>, namelist: &mut Vec<String>, minyear: Option<&mut Vec<String>>) {
        let state = self.name(nlid, nid);
        match unall {
            None | Some(Schema::Base(_)) => {
                let base = state.and_then(|s| s.base.clone());
                if let Some(base) = &base {
                    namelist.push(base.clone());
                }
                if let Some(minyear) = minyear {
                    minyear.push(base.unwrap_or_default());
                }
            }
            Some(unall) => {
                let mut minyear = minyear;
                let Some(state) = state else { return };
                for (i, schema) in state.schema.iter().enumerate() {
                    if schema == unall {
                        if let Some(ns) = state.namestrings.get(i) {
                            namelist.push(ns.clone());
                        }
                        if let Some(minyear) = minyear.as_deref_mut() {
                            minyear.push(state.namestrings.get(i).cloned().unwrap_or_default());
                        }
                    }
                }
            }
        }
    }

    fn create_uniquename_info(&mut self, cites: &[Cite]) {
        self.uncount.clear();
        self.uncount_all.clear();
        'main: for x in cites {
            let nlid = x.list.id;
            let mut un = x.un.clone();
            let ul = self.ul.get(&nlid).copied().filter(|&ul| ul > 0);
            let count = x.list.names.len();
            let mut trunc = vec![false; count];
            let mut basenames = Vec::new();
            let mut allnames = Vec::new();
            for (i, name) in x.list.names.iter().enumerate() {
                if let Some(v) = name.options.get("uniquename") {
                    un = uniquename_mode(v).to_owned();
                }
                if un == "false" {
                    continue 'main;
                }
                let index = i + 1;
                if matches!(un.as_str(), "allinit" | "allfull" | "minyearinit" | "minyearfull")
                    || ul.is_some_and(|ul| index <= ul) || x.list.others || count <= x.maxcn || index <= x.mincn {
                    trunc[i] = true;
                    if is_min(&un) {
                        let state = self.name(nlid, i);
                        basenames.push(state.and_then(|s| s.base.clone()).unwrap_or_default());
                        allnames.push(state.map(|s| s.namestring.clone()).unwrap_or_default());
                    }
                }
            }
            let (mut min_basename, mut min_namestring) = (String::new(), String::new());
            if is_min(&un) {
                min_basename = join(&basenames);
                min_namestring = join(&allnames);
                if basenames.len() < count || x.list.others {
                    min_basename.push_str("\u{10FFFD}et al");
                    min_namestring.push_str("\u{10FFFD}et al");
                }
            }
            let lyear = x.labelyear.unwrap_or("");
            for i in 0..count {
                let namestrings = self.namedis(nlid, i).0;
                let (scope, key) = match un.as_str() {
                    "init" | "full" | "allinit" | "allfull" => ("global".to_owned(), join(&namestrings)),
                    "mininit" | "minfull" => {
                        self.name_mut(nlid, i).unmininfo = Some(min_basename.clone());
                        (min_basename.clone(), min_namestring.clone())
                    }
                    "minyearinit" | "minyearfull" => {
                        self.name_mut(nlid, i).unmininfo = Some(format!("{min_basename}{lyear}"));
                        (format!("{min_basename}{lyear}"), format!("{min_namestring}{lyear}"))
                    }
                    _ => (String::new(), String::new()),
                };
                if trunc[i] {
                    for ns in &namestrings {
                        self.uncount.entry((ns.clone(), scope.clone())).or_default().insert(key.clone());
                    }
                }
                if x.ul_truthy != "false" {
                    for ns in &namestrings {
                        self.uncount_all.entry((ns.clone(), scope.clone())).or_default().insert(key.clone());
                    }
                }
            }
        }
    }

    fn generate_uniquename(&mut self, cites: &[Cite]) {
        'main: for x in cites {
            let nlid = x.list.id;
            let mut un = x.un.clone();
            let ul = self.ul.get(&nlid).copied().filter(|&ul| ul > 0);
            let count = x.list.names.len();
            let mut trunc = vec![false; count];
            for (i, name) in x.list.names.iter().enumerate() {
                if let Some(v) = name.options.get("uniquename") {
                    un = uniquename_mode(v).to_owned();
                }
                if un == "false" {
                    continue 'main;
                }
                let index = i + 1;
                if matches!(un.as_str(), "allinit" | "allfull")
                    || ul.is_some_and(|ul| index <= ul) || x.list.others || count <= x.maxcn || index <= x.mincn {
                    trunc[i] = true;
                } else {
                    // reset_uniquename: anything no longer visible goes back to its base.
                    let state = self.name_mut(nlid, i);
                    state.un = Some(Schema::Base(state.base_parts.clone()));
                }
            }
            for i in 0..count {
                let (namestrings, schema) = self.namedis(nlid, i);
                let scope = if is_min(&un) { self.name(nlid, i).and_then(|s| s.unmininfo.clone()).unwrap_or_default() } else { "global".to_owned() };
                let unique = |counts: &HashMap<(String, String), BTreeSet<String>>, ns: &String| {
                    counts.get(&(ns.clone(), scope.clone())).is_some_and(|keys| keys.len() == 1)
                };
                if trunc[i] {
                    if let Some(j) = namestrings.iter().position(|ns| unique(&self.uncount, ns)) {
                        self.set_uniquename(nlid, i, schema.get(j).cloned());
                    }
                    if self.name(nlid, i).is_none_or(|s| s.un.is_none()) {
                        self.set_uniquename(nlid, i, schema.first().cloned());
                    }
                }
                if x.ul != "false" {
                    if let Some(j) = namestrings.iter().position(|ns| unique(&self.uncount_all, ns)) {
                        self.name_mut(nlid, i).unall = schema.get(j).cloned();
                    }
                    let state = self.name_mut(nlid, i);
                    if state.unall.is_none() {
                        state.unall = schema.first().cloned();
                    }
                }
            }
        }
    }

    fn create_uniquelist_info(&mut self, cites: &[Cite]) {
        self.ulcount.clear();
        self.ulfinal.clear();
        self.ulfinal_year.clear();
        self.ulminyear.clear();
        for x in cites {
            if x.ul == "false" {
                continue;
            }
            let nlid = x.list.id;
            let count = x.list.names.len();
            let mut namelist = Vec::new();
            let mut minyear = Vec::new();
            for i in 0..count {
                let flag = x.ul == "minyear" && count > x.maxcn && i < x.mincn;
                let unall = self.name(nlid, i).and_then(|s| s.unall.clone());
                self.push_ul_names(nlid, i, unall.as_ref(), &mut namelist, flag.then_some(&mut minyear));
                *self.ulcount.entry(join(&namelist)).or_default() += 1;
            }
            let joined = join(&namelist);
            *self.ulfinal.entry(joined.clone()).or_default() += 1;
            if x.ul == "minyear" {
                // add_uniquelistcount_final($namelist, $labelyear) counts the final list again.
                *self.ulfinal.entry(joined.clone()).or_default() += 1;
                if let Some(year) = x.labelyear.filter(|y| truthy(y)) {
                    *self.ulfinal_year.entry(year.to_owned()).or_default().entry(joined.clone()).or_default() += 1;
                }
            }
            if !minyear.is_empty() {
                self.ulminyear.entry((join(&minyear), x.labelyear.unwrap_or("0").to_owned())).or_default().insert(joined);
            }
        }
    }

    fn generate_uniquelist(&mut self, cites: &[Cite]) {
        'main: for x in cites {
            if x.ul == "false" {
                continue;
            }
            let nlid = x.list.id;
            let count = x.list.names.len();
            let mut namelist = Vec::new();
            for i in 0..count {
                let unall = self.name(nlid, i).and_then(|s| s.unall.clone());
                self.push_ul_names(nlid, i, unall.as_ref(), &mut namelist, None);
                if x.ul == "minyear" && count > x.maxcn && i < x.mincn
                    && self.ulminyear.get(&(join(&namelist), x.labelyear.unwrap_or("0").to_owned())).map_or(0, BTreeSet::len) == 1 {
                    continue 'main;
                }
                if self.list_count(&namelist) == 1 {
                    break;
                }
            }
            self.set_uniquelist(x, &namelist);
        }
    }

    fn list_count(&self, list: &[String]) -> usize { self.ulcount.get(&join(list)).copied().unwrap_or(0) }
    /// Keys of `{uniquelistcount}{global}{final}`, including uniquelist=minyear year buckets.
    fn final_keys(&self) -> impl Iterator<Item = &String> { self.ulfinal.keys().chain(self.ulfinal_year.keys()) }

    fn set_uniquelist(&mut self, x: &Cite, namelist: &[String]) {
        let nlid = x.list.id;
        let mut uniquelist = namelist.len();
        let count = x.list.names.len();
        if self.ul.get(&nlid) != Some(&uniquelist) {
            self.changed = true;
        }
        if uniquelist <= 1 || count <= x.maxcn || (uniquelist <= x.mincn && x.mincn != count) {
            return;
        }
        if self.ulfinal.get(&join(namelist)).copied().unwrap_or(0) > 1 {
            match self.namelist_differs_index(namelist) {
                None | Some(0) => return,
                Some(index) => uniquelist = index + 1,
            }
        } else if count > uniquelist && !self.namelist_differs_nth(namelist, uniquelist, &x.ul, x.labelyear) {
            uniquelist -= 1;
        }
        self.ul.insert(nlid, uniquelist);
    }

    fn namelist_differs_index(&self, list: &[String]) -> Option<usize> {
        let mut index = None;
        for key in self.final_keys() {
            let other = split(key);
            if other.len() == list.len() && other.iter().zip(list).all(|(a, b)| *a == b) {
                continue;
            }
            for (i, item) in list.iter().enumerate() {
                if other.get(i).is_some_and(|o| o == item) {
                    index = Some(index.map_or(i, |index: usize| index.max(i)));
                } else {
                    break;
                }
            }
        }
        index.map(|index| if index == list.len() - 1 { index } else { index + 1 })
    }

    fn namelist_differs_nth(&self, list: &[String], n: usize, ul: &str, labelyear: Option<&str>) -> bool {
        let keys: Vec<&String> = if ul == "minyear" {
            labelyear.and_then(|year| self.ulfinal_year.get(year)).map(|lists| lists.keys().collect()).unwrap_or_default()
        } else {
            self.final_keys().collect()
        };
        keys.into_iter().any(|key| {
            let other = split(key);
            other.len() >= list.len() && list[n - 1] != other[n - 1] && other[..n - 1].iter().zip(&list[..n - 1]).all(|(a, b)| *a == b)
        })
    }
}

/// Biber's per-datalist uniqueness: process_entries_pre's namedis, then the
/// uniquename/uniquelist fixpoint, stored on every copy of each name list.
pub(crate) fn uniqueness(c: &Control, entries: &mut [Entry], template: &str) -> Result<(), String> {
    let mut state = State::default();
    for e in entries.iter() {
        for (nlid, nid, dis) in process_namedis(c, e, template)? {
            if dis.nameun == "false" && dis.namelistul == "false" {
                continue;
            }
            let base = dis.schema.iter().position(|s| matches!(s, Schema::Base(_)));
            let name = state.name_mut(nlid, nid);
            name.namestring = dis.namestring;
            if let Some(i) = base {
                name.base = dis.namestrings.get(i).cloned();
                if let Schema::Base(parts) = &dis.schema[i] {
                    name.base_parts = parts.clone();
                }
            }
            name.namestrings = dis.namestrings;
            name.schema = dis.schema;
        }
    }
    let cites: Vec<Cite> = entries.iter().filter_map(|e| {
        let list = e.names.get(e.computed.get("labelnamesource")?)?;
        let un = list.options.get("uniquename").map_or_else(|| uniquename_mode(option(c, e, "uniquename")), |v| uniquename_mode(v)).to_owned();
        let entry_ul = uniquelist_mode(option(c, e, "uniquelist"));
        let list_ul = list.options.get("uniquelist").map(|v| uniquelist_mode(v));
        let number = |key: &str, fallback: usize| option(c, e, key).parse().unwrap_or(fallback);
        Some(Cite {
            list,
            un,
            ul_truthy: list.options.get("uniquelist").filter(|v| truthy(v)).map_or(entry_ul, |v| uniquelist_mode(v)).to_owned(),
            ul: list_ul.unwrap_or(entry_ul).to_owned(),
            maxcn: number("maxcitenames", 3),
            mincn: number("mincitenames", 1),
            labelyear: e.computed.get("labelyear").map(String::as_str),
        })
    }).collect();
    // Biber::uniqueness: alternate until neither pass changes anything; uniquelist runs at least once.
    state.changed = true;
    let mut first_ul_pass = true;
    loop {
        if !state.changed {
            break;
        }
        state.changed = false;
        state.create_uniquename_info(&cites);
        state.generate_uniquename(&cites);
        if !first_ul_pass && !state.changed {
            break;
        }
        state.changed = false;
        first_ul_pass = false;
        state.create_uniquelist_info(&cites);
        state.generate_uniquelist(&cites);
    }
    drop(cites);
    for e in entries.iter_mut() {
        for list in e.names.values_mut() {
            list.uniquelist = state.ul.get(&list.id).copied();
            for (nid, name) in list.names.iter_mut().enumerate() {
                name.uniquename = state.name(list.id, nid).and_then(|s| {
                    let un = s.un.as_ref()?;
                    let (summary, part, level) = match un {
                        Schema::Base(_) => (0, "base".to_owned(), None),
                        Schema::Part(part, level) => (if *level == UniqueLevel::Init { 1 } else { 2 }, part.clone(), Some(*level)),
                    };
                    // generate_contextdata: per-namepart values up to where uniqueness is established.
                    let mut levels = BTreeMap::new();
                    if let Some(i) = s.schema.iter().position(|nss| nss == un) {
                        let earlier = s.schema.get(1..i).unwrap_or_default().iter().filter(|nss| !matches!(nss, Schema::Base(_) | Schema::Part(_, UniqueLevel::Full)));
                        for nss in earlier.chain(std::iter::once(&s.schema[i])) {
                            if let Schema::Part(part, level) = nss {
                                levels.insert(part.clone(), if *level == UniqueLevel::Init { 1 } else { 2 });
                            }
                        }
                    }
                    Some(NameUnique { summary, part, level, parts: levels, base_parts: s.base_parts.clone().unwrap_or_default() })
                });
            }
        }
    }
    Ok(())
}
