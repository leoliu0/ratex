use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::{run_configured, run_tool_configured, Options};
use tex_kpse::fs;

/// Run the Biber command-line personality, with arguments excluding argv[0].
///
/// Returns an exit status instead of terminating the embedding executable.
/// Job names and separate directory-option values retain their native OS bytes.
pub fn cli_main(args: &[OsString]) -> i32 {
    match execute(args) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("ERROR - {error}");
            2
        }
    }
}

fn execute(args: &[OsString]) -> Result<i32, String> {
    let mut input = PathBuf::from(".");
    let mut output = None;
    let mut outfile=None;
    let mut quiet = false;
    let mut job = None;
    let mut overrides = std::collections::BTreeMap::new();
    let mut configfile = None;
    let mut noconf = false;
    let mut positional_only = false;
    let mut args = args.iter();
    while let Some(raw) = args.next() {
        if positional_only {
            set_job(&mut job, raw)?;
            continue;
        }
        match raw.to_str() {
            Some("-q" | "--quiet") => quiet = true,
            Some("--sortlocale" | "-l" | "--sortcase" | "--sortupper" | "--collate-options" | "--collate_options" | "-c") => {
                let option = raw.to_str().unwrap();
                let value = args.next().ok_or_else(|| format!("Missing {option} value"))?;
                set_override(&mut overrides, option, value)?;
            }
            Some("--validate-datamodel" | "--validate_datamodel" | "-V") => {
                overrides.insert("validate_datamodel".into(), "1".into());
            }
            Some("--configfile" | "-g") => {
                configfile = Some(PathBuf::from(args.next().ok_or("Missing --configfile value")?));
            }
            Some("--noconf") => noconf = true,
            Some("--output-file" | "--output_file" | "--outfile" | "-O") => {
                outfile=Some(PathBuf::from(args.next().ok_or("Missing --output-file value")?));
            }
            Some("--output-directory") => {
                output = Some(PathBuf::from(args.next().ok_or("Missing --output-directory value")?));
            }
            Some("--input-directory") => {
                input = PathBuf::from(args.next().ok_or("Missing --input-directory value")?);
            }
            Some("--version" | "-v") => {
                println!("biber version: 2.22");
                return Ok(0);
            }
            Some("--help" | "-h") => {
                println!("Usage: biber [--input-directory DIR] [--output-directory DIR] [-q|--quiet] [--sortlocale LOCALE] [--sortcase true|false] [--sortupper true|false] [--collate-options KEY=VALUE] [--configfile FILE] [--validate-datamodel] <job[.bcf]>");
                return Ok(0);
            }
            Some("--") => positional_only = true,
            _ => {
                if let Some(value) = option_value(raw, "--output-directory=") {
                    output = Some(PathBuf::from(value));
                } else if let Some(value) = option_value(raw, "--input-directory=") {
                    input = PathBuf::from(value);
                } else if let Some(value) = option_value(raw, "--configfile=") {
                    configfile = Some(PathBuf::from(value));
                } else if let Some(value)=option_value(raw,"--output-file=").or_else(||option_value(raw,"--output_file=")).or_else(||option_value(raw,"--outfile=")) {
                    outfile=Some(PathBuf::from(value));
                } else if let Some((option, value)) = raw.to_str().and_then(|arg| arg.split_once('='))
                    .filter(|(option, _)| value_option(option)) {
                    set_override(&mut overrides, option, OsStr::new(value))?;
                } else if let Some(option) = raw.to_str().filter(|option| value_option(option)) {
                    let value = args.next().ok_or_else(|| format!("Missing {option} value"))?;
                    set_override(&mut overrides, option, value)?;
                } else if let Some(option) = raw.to_str().filter(|option| flag_option(option)) {
                    set_override(&mut overrides, option, OsStr::new("1"))?;
                } else if raw.as_encoded_bytes().starts_with(b"-") {
                    return Err(format!("Unknown option {}", raw.to_string_lossy()));
                } else {
                    set_job(&mut job, raw)?;
                }
            }
        }
    }
    let mut bcf = job.ok_or("Missing job name (use --help for usage)")?;
    let tool=overrides.get("tool").is_some_and(|value|value=="1");
    if !tool && bcf.extension().is_none() {
        bcf.set_extension("bcf");
    }
    if !bcf.is_absolute() && input!=Path::new(".") {
        bcf = input.join(bcf);
    }
    let output_directory=output.clone();
    let out=match (output,outfile){
        (Some(_),Some(file)) if file==Path::new("-")=>Some(file),
        (Some(dir),Some(file))=>Some(dir.join(file)),
        (None,Some(file))=>Some(file),
        (Some(dir),None)=>{
            let file=if tool{
                let extension=if overrides.get("output_format").is_some_and(|format|format=="biblatexml"){"bltxml"}else{"bib"};
                let mut file=bcf.file_stem().ok_or("Job has no file name")?.to_os_string();file.push(format!("_bibertool.{extension}"));PathBuf::from(file)
            }else{PathBuf::from(bcf.file_name().ok_or("Job has no file name")?).with_extension("bbl")};
            Some(dir.join(file))
        },
        (None,None)=>None,
    };
    if let Some(directory)=&output_directory{fs::create_dir_all(directory).map_err(|error|error.to_string())?;}
    if let Some(parent) = out.as_ref().and_then(|path| path.parent()) {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let find = |name: &str| find_file(name, &input, bcf.parent().unwrap_or(Path::new(".")));
    let configuration = if noconf { None } else {
        configfile.or_else(default_config).map(|path| fs::read_to_string(&path)
            .map_err(|error| format!("Cannot read Biber configuration {}: {error}", path.display()))).transpose()?
    };
    let options=Options {bcf:bcf.clone(),output:out,output_directory,find_file:&find};
    if quiet{overrides.insert("quiet".into(),"1".into());}
    let result=if tool{run_tool_configured(&options,&overrides,configuration.as_deref())?}else{run_configured(&options,&overrides,configuration.as_deref())?};
    if !quiet&&result.bbl!=Path::new("-") {
        print!("{}", result.log);
    }
    Ok(if result.errors==0{0}else{2})
}

fn set_override(options: &mut std::collections::BTreeMap<String, String>, option: &str, value: &OsStr) -> Result<(), String> {
    let value = value.to_str().ok_or_else(|| format!("{option} requires a Unicode value"))?;
    let key = match option {
        "--sortlocale" | "-l" => std::borrow::Cow::Borrowed("sortlocale"),
        "--sortcase" => std::borrow::Cow::Borrowed("sortcase"),
        "--sortupper" => std::borrow::Cow::Borrowed("sortupper"),
        "--collate-options" | "--collate_options" | "-c" => {
            let (name, value) = value.split_once('=').ok_or_else(|| format!("Invalid collation option '{value}', expected NAME=VALUE"))?;
            options.insert(format!("collate_options.{name}"), value.into());
            return Ok(());
        }
        "-m" => std::borrow::Cow::Borrowed("mincrossrefs"),
        "--bibencoding" | "--input-encodinge" => std::borrow::Cow::Borrowed("input_encoding"),
        "--outfile" | "-O" => std::borrow::Cow::Borrowed("output_file"),
        "--outformat" => std::borrow::Cow::Borrowed("output_format"),
        _ => {
            let option=option.strip_prefix("--").ok_or_else(||format!("Unknown option {option}"))?;
            if option.contains('-'){std::borrow::Cow::Owned(option.replace('-',"_"))}else{std::borrow::Cow::Borrowed(option)}
        },
    };
    options.insert(key.into_owned(), match value { "true" => "1", "false" => "0", _ => value }.into());
    Ok(())
}

fn value_option(option:&str)->bool {
    matches!(option,
        "--sortlocale"|"--sortcase"|"--sortupper"|"--collate-options"|"--collate_options"|
        "--input-encoding"|"--input_encoding"|"--input-encodinge"|"--bibencoding"|
        "--input-format"|"--input_format"|"--annotation-marker"|"--annotation_marker"|
        "--named-annotation-marker"|"--named_annotation_marker"|"--xsvsep"|"--xdatamarker"|
        "--xdatasep"|"--xnamesep"|"--namesep"|"--listsep"|"--others-string"|"--others_string"|"--mincrossrefs"|"-m"|
        "--output-file"|"--output_file"|"--outfile"|"-O"|"--output-format"|"--output_format"|"--outformat"|
        "--output-fieldcase"|"--output_fieldcase"|"--output-field-order"|"--output_field_order"|
        "--output-field-replace"|"--output_field_replace"|"--output-indent"|"--output_indent"|
        "--output-listsep"|"--output_listsep"|"--output-namesep"|"--output_namesep"|
        "--output-xdatamarker"|"--output_xdatamarker"|"--output-xdatasep"|"--output_xdatasep"|
        "--output-xnamesep"|"--output_xnamesep"|"--output-macro-fields"|"--output_macro_fields"|
        "--output-annotation-marker"|"--output_annotation_marker"|
        "--output-named-annotation-marker"|"--output_named_annotation_marker"|
        "--output-encoding"|"--output_encoding"|
        "--logfile"|
        "--output-safecharsset"|"--output_safecharsset"|"--wraplines")
}

fn flag_option(option:&str)->bool {
    matches!(option,"--tool"|"--output-align"|"--output_align"|
        "--output-all-macrodefs"|"--output_all_macrodefs"|"--output-no-macrodefs"|"--output_no_macrodefs"|
        "--output-resolve"|"--output_resolve"|"--output-resolve-xdata"|"--output_resolve_xdata"|
        "--output-resolve-crossrefs"|"--output_resolve_crossrefs"|"--output-resolve-sets"|"--output_resolve_sets"|
        "--output-legacy-dates"|"--output_legacy_dates"|"--output-xname"|"--output_xname"|
        "--output-safechars"|"--output_safechars"|"--strip-comments"|"--strip_comments"|
        "--validate-bltxml"|"--validate_bltxml"|"--no-bltxml-schema"|"--no_bltxml_schema"|
        "--ssl-nointernalca"|"--ssl_nointernalca"|"--ssl-noverify-host"|"--ssl_noverify_host"|
        "--no-default-datamodel"|"--no_default_datamodel"|"--nostdmacros"|"--noxname"|
        "--tool-noremove-missing-dependants"|"--tool_noremove_missing_dependants"|"--dieondatamodel")
}

fn default_config() -> Option<PathBuf> {
    let mut candidates = vec![PathBuf::from("biber.conf"), PathBuf::from(".biber.conf")];
    if let Some(home) = std::env::var_os("HOME") { candidates.push(PathBuf::from(home).join(".biber.conf")); }
    if let Some(root) = std::env::var_os("XDG_CONFIG_HOME") { candidates.push(PathBuf::from(root).join("biber/biber.conf")); }
    if let Some(home) = std::env::var_os("HOME") { candidates.push(PathBuf::from(home).join(".config/biber/biber.conf")); }
    candidates.into_iter().find(|path| fs::metadata(path).is_ok_and(|metadata| metadata.is_file()))
}

fn set_job(job: &mut Option<PathBuf>, value: &OsStr) -> Result<(), String> {
    if job.replace(PathBuf::from(value)).is_some() {
        return Err("Only one job can be processed at a time".into());
    }
    Ok(())
}

fn option_value<'a>(arg: &'a OsStr, prefix: &str) -> Option<&'a OsStr> {
    let value = arg.as_encoded_bytes().strip_prefix(prefix.as_bytes())?;
    // The removed prefix is ASCII ending in '=', a boundary in every native
    // OsStr encoding; the suffix retains exactly its original valid encoding.
    Some(unsafe { OsStr::from_encoded_bytes_unchecked(value) })
}

fn find_file(name: &str, input: &Path, bcfdir: &Path) -> Option<PathBuf> {
    let path = PathBuf::from(name);
    let readable = |path: &Path| fs::metadata(path).is_ok_and(|metadata| metadata.is_file());
    for candidate in [path.clone(), input.join(&path), bcfdir.join(&path)] {
        if readable(&candidate) {
            return Some(candidate);
        }
    }
    if let Some(var) = std::env::var_os("BIBINPUTS") {
        for root in std::env::split_paths(&var) {
            if root.as_os_str().is_empty() {
                continue;
            }
            let recursive = root.as_os_str().as_encoded_bytes().ends_with(b"//");
            // PathBuf normalizes a trailing separator; no string conversion is
            // needed, so non-Unicode search roots remain usable.
            let candidate = root.join(&path);
            if readable(&candidate) {
                return Some(candidate);
            }
            if recursive {
                if let Some(found) = recursive_find(&root, &path) {
                    return Some(found);
                }
            }
        }
    }
    None
}

fn recursive_find(root: &Path, name: &Path) -> Option<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let candidate = dir.join(name);
        if fs::metadata(&candidate).is_ok_and(|metadata| metadata.is_file()) {
            return Some(candidate);
        }
        if let Ok(files) = fs::read_dir(dir) {
            for item in files.flatten() {
                if item.file_type().is_ok_and(|kind| kind.is_dir() && !kind.is_symlink()) {
                    pending.push(item.path());
                }
            }
        }
    }
    None
}
