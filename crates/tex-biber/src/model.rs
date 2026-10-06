use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
pub struct Name { pub family: String, pub given: String, pub prefix: String, pub suffix: String, pub extra_parts: BTreeMap<String,String>, pub initials: BTreeMap<String,String>, pub hash: String, pub hashid: String, pub strip: BTreeSet<String>, pub options: BTreeMap<String,String>, pub initial_tokens: BTreeMap<String,Vec<String>>, pub uniquename: Option<NameUnique> }
/// A namedisschema disambiguation level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UniqueLevel { Init, Full, FullOnly }
/// The DataList uniquename state of a name: `un`/`uniquepart` summary, the level used by
/// `_getnamehash_u` (none for a base result), per-namepart values and the base parts.
#[derive(Clone, Debug, Default)]
pub struct NameUnique { pub summary: u8, pub part: String, pub level: Option<UniqueLevel>, pub parts: BTreeMap<String,u8>, pub base_parts: Vec<String> }
/// A fresh Biber::Entry::Names object identity.
pub fn namelist_id()->usize {static NEXT:std::sync::atomic::AtomicUsize=std::sync::atomic::AtomicUsize::new(1);NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed)}
impl Name {
    pub fn part(&self,key:&str)->&str {
        match key {"family"=>&self.family,"given"=>&self.given,"prefix"=>&self.prefix,"suffix"=>&self.suffix,_=>self.extra_parts.get(key).map(String::as_str).unwrap_or("")}
    }
    pub fn set_part(&mut self,key:&str,value:String) {
        match key {"family"=>self.family=value,"given"=>self.given=value,"prefix"=>self.prefix=value,"suffix"=>self.suffix=value,_=>{self.extra_parts.insert(key.to_owned(),value);}}
    }
}
/// `id` is the Biber::Entry::Names object identity: inheritance and entry clones share
/// the object, and with it the per-namelist uniquename/uniquelist DataList state.
#[derive(Clone, Debug, Default)]
pub struct NameList { pub id: usize, pub visible: Option<[usize;4]>, pub names: Vec<Name>, pub others: bool, pub hash: String, pub fullhash: String, pub bibnamehash: String, pub uniquelist: Option<usize>, pub options: BTreeMap<String,String> }
#[derive(Clone, Debug, Default)]
pub struct Entry { pub key: String, pub kind: String, pub source: String, pub fields: BTreeMap<String,String>, pub names: BTreeMap<String,NameList>, pub lists: BTreeMap<String,Vec<String>>, pub ranges: BTreeMap<String,Vec<(String,String)>>, pub annotations: Vec<Annotation>, pub computed: BTreeMap<String,String>, pub flags: BTreeSet<String>, pub warnings: Vec<String>, pub input_warnings: Vec<String>, pub cite_count: i32 }
#[derive(Clone, Debug, Default)]
pub struct Annotation { pub field: String, pub name: String, pub item: String, pub part: String, pub literal: bool, pub value: String }
/// One `<bcf:sortitem>` (a field or `literal` string) with its own attributes.
#[derive(Clone, Debug, Default)]
pub struct SortElement { pub name: String, pub literal: bool, pub substring_side: Option<String>, pub substring_width: Option<String>, pub pad_side: Option<String>, pub pad_width: Option<String>, pub pad_char: Option<String> }
#[derive(Clone, Debug, Default)]
pub struct SortItem { pub elements: Vec<SortElement>, pub descending: bool, pub final_: bool, pub locale: Option<String>, pub sortcase: Option<bool>, pub sortupper: Option<bool> }
#[derive(Clone, Debug, Default)]
pub struct DataList { pub name: String, pub sorting: String, pub kind: String, pub labelprefix: String, pub namekey: String, pub unique_template: String, pub alpha_template: String, pub hash_template: String }
/// `allkeys_order` is Biber's section citekey list after allkeys datasource
/// loading: per datasource, map-created keys followed by surviving file keys.
#[derive(Clone, Debug, Default)]
pub struct Section { pub number: usize, pub sources: Vec<String>, pub datasource_formats: BTreeMap<String,String>, pub datasource_encodings: BTreeMap<String,String>, pub citekeys: Vec<String>, pub cite_counts: BTreeMap<String,i32>, pub cite_orders: BTreeMap<String,(usize,usize)>, pub nocite: BTreeSet<String>, pub lists: Vec<DataList>, pub created_keys: Vec<(String,String)>, pub allkeys_order: Vec<String> }
#[derive(Clone, Debug, Default)]
pub struct FieldSpec { pub datatype: String, pub fieldtype: String, pub format: String, pub skip_output: bool, pub nullok: bool }
#[derive(Clone, Debug, Default)]
pub struct LabelPart { pub fields: Vec<LabelField> }
#[derive(Clone, Debug, Default)]
pub struct LabelField { pub name: String, pub literal: bool, pub final_: bool, pub attributes: BTreeMap<String,String> }
#[derive(Clone, Debug, Default)]
pub struct LabelTemplates { pub by_type: BTreeMap<String,Vec<LabelPart>> }
#[derive(Clone, Debug, Default)]
pub struct SourceMap { pub steps: Vec<BTreeMap<String,String>>, pub patterns: BTreeMap<String,crate::perl_regex::Regex>, pub overwrite: bool, pub per_type: Vec<String>, pub per_not_type: Vec<String>, pub per_source: Vec<String>, pub foreach: Option<String>, pub refsection: Option<usize>, pub level: String, pub datatype: String, pub fieldsets: BTreeMap<String,Vec<String>> }
#[derive(Clone, Debug, Default)]
pub struct Inheritance { pub types: Vec<(String,String)>, pub fields: Vec<(String,Option<String>)>, pub overrides: BTreeSet<(String,String)> }
#[derive(Clone, Debug, Default)]
pub struct Control { pub options: BTreeMap<String,String>, pub text_rules: BTreeMap<String,Vec<String>>, pub type_options: BTreeMap<String,BTreeMap<String,String>>, pub fields: BTreeMap<String,FieldSpec>, pub field_order: Vec<String>, pub sections: Vec<Section>, pub sorting: BTreeMap<String,Vec<SortItem>>, pub sorting_locales: BTreeMap<String,String>, pub labelalpha: Vec<LabelPart>, pub name_templates: BTreeMap<String,BTreeMap<String,Vec<Vec<NameTemplatePart>>>>, pub sourcemaps: Vec<SourceMap>, pub inheritance: Vec<Inheritance>, pub warnings: Vec<String>, pub nameparts: Vec<String>, pub labelname: Vec<String>, pub labeltitle: Vec<String>, pub labeldate: Vec<String>, pub labeldate_specs: BTreeMap<String,Vec<LabelDateSpec>>, pub extradate_context: Vec<String>, pub extradate_spec: Vec<Vec<String>>, pub skip_types: BTreeSet<String>, pub datamodel: crate::validation::Datamodel, pub label_templates: LabelTemplates }
#[derive(Clone, Debug, Default)]
pub struct LabelDateSpec { pub name: String, pub literal: bool }
#[derive(Clone, Debug, Default)]
pub struct NameTemplatePart { pub name: String, pub attributes: BTreeMap<String,String> }
impl Control { pub fn option<'a>(&'a self, kind: &str, name: &str) -> &'a str { self.type_options.get(kind).and_then(|o|o.get(name)).or_else(||self.options.get(name)).map(String::as_str).unwrap_or("") } }
impl Control {
    pub fn labelalpha_template(&self, kind: &str) -> &[LabelPart] { self.label_templates.by_type.get(kind).map(Vec::as_slice).unwrap_or(&self.labelalpha) }
    /// getblxoption(undef, '<spec>', $entrytype): a per-type label*spec replaces the global one.
    pub fn label_spec<'a>(&'a self, kind: &str, key: &str, global: &'a [String]) -> Vec<&'a str> {
        match self.type_options.get(kind).and_then(|o|o.get(key)) {Some(spec)=>spec.split(',').collect(),None=>global.iter().map(String::as_str).collect()}
    }
}
