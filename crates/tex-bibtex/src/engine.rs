//! Global BibTeX state and the top-level program flow (bibtex.web part 1:
//! read the `.aux` file(s), then read and execute the `.bst` file, whose
//! `READ` command reads the `.bib` file(s)).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use tex_kpse::fs::PathExt;
use tex_kpse::{Format, Kpse};

use crate::input::Scanner;
use crate::log::{History, Log};

/// An immutable BibTeX string (field values, literals, cite keys, ...).
pub type Str = Rc<[u8]>;

/// TeX Live's `ent_str_size` and `glob_str_size` (texmf.cnf); `entry.max$`
/// and `global.max$` start out with these values.
pub const ENT_STR_SIZE: usize = 500;
pub const GLOB_STR_SIZE: usize = 200_000;
/// `max_print_line` / `min_print_line` for `.bbl` line breaking.
pub const MAX_PRINT_LINE: usize = 79;
pub const MIN_PRINT_LINE: usize = 3;

pub type FnId = u32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Builtin {
    Eq,
    Gt,
    Lt,
    Plus,
    Minus,
    Concat,
    Gets,
    AddPeriod,
    CallType,
    ChangeCase,
    ChrToInt,
    Cite,
    Duplicate,
    Empty,
    FormatName,
    If,
    IntToChr,
    IntToStr,
    Missing,
    Newline,
    NumNames,
    Pop,
    Preamble,
    Purify,
    Quote,
    Skip,
    Stack,
    Substring,
    Swap,
    TextLength,
    TextPrefix,
    Top,
    Type,
    Warning,
    While,
    Width,
    Write,
}

pub const BUILTINS: &[(&str, Builtin)] = &[
    ("=", Builtin::Eq),
    (">", Builtin::Gt),
    ("<", Builtin::Lt),
    ("+", Builtin::Plus),
    ("-", Builtin::Minus),
    ("*", Builtin::Concat),
    (":=", Builtin::Gets),
    ("add.period$", Builtin::AddPeriod),
    ("call.type$", Builtin::CallType),
    ("change.case$", Builtin::ChangeCase),
    ("chr.to.int$", Builtin::ChrToInt),
    ("cite$", Builtin::Cite),
    ("duplicate$", Builtin::Duplicate),
    ("empty$", Builtin::Empty),
    ("format.name$", Builtin::FormatName),
    ("if$", Builtin::If),
    ("int.to.chr$", Builtin::IntToChr),
    ("int.to.str$", Builtin::IntToStr),
    ("missing$", Builtin::Missing),
    ("newline$", Builtin::Newline),
    ("num.names$", Builtin::NumNames),
    ("pop$", Builtin::Pop),
    ("preamble$", Builtin::Preamble),
    ("purify$", Builtin::Purify),
    ("quote$", Builtin::Quote),
    ("skip$", Builtin::Skip),
    ("stack$", Builtin::Stack),
    ("substring$", Builtin::Substring),
    ("swap$", Builtin::Swap),
    ("text.length$", Builtin::TextLength),
    ("text.prefix$", Builtin::TextPrefix),
    ("top$", Builtin::Top),
    ("type$", Builtin::Type),
    ("warning$", Builtin::Warning),
    ("while$", Builtin::While),
    ("width$", Builtin::Width),
    ("write$", Builtin::Write),
];

/// One element of a wizard-defined function body.
#[derive(Clone, Debug)]
pub enum Op {
    Call(FnId),
    Quote(FnId),
    Int(i32),
    Str(Str),
}

/// A `.bst` function class (`fn_type`) with its `fn_info`.
#[derive(Clone, Debug)]
pub enum FnKind {
    Builtin(Builtin),
    Wiz(Rc<[Op]>),
    Field(usize),
    IntEntry(usize),
    StrEntry(usize),
    IntGlobal(usize),
    StrGlobal(usize),
}

pub struct FnDef {
    pub name: Str,
    pub kind: FnKind,
}

/// A literal-stack element.
#[derive(Clone, Debug)]
pub enum Lit {
    Int(i32),
    Str(Str),
    Fn(FnId),
    /// a missing field; carries the field's function id (for messages)
    Missing(FnId),
    /// error recovery after popping an empty stack
    Empty,
}

/// `type_list` element.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EntryType {
    /// no database entry seen (yet)
    Empty,
    /// entry type not defined by the style
    Undefined,
    Type(FnId),
}

/// A fatal error: the program jumps to `close_up_shop`.
pub struct Fatal;

pub struct BibInput {
    /// the `\bibdata` argument, as printed in messages
    pub name: Vec<u8>,
    pub bytes: Vec<u8>,
    /// found in the embedded archive rather than on disk
    pub embedded: bool,
}

pub struct Engine {
    pub log: Log,
    pub min_crossrefs: u32,
    pub verbose: bool,
    pub top_aux_dir: PathBuf,
    kpse: Option<Kpse>,

    // .aux results
    pub bib_inputs: Vec<BibInput>,
    pub bib_seen: bool,
    pub bst_seen: bool,
    pub bst_name: Option<Vec<u8>>,
    pub bst_bytes: Vec<u8>,
    pub citation_seen: bool,
    pub all_entries: bool,
    pub all_marker: usize,

    // cite keys: exact spelling -> cite_list index (`ilk_info[cite_loc]`)
    // and lower-case form -> exact spelling (`ilk_info[lc_cite_loc]`)
    pub cite_list: Vec<Str>,
    pub cite_map: HashMap<Box<[u8]>, usize>,
    pub lc_cite_map: HashMap<Box<[u8]>, Str>,
    pub num_cites: usize,
    pub old_num_cites: usize,

    // .bst function table
    pub fns: Vec<FnDef>,
    pub fn_map: HashMap<Box<[u8]>, FnId>,
    pub macros: HashMap<Box<[u8]>, Str>,
    pub num_fields: usize,
    pub num_ent_ints: usize,
    pub num_ent_strs: usize,
    pub glb_ints: Vec<i32>,
    pub glb_strs: Vec<Str>,
    pub crossref_num: usize,
    pub sort_key_num: usize,
    pub b_default: FnId,
    pub impl_fn_num: u32,
    pub entry_seen: bool,
    pub read_seen: bool,
    pub read_performed: bool,
    pub reading_completed: bool,
    pub bst_line: usize,

    // database
    pub type_list: Vec<EntryType>,
    pub entry_exists: Vec<bool>,
    pub cite_info: Vec<Option<Str>>,
    pub xref_count: Vec<u32>,
    pub field_info: Vec<Option<Str>>,
    pub preambles: Vec<Str>,
    pub bib_name: Vec<u8>,
    pub bib_line: usize,

    // execution
    pub entry_ints: Vec<i32>,
    pub entry_strs: Vec<Str>,
    pub sorted_cites: Vec<usize>,
    pub stack: Vec<Lit>,
    pub mess_with_entries: bool,
    pub cite_ptr: usize,
    pub out_buf: Vec<u8>,
    pub bbl: Vec<u8>,
    pub null_str: Str,
    /// bibtex.web's global `brace_level`, shared by several built-ins
    pub brace_level: i32,
}

impl Engine {
    fn new(min_crossrefs: u32, verbose: bool, top_aux_dir: PathBuf) -> Self {
        let null_str: Str = Rc::from(&b""[..]);
        let mut e = Engine {
            log: Log::new(),
            min_crossrefs,
            verbose,
            top_aux_dir,
            kpse: None,
            bib_inputs: Vec::new(),
            bib_seen: false,
            bst_seen: false,
            bst_name: None,
            bst_bytes: Vec::new(),
            citation_seen: false,
            all_entries: false,
            all_marker: 0,
            cite_list: Vec::new(),
            cite_map: HashMap::new(),
            lc_cite_map: HashMap::new(),
            num_cites: 0,
            old_num_cites: 0,
            fns: Vec::new(),
            fn_map: HashMap::new(),
            macros: HashMap::new(),
            num_fields: 0,
            num_ent_ints: 0,
            num_ent_strs: 0,
            glb_ints: Vec::new(),
            glb_strs: Vec::new(),
            crossref_num: 0,
            sort_key_num: 0,
            b_default: 0,
            impl_fn_num: 0,
            entry_seen: false,
            read_seen: false,
            read_performed: false,
            reading_completed: false,
            bst_line: 0,
            type_list: Vec::new(),
            entry_exists: Vec::new(),
            cite_info: Vec::new(),
            xref_count: Vec::new(),
            field_info: Vec::new(),
            preambles: Vec::new(),
            bib_name: Vec::new(),
            bib_line: 0,
            entry_ints: Vec::new(),
            entry_strs: Vec::new(),
            sorted_cites: Vec::new(),
            stack: Vec::new(),
            mess_with_entries: false,
            cite_ptr: 0,
            out_buf: Vec::new(),
            bbl: Vec::new(),
            null_str,
            brace_level: 0,
        };
        for &(name, b) in BUILTINS {
            e.define(name.as_bytes(), FnKind::Builtin(b));
        }
        e.b_default = e.fn_map[&b"skip$"[..]];
        // pre-defined field, entry string and integer globals
        e.crossref_num = e.num_fields;
        e.define(b"crossref", FnKind::Field(e.num_fields));
        e.num_fields += 1;
        e.sort_key_num = e.num_ent_strs;
        e.define(b"sort.key$", FnKind::StrEntry(e.num_ent_strs));
        e.num_ent_strs += 1;
        e.glb_ints.push(ENT_STR_SIZE as i32);
        e.define(b"entry.max$", FnKind::IntGlobal(0));
        e.glb_ints.push(GLOB_STR_SIZE as i32);
        e.define(b"global.max$", FnKind::IntGlobal(1));
        e
    }

    /// Insert a new `.bst` function name (already lower-cased).
    pub fn define(&mut self, name: &[u8], kind: FnKind) -> FnId {
        let id = self.fns.len() as FnId;
        self.fns.push(FnDef {
            name: Rc::from(name),
            kind,
        });
        self.fn_map.insert(name.into(), id);
        id
    }

    pub fn lookup_fn(&self, name: &[u8]) -> Option<FnId> {
        self.fn_map.get(name).copied()
    }

    /// `print_fn_class`.
    pub fn fn_class_name(&self, id: FnId) -> &'static str {
        match self.fns[id as usize].kind {
            FnKind::Builtin(_) => "built-in",
            FnKind::Wiz(_) => "wizard-defined",
            FnKind::Field(_) => "field",
            FnKind::IntEntry(_) => "integer-entry-variable",
            FnKind::StrEntry(_) => "string-entry-variable",
            FnKind::IntGlobal(_) => "integer-global-variable",
            FnKind::StrGlobal(_) => "string-global-variable",
        }
    }

    pub fn bst_file_name(&self) -> Vec<u8> {
        let mut n = self.bst_name.clone().unwrap_or_default();
        n.extend_from_slice(b".bst");
        n
    }

    /// `print_bib_name`: the `\bibdata` argument plus `.bib` unless present.
    pub fn bib_file_name(name: &[u8]) -> Vec<u8> {
        let mut n = name.to_vec();
        if !name.ends_with(b".bib") {
            n.extend_from_slice(b".bib");
        }
        n
    }

    // ------------------------------------------------------------------
    // file lookup
    // ------------------------------------------------------------------

    /// Find a `.bst`/`.bib` file the way `kpse_find_file` would for
    /// BibTeX: next to the top-level `.aux` file, then the kpathsea
    /// search path, then the embedded package archive. The flag is set for
    /// an embedded file, which messages announce as `<embedded:NAME>`.
    pub fn open_input(&mut self, name: &[u8], fmt: Format) -> Option<(Vec<u8>, bool)> {
        let name = std::str::from_utf8(name).ok()?;
        let ext = fmt.extensions()[0];
        let filename = if name.ends_with(ext) {
            name.to_string()
        } else {
            format!("{name}{ext}")
        };
        let local = self.top_aux_dir.join(&filename);
        if local.tex_is_file() {
            return tex_kpse::fs::read(&local).ok().map(|b| (b, false));
        }
        // an ls-R hit must be a regular file: a bare name can match a
        // directory (e.g. `plain` under the makeindex tree)
        let kpse = self.kpse.get_or_insert_with(Kpse::new);
        if let Some(hit) = kpse.find(&filename, fmt).filter(|p| p.tex_is_file()) {
            return tex_kpse::fs::read(&hit).ok().map(|b| (b, false));
        }
        tex_kpse::get_embedded_package(&filename).map(|b| (b, true))
    }

    /// How an opened input is announced in the `.blg`: `file_name`, or
    /// `<embedded:file_name>` for a file from the embedded archive.
    pub fn announced_name(file_name: Vec<u8>, embedded: bool) -> Vec<u8> {
        if !embedded {
            return file_name;
        }
        let mut label = b"<embedded:".to_vec();
        label.extend_from_slice(&file_name);
        label.push(b'>');
        label
    }

    /// Resolve an `\@input` file name: relative to the top-level `.aux`
    /// file's directory first (where LaTeX writes it), then as given.
    pub fn open_aux(&self, name: &[u8]) -> Option<Vec<u8>> {
        let name = std::str::from_utf8(name).ok()?;
        let p = Path::new(name);
        if p.is_relative() {
            let cand = self.top_aux_dir.join(p);
            if cand.tex_is_file() {
                return tex_kpse::fs::read(&cand).ok();
            }
        }
        if p.tex_is_file() {
            return tex_kpse::fs::read(p).ok();
        }
        None
    }

    // ------------------------------------------------------------------
    // shared error printing
    // ------------------------------------------------------------------

    /// `print_bad_input_line` (does the `mark_error`).
    pub fn print_bad_input_line(&mut self, sc: &Scanner) {
        let text = sc.bad_input_line();
        self.log.print(text);
        self.log.mark_error();
    }

    /// `print_confusion`.
    pub fn confusion(&mut self, what: &str) -> Fatal {
        self.log.println(what.to_string() + "---this can't happen");
        self.log.println("*Please notify the BibTeX maintainer*");
        self.log.mark_fatal();
        Fatal
    }
}

fn usage() -> i32 {
    eprintln!("Usage: bibtex [OPTION]... AUXFILE[.aux]");
    eprintln!("  -min-crossrefs=NUMBER  include item after NUMBER cross-refs; default 2");
    eprintln!("  -terse                 do not print progress reports");
    1
}

/// Run BibTeX with command-line `args` (program name excluded).
///
/// Writes `<job>.bbl` and `<job>.blg` next to the `.aux` file and returns
/// TeX Live's exit status: 0 for a clean run or warnings only, 2 when error
/// messages were issued (the `.bbl` is still written), 3 after a fatal
/// error, and 1 when the `.aux` file can't be opened or the command line
/// is unusable.
pub fn run(args: &[String], version: &str) -> i32 {
    let mut aux_arg: Option<&str> = None;
    let mut min_crossrefs = 2u32;
    let mut verbose = true;
    for a in args {
        let opt = a.strip_prefix("--").or_else(|| a.strip_prefix('-'));
        match opt {
            Some("terse") => verbose = false,
            Some(o) if o.starts_with("min-crossrefs=") => {
                match o["min-crossrefs=".len()..].parse() {
                    Ok(n) => min_crossrefs = n,
                    Err(_) => {
                        eprintln!("bibtex: bad -min-crossrefs value `{a}'");
                        return 1;
                    }
                }
            }
            Some(_) => eprintln!("bibtex: ignoring unknown option {a}"),
            None if aux_arg.is_none() => aux_arg = Some(a),
            None => {
                eprintln!("bibtex: Need exactly one file argument.");
                return usage();
            }
        }
    }
    let Some(aux_arg) = aux_arg else {
        eprintln!("bibtex: Need exactly one file argument.");
        return usage();
    };
    let base = aux_arg.strip_suffix(".aux").unwrap_or(aux_arg);
    let aux_name = format!("{base}.aux");
    let Ok(aux_bytes) = tex_kpse::fs::read(&aux_name) else {
        print_terminal(format!("I couldn't open file name `{aux_name}'\n").as_bytes());
        return 1;
    };
    let top_dir = Path::new(&aux_name)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();

    let mut e = Engine::new(min_crossrefs, verbose, top_dir);
    let banner = format!("This is BibTeX, Version 0.99e (Ratex {version})\n");
    if verbose {
        e.log.print(banner);
        e.log.print(format!("The top-level auxiliary file: {aux_name}\n"));
    } else {
        e.log.log_only(banner);
        e.log.log_only(format!("The top-level auxiliary file: {aux_name}\n"));
    }
    let _ = e.run_job(aux_name.as_bytes(), aux_bytes);
    if e.read_performed && !e.reading_completed {
        let name = Engine::bib_file_name(&e.bib_name);
        let msg = format!("Aborted at line {} of file ", e.bib_line);
        e.log.print(msg);
        e.log.println(name);
    }
    if tex_kpse::fs::write(format!("{base}.bbl"), &e.bbl).is_err() {
        e.log.println(format!("I couldn't open file name `{base}.bbl'"));
        e.log.mark_fatal();
    }
    e.log.print_history();
    let _ = tex_kpse::fs::write(format!("{base}.blg"), &e.log.blg);
    print_terminal(&e.log.term);
    match e.log.history {
        History::Spotless | History::Warning => 0,
        History::Error => 2,
        History::Fatal => 3,
    }
}

fn print_terminal(bytes: &[u8]) {
    if !tex_kpse::fs::is_memory() {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(bytes);
        let _ = out.flush();
    }
}

impl Engine {
    fn run_job(&mut self, aux_name: &[u8], aux_bytes: Vec<u8>) -> Result<(), Fatal> {
        self.read_aux(aux_name, aux_bytes)?;
        if self.bst_name.is_some() {
            self.read_bst()?;
        }
        // like bibtex.web, a partial line left in `out_buf` is never written
        Ok(())
    }
}
