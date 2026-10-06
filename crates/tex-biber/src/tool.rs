//! Biber 2.22 datasource conversion. No control file or label generation is used.
use crate::{bib, biblatexml, config, model::*, process, validation, Options, Outcome};
use std::collections::{BTreeMap, BTreeSet};
use tex_kpse::fs;
use std::path::{Path, PathBuf};
use crate::collation::normalization::normalize;
pub(crate) mod writers;
pub(crate) mod recode;
mod schema;

const DEFAULT: &str = include_str!("tool-default.xml");
const SCHEMA: &str = include_str!("tool-default.rng");

pub(crate) fn option<'a>(c: &'a Control, key: &str, fallback: &'a str) -> &'a str {
    c.options.get(key).map(String::as_str).unwrap_or(fallback)
}
pub(crate) fn enabled(c: &Control, key: &str) -> bool {
    matches!(option(c,key,"0"), "1" | "true" | "yes")
}
fn defaults(configuration: Option<&str>) -> Result<Control,String> {
    let source=DEFAULT;
    let document=roxmltree::Document::parse(source).map_err(|e|e.to_string())?;
    if document.root_element().tag_name().name()!="config" {return Err("Biber configuration root is not config".into());}
    let mut xml=String::from("<controlfile version=\"3.11\"><section number=\"99999\"/>");
    for node in document.root_element().children().filter(roxmltree::Node::is_element) {
        xml.push_str(&source[node.range()]);
    }
    xml.push_str("</controlfile>");
    let mut control=crate::bcf::parse(&xml)?;
    for (key,value) in [("sortingtemplatename","tool"),("sortlocale","en_US"),("sortcase","1"),("sortupper","1"),("useprefix","0"),("useauthor","1"),("useeditor","1"),("usetranslator","1"),("maxsortnames","100"),("minsortnames","100"),("maxitems","100"),("minitems","100"),("input_format","bibtex"),("output_format","bibtex"),("tool","1"),("collate_options.level","4"),("collate_options.variable","non-ignorable"),("collate_options.normalization","prenormalized")] {
        control.options.insert(key.into(),value.into());
    }
    let mut scalar=String::from("<config>");
    for node in document.root_element().children().filter(roxmltree::Node::is_element) {
        if !matches!(node.tag_name().name(),"sourcemap"|"inheritance"|"labelalphatemplate"|"labelalphanametemplate"|"namehashtemplate"|"uniquenametemplate"|"sortingnamekeytemplate"|"sortingtemplate"|"optionscope"|"datamodel") {
            scalar.push_str(&source[node.range()]);
        }
    }
    scalar.push_str("</config>");
    config::apply(&mut control,&scalar)?;
    if let Some(configuration)=configuration{config::apply(&mut control,configuration)?;}
    Ok(control)
}


/// The source path occupies Options::bcf; the output callback resolves local data.
pub(crate) fn run(opts: &Options<'_>, overrides: &BTreeMap<String,String>, configuration: Option<&str>) -> Result<Outcome,String> {
    let mut control=defaults(configuration)?;
    control.options.extend(overrides.clone());
    if enabled(&control,"output_resolve") {
        for key in ["output_resolve_xdata","output_resolve_crossrefs","output_resolve_sets"] {control.options.insert(key.into(),"1".into());}
    }
    let format=option(&control,"output_format","bibtex");
    if !matches!(format,"bibtex"|"biblatexml") {
        return Err(if format=="bibjson" {"BibJSON tool output is not implemented".into()} else {"Biber: Output format in tool mode must be one of 'bibtex', 'biblatexml' or 'bibjson'".into()});
    }
    crate::collation::validate_options(&control)?;
    crate::names::validate_control_rules(&control)?;
    // User configuration overlays the tool defaults; the bundled datamodel
    // remains the base unless it is explicitly disabled.
    control.fields.clear();control.field_order.clear();control.nameparts.clear();
    control.datamodel=Default::default();control.skip_types.clear();
    if !enabled(&control,"no_default_datamodel"){
        let doc=roxmltree::Document::parse(DEFAULT).map_err(|e|e.to_string())?;
        for model in doc.root_element().children().filter(|n|n.is_element()&&n.tag_name().name()=="datamodel"){parse_model(model,&mut control)?;}
    }
    let model_source=configuration.unwrap_or(DEFAULT);
    if enabled(&control,"no_default_datamodel")||configuration.is_some(){
        let doc=roxmltree::Document::parse(model_source).map_err(|e|e.to_string())?;
        for model in doc.root_element().children().filter(|n|n.is_element()&&n.tag_name().name()=="datamodel"){parse_model(model,&mut control)?;}
    }
    if option(&control,"output_encoding","UTF-8").to_ascii_lowercase().ends_with("ascii")&&!option(&control,"input_encoding","UTF-8").to_ascii_lowercase().ends_with("ascii"){control.options.insert("output_safechars".into(),"1".into());}
    let format=option(&control,"output_format","bibtex").to_owned();
    let source=opts.bcf.to_string_lossy().into_owned();
    let path=if fs::metadata(&opts.bcf).is_ok_and(|metadata|metadata.is_file()){Some(opts.bcf.clone())}else{(opts.find_file)(&source)};
    let remote=path.is_none();
    let bytes=if let Some(path)=path{fs::read(&path).map_err(|e|format!("Cannot read {}: {e}",path.display()))?}else{crate::remote::Cache::default().fetch(&source,&control.options)?.ok_or_else(||format!("Cannot find datasource {source}"))?.to_vec()};
    let text=crate::encoding::decode(&bytes,option(&control,"input_encoding","UTF-8"))?;
    let mut db=bib::Database::default();
    let input=option(&control,"input_format","bibtex");
    let generated_schema=if format=="biblatexml"||input=="biblatexml"{Some(schema::generate(&control))}else{None};
    let schema_path=opts.output_directory.as_ref().map(|parent|parent.join(opts.bcf.file_name().unwrap_or_default()).with_extension("rng")).unwrap_or_else(||opts.bcf.with_extension("rng"));
    if let Some(schema)=&generated_schema{
        if !enabled(&control,"no_bltxml_schema"){fs::write(&schema_path,schema).map_err(|e|format!("Cannot write tool schema: {e}"))?;}
        if input=="biblatexml"&&enabled(&control,"validate_bltxml"){
            if !enabled(&control,"no_bltxml_schema"){schema::validate(&text,schema).map_err(|_|format!("'{source}' failed to validate against schema '{}'",schema_path.display()))?;}
            else if let Ok(schema)=fs::read_to_string(&schema_path){schema::validate(&text,&schema)?;}
        }
    }
    db.standard_macros(enabled(&control,"nostdmacros"));
    let mut section=Section {number:99999,sources:vec![source.clone()],citekeys:vec!["*".into()],..Default::default()};
    section.datasource_formats.insert(source.clone(),input.into());
    let metadata=match input {
        "bibtex"=>{db.add(&text,&source)?;bib::decode_entries(&control,&mut db.entries);writers::Metadata::parse(&text,enabled(&control,"nostdmacros"))?},
        "biblatexml"=>{biblatexml::add(&control,&mut section,&mut db,&text,&source,remote)?;writers::Metadata::default()},
        other=>return Err(format!("Unsupported datasource datatype '{other}'")),
    };
    for entry in &mut db.entries {entry.source=source.clone();if remote{entry.computed.insert("sourcemap_remote".into(),"1".into());}}
    let mut warnings=control.warnings.clone();
    if input=="biblatexml"&&enabled(&control,"validate_bltxml")&&enabled(&control,"no_bltxml_schema")&&fs::metadata(&schema_path).is_err(){warnings.push(format!("Cannot find XML::LibXML::RelaxNG schema '{}'. Skipping validation : No such file or directory",schema_path.display()));}
    bib::sourcemaps(&control,&mut section,&mut db.entries,&mut warnings)?;
    warnings.append(&mut db.warnings);
    for entry in &mut db.entries {
        entry.computed.insert("refsection".into(),"99999".into());
        validation::validate_input(&control,entry);
        if !entry.computed.get("sourcemap_datatype").is_some_and(|datatype|datatype.starts_with("biblatexml")) {bib::normalize(&control,entry);}
        warnings.append(&mut entry.input_warnings);
    }
    crate::resolve_alias_refs(&mut db.entries);
    prune_missing(&control,&mut db.entries,&mut warnings);
    if enabled(&control,"output_resolve_sets"){resolve_sets(&mut db.entries,&mut warnings);}
    if enabled(&control,"output_resolve_crossrefs") || enabled(&control,"output_resolve_xdata") {
        // The shared inheritance engine resolves both classes. Withhold the class
        // not requested so it cannot leak inherited data into tool output.
        let mut withheld=Vec::with_capacity(db.entries.len());
        for entry in &mut db.entries {
            let mut refs=BTreeMap::new();
            for key in ["crossref","xref","xdata"] {
                let resolve=if key=="xdata" {enabled(&control,"output_resolve_xdata")}else{enabled(&control,"output_resolve_crossrefs")};
                if !resolve {if let Some(value)=entry.fields.remove(key){refs.insert(key.to_owned(),value);}}
            }
            withheld.push(refs);
        }
        bib::inherit(&control,&mut db.entries,&mut warnings);
        for (entry,refs) in db.entries.iter_mut().zip(withheld) {
            entry.fields.extend(refs);
            if enabled(&control,"output_resolve_crossrefs") {entry.fields.remove("crossref");}
        }
    }
    let mut errors=Vec::new();
    for entry in &mut db.entries {warnings.append(&mut entry.warnings);}
    validation::validate(&control,&mut db.entries)?;
    for entry in &mut db.entries {if enabled(&control,"dieondatamodel"){errors.append(&mut entry.warnings);}else{warnings.append(&mut entry.warnings);}}
    let sorting=option(&control,"sortingtemplatename","tool");
    if !control.sorting.contains_key(sorting) {return Err(format!("Sorting template '{sorting}' is not defined"));}
    let list=DataList {sorting:sorting.into(),namekey:"global".into(),..Default::default()};
    let order=process::sort(&control,&list,&db.entries);
    let output=opts.output.clone().unwrap_or_else(||default_output(&opts.bcf,&format));
    let output_warning_start=warnings.len();
    let rendered=if format=="bibtex" {writers::bibtex(&control,&db.entries,&order,&metadata,&mut warnings)?}else{writers::xml(&control,&db.entries,&order,&opts.bcf)};
    let rendered=normalize(&rendered,"NFC");
    let bytes=crate::encoding::encode_output(&rendered,option(&control,"output_encoding","UTF-8"))?;
    let logfile=logfile_path(&control,opts,true);
    let banner=format!("INFO - This is TeXres Biber 2.22 running in TOOL mode\nINFO - Logfile is '{}'\n",logfile.display());
    let mut log=banner.clone();
    if input=="biblatexml"&&!enabled(&control,"no_bltxml_schema"){log.push_str(&format!("INFO - Writing BibLaTeXML RNG schema '{}' for datamodel\n",schema_path.display()));}
    log.push_str(&format!("INFO - Looking for {input} file '{source}'\n"));
    if input=="bibtex"{log.push_str(&format!("INFO - LaTeX decoding ...\nINFO - Found BibTeX data source '{source}'\n"));}else{log.push_str(&format!("INFO - Found BibLaTeXML data file '{source}'\n"));}
    for warning in &warnings[..output_warning_start]{log.push_str(&format!("WARN - {warning}\n"));}
    if enabled(&control,"validate_datamodel"){log.push_str("INFO - Datamodel validation starting\n");}
    for error in &errors{log.push_str(&format!("ERROR - {error}\n"));}
    if enabled(&control,"validate_datamodel"){log.push_str("INFO - Datamodel validation complete\n");}
    let collate_info=crate::collation::tool_log_info(&control,&list);
    for info in collate_info.iter().filter(|info|!info.starts_with("No sort tailoring")){log.push_str(&format!("INFO - {info}\n"));}
    let locale=control.sorting_locales.get(sorting).map(String::as_str).unwrap_or_else(||option(&control,"sortlocale","en_US"));
    log.push_str(&format!("INFO - Sorting list '{sorting}/global//global/global/global' of type 'entry' with template '{sorting}' and locale '{locale}'\n"));
    for info in collate_info.iter().filter(|info|info.starts_with("No sort tailoring")){log.push_str(&format!("INFO - {info}\n"));}
    for warning in &warnings[output_warning_start..]{log.push_str(&format!("WARN - {warning}\n"));}
    if format=="bibtex"{
        log.push_str(&format!("INFO - Writing '{}' with encoding '{}'\n",output.display(),option(&control,"output_encoding","UTF-8")));
        if enabled(&control,"output_safechars"){log.push_str("INFO - Converting UTF-8 to TeX macros on output\n");}
    }
    let noisy_stdout=output==Path::new("-")&&!enabled(&control,"quiet");
    if output==Path::new("-") {
        use std::io::Write;
        let mut stdout=std::io::stdout().lock();
        if noisy_stdout{stdout.write_all(if format=="bibtex"{log.as_bytes()}else{banner.as_bytes()}).map_err(|e|e.to_string())?;}
        stdout.write_all(&bytes).map_err(|e|format!("Cannot write tool output to stdout: {e}"))?;
    } else {
        fs::write(&output,&bytes).map_err(|e|format!("Cannot write {}: {e}",output.display()))?;
    }
    let prefix_end=if format=="bibtex"{log.len()}else{banner.len()};
    let logged_warnings=warnings.len();
    if format=="biblatexml"&&enabled(&control,"validate_bltxml"){
        if !enabled(&control,"no_bltxml_schema"){schema::validate(&rendered,generated_schema.as_deref().unwrap())?;}
        else if let Ok(schema)=fs::read_to_string(&schema_path){schema::validate(&rendered,&schema)?;}
        else{warnings.push(format!("Cannot find XML::LibXML::RelaxNG schema '{}'. Skipping validation : No such file or directory",schema_path.display()));}
    }
    for warning in &warnings[logged_warnings..]{log.push_str(&format!("WARN - {warning}\n"));}
    log.push_str(&format!("INFO - Output to {}\n",output.display()));
    if format=="biblatexml"&&input!="biblatexml"&&!enabled(&control,"no_bltxml_schema"){log.push_str(&format!("INFO - Writing BibLaTeXML RNG schema '{}' for datamodel\n",schema_path.display()));}
    if !warnings.is_empty(){log.push_str(&format!("INFO - WARNINGS: {}\n",warnings.len()));}
    if !errors.is_empty(){log.push_str(&format!("INFO - ERRORS: {}\n",errors.len()));}
    if noisy_stdout{use std::io::Write;std::io::stdout().lock().write_all(log[prefix_end..].as_bytes()).map_err(|e|e.to_string())?;}
    fs::write(logfile,log.as_bytes()).map_err(|e|format!("Cannot write bibliography log: {e}"))?;
    Ok(Outcome {bbl:output,warnings:warnings.len() as u32,errors:errors.len() as u32,log})
}
fn default_output(path:&Path,format:&str)->PathBuf {
    let stem=path.file_stem().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{stem}_bibertool.{}",if format=="biblatexml"{"bltxml"}else{"bib"}))
}
fn prune_missing(c:&Control,entries:&mut [Entry],warnings:&mut Vec<String>) {
    let keys=entries.iter().map(|e|e.key.clone()).collect::<BTreeSet<_>>();
    for entry in entries {
        for field in ["crossref","xref","xdata","related","entryset"] {
            let Some(value)=entry.fields.get(field).cloned()else{continue};
            let mut retained=Vec::new();
            for key in bib::split_list(&value,",") {
                if keys.contains(&key) {retained.push(key);}else{
                    warnings.push(format!("I didn't find a database entry for '{key}' (section 99999)"));
                    if enabled(c,"tool_noremove_missing_dependants") {retained.push(key);}
                }
            }
            if retained.is_empty(){entry.fields.remove(field);}else{entry.fields.insert(field.into(),retained.join(","));}
        }
    }
}
fn resolve_sets(entries:&mut Vec<Entry>,warnings:&mut Vec<String>){
    let snapshot=entries.clone();
    let index=snapshot.iter().map(|e|(e.key.clone(),e)).collect::<BTreeMap<_,_>>();
    let cited=index.keys().cloned().collect::<BTreeSet<_>>();
    let mut members=BTreeSet::new();
    for set in snapshot.iter().filter(|e|e.kind=="set"){
        if let Some(keys)=set.fields.get("entryset"){members.extend(bib::split_list(keys,","));}
        else{warnings.push(format!("Set entry '{}' has no entryset field, ignoring",set.key));}
    }
    let mut known=BTreeMap::new();let mut clones=Vec::new();
    for entry in entries.iter_mut().filter(|e|members.contains(&e.key)){
        crate::related_clones(entry,&index,&cited,99999,&mut known,&mut clones,warnings);
    }
    for mut clone in clones{
        if let Some(options)=clone.fields.get_mut("options"){*options=options.replace("skipbib=true","skipbib=1").replace("skipbiblist=true","skipbiblist=1").replace("skiplab=true","skiplab=1");}
        if !entries.iter().any(|e|e.key==clone.key){entries.push(clone);}
    }
}
fn parse_model(model:roxmltree::Node<'_, '_>,control:&mut Control)->Result<(),String>{
    validation::parse(model,control)?;
    for constant in model.children().filter(|n|n.is_element()&&n.tag_name().name()=="constants").flat_map(|n|n.children().filter(|n|n.is_element())){
        if constant.attribute("name")==Some("gender"){control.options.insert("tool.gender".into(),constant.text().unwrap_or("").trim().into());}
    }
    Ok(())
}
pub(crate) fn logfile_path(control:&Control,opts:&Options<'_>,tool:bool)->PathBuf{
    let path=if let Some(logfile)=control.options.get("logfile"){PathBuf::from(if logfile.ends_with(".blg"){logfile.clone()}else{format!("{logfile}.blg")})}
        else if tool{let mut path=opts.bcf.as_os_str().to_os_string();path.push(".blg");PathBuf::from(path)}
        else{opts.bcf.with_extension("blg")};
    opts.output_directory.as_ref().map(|directory|directory.join(path.file_name().unwrap_or_default())).unwrap_or(path)
}
