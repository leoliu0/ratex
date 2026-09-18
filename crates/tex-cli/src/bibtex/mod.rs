#![allow(dead_code, unused_assignments)]

//! tex-bibtex: a BibTeX engine in Rust.
//!
//! Pipeline: parse .aux (\\citation, \\bibstyle, \\bibdata, \\bibcite) →
//! parse .bst → parse .bib databases → resolve crossrefs → execute the
//! style program → write jobname.bbl and jobname.blg.

mod bib;
mod bst;
mod exec;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tex_kpse::{Format, Kpse};

use bib::Database;
use exec::{Interp, RtEntry, ENT_STR_SIZE};

// ---------------------------------------------------------------------------
// logging (.blg)
// ---------------------------------------------------------------------------

pub struct Logger {
    lines: Vec<String>,
    pub errors: usize,
    pub warnings: usize,
}

impl Logger {
    fn new() -> Self {
        Logger {
            lines: Vec::new(),
            errors: 0,
            warnings: 0,
        }
    }
    fn info(&mut self, msg: impl AsRef<str>) {
        self.lines.push(msg.as_ref().to_string());
    }
    fn warn(&mut self, msg: impl AsRef<str>) {
        self.warnings += 1;
        self.lines.push(format!("Warning--{}", msg.as_ref()));
    }
    fn error(&mut self, msg: impl AsRef<str>) {
        self.errors += 1;
        self.lines.push(format!("error: {}", msg.as_ref()));
    }
}

// ---------------------------------------------------------------------------
// .aux parsing
// ---------------------------------------------------------------------------

struct Aux {
    cites: Vec<String>,
    style: Option<String>,
    bib_files: Vec<String>,
    all_entries: bool,
}

fn aux_arg(src: &str, i: &mut usize) -> Option<String> {
    let b = src.as_bytes();
    while *i < b.len() && b[*i].is_ascii_whitespace() {
        *i += 1;
    }
    if *i >= b.len() || b[*i] != b'{' {
        return None;
    }
    *i += 1;
    let start = *i;
    while *i < b.len() && b[*i] != b'}' {
        *i += 1;
    }
    let s = src[start..*i].to_string();
    if *i < b.len() {
        *i += 1;
    }
    Some(s)
}

fn parse_aux_file(path: &Path, aux: &mut Aux, depth: usize) -> Result<(), String> {
    if depth > 16 {
        return Err(format!("\\@input nesting too deep at {}", path.display()));
    }
    let src = std::fs::read_to_string(path)
        .map_err(|e| format!("couldn't open aux file {}: {e}", path.display()))?;
    let b = src.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] != b'\\' {
            i += 1;
            continue;
        }
        // command name: letters and @
        let start = i + 1;
        let mut j = start;
        while j < b.len() && (b[j].is_ascii_alphabetic() || b[j] == b'@') {
            j += 1;
        }
        let name = &src[start..j];
        let mut rest = j;
        match name {
            "citation" => {
                if let Some(arg) = aux_arg(&src, &mut rest) {
                    for key in arg.split(',') {
                        let key = key.trim();
                        if key.is_empty() {
                            continue;
                        }
                        if key == "*" {
                            aux.all_entries = true;
                        } else if !aux.cites.iter().any(|k| k == key) {
                            aux.cites.push(key.to_string());
                        }
                    }
                }
                i = rest;
            }
            "bibdata" => {
                if !aux.bib_files.is_empty() {
                    return Err("illegal, another \\bibdata command".into());
                }
                if let Some(arg) = aux_arg(&src, &mut rest) {
                    for f in arg.split(',') {
                        let f = f.trim().strip_suffix(".bib").unwrap_or(f.trim());
                        if !f.is_empty() {
                            aux.bib_files.push(f.to_string());
                        }
                    }
                }
                i = rest;
            }
            "bibstyle" => {
                if aux.style.is_some() {
                    return Err("illegal, another \\bibstyle command".into());
                }
                aux.style = aux_arg(&src, &mut rest);
                i = rest;
            }
            "@input" => {
                if let Some(arg) = aux_arg(&src, &mut rest) {
                    let nested = resolve_input(path, &arg);
                    parse_aux_file(&nested, aux, depth + 1)?;
                }
                i = rest;
            }
            _ => {
                // \bibcite, \relax, \@writefile, ...: skip this line
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
        }
        i = rest;
    }
    Ok(())
}

fn resolve_input(aux_path: &Path, name: &str) -> PathBuf {
    let p = Path::new(name);
    if p.is_absolute() && p.exists() {
        return p.to_path_buf();
    }
    if let Some(dir) = aux_path.parent() {
        let cand = dir.join(p);
        if cand.exists() {
            return cand;
        }
    }
    if p.exists() {
        p.to_path_buf()
    } else {
        let with_ext = if name.ends_with(".aux") {
            name.to_string()
        } else {
            format!("{name}.aux")
        };
        if let Some(dir) = aux_path.parent() {
            let cand = dir.join(&with_ext);
            if cand.exists() {
                return cand;
            }
        }
        PathBuf::from(with_ext)
    }
}

// ---------------------------------------------------------------------------
// file resolution
// ---------------------------------------------------------------------------

struct ResolvedText {
    label: String,
    source: String,
}

fn read_text_file(path: PathBuf) -> Result<ResolvedText, String> {
    let label = path.display().to_string();
    std::fs::read_to_string(&path)
        .map(|source| ResolvedText { label, source })
        .map_err(|error| format!("couldn't read {}: {error}", path.display()))
}

fn resolve_text_with_kpse(
    name: &str,
    aux_dir: Option<&Path>,
    fmt: Format,
) -> Result<Option<ResolvedText>, String> {
    let ext = fmt.extensions()[0];
    let bare = name.strip_suffix(ext).unwrap_or(name);
    let filename = format!("{bare}{ext}");
    if let Some(dir) = aux_dir {
        let cand = dir.join(&filename);
        if cand.is_file() {
            return read_text_file(cand).map(Some);
        }
    }
    // ls-R hit must be a regular file: a bare name can match a directory
    // (e.g. `plain` under the makeindex tree)
    let kpse = Kpse::new();
    if let Some(hit) = kpse.find(&filename, fmt) {
        if hit.is_file() {
            return read_text_file(hit).map(Some);
        }
    }
    match kpse.find(name, fmt) {
        Some(hit) if hit.is_file() => return read_text_file(hit).map(Some),
        _ => {}
    }
    let Some(bytes) = tex_kpse::get_embedded_package(&filename) else {
        return Ok(None);
    };
    let label = format!("<embedded:{filename}>");
    match String::from_utf8(bytes) {
        Ok(source) => Ok(Some(ResolvedText { label, source })),
        Err(_) => Err(format!("couldn't read {label}: input is not valid UTF-8")),
    }
}

fn missing_input_message(name: &str, fmt: Format) -> String {
    let ext = fmt.extensions()[0];
    let bare = name.strip_suffix(ext).unwrap_or(name);
    match fmt {
        Format::Bst => format!("I couldn't open style file {bare}.bst"),
        Format::Bib => format!("I couldn't open database file {bare}.bib"),
        _ => format!("I couldn't open input file {name}"),
    }
}

// ---------------------------------------------------------------------------
// entry
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// top-level run
// ---------------------------------------------------------------------------

pub fn run(args: &[String], version: &str) -> i32 {
    let mut aux_name: Option<String> = None;
    let mut min_crossrefs: usize = 2;
    let mut _terse = false;
    for a in args {
        match a.as_str() {
            "-terse" | "--terse" => _terse = true,
            _ => {
                if let Some(v) = a.strip_prefix("-min-crossrefs=") {
                    min_crossrefs = v.parse().unwrap_or(2);
                } else if a.starts_with('-') {
                    eprintln!("tex-bibtex: ignoring unknown option {a}");
                } else if aux_name.is_none() {
                    aux_name = Some(a.clone());
                }
            }
        }
    }
    let Some(aux_arg) = aux_name else {
        eprintln!("Usage: tex-bibtex [options] auxfile [flags]");
        eprintln!("  reads \\citation, \\bibstyle, \\bibdata from the .aux file;");
        eprintln!("  writes a .bbl and a .blg next to it.");
        return 2;
    };
    // accept "dir/jobname", "dir/jobname.aux", "jobname.aux"
    let path = PathBuf::from(&aux_arg);
    let aux_path = if path.extension().is_none() {
        path.with_extension("aux")
    } else {
        path
    };
    let aux_dir = aux_path.parent().map(|p| p.to_path_buf());

    let mut log = Logger::new();
    log.info(format!("This is BibTeX, Version {version}"));
    log.info(format!(
        "The top-level auxiliary file: {}",
        aux_path.display()
    ));

    if !aux_path.exists() {
        let msg = format!("I couldn't open auxiliary file {}", aux_path.display());
        eprintln!("tex-bibtex: {msg}");
        log.error(&msg);
        write_blg(&aux_path, &log);
        return 2;
    }

    let mut aux = Aux {
        cites: Vec::new(),
        style: None,
        bib_files: Vec::new(),
        all_entries: false,
    };
    if let Err(e) = parse_aux_file(&aux_path, &mut aux, 0) {
        eprintln!("tex-bibtex: {e}");
        log.error(&e);
        write_blg(&aux_path, &log);
        return 2;
    }

    let Some(style_name) = aux.style.clone() else {
        let msg = "I found no \\bibstyle command".to_string();
        eprintln!("tex-bibtex: {msg}");
        log.error(&msg);
        write_blg(&aux_path, &log);
        return 2;
    };
    if aux.bib_files.is_empty() {
        let msg = "I found no \\bibdata command".to_string();
        eprintln!("tex-bibtex: {msg}");
        log.error(&msg);
        write_blg(&aux_path, &log);
        return 2;
    }

    // --- locate and parse the .bst file --------------------------------
    let bst_input = match resolve_text_with_kpse(&style_name, aux_dir.as_deref(), Format::Bst) {
        Ok(Some(input)) => input,
        Ok(None) => {
            let msg = missing_input_message(&style_name, Format::Bst);
            eprintln!("tex-bibtex: {msg}");
            log.error(&msg);
            write_blg(&aux_path, &log);
            return 2;
        }
        Err(e) => {
            eprintln!("tex-bibtex: {e}");
            log.error(&e);
            write_blg(&aux_path, &log);
            return 2;
        }
    };
    log.info(format!("The style file: {}", bst_input.label));
    let program = match bst::parse_bst(&bst_input.source) {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("{}: {e}", bst_input.label);
            eprintln!("tex-bibtex: {msg}");
            log.error(&msg);
            write_blg(&aux_path, &log);
            return 2;
        }
    };

    // --- locate and parse the .bib files -------------------------------
    let mut db = Database::default();
    // Pre-populate string macros from the .bst (month abbreviations, journal
    // names, ...) so unquoted names like `jan` in .bib values resolve during
    // tokenization, before READ re-inserts them into the interpreter.
    for cmd in &program.cmds {
        if let bst::BstCmd::Macro(name, val) = cmd {
            db.macros.insert(name.to_ascii_lowercase(), val.clone());
        }
    }
    for (n, bf) in aux.bib_files.iter().enumerate() {
        match resolve_text_with_kpse(bf, aux_dir.as_deref(), Format::Bib) {
            Ok(Some(input)) => {
                log.info(format!("Database file #{}: {}", n + 1, input.label));
                bib::parse_bib(&input.source, bf, &mut db, &mut log);
            }
            Ok(None) => {
                let msg = missing_input_message(bf, Format::Bib);
                eprintln!("tex-bibtex: {msg}");
                log.error(&msg);
                write_blg(&aux_path, &log);
                return 2;
            }
            Err(error) => {
                eprintln!("tex-bibtex: {error}");
                log.error(&error);
                write_blg(&aux_path, &log);
                return 2;
            }
        }
    }

    // --- run the .bst program ------------------------------------------
    let mut interp = Interp::new(&mut log);
    for pre in &db.preamble {
        interp.preamble(pre);
    }
    let mut read_done = false; // ENTRY/READ bookkeeping
    for cmd in &program.cmds {
        if let Err(e) = run_cmd(
            cmd,
            &aux,
            &mut db,
            &mut interp,
            &mut read_done,
            min_crossrefs,
        ) {
            let msg = e.to_string();
            eprintln!("tex-bibtex: {msg}");
            interp.log.error(&msg);
            break;
        }
    }
    // --- outputs --------------------------------------------------------
    let bbl = interp.take_output();
    let mut bbl_path = aux_path.clone();
    bbl_path.set_extension("bbl");
    if let Err(e) = std::fs::write(&bbl_path, &bbl) {
        eprintln!("tex-bibtex: couldn't write {}: {e}", bbl_path.display());
        return 2;
    }
    write_blg(&aux_path, interp.log);

    if interp.log.errors > 0 {
        1
    } else {
        0
    }
}

fn write_blg(aux_path: &Path, log: &Logger) {
    let mut p = aux_path.to_path_buf();
    p.set_extension("blg");
    let mut out = log.lines.join("\n");
    out.push('\n');
    if log.warnings > 0 || log.errors > 0 {
        out.push_str(&format!(
            "\n(There were {} error messages and {} warning messages)\n",
            log.errors, log.warnings
        ));
    }
    let _ = std::fs::write(p, out);
}

fn run_cmd(
    cmd: &bst::BstCmd,
    aux: &Aux,
    db: &mut Database,
    interp: &mut Interp,
    read_done: &mut bool,
    min_crossrefs: usize,
) -> Result<(), String> {
    use bst::BstCmd;
    match cmd {
        BstCmd::Entry { fields, ints, strs } => {
            interp.declare_fields(fields);
            interp.declare_entry_ints(ints);
            interp.declare_entry_strs(strs);
        }
        BstCmd::Function(name, body) => interp.define_function(name, body.clone()),
        BstCmd::Integers(names) => interp.declare_ints(names),
        BstCmd::Strings(names) => interp.declare_strs(names),
        BstCmd::Macro(name, val) => {
            db.macros.insert(name.to_ascii_lowercase(), val.clone());
        }
        BstCmd::Read => {
            if *read_done {
                return Err("READ executed twice".into());
            }
            *read_done = true;
            do_read(aux, db, interp, min_crossrefs)?;
        }
        BstCmd::Sort => interp.cmd_sort(),
        BstCmd::Execute(name) => interp.cmd_execute(name)?,
        BstCmd::Iterate(name) => interp.cmd_iterate(name)?,
        BstCmd::Reverse(name) => interp.cmd_reverse(name)?,
    }
    Ok(())
}

/// The READ command (bibtex.web's database reading model):
///
/// 1. aux citations seed the cite list (in order, case-insensitively deduped).
/// 2. .bib entries are read sequentially; an entry is read only if its key is
///    already on the cite list. Storing a child's crossref field adds the
///    parent to the cite list (counted), so parents defined *after* their
///    children are picked up; parents defined before are never read.
/// 3. Children inherit missing fields from read parents and their crossref
///    value is canonicalized to the parent's spelling.
/// 4. Unread parents / bad crossrefs produce errors and the crossref field is
///    cleared; crossref-added parents are kept only at >= min_crossrefs.
fn do_read(
    aux: &Aux,
    db: &Database,
    interp: &mut Interp,
    min_crossrefs: usize,
) -> Result<(), String> {
    #[derive(Clone)]
    struct Cite {
        key: String, // spelling as it will appear in cite$ / canonical crossref
        lc: String,
        aux_cited: bool,
        crossref_added: bool,
        count: usize,
        read: Option<usize>, // db index once read
    }
    let mut cite_list: Vec<Cite> = Vec::new();
    let mut pos_of: HashMap<String, usize> = HashMap::new(); // lc -> cite_list idx

    let add_cite = |cite_list: &mut Vec<Cite>,
                    pos_of: &mut HashMap<String, usize>,
                    key: &str,
                    aux_cited: bool|
     -> Option<usize> {
        let lc = key.to_ascii_lowercase();
        if let Some(&i) = pos_of.get(&lc) {
            return Some(i);
        }
        let i = cite_list.len();
        cite_list.push(Cite {
            key: key.to_string(),
            lc,
            aux_cited,
            crossref_added: !aux_cited,
            count: if aux_cited { 0 } else { 1 },
            read: None,
        });
        pos_of.insert(cite_list[i].lc.clone(), i);
        None::<usize>
    };

    for key in &aux.cites {
        add_cite(&mut cite_list, &mut pos_of, key, true);
    }
    if aux.all_entries {
        for e in &db.entries {
            add_cite(&mut cite_list, &mut pos_of, &e.cite, true);
        }
    }
    let old_num_cites = cite_list.len();

    // sequential read
    for (idx, entry) in db.entries.iter().enumerate() {
        let lc = entry.cite.to_ascii_lowercase();
        let Some(&i) = pos_of.get(&lc) else { continue };
        cite_list[i].read = Some(idx);
        // storing the crossref field registers the parent
        if let Some((_, parent)) = entry.fields.iter().find(|(n, _)| n == "crossref") {
            let plc = parent.to_ascii_lowercase();
            match pos_of.get(&plc).copied() {
                Some(pi) => {
                    let c = &mut cite_list[pi];
                    if c.crossref_added && pi >= old_num_cites {
                        c.count += 1;
                    }
                }
                None => {
                    if !aux.all_entries {
                        add_cite(&mut cite_list, &mut pos_of, parent, false);
                    }
                }
            }
        }
    }

    // crossref resolution: inheritance + canonicalization; bad refs error out
    for i in 0..cite_list.len() {
        let Some(db_idx) = cite_list[i].read else {
            continue;
        };
        let child = &db.entries[db_idx];
        let Some((_, parent_val)) = child.fields.iter().find(|(n, _)| n == "crossref") else {
            continue;
        };
        let plc = parent_val.to_ascii_lowercase();
        let parent_i = pos_of.get(&plc).copied();
        let Some(pi) = parent_i else {
            interp.log.error(format!(
                "a bad cross reference---entry \"{}\"\nrefers to entry \"{parent_val}\", which doesn't exist",
                child.cite
            ));
            interp.clear_crossref(db_idx);
            continue;
        };
        if cite_list[pi].read.is_none() {
            interp.log.error(format!(
                "a bad cross reference---entry \"{}\"\nrefers to entry \"{parent_val}\", which doesn't exist",
                child.cite
            ));
            interp.clear_crossref(db_idx);
            continue;
        }
        let parent_db = cite_list[pi].read.unwrap();
        interp.canonicalize_crossref(db_idx, &cite_list[pi].key);
        // inherit missing fields (everything except crossref itself)
        for (name, val) in &db.entries[parent_db].fields {
            if name == "crossref" {
                continue;
            }
            interp.inherit_field(db_idx, name, val);
        }
        if cite_list[pi].crossref_added && cite_list[pi].count < min_crossrefs {
            interp.clear_crossref(db_idx);
        }
    }

    // remove unread / under-crossed; warn for missing aux-cited keys
    let mut keep: Vec<usize> = Vec::new(); // cite_list positions
    for (i, c) in cite_list.iter().enumerate() {
        let keepable = aux.all_entries || c.aux_cited || c.count >= min_crossrefs;
        if !keepable {
            continue;
        }
        match c.read {
            None => interp
                .log
                .warn(format!("I didn't find a database entry for \"{}\"", c.key)),
            Some(_) => keep.push(i),
        }
    }

    // build runtime entries in final cite-list order
    let nf = interp.num_fields();
    let ni = interp.num_ent_ints();
    let ns = interp.num_ent_strs();
    let mut rt_entries: Vec<RtEntry> = Vec::with_capacity(keep.len());
    for &i in &keep {
        let db_idx = cite_list[i].read.unwrap();
        let e = &db.entries[db_idx];
        let mut fields: Vec<Option<String>> = vec![None; nf];
        for (name, val) in &e.fields {
            match interp.field_index(name) {
                // database fields are not subject to the entry-string cap
                Some(idx) if idx < nf => fields[idx] = Some(val.clone()),
                _ => {
                    if name != "crossref" {
                        interp.log.warn(format!(
                            "the style file ignores the field \"{name}\" (entry \"{}\")",
                            e.cite
                        ));
                    }
                }
            }
        }
        // apply inheritance recorded for this entry
        interp.apply_inherited(db_idx, &mut fields);
        rt_entries.push(RtEntry {
            cite: cite_list[i].key.clone(),
            type_name: e.type_name.clone(),
            fields,
            ent_ints: vec![0; ni],
            ent_strs: vec![String::new(); ns],
        });
    }
    interp.load_entries(rt_entries);
    Ok(())
}

fn trunc_entry(s: &str) -> String {
    if s.len() > ENT_STR_SIZE {
        let mut end = ENT_STR_SIZE;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        s[..end].to_string()
    } else {
        s.to_string()
    }
}
