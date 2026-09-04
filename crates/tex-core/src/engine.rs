//! Engine state: control-sequence table, equivalents, input stack, modes,
//! lists, fonts, output files; plus primitive registration.

use crate::eqtb::{Equiv, Eqtb, LevelType};
use crate::input::InputStack;
use crate::prim::*;
use crate::token::{CsId, CsTable, Token};


#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScannerStatus {
    Normal,
    Skipping,
    Defining,
    Aligning,
    Absorbing, // \mark, \write text
}

#[derive(Clone, Debug)]
pub struct IfState {
    pub accepting: bool,   // currently taking the true branch
    pub matched: bool,     // some branch was taken already
    pub if_case: i32,      // >=0: \ifcase with this many cases left
    pub loc_file: String,
    pub loc_line: u32,
    pub loc_cs: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Vertical,
    InternalVertical,
    Horizontal,
    RestrictedHorizontal,
    Math,
    DisplayMath,
}
impl Mode {
    pub fn is_v(self) -> bool {
        matches!(self, Mode::Vertical | Mode::InternalVertical)
    }
    pub fn is_h(self) -> bool {
        matches!(self, Mode::Horizontal | Mode::RestrictedHorizontal)
    }
    pub fn is_m(self) -> bool {
        matches!(self, Mode::Math | Mode::DisplayMath)
    }
    pub fn is_inner(self) -> bool {
        matches!(self, Mode::InternalVertical | Mode::RestrictedHorizontal | Mode::Math)
    }
}

/// Interned ids for control sequences the engine itself references.
pub struct Ids {
    pub par: CsId,
    pub cs_escape: u8,
}

pub struct Engine {
    pub cs: CsTable,
    pub eqtb: Eqtb,
    pub input: InputStack,
    pub ids: Ids,

    // current file line scanning
    pub pending_retokenize: bool,

    // current token being processed
    pub cur_tok: Token,
    pub cur_cs: Option<CsId>,
    pub cur_prim: Option<Prim>,
    /// char code for register-prefixed primitives etc.
    pub cur_chr: i32,

    pub mode: Mode,
    pub mode_level: u16, // nesting of box modes
    pub cur_list: Vec<crate::boxes::Node>,
    pub prev_depth: i32,  // special marker: -1000pt means unset
    pub space_factor: i32,
    pub prev_graf: i32,
    pub after_token: bool,

    pub ini_mode: bool, // -ini: format-building mode
    pub format_name: String,
    pub job_name: String,
    pub halt_on_error: bool,
    pub error_count: i32,

    // write streams
    pub write_streams: Vec<Option<std::fs::File>>,
    pub writebuf: Vec<(u16, String)>, // pending closed-stream writes go to terminal if 16/17/18

    // hyphenation
    pub hyphen_trie: crate::hyphen::Trie,
    pub hyphen_exceptions: Vec<(String, Vec<u8>)>,
    pub par_shape: Vec<(i32, i32)>,
    /// group level of the current par_shape assignment (tex.web tracks
    /// par_shape_ptr's level through eq_define like any eqtb entry)
    pub par_shape_level: u16,

    // output
    pub pdf_doc: crate::pdfout::PdfDoc,
    pub out_file: Option<std::fs::File>,
    pub font_loader: crate::fontload::FontLoader,
    pub pdf_outlines: Vec<(String, String, i32)>,

    pub job_running: bool,
    pub end_occurred: bool,
    // scanning state
    pub if_stack: Vec<IfState>,
    pub pushed: Vec<Token>, // lookahead pushback
    /// fill order of the last scan_dimen unit (0=normal, 1=fil, 2=fill, 3=filll)
    pub cur_fill_order: u8,
    /// debug: last expanded macro names
    pub last_macros: std::collections::VecDeque<String>,
    /// debug: recent raw tokens
    pub tok_ring: std::collections::VecDeque<(u32, u32)>,
    /// delimiter text collected before the first # of a \def param text
    pub def_prefix: Vec<Token>,
    pub global_flag: bool,
    pub long_flag: bool,
    pub outer_flag: bool,
    pub protected_flag: bool,
    /// >0 while scan_int/scan_dimen: e-TeX expands protected macros when
    /// looking for a number (`\romannumeral\protected...`).
    pub expand_protected: u32,
    pub in_expanded_scan: bool,
    /// tokens from \\unexpanded still sitting on the input; e-scan must not
    /// ##-collapse them (\\usenone{#1}\\unexpanded{#1} inside \\expanded).
    pub unexp_protect: usize,
    /// e-TeX \\ifincsname: \\csname nesting depth
    pub csname_depth: u32,

    /// noexpand'd token pending (returned once, unexpanded)
    pub no_expand_tok: Option<Token>,
    pub align_state: i32, // & nesting balance for runaway detection
    pub ss_trace: Vec<String>,
    pub format_done: bool,
    pub trace_ltx: u32,
    /// \pdfpageattr / \pdfpagesattr dict bodies (global in pdfTeX)
    pub pdf_page_attr: String,
    pub pdf_pages_attr: String,
    pub left_delim: Option<i32>,
    pub right_delim: Option<i32>,
    pub math_limits: Option<u8>,
    pub last_delim: Option<i32>,
    pub pending_the_string: Option<String>,
    /// the main vertical list fed to the page builder (outer VM)
    pub page_list: Vec<crate::boxes::Node>,
    pub setbox_target: Option<u16>,
    /// box_kinds depth of the group that consumes `setbox_target` (tex.web
    /// keeps the box-register location group-local; inner boxes must not
    /// steal the pending \setbox target)
    pub setbox_depth: usize,
    /// tex.web holds assignment prefixes in prefixed_command locals: a
    /// `\global` before `\setbox<n>=\vbox{..}` must survive the box BODY
    /// (whose own assignments would otherwise consume it) and land on the
    /// register assignment when the body closes.
    pub setbox_global: bool,
    /// outer \\setbox/\\shipout targets parked across nested \\setbox
    pub setbox_stack: Vec<(Option<u16>, usize, bool)>,
    pub pending_box_shift: Option<(i32, bool)>,
    pub box_targets: Vec<Option<(i32, bool)>>,
    pub box_shifts: Vec<i32>,
    pub box_kinds: Vec<u8>,
    pub insert_nums: Vec<u16>,
    pub shipout_pending: bool,
    /// box_kinds depth of the \\shipout box (tex.web box_context);
    /// inner boxes must not consume the pending shipout.
    pub shipout_depth: usize,
    pub par_page_lists: Vec<Vec<crate::boxes::Node>>,
    pub read_eof: Vec<bool>, // (amount, is_hmove)
    pub read_files: Vec<Option<std::io::BufReader<std::fs::File>>>,
    pub out_dir: String,
    /// directory of the primary input file; relative \\input/\\openin names
    /// resolve here before falling back to the TDS (matches running TeX from
    /// the document's own directory).
    pub main_dir: Option<std::path::PathBuf>,
    pub job_ended_by_end: bool,
    pub align_preamble: Vec<crate::align::ColSpec>,
    pub align_rows: Vec<Vec<crate::align::Cell>>,
    pub align_col_widths: Vec<i32>,
    pub align_cur_row: Vec<crate::align::Cell>,
    pub align_cur_col: i32,
    pub align_scanning_cell: bool,
    /// `pushed` length when the current align toklist was installed.
    /// Expansions after that point outrank the toklist; older `pushed`
    /// tokens (e.g. a \\futurelet peek) wait until the toklist finishes.
    pub align_pushed_base: usize,
    pub align_noalign_save_base: usize,
    pub align_done: bool,
    pub align_to: Option<(i32, bool)>, // \halign to/spread <dimen>: (dimen, is_spread)
    pub in_output: bool,
    pub output_depth: usize,
    pub dead_cycles: i32,
    pub page_prev_depth: i32,
    pub page_total: i64,
    pub page_depth: i64,
    pub page_processed: usize,
    pub page_best_break: Option<usize>,
    pub page_break_penalty: i32,
    /// true cost of the carried best break (BreakSpot::carried used a
    /// synthetic DEPLORABLE, losing real cost across build_page calls)
    pub page_best_cost: i64,
    pub page_goal_set: bool,
    pub page_stretch: [i64; 4],
    pub page_shrink: [i64; 4],
    pub vsplat_remainder: Option<Vec<crate::boxes::Node>>,
    pub math_lists: Vec<Vec<crate::boxes::Node>>,
    /// tex.web mlist_penalties as a conversion-scope global: insert
    /// \binoppenalty/\relpenalty breakpoints after Bin/Rel atoms when
    /// converting inline TEXT math (mode>0); restored on exit
    pub math_penalties: std::cell::Cell<bool>,
    pub gt_steps: u64,
    pub rt_steps: u64,
    pub mac_depth: u32,
    pub current_macro: String,
    pub loop_traced: bool,
    pub math_style_stack: Vec<crate::boxes::MathStyle>,
    /// tex.web §1181 (init_math): \\predisplaysize, \\displaywidth and
    /// \\displayindent are computed at display entry from the final line of
    /// the interrupted paragraph and consumed by finish_display.
    pub pre_display_size: i64,
    pub pre_display_l: i64,
    pub pre_display_s: i64,
    /// tex.web keeps the interrupted paragraph's final line in just_box so
    /// finish_display can measure \predisplaysize AFTER the page builder has
    /// consumed the contributions. We clone the last broken line here at
    /// paragraph end; display entry takes it (a stale line never survives —
    /// every paragraph break rewrites or clears it).
    pub last_par_line: Option<crate::boxes::Node>,
    /// when a display interrupts a paragraph, the paragraph's widow penalty
    /// becomes \displaywidowpenalty (tex.web §21764 line_break argument);
    /// set by enter_math, consumed by end_paragraph
    pub next_par_widow: Option<i32>,
    /// \eqno/\leqno state: on the primitive the formula mlist is parked here
    /// and the tag collects into a fresh list (tex.web start_eq_no); the
    /// bool marks \leqno (tag on the left)
    pub pending_display_formula: Option<Vec<crate::boxes::Node>>,
    pub eqno_leqno: Option<bool>,
    /// tex.web subformula boundaries in math mode: positions in the current
    /// math list where `{` groups opened — \over's numerator stops there
    pub math_group_marks: Vec<usize>,
    pub scanner_status: ScannerStatus,
    pub saved_lists: Vec<(Mode, Vec<crate::boxes::Node>, i32, i32)>,
    /// saved state pushed by paragraph start (pops with \par, not with groups)
    pub par_saves: usize,
    /// set when a display just ended: text resumes hmode directly
    /// (tex.web resume_after_display §1194 — no \parskip, no \parindent,
    /// no \everypar); consumed by the next start_paragraph
    pub resume_after_display: bool,
    /// set when build_page ships a page that consumed the lines of the
    /// paragraph currently being broken (tex.web soft page break inside a
    /// paragraph): the resumed partial content has NO complete line yet, so
    /// just_box/\predisplaysize must not use the stale last_par_line clone
    pub par_interrupted: bool,
    pub unless_next: bool,
    pub last_badness: i32,
    pub pdf_last_x: i32,
    pub pdf_last_y: i32,
    /// \pdflastobj / \pdflastxform / \pdflastximage / \pdflastlink /
    /// \pdflastannot: object numbers of the last allocated PDF objects.
    pub pdf_last_obj: i32,
    pub pdf_last_xform: i32,
    pub pdf_last_ximage: i32,
    pub pdf_last_link: i32,
    pub pdf_last_annot: i32,
    /// next free object number for \pdfobj-style reservations (pdfTeX
    /// reserves 1..4 for Catalog/Pages/Info/Outlines).
    pub pdf_next_obj: i32,
    pub marks: [Vec<Vec<Token>>; 5], // top, first, bot, splitfirst, splitbot (class-indexed)
    pub last_named_cs: Option<CsId>,
    pub align_in_noalign: bool,
    /// \\everycr already inserted for the row currently starting; stops
    /// align_start_row from re-pushing it when the post-everycr content
    /// token arrives.
    pub align_everycr_done: bool,
    pub align_cell_toks: Vec<Token>,
    pub after_assignment: Option<Token>,

    pub log: String,
    pub term: String,
}

impl Engine {
    pub fn current_line_text(&self) -> String {
        for s in self.input.stack.iter().rev() {
            if let crate::input::Source::File { line_buf, .. } = s {
                return line_buf
                    .as_ref()
                    .map(|b| String::from_utf8_lossy(b).into_owned())
                    .unwrap_or_default();
            }
        }
        String::new()
    }


    pub fn new(ini_mode: bool) -> Engine {
        let mut cs = CsTable::new();
        let par = cs.intern(b"par");
        let e = Engine {
            ids: Ids { par, cs_escape: b'\\' },
            cs,
            eqtb: Eqtb::new(ini_mode),
            input: InputStack::new(),
            par_saves: 0,
            resume_after_display: false,
            par_interrupted: false,
            pending_retokenize: false,
            cur_tok: crate::token::EOF_TOKEN,
            cur_cs: None,
            cur_prim: None,
            cur_chr: 0,
            mode: Mode::Vertical,
            mode_level: 0,
            cur_list: Vec::new(),
            prev_depth: -1000 * 65536,
            space_factor: 1000,
            prev_graf: 0,
            after_token: false,
            ini_mode,
            format_name: String::new(),
            job_name: String::new(),
            halt_on_error: false,
            error_count: 0,
            write_streams: (0..16).map(|_| None).collect(),
            writebuf: Vec::new(),
            hyphen_trie: crate::hyphen::Trie::new(),
            hyphen_exceptions: Vec::new(),
            par_shape: Vec::new(),
            par_shape_level: crate::eqtb::LEVEL_ONE,
            pdf_doc: crate::pdfout::PdfDoc::new(),
            out_file: None,
            font_loader: crate::fontload::FontLoader::new(),
            pdf_outlines: Vec::new(),
            job_running: true,
            end_occurred: false,
            if_stack: Vec::new(),
            pushed: Vec::new(),
            cur_fill_order: 0,
            last_macros: std::collections::VecDeque::new(),
            tok_ring: std::collections::VecDeque::new(),
            def_prefix: Vec::new(),
            global_flag: false,
            long_flag: false,
            outer_flag: false,
            protected_flag: false,
            expand_protected: 0,
            in_expanded_scan: false,
            unexp_protect: 0,
            csname_depth: 0,

            no_expand_tok: None,
            align_state: 0,
            ss_trace: Vec::new(),
            format_done: false,
            trace_ltx: 0,
            pdf_page_attr: String::new(),
            pdf_pages_attr: String::new(),
            pdf_last_obj: 0,
            pdf_last_xform: 0,
            pdf_last_ximage: 0,
            pdf_last_link: 0,
            pdf_last_annot: 0,
            pdf_next_obj: 5,
            left_delim: None,
            right_delim: None,
            setbox_target: None,
            setbox_depth: usize::MAX,
            setbox_global: false,
            setbox_stack: Vec::new(),
            pending_box_shift: None,
            pending_the_string: None,
            math_limits: None,
            last_delim: None,
            page_list: Vec::new(),
            box_targets: Vec::new(),
            box_shifts: Vec::new(),
            box_kinds: Vec::new(),
            insert_nums: Vec::new(),
            shipout_pending: false,
            shipout_depth: usize::MAX,
            par_page_lists: Vec::new(),
            read_eof: Vec::new(),
            read_files: Vec::new(),
            out_dir: String::new(),
            main_dir: None,
            job_ended_by_end: false,
            align_preamble: Vec::new(),
            align_rows: Vec::new(),
            align_col_widths: Vec::new(),
            align_cur_row: Vec::new(),
            align_cur_col: 0,
            align_in_noalign: false,
            align_everycr_done: false,
            align_cell_toks: Vec::new(),
            align_scanning_cell: false,
            align_pushed_base: 0,
            align_noalign_save_base: 0,
            align_to: None,
            align_done: false,
            in_output: false,
            output_depth: 0,
            dead_cycles: 0,
            page_prev_depth: -1000 * 65536,
            page_total: 0,
            page_depth: 0,
            page_processed: 0,
            page_best_break: None,
            page_break_penalty: 0,
            page_best_cost: 0,
            page_goal_set: false,
            page_stretch: [0; 4],
            page_shrink: [0; 4],
            vsplat_remainder: None,
            math_lists: Vec::new(),
            math_penalties: std::cell::Cell::new(false),
            pre_display_size: -0x3FFF_FFFF,
            pre_display_l: 0,
            last_par_line: None,
            next_par_widow: None,
            pending_display_formula: None,
            eqno_leqno: None,
            math_group_marks: Vec::new(),
            pre_display_s: 0,
            gt_steps: 0,
            rt_steps: 0,
            mac_depth: 0,
            current_macro: String::new(),
            loop_traced: false,
            math_style_stack: Vec::new(),
            scanner_status: ScannerStatus::Normal,
            saved_lists: Vec::new(),
            unless_next: false,
            last_badness: 0,
            pdf_last_x: 0,
            pdf_last_y: 0,
            marks: [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            log: String::new(),
            term: String::new(),
            last_named_cs: None,
            after_assignment: None,
        };
        e
    }

    pub fn init_primitives(&mut self) {
        use Prim::*;
        let mut def = |name: &[u8], p: Prim, e: &mut Engine| {
            let id = e.cs.intern(name);
            e.eqtb.assign(id, Equiv::Prim(p), true);
        };
        macro_rules! d {
            ($e:expr, $name:literal, $p:expr) => {
                def($name, $p, $e)
            };
        }
        let eng = self;
        d!(eng, b"relax", Relax);
        d!(eng, b"expandafter", ExpandAfter);
        d!(eng, b"noexpand", NoExpand);
        d!(eng, b"csname", CsName);
        d!(eng, b"lastnamedcs", LastNamedCs);
        d!(eng, b"endcsname", EndCsName);
        d!(eng, b"the", The);
        d!(eng, b"string", String);
        d!(eng, b"meaning", Meaning);
        d!(eng, b"number", Number);
        d!(eng, b"romannumeral", RomanNumeral);
        d!(eng, b"lowercase", Lowercase);
        d!(eng, b"uppercase", Uppercase);
        d!(eng, b"detokenize", Detokenize);
        d!(eng, b"expanded", Expanded);
        d!(eng, b"unexpanded", UnExpanded);
        d!(eng, b"scantokens", ScanTokens);
        d!(eng, b"input", Input);
        d!(eng, b"numexpr", NumExpr);
        d!(eng, b"dimexpr", DimExpr);
        d!(eng, b"glueexpr", GlueExpr);
        d!(eng, b"muexpr", MuExpr);
        d!(eng, b"endinput", EndInput);
        d!(eng, b"dump", Dump);
        d!(eng, b"patterns", Patterns);
        d!(eng, b"hyphenation", Hyphenation);
        d!(eng, b"if", IfChar);
        d!(eng, b"ifcat", IfCat);
        d!(eng, b"ifodd", IfOdd);
        d!(eng, b"ifnum", IfNum);
        d!(eng, b"ifdim", IfDim);
        d!(eng, b"ifvoid", IfVoid);
        d!(eng, b"ifhbox", IfHBox);
        d!(eng, b"ifvbox", IfVBox);
        d!(eng, b"ifhmode", IfHMode);
        d!(eng, b"ifvmode", IfVMode);
        d!(eng, b"ifinner", IfInner);
        d!(eng, b"ifmmode", IfMMode);
        d!(eng, b"ifeof", IfEOF);
        d!(eng, b"iftrue", IfTrue);
        d!(eng, b"iffalse", IfFalse);
        d!(eng, b"ifdefined", IfDef);
        d!(eng, b"ifcsname", IfCSName);
        d!(eng, b"ifincsname", IfInCsName);

        d!(eng, b"ifx", IfX);
        d!(eng, b"ifcase", IfCase);
        d!(eng, b"iffontchar", IfFontChar);
        d!(eng, b"else", Else);
        d!(eng, b"or", Or);
        d!(eng, b"fi", Fi);
        d!(eng, b"unless", Unless);
        d!(eng, b"bgroup", BGroup);
        d!(eng, b"egroup", EGroup);
        d!(eng, b"begingroup", BeginGroup);
        d!(eng, b"endgroup", EndGroup);
        d!(eng, b"afterassignment", AfterAssignment);
        d!(eng, b"aftergroup", AfterGroup);
        d!(eng, b"def", Def);
        d!(eng, b"gdef", GDef);
        d!(eng, b"edef", EDef);
        d!(eng, b"xdef", XDef);
        d!(eng, b"let", Let);
        d!(eng, b"futurelet", FutureLet);
        d!(eng, b"global", Global);
        d!(eng, b"long", Long);
        d!(eng, b"outer", Outer);
        d!(eng, b"protected", Protected);
        d!(eng, b"catcode", CatCode);
        d!(eng, b"mathcode", MathCode);
        d!(eng, b"delcode", DelCode);
        d!(eng, b"lccode", LcCodeP);
        d!(eng, b"sfcode", SfCodeP);
        d!(eng, b"uccode", UcCodeP);
        d!(eng, b"count", Count);
        d!(eng, b"dimen", Dimen);
        d!(eng, b"skip", Skip);
        d!(eng, b"muskip", MuSkip);
        d!(eng, b"toks", Toks);
        d!(eng, b"box", Box);
        d!(eng, b"copy", Copy);
        d!(eng, b"wd", Wd);
        d!(eng, b"ht", Ht);
        d!(eng, b"dp", Dp);
        d!(eng, b"countdef", CountDef);
        d!(eng, b"dimendef", DimenDef);
        d!(eng, b"skipdef", SkipDef);
        d!(eng, b"muskipdef", MuSkipDef);
        d!(eng, b"toksdef", ToksDef);
        d!(eng, b"chardef", CharDef);
        d!(eng, b"mathchardef", MathCharDef);
        d!(eng, b"fontdimen", FontDimen);
        d!(eng, b"hyphenchar", HyphenChar);
        d!(eng, b"skewchar", SkewChar);
        d!(eng, b"parshape", ParShape);
        d!(eng, b"setbox", SetBox);
        d!(eng, b"advance", Advance);
        d!(eng, b"multiply", Multiply);
        d!(eng, b"divide", Divide);
        // named int params
        let intnames: &[(&[u8], IntParam)] = &[
            (b"pretolerance", IntParam::Pretolerance),
            (b"tolerance", IntParam::Tolerance),
            (b"linepenalty", IntParam::LinePenalty),
            (b"hyphenpenalty", IntParam::HyphenPenalty),
            (b"exhyphenpenalty", IntParam::ExHyphenPenalty),
            (b"clubpenalty", IntParam::ClubPenalty),
            (b"widowpenalty", IntParam::WidowPenalty),
            (b"displaywidowpenalty", IntParam::DisplayWidowPenalty),
            (b"brokenpenalty", IntParam::BrokenPenalty),
            (b"binoppenalty", IntParam::BinOpPenalty),
            (b"relpenalty", IntParam::RelPenalty),
            (b"predisplaypenalty", IntParam::PreDisplayPenalty),
            (b"postdisplaypenalty", IntParam::PostDisplayPenalty),
            (b"interlinepenalty", IntParam::InterLinePenalty),
            (b"doublehyphendemerits", IntParam::DoubleHyphenDemerits),
            (b"finalhyphendemerits", IntParam::FinalHyphenDemerits),
            (b"adjdemerits", IntParam::AdjDemerits),
            (b"mag", IntParam::Mag),
            (b"delimiterfactor", IntParam::DelimiterFactor),
            (b"looseness", IntParam::Looseness),
            (b"hbadness", IntParam::HBadness),
            (b"vbadness", IntParam::VBadness),
            (b"pausing", IntParam::Pausing),
            (b"tracingonline", IntParam::TracingOnline),
            (b"tracingmacros", IntParam::TracingMacros),
            (b"tracingstats", IntParam::TracingStats),
            (b"tracingparagraphs", IntParam::TracingParagraphs),
            (b"tracingpages", IntParam::TracingPages),
            (b"tracingoutput", IntParam::TracingOutput),
            (b"tracinglostchars", IntParam::TracingLostChars),
            (b"tracingcommands", IntParam::TracingCommands),
            (b"tracingrestores", IntParam::TracingRestores),
            (b"tracingif", IntParam::TracingIf),
            (b"tracingfonts", IntParam::TracingFonts),
            (b"showboxbreadth", IntParam::ShowBoxBreadth),
            (b"showboxdepth", IntParam::ShowBoxDepth),
            (b"errorstopmode", IntParam::ErrorStopMode),
            (b"scrollmode", IntParam::ScrollMode),
            (b"nonstopmode", IntParam::NonStopMode),
            (b"batchmode", IntParam::BatchMode),
            (b"language", IntParam::Language),
            (b"uchyph", IntParam::UcHyph),
            (b"lefthyphenmin", IntParam::LeftHyphenMin),
            (b"righthyphenmin", IntParam::RightHyphenMin),
            (b"escapechar", IntParam::EscapeChar),
            (b"endlinechar", IntParam::EndLineChar),
            (b"newlinechar", IntParam::NewLineChar),
            (b"defaulthyphenchar", IntParam::Defaulthyphenchar),
            (b"defaultskewchar", IntParam::Defaultskewchar),
            (b"errorcontextlines", IntParam::ErrorContextLines),
            (b"maxdeadcycles", IntParam::MaxDeadCycles),
            (b"insertpenalties", IntParam::InsertPenalties),
            (b"outputpenalty", IntParam::OutputPenalty),
            (b"floatingpenalty", IntParam::FloatingPenalty),
            (b"hangafter", IntParam::HangAfter),
            (b"prevgraf", IntParam::PrevGraf),
            (b"fam", IntParam::CurFam),
            (b"time", IntParam::Time),
            (b"day", IntParam::Day),
            (b"month", IntParam::Month),
            (b"year", IntParam::Year),
            (b"inputlineno", IntParam::InputLineNo),
            (b"badness", IntParam::Badness),
            (b"deadcycles", IntParam::DeadCycles),
            (b"eTeXversion", IntParam::EtxVersion),
            (b"pdfoutput", IntParam::PdfOutput),
            (b"pdfadjustspacing", IntParam::PdfAdjustSpacing),
            (b"pdfprotrudechars", IntParam::PdfProtrudeChars),
            (b"pdfminorversion", IntParam::PdfMinorVersion),
            (b"pdftexversion", IntParam::PdfTexVersion),
            (b"interactionmode", IntParam::InteractionMode),
            (b"currentgrouplevel", IntParam::CurrentGroupLevel),
            (b"currentgrouptype", IntParam::CurrentGroupType),
            (b"currentiflevel", IntParam::CurrentIfLevel),
            (b"currentiftype", IntParam::CurrentIfType),
            (b"currentifbranch", IntParam::CurrentIfBranch),
            (b"lastnodetype", IntParam::LastNodeType),
            (b"savingvdiscards", IntParam::SavingVDiscards),
            (b"tracingnesting", IntParam::TracingNesting),
            (b"pdfobjcompresslevel", IntParam::PdfObjCompressLevel),
            (b"pdfgentounicode", IntParam::PdfGenToUnicode),
            (b"paperquality", IntParam::PaperQuality),
            (b"globaldefs", IntParam::GlobalDefs),
        ];
        for (n, p) in intnames {
            let id = eng.cs.intern(n);
            eng.eqtb.assign(id, Equiv::Prim(Prim::IntP(*p)), true);
        }
        eng.eqtb.int_params[IntParam::PdfOutput.idx() as usize] = 1;
        eng.eqtb.int_params[IntParam::PdfTexVersion.idx() as usize] = 140;
        eng.eqtb.int_params[IntParam::PdfMinorVersion.idx() as usize] = 7;
        eng.eqtb.int_params[IntParam::EtxVersion.idx() as usize] = 2;
        eng.eqtb.int_params[IntParam::PaperQuality.idx() as usize] = 1;
        let dimnames: &[(&[u8], DimParam)] = &[
            (b"parindent", DimParam::ParIndent),
            (b"mathsurround", DimParam::MathSurround),
            (b"lineskiplimit", DimParam::LineSkipLimit),
            (b"hsize", DimParam::HSize),
            (b"vsize", DimParam::VSize),
            (b"maxdepth", DimParam::MaxDepth),
            (b"splitmaxdepth", DimParam::SplitMaxDepth),
            (b"boxmaxdepth", DimParam::BoxMaxDepth),
            (b"displayindent", DimParam::DisplayIndent),
            (b"displaywidth", DimParam::DisplayWidth),
            (b"predisplaysize", DimParam::PreDisplaySize),
            (b"hangindent", DimParam::HangIndent),
            (b"emergencystretch", DimParam::EmergencyStretch),
            (b"pagegoal", DimParam::PageGoal),
            (b"pagetotal", DimParam::PageTotal),
            (b"pagedepth", DimParam::PageDepth),
            (b"pagestretch", DimParam::PageStretch),
            (b"pagefilstretch", DimParam::PageFilStretch),
            (b"pagefillstretch", DimParam::PageFillStretch),
            (b"pagefilllstretch", DimParam::PageFilllStretch),
            (b"pageshrink", DimParam::PageShrink),
            (b"delimitershortfall", DimParam::DelimiterShortfall),
            (b"hfuzz", DimParam::Hfuzz),
            (b"vfuzz", DimParam::Vfuzz),
            (b"overfullrule", DimParam::OverfullRule),
            (b"nulldelimiterspace", DimParam::NullDelimiterSpace),
            (b"scriptspace", DimParam::ScriptSpace),
            (b"topskip", DimParam::TopSkip),
            // pdfTeX page/origin driver dimensions (real pdfTeX dimensions,
            // so \setlength/\ifdim/\the/\divide all work on them)
            (b"pdfpagewidth", DimParam::PdfPageWidth),
            (b"pdfpageheight", DimParam::PdfPageHeight),
            (b"pdfhorigin", DimParam::PdfHOrigin),
            (b"pdfvorigin", DimParam::PdfVOrigin),
            (b"pdflinkmargin", DimParam::PdfLinkMargin),
            (b"pdfdestmargin", DimParam::PdfDestMargin),
            (b"pdfthreadmargin", DimParam::PdfThreadMargin),
            (b"hoffset", DimParam::HOffset),
            (b"voffset", DimParam::VOffset),
            (b"prevdepth", DimParam::PrevDepth),
        ];
        for (n, p) in dimnames {
            let id = eng.cs.intern(n);
            eng.eqtb.assign(id, Equiv::Prim(Prim::DimP(*p)), true);
        }
        let gluenames: &[(&[u8], GlueParam)] = &[
            (b"lineskip", GlueParam::LineSkip),
            (b"baselineskip", GlueParam::BaselineSkip),
            (b"parskip", GlueParam::ParSkip),
            (b"leftskip", GlueParam::LeftSkip),
            (b"rightskip", GlueParam::RightSkip),
            (b"parfillskip", GlueParam::ParFillSkip),
            (b"spaceskip", GlueParam::SpaceSkip),
            (b"xspaceskip", GlueParam::XSpaceSkip),
            (b"abovedisplayskip", GlueParam::AboveDisplaySkip),
            (b"abovedisplayshortskip", GlueParam::AboveDisplayShortSkip),
            (b"belowdisplayskip", GlueParam::BelowDisplaySkip),
            (b"belowdisplayshortskip", GlueParam::BelowDisplayShortSkip),
            (b"splittopskip", GlueParam::SplitTopSkip),
            (b"tabskip", GlueParam::TabSkip),
            (b"thinmuskip", GlueParam::ThinMuSkip),
            (b"medmuskip", GlueParam::MedMuSkip),
            (b"thickmuskip", GlueParam::ThickMuSkip),
        ];
        for (n, p) in gluenames {
            let id = eng.cs.intern(n);
            eng.eqtb.assign(id, Equiv::Prim(Prim::GlueP(*p)), true);
        }
        let toksnames: &[(&[u8], ToksParam)] = &[
            (b"everypar", ToksParam::EveryPar),
            (b"everymath", ToksParam::EveryMath),
            (b"everydisplay", ToksParam::EveryDisplay),
            (b"everyhbox", ToksParam::EveryHBox),
            (b"everyvbox", ToksParam::EveryVBox),
            (b"everyjob", ToksParam::EveryJob),
            (b"everycr", ToksParam::EveryCr),
            (b"everyeof", ToksParam::EveryEOF),
            (b"output", ToksParam::Output),
            (b"errhelp", ToksParam::ErrHelp),
        ];
        for (n, p) in toksnames {
            let id = eng.cs.intern(n);
            eng.eqtb.assign(id, Equiv::Prim(Prim::ToksP(*p)), true);
        }
        d!(eng, b"font", Font);
        d!(eng, b"fontname", FontName);
        d!(eng, b"fontid", FontIdPrim);
        d!(eng, b"hskip", HSkip);
        d!(eng, b"vskip", VSkip);
        d!(eng, b"mskip", MSkip);
        d!(eng, b"hfil", HFil);
        d!(eng, b"hfill", HFill);
        d!(eng, b"hfilll", HFilL);
        d!(eng, b"hfilneg", HFilNeg);
        d!(eng, b"hss", HSS);
        d!(eng, b"vfil", VFil);
        d!(eng, b"vfill", VFill);
        d!(eng, b"vfilll", VFilL);
        d!(eng, b"vfilneg", VFilNeg);
        d!(eng, b"vss", VSS);
        d!(eng, b"kern", Kern);
        d!(eng, b"mkern", MKern);
        d!(eng, b"moveleft", HMove);
        d!(eng, b"moveright", HMove);
        d!(eng, b"raise", VMove);
        d!(eng, b"lower", VMove);
        d!(eng, b"hbox", HBox);
        d!(eng, b"vbox", VBox);
        d!(eng, b"vtop", VTop);
        d!(eng, b"vcenter", VCenter);
        d!(eng, b"hrule", HRule);
        d!(eng, b"vrule", VRule);
        d!(eng, b"leaders", Leaders);
        d!(eng, b"cleaders", CLeaders);
        d!(eng, b"xleaders", XLeaders);
        d!(eng, b"par", Par);
        d!(eng, b"indent", Indent);
        d!(eng, b"noindent", NoIndent);
        d!(eng, b"unskip", UnSkip);
        d!(eng, b"ignorespaces", IgnoreSpaces);
        d!(eng, b"unkern", UnKern);
        d!(eng, b"unpenalty", UnPenalty);
        d!(eng, b"unhbox", UnHBox);
        d!(eng, b"unvbox", UnVBox);
        d!(eng, b"unhcopy", UnHCopy);
        d!(eng, b"unvcopy", UnVCopy);
        d!(eng, b"lastbox", LastBox);

        d!(eng, b"gluestretch", GlueStretch);
        d!(eng, b"glueshrink", GlueShrink);
        d!(eng, b"gluestretchorder", GlueStretchOrder);
        d!(eng, b"glueshrinkorder", GlueShrinkOrder);
        d!(eng, b"lastkern", LastKern);
        d!(eng, b"lastpenalty", LastPenalty);
        d!(eng, b"lastskip", LastSkip);
        d!(eng, b"vsplit", VSplit);
        d!(eng, b"penalty", Penalty);
        d!(eng, b"penalties", Penalties);
        d!(eng, b"insert", Insert);
        d!(eng, b"vadjust", VAdjust);
        d!(eng, b"mark", MarkPrim);
        d!(eng, b"marks", MarkPrim);
        d!(eng, b"topmark", TopMark);
        d!(eng, b"topmarks", TopMark);
        d!(eng, b"firstmark", FirstMark);
        d!(eng, b"firstmarks", FirstMark);
        d!(eng, b"botmark", BotMark);
        d!(eng, b"botmarks", BotMark);
        d!(eng, b"splitfirstmark", SplitFirstMark);
        d!(eng, b"splitfirstmarks", SplitFirstMark);
        d!(eng, b"splitbotmark", SplitBotMark);
        d!(eng, b"splitbotmarks", SplitBotMark);
        d!(eng, b"shipout", ShipOut);
        d!(eng, b"char", Char);
        d!(eng, b"radical", Radical);
        d!(eng, b"delimiter", Delimiter);
        d!(eng, b"eqno", EqNo);
        d!(eng, b"leqno", LeqNo);
        d!(eng, b"mathchar", MathChar);
        d!(eng, b"mathaccent", MathAccent);
        d!(eng, b"overline", Overline);
        d!(eng, b"underline", Underline);
        d!(eng, b"radical", Radical);
        d!(eng, b"eqno", EqNo);
        d!(eng, b"leqno", LeqNo);
        d!(eng, b"above", Above);
        d!(eng, b"over", Over);
        d!(eng, b"atop", Atop);
        d!(eng, b"overwithdelims", OverWithDelims);
        d!(eng, b"atopwithdelims", AtopWithDelims);
        d!(eng, b"abovewithdelims", AboveWithDelims);
        d!(eng, b"textfont", TextFont);
        d!(eng, b"scriptfont", ScriptFont);
        d!(eng, b"scriptscriptfont", ScriptScriptFont);
        d!(eng, b"left", Left);
        d!(eng, b"right", Right);
        d!(eng, b"middle", Middle);
        d!(eng, b"nolimits", NoLimits);
        d!(eng, b"limits", Limits);
        d!(eng, b"displaylimits", DisplayLimits);
        d!(eng, b"mathchoice", MathChoice);
        d!(eng, b"mathord", MathOrd);
        d!(eng, b"mathop", MathOp);
        d!(eng, b"mathbin", MathBin);
        d!(eng, b"mathrel", MathRel);
        d!(eng, b"mathopen", MathOpen);
        d!(eng, b"mathclose", MathClose);
        d!(eng, b"mathpunct", MathPunct);
        d!(eng, b"mathinner", MathInner);
        d!(eng, b"displaystyle", DisplayStyle);
        d!(eng, b"textstyle", TextStyle);
        d!(eng, b"scriptstyle", ScriptStyle);
        d!(eng, b"scriptscriptstyle", ScriptScriptStyle);
        d!(eng, b"span", Span);
        d!(eng, b"cr", Cr);
        d!(eng, b"crcr", CrCr);
        d!(eng, b"omit", Omit);
        d!(eng, b"noalign", NoAlign);
        d!(eng, b"halign", HAlign);
        d!(eng, b"valign", VAlign);
        d!(eng, b"immediate", Immediate);
        d!(eng, b"openout", OpenOut);
        d!(eng, b"closeout", CloseOut);
        d!(eng, b"write", Write);
        d!(eng, b"special", Special);
        d!(eng, b"message", Message);
        d!(eng, b"errmessage", ErrMessage);
        d!(eng, b"openin", OpenIn);
        d!(eng, b"closein", CloseIn);
        d!(eng, b"read", Read);
        d!(eng, b"readline", ReadLine);
        d!(eng, b"jobname", JobName);
        d!(eng, b"show", Show);
        d!(eng, b"showbox", ShowBox);
        d!(eng, b"showthe", ShowThe);
        d!(eng, b"showlists", ShowLists);
        d!(eng, b"showgroups", ShowGroups);
        d!(eng, b"showtokens", ShowTokens);
        d!(eng, b"showifs", ShowIfs);
        d!(eng, b"pdfliteral", PdfLiteral);
        d!(eng, b"pdfsave", PdfSave);
        d!(eng, b"pdfrestore", PdfRestore);
        d!(eng, b"pdfsetmatrix", PdfSetMatrix);
        d!(eng, b"pdfstartlink", PdfStartLink);
        d!(eng, b"pdfendlink", PdfEndLink);
        d!(eng, b"pdfdest", PdfDest);
        d!(eng, b"pdfoutline", PdfOutline);
        d!(eng, b"pdfinfo", PdfInfo);
        d!(eng, b"pdfcatalog", PdfCatalog);
        d!(eng, b"pdfnames", PdfNames);
        d!(eng, b"pdfannot", PdfAnnot);
        d!(eng, b"pdfpageattr", PdfPageAttr);
        d!(eng, b"pdfcolorstackinit", PdfColorStackInit);
        d!(eng, b"pdfcolorstack", PdfColorStack);
        d!(eng, b"Ucharcat", UcharCat);
        d!(eng, b"pdfsavepos", PdfSavePos);
        d!(eng, b"pdflastxpos", PdfLastXPos);
        d!(eng, b"pdflastypos", PdfLastYPos);
        d!(eng, b"pdftexrevision", PdfTexRevision);
        d!(eng, b"pdfmapfile", PdfMapFile);
        d!(eng, b"pdfmapline", PdfMapLine);
        d!(eng, b"pdfglyphtounicode", PdfGlyphToUnicode);
        d!(eng, b"pdffontattr", PdfFontAttr);
        d!(eng, b"pdfxform", PdfXForm);
        d!(eng, b"pdfximage", PdfXImage);
        d!(eng, b"pdfrefxform", PdfRefXForm);
        d!(eng, b"pdfrefximage", PdfRefXImage);
        d!(eng, b"pdfpagesattr", PdfPagesAttr);
        d!(eng, b"pdfcompresslevel", PdfCompressorLevel);
        d!(eng, b"pdfobj", PdfObj);
        d!(eng, b"pdfrefobj", PdfRefObj);
        d!(eng, b"pdfuncompress", PdfUncompress);
        d!(eng, b"pdftolerance", PdfTolerance);
        d!(eng, b"pdfpagebox", PdfPageBox);
        d!(eng, b"pdfthread", PdfThread);
        d!(eng, b"pdfstartthread", PdfStartThread);
        d!(eng, b"pdfendthread", PdfEndThread);
        d!(eng, b"pdflastobj", PdfLastObj);
        d!(eng, b"pdflastxform", PdfLastXForm);
        d!(eng, b"pdflastximage", PdfLastXImage);
        d!(eng, b"pdflastlink", PdfLastLink);
        d!(eng, b"pdflastannot", PdfLastAnnot);
        d!(eng, b"pdffilesize", PdfFileSize);
        d!(eng, b"pdfmdfivesum", PdfMdFiveSum);
        d!(eng, b"pdffilemoddate", PdfFileModDate);
        d!(eng, b"pdffiledump", PdfFileDump);
        d!(eng, b"pdfstrcmp", PdfStrCmp);
        d!(eng, b"pdfshellescape", PdfShellEscape);
        d!(eng, b"pdfelapsedtime", PdfElapsedTime);
        d!(eng, b"pdfresettimer", PdfResetTimer);
        d!(eng, b"pdfuniformdeviate", PdfUniformDeviate);
        d!(eng, b"pdfnormaldeviate", PdfNormalDeviate);
        d!(eng, b"pdfescapestring", PdfEscapeString);
        d!(eng, b"pdfescapename", PdfEscapeName);
        d!(eng, b"pdfescapehex", PdfEscapeHex);
        d!(eng, b"pdfunescapehex", PdfUnescapeHex);
        d!(eng, b"filesize", FileSize);
        d!(eng, b"end", End);
        // TeX82 defaults (tex.web §25 / plain.tex)
        eng.eqtb.glue_params[GlueParam::ParFillSkip.idx() as usize] =
            crate::boxes::Glue::fil(crate::boxes::GLUE_FIL, 0);
        eng.eqtb.glue_params[GlueParam::BaselineSkip.idx() as usize] =
            crate::boxes::Glue::new(12 * 65536);
        eng.eqtb.glue_params[GlueParam::LineSkip.idx() as usize] =
            crate::boxes::Glue::new(65536);
        // plain.tex / fontmath.ltx: \thinmuskip=3mu, \medmuskip=4mu plus 2mu
        // minus 4mu, \thickmuskip=5mu plus 5mu — stored mu-denominated
        // (tex.web §431); math_glue converts with the current em at use.
        eng.eqtb.glue_params[GlueParam::ThinMuSkip.idx() as usize] =
            crate::boxes::Glue::new(3 * 65536);
        eng.eqtb.glue_params[GlueParam::MedMuSkip.idx() as usize] =
            crate::boxes::Glue { width: 4 * 65536, stretch: 2 * 65536, shrink: 4 * 65536, stretch_order: 0, shrink_order: 0 };
        eng.eqtb.glue_params[GlueParam::ThickMuSkip.idx() as usize] =
            crate::boxes::Glue { width: 5 * 65536, stretch: 5 * 65536, shrink: 0, stretch_order: 0, shrink_order: 0 };
        eng.eqtb.int_params[IntParam::EndLineChar.idx() as usize] = 13;
        eng.eqtb.int_params[IntParam::EscapeChar.idx() as usize] = 92;
        eng.eqtb.int_params[IntParam::NewLineChar.idx() as usize] = -1;
        eng.eqtb.int_params[IntParam::MaxDeadCycles.idx() as usize] = 25;
        eng.eqtb.int_params[IntParam::Mag.idx() as usize] = 1000;
        eng.eqtb.int_params[IntParam::Tolerance.idx() as usize] = 10000;
        eng.eqtb.int_params[IntParam::Pretolerance.idx() as usize] = 100;
        eng.eqtb.int_params[IntParam::HangAfter.idx() as usize] = 1;
        eng.eqtb.int_params[IntParam::ErrorContextLines.idx() as usize] = 5;
        eng.eqtb.int_params[IntParam::LeftHyphenMin.idx() as usize] = 2;
        eng.eqtb.int_params[IntParam::RightHyphenMin.idx() as usize] = 3;
        eng.eqtb.int_params[IntParam::Defaulthyphenchar.idx() as usize] = 45;
        eng.eqtb.int_params[IntParam::Defaultskewchar.idx() as usize] = -1;
        eng.eqtb.int_params[IntParam::DelimiterFactor.idx() as usize] = 901;
        eng.eqtb.int_params[IntParam::ShowBoxBreadth.idx() as usize] = 5;
        eng.eqtb.int_params[IntParam::ShowBoxDepth.idx() as usize] = 3;
        eng.eqtb.int_params[IntParam::PdfOutput.idx() as usize] = 1;
        eng.eqtb.int_params[IntParam::EtxVersion.idx() as usize] = 2;
        eng.eqtb.int_params[IntParam::PdfMinorVersion.idx() as usize] = 7;
        // plain.tex paper: keep the page builder from firing on every box
        let sp_in: i32 = 4736287;
        eng.eqtb.dim_params[DimParam::HSize.idx() as usize] = (sp_in as i64 * 13 / 2) as i32;
        eng.eqtb.dim_params[DimParam::VSize.idx() as usize] = (sp_in as i64 * 89 / 10) as i32;
        // pdfTeX driver defaults: origin 1in from the page corner, US-letter
        // page geometry (geometry.sty overrides via \pdfpagewidth assignment)
        eng.eqtb.dim_params[DimParam::PdfHOrigin.idx() as usize] = sp_in;
        eng.eqtb.dim_params[DimParam::PdfVOrigin.idx() as usize] = sp_in;
        eng.eqtb.dim_params[DimParam::PdfPageWidth.idx() as usize] = (sp_in as i64 * 17 / 2) as i32;
        eng.eqtb.dim_params[DimParam::PdfPageHeight.idx() as usize] = (sp_in as i64 * 11) as i32;
    }
    pub fn pop_group(&mut self) -> crate::eqtb::LevelType {
        let mut ag = Vec::new();
        let mut ps = None;
        let ty = self.eqtb.pop_level_full(&mut ag, &mut ps);
        // tex.web: par_shape_ptr's level is tracked like any eqtb entry —
        // restore iff its current assignment is local to the closing group
        // (a later global assign leaves level == LEVEL_ONE and wins)
        if let Some((old, old_lvl)) = ps {
            if self.par_shape_level > crate::eqtb::LEVEL_ONE {
                self.par_shape = old;
                self.par_shape_level = old_lvl;
            }
        }
        self.pushed.extend(ag);
        ty
    }

    /// tex.web eq_define(par_shape_loc): level-tracked \parshape assignment —
    /// a local assign at a deeper group pushes the old value; the normal_
    /// paragraph clear is local too, so LaTeX's `{\@@par}` list wrapper rolls
    /// it back and the shape persists across items
    pub fn assign_par_shape(&mut self, new: Vec<(i32, i32)>, global: bool) {
        if crate::debug_flag("SHAPE") {
            eprintln!("ASSIGN-SHAPE n={} lvl={} cur_shape_lvl={} stack={} line={}", new.len(), self.eqtb.cur_level, self.par_shape_level, self.eqtb.save_stack.len(), self.input.current_file_line());
        }
        if global {
            self.par_shape = new;
            self.par_shape_level = crate::eqtb::LEVEL_ONE;
            return;
        }
        let lvl = self.eqtb.cur_level;
        if self.par_shape_level < lvl {
            let old = std::mem::replace(&mut self.par_shape, new);
            let old_lvl = self.par_shape_level;
            self.eqtb
                .save_stack
                .push(crate::eqtb::SaveItem::ParShape(old, old_lvl));
            self.par_shape_level = lvl;
        } else {
            self.par_shape = new;
        }
    }
    /// tex.web box_context: nest \setbox so an inner \setbox inside
    /// \shipout\vbox{\setbox...} cannot clobber the outer target. The
    /// pending \global prefix travels with the target.
    pub fn park_setbox(&mut self, idx: u16) {
        let g = std::mem::take(&mut self.global_flag);
        self.setbox_stack.push((self.setbox_target.take(), self.setbox_depth, self.setbox_global));
        self.setbox_target = Some(idx);
        self.setbox_depth = self.box_kinds.len();
        self.setbox_global = g;
    }
    pub fn unpark_setbox(&mut self) {
        match self.setbox_stack.pop() {
            Some((t, d, g)) => {
                self.setbox_target = t;
                self.setbox_depth = d;
                self.setbox_global = g;
            }
            None => {
                self.setbox_target = None;
                self.setbox_depth = usize::MAX;
                self.setbox_global = false;
            }
        }
    }
    pub fn trigger_after_assignment(&mut self) {
        if let Some(t) = self.after_assignment.take() {
            self.pushed.push(t);
        }
    }
}
