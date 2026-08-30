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
    pub line_buf: Option<Vec<u8>>,
    pub line_pos: usize,
    pub line_reload: bool,
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
    /// noexpand'd token pending (returned once, unexpanded)
    pub no_expand_tok: Option<Token>,
    pub cur_font: u16, // current font id (0 = none)
    pub align_state: i32, // & nesting balance for runaway detection
    pub format_done: bool,
    pub pdf_horigin: i32,
    pub pdf_vorigin: i32,
    pub pdf_page_width: Option<i32>,
    pub pdf_page_height: Option<i32>,
    pub left_delim: Option<i32>,
    pub right_delim: Option<i32>,
    pub math_limits: Option<u8>,
    pub last_delim: Option<i32>,
    pub pending_the_string: Option<String>,
    /// the main vertical list fed to the page builder (outer VM)
    pub page_list: Vec<crate::boxes::Node>,
    pub setbox_target: Option<u16>,
    pub pending_box_shift: Option<(i32, bool)>,
    pub box_targets: Vec<Option<(i32, bool)>>,
    pub box_shifts: Vec<i32>,
    pub box_kinds: Vec<u8>,
    pub insert_nums: Vec<u16>,
    pub shipout_pending: bool,
    pub par_page_lists: Vec<Vec<crate::boxes::Node>>,
    pub read_eof: Vec<bool>, // (amount, is_hmove)
    pub read_files: Vec<Option<std::fs::File>>,
    pub out_dir: String,
    pub job_ended_by_end: bool,
    pub align_preamble: Vec<crate::align::ColSpec>,
    pub align_rows: Vec<Vec<crate::align::Cell>>,
    pub align_col_widths: Vec<i32>,
    pub align_cur_row: Vec<crate::align::Cell>,
    pub align_cur_col: i32,
    pub align_in_noalign: bool,
    pub align_cell_toks: Vec<crate::token::Token>,
    pub align_scanning_cell: bool,
    pub align_done: bool,
    pub align_noalign_toks: Vec<Vec<crate::token::Token>>,
    pub in_output: bool,
    pub output_depth: usize,
    pub dead_cycles: i32,
    pub page_prev_depth: i32,
    pub page_total: i64,
    pub page_depth: i64,
    pub page_processed: usize,
    pub page_best_break: Option<usize>,
    pub page_break_penalty: i32,
    pub page_goal_set: bool,
    pub vsplat_remainder: Option<Vec<crate::boxes::Node>>,
    pub math_lists: Vec<Vec<crate::boxes::Node>>,
    pub gt_steps: u64,
    pub rt_steps: u64,
    pub mac_depth: u32,
    pub current_macro: String,
    pub loop_traced: bool,
    pub math_style_stack: Vec<crate::boxes::MathStyle>,
    pub scanner_status: ScannerStatus,
    pub saved_lists: Vec<(Mode, Vec<crate::boxes::Node>, i32, i32)>,
    /// saved state pushed by paragraph start (pops with \par, not with groups)
    pub par_saves: usize,
    pub unless_next: bool,
    pub last_badness: i32,
    pub pdf_last_x: i32,
    pub pdf_last_y: i32,
    pub marks: [Vec<Vec<Token>>; 5], // top, first, bot, splitfirst, splitbot (class-indexed)
    pub last_named_cs: Option<CsId>,

    pub log: String,
    pub term: String,
}

impl Engine {
    pub fn current_line_text(&self) -> String {
        self.line_buf
            .as_ref()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default()
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
            line_buf: None,
            line_pos: 0,
            line_reload: true,
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
            no_expand_tok: None,
            cur_font: 0,
            align_state: 0,
            format_done: false,
            // pdfTeX default origin: 1in from the page corner (tex.web: 4736286sp)
            pdf_horigin: 4_736_287,
            pdf_vorigin: 4_736_287,
            pdf_page_width: None,
            pdf_page_height: None,
            left_delim: None,
            right_delim: None,
            math_limits: None,
            last_delim: None,
            pending_the_string: None,
            page_list: Vec::new(),
            setbox_target: None,
            pending_box_shift: None,
            box_targets: Vec::new(),
            box_shifts: Vec::new(),
            box_kinds: Vec::new(),
            insert_nums: Vec::new(),
            shipout_pending: false,
            par_page_lists: Vec::new(),
            read_eof: Vec::new(),
            read_files: Vec::new(),
            out_dir: String::new(),
            job_ended_by_end: false,
            align_preamble: Vec::new(),
            align_rows: Vec::new(),
            align_col_widths: Vec::new(),
            align_cur_row: Vec::new(),
            align_cur_col: 0,
            align_in_noalign: false,
            align_cell_toks: Vec::new(),
            align_scanning_cell: false,
            align_done: false,
            align_noalign_toks: Vec::new(),
            in_output: false,
            output_depth: 0,
            dead_cycles: 0,
            page_prev_depth: -1000 * 65536,
            page_total: 0,
            page_depth: 0,
            page_processed: 0,
            page_best_break: None,
            page_break_penalty: 0,
            page_goal_set: false,
            vsplat_remainder: None,
            math_lists: Vec::new(),
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
            last_named_cs: None,
            marks: [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            log: String::new(),
            term: String::new(),
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
        d!(eng, b"ifx", IfX);
        d!(eng, b"ifcase", IfCase);
        d!(eng, b"else", Else);
        d!(eng, b"or", Or);
        d!(eng, b"fi", Fi);
        d!(eng, b"unless", Unless);
        d!(eng, b"bgroup", BGroup);
        d!(eng, b"egroup", EGroup);
        d!(eng, b"begingroup", BeginGroup);
        d!(eng, b"endgroup", EndGroup);
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
        ];
        for (n, p) in intnames {
            let id = eng.cs.intern(n);
            eng.eqtb.assign(id, Equiv::Prim(Prim::IntP(*p)), true);
        }
        eng.eqtb.int_params[IntParam::PdfOutput.idx() as usize] = 1;
        eng.eqtb.int_params[IntParam::PdfTexVersion.idx() as usize] = 140;
        eng.eqtb.int_params[IntParam::PdfMinorVersion.idx() as usize] = 7;
        eng.eqtb.int_params[IntParam::EtxVersion.idx() as usize] = 2;
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
        d!(eng, b"par", Par);
        d!(eng, b"indent", Indent);
        d!(eng, b"noindent", NoIndent);
        d!(eng, b"unskip", UnSkip);
        d!(eng, b"unkern", UnKern);
        d!(eng, b"unpenalty", UnPenalty);
        d!(eng, b"lastbox", LastBox);
        d!(eng, b"lastkern", LastKern);
        d!(eng, b"lastpenalty", LastPenalty);
        d!(eng, b"lastskip", LastSkip);
        d!(eng, b"vsplit", VSplit);
        d!(eng, b"penalty", Penalty);
        d!(eng, b"penalties", Penalties);
        d!(eng, b"insert", Insert);
        d!(eng, b"vadjust", VAdjust);
        d!(eng, b"mark", MarkPrim);
        d!(eng, b"topmark", TopMark);
        d!(eng, b"firstmark", FirstMark);
        d!(eng, b"botmark", BotMark);
        d!(eng, b"splitfirstmark", SplitFirstMark);
        d!(eng, b"splitbotmark", SplitBotMark);
        d!(eng, b"shipout", ShipOut);
        d!(eng, b"mathchar", MathChar);
        d!(eng, b"mathaccent", MathAccent);
        d!(eng, b"radical", Radical);
        d!(eng, b"delimiter", Delimiter);
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
        d!(eng, b"pdfdest", PdfDest);
        d!(eng, b"pdfoutline", PdfOutline);
        d!(eng, b"pdfinfo", PdfInfo);
        d!(eng, b"pdfcatalog", PdfCatalog);
        d!(eng, b"pdfannot", PdfAnnot);
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
        d!(eng, b"pdfhorigin", PdfHOrigin);
        d!(eng, b"pdfvorigin", PdfVOrigin);
        d!(eng, b"pdfpagewidth", PdfPageWidth);
        d!(eng, b"pdfpageheight", PdfPageHeight);
        d!(eng, b"pdfpagebox", PdfPageBox);
        d!(eng, b"pdfthread", PdfThread);
        d!(eng, b"pdfstartthread", PdfStartThread);
        d!(eng, b"pdfendthread", PdfEndThread);
        d!(eng, b"pdflinkmargin", PdfLinkMargin);
        d!(eng, b"pdfdestmargin", PdfDestMargin);
        d!(eng, b"pdfthreadmargin", PdfThreadMargin);
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
        let _ = def;
    }
}
