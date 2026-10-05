//! Offline biblatex bibliography processing, using the Biber control-file protocol.
//!
//! The control protocol targets biblatex BCF 3.11 and BBL 3.3, verified against
//! Biber 2.22. Bibliography data are resolved exclusively through the caller's
//! [`Options::find_file`] callback; no subprocess or network access is used.
//! Files go through the shared TeX resource filesystem, so the same API works
//! for ordinary files and an installed in-memory compilation context.
//! Missing sources and citation keys produce warnings and a usable BBL.
//! Unsupported sourcemap expressions are reported instead of silently ignored.
mod bcf;
mod bib;
mod cli;
mod dates;
mod model;
mod names;
mod output;
mod process;

pub use cli::cli_main;

use std::collections::{BTreeMap,BTreeSet};
use std::path::PathBuf;
use md5::{Digest,Md5};
use tex_kpse::fs;
use model::*;

/// Paths and datasource resolution for one bibliography job.
pub struct Options<'a> {
    /// The biblatex-generated control file.
    pub bcf: PathBuf,
    /// BBL destination; defaults to the control path with its extension replaced.
    pub output: Option<PathBuf>,
    /// Resolve a datasource name to a readable file; `None` means not found.
    pub find_file: &'a dyn Fn(&str) -> Option<PathBuf>,
}
/// Written outputs and diagnostics for a completed bibliography job.
#[derive(Debug)]
pub struct Outcome { pub bbl:PathBuf,pub warnings:u32,pub errors:u32,pub log:String }

/// Return local datasource names from a Biber control file without reading them.
pub fn datasource_names(xml:&str)->Result<Vec<String>,String>{
    let control=bcf::parse(xml)?;let mut names=vec![];
    for s in control.sections {for name in s.sources {if !names.contains(&name){names.push(name)}}}Ok(names)
}

/// Process all bibliography sections and write a BBL plus a sibling BLG log.
pub fn run(opts:&Options<'_>)->Result<Outcome,String>{
    let xml=fs::read_to_string(&opts.bcf).map_err(|e|format!("Cannot read {}: {e}",opts.bcf.display()))?;
    let control=bcf::parse(&xml)?;let mut warnings=control.warnings.clone();let mut rendered=vec![];let mut preambles=vec![];
    for section in &control.sections {
        let mut db=bib::Database::default();
        for source in &section.sources {
            let Some(path)=(opts.find_file)(source)else{warnings.push(format!("Cannot find datasource {source}"));continue};
            match fs::read_to_string(&path){Ok(text)=>{let first=db.entries.len();db.add(&text).map_err(|e|format!("{}: {e}",path.display()))?;for e in &mut db.entries[first..]{e.source=source.clone();bib::sourcemaps(&control,e,source);}},Err(e)=>warnings.push(format!("Cannot read datasource {source}: {e}"))}
        }
        warnings.append(&mut db.warnings);for p in &db.preamble {if !preambles.contains(p){preambles.push(p.clone())}}
        for e in &mut db.entries {bib::normalize(&control,e);}
        resolve_alias_refs(&mut db.entries);
        bib::inherit(&control,&mut db.entries,&mut warnings);
        let mut entries=select(&control,section,&db.entries,&mut warnings);
        process::prepare(&control,&mut entries);
        for e in &entries {warnings.extend(e.warnings.clone());}
        rendered.push((section.clone(),entries));
    }
    let bbl=output::render(&control,&rendered,&preambles.join("%\n"));
    let path=opts.output.clone().unwrap_or_else(||opts.bcf.with_extension("bbl"));
    fs::write(&path,bbl.as_bytes()).map_err(|e|format!("Cannot write {}: {e}",path.display()))?;
    let mut log=format!("INFO - This is TeXres Biber {}\nINFO - Reading '{}'\n",env!("CARGO_PKG_VERSION"),opts.bcf.display());
    for warning in &warnings {log.push_str(&format!("WARN - {warning}\n"));}
    log.push_str(&format!("INFO - Output to {}\nINFO - WARNINGS: {}\n",path.display(),warnings.len()));
    fs::write(path.with_extension("blg"),log.as_bytes()).map_err(|e|format!("Cannot write bibliography log: {e}"))?;
    Ok(Outcome {bbl:path,warnings:warnings.len() as u32,errors:0,log})
}

fn select(control:&Control,section:&Section,all:&[Entry],warnings:&mut Vec<String>)->Vec<Entry>{
    let aliases=alias_map(all);let mut index=all.iter().map(|e|(e.key.clone(),e)).collect::<BTreeMap<_,_>>();let mut keys=vec![];let mut missing=vec![];
    for (alias,key) in &aliases {if let Some(e)=all.iter().find(|e|&e.key==key){index.insert(alias.clone(),e);}}
    for key in &section.citekeys {if key=="*"{for e in all {if !control.skip_types.contains(&e.kind)&&e.kind!="xdata"&&!keys.contains(&e.key){keys.push(e.key.clone())}}}else if let Some(e)=index.get(key){if !keys.contains(&e.key){keys.push(e.key.clone())}}else{warnings.push(format!("I didn't find a database entry for '{key}'"));missing.push(Entry {key:key.clone(),kind:"missing".into(),..Default::default()});}}
    let mut refs=BTreeMap::<(String,String),usize>::new();for key in &keys {if let Some(e)=index.get(key.as_str()){for field in ["crossref","xref"]{if let Some(parent)=e.fields.get(field){*refs.entry((field.into(),parent.clone())).or_default()+=1}}}}
    let mut inherited_sources=BTreeSet::new();
    for ((field,key),count) in refs {let min=control.options.get(if field=="crossref"{"mincrossrefs"}else{"minxrefs"}).and_then(|s|s.parse().ok()).unwrap_or(2);if count>=min&&index.contains_key(key.as_str())&&!keys.contains(&key){inherited_sources.insert((field,key.clone()));keys.push(key);}}
    let mut entries=keys.iter().filter_map(|k|index.get(k.as_str()).copied()).cloned().collect::<Vec<_>>();
    let selected=keys.iter().cloned().collect::<BTreeSet<_>>();
    for e in &mut entries {
        let citation=section.citekeys.iter().find(|key|index.get(*key).is_some_and(|target|target.key==e.key)).map(String::as_str).unwrap_or(&e.key);
        e.cite_count=section.cite_counts.get(citation).copied().unwrap_or(-1);
        if section.nocite.contains(citation) || (section.nocite.contains("*") && !section.citekeys.contains(&e.key)) {e.flags.insert("nocite".into());}
        let (order,intorder)=section.cite_orders.get(citation).copied().or_else(||section.cite_orders.get("*").copied()).unwrap_or((usize::MAX,usize::MAX));
        e.computed.insert("citeorder".into(),order.to_string());e.computed.insert("intciteorder".into(),intorder.to_string());
        for field in ["crossref","xref"] {if e.fields.get(field).is_some_and(|k|!selected.contains(k)){e.fields.remove(field);}}
    }
    for (field,key) in inherited_sources {if let Some(e)=entries.iter_mut().find(|e|e.key==key){e.flags.insert(format!("{field}source"));}}
    let mut sets=vec![];for e in &entries {if e.kind=="set" {if let Some(children)=e.fields.get("entryset"){for child in bib::split_list(children,","){sets.push((e.key.clone(),child));}}}}
    for (set,child) in sets {if !entries.iter().any(|e|e.key==child){if let Some(e)=index.get(child.as_str()){let mut clone=(*e).clone();clone.cite_count=-1;entries.push(clone)}else{warnings.push(format!("Missing set member {child}"));continue}}
        if let Some(e)=entries.iter_mut().find(|e|e.key==child){e.computed.insert("inset".into(),set);e.fields.insert("options".into(),"skipbib=true,skipbiblist=true,skiplab=true".into());}
    }
    let mut clones=vec![];for e in &mut entries {if let Some(related)=e.fields.get("related").cloned(){let mut clonekeys=vec![];for key in bib::split_list(&related,","){if let Some(source)=index.get(key.as_str()){let clonekey=md5_hex(key.as_bytes());clonekeys.push(clonekey.clone());let mut clone=(*source).clone();clone.key=clonekey;clone.cite_count=-1;clone.fields.remove("related");clone.fields.insert("clonesourcekey".into(),key);clone.fields.insert("options".into(),"skipbib=true,skipbiblist=true,skiplab=true".into());clone.computed.insert("relatedclone".into(),"1".into());clones.push(clone);}else{warnings.push(format!("Missing related entry {key}"));}}e.fields.insert("related".into(),clonekeys.join(","));}}
    let referenced_aliases=entries.iter().chain(clones.iter()).filter_map(|e|e.computed.get("referencealiases")).flat_map(|v|bib::split_list(v,",")).collect::<BTreeSet<_>>();
    for (alias,target) in aliases {if section.citekeys.contains(&alias)||section.nocite.contains("*")||referenced_aliases.contains(&alias){let mut e=Entry {key:alias,kind:"alias".into(),..Default::default()};e.computed.insert("target".into(),target);entries.push(e);}}
    for e in clones {if !entries.iter().any(|x|x.key==e.key){entries.push(e)}}entries.extend(missing);entries
}

fn alias_map(entries:&[Entry])->BTreeMap<String,String>{
    let mut aliases=BTreeMap::new();
    for e in entries {if let Some(ids)=e.fields.get("ids"){for alias in bib::split_list(ids,","){if !entries.iter().any(|e|e.key==alias){aliases.entry(alias).or_insert_with(||e.key.clone());}}}}aliases
}
fn resolve_alias_refs(entries:&mut [Entry]){
    let aliases=alias_map(entries);
    for e in entries {
        let mut used=BTreeSet::new();
        for field in ["crossref","xref","xdata","entryset","related"]{if let Some(value)=e.fields.get_mut(field){*value=bib::split_list(value,",").into_iter().map(|key|{if let Some(target)=aliases.get(&key){used.insert(key);target.clone()}else{key}}).collect::<Vec<_>>().join(",");}}
        if !used.is_empty(){e.computed.insert("referencealiases".into(),used.into_iter().collect::<Vec<_>>().join(","));}
    }
}

pub(crate) fn md5_hex(bytes:&[u8])->String{
    const HEX:&[u8;16]=b"0123456789abcdef";
    let mut out=String::with_capacity(32);
    for byte in Md5::digest(bytes){out.push(HEX[(byte>>4)as usize]as char);out.push(HEX[(byte&15)as usize]as char);}
    out
}
