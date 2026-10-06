//! Self-contained biblatex bibliography processing, using the Biber control-file protocol.
//!
//! The control protocol targets biblatex BCF 3.11 and BBL 3.3, verified against
//! Biber 2.22. The caller's [`Options::find_file`] callback takes precedence;
//! unresolved HTTP(S) and FTP datasources use the built-in transport, never a subprocess.
//! Files go through the shared TeX resource filesystem, so the same API works
//! for ordinary files and an installed in-memory compilation context.
//! Missing sources and citation keys produce warnings and a usable BBL.
//! Datasource transformations are applied before inheritance and label generation.
mod bcf;
mod bib;
mod blob;
mod cli;
mod collation;
mod config;
mod biblatexml;
mod dates;
mod encoding;
mod gcstring;
mod model;
mod labels;
mod names;
mod output;
mod perl_regex;
mod perl_pattern;
mod perl_unicode;
mod perl_vm;
mod process;
mod remote;
mod tool;
mod transliteration;
mod uniqueness;
mod validation;

pub use cli::cli_main;

use std::collections::{BTreeMap,BTreeSet};
use std::path::PathBuf;
use md5::{Digest,Md5};
use tex_kpse::fs;
use model::*;

/// Paths and datasource resolution for one bibliography job.
pub struct Options<'a> {
    /// The biblatex control file, or a datasource for [`run_tool_configured`].
    pub bcf: PathBuf,
    /// Output destination; defaults to BBL or the tool-mode conversion filename.
    pub output: Option<PathBuf>,
    /// Directory for default outputs and logs, independent of an explicit output filename.
    pub output_directory: Option<PathBuf>,
    /// Resolve a datasource to a local/resource file; unresolved URLs use built-in transport.
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
    run_configured(opts, &BTreeMap::new(), None)
}

/// Convert a local BibTeX or BibLaTeXML datasource using Biber tool mode.
///
/// `opts.bcf` names the datasource. Configuration and override keys follow
/// [`run_configured`]; no bibliography labels or control file are generated.
pub fn run_tool_configured(opts:&Options<'_>,overrides:&BTreeMap<String,String>,configuration:Option<&str>)->Result<Outcome,String>{
    bib::reset_sourcemap_unique();
    tool::run(opts,overrides,configuration)
}

/// Process a bibliography with explicit Biber configuration and CLI overrides.
///
/// Configuration is XML text; option keys use Biber's underscore spelling.
/// CLI overrides have precedence over configuration and BCF option values.
pub fn run_configured(opts:&Options<'_>, overrides:&BTreeMap<String,String>, configuration:Option<&str>)->Result<Outcome,String>{
    bib::reset_sourcemap_unique();
    let bytes=fs::read(&opts.bcf).map_err(|e|format!("Cannot read {}: {e}",opts.bcf.display()))?;
    let xml=encoding::decode_bcf(&bytes)?;
    let mut control=bcf::parse(&xml)?;
    if let Some(configuration)=configuration {config::apply(&mut control,configuration)?;}
    control.options.extend(overrides.clone());
    if let Some(locale)=overrides.get("sortlocale"){control.options.insert("sortlocale_override".into(),locale.clone());}
    collation::validate_options(&control)?;
    names::validate_control_rules(&control)?;
    let mut warnings=control.warnings.clone();let mut errors=vec![];let mut rendered=vec![];let mut preambles=vec![];
    let mut remote_sources=remote::Cache::default();
    for original_section in &control.sections {
        let mut section=original_section.clone();
        let mut db=bib::Database::default();
        db.standard_macros(control.options.get("nostdmacros").is_some_and(|value|matches!(value.as_str(),"1"|"true")));
        for source in &original_section.sources {
            let local=(opts.find_file)(source);
            let is_local=local.is_some();
            let path=local.unwrap_or_else(||PathBuf::from(source));
            let local_data;
            let remote_data;
            let data=if is_local{
                local_data=fs::read(&path);
                local_data.as_deref().map_err(|error|error.to_string())
            }else{
                remote_data=remote_sources.fetch(source,&control.options)?;
                match remote_data.as_deref(){
                    Some(bytes)=>Ok(bytes),
                    None=>{warnings.push(format!("Cannot find datasource {source}"));continue}
                }
            };
            match data{
                Ok(bytes)=>{
                    let datatype=section.datasource_formats.get(source).map(String::as_str).unwrap_or("bibtex");
                    let charset=if datatype=="biblatexml"{"UTF-8"}else{
                        section.datasource_encodings.get(source).or_else(||control.options.get("input_encoding")).or_else(||control.options.get("inputencoding")).map(String::as_str).unwrap_or("UTF-8")
                    };
                    let text=encoding::decode(&bytes,charset).map_err(|e|format!("Data file '{source}' cannot be read in encoding '{charset}': {e}"))?;
                    let first=db.entries.len();
                    match section.datasource_formats.get(source).map(String::as_str).unwrap_or("bibtex") {
                        "bibtex"=>{
                            if text.is_empty(){warnings.push(format!("Data source '{source}' is empty, ignoring"));continue}
                            if !text.contains('@'){warnings.push(format!("Data source '{source}' contains no BibTeX entries/macros, ignoring"));continue}
                            db.add(&text,source).map_err(|e|format!("{}: {e}",path.display()))?;
                            bib::decode_entries(&control,&mut db.entries[first..]);
                        },
                        "biblatexml"=>biblatexml::add(&control,&mut section,&mut db,&text,source,!is_local).map_err(|e|format!("{}: {e}",path.display()))?,
                        datatype=>return Err(format!("Unsupported datasource datatype '{datatype}' for '{source}'")),
                    }
                    for e in &mut db.entries[first..]{
                        e.source=source.clone();
                        if !is_local{e.computed.insert("sourcemap_remote".into(),"1".into());}
                    }
                },
                Err(e)=>warnings.push(format!("Cannot read datasource {source}: {e}"))
            }
        }
        bib::sourcemaps(&control,&mut section,&mut db.entries,&mut warnings)?;
        warnings.append(&mut db.warnings);for p in &db.preamble {if !preambles.contains(p){preambles.push(p.clone())}}
        for e in &mut db.entries {
            e.computed.insert("refsection".into(),section.number.to_string());
            validation::validate_input(&control,e);
            if !e.computed.get("sourcemap_datatype").is_some_and(|datatype|datatype.starts_with("biblatexml")){bib::normalize(&control,e);}
            warnings.append(&mut e.input_warnings);
        }
        resolve_alias_refs(&mut db.entries);
        bib::inherit(&control,&mut db.entries,&mut warnings);
        let mut entries=select(&control,&section,&db.entries,&mut warnings);
        if control.options.get("dieondatamodel").is_some_and(|value|matches!(value.as_str(),"1"|"true")) {
            let previous=entries.iter().map(|entry|entry.warnings.len()).collect::<Vec<_>>();
            validation::validate(&control,&mut entries)?;
            for (entry,previous) in entries.iter_mut().zip(previous){errors.extend(entry.warnings.drain(previous..));}
        }else{validation::validate(&control,&mut entries)?;}
        process::prepare(&control,&mut entries)?;
        for e in &entries {warnings.extend(e.warnings.clone());}
        rendered.push((section,entries));
    }
    let bbl=output::render(&control,&rendered,&preambles.join("%\n"))?;
    let path=opts.output.clone().unwrap_or_else(||opts.output_directory.as_ref().map(|directory|directory.join(opts.bcf.file_name().unwrap_or_default()).with_extension("bbl")).unwrap_or_else(||opts.bcf.with_extension("bbl")));
    let logfile=tool::logfile_path(&control,opts,false);
    let encoding=control.options.get("output_encoding").map(String::as_str).unwrap_or("UTF-8");
    let bytes=encoding::encode_output(&bbl.text,encoding)?;
    let display_encoding=if matches!(encoding.to_ascii_lowercase().as_str(),"utf8"|"utf-8"){"UTF-8"}else{encoding};
    let mut log=format!("INFO - This is TeXres Biber {}\nINFO - Logfile is '{}'\nINFO - Reading '{}'\n",env!("CARGO_PKG_VERSION"),logfile.display(),opts.bcf.display());
    for section in &control.sections{
        if section.citekeys.iter().any(|key|key=="*"){log.push_str(&format!("INFO - Using all citekeys in bib section {}\n",section.number));}
        else{log.push_str(&format!("INFO - Found {} citekeys in bib section {}\n",section.citekeys.len(),section.number));}
    }
    for (section,_) in &rendered{
        log.push_str(&format!("INFO - Processing section {}\n",section.number));
        for source in &section.sources{
            let datatype=section.datasource_formats.get(source).map(String::as_str).unwrap_or("bibtex");
            log.push_str(&format!("INFO - Looking for {datatype} file '{source}' for section {}\n",section.number));
            if datatype=="bibtex"{log.push_str("INFO - LaTeX decoding ...\n");log.push_str(&format!("INFO - Found BibTeX data source '{source}'\n"));}
            else if datatype=="biblatexml"{log.push_str(&format!("INFO - Found BibLaTeXML data file '{source}'\n"));}
        }
    }
    for warning in &warnings {log.push_str(&format!("WARN - {warning}\n"));}
    for error in &errors {log.push_str(&format!("ERROR - {error}\n"));}
    for info in bbl.infos{log.push_str(&format!("INFO - {info}\n"));}
    log.push_str(&format!("INFO - Writing '{}' with encoding '{display_encoding}'\n",path.display()));
    if control.options.get("output_safechars").is_some_and(|value|matches!(value.as_str(),"1"|"true"|"yes"))||encoding.to_ascii_lowercase().ends_with("ascii")&&!control.options.get("input_encoding").is_some_and(|value|value.to_ascii_lowercase().ends_with("ascii")){
        log.push_str("INFO - Converting UTF-8 to TeX macros on output to .bbl\n");
    }
    for warning in bbl.warnings{log.push_str(&format!("WARN - {warning}\n"));warnings.push(warning);}
    let before_output=log.len();
    log.push_str(&format!("INFO - Output to {}\n",path.display()));
    if !warnings.is_empty(){log.push_str(&format!("INFO - WARNINGS: {}\n",warnings.len()));}
    if !errors.is_empty(){log.push_str(&format!("INFO - ERRORS: {}\n",errors.len()));}
    if path==std::path::Path::new("-"){
        use std::io::Write;
        let quiet=control.options.get("quiet").is_some_and(|value|matches!(value.as_str(),"1"|"true"|"yes"));
        let mut stdout=std::io::stdout().lock();
        if !quiet{stdout.write_all(log[..before_output].as_bytes()).map_err(|error|format!("Cannot write bibliography log to stdout: {error}"))?;}
        stdout.write_all(&bytes).map_err(|error|format!("Cannot write bibliography output to stdout: {error}"))?;
        if !quiet{stdout.write_all(log[before_output..].as_bytes()).map_err(|error|format!("Cannot write bibliography log to stdout: {error}"))?;}
    }else{fs::write(&path,&bytes).map_err(|error|format!("Cannot write {}: {error}",path.display()))?;}
    if !control.options.get("nolog").is_some_and(|value|matches!(value.as_str(),"1"|"true"|"yes")){fs::write(&logfile,log.as_bytes()).map_err(|error|format!("Cannot write bibliography log: {error}"))?;}
    Ok(Outcome {bbl:path,warnings:warnings.len() as u32,errors:errors.len() as u32,log})
}

fn select(control:&Control,section:&Section,all:&[Entry],warnings:&mut Vec<String>)->Vec<Entry>{
    let aliases=alias_map(all);let mut index=all.iter().map(|e|(e.key.clone(),e)).collect::<BTreeMap<_,_>>();let mut keys=vec![];let mut missing=vec![];
    for (alias,key) in &aliases {if let Some(e)=all.iter().find(|e|&e.key==key){index.insert(alias.clone(),e);}}
    // Allkeys sections drop explicitly listed citekeys (Biber::parse_ctrlfile del_citekeys), so unknown ones are never reported.
    let allkeys=section.cite_orders.contains_key("*");
    for key in &section.citekeys {if key=="*"{for e in all {if !keys.contains(&e.key){keys.push(e.key.clone())}}}else if let Some(e)=index.get(key){if !keys.contains(&e.key){keys.push(e.key.clone())}}else if !allkeys{warnings.push(format!("I didn't find a database entry for '{key}' (section {})",section.number));missing.push(Entry {key:key.clone(),kind:"missing".into(),..Default::default()});}}
    let mut refs=BTreeMap::<(String,String),usize>::new();for key in &keys {if let Some(e)=index.get(key.as_str()){for field in ["crossref","xref"]{if let Some(parent)=e.fields.get(field){*refs.entry((field.into(),parent.clone())).or_default()+=1}}}}
    let mut inherited_sources=BTreeSet::new();
    for ((field,key),count) in refs {let min=control.options.get(if field=="crossref"{"mincrossrefs"}else{"minxrefs"}).and_then(|s|s.parse().ok()).unwrap_or(2);if count>=min&&index.contains_key(key.as_str())&&!keys.contains(&key){inherited_sources.insert((field,key.clone()));keys.push(key);}}
    let positions=section.allkeys_order.iter().enumerate().map(|(i,k)|(k.as_str(),i)).collect::<std::collections::HashMap<_,_>>();
    // Allkeys sections start from Biber's datasource key order, not citation order.
    if allkeys {keys.sort_by_key(|k|positions.get(k.as_str()).copied().unwrap_or(usize::MAX));}
    let mut entries=keys.iter().filter_map(|k|index.get(k.as_str()).copied()).cloned().collect::<Vec<_>>();
    let selected=keys.iter().cloned().collect::<BTreeSet<_>>();
    let max_order=section.cite_orders.values().map(|(order,_)|*order).max().unwrap_or(0);
    for e in &mut entries {
        let citation=section.citekeys.iter().find(|key|index.get(*key).is_some_and(|target|target.key==e.key)).map(String::as_str).unwrap_or(&e.key);
        e.cite_count=section.cite_counts.get(citation).copied().unwrap_or(-1);
        if section.nocite.contains(citation) || (section.nocite.contains("*") && !section.citekeys.contains(&e.key)) {e.flags.insert("nocite".into());}
        // Internals::_sort_citeorder: keyorder exists only for \cite'd keys;
        // allkeys entries fall back to keyorder max plus datasource position.
        let ko=section.cite_orders.get(&e.key).map(|(order,_)|*order).filter(|&order|order!=0);
        let order=if allkeys {
            let biborder=max_order+positions.get(e.key.as_str()).map_or(0,|p|p+1);
            match (ko,section.cite_orders.get("*")) {(Some(ko),Some(&(all,_))) if all<ko=>biborder,(Some(ko),_)=>ko,_=>biborder}.to_string()
        } else {ko.map(|o|o.to_string()).unwrap_or_default()};
        let intorder=section.cite_orders.get(&e.key).map(|(_,intorder)|intorder.to_string()).unwrap_or_default();
        e.computed.insert("citeorder".into(),order);e.computed.insert("intciteorder".into(),intorder);
        for field in ["crossref","xref"] {if e.fields.get(field).is_some_and(|k|!selected.contains(k)){e.fields.remove(field);}}
    }
    for (field,key) in inherited_sources {if let Some(e)=entries.iter_mut().find(|e|e.key==key){e.flags.insert(format!("{field}source"));}}
    let mut sets=vec![];for e in &entries {if e.kind=="set" {if let Some(children)=e.fields.get("entryset"){for child in bib::split_list(children,","){sets.push((e.key.clone(),child));}}}}
    for (set,child) in sets {if !entries.iter().any(|e|e.key==child){if let Some(e)=index.get(child.as_str()){let mut clone=(*e).clone();clone.cite_count=-1;entries.push(clone)}else{warnings.push(format!("Missing set member {child}"));continue}}
        if let Some(e)=entries.iter_mut().find(|e|e.key==child){
            if e.fields.remove("entryset").is_some(){e.warnings.push(format!("Field 'entryset' is no longer needed in set member entries in Biber - ignoring in entry '{child}'"));}
            e.computed.insert("inset".into(),set);set_dataonly_options(e);
        }
    }
    let mut clones=vec![];let mut clonekeys=BTreeMap::new();
    for e in &mut entries {related_clones(e,&index,&selected,section.number,&mut clonekeys,&mut clones,warnings);}
    // Entry::relclone add_citekeys()s each clone, so it is appended to orig_order_citekeys and gets process_nocite/_sort_citeorder like any citekey.
    let mut next=section.allkeys_order.len();
    for e in &mut clones {
        if section.nocite.contains("*")&&!section.citekeys.contains(&e.key){e.flags.insert("nocite".into());}
        if allkeys {let p=positions.get(e.key.as_str()).copied().unwrap_or_else(||{next+=1;next-1});e.computed.insert("citeorder".into(),(max_order+p+1).to_string());e.computed.insert("intciteorder".into(),String::new());}
    }
    let referenced_aliases=entries.iter().chain(clones.iter()).filter_map(|e|e.computed.get("referencealiases")).flat_map(|v|bib::split_list(v,",")).collect::<BTreeSet<_>>();
    for (alias,target) in aliases {if section.citekeys.contains(&alias)||section.nocite.contains("*")||referenced_aliases.contains(&alias){let mut e=Entry {key:alias,kind:"alias".into(),..Default::default()};e.computed.insert("target".into(),target);entries.push(e);}}
    for e in clones {if !entries.iter().any(|x|x.key==e.key){entries.push(e)}}entries.extend(missing);entries
}

fn set_dataonly_options(entry:&mut Entry){
    let mut options=entry.fields.get("options").map(|value|bib::split_list(value,",")).unwrap_or_default();
    options.retain(|option|!matches!(option.split('=').next().unwrap_or("").trim(),"skipbib"|"skipbiblist"|"skiplab"|"uniquename"|"uniquelist"));
    options.extend(["skipbib=true","skipbiblist=true","skiplab=true","uniquename=false","uniquelist=false"].into_iter().map(str::to_owned));
    entry.fields.insert("options".into(),options.join(","));
}

fn related_clones(entry:&mut Entry,index:&BTreeMap<String,&Entry>,cited:&BTreeSet<String>,section:usize,known:&mut BTreeMap<String,String>,clones:&mut Vec<Entry>,warnings:&mut Vec<String>){
    let Some(related)=entry.fields.get("related").cloned()else{return};
    let mut keys=vec![];let mut created=vec![];
    for key in bib::split_list(&related,","){
        if let Some(clonekey)=known.get(&key){keys.push(clonekey.clone());continue}
        let Some(source)=index.get(&key)else{
            warnings.push(format!("I didn't find a database entry for related entry '{key}' in entry '{}' - ignoring (section {section})",entry.key));continue
        };
        let clonekey=md5_hex(key.as_bytes());known.insert(key.clone(),clonekey.clone());keys.push(clonekey.clone());
        let mut clone=(*source).clone();clone.key=clonekey;clone.cite_count=-1;clone.fields.insert("clonesourcekey".into(),key.clone());
        if let Some(options)=entry.fields.get("relatedoptions"){
            clone.fields.insert("options".into(),options.clone());
            if cited.contains(&key){merge_default_options(&mut clone,&["skipbib=true","skipbiblist=true"]);}
        }else{merge_default_options(&mut clone,&["skipbib=true","skipbiblist=true","skiplab=true","uniquename=false","uniquelist=false"]);}
        clone.computed.insert("relatedclone".into(),"1".into());
        related_clones(&mut clone,index,cited,section,known,clones,warnings);created.push(clone);
    }
    clones.extend(created);
    if keys.is_empty(){entry.fields.remove("related");entry.fields.remove("relatedtype");entry.fields.remove("relatedstring");}
    else{entry.fields.insert("related".into(),keys.join(","));}
}

fn merge_default_options(entry:&mut Entry,defaults:&[&str]){
    let mut options=entry.fields.get("options").map(|value|bib::split_list(value,",")).unwrap_or_default();
    for default in defaults{
        let name=default.split('=').next().unwrap();
        if !options.iter().any(|option|option.split('=').next().unwrap_or("").trim()==name){options.push((*default).into());}
    }
    entry.fields.insert("options".into(),options.join(","));
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
