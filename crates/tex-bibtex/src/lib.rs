#![allow(dead_code)]

//! tex-bibtex: a pure-Rust BibTeX replacement.
//!
//! Reproduces bibtex 0.99e .bbl output byte-for-byte for standard styles.

pub mod aux;
pub mod bib;
pub mod bst;
pub mod classes;
pub mod interp;
pub mod names;
pub mod outbuf;

use bib::BibParser;
use bst::BstCommand;
use interp::{Entry, Interp};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub struct RunOutcome {
    pub status: i32,
    pub bbl: String,
    pub blg: String,
    pub stdout: String,
}

/// Tunables matching the real `bibtex` command line.
#[derive(Debug, Clone)]
pub struct RunOpts {
    /// `-min-crossrefs=N`: cross-ref'd entries included this often or more.
    pub min_crossrefs: usize,
    /// `-terse`: suppress progress/warning chatter on stdout.
    pub terse: bool,
}

impl Default for RunOpts {
    fn default() -> Self {
        RunOpts {
            min_crossrefs: 2,
            terse: false,
        }
    }
}

fn search_file(name: &str, env_vars: &[&str], cwd: &Path) -> Option<PathBuf> {
    let direct = cwd.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    for env_var in env_vars {
        let Ok(paths) = std::env::var(env_var) else {
            continue;
        };
        for dir in paths.split(':') {
            if dir.is_empty() {
                continue;
            }
            // kpathsea's `//` suffix means "search this subtree recursively"
            if let Some(base) = dir.strip_suffix("//") {
                let base = if base.is_empty() { "/" } else { base };
                if let Some(hit) = walk_for(Path::new(base), name, 0) {
                    return Some(hit);
                }
                continue;
            }
            let cand = Path::new(dir).join(name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    let fmt = if name.ends_with(".bst") {
        tex_kpse::Format::Bst
    } else if name.ends_with(".bib") {
        tex_kpse::Format::Bib
    } else {
        return None;
    };
    tex_kpse::Kpse::new()
        .find(name, fmt)
        .filter(|p| p.is_file())
}

/// Bounded recursive walk used for kpathsea `//` path components.
fn walk_for(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    if depth > 12 {
        return None;
    }
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if let Some(hit) = walk_for(&e.path(), name, depth + 1) {
                return Some(hit);
            }
        }
    }
    None
}

/// Run bibtex for `jobname` (no extension) in `cwd`. Returns the outcome;
/// .bbl/.blg are written by the caller (or pass write_files=true).
pub fn run(jobname: &str, cwd: &Path, write_files: bool) -> RunOutcome {
    run_opts(jobname, cwd, write_files, &RunOpts::default())
}

/// Run bibtex with explicit options.
pub fn run_opts(jobname: &str, cwd: &Path, write_files: bool, opts: &RunOpts) -> RunOutcome {
    let mut blg: Vec<String> = Vec::new();
    let mut status = 0;

    let aux_path = cwd.join(format!("{}.aux", jobname));
    let aux_text = match std::fs::read_to_string(&aux_path) {
        Ok(t) => t,
        Err(_) => {
            let msg = format!("I couldn't open file name `{}`", aux_path.display());
            blg.push(msg.clone());
            return finish(1, String::new(), blg, jobname, cwd, write_files, opts);
        }
    };
    let aux_name = aux_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    blg.push("The top-level auxiliary file: ".to_string() + &aux_name);
    let a = aux::parse_aux(&aux_text);

    // style file
    let style_name = a.bibstyle.clone().unwrap_or_default();
    let bst_text = search_file(
        &format!("{}.bst", style_name),
        &["TEXBSTINPUTS", "BSTINPUTS"],
        cwd,
    )
    .and_then(|p| std::fs::read_to_string(p).ok());
    let Some(bst_text) = bst_text else {
        blg.push(format!("I couldn't open style file {}.bst", style_name));
        return finish(2, String::new(), blg, jobname, cwd, write_files, opts);
    };
    blg.push(format!("The style file: {}.bst", style_name));

    let cmds = bst::BstParser::new(&bst_text).parse();

    let mut it = Interp::new();
    // `crossref` is pre-defined as a field in bibtex
    it.fields.insert("crossref".into());

    // bookkeeping for READ
    let mut read_done = false;

    let result = (|| -> Result<(), String> {
        for cmd in &cmds {
            match cmd {
                BstCommand::Entry { fields, ints, strs } => {
                    for f in fields {
                        it.fields.insert(f.clone());
                    }
                    for v in ints {
                        it.entry_int_vars.insert(v.clone());
                    }
                    for v in strs {
                        it.entry_str_vars.insert(v.clone());
                    }
                    it.entry_str_vars.insert("sort.key$".into());
                }
                BstCommand::Integers(names) => {
                    for n in names {
                        it.globals_i.entry(n.clone()).or_insert(0);
                    }
                }
                BstCommand::Strings(names) => {
                    for n in names {
                        it.globals_s.entry(n.clone()).or_default();
                    }
                }
                BstCommand::Function(name, body) => {
                    it.fns.insert(name.clone(), body.clone());
                }
                BstCommand::Macro(name, val) => {
                    it.macros.insert(name.clone(), val.clone());
                }
                BstCommand::Read => {
                    if read_done {
                        continue;
                    }
                    read_done = true;
                    read_database(&mut it, &a, cwd, opts)?;
                }
                BstCommand::Execute(name) => it.execute(name)?,
                BstCommand::Iterate(name) => it.iterate(name, false)?,
                BstCommand::Reverse(name) => it.iterate(name, true)?,
                BstCommand::Sort => it.sort_cites(),
            }
        }
        Ok(())
    })();

    blg.extend(it.blg.clone());
    match result {
        Ok(()) => {}
        Err(e) => {
            status = 2;
            blg.push(format!("(There was an error message: {})", e));
        }
    }
    // web2c bibtex exit codes: 0 spotless/warnings-only, 2 on errors.
    // (Warnings do not change the exit status; see bibtex.web's history.)
    if status == 0 && it.errors > 0 {
        status = 2;
    }
    if it.errors == 1 {
        blg.push("(There was 1 error message)".into());
    } else if it.errors > 1 {
        blg.push(format!("(There were {} error messages)", it.errors));
    } else if it.warnings == 1 {
        blg.push("(There was 1 warning message)".into());
    } else if it.warnings > 1 {
        blg.push(format!("(There were {} warning messages)", it.warnings));
    }

    let bbl = it.out.finish();
    finish(status, bbl, blg, jobname, cwd, write_files, opts)
}

/// One `READ`: load every database file, then resolve the citation list
/// exactly as bibtex.web does — aux citations first (original spelling,
/// case-insensitive matching), with `*` pulling in every database entry
/// and crossref'd targets appended when cited `min_crossrefs` times.
fn read_database(it: &mut Interp, a: &aux::Aux, cwd: &Path, opts: &RunOpts) -> Result<(), String> {
    // Parsed database files in order, with the macros visible to each.
    let mut files: Vec<(String, Vec<bib::RawEntry>)> = Vec::new();
    let mut preambles: Vec<String> = Vec::new();
    let mut macros: HashMap<String, String> = it.macros.clone();

    for name in &a.bibdata {
        let fname = if name.ends_with(".bib") {
            name.clone()
        } else {
            format!("{}.bib", name)
        };
        let Some(path) = search_file(&fname, &["TEXBIBINPUTS", "BIBINPUTS"], cwd) else {
            it.blg
                .push(format!("I couldn't open database file {}", fname));
            continue;
        };
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let bib_macros: HashMap<String, Vec<u8>> = macros
            .iter()
            .map(|(k, v)| (k.clone(), v.as_bytes().to_vec()))
            .collect();
        let parsed = BibParser::new(&text, bib_macros).parse();
        for s in &parsed.strings {
            macros.insert(s.0.clone(), String::from_utf8_lossy(&s.1).into_owned());
        }
        let db_name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| fname.clone());
        it.blg
            .push(format!("Database file #{}: {}", files.len() + 1, db_name));
        for w in &parsed.warnings {
            it.blg.push(w.clone());
        }
        files.push((db_name, parsed.entries));
        for p in &parsed.preamble_parts {
            preambles.push(String::from_utf8_lossy(p).into_owned());
        }
    }
    it.macros = macros;
    it.preamble = preambles.concat();

    // Citation list: aux cites first (deduped case-insensitively), then
    // database additions in read order. Keys match case-insensitively.
    let num_aux_cites = a.cites.len();
    let mut cites: Vec<String> = Vec::with_capacity(num_aux_cites);
    let mut seen_lc: HashMap<String, usize> = HashMap::new();
    for c in &a.cites {
        let lc = c.to_ascii_lowercase();
        if seen_lc.contains_key(&lc) {
            continue;
        }
        seen_lc.insert(lc, cites.len());
        cites.push(c.clone());
    }

    // Storage walk: an entry is stored only if its key is already on the
    // citation list when read (so crossref'd parents must come after the
    // entries citing them — bibtex.web's actual discipline).
    let mut stored_lc: HashMap<String, &bib::RawEntry> = HashMap::new();
    let mut xref_count: HashMap<usize, usize> = HashMap::new();
    for (db_name, entries) in &files {
        for ent in entries {
            let lc = ent.key.to_ascii_lowercase();
            if stored_lc.contains_key(&lc) {
                it.errors += 1;
                it.blg.push(format!(
                    "Repeated entry---line {} of file {}\nI'm skipping whatever remains of this entry",
                    ent.line, db_name
                ));
                continue;
            }
            if !a.cite_all && !seen_lc.contains_key(&lc) {
                continue; // uncited: scanned and discarded
            }
            if a.cite_all && !seen_lc.contains_key(&lc) {
                let p = cites.len();
                seen_lc.insert(lc.clone(), p);
                cites.push(ent.key.clone());
            }
            stored_lc.insert(lc.clone(), ent);
            // an entry that joined the list only via crossref takes its
            // database key as the official cite-list spelling
            if !a.cite_all {
                if let Some(&p) = seen_lc.get(&lc) {
                    if p >= num_aux_cites {
                        cites[p] = ent.key.clone();
                    }
                }
            }
            if !a.cite_all {
                if let Some(v) = ent.fields.iter().find(|(n, _)| n == "crossref") {
                    let target = String::from_utf8_lossy(&v.1).trim().to_string();
                    if !target.is_empty() {
                        let lct = target.to_ascii_lowercase();
                        match seen_lc.get(&lct).copied() {
                            Some(pos) if pos >= num_aux_cites => {
                                *xref_count.entry(pos).or_insert(0) += 1;
                            }
                            Some(_) => {} // aux-cited target: included anyway
                            None => {
                                seen_lc.insert(lct, cites.len());
                                xref_count.insert(cites.len(), 1);
                                cites.push(target);
                            }
                        }
                    }
                }
            }
        }
    }

    // Materialize per-cite entries (declared fields only; extra fields are
    // warned about at read time, exactly as bibtex.web stores them).
    let mut data: Vec<Option<Entry>> = cites
        .iter()
        .map(|c| {
            stored_lc.get(&c.to_ascii_lowercase()).map(|ent| {
                let mut e = Entry {
                    etype: ent.etype.clone(),
                    fields: HashMap::new(),
                    evars: HashMap::new(),
                    eints: HashMap::new(),
                };
                for (fname, fval) in &ent.fields {
                    if it.fields.contains(fname) || fname == "crossref" {
                        e.fields
                            .insert(fname.clone(), String::from_utf8_lossy(fval).into_owned());
                    }
                    // undeclared fields are dropped silently, as bibtex does
                }
                for fname in &ent.dup_fields {
                    if it.fields.contains(fname) {
                        it.warn(&format!("I'm ignoring {}'s extra \"{}\" field", c, fname));
                    }
                }
                e
            })
        })
        .collect();

    // Add cross-reference information: make each pointer carry the
    // parent's official cite-list spelling and inherit missing fields.
    let pos_of: HashMap<String, usize> = cites
        .iter()
        .enumerate()
        .map(|(i, c)| (c.to_ascii_lowercase(), i))
        .collect();
    for i in 0..cites.len() {
        let target = match data[i].as_ref().and_then(crossref_target) {
            Some(t) => t,
            None => continue,
        };
        let Some(&p) = pos_of.get(&target.to_ascii_lowercase()) else {
            continue;
        };
        if data[p].is_some() {
            // official spelling: the aux spelling for aux-cited parents,
            // otherwise the database key's own spelling
            let official = if p < num_aux_cites {
                cites[p].clone()
            } else {
                stored_lc
                    .get(&cites[p].to_ascii_lowercase())
                    .map(|raw| raw.key.clone())
                    .unwrap_or_else(|| cites[p].clone())
            };
            let parent_fields = data[p].as_ref().unwrap().fields.clone();
            let ent = data[i].as_mut().unwrap();
            ent.fields.insert("crossref".into(), official);
            // a missing field of the child takes the parent's value
            for (fname, fval) in &parent_fields {
                if fname == "crossref" {
                    continue;
                }
                ent.fields
                    .entry(fname.clone())
                    .or_insert_with(|| fval.clone());
            }
        }
    }

    // Subtract cross-reference information: report and drop pointers to
    // entries that did not make it; warn about nested pointers.
    let mut broken: HashSet<usize> = HashSet::new();
    for i in 0..cites.len() {
        let Some(ent) = data[i].as_ref() else {
            continue;
        };
        let Some(target) = crossref_target(ent) else {
            continue;
        };
        let tl = target.to_ascii_lowercase();
        match pos_of.get(&tl).copied() {
            None => {
                it.errors += 1;
                it.blg.push(format!(
                    "A bad cross reference---entry \"{}\"\nrefers to entry \"{}\", which doesn't exist",
                    cites[i], target
                ));
                broken.insert(i);
            }
            Some(p) if data[p].is_none() => {
                it.errors += 1;
                it.blg.push(format!(
                    "A bad cross reference---entry \"{}\"\nrefers to entry \"{}\", which doesn't exist",
                    cites[i], target
                ));
                broken.insert(i);
            }
            Some(p) => {
                if data[p]
                    .as_ref()
                    .and_then(|e| e.fields.get("crossref"))
                    .map(|v| !v.is_empty())
                    .unwrap_or(false)
                {
                    it.warn(&format!(
                        "you've nested cross references--entry \"{}\"\nrefers to entry \"{}\", which also refers to something",
                        cites[i], target
                    ));
                }
                if !a.cite_all
                    && p >= num_aux_cites
                    && xref_count.get(&p).copied().unwrap_or(0) < opts.min_crossrefs
                {
                    broken.insert(i);
                }
            }
        }
    }
    for &i in &broken {
        if let Some(ent) = data[i].as_mut() {
            ent.fields.remove("crossref");
        }
    }

    // Remove missing entries or those cross-referenced too few times.
    let mut kept: Vec<String> = Vec::new();
    for (pos, c) in cites.iter().enumerate() {
        let _lc = c.to_ascii_lowercase();
        let aux_cited = a.cite_all || pos < num_aux_cites;
        let exists = data[pos].is_some();
        if !exists {
            it.warn(&format!("I didn't find a database entry for \"{}\"", c));
            continue;
        }
        if !aux_cited && xref_count.get(&pos).copied().unwrap_or(0) < opts.min_crossrefs {
            continue;
        }
        kept.push(c.clone());
    }

    // Hand the survivors to the interpreter.
    it.cites = kept;
    for (i, c) in it.cites.iter().enumerate() {
        it.orig_idx.insert(c.clone(), i);
    }
    for (pos, c) in cites.iter().enumerate() {
        if let Some(e) = data[pos].take() {
            if it.orig_idx.contains_key(c) {
                it.entries.insert(c.clone(), e);
            }
        }
    }
    Ok(())
}

fn crossref_target(e: &Entry) -> Option<String> {
    e.fields
        .get("crossref")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn finish(
    status: i32,
    bbl: String,
    blg: Vec<String>,
    jobname: &str,
    cwd: &Path,
    write_files: bool,
    opts: &RunOpts,
) -> RunOutcome {
    let mut blg_text = String::new();
    blg_text.push_str("This is BibTeX, Version 0.99 (tex-bibtex, Rust)\n");
    let mut stdout = if opts.terse {
        String::new()
    } else {
        blg_text.clone()
    };
    for l in &blg {
        blg_text.push_str(l);
        blg_text.push('\n');
        // non-terse bibtex echoes progress, warnings and errors on stdout
        let is_progress = l.starts_with("The ")
            || l.starts_with("Database file #")
            || l.starts_with("Warning--")
            || l.starts_with("(There ")
            || l.starts_with("A bad cross reference");
        if !opts.terse && is_progress {
            stdout.push_str(l);
            stdout.push('\n');
        }
    }
    if write_files {
        let _ = std::fs::write(cwd.join(format!("{}.bbl", jobname)), &bbl);
        let _ = std::fs::write(cwd.join(format!("{}.blg", jobname)), &blg_text);
    }
    RunOutcome {
        status,
        bbl,
        blg: blg_text,
        stdout,
    }
}
