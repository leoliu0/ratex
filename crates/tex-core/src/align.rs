//! \halign: preamble scanning, cell collection, two-phase width computation
//! and row assembly (supports \omit, \span, \noalign, \everycr, \crcr).
//!
//! Architecture (tex.web "alignment" chapter, adapted to this engine's
//! replay-based token flow):
//!
//! * `begin_halign` scans `{ <preamble> \cr` into `align_preamble` entries
//!   (u part = tokens before #, v part = tokens after #, span = extra grid
//!   columns absorbed by preamble \span). A bare `&` in the preamble (LaTeX
//!   tabular emits one between column templates) is silently dropped.
//! * A cell is an ordinary group (box_kinds marker CELL_GROUP_KIND) in
//!   restricted horizontal mode. `align_start_cell` peeks one expanding
//!   token: if it is \omit the template is skipped, otherwise the u part is
//!   pushed as an input source and the peeked token re-queued underneath it.
//! * `&` (`align_tab`) and `\cr` (`align_cr`) close the cell by pushing
//!   `[v part..., \crcr-token]` (the v part is skipped when \omit was
//!   given). The close stream is marked PH_CLOSE; when the \crcr sentinel
//!   dispatches, the cell is finished synchronously: packed to its natural
//!   width, stored in the row, and the next cell started / row ended.
//!   A \crcr token is used (not a magic sentinel) so that scanner
//!   lookaheads across the close stream push it back instead of acting on
//!   it, exactly like tex.web's frozen \cr.
//!   After a row ends, the next token is inspected: \noalign material and
//!   stray \cr/\crcr are consumed, `}` ends the alignment, anything else
//!   starts the next row.
//! * `\span` in a row widens the current cell's span and plays the absorbed
//!   column's u part (again with a one-token \omit peek). `\noalign{...}`
//!   typesets its text in internal vertical mode and stores the resulting
//!   vbox as a row entry marked with span NOALIGN_SPAN.
//! * When the alignment group's `}` arrives (build.rs end_box, kind 7),
//!   `finish_halign` computes the column widths (max natural width per
//!   column; spanning cells distribute any deficit over the covered
//!   columns), re-packs every cell to its final width (the "unset box"
//!   pass: \hfil etc. stretch), wraps each row in boundary tabskips
//!   (T_0 before column 0, T_n after the last column) with tabskip glue
//!   between grid columns and baselineskip/lineskip glue between rows,
//!   packs the vbox and appends it like a box result (or assigns it to
//!   \setbox target).
//!
//! Known simplifications versus tex.web:
//! * unset-box glue is implemented by re-packing the cell list to the final
//!   width, so finite glue stretches along with fil glue (TeX freezes
//!   finite glue in unset boxes);
//! * \everycr plays at row start, before the next row's u part;
//! * nested \halign is supported through an engine-owned state stack (the
//!   outer preamble/rows survive an inner \halign used e.g. inside a
//!   p-column cell).

use crate::boxes::{Glue, Node, NodeList};
use crate::engine::{Engine, Mode, ScannerStatus};
use crate::eqtb::{Equiv, LevelType};
use crate::prim::{DimParam, GlueParam, Prim, ToksParam};
use crate::token::Token;

/// sentinel appended to a cell's close stream; intercepted by the expansion
/// loop (expand.rs get_token_inner) and routed to `finish_cell_typeset`.
pub const CELL_END_TOKEN: Token = Token(0xE000_0002);
/// tex.web's frozen end-template marker. It is consumed only after every
pub(crate) const U_END_TOKEN: Token = Token(0xFFFF_FFFA);

#[derive(Clone, Default)]
pub struct ColSpec {
    /// tokens played before the cell content
    pub u_part: Vec<Token>,
    /// tokens played after the cell content
    pub v_part: Vec<Token>,
    /// extra grid columns absorbed by preamble \span
    pub span: u16,
    pub tabskip: Glue,
}

#[derive(Clone, Default)]
pub struct Cell {
    /// natural-width hpacked box; re-packed to the final width at the end
    pub packed: Option<Node>,
    /// grid columns covered beyond the first (preamble + row \span)
    pub span: u16,
}

/// span sentinel marking a one-cell row holding a \noalign box
pub const NOALIGN_SPAN: u16 = u16::MAX;

/// box_kinds marker for an open alignment cell or \noalign group
const CELL_GROUP_KIND: u8 = 8;

/// align_state phase encoding (align_state is free for use inside rows:
/// the dispatcher only tests `align_state > 0` for \span placement, which
/// matches "a cell is open").
pub const PH_IDLE: i32 = 0; // no cell open
const PH_U: i32 = 1; // u part of the template is playing
pub(crate) const PH_CONTENT: i32 = 2; // cell content phase
const PH_OMIT: i32 = 4; // template omitted for the current cell
pub(crate) const PH_CLOSE: i32 = 8; // close stream pushed; sentinel not yet seen

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum AlignCloseReason {
    #[default]
    NextCell,
    EndRow,
    Span,
}

pub(crate) const U_PART_SRC: &str = "<align-u>";
const CELL_SRC: &str = "<align-cell>";
pub(crate) const PEEK_SRC: &str = "<align-peek>";

/// saved outer alignment state for nested \halign (tabular in a p-cell)
pub(crate) struct AlignSave {
    preamble: Vec<ColSpec>,
    loop_start: Option<usize>,
    rows: Vec<Vec<Cell>>,
    cur_row: Vec<Cell>,
    cur_col: i32,
    widths: Vec<i32>,
    scanning_cell: bool,
    close_reason: AlignCloseReason,
    in_noalign: bool,
    state: i32,
    done: bool,
    to: Option<(i32, bool)>,
    pushed_base: usize,
    delimiter_balance_base: i32,
    cell_level: u16,
    brace_depth: i32,
    noalign_save_base: usize,
    t0: Glue,
    everycr_done: bool,
    origin: Option<crate::input::SourceMark>,
    /// pending \\vadjust material of the row in progress (tex.web cur_head
    /// adjustment list) and the per-row drains already completed
    adjust: Vec<Node>,
    row_adjust: Vec<Vec<Node>>,
    is_valign: bool,
}

impl Engine {
    pub fn align_phase(&self) -> i32 {
        self.align_state & (PH_U | PH_CONTENT)
    }

    fn align_omitted(&self) -> bool {
        self.align_state & PH_OMIT != 0
    }

    fn align_set_phase(&mut self, phase: i32) {
        self.align_state = (self.align_state & PH_OMIT) | phase;
    }

    fn align_token_list_brace_balance(&self) -> i32 {
        self.input
            .stack
            .iter()
            .fold(0i32, |sum, source| match source {
                crate::input::Source::TokList { pos, toks, .. } => {
                    sum + toks[..*pos].iter().fold(0i32, |depth, t| {
                        if t.is_char() && t.cc() == 1 {
                            depth + 1
                        } else if t.is_char() && t.cc() == 2 {
                            depth - 1
                        } else {
                            depth
                        }
                    })
                }
                crate::input::Source::MacroFrame(frame) => sum + frame.delivered_brace_balance,
                _ => sum,
            })
    }

    pub(crate) fn align_delimiter_hidden(&self) -> bool {
        // tex.web @7257-7264: a row delimiter ends the entry only at
        // align_state = 0, where align_state is the cumulative net brace
        // depth since the u-template finished (@7007-7008 reset). The old
        // save-stack + active-token-list heuristic missed braces whose
        // source had already been popped (expl3 \exp_args:No re-serves an
        // expanded token list whose { } no longer appear in any active
        // source), closing the outer alignment mid-content (array.sty
        // table setup inside an amsmath align cell).
        self.align_brace_depth != 0
    }

    /// TeX's frozen end-template token resets alignment state only after all
    /// expansions and commands emitted by the u-template have completed.
    pub(crate) fn align_u_template_finished(&mut self) {
        if self.align_phase() == PH_U {
            self.align_cell_level = self.eqtb.cur_level;
            self.align_delimiter_balance_base = self.align_token_list_brace_balance();
            self.align_brace_depth = 0;
            self.align_set_phase(PH_CONTENT);
        }
    }

    /// tex.web get_next: a row delimiter at alignment brace-depth zero
    /// cannot be consumed by a macro parameter scanner. Queue the current
    /// template's close stream so scanning proceeds through the same input
    /// TeX would insert at the end of the cell.
    pub(crate) fn align_intercept_raw_token(&mut self, t: Token) -> bool {
        if self.scanner_status != ScannerStatus::Aligning
            || self.align_phase() != PH_CONTENT
            || self.align_state & PH_CLOSE != 0
        {
            return false;
        }
        let is_tab = if t.is_char() && t.cc() == 4 {
            true
        } else if let Some(id) = t.is_cs().then(|| t.cs_id()) {
            matches!(self.eqtb.resolve(id), Some(crate::eqtb::Equiv::CharTok(raw)) if Token(*raw).cc() == 4)
        } else {
            false
        };
        if is_tab {
            if self.align_delimiter_hidden() {
                return false;
            }
            self.align_tab();
            return true;
        }
        let Some(id) = t.is_cs().then(|| t.cs_id()) else {
            return false;
        };
        let prim = match self.eqtb.resolve(id) {
            Some(Equiv::Prim(p @ (Prim::Cr | Prim::CrCr))) => *p,
            _ => return false,
        };
        if self.align_delimiter_hidden() {
            return false;
        }
        self.cur_tok = t;
        self.cur_cs = Some(id);
        self.cur_prim = Some(prim);
        self.align_cr();
        true
    }
    /// true when an outer alignment state is saved for this engine (we are
    /// the inner \halign of a nesting)
    fn align_has_save(&self) -> bool {
        !self.align_stack.is_empty()
    }

    // ------------------------------------------------------------------
    // \halign
    // ------------------------------------------------------------------

    pub fn begin_halign(&mut self) {
        let origin = self.current_token_source_mark();
        if self.scanner_status == ScannerStatus::Aligning
            || self.box_kinds.iter().any(|&k| k == 7)
            || self.align_origin.is_some()
        {
            let save = AlignSave {
                preamble: std::mem::take(&mut self.align_preamble),
                loop_start: self.align_loop_start.take(),
                rows: std::mem::take(&mut self.align_rows),
                cur_row: std::mem::take(&mut self.align_cur_row),
                cur_col: self.align_cur_col,
                widths: std::mem::take(&mut self.align_col_widths),
                scanning_cell: self.align_scanning_cell,
                in_noalign: self.align_in_noalign,
                state: self.align_state,
                done: self.align_done,
                to: self.align_to,
                pushed_base: self.align_pushed_base,
                delimiter_balance_base: self.align_delimiter_balance_base,
                cell_level: self.align_cell_level,
                brace_depth: self.align_brace_depth,
                close_reason: self.align_close_reason,
                everycr_done: self.align_everycr_done,
                adjust: std::mem::take(&mut self.align_adjust),
                row_adjust: std::mem::take(&mut self.align_row_adjust),
                noalign_save_base: self.align_noalign_save_base,
                t0: self.align_t0.clone(),
                origin: self.align_origin.take(),
                is_valign: self.align_is_valign,
            };
            self.align_stack.push(save);
        }
        self.align_origin = origin;
        self.align_is_valign = false;
        self.align_state = PH_IDLE;
        // tex.web §15332 / §15369: align_state := -1000000 while preamble scans.
        self.align_brace_depth = -1_000_000;
        // tex.web §1130 & §774: \halign is valid in vertical modes AND in
        // DisplayMath (e.g. \eqalign, amsmath \align, etc.). Inside DisplayMath,
        // the alignment box is centered on the display line.
        if self.mode == Mode::Horizontal {
            // tex.web negates unrestricted horizontal mode (giving the
            // alignment valign-like semantics); LaTeX never uses this form,
            // tabular wraps its \halign in \hbox (restricted mode)
            self.error("\\halign in horizontal mode");
            self.align_nested_restore();
            return;
        }
        self.scanner_status = ScannerStatus::Aligning;
        // \halign to <dimen> / \halign spread <dimen> (tex.web scan_spec)
        self.align_to = None;
        self.align_t0 = self.eqtb.glue_params[GlueParam::TabSkip.idx() as usize].clone();
        self.align_to = None;
        if self.scan_keyword(b"to") {
            let d = self.scan_dimen(false, false);
            self.align_to = Some((d, false));
        } else if self.scan_keyword(b"spread") {
            let d = self.scan_dimen(false, false);
            self.align_to = Some((d, true));
        }
        if !self.scan_align_preamble() {
            let nested = self.align_has_save();
            self.align_nested_restore();
            if !nested {
                self.scanner_status = ScannerStatus::Normal;
                self.align_state = PH_IDLE;
                self.align_to = None;
            }
            return;
        }
        // enter the alignment group (build.rs end_box pops this for kind 7
        // and calls finish_halign)
        self.saved_lists.push((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
        ));
        self.push_group_level(LevelType::Box);
        self.box_targets.push(None);
        self.box_shifts.push(0);
        self.box_kinds.push(7);
        self.mode = Mode::InternalVertical;
        self.prev_graf = 0;
        // tex.web §15350 / init_align: the alignment's internal vertical list inherits prev_depth
        // from the enclosing context (preserved from outer vertical mode / display math).
        self.align_rows.clear();
        self.align_row_adjust.clear();
        self.align_adjust.clear();
        self.align_cur_row.clear();
        self.align_cur_col = 0;
        self.align_in_noalign = false;
        self.align_state = PH_IDLE;
        self.align_done = false;
        self.align_everycr_done = false;
        // tex.web §15512: align_state := 1000000 between rows.
        self.align_brace_depth = 1_000_000;
        self.align_row_inspect();
    }
    pub fn begin_valign(&mut self) {
        if self.mode.is_v() {
            self.start_paragraph(true);
        }
        let origin = self.current_token_source_mark();
        if self.scanner_status == ScannerStatus::Aligning
            || self.box_kinds.iter().any(|&k| k == 7)
            || self.align_origin.is_some()
        {
            let save = AlignSave {
                preamble: std::mem::take(&mut self.align_preamble),
                loop_start: self.align_loop_start.take(),
                rows: std::mem::take(&mut self.align_rows),
                cur_row: std::mem::take(&mut self.align_cur_row),
                cur_col: self.align_cur_col,
                widths: std::mem::take(&mut self.align_col_widths),
                scanning_cell: self.align_scanning_cell,
                close_reason: self.align_close_reason,
                in_noalign: self.align_in_noalign,
                state: self.align_state,
                done: self.align_done,
                to: self.align_to,
                pushed_base: self.align_pushed_base,
                delimiter_balance_base: self.align_delimiter_balance_base,
                cell_level: self.align_cell_level,
                brace_depth: self.align_brace_depth,
                everycr_done: self.align_everycr_done,
                adjust: std::mem::take(&mut self.align_adjust),
                row_adjust: std::mem::take(&mut self.align_row_adjust),
                noalign_save_base: self.align_noalign_save_base,
                t0: self.align_t0.clone(),
                origin: self.align_origin.take(),
                is_valign: self.align_is_valign,
            };
            self.align_stack.push(save);
        }
        self.align_origin = origin;
        self.align_is_valign = true;
        self.align_state = PH_IDLE;
        self.align_brace_depth = -1_000_000;
        self.scanner_status = ScannerStatus::Aligning;
        self.align_to = None;
        self.align_t0 = self.eqtb.glue_params[GlueParam::TabSkip.idx() as usize].clone();
        if self.scan_keyword(b"to") {
            let d = self.scan_dimen(false, false);
            self.align_to = Some((d, false));
        } else if self.scan_keyword(b"spread") {
            let d = self.scan_dimen(false, false);
            self.align_to = Some((d, true));
        }
        if !self.scan_align_preamble() {
            let nested = self.align_has_save();
            self.align_nested_restore();
            if !nested {
                self.scanner_status = ScannerStatus::Normal;
                self.align_state = PH_IDLE;
                self.align_to = None;
                self.align_is_valign = false;
            }
            return;
        }
        self.saved_lists.push((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
        ));
        self.push_group_level(LevelType::Box);
        self.box_targets.push(None);
        self.box_shifts.push(0);
        self.box_kinds.push(7);
        self.mode = Mode::RestrictedHorizontal;
        self.space_factor = 1000;
        self.prev_graf = 0;
        self.prev_depth = -1000 * 65536;
        self.align_rows.clear();
        self.align_row_adjust.clear();
        self.align_adjust.clear();
        self.align_cur_row.clear();
        self.align_cur_col = 0;
        self.align_in_noalign = false;
        self.align_state = PH_IDLE;
        self.align_done = false;
        self.align_everycr_done = false;
        self.align_brace_depth = 1_000_000;
        self.align_row_inspect();
    }

    fn align_nested_restore(&mut self) {
        let outer = self.align_stack.pop();
        if let Some(sv) = outer {
            self.align_preamble = sv.preamble;
            self.align_loop_start = sv.loop_start;
            self.align_rows = sv.rows;
            self.align_cur_row = sv.cur_row;
            self.align_cur_col = sv.cur_col;
            self.align_col_widths = sv.widths;
            self.align_scanning_cell = sv.scanning_cell;
            self.align_in_noalign = sv.in_noalign;
            self.align_state = sv.state;
            self.align_done = sv.done;
            self.align_to = sv.to;
            self.align_pushed_base = sv.pushed_base;
            self.align_delimiter_balance_base = sv.delimiter_balance_base;
            self.align_cell_level = sv.cell_level;
            self.align_brace_depth = sv.brace_depth;
            self.align_scanning_cell = sv.scanning_cell;
            self.align_close_reason = sv.close_reason;
            self.align_in_noalign = sv.in_noalign;
            self.align_noalign_save_base = sv.noalign_save_base;
            self.align_t0 = sv.t0;
            self.align_adjust = sv.adjust;
            self.align_row_adjust = sv.row_adjust;
            self.align_everycr_done = sv.everycr_done;
            self.align_origin = sv.origin;
            self.align_is_valign = sv.is_valign;
        } else {
            self.align_origin = None;
            self.align_close_reason = AlignCloseReason::default();
            self.align_brace_depth = 0;
            self.align_is_valign = false;
        }
    }

    fn fatal_alignment_eof(&mut self, message: &str) {
        let source = self
            .align_origin
            .as_ref()
            .map(crate::input::SourceMark::to_context);
        self.fatal_error_at(message, source);
    }

    /// scan `{ <u # v>... \cr`; the preamble ends at \cr/\crcr. Returns
    /// false (after reporting) on any fatal error.
    fn scan_align_preamble(&mut self) -> bool {
        // tex.web scan_left_brace
        self.skip_spaces_relax();
        let t = self.get_token();
        if t == crate::input::EOF_MARKER {
            self.fatal_alignment_eof("File ended while scanning an alignment preamble");
            return false;
        }
        if !self.token_is_left_brace(t) {
            self.error("Missing { inserted for alignment preamble");
            if !(t.is_char() && t.cc() == 2) {
                // a stray } must not close the (not yet started) group
                self.pushed.push(t);
            }
        }
        let mut entries: Vec<ColSpec> = Vec::new();
        let mut cur = ColSpec::default();
        let mut in_u = true;
        let mut depth = 0i32;
        loop {
            // tex.web scan_template uses get_token (NON-expanding): u/v part
            // tokens are stored literally and expanded at cell time. Expanding
            // here runs NFSS (\\footnotesize -> \\@setfontsize -> \\let
            // \\@currsize) during the scan; the \\let never executes and the
            // size machinery loops. Resolve CharTok/prims only to RECOGNIZE
            // structural tokens (array's \\@sharp is \\let to #); store the
            // original token.
            let t = self.raw_token();
            if t == crate::input::EOF_MARKER {
                self.fatal_alignment_eof("File ended while scanning an alignment preamble");
                return false;
            }
            let t = if t == crate::input::PAR_END {
                Token::from_cs(self.partoken_id())
            } else {
                t
            };
            let prim = if t.is_cs() {
                match self.eqtb.resolve(t.cs_id()) {
                    Some(crate::eqtb::Equiv::Prim(p)) => Some(*p),
                    _ => None,
                }
            } else {
                None
            };
            let eff = if t.is_cs() {
                match self.eqtb.resolve(t.cs_id()) {
                    Some(crate::eqtb::Equiv::CharTok(v)) => Token(*v),
                    _ => t,
                }
            } else {
                t
            };
            match prim {
                Some(Prim::Cr) | Some(Prim::CrCr) => break,
                Some(Prim::Span) => {
                    // tex.web §783: in the preamble, `\span` causes the
                    // following macro to be expanded! It does NOT mean
                    // multicolumn (that is only valid in row cells).
                    let next = self.get_token();
                    self.pushed.push(next);
                    continue;
                }
                Some(Prim::Omit) => {
                    self.error("\\omit is not allowed in an alignment preamble");
                    continue;
                }
                Some(Prim::GlueP(crate::prim::GlueParam::TabSkip)) => {
                    self.scan_optional_equals();
                    let g = self.scan_glue(false);
                    self.eqtb
                        .assign_glue_param(crate::prim::GlueParam::TabSkip, g, false);
                    continue;
                }
                _ => {}
            }
            if eff.is_char() {
                match eff.cc() {
                    1 => depth += 1,
                    2 => {
                        depth -= 1;
                        if depth < 0 {
                            // the alignment group's closing brace cannot
                            // appear inside the preamble
                            self.error("Missing \\cr inserted in alignment preamble");
                            break;
                        }
                    }
                    // LaTeX wraps # in {\\hfil ... # ...} so the marker is
                    // at depth 1; still split u/v there (Knuth only does
                    // depth 0, which leaves # in the u-part and hangs).
                    6 => {
                        if in_u {
                            in_u = false;
                        } else {
                            self.error("Only one # allowed per alignment entry");
                        }
                        continue;
                    }
                    4 if depth == 0 => {
                        let all_spaces =
                            cur.u_part.iter().all(|tok| tok.is_char() && tok.cc() == 10);
                        if in_u && (cur.u_part.is_empty() || all_spaces) && cur.v_part.is_empty() {
                            // tex.web §782: an empty template between two &
                            // marks the start of the periodic preamble. This
                            // may follow already completed columns (`#&&...`).
                            self.align_loop_start = Some(entries.len());
                            cur.u_part.clear();
                        } else {
                            cur.tabskip =
                                self.eqtb.glue_params[GlueParam::TabSkip.idx() as usize].clone();
                            entries.push(std::mem::take(&mut cur));
                            in_u = true;
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            if in_u {
                if cur.u_part.is_empty() && t.is_char() && t.cc() == 10 {
                    continue;
                }
                cur.u_part.push(t);
            } else {
                cur.v_part.push(t);
            }
        }
        // the entry in progress at \cr is always a column, even when its
        // template is empty (#\cr is a legal single bare column)
        cur.tabskip = self.eqtb.glue_params[GlueParam::TabSkip.idx() as usize].clone();
        entries.push(cur);
        self.align_preamble = entries;

        true
    }

    // ------------------------------------------------------------------
    // cells
    // ------------------------------------------------------------------

    /// push the group context for a cell or \noalign group. The group is
    /// popped by finish_cell_typeset (a stray `}` mid-cell pops it via
    /// end_box instead, degrading gracefully without corrupting the stack).
    fn align_push_cell_group(&mut self, mode: Mode) {
        self.saved_lists.push((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
        ));
        self.prev_graf = 0;
        self.push_group_level(LevelType::Box);

        self.box_targets.push(None);
        self.box_shifts.push(0);
        self.box_kinds.push(CELL_GROUP_KIND);
        self.mode = mode;
        if mode.is_v() {
            self.prev_depth = -1000 * 65536;
        }
        // NOTE: tex.web parks align_state at 1000000 only while a u-template
        // plays (init_col @15564), not for cell groups generally; a
        // noalign cell has no u-part that would ever reset it. Parking is
        // done at the u-part push sites (align_start_cell/align_span).
    }

    /// retrieve ColSpec for column index, wrapping around via align_loop_start per tex.web §785
    fn get_col_spec(&self, col: usize) -> Option<ColSpec> {
        if self.align_preamble.is_empty() {
            return None;
        }
        if col < self.align_preamble.len() {
            return Some(self.align_preamble[col].clone());
        }
        if let Some(loop_start) = self.align_loop_start {
            let loop_len = self.align_preamble.len().saturating_sub(loop_start);
            if loop_len > 0 {
                let offset = (col - loop_start) % loop_len;
                return Some(self.align_preamble[loop_start + offset].clone());
            }
        }
        None
    }

    /// start the cell at `align_cur_col`
    fn align_start_cell(&mut self, first: Option<crate::token::Token>) {
        let col = self.align_cur_col as usize;
        let spec = match self.get_col_spec(col) {
            Some(s) => s,
            None => return, // empty or exhausted preamble
        };
        let cell_mode = if self.align_is_valign {
            Mode::InternalVertical
        } else {
            Mode::RestrictedHorizontal
        };
        self.align_push_cell_group(cell_mode);
        if self.align_is_valign {
            self.prev_depth = -1000 * 65536;
        }
        self.align_cell_level = self.eqtb.cur_level;
        self.align_delimiter_balance_base = self.align_token_list_brace_balance();
        while self.align_cur_row.len() <= col {
            self.align_cur_row.push(Cell::default());
        }
        self.align_cur_row[col].span = spec.span;
        self.align_state = PH_U;
        let t = match first {
            Some(t) => t,
            None => {
                let t = self.align_peek_expanding();
                if t == crate::input::EOF_MARKER {
                    self.fatal_alignment_eof("File ended while starting an alignment cell");
                    return;
                }
                t
            }
        };
        // tex.web init_col @15564: while the u-template plays, align_state is
        // parked at 1000000; u-part finish (or omit) resets to 0.
        self.align_brace_depth = 1_000_000;
        let is_omit =
            t.is_cs() && matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Prim(Prim::Omit)));
        if is_omit {
            self.align_state = PH_CONTENT | PH_OMIT;
            // tex.web init_col @15562: \omit resets align_state to 0.
            self.align_brace_depth = 0;
            return;
        }
        // Queue cell content below the u-template and its end marker.
        self.align_pushed_base = self.pushed.len();
        self.push_tokens_named(vec![t], PEEK_SRC);
        if spec.u_part.is_empty() {
            self.align_u_template_finished();
            self.align_set_phase(PH_CONTENT);
        } else {
            self.align_pushed_base = self.pushed.len();
            self.push_tokens_named(spec.u_part, U_PART_SRC);
        }
    }
    fn align_discard_u_part(&mut self) {
        let is_u = matches!(
            self.input.stack.last(),
            Some(crate::input::Source::TokList { name, .. }) if *name == U_PART_SRC
        );
        if is_u {
            self.input.stack.pop();
        }
        self.align_u_template_finished();
    }
    /// token terminating a close stream. The \crcr primitive token is used
    /// (not CELL_END_TOKEN): scanners may look ahead across the close
    /// stream (e.g. \hskip's fill check), and a scanner swallowing
    /// CELL_END_TOKEN would pack the cell mid-scan. A \crcr cs token is
    /// pushed back by scanners and only acted on at dispatch level, like
    /// tex.web's frozen \cr.
    pub(crate) fn crcr_token(&self) -> Token {
        match self.cs.lookup(b"crcr") {
            Some(id) => Token::from_cs(id),
            None => CELL_END_TOKEN,
        }
    }

    /// close the current cell or noalign group: push [v part..., \crcr]
    /// (v part skipped when the template was omitted). The pending state is
    /// marked with PH_CLOSE; when the sentinel dispatches, the cell is
    /// finished synchronously. `row_continues` selects whether the cell is
    /// followed by another cell (&) or ends the row (\cr).
    fn align_push_close(&mut self, reason: AlignCloseReason) {
        let col = self.align_cur_col as usize;
        self.align_close_reason = reason;
        self.align_scanning_cell = reason == AlignCloseReason::NextCell;
        let omit = self.align_omitted();
        self.align_set_phase(PH_CONTENT);
        self.align_state |= PH_CLOSE;
        // tex.web @15583: while v_template plays, align_state := 1000000.
        self.align_brace_depth = 1_000_000;
        let mut close: Vec<Token> = Vec::new();
        if !omit {
            // tex.web fin_col: the v part played at the end of a cell is
            // that of the LAST entry the cell covers (cur_align advances
            // through spanned columns)
            let last = col
                + self
                    .align_cur_row
                    .get(col)
                    .map(|c| c.span as usize)
                    .unwrap_or(0);
            if let Some(spec) = self.get_col_spec(last) {
                close.extend(spec.v_part.iter().cloned());
            }
        }
        close.push(self.crcr_token());
        self.align_pushed_base = self.pushed.len();
        self.push_tokens_named(close, CELL_SRC);
    }

    pub fn align_tab(&mut self) {
        if self.scanner_status != ScannerStatus::Aligning {
            self.error("Misplaced alignment tab character &");
            return;
        }
        if self.align_state & PH_CLOSE != 0 {
            // a & played back from the close stream (tabular preambles put
            // one at the end of every v part) is structural noise
            return;
        }
        // A template u part may open box groups inside the cell (LaTeX
        // p-columns: `\@startpbox` = `\vtop\bgroup ...`), so the CELL group
        // is not necessarily the top of box_kinds. tex.web unwinds such
        // template groups via the v part when the row machinery fires; a
        // `&` is only genuinely misplaced between rows / outside a cell.
        if self.align_phase() == PH_IDLE || !self.box_kinds.contains(&CELL_GROUP_KIND) {
            self.error("Misplaced alignment tab character &");
            return;
        }
        let col = self.align_cur_col as usize;
        let cur_span = self
            .align_cur_row
            .get(col)
            .map(|c| c.span as usize)
            .unwrap_or(0);
        let next_col = col + 1 + cur_span;
        if self.get_col_spec(next_col).is_none() {
            self.error("Extra alignment tab has been changed to \\cr");
            if self.align_phase() == PH_U {
                self.align_discard_u_part();
            }
            self.align_push_close(AlignCloseReason::EndRow);
            return;
        }
        if self.align_phase() == PH_U {
            self.align_discard_u_part();
        }
        self.align_push_close(AlignCloseReason::NextCell);
    }
    pub fn align_cr(&mut self) {
        if self.scanner_status != ScannerStatus::Aligning {
            self.error("Misplaced \\cr");
            return;
        }
        if self.align_state & PH_CLOSE != 0 {
            // the close stream's sentinel: finish the cell / noalign group
            // synchronously (dispatch level, no scanner is mid-flight)
            self.align_state &= !PH_CLOSE;
            if self.align_in_noalign {
                self.align_finish_noalign_now();
            } else if self.align_close_reason == AlignCloseReason::Span {
                self.align_absorb_span_now();
            } else {
                self.align_finish_cell_now();
            }
            return;
        }
        // tex.web: a `\crcr` between rows (no row in progress, not inside a
        // \noalign body) is silently absorbed — e.g. longtable's
        // \LT@end@hd@ft opens with \crcr after the caption row already ended
        // with \\. Only a true \cr is "Misplaced" there.
        if self.cur_prim == Some(Prim::CrCr)
            && self.align_phase() == PH_IDLE
            && !self.align_in_noalign
        {
            return;
        }
        // or fully outside a cell is misplaced; otherwise let the close
        // stream's v part unwind the template groups and finish the cell.
        if self.align_phase() == PH_IDLE || !self.box_kinds.contains(&CELL_GROUP_KIND) {
            self.error("Misplaced \\cr");
            return;
        }
        if self.align_phase() == PH_U {
            self.align_discard_u_part();
        }
        self.align_push_close(AlignCloseReason::EndRow);
    }

    /// \omit: skip the u part now and the v part when the cell closes.
    /// Valid only at the very start of a cell (nothing typeset yet).
    pub fn align_omit(&mut self) {
        if self.scanner_status != ScannerStatus::Aligning || self.align_phase() == PH_IDLE {
            self.error("Misplaced \\omit");
            return;
        }
        if self.align_omitted() {
            self.error("Duplicate \\omit");
            return;
        }
        match self.align_phase() {
            PH_U => {
                self.align_discard_u_part();
                self.align_state = PH_CONTENT | PH_OMIT;
            }
            _ => {
                if !self.cur_list.is_empty() {
                    self.error("Misplaced \\omit");
                    return;
                }
                self.align_state |= PH_OMIT;
            }
        }
        // align_state := 0.
        self.align_brace_depth = 0;
    }

    /// \span in a row: the current cell absorbs the next grid column; the
    /// absorbed column's u part plays next (skipped when \omit follows).
    /// \span in a row: ends the current column's content by playing its v-template
    /// via the close stream, then absorbs the next grid column.
    pub fn align_span(&mut self) {
        if self.scanner_status != ScannerStatus::Aligning || self.align_phase() == PH_IDLE {
            self.error("\\span outside alignment");
            return;
        }
        if self.align_state & PH_CLOSE != 0 {
            return;
        }
        if self.align_phase() == PH_IDLE || !self.box_kinds.contains(&CELL_GROUP_KIND) {
            self.error("\\span outside alignment");
            return;
        }
        if self.align_phase() == PH_U {
            self.align_discard_u_part();
        }
        self.align_push_close(AlignCloseReason::Span);
    }

    /// called when the close stream from a `\span` delimiter dispatches its
    /// sentinel \crcr: keeps the open cell box group intact, advances the
    /// cell's span counter to absorb the next grid column, and plays the
    /// absorbed column's u-part (unless \omit follows).
    fn align_absorb_span_now(&mut self) {
        let col = self.align_cur_col as usize;
        while self.align_cur_row.len() <= col {
            self.align_cur_row.push(Cell::default());
        }
        let cell = &mut self.align_cur_row[col];
        cell.span = cell.span.saturating_add(1);
        let span = cell.span as usize;

        // tex.web @15623: align_state := 1000000; get_next; cur_align := p; init_col;
        self.align_brace_depth = 1_000_000;
        let t = self.align_peek_expanding();
        if t == crate::input::EOF_MARKER {
            self.fatal_alignment_eof("File ended after \\span");
            return;
        }
        let is_omit =
            t.is_cs() && matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Prim(Prim::Omit)));
        if is_omit {
            self.align_state = (self.align_state & PH_OMIT) | PH_CONTENT | PH_OMIT;
            self.align_brace_depth = 0;
            return;
        }
        self.align_state = (self.align_state & PH_OMIT) | PH_U;
        // Re-queue the peeked token below the absorbed column's u-template.
        self.push_tokens_named(vec![t], PEEK_SRC);
        let u = self
            .get_col_spec(col + span)
            .map(|s| s.u_part.clone())
            .unwrap_or_default();
        if u.is_empty() {
            self.align_u_template_finished();
            self.align_state = (self.align_state & PH_OMIT) | PH_CONTENT;
        } else {
            self.align_pushed_base = self.pushed.len();
            self.push_tokens_named(u, U_PART_SRC);
        }
    }

    // cell completion (at dispatch level, from the \crcr sentinel)
    // ------------------------------------------------------------------

    /// kept for expand.rs's CELL_END interception (defensive only: close
    /// streams are terminated with a \crcr primitive token, which scanners
    /// push back instead of swallowing mid-scan)
    pub fn finish_cell_typeset(&mut self) {
        self.align_finish_cell_now();
    }

    fn align_pop_cell_group(&mut self) -> Option<(NodeList, i32)> {
        if self.box_kinds.last() != Some(&CELL_GROUP_KIND) || self.saved_lists.is_empty() {
            return None;
        }
        let end_pd = self.prev_depth;
        let inner = std::mem::take(&mut self.cur_list);
        let _ = self.box_targets.pop().flatten();
        let _ = self.box_shifts.pop().unwrap_or(0);
        let _ = self.box_kinds.pop();
        self.pop_group();
        let (om, ol, pd, sf, pg) = self.saved_lists.pop().unwrap();
        self.prev_graf = pg;
        self.cur_list = ol;
        self.mode = om; // tex.web unsave: the enclosing level's mode returns
        self.prev_depth = pd;
        self.space_factor = sf;
        Some((inner, end_pd))
    }

    pub fn align_noalign(&mut self) {
        // after real cell content (or inside a noalign) stays an error.
        let phantom = self.scanner_status == ScannerStatus::Aligning
            && !self.align_in_noalign
            && self.align_phase() == PH_U
            && self.align_cur_col == 0
            && self.cur_list.is_empty();
        if self.scanner_status != ScannerStatus::Aligning
            || self.align_in_noalign
            || (self.align_phase() != PH_IDLE && !phantom)
        {
            self.error("Misplaced \\noalign");
            return;
        }
        if phantom {
            let _ = self.align_pop_cell_group();
            self.align_state = PH_IDLE;
        }
        // tex.web 1124-1131: \noalign consumes only the opening brace; the
        // body EXECUTES inside the no-align group and the matching `}`
        // closes it (see end_group). Eager balanced scanning broke
        // `\noalign{\ifnum0=`}\fi}` — booktabs/\@BTendrule's standard
        // idiom — because the conditional must eat its own `}` at
        // execution time, not at scan time.
        self.skip_spaces_relax();
        let open = self.get_token();
        if !(open.is_char() && open.cc() == 1) {
            self.error("Missing { inserted for \\noalign");
            return;
        }
        self.align_in_noalign = true;
        let outer_pd = self.prev_depth;
        self.align_push_cell_group(Mode::InternalVertical);
        // tex.web §15514: \noalign runs in internal vertical mode inheriting the
        // preceding row's depth (or ignore_depth if at the alignment start).
        self.prev_depth = self
            .align_rows
            .last()
            .and_then(|r| {
                if r.len() == 1 && r[0].span == NOALIGN_SPAN {
                    match &r[0].packed {
                        Some(Node::Box { shift, .. }) => Some(*shift),
                        _ => None,
                    }
                } else {
                    r.iter()
                        .filter_map(|c| match &c.packed {
                            Some(Node::Box { d, .. }) => Some(*d),
                            _ => None,
                        })
                        .max()
                }
            })
            .unwrap_or(outer_pd);
        // group pushed above is LevelType::Box — without an explicit
        // Simple level for the consumed `{`, the body's `}` would close
        self.align_noalign_save_base = self.eqtb.save_stack.len();
        self.align_pushed_base = self.pushed.len();
        self.push_group_level(LevelType::Simple);
    }

    /// tex.web hpack @12956, 13006-13016: the natural-width pack of an
    /// alignment cell transfers every top-level \vadjust node out of its
    /// hlist onto the alignment level's adjustment list (`cur_tail`),
    /// preserving source order. The inner vlist of each adjustment joins
    /// the accumulator; the node itself contributes no width.
    fn align_collect_adjustments(&mut self, list: &mut NodeList) {
        let mut i = 0;
        while i < list.len() {
            if matches!(list[i], Node::VAdjust(_)) {
                match list.remove(i) {
                    Node::VAdjust(items) => self.align_adjust.extend(items),
                    _ => unreachable!(),
                }
            } else {
                i += 1;
            }
        }
    }
    fn align_finish_cell_now(&mut self) {
        let Some((mut inner, _)) = self.align_pop_cell_group() else {
            return;
        };
        // tex.web hpack @12956/13006-13016: the natural-width pack of a
        // cell transfers every \vadjust node out of its hlist onto the
        // alignment level's adjustment list (cur_tail), in source order.
        self.align_collect_adjustments(&mut inner);
        let col = self.align_cur_col as usize;
        let span = self.align_cur_row.get(col).map(|c| c.span).unwrap_or(0);
        let packed = if self.align_is_valign {
            crate::boxes::vpack(inner, None, crate::boxes::VBOX, &self.eqtb).node
        } else {
            crate::boxes::hpack(inner, None, crate::boxes::HBOX, &self.eqtb).node
        };
        while self.align_cur_row.len() <= col {
            self.align_cur_row.push(Cell::default());
        }
        self.align_cur_row[col] = Cell {
            packed: Some(packed),
            span,
        };
        self.align_state = PH_IDLE;
        self.align_brace_depth = 1_000_000;
        if self.align_scanning_cell {
            // & closed this cell: start the next one
            let next = col + 1 + span as usize;
            if self.get_col_spec(next).is_none() {
                // align_tab normally reroutes this to \cr
                self.error("Extra alignment tab has been changed to \\cr");
                self.align_finish_row();
            } else {
                self.align_cur_col = next as i32;
                self.align_start_cell(None);
            }
        } else {
            self.align_finish_row();
        }
    }

    /// capture the open \noalign text as a RAW (unpacked) vbox node; tex.web
    /// splices noalign material into the alignment's vlist as-is, and the
    /// enclosing pack resolves any running-width rules (\toprule's \hrule)
    /// to the alignment width. Packing here would freeze them at \hsize.
    pub(crate) fn align_finish_noalign_now(&mut self) {
        // tex.web §21663 (no_align_group handle_right_brace): `end_graf; unsave; align_peek;`
        // Closing \noalign while a paragraph is running forces \par first,
        // breaking the paragraph into lines and restoring mode to InternalVertical.
        if self.mode == Mode::Horizontal {
            self.par_primitive();
        }
        let Some((inner, end_pd)) = self.align_pop_cell_group() else {
            return;
        };
        self.align_in_noalign = false;
        self.align_state = PH_IDLE;
        if !inner.is_empty() {
            let (w, h, d) = crate::boxes::vlist_dims(&inner, &self.eqtb);
            let node = Node::Box {
                kind: crate::boxes::VBOX,
                w,
                h,
                d,
                shift: end_pd,
                list: inner,
                glue_sign: 0,
                glue_order: 0,
                glue_set: 0.0,
                font: None,
            };
            self.align_rows.push(vec![Cell {
                packed: Some(node),
                span: NOALIGN_SPAN,
            }]);
            self.align_row_adjust
                .push(std::mem::take(&mut self.align_adjust));
        }
        // no \\vadjust material can originate between rows (it is forbidden
        // in vertical mode, tex.web @21195), so this row's drain is empty;
        // the push keeps align_row_adjust parallel to align_rows.
        // \noalign may be followed by another \noalign, \cr or }
        // tex.web @21663 (align_peek): align_state := 1000000 between rows.
        self.align_brace_depth = 1_000_000;
        self.align_row_inspect();
    }

    /// pack up the finished row and inspect what follows it
    fn align_finish_row(&mut self) {
        let row = std::mem::take(&mut self.align_cur_row);
        if !row.is_empty() {
            // tex.web fin_row: after the row's unset box joins the
            // alignment vlist, the collected adjustment material is spliced
            // in raw right behind it (init_row later rewinds cur_tail to
            // cur_head; the per-row drain keeps that rewind implicit).
            let adj = std::mem::take(&mut self.align_adjust);
            self.align_rows.push(row);
            self.align_row_adjust.push(adj);
        }
        self.align_cur_col = 0;
        self.align_everycr_done = false;
        // tex.web @15512 (align_peek): align_state := 1000000 between rows.
        self.align_brace_depth = 1_000_000;
        self.align_row_inspect();
    }

    /// peek one token, expanding macros and skipping spaces first:
    /// tex.web's align_peek uses get_x_token, so row boundaries see through
    /// macros like a \noalign wrapper defined as \def\br{\noalign{\hrule}}
    fn align_peek_expanding(&mut self) -> Token {
        loop {
            // Pre-expansion guard: the blank line's PAR_END sentinel and a
            // literal \par cs must be skipped BEFORE get_x_raw, because
            // get_x_raw fully expands macros — \par expands to \para_end:
            // whose leading \scan_stop: would start a phantom row (tex.web
            // §783: vmode+par between rows is a no-op, never expanded).
            let raw = self.raw_token();
            if raw == crate::input::EOF_MARKER {
                return raw;
            }
            if self.is_partoken(raw) {
                continue;
            }
            if raw.is_cs() {
                let mut id = raw.cs_id();
                while let Some(crate::eqtb::Equiv::Alias(n)) = self.eqtb.get(id) {
                    id = *n;
                }
                if let Some(crate::eqtb::Equiv::Macro(m)) = self.eqtb.get(id) {
                    if m.protected {
                        self.cur_tok = raw;
                        self.cur_cs = Some(raw.cs_id());
                        self.cur_prim = None;
                        return raw;
                    }
                }
            }
            let t = self.get_x_raw_from(raw);
            if t == crate::input::EOF_MARKER {
                return t;
            }
            if t.is_char() && t.cc() == 10 {
                continue;
            }
            // tex.web §783: \par between rows (or blank lines) in an alignment
            return t;
        }
    }
    /// after a \cr (or at alignment start): decide between \noalign, another
    /// \cr, the alignment's closing brace, or the next row's first cell.
    fn align_row_inspect(&mut self) {
        let ec = (*self.eqtb.tok_params[ToksParam::EveryCr.idx() as usize]).clone();
        if !ec.is_empty() && !self.align_everycr_done {
            self.align_everycr_done = true;
            self.push_tokens_named(ec, "<everycr>");
        }
        loop {
            let t = self.align_peek_expanding();
            if t == crate::input::EOF_MARKER {
                self.fatal_alignment_eof("File ended during an alignment");
                return;
            }
            if t.is_cs() {
                match self.cur_prim {
                    Some(Prim::NoAlign) => {
                        self.align_noalign();
                        return;
                    }
                    // tex.web §15518-15522: between rows, only \crcr is ignored.
                    // A true \cr starts a new row (whose first cell closes immediately).
                    Some(Prim::CrCr) => continue,
                    _ => {}
                }
            }
            if self.is_right_brace(t) {
                self.align_done = true;
                self.align_pushed_base = self.pushed.len();
                self.push_tokens_named(vec![t], PEEK_SRC);
                return;
            }
            self.align_start_row(Some(t));
            return;
        }
    }

    fn align_start_row(&mut self, first: Option<crate::token::Token>) {
        self.align_cur_col = 0;
        self.align_start_cell(first);
    }
    // ------------------------------------------------------------------
    // final packaging (build.rs end_box, kind 7)
    // ------------------------------------------------------------------

    pub fn finish_halign(&mut self) {
        let rows_in = std::mem::take(&mut self.align_rows);
        let max_row_cols = rows_in.iter().map(|r| r.len()).max().unwrap_or(0);
        let ncols = self.align_preamble.len().max(max_row_cols);
        let mut widths = vec![0i32; ncols];
        let col_tabskip = |align_preamble: &[ColSpec],
                           loop_start: Option<usize>,
                           t0: &Glue,
                           col: usize|
         -> Glue {
            if align_preamble.is_empty() {
                return t0.clone();
            }
            if col < align_preamble.len() {
                return align_preamble[col].tabskip.clone();
            }
            if let Some(ls) = loop_start {
                let loop_len = align_preamble.len().saturating_sub(ls);
                if loop_len > 0 {
                    let offset = (col - ls) % loop_len;
                    return align_preamble[ls + offset].tabskip.clone();
                }
            }
            t0.clone()
        };
        if self.align_is_valign {
            let mut cols: NodeList = Vec::new();
            cols.push(Node::Glue(self.align_t0.clone()));
            for (c, col) in rows_in.into_iter().enumerate() {
                let mut col_items: NodeList = Vec::new();
                for cell in col {
                    if let Some(box_node) = cell.packed {
                        col_items.push(box_node);
                    }
                }
                let col_vbox =
                    crate::boxes::vpack(col_items, None, crate::boxes::VBOX, &self.eqtb).node;
                cols.push(col_vbox);
                let t = col_tabskip(
                    &self.align_preamble,
                    self.align_loop_start,
                    &self.align_t0,
                    c,
                );
                cols.push(Node::Glue(t));
            }
            let hbox = crate::boxes::hpack(
                cols,
                self.align_to.map(|(d, _)| d),
                crate::boxes::HBOX,
                &self.eqtb,
            )
            .node;
            self.align_preamble.clear();
            self.align_rows.clear();
            self.align_cur_row.clear();
            self.align_adjust.clear();
            self.align_row_adjust.clear();
            self.align_done = false;
            self.align_in_noalign = false;
            self.align_everycr_done = false;
            let nested = self.align_has_save();
            self.align_nested_restore();
            if !nested {
                self.scanner_status = ScannerStatus::Normal;
                self.align_to = None;
                self.align_brace_depth = 0;
                self.align_is_valign = false;
            }
            if self.setbox_target.is_some() && self.setbox_depth == self.box_kinds.len() {
                let idx = self.setbox_target.take().unwrap();
                let g = self.setbox_global;
                self.unpark_setbox();
                self.eqtb.assign_box(idx, Some(hbox), g);
            } else {
                self.append_box_node(Some(hbox));
            }
            return;
        }
        for row in &rows_in {
            for (c, cell) in row.iter().enumerate() {
                if cell.span == NOALIGN_SPAN || cell.packed.is_none() {
                    continue;
                }
                let w = match &cell.packed {
                    Some(Node::Box { w, .. }) => *w,
                    _ => 0,
                };
                if cell.span == 0 {
                    if c < ncols {
                        widths[c] = widths[c].max(w);
                    }
                }
            }
        }

        let mut spans: Vec<(usize, usize, i32)> = Vec::new();
        for row in &rows_in {
            for (c, cell) in row.iter().enumerate() {
                if cell.span != NOALIGN_SPAN && cell.span > 0 && cell.packed.is_some() {
                    let w = match &cell.packed {
                        Some(Node::Box { w, .. }) => *w,
                        _ => 0,
                    };
                    spans.push((c, cell.span as usize, w));
                }
            }
        }
        for j in 0..ncols {
            let mut w = widths[j] as i64;
            for &(c, s, nat) in &spans {
                let end = (c + s).min(ncols.saturating_sub(1));
                if end != j {
                    continue;
                }
                let pre: i64 = (c..j)
                    .map(|k| {
                        widths[k] as i64
                            + col_tabskip(
                                &self.align_preamble,
                                self.align_loop_start,
                                &self.align_t0,
                                k,
                            )
                            .width as i64
                    })
                    .sum();
                w = w.max(nat as i64 - pre);
            }
            widths[j] = w.max(0) as i32;
        }

        self.align_col_widths = widths.clone();
        let mut rows: NodeList = Vec::new();
        let bs = self.eqtb.glue_params[GlueParam::BaselineSkip.idx() as usize].clone();
        let ls = self.eqtb.glue_params[GlueParam::LineSkip.idx() as usize].clone();
        let lsl = self.eqtb.dim_params[DimParam::LineSkipLimit.idx() as usize];
        // tex.web init_align (§115349) inside $$: the align level's
        // prev_depth inherits the enclosing display vlist's prev_depth
        // (push_nest copies the aux record), so append_to_vlist computes
        // interline glue even before the FIRST row — unless the outer
        // prev_depth is the ignore sentinel (no glue). Seed `prev` from
        // self.prev_depth (restored by end_box to the outer value).
        let mut prev: Option<(i32, i32)> =
            if self.mode == Mode::DisplayMath && self.prev_depth > -1000 * 65536 {
                Some((0, self.prev_depth))
            } else {
                None
            };
        // fin_row splices each row's adjustment drain into the vlist raw,
        // directly after the row box and WITHOUT interline glue
        // (append_to_vlist never sees it; prev_depth keeps the row's depth).
        let row_adj = std::mem::take(&mut self.align_row_adjust);
        for (row, adj) in rows_in.into_iter().zip(row_adj) {
            if row.len() == 1 && row[0].span == NOALIGN_SPAN {
                if let Some(node) = row.into_iter().next().and_then(|c| c.packed) {
                    let (items, end_pd) = match node {
                        Node::Box { list, shift, .. } => (list, shift),
                        other => (vec![other], 0),
                    };
                    if end_pd <= -1000 * 65536 {
                        prev = None;
                    } else {
                        let mut last_h = 0;
                        for item in &items {
                            if let Node::Box { h, .. } = item {
                                last_h = *h;
                            }
                        }
                        prev = Some((last_h, end_pd));
                    }
                    rows.extend(items);
                }
                continue;
            }
            let mut line: NodeList = Vec::new();
            line.push(Node::Glue(self.align_t0.clone()));
            let mut g = 0usize;
            for (c, cell) in row.into_iter().enumerate() {
                // grid slots covered by an earlier spanning cell exist only
                // as unpacked placeholders; they contribute no box and no
                // tabskip of their own (the span's target already includes
                // the covered columns and their tabskips)
                if cell.packed.is_none() {
                    g = g.max(c + 1);
                    continue;
                }
                while g < c {
                    if g > 0 {
                        let t = col_tabskip(
                            &self.align_preamble,
                            self.align_loop_start,
                            &self.align_t0,
                            g - 1,
                        );
                        line.push(Node::Glue(t));
                    }
                    g += 1;
                }
                if c > 0 {
                    let t = col_tabskip(
                        &self.align_preamble,
                        self.align_loop_start,
                        &self.align_t0,
                        c - 1,
                    );
                    line.push(Node::Glue(t));
                }
                let s = cell.span as usize;
                let target: i64 = if ncols == 0 {
                    0
                } else {
                    let lo = c;
                    let hi = (c + s).min(ncols - 1);
                    let mut sum = (lo..=hi).map(|i| widths[i] as i64).sum::<i64>();
                    for i in lo..hi {
                        sum += col_tabskip(
                            &self.align_preamble,
                            self.align_loop_start,
                            &self.align_t0,
                            i,
                        )
                        .width as i64;
                    }
                    sum
                };
                let mut inner = match cell.packed {
                    Some(Node::Box { list, .. }) => list,
                    Some(other) => vec![other],
                    None => Vec::new(),
                };
                let (mut s_ord, mut h_ord) = (0u8, 0u8);
                for node in &inner {
                    if let Node::Glue(g) = node {
                        if g.stretch != 0 {
                            s_ord = s_ord.max(g.stretch_order);
                        }
                        if g.shrink != 0 {
                            h_ord = h_ord.max(g.shrink_order);
                        }
                    }
                }
                if s_ord > 0 || h_ord > 0 {
                    for node in &mut inner {
                        if let Node::Glue(g) = node {
                            if s_ord > 0 && g.stretch_order == 0 {
                                g.stretch = 0;
                            }
                            if h_ord > 0 && g.shrink_order == 0 {
                                g.shrink = 0;
                            }
                        }
                    }
                }
                line.push(
                    crate::boxes::hpack(inner, Some(target as i32), crate::boxes::HBOX, &self.eqtb)
                        .node,
                );
                g = c + s + 1;
            }
            while g < ncols {
                if g > 0 {
                    let t = col_tabskip(
                        &self.align_preamble,
                        self.align_loop_start,
                        &self.align_t0,
                        g - 1,
                    );
                    line.push(Node::Glue(t));
                }
                g += 1;
            }
            let last_t = if ncols > 0 {
                col_tabskip(
                    &self.align_preamble,
                    self.align_loop_start,
                    &self.align_t0,
                    ncols - 1,
                )
            } else {
                self.align_t0.clone()
            };
            line.push(Node::Glue(last_t));
            let mut rowbox = match self.align_to {
                None => crate::boxes::hpack(line, None, crate::boxes::HBOX, &self.eqtb).node,
                Some((d, false)) => {
                    crate::boxes::hpack(line, Some(d), crate::boxes::HBOX, &self.eqtb).node
                }
                Some((d, true)) => {
                    let nat = crate::boxes::hpack(line, None, crate::boxes::HBOX, &self.eqtb).node;
                    let w = match &nat {
                        Node::Box { w, .. } => *w,
                        _ => 0,
                    };
                    let list = match nat {
                        Node::Box { list, .. } => list,
                        other => vec![other],
                    };
                    crate::boxes::hpack(
                        list,
                        Some(w.saturating_add(d)),
                        crate::boxes::HBOX,
                        &self.eqtb,
                    )
                    .node
                }
            };
            let (h, d) = match &mut rowbox {
                Node::Box { h, d, list, .. } => {
                    // tex.web §15938: height(r) := height(q); depth(r) := depth(q)
                    // every cell box r in row q inherits the row's height and depth
                    for node in list {
                        if let Node::Box { h: ch, d: cd, .. } = node {
                            *ch = *h;
                            *cd = *d;
                        }
                    }
                    (*h, *d)
                }
                _ => (0, 0),
            };
            align_interline(&mut rows, &mut prev, h, d, &bs, &ls, lsl);
            rows.push(rowbox);
            // tex.web fin_row §15724-5: the row's migrated \\vadjust
            // material follows the row box raw (no interline glue before
            // it; `prev` already holds the row box's height/depth).
            if !adj.is_empty() {
                rows.extend(adj);
            }
        }
        self.align_preamble.clear();
        self.align_rows.clear();
        self.align_cur_row.clear();
        self.align_adjust.clear();
        self.align_row_adjust.clear();
        self.align_done = false;
        self.align_in_noalign = false;
        self.align_everycr_done = false;
        let nested = self.align_has_save();
        self.align_nested_restore();
        if !nested {
            self.scanner_status = ScannerStatus::Normal;
            self.align_to = None;
            self.align_brace_depth = 0;
        }
        if self.setbox_target.is_some() && self.setbox_depth == self.box_kinds.len() {
            let vbox = crate::boxes::vpack(rows, None, crate::boxes::VBOX, &self.eqtb).node;
            let idx = self.setbox_target.take().unwrap();
            let g = self.setbox_global;
            self.unpark_setbox();
            self.eqtb.assign_box(idx, Some(vbox), g);
        } else if self.mode == Mode::InternalVertical {
            // Inside \vbox (e.g. longtable chunks) or \vcenter: append rows
            // directly so \lastbox in \LT@echunk retrieves the last row's hbox
            // and the enclosing vbox packs the rest.
            if let Some((_, d)) = prev {
                self.prev_depth = d;
            }
            self.cur_list.extend(rows);
        } else if self.mode == Mode::DisplayMath {
            // tex.web §16078: the alignment rows (already carrying their
            // interline glue, including the leading glue seeded from the
            // outer vlist's prev_depth) plus \noalign material join the
            // display vlist raw. prev_depth := the align level's final
            // prev_depth (last row's depth), tracked by `prev`.
            let final_pd = prev.map(|(_, d)| d).unwrap_or(self.prev_depth);
            self.prev_depth = final_pd;
            self.display_halign = Some((rows, final_pd));
        } else {
            let vbox = crate::boxes::vpack(rows, None, crate::boxes::VBOX, &self.eqtb).node;
            self.append_box_node(Some(vbox));
        }
    }
}

/// tex.web app_to_vlist: baselineskip glue between rows, lineskip when the
/// gap falls below \lineskiplimit
fn align_interline(
    rows: &mut NodeList,
    prev: &mut Option<(i32, i32)>,
    h: i32,
    d: i32,
    bs: &Glue,
    ls: &Glue,
    lsl: i32,
) {
    if let Some((_, pd)) = *prev {
        let gap = bs.width - pd - h;
        if gap < lsl {
            rows.push(Node::Glue(ls.clone()));
        } else {
            let mut g = bs.clone();
            g.width = gap;
            rows.push(Node::Glue(g));
        }
    }
    *prev = Some((h, d));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;

    fn run_in(e: &mut Engine, src: &str) {
        e.init_primitives();
        e.add_nullfont();
        // unbooted engine: catcodes default to plain-like INITEX values only
        // after \catcode assignments, so set the alignment-relevant ones
        let full = format!(
            "\\catcode`\\{{=1 \\catcode`\\}}=2 \\catcode`\\#=6 \\catcode`\\&=4 \\baselineskip=12pt {}\n",
            src
        );
        e.input
            .push_file("driver.tex".to_string(), full.as_bytes().to_vec());
        e.run();
    }

    fn run(src: &str) -> Engine {
        let mut e = Engine::new(true);
        run_in(&mut e, src);
        e
    }

    fn vbox_of(e: &Engine) -> (i32, &NodeList) {
        let n = e
            .page_list
            .iter()
            .find(|n| matches!(n, Node::Box { kind, .. } if *kind == crate::boxes::VBOX))
            .expect("alignment vbox on page list");
        match n {
            Node::Box { w, list, .. } => (*w, list),
            _ => unreachable!(),
        }
    }

    fn row_of(n: &Node) -> &NodeList {
        match n {
            Node::Box { kind, list, .. } if *kind == crate::boxes::HBOX => list,
            other => panic!("expected hbox row, got {:?}", other),
        }
    }

    fn box_w(n: &Node) -> i32 {
        match n {
            Node::Box { w, .. } => *w,
            other => panic!("expected box, got {:?}", other),
        }
    }

    #[test]
    fn abandoned_nested_alignment_cannot_affect_a_later_engine() {
        let mut engine = Box::new(Engine::new(true));
        let address = (&*engine) as *const Engine;
        run_in(&mut engine, r"\vbox{\halign{#\cr \vbox{\halign{#\cr inner");
        engine.finish_job_diagnostics();
        assert!(engine.stopped_on_error);
        assert_eq!(engine.align_stack.len(), 1);

        // Replace the engine without changing its allocation. The old
        // address-keyed thread-local stack would now match this new engine.
        *engine = Engine::new(true);
        assert_eq!((&*engine) as *const Engine, address);
        run_in(&mut engine, r"\halign{#\cr ok\cr}\end");
        assert_eq!(
            engine.error_count, 0,
            "later engine inherited alignment state:\n{}",
            engine.diagnostic_output
        );
        assert!(engine.align_stack.is_empty());
    }

    #[test]
    fn semisimple_group_does_not_hide_alignment_delimiters() {
        // expl3/siunitx uses \begingroup while looking ahead for a cell
        // delimiter; the v-part closes that group after the delimiter fires.
        let e = run("\\setbox0=\\vbox{\\halign{#\\endgroup\\cr \\begingroup a\\cr}}\n");
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        assert!(e.eqtb.boxed[0].is_some());
    }

    #[test]
    fn halign_two_rows_two_columns() {
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\def\\quad{\\hskip1em\\relax}\n",
            "\\halign{#\\hfil\\quad&#\\hfil\\cr a&bb\\cr ccc&d\\cr}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        let (w, list) = vbox_of(&e);
        assert_eq!(list.len(), 3, "2 rows + interrow glue, got {:?}", list);
        let (w0, w1) = (e.align_col_widths[0], e.align_col_widths[1]);
        assert!(w0 > 0 && w1 > 0);
        // both rows packed to the same total width
        assert_eq!(box_w(&list[0]), w0 + w1);
        assert_eq!(box_w(&list[2]), w0 + w1);
        assert_eq!(w, w0 + w1);
        // row 1 = [T_0, cell(a), tabskip, cell(bb), T_n]
        let r1 = row_of(&list[0]);
        assert_eq!(r1.len(), 5);
        assert_eq!(box_w(&r1[1]), w0);
        assert_eq!(box_w(&r1[3]), w1);
        // cell(a) + \quad stretched to the width of 'ccc': fil glue set
        match &r1[1] {
            Node::Box {
                glue_sign,
                glue_set,
                ..
            } => {
                assert_eq!(*glue_sign, 1);
                assert!(*glue_set > 0.0);
            }
            _ => unreachable!(),
        }
        // row 2 = [T_0, cell(ccc), tabskip, cell(d), T_n]
        let r2 = row_of(&list[2]);
        assert_eq!(r2.len(), 5);
        assert_eq!(box_w(&r2[1]), w0);
        assert_eq!(box_w(&r2[3]), w1);
        // 'ccc' at natural width: no glue set
        match &r2[1] {
            Node::Box { glue_sign, .. } => assert_eq!(*glue_sign, 0),
            _ => unreachable!(),
        }
        // column 1 is as wide as 'ccc', column 2 as wide as 'bb'
        let f = &e.eqtb.fonts[e.eqtb.cur_font_val as usize];
        let wc = f.char_width(b'c');
        let wb = f.char_width(b'b');
        // column 1 = max('ccc', 'a' + \quad); \quad = 1em = cmr10 quad
        // (655361su under tex.web's truncating store_scaled)
        assert_eq!(w0, wc * 3 + 655361);
        assert_eq!(w1, wb * 2);
    }

    #[test]
    fn noalign_hrule_between_rows() {
        // \hbox cells exercise the begin_box-inside-a-cell path (LaTeX
        // \@arstrut / rule boxes take this route in real tabulars)
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\halign{#\\cr \\hbox{a}\\cr \\noalign{\\hrule} \\hbox{bb}\\cr}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        let (w, list) = vbox_of(&e);
        let boxes = list
            .iter()
            .filter(|n| matches!(n, Node::Box { .. }))
            .count();
        // tex.web: \noalign material splices into the alignment vlist raw —
        // the \hrule sits directly between the row boxes, and the final pack
        // resolved its running width to the alignment width (the widest row)
        let rule = list
            .iter()
            .find_map(|n| match n {
                Node::Rule { width, .. } => Some(*width),
                _ => None,
            })
            .expect("noalign \\hrule spliced into the alignment vlist");
        let row_w = list
            .iter()
            .filter_map(|n| match n {
                Node::Box { w, .. } => Some(*w),
                _ => None,
            })
            .max()
            .unwrap();
        assert_eq!(rule, row_w, "rule takes alignment width");
        assert_eq!(w, row_w, "alignment vbox width = widest row");
        // row / rule / row order
        assert!(matches!(list[0], Node::Box { .. }));
        assert!(matches!(list[list.len() - 1], Node::Box { .. }));
        assert_eq!(boxes, 2, "two row boxes, no noalign wrapper: {:?}", list);
    }

    #[test]
    fn noalign_vbox_paragraph_closes_before_alignment() {
        // amsmath's overwide-display marker uses this exact shape:
        // \noalign{\vbox{\noindent\hbox to<width>{...}}}. The paragraph
        // started by \noindent must end when the vbox closes even though the
        // alignment scanner is between rows.
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\halign{#\\cr a\\cr",
            "\\noalign{\\vbox{\\noindent\\hbox to20pt{\\hfil}}}",
            "}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        assert_eq!(e.mode, Mode::Vertical);
        assert!(
            e.saved_lists.is_empty(),
            "saved list leak: {:?}",
            e.saved_lists
        );
        assert!(e.box_kinds.is_empty(), "box stack leak: {:?}", e.box_kinds);
    }

    #[test]
    fn span_cell_covers_two_columns() {
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\halign{#\\hfil& #\\hfil& #\\hfil\\cr a& \\span\\omit bb\\cr}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        assert_eq!(e.align_col_widths.len(), 3);
        let (w, list) = vbox_of(&e);
        assert_eq!(list.len(), 1, "single row: {:?}", list);
        // tex.web fin_align: a span contributes only to its LAST column;
        // 'bb' is covered by columns 1-2, so column 1 stays 0 and column 2
        // carries the natural width w(bb)
        let f = &e.eqtb.fonts[e.eqtb.cur_font_val as usize];
        let wb = f.char_width(b'b');
        let (w1, w2) = (e.align_col_widths[1], e.align_col_widths[2]);
        assert_eq!(w1, 0, "col 1 stays 0: {:?}", e.align_col_widths);
        assert_eq!(w2, wb * 2, "col 2 gets span: {:?}", e.align_col_widths);
        let r = row_of(&list[0]);
        assert_eq!(r.len(), 5, "T_0, cell, tabskip, span cell, T_n: {:?}", r);
        assert_eq!(box_w(&r[3]), w1 + w2);
        assert_eq!(w, box_w(&r[1]) + w1 + w2);
    }

    #[test]
    fn span_deficit_lands_on_last_column() {
        // tex.web: w_j = max over spans ending at j of
        // w_ij - sum_{k=i}^{j-1}(tabskip_k + w_k); the wide \multicolumn
        // cell widens only column 2, columns 0 and 1 stay natural
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\halign{#\\hfil& #\\hfil& #\\hfil\\cr a&b&c\\cr \\span\\omit XXXXX\\cr}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        let f = &e.eqtb.fonts[e.eqtb.cur_font_val as usize];
        let w = |ch: u8| f.char_width(ch) as i64;
        let widths = e.align_col_widths.clone();
        assert_eq!(widths[0] as i64, w(b'a'));
        let nat = w(b'X') * 5;
        let expect1 = nat - w(b'a');
        assert_eq!(
            widths[1] as i64, expect1,
            "deficit on column 1 (last column of 2-col span)"
        );
        assert_eq!(widths[2] as i64, w(b'c'), "col 2 stays natural");
    }

    #[test]
    fn tabular_style_halign_in_hbox() {
        // LaTeX tabular wraps its \halign in \hbox: the alignment must be
        // legal in restricted horizontal mode and land as a box in the
        // surrounding \hbox
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\hbox{\\halign{#\\hfil& #\\hfil\\cr a&bb\\cr ccc&d\\cr}}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        // the page list holds the outer hbox
        let outer = e
            .page_list
            .iter()
            .find(|n| matches!(n, Node::Box { kind, .. } if *kind == crate::boxes::HBOX))
            .expect("outer hbox on page list");
        let inner = match outer {
            Node::Box { list, .. } => list,
            _ => unreachable!(),
        };
        assert_eq!(inner.len(), 1, "alignment vbox inside hbox: {:?}", inner);
        match &inner[0] {
            Node::Box { w, .. } => {
                let (w0, w1) = (e.align_col_widths[0], e.align_col_widths[1]);
                assert!(w0 > 0 && w1 > 0);
                assert_eq!(*w, w0 + w1);
            }
            other => panic!("expected alignment vbox: {:?}", other),
        }
    }

    #[test]
    fn halign_to_and_spread_widths() {
        // \tabskip with fil stretch absorbs the to/spread difference
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\tabskip=0pt plus 1fil\n",
            "\\vbox{\\halign to 120pt{#\\hfil& #\\hfil\\cr a&bb\\cr ccc&d\\cr}}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        let (_, list) = vbox_of(&e);
        let target = 120 * 65536;
        for n in list.iter().filter(|n| matches!(n, Node::Box { .. })) {
            match n {
                Node::Box { w, .. } => assert_eq!(*w, target, "row packed to 120pt"),
                _ => unreachable!(),
            }
        }
        // column widths stay natural under `to`
        let f = &e.eqtb.fonts[e.eqtb.cur_font_val as usize];
        let wc3 = f.char_width(b'c') * 3;
        assert_eq!(e.align_col_widths[0], wc3, "natural col 0");

        let e2 = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\tabskip=0pt plus 1fil\n",
            "\\vbox{\\halign spread 20pt{#\\hfil& #\\hfil\\cr a&bb\\cr}}\n",
        ));
        assert_eq!(e2.error_count, 0, "errors:\n{}", e2.term);
        let (_, list2) = vbox_of(&e2);
        let f2 = &e2.eqtb.fonts[e2.eqtb.cur_font_val as usize];
        let nat = f2.char_width(b'a') + f2.char_width(b'b') * 2;
        match &list2[0] {
            Node::Box { w, .. } => assert_eq!(*w, nat + 20 * 65536, "spread row width"),
            other => panic!("expected row box: {:?}", other),
        }
    }

    #[test]
    fn nested_halign_keeps_outer_aligning() {
        // an inner \halign inside a cell must restore scanner_status so the
        // outer alignment's \cr still works
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\vbox{\\halign{#\\hfil\\cr \\vbox{\\halign{#\\hfil\\cr xx\\cr y\\cr}}\\cr}}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        let (_, list) = vbox_of(&e);
        assert_eq!(list.len(), 1, "one outer row: {:?}", list);
        // in TeX (§800), \halign in vmode appends rows directly to the enclosing vbox
        let row = row_of(&list[0]);
        match &row[1] {
            Node::Box {
                list: inner_cell, ..
            } => {
                assert!(
                    matches!(inner_cell[0], Node::Box { .. }),
                    "{:?}",
                    inner_cell
                )
            }
            other => panic!("expected cell box: {:?}", other),
        }
    }

    #[test]
    fn crcr_terminates_row() {
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\halign{#\\cr a\\crcr bb\\crcr}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        let (_, list) = vbox_of(&e);
        assert_eq!(list.len(), 3, "2 rows + glue: {:?}", list);
        assert_eq!(e.align_col_widths[0], {
            let f = &e.eqtb.fonts[e.eqtb.cur_font_val as usize];
            f.char_width(b'b') * 2
        });
    }

    #[test]
    fn ampersand_in_braces_is_misplaced() {
        // braces hide & from the row: tex.web reports a misplaced tab
        // instead of splitting the cell
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\halign{#\\cr {a&b}\\cr}\n",
        ));
        assert!(e.error_count >= 1, "expected misplaced tab error");
    }

    #[test]
    fn finite_glue_frozen_in_unset_box() {
        // tex.web unset boxes: finite glue (order 0) does not stretch when
        // higher-order (\hfil) stretch is present
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\halign{#\\hskip 0pt plus 2pt\\hfil\\cr a\\cr bb\\cr}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        let (_, list) = vbox_of(&e);
        let f = &e.eqtb.fonts[e.eqtb.cur_font_val as usize];
        let deficit = f.char_width(b'b') * 2 - f.char_width(b'a');
        let r = row_of(&list[0]);
        match &r[1] {
            Node::Box {
                glue_sign,
                glue_set,
                ..
            } => {
                assert_eq!(*glue_sign, 1, "stretching");
                // only the 1fil glue stretches: glue_set = deficit / fil
                let expected = deficit as f64 / 65536.0;
                assert!(
                    (glue_set - expected).abs() < 1e-3,
                    "fil-only stretch: glue_set={} expected={}",
                    glue_set,
                    expected
                );
            }
            other => panic!("expected cell box: {:?}", other),
        }
    }

    #[test]
    fn booktabs_style_table_skeleton() {
        // a LaTeX-tabular-shaped alignment inside \hbox: \toprule-like
        // \noalign rule before the first row, a \multicolumn header
        // (\span\omit), body rows, \midrule, and a \bottomrule followed by
        // \crcr before the closing brace (like \endtabular)
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\def\\br{\\noalign{\\hrule}}\n",
            "\\hbox{\\halign{\\hfil#& \\hfil#\\hfil& #\\hfil\\cr\n",
            "\\br\n",
            "\\span\\omit \\hfil Header\\hfil\\cr\n",
            "\\br\n",
            "a&bb&ccc\\cr\n",
            "d&e&f\\cr\n",
            "\\br\n",
            "\\crcr}}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        // alignment vbox sits inside the hbox
        let outer = e
            .page_list
            .iter()
            .find(|n| matches!(n, Node::Box { kind, .. } if *kind == crate::boxes::HBOX))
            .expect("outer hbox on page list");
        let align_box = match outer {
            Node::Box { list, .. } => match &list[0] {
                Node::Box { list, .. } => list,
                other => panic!("expected alignment vbox: {:?}", other),
            },
            _ => unreachable!(),
        };
        // 3 rules + 3 rows, interleaved with glue; tex.web splices noalign
        // material raw, so the rules sit directly in the alignment vlist
        // with their running width resolved to the alignment width
        let rows: Vec<&Node> = align_box
            .iter()
            .filter(|n| matches!(n, Node::Box { .. }))
            .collect();
        assert_eq!(rows.len(), 3, "rows: {:?}", align_box);
        let rules: Vec<i32> = align_box
            .iter()
            .filter_map(|n| match n {
                Node::Rule { width, .. } => Some(*width),
                _ => None,
            })
            .collect();
        assert_eq!(rules.len(), 3, "top/mid/bottom rules: {:?}", align_box);
        let align_w = rows
            .iter()
            .filter_map(|n| match n {
                Node::Box { w, .. } => Some(*w),
                _ => None,
            })
            .max()
            .unwrap();
        assert!(
            rules.iter().all(|rw| *rw == align_w),
            "rules span the alignment width {align_w}: {rules:?}"
        );
        // the \multicolumn header spans all three columns
        let w = |ch: u8| e.eqtb.fonts[e.eqtb.cur_font_val as usize].char_width(ch) as i64;
        let expect_w2 = (w(b'H') + w(b'e') + w(b'a') + w(b'd') + w(b'r'))
            - e.align_col_widths[0] as i64
            - e.align_col_widths[1] as i64;
        assert!(
            e.align_col_widths[2] as i64 >= expect_w2 - 1,
            "header deficit on col 2: {:?}",
            e.align_col_widths
        );
    }

    #[test]
    fn consecutive_math_shifts_in_alignment_stay_inline() {
        // LaTeX array templates can contain `$$$` before the parameter
        // marker. In restricted horizontal mode these are three successive
        // inline-math toggles, not a display-math opener plus a closer.
        let e = run(concat!(
            "\\catcode`\\$=3\n",
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\halign{$$$#$\\cr a\\over b\\cr}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    }

    #[test]
    fn misplaced_align_tokens_error() {
        // stray alignment tokens outside any alignment
        let e = run("x & y \\cr\n");
        assert!(e.error_count >= 1);
    }

    // begin_box now consumes the `{` itself (build.rs fix), so the outer
    // \vbox completes and the alignment vbox reaches the page list
    #[test]
    fn halign_inside_vbox() {
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\vbox{\\halign{#\\hfil\\cr a\\cr bb\\cr}}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        // in TeX (§800), \halign in vmode appends rows directly to the enclosing vbox
        let (_, list) = vbox_of(&e);
        assert_eq!(list.len(), 3, "2 rows + interrow glue: {:?}", list);
        let w = e.align_col_widths[0];
        assert_eq!(box_w(&list[0]), w);
        assert_eq!(box_w(&list[2]), w);
    }

    #[test]
    fn macro_argument_cannot_discard_alignment_row_end() {
        // tex.web get_next inserts the v-part before a parameter scanner can
        // consume the source \cr. Otherwise \eat would discard the row end.
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\def\\eat#1{}\n",
            "\\setbox0=\\vbox{\\halign{#\\relax\\cr A\\eat\\cr}}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        assert!(e.eqtb.boxed[0].is_some(), "alignment box missing");
    }

    #[test]
    fn template_box_group_does_not_hide_row_end_from_macro_scanner() {
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\def\\eat#1{}\n",
            "\\setbox0=\\vbox{\\halign{\\hbox\\bgroup#\\egroup\\cr A\\eat\\cr}}\n",
            "\\count0=37\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        assert_eq!(e.eqtb.count[0], 37, "input after alignment was not reached");
    }

    #[test]
    fn control_sequence_begin_group_does_not_hide_alignment_tab() {
        // TeX's alignment brace counter follows explicit brace tokens. A
        // control sequence \let to `{` starts a group but does not protect
        // a top-level alignment delimiter while a macro argument is scanned.
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\def\\scan#1X{Q}\n",
            "\\let\\bgroup={\\let\\egroup=}\n",
            "\\halign{#\\hfil&#\\hfil\\cr\n",
            "\\bgroup\\scan A&BX\\egroup L&R\\cr}\n",
        ));
        assert!(
            e.error_count > 0,
            "a control-sequence group incorrectly hid the alignment tab"
        );
    }
    #[test]
    fn nested_alignment_inside_alignment_cell() {
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\def\\mat#1{\\vbox{\\halign{##\\hfil&##\\hfil\\cr #1\\crcr}}}\n",
            "\\halign{#\\hfil\\cr \\mat{a&b\\cr c&d\\cr}\\cr}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
    }

    // ------------------------------------------------------------------
    // \vadjust migration out of alignment cells (tex.web fin_col
    // §15666-8 / fin_row §15724-5)
    // ------------------------------------------------------------------

    fn box0_list(e: &Engine) -> &NodeList {
        match e.eqtb.boxed[0].as_ref().expect("box register 0 assigned") {
            Node::Box { list, .. } => list,
            other => panic!("expected vbox, got {other:?}"),
        }
    }

    fn box_h(n: &Node) -> i32 {
        match n {
            Node::Box { h, .. } => *h,
            other => panic!("expected box, got {other:?}"),
        }
    }

    fn glue_w(n: &Node) -> i32 {
        match n {
            Node::Glue(g) => g.width,
            other => panic!("expected glue, got {other:?}"),
        }
    }
    #[test]
    fn vadjust_cell_migrates_after_its_row() {
        // a \\vadjust inside a (tabular-shaped) halign cell must leave the
        // cell's hlist when the row packs and join the alignment vlist
        // directly after the completed row; the enclosing vbox grows by
        // exactly the migrated skip.
        let base =
            "\\font\\cmr=cmr10 \\cmr\n\\setbox0=\\vbox{\\halign{#\\hfil\\cr %s\\cr c\\cr}}\n";
        let plain = run(&base.replace("%s", "a"));
        let adj = run(&base.replace("%s", "a\\vadjust{\\vskip2pt}"));
        assert_eq!(plain.error_count, 0, "errors:\n{}", plain.term);
        assert_eq!(adj.error_count, 0, "errors:\n{}", adj.term);
        // baseline: row, interline glue, row (adjustment material absent)
        let p = box0_list(&plain);
        assert_eq!(p.len(), 3, "baseline vlist: {p:?}");
        // migrated: row, 2pt, interline glue, row — the skip sits AFTER
        // its row box, never inside it (tex.web fin_row §15724-5)
        let l = box0_list(&adj);
        assert_eq!(l.len(), 4, "migrated vlist: {l:?}");
        assert!(matches!(l[0], Node::Box { .. }));
        assert_eq!(glue_w(&l[1]), 2 * 65536, "row 1 adjustment");
        assert!(matches!(l[2], Node::Glue(_)), "interline glue after it");
        assert_eq!(
            glue_w(&l[2]),
            glue_w(&p[1]),
            "the splice must not disturb interline glue (prev_depth keeps the row depth)"
        );
        assert!(matches!(l[3], Node::Box { .. }), "row 2 box");
        // row boxes must not contain the adjustment node
        for r in [0usize, 3] {
            assert!(
                row_of(&l[r]).iter().all(|n| !matches!(n, Node::VAdjust(_))),
                "vadjust stuck in row {r}"
            );
        }
        // consumer-visible height: the skip absorbs row 1's running depth
        // and adds its full width to the vbox total
        let ph = box_h(plain.eqtb.boxed[0].as_ref().unwrap());
        let ah = box_h(adj.eqtb.boxed[0].as_ref().unwrap());
        assert_eq!(ah, ph + 2 * 65536, "migrated 2pt skip raises the box");
    }

    #[test]
    fn vadjust_multi_cell_source_order() {
        // two cells in one row contribute their adjustments in cell/source
        // order after the row (tex.web cur_tail accumulation in hpack order)
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\setbox0=\\vbox{\\halign{#\\hfil&#\\hfil\\cr ",
            "a\\vadjust{\\vskip1pt}&b\\vadjust{\\vskip2pt}\\cr}}\n",
        ));
        assert_eq!(
            e.error_count,
            0,
            "errors: {:?}",
            e.diagnostics.iter().map(|d| d.render()).collect::<Vec<_>>()
        );
        let l = box0_list(&e);
        assert_eq!(l.len(), 3, "row + two adjustments: {l:?}");
        assert!(matches!(l[0], Node::Box { .. }));
        assert_eq!(glue_w(&l[1]), 1 * 65536, "cell 1 first");
        assert_eq!(glue_w(&l[2]), 2 * 65536, "cell 2 after");
    }

    #[test]
    fn vadjust_nested_alignment_stays_in_inner_level() {
        // an inner \\halign inside a cell owns its own adjustment level
        // (tex.web push_alignment saves cur_head/cur_tail): the inner
        // \\vskip must land in the inner vbox, not after the outer row
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\setbox0=\\vbox{\\halign{#\\vadjust{\\vskip1pt}\\cr ",
            "\\vbox{\\halign{#\\vadjust{\\vskip3pt}\\cr x\\cr}}\\cr}}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        let outer = box0_list(&e);
        // outer level: row box then the outer row's own 1pt adjustment;
        // the inner 3pt skip never escapes into the outer vlist
        assert_eq!(outer.len(), 2, "outer vlist: {outer:?}");
        assert!(matches!(outer[0], Node::Box { .. }));
        assert_eq!(glue_w(&outer[1]), 1 * 65536);
        // descend: outer row -> cell box -> inner vbox -> inner row + 3pt
        let cell = row_of(&outer[0])
            .iter()
            .find(|n| matches!(n, Node::Box { .. }))
            .expect("cell box in outer row");
        let inner_vbox = row_of(cell)
            .iter()
            .find(|n| matches!(n, Node::Box { .. }))
            .expect("inner vbox in cell");
        let inner_list = box0_items(inner_vbox);
        assert_eq!(inner_list.len(), 2, "inner row + inner adjustment");
        assert!(matches!(inner_list[0], Node::Box { .. }));
        assert_eq!(glue_w(&inner_list[1]), 3 * 65536);
    }

    fn box0_items(n: &Node) -> &NodeList {
        match n {
            Node::Box { list, .. } => list,
            other => panic!("expected box, got {other:?}"),
        }
    }

    #[test]
    fn vadjust_row_boundary_does_not_leak() {
        // a fresh row starts with an empty adjustment drain even when the
        // previous row migrated material (tex.web init_row §15537)
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\setbox0=\\vbox{\\halign{#\\cr ",
            "a\\vadjust{\\vskip2pt}\\cr b\\cr}}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        let l = box0_list(&e);
        // row1, 2pt, interline, row2 — row 2 has no adjustment and the
        // accumulator was reset for it
        assert_eq!(l.len(), 4, "no leaked second adjustment: {l:?}");
        assert_eq!(glue_w(&l[1]), 2 * 65536);
        assert!(matches!(l[2], Node::Glue(_)));
        assert!(matches!(l[3], Node::Box { .. }));
    }

    #[test]
    fn everycr_noalign_paragraph_unpacks_cleanly() {
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\everycr{\\noalign{X}}\n",
            "\\setbox0=\\vbox{\\halign{#\\crcr 1\\crcr}}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        let h = box_h(e.eqtb.boxed[0].as_ref().expect("box 0"));
        // 30.83331pt = 2020692sp
        assert_eq!(h, 2020692, "expected 30.83331pt, got {h}");
    }

    #[test]
    fn token_list_expansion_with_ampersand_does_not_close_cell() {
        // Expanding macros whose replacement texts define inner macros with &
        // must not fire alignment tab interception while scanning the definition.
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\def\\definer{\\def\\rowcontent{1 & 2}}\n",
            "\\halign{#\\hfil&#\\hfil\\cr \\definer \\rowcontent\\cr}\n",
        ));
        assert_eq!(
            e.error_count,
            0,
            "errors: {:?}",
            e.diagnostics.iter().map(|d| d.render()).collect::<Vec<_>>()
        );
    }
    #[test]
    fn nested_halign_preserves_outer_align_brace_depth() {
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\halign{#\\hfil&#\\hfil\\cr\n",
            "1&\\vbox{\\halign{#\\cr a\\cr b\\crcr}}\\vbox{\\halign{#&#\\cr c&d\\cr}}\\cr\n",
            "2&3\\cr}\n"
        ));
        assert_eq!(
            e.error_count,
            0,
            "errors: {:?}",
            e.diagnostics.iter().map(|d| d.render()).collect::<Vec<_>>()
        );
    }
    #[test]
    fn alphabetic_char_constant_brace_does_not_corrupt_align_brace_depth() {
        // tex.web §442: alphabetic char constant `\ifnum 0=`}\fi` must not
        // leave align_state negative. A subsequent macro definition containing
        // \cr or & must not have its body cut off by premature delimiter interception.
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\halign{#\\hfil&#\\hfil\\cr\n",
            "  \\ifnum 0=`}\\fi\n",
            "  \\gdef\\temp{\\cr}\n",
            "  1 & 2\\temp}\n"
        ));
        assert_eq!(
            e.error_count,
            0,
            "errors: {:?}",
            e.diagnostics.iter().map(|d| d.render()).collect::<Vec<_>>()
        );
    }
    #[test]
    fn fraction_in_align_environment_does_not_corrupt_align_brace_depth() {
        // \frac ends with \over #2} which is scanned by scan_math_rest_of_group.
        // The closing brace must be put back with push_token (not pushed.push)
        // so align_brace_depth is not double-decremented.
        let e = run(concat!(
            "\\catcode`\\$=3\n",
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\halign{#\\hfil&#\\hfil\\cr\n",
            "  ${1\\over 2}$ & 2\\cr\n",
            "  3 & 4\\cr}\n"
        ));
        assert_eq!(
            e.error_count,
            0,
            "errors: {:?}",
            e.diagnostics.iter().map(|d| d.render()).collect::<Vec<_>>()
        );
    }
    #[test]
    fn ifcase_skipping_tabs_in_alignment_does_not_prematurely_advance_columns() {
        // tex.web §494 / line 9663: scanner_status is set to `skipping` during
        // conditional pass_text / skip_branch / skip_case_skip. Delimiters `&`
        // in skipped branches (e.g. LaTeX's \@@eqncr \ifcase\@eqcnt & & &\or & &\or &\fi)
        // must not be intercepted by align_intercept_raw_token.
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\def\\testeqncr{\\ifcase 0 & & &\\or & &\\or &\\else\\fi\\cr}\n",
            "\\halign{#&#&#&#\\cr\n",
            "  a \\testeqncr\n",
            "  b \\testeqncr}\n",
        ));
        assert_eq!(
            e.error_count,
            0,
            "errors: {:?}",
            e.diagnostics.iter().map(|d| d.render()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn macro_definition_and_arguments_with_ampersand_respect_align_brace_depth() {
        // tex.web @7257-7264 / @7335 / @7492: align_state tracks cumulative
        // brace balance; raw tokens for `&` inside braces (such as macro
        // bodies or expl3 token list comparisons) are not intercepted by the
        // outer alignment delimiter scanner.
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\halign{#&#\\cr\n",
            "  \\def\\foo#1{\\def\\tmp{#1}}%\n",
            "  \\foo{x & y}%\n",
            "  1 & 2\\cr}\n",
        ));
        assert_eq!(
            e.error_count,
            0,
            "errors: {:?}",
            e.diagnostics.iter().map(|d| d.render()).collect::<Vec<_>>()
        );
    }
    #[test]
    fn hash_brace_parameter_delimiter_does_not_leak_align_brace_depth() {
        // tex.web §392 / @8063: When a macro parameter delimiter ends with `#{`,
        // both the delimiter match and the subsequent macro body scan encounter
        // a left brace. TeX decrements align_state to prevent double-counting.
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\halign{#&#\\cr\n",
            "  \\def\\macrodelim#1#{\\def\\macrobody{#1}}%\n",
            "  \\macrodelim text {body content}%\n",
            "  col1 & col2\\cr}\n",
        ));
        assert_eq!(
            e.error_count,
            0,
            "errors: {:?}",
            e.diagnostics.iter().map(|d| d.render()).collect::<Vec<_>>()
        );
    }
    #[test]
    fn control_sequence_let_to_ampersand_is_intercepted_as_alignment_tab() {
        // tex.web @7264: A control sequence \let to `&` (such as amscd's
        // \ampersand@) has cur_cmd == tab_mark and must be intercepted
        // as an alignment tab when align_state == 0.
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\let\\myamp=&\n",
            "\\halign{#&#\\cr\n",
            "  col1 \\myamp col2\\cr}\n",
        ));
        assert_eq!(
            e.error_count,
            0,
            "errors: {:?}",
            e.diagnostics.iter().map(|d| d.render()).collect::<Vec<_>>()
        );
    }
    #[test]
    fn afterassignment_closing_brace_does_not_corrupt_align_brace_depth() {
        // \afterassignment saving `}` followed by an assignment inside an alignment
        // cell must not double-decrement align_brace_depth.
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\toksdef\\mytoks=0\n",
            "\\def\\nowrite#1#{{\\afterassignment}\\mytoks}\n",
            "\\halign{#&#\\cr\n",
            "  \\nowrite\\unused{\\string\\foo{bar}}%\n",
            "  col1 & col2\\cr}\n",
        ));
        assert_eq!(
            e.error_count,
            0,
            "errors: {:?}",
            e.diagnostics.iter().map(|d| d.render()).collect::<Vec<_>>()
        );
    }
    #[test]
    fn u_template_exhaustion_switches_phase_before_cell_content_peek() {
        // When a u-template macro peeks past the u-part, u-template exhaustion
        // must pop U_PART_SRC and transition to PH_CONTENT so that \futurelet
        // does not delay u-template completion past subsequent brace-depth updates.
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\def\\cellbegin#1\\relax{\\futurelet\\next\\afterpeek}\n",
            "\\def\\afterpeek#1{#1}\n",
            "\\halign{\\cellbegin\\relax #\\cr\n",
            "  123\\cr}\n",
        ));
        assert_eq!(
            e.error_count,
            0,
            "errors: {:?}",
            e.diagnostics.iter().map(|d| d.render()).collect::<Vec<_>>()
        );
    }
    #[test]
    fn expl3_style_macro_scan_with_embedded_ampersand_in_alignment_cell() {
        // Reproduces array.sty \tl_if_in:nnTF where an expanded token list
        // containing `&` is scanned as a delimited macro argument inside
        // an alignment cell. tex.web @7257-7264: align_state protects the `&`
        // while enclosed in braces across macro expansions.
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\def\\search#1&{\\def\\found{#1}}\n",
            "\\def\\wrapper#1{\\search#1}\n",
            "\\halign{#&#\\cr\n",
            "  \\wrapper{{nested & tokens}&}%\n",
            "  cell1 & cell2\\cr}\n",
        ));
        assert_eq!(
            e.error_count,
            0,
            "errors: {:?}",
            e.diagnostics.iter().map(|d| d.render()).collect::<Vec<_>>()
        );
    }
    #[test]
    fn consecutive_cr_creates_empty_row() {
        // tex.web §15518-15522: between rows, only \crcr is ignored.
        // A true \cr starts a new row whose first cell closes immediately.
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\setbox0=\\vbox{\\halign{#\\cr\n",
            "  row1\\cr\n",
            "  \\cr\n",
            "  row3\\cr}}\n",
        ));
        assert_eq!(e.error_count, 0);
        let b = e.eqtb.boxed[0].as_ref().unwrap();
        let Node::Box { list, .. } = b else {
            panic!("expected vbox");
        };
        let hboxes: Vec<_> = list
            .iter()
            .filter(|n| matches!(n, Node::Box { .. }))
            .collect();
        assert_eq!(
            hboxes.len(),
            3,
            "expected 3 row boxes for 2 non-empty + 1 empty row"
        );
    }

    #[test]
    fn valign_packages_columns_into_hbox() {
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\setbox0=\\hbox{\\valign{#\\cr\n",
            "  \\vfil\\hbox{A}\\vfil\\cr\n",
            "  \\hbox{B}\\crcr}}\n",
        ));
        assert_eq!(e.error_count, 0);
        let b = e.eqtb.boxed[0].as_ref().unwrap();
        let Node::Box { list, .. } = b else {
            panic!("expected hbox");
        };
        let valign_box = list.iter().find(|n| matches!(n, Node::Box { .. })).unwrap();
        let Node::Box { list: vcols, .. } = valign_box else {
            panic!("expected valign hbox");
        };
        let vboxes: Vec<_> = vcols
            .iter()
            .filter(|n| matches!(n, Node::Box { .. }))
            .collect();
        assert_eq!(vboxes.len(), 2, "expected 2 column vboxes");
    }

    #[test]
    fn span_in_alignment_cell_unwinds_v_part_and_absorbs_column() {
        // Preamble with math templates in both columns: $#$ & $##$.
        // A \span in column 1 must unwind column 1's v-part (closing math)
        // and play column 2's u-part (opening math), without error.
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\setbox0=\\vbox{\\halign{$#$&$#$\\cr\n",
            "  x\\span y\\cr}}\n",
        ));
        assert_eq!(e.error_count, 0);
        let b = e.eqtb.boxed[0].as_ref().unwrap();
        let Node::Box { list, .. } = b else {
            panic!("expected vbox");
        };
        let hboxes: Vec<_> = list
            .iter()
            .filter(|n| matches!(n, Node::Box { .. }))
            .collect();
        assert_eq!(hboxes.len(), 1, "expected 1 row box");
    }
}
