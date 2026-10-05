use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
pub struct Name { pub family: String, pub given: String, pub prefix: String, pub suffix: String, pub initials: BTreeMap<String,String>, pub hash: String, pub hashid: String, pub strip: BTreeSet<String>, pub options: BTreeMap<String,String>, pub initial_tokens: BTreeMap<String,Vec<String>>, pub un: u8, pub uniquepart: String }
#[derive(Clone, Debug, Default)]
pub struct NameList { pub names: Vec<Name>, pub others: bool, pub hash: String, pub fullhash: String, pub bibnamehash: String, pub uniquelist: usize, pub options: BTreeMap<String,String> }
#[derive(Clone, Debug, Default)]
pub struct Entry { pub key: String, pub kind: String, pub source: String, pub fields: BTreeMap<String,String>, pub names: BTreeMap<String,NameList>, pub lists: BTreeMap<String,Vec<String>>, pub computed: BTreeMap<String,String>, pub flags: BTreeSet<String>, pub warnings: Vec<String>, pub cite_count: i32 }
#[derive(Clone, Debug, Default)]
pub struct SortItem { pub fields: Vec<String>, pub descending: bool, pub final_: bool, pub literal: Option<String>, pub substring_side: Option<String>, pub substring_width: Option<usize>, pub pad_side: Option<String>, pub pad_width: Option<usize>, pub pad_char: Option<String> }
#[derive(Clone, Debug, Default)]
pub struct DataList { pub name: String, pub sorting: String, pub kind: String, pub labelprefix: String, pub namekey: String }
#[derive(Clone, Debug, Default)]
pub struct Section { pub number: usize, pub sources: Vec<String>, pub citekeys: Vec<String>, pub cite_counts: BTreeMap<String,i32>, pub cite_orders: BTreeMap<String,(usize,usize)>, pub nocite: BTreeSet<String>, pub lists: Vec<DataList> }
#[derive(Clone, Debug, Default)]
pub struct FieldSpec { pub datatype: String, pub fieldtype: String, pub format: String, pub label: bool, pub skip_output: bool, pub nullok: bool }
#[derive(Clone, Debug, Default)]
pub struct LabelPart { pub fields: Vec<LabelField> }
#[derive(Clone, Debug, Default)]
pub struct LabelField { pub name: String, pub literal: bool, pub strwidth: Option<usize>, pub strside: String, pub ifnames: Option<usize>, pub names: Option<usize>, pub final_: bool }
#[derive(Clone, Debug, Default)]
pub struct SourceMap { pub steps: Vec<BTreeMap<String,String>>, pub patterns: BTreeMap<String,regex::Regex>, pub overwrite: bool, pub per_type: Vec<String>, pub per_source: Vec<String> }
#[derive(Clone, Debug, Default)]
pub struct Inheritance { pub types: Vec<(String,String)>, pub fields: Vec<(String,Option<String>)>, pub override_target: bool }
#[derive(Clone, Debug, Default)]
pub struct Control { pub options: BTreeMap<String,String>, pub type_options: BTreeMap<String,BTreeMap<String,String>>, pub fields: BTreeMap<String,FieldSpec>, pub field_order: Vec<String>, pub sections: Vec<Section>, pub sorting: BTreeMap<String,Vec<SortItem>>, pub labelalpha: Vec<LabelPart>, pub name_templates: BTreeMap<String,BTreeMap<String,Vec<Vec<NameTemplatePart>>>>, pub sourcemaps: Vec<SourceMap>, pub inheritance: Vec<Inheritance>, pub warnings: Vec<String>, pub nameparts: Vec<String>, pub labelname: Vec<String>, pub labeltitle: Vec<String>, pub labeldate: Vec<String>, pub labeldate_specs: Vec<LabelDateSpec>, pub extradate_context: Vec<String>, pub extradate_spec: Vec<Vec<String>>, pub skip_types: BTreeSet<String> }
#[derive(Clone, Debug, Default)]
pub struct LabelDateSpec { pub name: String, pub literal: bool }
#[derive(Clone, Debug, Default)]
pub struct NameTemplatePart { pub name: String, pub attributes: BTreeMap<String,String> }
impl Control { pub fn option<'a>(&'a self, kind: &str, name: &str) -> &'a str { self.type_options.get(kind).and_then(|o|o.get(name)).or_else(||self.options.get(name)).map(String::as_str).unwrap_or("") } }
