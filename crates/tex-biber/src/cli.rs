use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::{run, Options};
use tex_kpse::fs;

/// Run the Biber command-line personality, with arguments excluding argv[0].
///
/// Returns an exit status instead of terminating the embedding executable.
/// Job names and separate directory-option values retain their native OS bytes.
pub fn cli_main(args: &[OsString]) -> i32 {
    match execute(args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("ERROR - {error}");
            2
        }
    }
}

fn execute(args: &[OsString]) -> Result<(), String> {
    let mut input = PathBuf::from(".");
    let mut output = None;
    let mut quiet = false;
    let mut job = None;
    let mut positional_only = false;
    let mut args = args.iter();
    while let Some(raw) = args.next() {
        if positional_only {
            set_job(&mut job, raw)?;
            continue;
        }
        match raw.to_str() {
            Some("-q" | "--quiet") => quiet = true,
            Some("--output-directory") => {
                output = Some(PathBuf::from(args.next().ok_or("Missing --output-directory value")?));
            }
            Some("--input-directory") => {
                input = PathBuf::from(args.next().ok_or("Missing --input-directory value")?);
            }
            Some("--version" | "-V") => {
                println!("TeXres Biber {} (BCF 3.11, BBL 3.3)", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            Some("--help" | "-h") => {
                println!("Usage: biber [--input-directory DIR] [--output-directory DIR] [-q|--quiet] <job[.bcf]>");
                return Ok(());
            }
            Some("--") => positional_only = true,
            _ => {
                if let Some(value) = option_value(raw, "--output-directory=") {
                    output = Some(PathBuf::from(value));
                } else if let Some(value) = option_value(raw, "--input-directory=") {
                    input = PathBuf::from(value);
                } else if raw.as_encoded_bytes().starts_with(b"-") {
                    return Err(format!("Unknown option {}", raw.to_string_lossy()));
                } else {
                    set_job(&mut job, raw)?;
                }
            }
        }
    }
    let mut bcf = job.ok_or("Missing job name (use --help for usage)")?;
    if bcf.extension().is_none() {
        bcf.set_extension("bcf");
    }
    if !bcf.is_absolute() {
        bcf = input.join(bcf);
    }
    let out = match output {
        Some(dir) => Some(dir.join(bcf.file_name().ok_or("Job has no file name")?).with_extension("bbl")),
        None => None,
    };
    if let Some(parent) = out.as_ref().and_then(|path| path.parent()) {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let find = |name: &str| find_file(name, &input, bcf.parent().unwrap_or(Path::new(".")));
    let result = run(&Options { bcf: bcf.clone(), output: out, find_file: &find })?;
    if !quiet {
        print!("{}", result.log);
    }
    Ok(())
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
