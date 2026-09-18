//! Engine state: control-sequence table, equivalents, input stack, modes,
//! lists, fonts, output files; plus primitive registration.

use crate::eqtb::{Eqtb, Equiv};
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
    pub accepting: bool,  // currently taking the true branch
    pub matched: bool,    // some branch was taken already
    pub if_case: i32,     // >=0: \ifcase with this many cases left
    pub evaluating: bool, // tex.web if_limit == if_code: condition still being evaluated
    pub loc_file: String,
    pub loc_line: u32,
    pub loc_cs: u32,
    pub(crate) loc: Option<crate::input::SourceMark>,
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
        matches!(
            self,
            Mode::InternalVertical | Mode::RestrictedHorizontal | Mode::Math
        )
    }
}
/// Hard stop for a runaway main loop (latex.ltx boot is well below this).
pub const MAX_MAIN_STEPS: u64 = 100_000_000;
/// Default resident-set cap. Override with TEX_MEM_LIMIT_MIB (0 disables).
pub const DEFAULT_RSS_LIMIT: u64 = 512 << 20;
pub const MAX_TERM_BYTES: usize = 32 << 20;
pub const MAX_PAGE_LIST: usize = 250_000;
pub const DEFAULT_MAX_ERRORS: usize = 100;
/// Cumulative expansion count is not a TeX capacity: valid large documents
/// have no fixed upper bound. Set TEX_EXPANSION_LIMIT to opt into a watchdog.
pub const DEFAULT_EXPANSION_LIMIT: u64 = 0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InteractionMode {
    Batch,
    Nonstop,
    Scroll,
    ErrorStop,
}

impl InteractionMode {
    pub const fn number(self) -> i32 {
        match self {
            Self::Batch => 0,
            Self::Nonstop => 1,
            Self::Scroll => 2,
            Self::ErrorStop => 3,
        }
    }

    pub const fn from_number(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::Batch),
            1 => Some(Self::Nonstop),
            2 => Some(Self::Scroll),
            3 => Some(Self::ErrorStop),
            _ => None,
        }
    }
}

/// Exact physical spelling of the most recently tokenized file token.
///
/// This deliberately stores only coordinates on the scanner hot path. The
/// source's reference-counted name, bytes, and include chain are cloned only
/// if a diagnostic or macro expansion actually needs the location.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PhysicalTokenSource {
    pub(crate) token: Token,
    pub(crate) semantic_cs: Option<CsId>,
    pub(crate) source_index: usize,
    pub(crate) line: u32,
    pub(crate) byte_column: usize,
    pub(crate) span: usize,
}

/// Interned ids for control sequences the engine itself references.
pub struct Ids {
    pub par: CsId,
    pub cs_escape: u8,
}

pub struct Engine {
    pub cs: CsTable,
    pub eqtb: Eqtb,
    /// Original control-sequence name of each primitive. Unlike an eqtb
    /// reverse lookup, this survives formats redefining (for example) \input.
    pub(crate) primitive_names: crate::FxHashMap<u16, &'static [u8]>,
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
    pub prev_depth: i32, // special marker: -1000pt means unset
    pub space_factor: i32,
    pub prev_graf: i32,
    pub after_token: bool,
    /// Consecutive non-control-sequence tokens seen where a definition target
    /// was required. This recovery state belongs to one TeX engine/job.
    pub(crate) definable_cs_recovery_count: u8,

    pub ini_mode: bool, // -ini: format-building mode
    pub format_name: String,
    pub job_name: String,
    pub halt_on_error: bool,
    pub interaction_mode: InteractionMode,
    pub max_errors: usize,
    pub error_count: i32,
    /// True when an error made continuing unsafe or the selected interaction
    /// mode requested an immediate stop. This differs from `end_occurred`,
    /// which is also set by a normal `\\end`.
    pub stopped_on_error: bool,
    pub diagnostics: crate::diagnostics::DiagnosticStore,
    /// Rendered diagnostics kept separate from routine TeX progress so the
    /// CLI can route them to stderr without moving successful progress there.
    pub diagnostic_output: String,
    /// Source captured for deferred expansion such as a shipout-time write.
    pub(crate) diagnostic_source_override: Option<crate::input::SourceContext>,
    /// Explicit expansion ancestry for a deferred or synthesized diagnostic.
    /// `Some(Vec::new())` deliberately suppresses irrelevant internal frames.
    pub(crate) diagnostic_trace_override: Option<Vec<CsId>>,
    /// Source of a LaTeX error printed before a terminal-input request. LaTeX
    /// reports missing packages through `\typeout` followed by `\read-1`, so
    /// the source has to survive until the read primitive reports the error.
    pub(crate) pending_terminal_error_source: Option<crate::input::SourceContext>,
    /// Macro ancestry for the token currently being processed. It survives
    /// tail expansion after the corresponding token lists have been popped.
    pub(crate) diagnostic_macro_trace: Vec<CsId>,
    /// True when the active macro chain exceeded its storage cap. Rendering
    /// inserts an ellipsis so retained frames never appear falsely adjacent.
    pub(crate) diagnostic_macro_trace_truncated: bool,
    pub(crate) diagnostic_token_from_file: bool,
    /// Suppress normal trace unwinding while a construct emitted by a macro
    /// scans physical input (for example a macro-generated definition).
    pub(crate) diagnostic_trace_hold: u16,
    pub(crate) diagnostic_source_cs: Option<CsId>,
    pub(crate) diagnostic_physical_source: Option<PhysicalTokenSource>,
    /// The physical call site is read by every parameterized macro expansion.
    /// Share the immutable bookmark so the success path performs one cheap
    /// reference-count increment rather than cloning all of its backing
    /// source handles.
    pub(crate) diagnostic_macro_call_site: Option<crate::input::SourceMark>,
    pub(crate) diagnostic_macro_call_span: usize,
    /// Source for a control sequence synthesized by an expandable primitive,
    /// keyed by the returned token so it cannot leak to a later command.
    pub(crate) diagnostic_synthetic_source: Option<(CsId, crate::input::SourceMark, usize)>,
    /// Opening locations for user-visible brace and `\\begingroup` levels.
    pub(crate) diagnostic_group_openings: Vec<(u16, crate::input::SourceMark)>,
    /// TeX applies `\\errhelp` to an explicit `\\errmessage`, rather than to
    /// unrelated engine errors that happen to follow the assignment.
    pub(crate) diagnostic_use_err_help: bool,

    // write streams
    pub write_streams: Vec<Option<std::fs::File>>,
    /// Resolved path for each open TeX output stream. `std::fs::File` does
    /// not retain a displayable path, but write failures need to name the
    /// destination that the user can fix.
    pub(crate) write_stream_paths: Vec<Option<String>>,
    pub writebuf: Vec<(u16, String)>, // pending closed-stream writes go to terminal if 16/17/18

    // hyphenation
    pub hyphen_trie: crate::hyphen::Trie,
    pub hyphen_exceptions: Vec<(String, Vec<u8>)>,
    pub par_shape: Vec<(i32, i32)>,
    /// group level of the current par_shape assignment (tex.web tracks
    /// par_shape_ptr's level through eq_define like any eqtb entry)
    pub par_shape_level: u16,
    /// e-TeX interline, club, widow, and display-widow penalty arrays.
    /// Shared slices make per-paragraph parameter snapshots allocation-free.
    pub penalty_shapes: [std::rc::Rc<[i32]>; 4],
    pub penalty_shape_levels: [u16; 4],

    // output
    pub pdf_doc: crate::pdfout::PdfDoc,
    pub out_file: Option<std::fs::File>,
    pub font_loader: crate::fontload::FontLoader,
    pub pdf_outlines: Vec<(String, String, i32)>,

    pub job_running: bool,
    pub end_occurred: bool,
    /// Set only when an executable `\\end` actually completed the job.
    pub explicit_end_seen: bool,
    /// Prevent duplicate end-of-job warnings when multiple callers finalize
    /// the same engine job.
    pub(crate) diagnostics_finished: bool,
    /// tokens dispatched by main_loop; capacity guard
    pub main_steps: u64,
    /// Expandable commands processed during this job. Unlike main_steps this
    /// also advances while get_token is searching for one unexpandable token.
    pub expansion_steps: u64,
    pub expansion_limit: u64,
    // scanning state
    pub if_stack: Vec<IfState>,
    pub(crate) pending_if_depth: Option<usize>,
    pub pushed: Vec<Token>, // lookahead pushback
    /// fill order of the last scan_dimen unit (0=normal, 1=fil, 2=fill, 3=filll)
    pub cur_fill_order: u8,
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
    /// True when the last expanded token was a parameter character protected
    /// by \unexpanded; the definition scanner must store it literally.
    pub unexpanded_parameter: bool,
    /// e-TeX \\ifincsname: \\csname nesting depth
    pub csname_depth: u32,

    pub last_macros: std::collections::VecDeque<String>,
    pub unexp_protect: usize,
    pub tok_ring: std::collections::VecDeque<(u32, u32)>,
    /// noexpand'd token pending (returned once, unexpanded)
    pub no_expand_tok: Option<Token>,
    pub align_state: i32, // & nesting balance for runaway detection
    /// A macro parameter scanner is reading at alignment brace depth zero.
    pub align_macro_arg: bool,
    pub ss_trace: Vec<String>,
    pub format_done: bool,
    pub trace_ltx: u32,
    /// \pdfpageattr / \pdfpagesattr dict bodies (global in pdfTeX)
    pub pdf_page_attr: String,
    pub pdf_page_attr_toks: Vec<Token>,
    pub pdf_pages_attr: String,
    pub pdf_pages_attr_toks: Vec<Token>,
    pub pdf_page_resources: Vec<u8>,
    pub pdf_page_resources_toks: Vec<Token>,
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
    /// Pending leader object boxes as (leader kind, surrounding box depth).
    pub leader_stack: Vec<(u8, usize)>,
    pub insert_nums: Vec<u16>,
    pub pdf_images: crate::FxHashMap<i32, PdfImageInfo>,
    pub pdf_xforms: crate::FxHashMap<i32, (i32, i32, i32)>,
    pub color_stacks: crate::FxHashMap<i32, Vec<String>>,
    pub shipout_pending: bool,
    /// box_kinds depth of the \\shipout box (tex.web box_context);
    /// inner boxes must not consume the pending shipout.
    pub shipout_depth: usize,
    pub par_page_lists: Vec<Vec<crate::boxes::Node>>,
    pub(crate) diagnostic_repeat: Option<crate::diagnostics::DiagnosticRepeat>,
    pub last_paragraph_layout: Option<crate::linebreak::ParagraphLayoutRecord>,
    pub last_pack: Option<crate::boxes::PackRecord>,
    pub read_eof: Vec<bool>, // (amount, is_hmove)
    pub read_files: Vec<Option<Box<dyn std::io::BufRead>>>,
    pub loaded_files: Vec<std::path::PathBuf>,
    /// Content identities captured when TeX actually read a disk input.
    /// Unlike end-of-job metadata, these remain correct if TeX rewrites the
    /// same auxiliary or included file later in the pass.
    pub loaded_file_digests: Vec<(std::path::PathBuf, u64, u64)>,
    /// File sizes observed by `\\pdffilesize`/`\\filesize`. These preserve
    /// the value used during expansion without paying to read file contents.
    pub loaded_file_sizes: Vec<(std::path::PathBuf, u64)>,
    /// Disk paths whose absence affected a file lookup. Dependency caches
    /// must invalidate when one of these paths later appears.
    pub missing_files: Vec<std::path::PathBuf>,
    pub out_dir: String,
    /// Optional directory for TeX-generated state (for example `.aux`,
    /// `.toc`, and files opened through `\\openout`).  When unset, output
    /// streams continue to use `out_dir`, preserving traditional pdfTeX
    /// behavior.  The PDF itself always uses `out_dir`.
    pub aux_dir: Option<std::path::PathBuf>,
    /// Permit LaTeX's first read of the managed main-job `.aux` to see an
    /// empty virtual file. Kept explicit so plain/INITEX and arbitrary aux
    /// inputs continue to report missing files.
    pub allow_missing_main_aux: bool,
    /// directory of the primary input file; relative \\input/\\openin names
    /// resolve here before falling back to the TDS (matches running TeX from
    /// the document's own directory).
    pub main_dir: Option<std::path::PathBuf>,
    pub job_ended_by_end: bool,
    pub align_preamble: Vec<crate::align::ColSpec>,
    pub align_tabskip_0: crate::boxes::Glue,
    pub align_loop_start: Option<usize>,
    pub align_rows: Vec<Vec<crate::align::Cell>>,
    pub align_col_widths: Vec<i32>,
    pub align_cur_row: Vec<crate::align::Cell>,
    pub align_cur_col: i32,
    pub align_scanning_cell: bool,
    pub(crate) align_close_reason: crate::align::AlignCloseReason,
    /// `pushed` length when the current align toklist was installed.
    /// Expansions after that point outrank the toklist; older `pushed`
    /// tokens (e.g. a \\futurelet peek) wait until the toklist finishes.
    pub align_pushed_base: usize,
    /// Brace-balance baseline of active token-list sources after the current
    /// alignment u-template completes.
    pub(crate) align_delimiter_balance_base: i32,
    /// eqtb group level after the current alignment u-template completes.
    pub(crate) align_cell_level: u16,
    /// Save-stack depth immediately before the simple group that executes a
    /// `\noalign` body. The body's closing brace ends the no-align row only
    /// when the stack returns to this exact depth.
    pub(crate) align_noalign_save_base: usize,
    /// tex.web align_state (tex.web @6745): net brace depth relative to the
    /// current alignment entry. A row delimiter ends the entry only at 0.
    /// Maintained cumulatively at token fetch (tex.web @7335/@7492); reset
    /// when a u-template finishes (tex.web @7007-7008) or an \omit cell
    /// starts (tex.web @15562); parked at 1000000 while a template plays
    /// (tex.web @15564).
    pub(crate) align_brace_depth: i32,
    /// Height/depth size target for e-TeX \middle delimiters inside the active
    /// \left...\right group.
    pub(crate) middle_delimiter_size: i32,
    pub(crate) align_is_valign: bool,
    pub align_done: bool,
    pub align_to: Option<(i32, bool)>, // \halign to/spread <dimen>: (dimen, is_spread)
    pub align_t0: crate::boxes::Glue,
    /// Outer alignment states parked while a nested \halign is active.
    /// Keeping this on the engine prevents a fatal job from leaking state to
    /// a later engine allocated at the same address.
    pub(crate) align_stack: Vec<crate::align::AlignSave>,
    /// Physical source location of the active `\halign`, retained so an EOF
    /// after an included file has been popped still points to the construct.
    pub(crate) align_origin: Option<crate::input::SourceMark>,
    pub in_output: bool,
    pub output_depth: usize,
    /// tex.web <Fire up the user's output routine> (@19925): while an
    /// output routine runs, top-level vertical appends land at a splice
    /// cursor that stays BEFORE the held-over remainder sitting in
    /// `page_list`. TeX runs the routine on a fresh `push_nest` list and
    /// <Resume the page builder> (@19937) splices that list ahead of the
    /// contribution-list remainder, so longtable's trailing
    /// `\copy\LT@head\nobreak` opens the NEXT page (the "(continued)"
    /// head), never following the chunk rows. The tuple carries
    /// (cursor, saved \prevdepth, saved \prevgraf): `mode:=-vmode; prev_depth:=ignore_depth`
    /// suppresses the head's interline glue (the page-top \topskip pad is
    /// build_page's job), and `pop_nest` restores the saved value.
    /// `None` outside the output routine. The 4th slot saves the interrupted
    /// list's `mode` (tex.web §19921 fire_up `push_nest(save_v_mode)`): the
    /// routine itself always runs in outer vertical mode, whatever mode the
    /// page fired in (mid-paragraph, mid-display, ...); `pop_nest` restores it.
    pub output_tail: Option<(usize, i32, i32, Mode)>,
    /// tex.web push_nest record for the output routine: the interrupted
    /// list's `cur_list` and `space_factor` (mode travels in `output_tail`).
    /// Saved once at the first routine dispatch, restored at `finish_output`.
    pub output_nest: Option<(Vec<crate::boxes::Node>, i32)>,
    /// Set at fire_up launch; the first `dispatch` after the firing primitive
    /// ends converts it into the active `output_tail` cursor. Post-fire appends
    /// inside the firing primitive itself stay contribution material at the tail.
    pub output_pending: bool,
    pub dead_cycles: i32,
    pub page_prev_depth: i32,
    pub page_total: i64,
    pub page_depth: i64,
    pub page_processed: usize,
    pub page_best_break: Option<usize>,
    pub page_break_penalty: i32,
    /// true cost of the carried best break (BreakSpot::carried used a
    pub page_best_cost: i64,
    pub page_best_goal: i64,
    pub page_goal: i64,
    pub page_goal_set: bool,
    /// tex.web `page_contents >= box_there`: a box or rule has already been
    /// contributed to the page under construction. Canonical page_contents
    /// is persistent engine state, never re-derived from the contribution
    /// list prefix (the list is swapped/parked by display math and paragraph
    /// capture, and its structure changes under output operations).
    pub page_box_seen: bool,
    pub page_stretch: [i64; 4],
    pub page_shrink: [i64; 4],
    /// tex.web `page_ins_head` chain: per-class insertion accounting state
    /// for the page under construction (height already placed, split status,
    /// breakpoint pointer, last/best ins-node records). Snapshotted with the
    /// best page break by `build_page` and restored by `fire_up` exactly like
    /// the carried `page_best_break`/`page_best_goal`, so incremental
    /// contribution batches keep one canonical class state.
    pub page_insertions: Vec<crate::page::PageInsState>,
    pub last_page_node_type: i32,
    pub last_page_penalty: i32,
    pub last_page_kern: i32,
    pub last_page_glue: Option<crate::boxes::Glue>,
    pub vsplat_remainder: Option<Vec<crate::boxes::Node>>,
    pub math_lists: Vec<Vec<crate::boxes::Node>>,
    /// tex.web mlist_penalties as a conversion-scope global: insert
    /// \binoppenalty/\relpenalty breakpoints after Bin/Rel atoms when
    /// converting inline TEXT math (mode>0); restored on exit
    pub math_penalties: std::cell::Cell<bool>,
    /// Monotonic identity for source-bearing math atoms. Math conversion can
    /// measure an atom more than once (notably accented scripted nuclei), so
    /// a stable id keeps `\tracinglostchars` to one warning per atom.
    /// Cheap source-mark arena for math atom ids. Keeping marks out of `Node`
    /// avoids increasing every node's size while still retaining included
    /// input after its source stack frame has closed.
    pub(crate) math_diagnostic_sources: Vec<Option<crate::input::SourceMark>>,
    pub(crate) math_diagnostic_depth: usize,
    pub(crate) reported_missing_math_atoms: crate::FxHashSet<(u64, u16, u8)>,
    pub(crate) token_vec_pool: Vec<Vec<crate::token::Token>>,
    pub current_macro: crate::token::CsId,
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
    /// Subformula boundaries: (math-list stack depth, opening position, saved mode).
    /// Nested scanners must not use or discard a surrounding list's marks.
    pub math_group_marks: Vec<(usize, usize, Mode)>,
    pub scanner_status: ScannerStatus,
    /// Semantic nest frames: mode, list, previous depth, space factor, paragraph lines.
    pub saved_lists: Vec<(Mode, Vec<crate::boxes::Node>, i32, i32, i32)>,
    /// saved state pushed by paragraph start (pops with \par, not with groups)
    pub par_saves: usize,
    /// set when a display just ended: text resumes hmode directly
    /// (tex.web resume_after_display §1194 — no \parskip, no \parindent,
    /// no \everypar); consumed by the next start_paragraph
    pub resume_after_display: bool,
    pub par_has_display: bool,
    /// set when build_page ships a page that consumed the lines of the
    /// paragraph currently being broken (tex.web soft page break inside a
    /// paragraph): the resumed partial content has NO complete line yet, so
    /// just_box/\predisplaysize must not use the stale last_par_line clone
    pub par_interrupted: bool,
    /// tex.web §1145: during init_math, the interrupted paragraph is
    /// broken into lines but build_page is deferred until AFTER push_math
    /// enters the display group (preventing output routine / math group
    /// save-level inversion).
    pub in_display_init: bool,
    /// `$$\halign$$` (amsmath align): rows+noalign stashed here instead of
    /// a packed vbox so finish_display_math can unbox them onto the page.
    pub display_halign: Option<(Vec<crate::boxes::Node>, i32)>,
    pub unless_next: bool,
    pub random_seed: i32,
    pub last_badness: i32,
    pub pdf_last_x: i32,
    pub pdf_last_y: i32,
    /// \pdflastobj / \pdflastxform / \pdflastximage / \pdflastlink /
    /// \pdflastannot: object numbers of the last allocated PDF objects.
    pub pdf_last_obj: i32,
    pub pdf_last_xform: i32,
    pub pdf_last_ximage: i32,
    pub pdf_last_ximage_pages: i32,
    pub pdf_last_link: i32,
    pub pdf_last_annot: i32,
    /// next free object number for \pdfobj-style reservations (pdfTeX
    /// reserves 1..4 for Catalog/Pages/Info/Outlines).
    pub pdf_next_obj: i32,
    /// Object numbers allocated specifically by `\pdfobj reserveobjnum` and
    /// still available for one `\pdfobj useobjnum` definition.
    pub(crate) pdf_reserved_objnums: crate::FxHashSet<i32>,
    pub pdf_match_subject: Vec<u8>,
    pub pdf_match_ranges: Vec<Option<(usize, usize)>>,
    pub marks: [Vec<Vec<Token>>; 5], // top, first, bot, splitfirst, splitbot (class-indexed)
    pub last_named_cs: Option<CsId>,
    pub align_in_noalign: bool,
    /// \\everycr already inserted for the row currently starting; stops
    /// align_start_row from re-pushing it when the post-everycr content
    /// token arrives.
    pub align_everycr_done: bool,
    pub align_cell_toks: Vec<Token>,
    /// tex.web `cur_head`/`cur_tail` (§15273): the alignment level's
    /// adjustment list. `\vadjust` material removed from each cell's
    /// hlist by the natural-width hpack (tex.web fin_col §15666-8)
    /// accumulates here in cell/source order and is spliced into the
    /// alignment vlist after the completed row (tex.web fin_row §15724).
    pub align_adjust: Vec<crate::boxes::Node>,
    /// Per-row drain of `align_adjust` (tex.web init_row §15537
    /// `cur_tail:=cur_head`): pushed by `align_finish_row`, parallel to
    /// `align_rows`; spliced raw into the alignment vlist after the row
    /// box it belongs to (tex.web fin_row §15724).
    pub align_row_adjust: Vec<Vec<crate::boxes::Node>>,
    pub after_assignment: Option<Token>,

    pub log: String,
    pub term: String,
}

impl Engine {
    pub(crate) fn append_transcript_bounded(buffer: &mut String, text: &str) {
        const MARKER: &str = "\n! Transcript truncated at the 32 MiB safety limit.\n";
        if text.is_empty() || (buffer.len() >= MAX_TERM_BYTES && buffer.ends_with(MARKER)) {
            return;
        }
        let remaining = MAX_TERM_BYTES.saturating_sub(buffer.len());
        if text.len() <= remaining {
            buffer.push_str(text);
            return;
        }
        let content_limit = MAX_TERM_BYTES.saturating_sub(MARKER.len());
        let mut old_keep = buffer.len().min(content_limit);
        while old_keep > 0 && !buffer.is_char_boundary(old_keep) {
            old_keep -= 1;
        }
        buffer.truncate(old_keep);
        let mut keep = content_limit.saturating_sub(buffer.len()).min(text.len());
        while keep > 0 && !text.is_char_boundary(keep) {
            keep -= 1;
        }
        buffer.push_str(&text[..keep]);
        buffer.push_str(MARKER);
    }

    fn truncate_transcript_bounded(buffer: &mut String) {
        const MARKER: &str = "\n! Transcript truncated at the 32 MiB safety limit.\n";
        if buffer.len() <= MAX_TERM_BYTES {
            return;
        }
        let mut keep = MAX_TERM_BYTES.saturating_sub(MARKER.len());
        while keep > 0 && !buffer.is_char_boundary(keep) {
            keep -= 1;
        }
        buffer.truncate(keep);
        Self::append_transcript_bounded(buffer, MARKER);
    }

    pub(crate) fn append_term(&mut self, text: &str) {
        if self.interaction_mode != InteractionMode::Batch {
            Self::append_transcript_bounded(&mut self.term, text);
        }
    }

    pub(crate) fn append_diagnostic(&mut self, text: &str) {
        if self.interaction_mode != InteractionMode::Batch {
            Self::append_transcript_bounded(&mut self.diagnostic_output, text);
        }
    }

    pub(crate) fn append_log(&mut self, text: &str) {
        Self::append_transcript_bounded(&mut self.log, text);
    }

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
    fn rss_limit_bytes() -> u64 {
        if cfg!(test) {
            return match std::env::var("TEX_MEM_LIMIT_MIB") {
                Ok(s) => s
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .map(|m| m.saturating_mul(1 << 20))
                    .unwrap_or(0),
                Err(_) => 0,
            };
        }
        match std::env::var("TEX_MEM_LIMIT_MIB") {
            Ok(s) if s.trim() == "0" => 0,
            Ok(s) => s
                .trim()
                .parse::<u64>()
                .ok()
                .map(|m| m.saturating_mul(1 << 20))
                .unwrap_or(DEFAULT_RSS_LIMIT),
            Err(_) => DEFAULT_RSS_LIMIT,
        }
    }

    fn resident_bytes() -> u64 {
        let Ok(buf) = std::fs::read_to_string("/proc/self/statm") else {
            return 0;
        };
        let pages = buf
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        pages.saturating_mul(4096)
    }

    /// Abort a runaway job before it can OOM the host.
    pub fn capacity_exceeded(&mut self) -> bool {
        if self.structural_capacity_exceeded() {
            return true;
        }
        if self.main_steps > MAX_MAIN_STEPS {
            self.capacity_error(&format!(
                "TeX capacity exceeded, sorry [main memory steps={}]",
                MAX_MAIN_STEPS
            ));
            return true;
        }
        if self.page_list.len() > MAX_PAGE_LIST || self.cur_list.len() > MAX_PAGE_LIST {
            self.capacity_error("TeX capacity exceeded, sorry [page/list size]");
            return true;
        }
        if self.term.len() > MAX_TERM_BYTES
            || self.log.len() > MAX_TERM_BYTES
            || self.diagnostic_output.len() > MAX_TERM_BYTES
        {
            Self::truncate_transcript_bounded(&mut self.term);
            Self::truncate_transcript_bounded(&mut self.log);
            Self::truncate_transcript_bounded(&mut self.diagnostic_output);
            self.capacity_error("TeX capacity exceeded, sorry [transcript size]");
            return true;
        }
        let lim = Self::rss_limit_bytes();
        if lim > 0 && Self::resident_bytes() > lim {
            self.capacity_error(&format!(
                "TeX capacity exceeded, sorry [memory {}MiB]",
                lim >> 20
            ));
            return true;
        }
        false
    }

    /// Check limits that can be crossed while fetching or dispatching one
    /// token. Unlike RSS accounting, these checks are cheap enough to run at
    /// every main-control boundary.
    pub(crate) fn structural_capacity_exceeded(&mut self) -> bool {
        if self.cs.capacity_exceeded() {
            self.capacity_error(&format!(
                "TeX capacity exceeded, sorry [hash size={}]",
                crate::token::MAX_HASH_NAMES
            ));
            return true;
        }
        if self.eqtb.save_stack_capacity_exceeded() {
            self.capacity_error(&format!(
                "TeX capacity exceeded, sorry [save size={}]",
                crate::eqtb::MAX_SAVE_STACK
            ));
            return true;
        }
        if self.eqtb.group_level_capacity_exceeded() {
            self.capacity_error(&format!(
                "TeX capacity exceeded, sorry [grouping levels={}]",
                crate::eqtb::MAX_GROUP_LEVEL
            ));
            return true;
        }
        false
    }

    /// Set both representations of TeX's interaction mode. The eqtb value is
    /// exposed as the readable e-TeX \interactionmode parameter.
    pub fn set_interaction_mode(&mut self, mode: InteractionMode) {
        if self.interaction_mode != mode {
            self.flush_diagnostic_repeats();
            self.interaction_mode = mode;
            self.eqtb.set_runtime_interaction_mode(mode.number());
        }
    }

    /// Apply a completed \interactionmode assignment at the main-control
    /// boundary, while its source token is still available for diagnostics.
    pub(crate) fn apply_pending_interaction_mode(&mut self) {
        let Some(value) = self.eqtb.take_pending_interaction_mode() else {
            return;
        };
        if let Some(mode) = InteractionMode::from_number(value) {
            if self.interaction_mode != mode {
                self.flush_diagnostic_repeats();
                self.interaction_mode = mode;
            }
        } else {
            self.error(&format!(
                "Bad interaction mode ({value}); expected 0 (batch), 1 (nonstop), 2 (scroll), or 3 (error stop); mode left unchanged"
            ));
        }
    }

    fn capacity_error(&mut self, msg: &str) {
        self.fatal_error(msg);
    }
    #[inline]
    pub fn partoken_id(&self) -> CsId {
        let raw = self.eqtb.int_params[crate::prim::IntParam::PartokenNameCs.idx() as usize];
        if raw >= 0 && (raw as usize) < self.cs.len() {
            raw as CsId
        } else {
            self.ids.par
        }
    }

    #[inline]
    pub fn is_partoken(&self, t: Token) -> bool {
        t == crate::input::PAR_END || (t.is_cs() && t.cs_id() == self.partoken_id())
    }

    pub fn new(ini_mode: bool) -> Engine {
        let mut cs = CsTable::new();
        let par = cs.intern(b"par");
        let e = Engine {
            ids: Ids {
                par,
                cs_escape: b'\\',
            },
            cs,
            eqtb: Eqtb::new(ini_mode),
            primitive_names: crate::FxHashMap::default(),
            input: InputStack::new(),
            par_saves: 0,
            resume_after_display: false,
            par_has_display: false,
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
            pdf_images: crate::FxHashMap::default(),
            pdf_xforms: crate::FxHashMap::default(),
            color_stacks: crate::FxHashMap::default(),
            prev_graf: 0,
            after_token: false,
            definable_cs_recovery_count: 0,
            ini_mode,
            format_name: String::new(),
            job_name: String::new(),
            halt_on_error: false,
            interaction_mode: InteractionMode::ErrorStop,
            max_errors: DEFAULT_MAX_ERRORS,
            error_count: 0,
            stopped_on_error: false,
            diagnostics: crate::diagnostics::DiagnosticStore::default(),
            diagnostic_output: String::new(),
            diagnostic_source_override: None,
            diagnostic_trace_override: None,
            pending_terminal_error_source: None,
            diagnostic_macro_trace: Vec::with_capacity(20),
            diagnostic_macro_trace_truncated: false,
            diagnostic_token_from_file: false,
            diagnostic_trace_hold: 0,
            diagnostic_source_cs: None,
            diagnostic_physical_source: None,
            diagnostic_macro_call_site: None,
            diagnostic_macro_call_span: 1,
            diagnostic_synthetic_source: None,
            diagnostic_group_openings: Vec::new(),
            diagnostic_use_err_help: false,
            write_streams: (0..16).map(|_| None).collect(),
            write_stream_paths: (0..16).map(|_| None).collect(),
            writebuf: Vec::new(),
            hyphen_trie: crate::hyphen::Trie::new(),
            hyphen_exceptions: Vec::new(),
            par_shape: Vec::new(),
            par_shape_level: crate::eqtb::LEVEL_ONE,
            penalty_shapes: std::array::from_fn(|_| std::rc::Rc::from([])),
            penalty_shape_levels: [crate::eqtb::LEVEL_ONE; 4],
            pdf_doc: crate::pdfout::PdfDoc::new(),
            out_file: None,
            font_loader: crate::fontload::FontLoader::new(),
            pdf_outlines: Vec::new(),
            job_running: true,
            end_occurred: false,
            explicit_end_seen: false,
            diagnostics_finished: false,
            main_steps: 0,
            expansion_steps: 0,
            expansion_limit: std::env::var("TEX_EXPANSION_LIMIT")
                .ok()
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(DEFAULT_EXPANSION_LIMIT),
            if_stack: Vec::new(),
            pending_if_depth: None,
            pushed: Vec::new(),
            token_vec_pool: Vec::with_capacity(512),
            cur_fill_order: 0,
            def_prefix: Vec::new(),
            global_flag: false,
            long_flag: false,
            outer_flag: false,
            protected_flag: false,
            expand_protected: 0,
            in_expanded_scan: false,
            unexpanded_parameter: false,
            csname_depth: 0,
            last_macros: std::collections::VecDeque::new(),
            unexp_protect: 0,
            tok_ring: std::collections::VecDeque::new(),

            no_expand_tok: None,
            align_state: 0,
            align_macro_arg: false,
            ss_trace: Vec::new(),
            format_done: false,
            trace_ltx: 0,
            pdf_page_attr: String::new(),
            pdf_page_attr_toks: Vec::new(),
            pdf_pages_attr: String::new(),
            pdf_pages_attr_toks: Vec::new(),
            pdf_page_resources: Vec::new(),
            pdf_page_resources_toks: Vec::new(),
            pdf_last_obj: 0,
            pdf_last_xform: 0,
            pdf_last_ximage: 0,
            pdf_last_ximage_pages: 0,
            pdf_next_obj: 5,
            pdf_reserved_objnums: crate::FxHashSet::default(),
            pdf_last_link: 0,
            pdf_last_annot: 0,
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
            leader_stack: Vec::new(),
            insert_nums: Vec::new(),

            shipout_pending: false,
            shipout_depth: usize::MAX,
            par_page_lists: Vec::new(),
            diagnostic_repeat: None,
            last_paragraph_layout: None,
            last_pack: None,
            read_eof: Vec::new(),
            read_files: Vec::new(),
            loaded_files: Vec::new(),
            loaded_file_digests: Vec::new(),
            loaded_file_sizes: Vec::new(),
            missing_files: Vec::new(),
            out_dir: String::new(),
            aux_dir: None,
            allow_missing_main_aux: false,
            main_dir: None,
            job_ended_by_end: false,
            align_preamble: Vec::new(),
            align_tabskip_0: crate::boxes::Glue::zero(),
            align_loop_start: None,
            align_rows: Vec::new(),
            align_adjust: Vec::new(),
            align_row_adjust: Vec::new(),
            align_cell_toks: Vec::new(),
            align_col_widths: Vec::new(),
            align_cur_row: Vec::new(),
            align_cur_col: 0,
            align_in_noalign: false,
            align_everycr_done: false,
            align_scanning_cell: false,
            align_close_reason: crate::align::AlignCloseReason::default(),
            align_pushed_base: 0,
            align_delimiter_balance_base: 0,
            align_cell_level: 0,
            align_noalign_save_base: 0,
            align_brace_depth: 0,
            middle_delimiter_size: 0,
            align_is_valign: false,
            align_to: None,
            align_t0: crate::boxes::Glue::zero(),
            align_done: false,
            align_stack: Vec::new(),
            align_origin: None,
            in_output: false,
            output_depth: 0,
            output_tail: None,
            output_nest: None,
            output_pending: false,
            dead_cycles: 0,
            page_prev_depth: -1000 * 65536,
            in_display_init: false,
            display_halign: None,
            page_total: 0,
            page_depth: 0,
            page_processed: 0,
            page_best_break: None,
            page_insertions: Vec::new(),
            page_break_penalty: 0,
            page_best_cost: 0,
            page_best_goal: 0x3FFF_FFFF,
            page_goal: 0x3FFF_FFFF,
            page_goal_set: false,
            page_box_seen: false,
            page_stretch: [0; 4],
            page_shrink: [0; 4],
            last_page_node_type: -1,
            last_page_penalty: 0,
            last_page_kern: 0,
            last_page_glue: None,
            vsplat_remainder: None,
            pdf_match_subject: Vec::new(),
            pdf_match_ranges: Vec::new(),
            math_lists: Vec::new(),
            math_penalties: std::cell::Cell::new(false),
            math_diagnostic_sources: Vec::new(),
            math_diagnostic_depth: 0,
            reported_missing_math_atoms: crate::FxHashSet::default(),
            pre_display_size: -0x3FFF_FFFF,
            pre_display_l: 0,
            last_par_line: None,
            next_par_widow: None,
            pending_display_formula: None,
            eqno_leqno: None,
            math_group_marks: Vec::new(),
            pre_display_s: 0,
            current_macro: 0,
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
            random_seed: 123456789,
        };
        e
    }

    pub fn init_primitives(&mut self) {
        use Prim::*;
        let def = |name: &'static [u8], p: Prim, e: &mut Engine| {
            let id = e.cs.intern(name);
            e.primitive_names.entry(p.code()).or_insert(name);
            e.eqtb.assign(id, Equiv::Prim(p), true);
        };
        macro_rules! d {
            ($e:expr, $name:literal, $p:expr) => {
                def($name, $p, $e)
            };
        }
        let eng = self;
        d!(eng, b"relax", Relax);
        d!(eng, b" ", ExSpace);
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
            (b"pdfoptionpdfminorversion", IntParam::PdfMinorVersion),
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
            (b"pdfcompresslevel", IntParam::PdfCompressLevel),
            (b"pdfgentounicode", IntParam::PdfGenToUnicode),
            (b"paperquality", IntParam::PaperQuality),
            (b"globaldefs", IntParam::GlobalDefs),
            (b"spacefactor", IntParam::SpaceFactor),
            (b"holdinginserts", IntParam::HoldingInserts),
            (b"pdfinfoomitdate", IntParam::PdfInfoOmitDate),
            (b"pdfsuppressptexinfo", IntParam::PdfSuppressPtexInfo),
            (b"partokencontext", IntParam::PartokenContext),
            (b"ignoreprimitiveerror", IntParam::IgnorePrimitiveError),
            (
                b"pdfsuppresswarningpagegroup",
                IntParam::PdfSuppressWarningPageGroup,
            ),
            (b"pdfadjustinterwordglue", IntParam::PdfAdjustInterwordGlue),
            (b"pdfprependkern", IntParam::PdfPrependKern),
            (b"pdfappendkern", IntParam::PdfAppendKern),
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
            (b"pdftrailerid", ToksParam::PdfTrailerId),
        ];
        for (n, p) in toksnames {
            let id = eng.cs.intern(n);
            eng.eqtb.assign(id, Equiv::Prim(Prim::ToksP(*p)), true);
        }
        d!(eng, b"font", Font);
        d!(eng, b"fontname", FontName);
        d!(eng, b"fontid", FontIdPrim);
        d!(eng, b"fontcharwd", FontCharWd);
        d!(eng, b"fontcharht", FontCharHt);
        d!(eng, b"fontchardp", FontCharDp);
        d!(eng, b"fontcharic", FontCharIc);
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
        d!(eng, b"/", ItalicCorrection);
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
        d!(eng, b"pagediscards", PageDiscards);
        d!(eng, b"splitdiscards", SplitDiscards);

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
        d!(eng, b"discretionary", Discretionary);
        d!(eng, b"insert", Insert);
        d!(eng, b"vadjust", VAdjust);
        d!(eng, b"mark", MarkPrim);
        d!(eng, b"marks", MarksClass);
        d!(eng, b"topmark", TopMark);
        d!(eng, b"topmarks", TopMarksClass);
        d!(eng, b"firstmark", FirstMark);
        d!(eng, b"firstmarks", FirstMarksClass);
        d!(eng, b"botmark", BotMark);
        d!(eng, b"botmarks", BotMarksClass);
        d!(eng, b"splitfirstmark", SplitFirstMark);
        d!(eng, b"splitfirstmarks", SplitFirstMarksClass);
        d!(eng, b"splitbotmark", SplitBotMark);
        d!(eng, b"splitbotmarks", SplitBotMarksClass);
        d!(eng, b"shipout", ShipOut);
        d!(eng, b"char", Char);
        d!(eng, b"accent", Accent);
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
        d!(eng, b"pdfmatch", PdfMatch);
        d!(eng, b"pdflastmatch", PdfLastMatch);
        d!(eng, b"nonscript", NonScript);
        d!(eng, b"pdfmapfile", PdfMapFile);
        d!(eng, b"pdfmapline", PdfMapLine);
        d!(eng, b"pdfglyphtounicode", PdfGlyphToUnicode);
        d!(eng, b"pdffontattr", PdfFontAttr);
        d!(eng, b"pdffontexpand", PdfFontExpand);
        d!(eng, b"pdfnoligatures", PdfNoLigatures);
        d!(eng, b"letterspacefont", LetterspaceFont);
        d!(eng, b"efcode", EfCode);
        d!(eng, b"lpcode", LpCode);
        d!(eng, b"rpcode", RpCode);
        d!(eng, b"leftmarginkern", LeftMarginKern);
        d!(eng, b"rightmarginkern", RightMarginKern);
        d!(eng, b"tagcode", TagCode);
        d!(eng, b"knbscode", KnBsCode);
        d!(eng, b"stbscode", StBsCode);
        d!(eng, b"shbscode", ShBsCode);
        d!(eng, b"knbccode", KnBcCode);
        d!(eng, b"knaccode", KnAcCode);
        d!(eng, b"pdffontsize", PdfFontSize);
        d!(eng, b"pdftexbanner", PdfBanner);
        d!(eng, b"partokenname", PartokenName);
        d!(eng, b"pdfxform", PdfXForm);
        d!(eng, b"pdfximage", PdfXImage);
        d!(eng, b"pdfrefxform", PdfRefXForm);
        d!(eng, b"pdfrefximage", PdfRefXImage);
        d!(eng, b"pdfpagesattr", PdfPagesAttr);
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
        d!(eng, b"pdflastximagepages", PdfLastXImagePages);
        d!(eng, b"pdfximagebbox", PdfXImageBBox);
        d!(eng, b"pdflastlink", PdfLastLink);
        d!(eng, b"pdflastannot", PdfLastAnnot);
        d!(eng, b"pdffilesize", PdfFileSize);
        d!(eng, b"pdfmdfivesum", PdfMdFiveSum);
        d!(eng, b"pdffilemoddate", PdfFileModDate);
        d!(eng, b"pdfcreationdate", PdfCreationDate);
        d!(eng, b"pdffiledump", PdfFileDump);
        d!(eng, b"pdfstrcmp", PdfStrCmp);
        d!(eng, b"pdfshellescape", PdfShellEscape);
        d!(eng, b"pdfelapsedtime", PdfElapsedTime);
        d!(eng, b"pdfresettimer", PdfResetTimer);
        d!(eng, b"pdfuniformdeviate", PdfUniformDeviate);
        d!(eng, b"pdfnormaldeviate", PdfNormalDeviate);
        d!(eng, b"pdfrandomseed", PdfRandomSeed);
        d!(eng, b"pdfsetrandomseed", PdfSetRandomSeed);
        d!(eng, b"randomseed", PdfRandomSeed);
        d!(eng, b"setrandomseed", PdfSetRandomSeed);
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
        eng.eqtb.glue_params[GlueParam::LineSkip.idx() as usize] = crate::boxes::Glue::new(65536);
        // plain.tex / fontmath.ltx: \thinmuskip=3mu, \medmuskip=4mu plus 2mu
        // minus 4mu, \thickmuskip=5mu plus 5mu — stored mu-denominated
        // (tex.web §431); math_glue converts with the current em at use.
        eng.eqtb.glue_params[GlueParam::ThinMuSkip.idx() as usize] =
            crate::boxes::Glue::new(3 * 65536);
        eng.eqtb.glue_params[GlueParam::MedMuSkip.idx() as usize] = crate::boxes::Glue {
            width: 4 * 65536,
            stretch: 2 * 65536,
            shrink: 4 * 65536,
            stretch_order: 0,
            shrink_order: 0,
        };
        eng.eqtb.glue_params[GlueParam::ThickMuSkip.idx() as usize] = crate::boxes::Glue {
            width: 5 * 65536,
            stretch: 5 * 65536,
            shrink: 0,
            stretch_order: 0,
            shrink_order: 0,
        };
        eng.eqtb.int_params[IntParam::EndLineChar.idx() as usize] = 13;
        eng.eqtb.int_params[IntParam::EscapeChar.idx() as usize] = 92;
        eng.eqtb.int_params[IntParam::NewLineChar.idx() as usize] = -1;
        eng.eqtb.int_params[IntParam::MaxDeadCycles.idx() as usize] = 25;
        eng.eqtb.int_params[IntParam::Mag.idx() as usize] = 1000;
        eng.eqtb.int_params[IntParam::Tolerance.idx() as usize] = 200;
        eng.eqtb.int_params[IntParam::Pretolerance.idx() as usize] = 100;
        eng.eqtb.int_params[IntParam::HangAfter.idx() as usize] = 1;
        eng.eqtb.int_params[IntParam::ErrorContextLines.idx() as usize] = 5;
        // INITEX starts with \tracinglostchars=0. Loaded formats normally set
        // it to 1; raw/plain callers can opt in after selecting a real font.
        eng.eqtb.int_params[IntParam::TracingLostChars.idx() as usize] = 0;
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
        eng.eqtb.int_params[IntParam::PartokenNameCs.idx() as usize] = eng.ids.par as i32;
        // eTeX extended-mode identity (pgf/pgfkeys probe \eTeXrevision).
        // NOTE: XeTeX primitives are deliberately NOT registered: packages
        // (iftex, hyperref, pgf) select the pdfTeX driver only when the
        // \XeTeX* names are undefined, and this engine is pdfTeX-compatible.
        d!(eng, b"eTeXrevision", EtxRevision);
        d!(eng, b"interlinepenalties", InterLinePenalties);
        d!(eng, b"clubpenalties", ClubPenalties);
        d!(eng, b"widowpenalties", WidowPenalties);
        d!(eng, b"displaywidowpenalties", DisplayWidowPenalties);
        d!(eng, b"pdfpageresources", PdfPageResources);
        // plain.tex paper: keep the page builder from firing on every box
        let sp_in: i32 = 4736287;
        eng.eqtb.dim_params[DimParam::HSize.idx() as usize] = (sp_in as i64 * 13 / 2) as i32;
        eng.eqtb.dim_params[DimParam::VSize.idx() as usize] = (sp_in as i64 * 89 / 10) as i32;
        // pdfTeX INITEX defaults: the origin is one inch, while the page
        // dimensions remain unset until the format's pdftexconfig.tex runs.
        eng.eqtb.dim_params[DimParam::PdfHOrigin.idx() as usize] = sp_in;
        eng.eqtb.dim_params[DimParam::PdfVOrigin.idx() as usize] = sp_in;
        eng.eqtb.dim_params[DimParam::PdfPageWidth.idx() as usize] = 0;
        eng.eqtb.dim_params[DimParam::PdfPageHeight.idx() as usize] = 0;
    }
    pub fn pop_group(&mut self) -> crate::eqtb::LevelType {
        let closing_level = self.eqtb.cur_level;
        let mut ag = Vec::new();
        let mut ps = None;
        let mut penalty_shapes = Vec::new();
        let ty = self
            .eqtb
            .pop_level_full(&mut ag, &mut ps, &mut penalty_shapes);
        // Shape pointers are level-tracked like eqtb entries. A later global
        // assignment suppresses restoration from an older local save item.
        if let Some((old, old_lvl)) = ps {
            if self.par_shape_level > crate::eqtb::LEVEL_ONE {
                self.par_shape = old;
                self.par_shape_level = old_lvl;
            }
        }
        for (kind, old, old_lvl) in penalty_shapes {
            let kind = kind as usize;
            if self.penalty_shape_levels[kind] > crate::eqtb::LEVEL_ONE {
                self.penalty_shapes[kind] = old;
                self.penalty_shape_levels[kind] = old_lvl;
            }
        }
        for t in ag {
            self.push_token(t);
        }
        self.forget_group_opening(closing_level);
        ty
    }

    /// tex.web eq_define(par_shape_loc): level-tracked \parshape assignment —
    /// a local assign at a deeper group pushes the old value; the normal_
    /// paragraph clear is local too, so LaTeX's `{\@@par}` list wrapper rolls
    /// it back and the shape persists across items
    pub fn assign_par_shape(&mut self, new: Vec<(i32, i32)>, global: bool) {
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

    pub(crate) fn assign_penalty_shape(&mut self, primitive: Prim, values: Vec<i32>, global: bool) {
        let kind = primitive
            .penalty_shape_index()
            .expect("penalty shape primitive");
        let values = std::rc::Rc::<[i32]>::from(values);
        if global {
            self.penalty_shapes[kind] = values;
            self.penalty_shape_levels[kind] = crate::eqtb::LEVEL_ONE;
            return;
        }
        let level = self.eqtb.cur_level;
        if self.penalty_shape_levels[kind] < level {
            let old = std::mem::replace(&mut self.penalty_shapes[kind], values);
            let old_level = self.penalty_shape_levels[kind];
            self.eqtb
                .save_stack
                .push(crate::eqtb::SaveItem::PenaltyShape(
                    kind as u8, old, old_level,
                ));
            self.penalty_shape_levels[kind] = level;
        } else {
            self.penalty_shapes[kind] = values;
        }
    }

    pub(crate) fn penalty_shape_value(&self, primitive: Prim, index: i32) -> i32 {
        let kind = primitive
            .penalty_shape_index()
            .expect("penalty shape primitive");
        let values = &self.penalty_shapes[kind];
        if index == 0 {
            return values.len() as i32;
        }
        if index < 0 || values.is_empty() {
            return 0;
        }
        values[(index as usize - 1).min(values.len() - 1)]
    }
    /// tex.web box_context: nest \setbox so an inner \setbox inside
    /// \shipout\vbox{\setbox...} cannot clobber the outer target. The
    /// pending \global prefix travels with the target.
    pub fn park_setbox(&mut self, idx: u16) {
        let g = self.take_global();
        self.park_setbox_with_global(idx, g);
    }
    /// Park a \setbox target after its prefix state has already been
    /// consumed. This avoids evaluating \globaldefs twice around scanners.
    pub fn park_setbox_with_global(&mut self, idx: u16, global: bool) {
        self.setbox_stack.push((
            self.setbox_target.take(),
            self.setbox_depth,
            self.setbox_global,
        ));
        self.setbox_target = Some(idx);
        self.setbox_depth = self.box_kinds.len();
        self.setbox_global = global;
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
            self.push_token(t);
        }
    }
    /// tex.web @7028 (back_input): putting a token back on the input reverses
    /// its align_state contribution (decr for left brace, incr for right brace).
    /// When the token is later fetched again by raw_token(), the contribution
    /// is re-applied, guaranteeing exact balance.
    #[inline(always)]
    pub fn push_token(&mut self, t: Token) {
        if t.0 < 0x8000_0000 && !t.is_cs() {
            let cc = (t.0 >> 24) as u8;
            if cc == 1 {
                self.align_brace_depth = self.align_brace_depth.saturating_sub(1);
                // push_token adjusts align_brace_depth
            } else if cc == 2 {
                self.align_brace_depth = self.align_brace_depth.saturating_add(1);
            }
        }
        self.pushed.push(t);
    }
    #[inline]
    pub fn is_right_brace(&self, t: Token) -> bool {
        if t.is_char() {
            t.cc() == 2
        } else if t.is_cs() {
            matches!(
                self.eqtb.resolve(t.cs_id()),
                Some(crate::eqtb::Equiv::Prim(crate::prim::Prim::EGroup))
            ) || matches!(
                self.eqtb.resolve(t.cs_id()),
                Some(crate::eqtb::Equiv::CharTok(v)) if Token(*v).cc() == 2
            )
        } else {
            false
        }
    }
    #[inline]
    pub fn is_left_brace(&self, t: Token) -> bool {
        if t.is_char() {
            t.cc() == 1
        } else if t.is_cs() {
            matches!(
                self.eqtb.resolve(t.cs_id()),
                Some(crate::eqtb::Equiv::Prim(crate::prim::Prim::BGroup))
            ) || matches!(
                self.eqtb.resolve(t.cs_id()),
                Some(crate::eqtb::Equiv::CharTok(v)) if Token(*v).cc() == 1
            )
        } else {
            false
        }
    }
    #[inline]
    pub fn is_macro_param(&self, t: Token) -> bool {
        if t.is_char() {
            t.cc() == 6
        } else if t.is_cs() {
            matches!(
                self.eqtb.resolve(t.cs_id()),
                Some(crate::eqtb::Equiv::CharTok(v)) if Token(*v).cc() == 6
            )
        } else {
            false
        }
    }
    /// tex.web page_contents == empty: true when no box or rule has been contributed to the current page.
    #[inline]
    pub fn page_contents_empty(&self) -> bool {
        !self.page_list.iter().any(|n| {
            matches!(
                n,
                crate::boxes::Node::Box { .. } | crate::boxes::Node::Rule { .. }
            )
        })
    }
}

#[derive(Clone, Debug)]
pub struct PdfImageInfo {
    pub path: String,
    pub used: bool,
    pub width: i32,
    pub height: i32,
    pub depth: i32,
    /// true when the file was imported as a PDF Form XObject during scan:
    /// the image bytes are already embedded, so shipping must not re-read it.
    pub embedded: bool,
    pub bbox: [i32; 4],
}
#[cfg(test)]
mod capacity_tests {
    use super::*;

    #[test]
    fn interaction_mode_parameter_updates_the_runtime_mode_for_all_valid_values() {
        for (value, expected) in [
            (0, InteractionMode::Batch),
            (1, InteractionMode::Nonstop),
            (2, InteractionMode::Scroll),
            (3, InteractionMode::ErrorStop),
        ] {
            let mut eng = Engine::new(false);
            eng.init_primitives();
            eng.set_interaction_mode(InteractionMode::Nonstop);
            eng.input.push_file(
                "mode.tex".to_string(),
                format!("\\interactionmode={value}\\end\n").into_bytes(),
            );

            eng.run();

            assert_eq!(eng.interaction_mode, expected, "value={value}");
            assert_eq!(
                eng.eqtb.int_params[IntParam::InteractionMode.idx() as usize],
                value,
                "value={value}"
            );
            assert_eq!(eng.error_count, 0, "value={value}: {}", eng.term);
        }
    }

    #[test]
    fn invalid_interaction_mode_is_a_located_error_and_keeps_the_current_mode() {
        let mut eng = Engine::new(false);
        eng.init_primitives();
        eng.set_interaction_mode(InteractionMode::Nonstop);
        eng.input.push_file(
            "bad-mode.tex".to_string(),
            b"\\interactionmode=9\\relax\\end\n".to_vec(),
        );

        eng.run();

        assert_eq!(eng.interaction_mode, InteractionMode::Nonstop);
        assert_eq!(
            eng.eqtb.int_params[IntParam::InteractionMode.idx() as usize],
            1
        );
        assert_eq!(eng.error_count, 1, "{}", eng.term);
        let diagnostic = eng.diagnostics.last().expect("interaction diagnostic");
        assert_eq!(
            diagnostic.message,
            "Bad interaction mode (9); expected 0 (batch), 1 (nonstop), 2 (scroll), or 3 (error stop); mode left unchanged"
        );
        let primary = diagnostic.primary.as_ref().expect("source location");
        assert_eq!(primary.name, "bad-mode.tex");
        assert_eq!(primary.line, 1);
        assert_eq!(primary.column, 18);
    }

    #[test]
    fn transcript_capacity_truncation_is_safe_inside_a_utf8_character() {
        let mut eng = Engine::new(false);
        eng.set_interaction_mode(InteractionMode::Nonstop);
        eng.log = "x".repeat(MAX_TERM_BYTES - 1);
        eng.log.push('界');

        assert!(eng.capacity_exceeded());
        assert!(eng.log.is_char_boundary(eng.log.len()));
        assert!(eng.log.len() <= MAX_TERM_BYTES);
        assert!(eng
            .log
            .contains("Transcript truncated at the 32 MiB safety limit"));
    }

    #[test]
    fn append_at_exact_transcript_limit_replaces_the_tail_with_a_marker() {
        let mut eng = Engine::new(false);
        eng.log = "x".repeat(MAX_TERM_BYTES);

        eng.append_log("a later diagnostic");

        assert_eq!(eng.log.len(), MAX_TERM_BYTES);
        assert!(eng
            .log
            .ends_with("\n! Transcript truncated at the 32 MiB safety limit.\n"));
    }

    #[test]
    fn transcript_text_cannot_forge_the_truncation_latch() {
        let mut eng = Engine::new(false);
        eng.log = "user text\n! Transcript truncated at the 32 MiB safety limit.\n".to_string();

        eng.append_log("a later diagnostic");

        assert!(eng.log.ends_with("a later diagnostic"));
        assert!(eng.log.len() < MAX_TERM_BYTES);
    }

    #[test]
    fn maximum_group_nesting_becomes_a_located_capacity_diagnostic() {
        let mut eng = Engine::new(false);
        eng.init_primitives();
        eng.set_interaction_mode(InteractionMode::Nonstop);
        eng.input.push_file(
            "groups.tex".to_string(),
            vec![b'{'; crate::eqtb::MAX_GROUP_LEVEL as usize],
        );

        eng.run();

        assert_eq!(eng.eqtb.cur_level, crate::eqtb::MAX_GROUP_LEVEL);
        assert!(eng.stopped_on_error);
        assert_eq!(eng.error_count, 1, "{}", eng.term);
        let diagnostic = eng.diagnostics.last().expect("capacity diagnostic");
        assert_eq!(
            diagnostic.message,
            "TeX capacity exceeded, sorry [grouping levels=65535]"
        );
        let primary = diagnostic.primary.as_ref().expect("source location");
        assert_eq!(primary.name, "groups.tex");
        assert_eq!(primary.line, 1);
        assert_eq!(primary.column, crate::eqtb::MAX_GROUP_LEVEL as usize);
    }

    #[test]
    fn hash_limit_becomes_a_fatal_engine_diagnostic() {
        let mut eng = Engine::new(true);
        eng.init_primitives();
        eng.cs.force_capacity_exceeded_for_test();

        assert!(eng.structural_capacity_exceeded());
        assert!(eng.stopped_on_error);
        assert!(eng.end_occurred);
        assert_eq!(
            eng.diagnostics
                .last()
                .map(|diagnostic| diagnostic.message.as_str()),
            Some("TeX capacity exceeded, sorry [hash size=2097152]")
        );
    }

    #[test]
    fn save_stack_limit_becomes_a_fatal_engine_diagnostic() {
        let mut eng = Engine::new(true);
        eng.init_primitives();
        eng.eqtb.save_stack.resize(
            crate::eqtb::MAX_SAVE_STACK,
            crate::eqtb::SaveItem::AfterGroup(Token::space()),
        );
        eng.eqtb
            .push_save(crate::eqtb::SaveItem::AfterGroup(Token::letter(b'x')));

        assert!(eng.structural_capacity_exceeded());
        assert!(eng.stopped_on_error);
        assert!(eng.end_occurred);
        assert_eq!(
            eng.diagnostics
                .last()
                .map(|diagnostic| diagnostic.message.as_str()),
            Some("TeX capacity exceeded, sorry [save size=100000]")
        );
    }

    #[test]
    fn step_limit_aborts() {
        let mut eng = Engine::new(true);
        eng.init_primitives();
        eng.main_steps = MAX_MAIN_STEPS + 1;
        assert!(eng.capacity_exceeded());
        assert!(eng.end_occurred);
        assert!(eng.error_count > 0);
    }

    #[test]
    fn page_list_limit_aborts() {
        let mut eng = Engine::new(true);
        eng.init_primitives();
        eng.page_list
            .resize(MAX_PAGE_LIST + 1, crate::boxes::Node::Penalty(0));
        assert!(eng.capacity_exceeded());
        assert!(eng.end_occurred);
    }
}
