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
//!   typesets its text in the alignment's own mode and stores the raw list
//!   as a row entry marked with span NOALIGN_SPAN.
//! * When the alignment group's `}` arrives (build.rs end_box, kind 7),
//!   `finish_halign` follows tex.web fin_align: column widths (with span
//!   merging and nullified empty columns), the preamble packed to the
//!   requested size, and every row/cell set from the preamble's glue.
//!   \halign rows get interline glue and join the enclosing vertical list;
//!   \valign rows join the enclosing horizontal list.
//!
//! Known simplifications versus tex.web:
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
    /// LuaTeX's `append_to_vlist_filter` decision for the row, kept on the
    /// row's first cell
    pub ctl: RowCtl,
}

/// What the `fin_row` callbacks made of a finished row (see
/// `lua_align.rs`): `handled` means `append_to_vlist_filter` ran, so TeX adds
/// no interline glue; `depth` is the `prev_depth` the callback asked for;
/// `repl` the nodes Lua left for the row.
#[derive(Clone, Default)]
pub struct RowCtl {
    pub handled: bool,
    pub depth: Option<i32>,
    pub repl: Option<Box<RowRepl>>,
}

/// What `fin_row` appended to the alignment list as Lua left it: the nodes
/// `append_to_vlist_filter` returned (for `\valign` just the row), in order.
#[derive(Clone, Default)]
pub struct RowRepl {
    pub items: Vec<RowItem>,
}

#[derive(Clone)]
pub enum RowItem {
    /// the unset row itself, which `fin_align` sets
    Row(RowBody),
    /// any other node; `fin_align` leaves it alone
    Node(Node),
}

/// The unset row as `fin_align` finds it: the packed dimensions (`w` only
/// matters for `\valign`) and the list of tabskip glue and unset cells.
#[derive(Clone)]
pub struct RowBody {
    pub w: i32,
    pub h: i32,
    pub d: i32,
    pub list: Vec<RowEntry>,
}

#[derive(Clone)]
pub enum RowEntry {
    Node(Node),
    Unset(UnsetCell),
}

/// An unset cell: the box at its natural size, the number of extra columns
/// it spans and the glue totals `fin_col` recorded (`glue_order`,
/// `glue_stretch`, `glue_sign` as shrink order, `glue_shrink`).
#[derive(Clone)]
pub struct UnsetCell {
    pub node: Node,
    pub span: u16,
    pub stretch_order: u8,
    pub stretch: i64,
    pub shrink_order: u8,
    pub shrink: i64,
}

/// span sentinel marking a one-cell row holding a \noalign box
pub const NOALIGN_SPAN: u16 = u16::MAX;

/// box_kinds marker for an open alignment cell or \noalign group
const CELL_GROUP_KIND: u8 = 8;

/// A row's collected adjustment list holds its `\vadjust` material flat and
/// its `\vadjust pre` material as `PreAdjust` nodes; return (pre, post).
fn split_pre_adjust(adj: NodeList) -> (NodeList, NodeList) {
    let mut pre = NodeList::new();
    let mut post = NodeList::with_capacity(adj.len());
    for n in adj {
        match n {
            Node::PreAdjust(items, _) => pre.extend(items),
            other => post.push(other),
        }
    }
    (pre, post)
}

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
    t0: Glue,
    everycr_done: bool,
    origin: Option<crate::input::SourceMark>,
    /// pending \\vadjust material of the row in progress (tex.web cur_head
    /// adjustment list) and the per-row drains already completed
    adjust: Vec<Node>,
    row_adjust: Vec<Vec<Node>>,
    is_valign: bool,
}

/// A read-only view of one alignment's state (see [`Engine::align_view`]).
pub(crate) struct AlignView<'a> {
    pub(crate) is_valign: bool,
    pub(crate) in_noalign: bool,
    pub(crate) preamble: &'a [ColSpec],
    pub(crate) loop_start: Option<usize>,
    pub(crate) t0: Glue,
    pub(crate) rows: &'a [Vec<Cell>],
    pub(crate) row_adjust: &'a [Vec<Node>],
    pub(crate) cur_row: &'a [Cell],
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
                crate::input::Source::MacroFrame(frame) => sum + frame.delivered_brace_balance(),
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
        let show_line = self.nest_line();
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
                t0: self.align_t0.clone(),
                origin: self.align_origin.take(),
                is_valign: self.align_is_valign,
            };
            self.align_stack.push(save);
        }
        self.align_origin = origin;
        self.align_is_valign = false;
        // the preamble may read pushback that predates \halign (`\expandafter\halign\expandafter{...`)
        self.align_pushed_base = 0;
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
        // tex.web scan_spec(align_group,false) opens the group before the
        // preamble is scanned, so a preamble `\tabskip` is local to it.
        self.push_align_group();
        if !self.scan_align_preamble() {
            let _ = self.pop_group();
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
        self.push_align_row_group();
        self.saved_lists.push((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
            self.nest_line(),
        ));
        self.box_targets.push(None);
        self.box_shifts.push(0);
        self.box_kinds.push(7);
        self.mode = Mode::InternalVertical;
        self.prev_graf = 0;
        self.show.aligns.push(crate::show_state::AlignNest {
            level: self.saved_lists.len(),
            line: show_line,
            row_line: 0,
        });
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
        let show_line = self.nest_line();
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
                t0: self.align_t0.clone(),
                origin: self.align_origin.take(),
                is_valign: self.align_is_valign,
            };
            self.align_stack.push(save);
        }
        self.align_origin = origin;
        self.align_is_valign = true;
        self.align_pushed_base = 0;
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
        self.push_align_group();
        if !self.scan_align_preamble() {
            let _ = self.pop_group();
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
        self.push_align_row_group();
        self.saved_lists.push((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
            self.nest_line(),
        ));
        self.box_targets.push(None);
        self.box_shifts.push(0);
        self.box_kinds.push(7);
        self.mode = Mode::RestrictedHorizontal;
        self.space_factor = 1000;
        self.prev_graf = 0;
        self.prev_depth = self.ignore_depth();
        self.show.aligns.push(crate::show_state::AlignNest {
            level: self.saved_lists.len(),
            line: show_line,
            row_line: 0,
        });
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
            self.error("Missing { inserted");
            if !(t.is_char() && t.cc() == 2) {
                // a stray } must not close the (not yet started) group
                self.pushed.push(t);
            }
        }
        let mut entries: Vec<ColSpec> = Vec::new();
        let mut cur = ColSpec::default();
        let mut in_u = true;
        let mut depth = 0i32;
        let mut loop_seen = false;
        // tex.web §777: scanner_status:=aligning, warning_index:=\halign.
        let owner = self
            .cs
            .lookup(if self.align_is_valign { b"valign" } else { b"halign" });
        let saved_outer_scan = self
            .outer_scan
            .replace((crate::expand::OuterScan::Preamble, owner));
        loop {
            // tex.web scan_template uses get_token (NON-expanding): u/v part
            // tokens are stored literally and expanded at cell time. Expanding
            // here runs NFSS (\\footnotesize -> \\@setfontsize -> \\let
            // \\@currsize) during the scan; the \\let never executes and the
            // size machinery loops. Resolve CharTok/prims only to RECOGNIZE
            // structural tokens (array's \\@sharp is \\let to #); store the
            // original token.
            let t = self.raw_token_outer();
            if t == crate::input::EOF_MARKER {
                // tex.web §336: the inserted `\cr}` ends the preamble and the
                // alignment; the end of the input is read again afterwards.
                let origin = self.align_origin.clone();
                self.outer_scan_file_ended(origin.as_ref());
                self.push_token(Token::char(2, b'}' as u32));
                if in_u {
                    self.error("Missing # inserted in alignment preamble");
                }
                break;
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
                Some(Prim::Cr) | Some(Prim::CrCr) => {
                    if in_u {
                        self.error("Missing # inserted in alignment preamble");
                    }
                    break;
                }
                Some(Prim::Span) => {
                    // tex.web §782: in the preamble, `\span` causes the
                    // following token to be expanded, once. It does NOT mean
                    // multicolumn (that is only valid in row cells).
                    let next = self.raw_token_outer();
                    if !self.expand_token_once(next) {
                        self.pushed.push(next);
                    }
                    continue;
                }
                Some(Prim::Omit) => {
                    self.error("\\omit is not allowed in an alignment preamble");
                    continue;
                }
                Some(Prim::GlueP(crate::prim::GlueParam::TabSkip)) => {
                    self.scan_optional_equals();
                    let g = self.scan_glue(false);
                    let global =
                        self.eqtb.int_params[crate::prim::IntParam::GlobalDefs.idx() as usize] > 0;
                    self.eqtb.assign_preamble_tabskip(g, global);
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
                            self.error("Only one # is allowed per tab");
                        }
                        continue;
                    }
                    4 if depth == 0 => {
                        let all_spaces =
                            cur.u_part.iter().all(|tok| tok.is_char() && tok.cc() == 10);
                        if in_u
                            && (cur.u_part.is_empty() || all_spaces)
                            && cur.v_part.is_empty()
                            && !loop_seen
                        {
                            // tex.web §782: an empty template between two &
                            // marks the start of the periodic preamble. This
                            // may follow already completed columns (`#&&...`).
                            loop_seen = true;
                            self.align_loop_start = Some(entries.len());
                            cur.u_part.clear();
                        } else {
                            if in_u {
                                self.error("Missing # inserted in alignment preamble");
                            }
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
                // tex.web §783: a spacer (implicit too) at the start of a
                // u-template is dropped
                if cur.u_part.is_empty() && eff.is_char() && eff.cc() == 10 {
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
        self.outer_scan = saved_outer_scan;

        true
    }

    // ------------------------------------------------------------------
    // cells
    // ------------------------------------------------------------------

    /// The alignment's own group (tex.web scan_spec(align_group,false)):
    /// `\halign to <dimen>` shows its specification in `\showgroups`.
    fn push_align_group(&mut self) {
        let (spec, exactly) = match self.align_to {
            Some((d, spread)) => (d, !spread),
            None => (0, true),
        };
        self.push_group_level_coded(
            LevelType::Box,
            crate::eqtb::GroupMeta {
                spec,
                exactly,
                ..crate::eqtb::GroupMeta::new(crate::eqtb::group_code::ALIGN)
            },
        );
    }
    /// tex.web init_align's `new_save_level(align_group)` after the preamble:
    /// the group of the current alignment entry. fin_col replaces it by
    /// `unsave; new_save_level(align_group)` at the end of every entry, and
    /// fin_align unsaves it before the group of the whole alignment.
    fn push_align_row_group(&mut self) {
        self.push_group_level_coded(
            LevelType::Box,
            crate::eqtb::GroupMeta::new(crate::eqtb::group_code::ALIGN),
        );
    }

    /// push the nest context for a cell or \noalign group. A cell lives in
    /// the alignment's entry group (see push_align_row_group); a \noalign
    /// body opens its own no_align_group. The cell is popped by
    /// finish_cell_typeset (a stray `}` mid-cell pops it via end_box
    /// instead, degrading gracefully without corrupting the stack).
    fn align_push_cell_group(&mut self, mode: Mode, code: u8) {
        self.saved_lists.push((
            self.mode,
            std::mem::take(&mut self.cur_list),
            self.prev_depth,
            self.space_factor,
            self.prev_graf,
            self.nest_line(),
        ));
        self.prev_graf = 0;
        if code != crate::eqtb::group_code::ALIGN {
            self.push_group_level_coded(LevelType::Box, crate::eqtb::GroupMeta::new(code));
        }

        self.box_targets.push(None);
        self.box_shifts.push(0);
        self.box_kinds.push(CELL_GROUP_KIND);
        self.mode = mode;
        if mode.is_v() {
            self.prev_depth = self.ignore_depth();
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
        self.align_push_cell_group(cell_mode, crate::eqtb::group_code::ALIGN);
        if self.align_is_valign {
            self.prev_depth = self.ignore_depth();
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

    /// tex.web §1128 align_error: `Misplaced \cr`, or `\crcr`.
    fn misplaced_cr(&mut self) {
        if self.cur_prim == Some(Prim::CrCr) {
            self.error("Misplaced \\crcr");
        } else {
            self.error("Misplaced \\cr");
        }
    }

    pub fn align_cr(&mut self) {
        if self.scanner_status != ScannerStatus::Aligning {
            self.misplaced_cr();
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
            self.misplaced_cr();
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
        // tex.web fin_col §791: a \span after the last column (with no
        // periodic preamble to extend) becomes \cr, exactly like an extra &
        let col = self.align_cur_col as usize;
        let cur_span = self
            .align_cur_row
            .get(col)
            .map(|c| c.span as usize)
            .unwrap_or(0);
        if self.get_col_spec(col + 1 + cur_span).is_none() {
            self.error("Extra alignment tab has been changed to \\cr");
            self.align_push_close(AlignCloseReason::EndRow);
            return;
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
        // tex.web §791 init_col: extra_info(cur_align) := cur_cmd for the
        // NEWLY absorbed column, so its v-part is inserted at the cell end
        // unless that column (not the first one) was \omit'ted
        let is_omit =
            t.is_cs() && matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Prim(Prim::Omit)));
        if is_omit {
            self.align_state = PH_CONTENT | PH_OMIT;
            self.align_brace_depth = 0;
            return;
        }
        self.align_state = PH_U;
        // Re-queue the peeked token below the absorbed column's u-template.
        self.push_tokens_named(vec![t], PEEK_SRC);
        let u = self
            .get_col_spec(col + span)
            .map(|s| s.u_part.clone())
            .unwrap_or_default();
        if u.is_empty() {
            self.align_u_template_finished();
            self.align_state = PH_CONTENT;
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

    /// The end of an alignment entry or of a \noalign body: tex.web fin_col
    /// does `unsave; new_save_level(align_group)` for an entry, the
    /// no_align_group's `}` a plain unsave.
    fn align_pop_cell_group(&mut self) -> Option<(NodeList, i32)> {
        let noalign = self.align_in_noalign;
        let popped = self.align_pop_cell_nest(true);
        if popped.is_some() && !noalign {
            self.push_align_row_group();
        }
        popped
    }

    /// Pop the nest context of a cell or \noalign body; `unsave` also closes
    /// the save-stack group of a \noalign body or the entry group (a
    /// phantom cell, opened before a \noalign was seen, has made none).
    fn align_pop_cell_nest(&mut self, unsave: bool) -> Option<(NodeList, i32)> {
        if self.box_kinds.last() != Some(&CELL_GROUP_KIND) || self.saved_lists.is_empty() {
            return None;
        }
        let end_pd = self.prev_depth;
        let inner = std::mem::take(&mut self.cur_list);
        let _ = self.box_targets.pop().flatten();
        let _ = self.box_shifts.pop().unwrap_or(0);
        let _ = self.box_kinds.pop();
        if unsave {
            self.pop_group();
        }
        let (om, ol, pd, sf, pg, _) = self.saved_lists.pop().unwrap();
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
            let _ = self.align_pop_cell_nest(false);
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
        // tex.web no_align: the material joins the alignment's own list, in
        // internal vertical mode for \halign and restricted horizontal mode
        // for \valign
        if self.align_is_valign {
            self.align_push_cell_group(Mode::RestrictedHorizontal, crate::eqtb::group_code::NO_ALIGN);
            self.space_factor = 1000;
        } else {
            self.align_push_cell_group(Mode::InternalVertical, crate::eqtb::group_code::NO_ALIGN);
        }
        // tex.web §15514: \noalign runs in internal vertical mode inheriting the
        // preceding row's depth (or ignore_depth if at the alignment start).
        let tex_depth = self.align_rows.last().and_then(|r| {
            if r.len() == 1 && r[0].span == NOALIGN_SPAN {
                match &r[0].packed {
                    Some(Node::Box { shift, .. }) => Some(*shift),
                    _ => None,
                }
            } else {
                // fin_row's natural hpack starts from depth 0
                Some(r.iter().fold(0, |d, c| match &c.packed {
                    Some(Node::Box { d: cd, .. }) => d.max(*cd),
                    _ => d,
                }))
            }
        });
        // with Lua shaping the rows the depth comes from what it left
        self.prev_depth = self.lua_align_noalign_depth().or(tex_depth).unwrap_or(outer_pd);
        self.align_pushed_base = self.pushed.len();
        // The consumed `{` is the no_align_group itself: its `}` is routed
        // to align_finish_noalign_now by align_close_noalign_brace.
    }

    /// tex.web §1132 handle_right_brace for no_align_group: `}` ends the
    /// \noalign body when that group is the innermost one. Returns false
    /// for any other `}`.
    pub(crate) fn align_close_noalign_brace(&mut self) -> bool {
        if self.align_in_noalign
            && self.eqtb.cur_group_code() == crate::eqtb::group_code::NO_ALIGN
            && self.box_kinds.last() == Some(&CELL_GROUP_KIND)
        {
            self.align_finish_noalign_now();
            return true;
        }
        false
    }

    /// tex.web hpack @12956, 13006-13016: the natural-width pack of an
    /// alignment cell transfers every top-level \vadjust node out of its
    /// hlist onto the alignment level's adjustment list (`cur_tail`),
    /// preserving source order. The inner vlist of each adjustment joins
    /// the accumulator; the node itself contributes no width.
    fn align_collect_adjustments(&mut self, list: &mut NodeList) {
        let mut i = 0;
        while i < list.len() {
            match list[i] {
                Node::VAdjust(_, _) => match list.remove(i) {
                    Node::VAdjust(items, _) => self.align_adjust.extend(items),
                    _ => unreachable!(),
                },
                // pdftex.web cur_pre_tail: kept apart from the post material
                // (split again by `split_pre_adjust` when the row is built)
                Node::PreAdjust(_, _) => self.align_adjust.push(list.remove(i)),
                _ => i += 1,
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
        // luatex fin_col: `filtered_hpack` runs the text passes and the
        // `hpack_filter` (group `align_set`, no direction) on the cell
        if self.engine_kind == crate::engine::EngineKind::LuaTeX && !self.align_is_valign {
            let list = std::mem::take(&mut inner);
            inner = self.lua_hpack_text_filter("align_set", 0, false, None, list);
        } else if self.engine_kind == crate::engine::EngineKind::LuaTeX {
            // luatex fin_col: `filtered_vpackage(..., 0, additional, 0,
            // align_set_group, -1, ...)` runs `vpack_filter` (maximum depth 0)
            let list = std::mem::take(&mut inner);
            inner = self.lua_pack_filter(
                crate::lua_callbacks::Cb::VpackFilter,
                "vpack filter",
                "align_set",
                0,
                false,
                Some(0),
                None,
                list,
            );
        }
        self.align_collect_adjustments(&mut inner);
        let col = self.align_cur_col as usize;
        let span = self.align_cur_row.get(col).map(|c| c.span).unwrap_or(0);
        let packed = if self.align_is_valign {
            // tex.web fin_col: `vpackage(link(head),natural,0)` moves the
            // whole depth into the cell's height
            crate::boxes::vpack_add_md(inner, None, false, crate::boxes::VBOX, &self.eqtb, 0).node
        } else {
            crate::boxes::hpack(inner, None, crate::boxes::HBOX, &self.eqtb).node
        };
        while self.align_cur_row.len() <= col {
            self.align_cur_row.push(Cell::default());
        }
        self.align_cur_row[col] = Cell {
            packed: Some(packed),
            span,
            ctl: RowCtl::default(),
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

    /// capture the open \noalign text as a raw list (carried in a Box node
    /// whose `shift` holds the prev_depth at the group's end); tex.web
    /// splices noalign material into the alignment's list as-is, and
    /// finish_halign extends running rules to the alignment's size.
    pub(crate) fn align_finish_noalign_now(&mut self) {
        // tex.web §21663 (no_align_group handle_right_brace): `end_graf; unsave; align_peek;`
        // Closing \noalign while a paragraph is running forces \par first,
        // breaking the paragraph into lines and restoring mode to InternalVertical.
        if self.mode == Mode::Horizontal {
            let saved = std::mem::replace(&mut self.lua_par_group, 7);
            self.par_primitive(Token::from_cs(self.ids.par));
            self.lua_par_group = saved;
        }
        let Some((inner, end_pd)) = self.align_pop_cell_group() else {
            return;
        };
        self.align_in_noalign = false;
        self.align_state = PH_IDLE;
        // an empty \noalign still matters when it reset \prevdepth
        // (\noalign{\nointerlineskip})
        if !inner.is_empty() || (!self.align_is_valign && end_pd <= self.ignore_depth()) {
            let node = Node::Box {
                kind: crate::boxes::VBOX,
                w: 0,
                h: 0,
                d: 0,
                shift: end_pd,
                list: inner,
                glue_sign: 0,
                glue_order: 0,
                glue_set: 0.0,
                lr: 0,
                dir: 0, attr: self.eqtb.cur_attr, subtype: 0,
            };
            self.align_rows.push(vec![Cell {
                packed: Some(node),
                span: NOALIGN_SPAN,
                ctl: RowCtl::default(),
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

    /// The tabskip glue that precedes the first column.
    pub(crate) fn align_col_tabskip_start(&self) -> Glue {
        self.align_t0.param(crate::boxes::glue_subtype::TAB_SKIP)
    }

    /// The tabskip glue following grid column `col` (tex.web
    /// `new_param_glue(tab_skip_code)`; the preamble's loop supplies the
    /// glue for columns beyond it).
    pub(crate) fn align_col_tabskip(&self, col: usize) -> Glue {
        let t0 = self.align_col_tabskip_start();
        let pre = &self.align_preamble;
        if pre.is_empty() {
            return t0;
        }
        let g = if col < pre.len() {
            pre[col].tabskip
        } else {
            match self.align_loop_start {
                Some(ls) if pre.len() > ls => pre[ls + (col - ls) % (pre.len() - ls)].tabskip,
                _ => return t0,
            }
        };
        g.param(crate::boxes::glue_subtype::TAB_SKIP)
    }

    /// pack up the finished row and inspect what follows it
    fn align_finish_row(&mut self) {
        let mut row = std::mem::take(&mut self.align_cur_row);
        if !row.is_empty() {
            // luatex fin_row: the text passes, `hpack_filter` and
            // `append_to_vlist_filter` see the finished unset row
            row[0].ctl = self.lua_fin_row(&row);
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
        let line = self.nest_line();
        if let Some(a) = self.show.aligns.last_mut() {
            a.row_line = line;
        }
        self.align_start_cell(first);
    }
    // ------------------------------------------------------------------
    // final packaging (build.rs end_box, kind 7)
    // ------------------------------------------------------------------

    /// tex.web fin_align (§800-§812) for \halign and \valign: compute the
    /// column widths (§801-§803, including span merging and nullified empty
    /// columns), pack the preamble to the requested size, then set every
    /// row and cell from the preamble's glue (§804-§810). Rows of a \valign
    /// are the transposed case: "width" is measured vertically.
    pub fn finish_halign(&mut self) {
        // tex.web fin_align unsaves twice: end_box has closed the group of
        // the last entry; this is the group of the whole alignment, which
        // also restores preamble assignments such as \tabskip.
        let _ = self.pop_group();
        let valign = self.align_is_valign;
        let rows_in = std::mem::take(&mut self.align_rows);
        let row_adj = std::mem::take(&mut self.align_row_adjust);
        let max_row_cols = rows_in.iter().map(|r| r.len()).max().unwrap_or(0);
        let ncols = self.align_preamble.len().max(max_row_cols);
        let t0 = self.align_t0.param(crate::boxes::glue_subtype::TAB_SKIP);
        // tabskip glue following each column (tex.web new_param_glue(tab_skip_code))
        let mut tabs: Vec<Glue> = (0..ncols).map(|col| self.align_col_tabskip(col)).collect();
        let size = |n: &Node| match n {
            Node::Box { w, h, .. } => {
                if valign {
                    *h
                } else {
                    *w
                }
            }
            _ => 0,
        };

        // §801: width(q) starts at null_flag; every single-column entry and
        // every span ending in q raises it. Span requirements ending in
        // column j are merged once all earlier widths are final.
        let mut widths: Vec<Option<i32>> = vec![None; ncols];
        let mut spans: Vec<(usize, usize, i32)> = Vec::new();
        for row in &rows_in {
            if row.len() == 1 && row[0].span == NOALIGN_SPAN {
                continue;
            }
            for (c, cell) in row.iter().enumerate() {
                let Some(packed) = &cell.packed else { continue };
                let w = size(packed);
                if cell.span == 0 {
                    widths[c] = Some(widths[c].map_or(w, |x| x.max(w)));
                } else {
                    let end = (c + cell.span as usize).min(ncols - 1);
                    spans.push((c, end, w));
                }
            }
        }
        for j in 0..ncols {
            for &(c, end, w) in &spans {
                if end != j {
                    continue;
                }
                let pre: i64 = (c..j)
                    .map(|k| widths[k].unwrap_or(0) as i64 + tabs[k].width as i64)
                    .sum();
                let need = (w as i64 - pre) as i32;
                widths[j] = Some(widths[j].map_or(need, |x| x.max(need)));
            }
            if widths[j].is_none() {
                // §802: nullify width(q) and the tabskip glue following it
                widths[j] = Some(0);
                tabs[j] = Glue::zero();
            }
        }
        let widths: Vec<i32> = widths.into_iter().map(|w| w.unwrap_or(0)).collect();
        self.align_col_widths.clone_from(&widths);

        // §804: package the preamble (unset columns of the final widths
        // separated by tabskip glue) to find the alignment's glue setting;
        // \overfullrule is suppressed for this pack.
        let cur_attr = self.eqtb.cur_attr;
        let column = |w: i32| Node::Box {
            kind: if valign { crate::boxes::VBOX } else { crate::boxes::HBOX },
            w: if valign { 0 } else { w },
            h: if valign { w } else { 0 },
            d: 0,
            shift: 0,
            list: Vec::new(),
            glue_sign: 0,
            glue_order: 0,
            glue_set: 0.0,
            lr: 0,
            dir: 0,
            attr: cur_attr, subtype: 0,
        };
        let mut preamble: NodeList = Vec::with_capacity(2 * ncols + 1);
        preamble.push(Node::Glue(t0, self.eqtb.cur_attr));
        for j in 0..ncols {
            preamble.push(column(widths[j]));
            preamble.push(Node::Glue(tabs[j], self.eqtb.cur_attr));
        }
        let (dim, spread) = match self.align_to {
            Some((d, sp)) => (Some(d), sp),
            None => (None, false),
        };
        // luatex fin_align: the preamble of a `\valign` goes through
        // `filtered_vpackage(..., preamble_group, ...)`
        if valign && self.engine_kind == crate::engine::EngineKind::LuaTeX {
            preamble = self.lua_preamble_filter(preamble, dim.unwrap_or(0), dim.is_some() && !spread);
        }
        let preamble_len = preamble.len();
        let mut res = if valign {
            crate::boxes::vpack_add_md(
                preamble,
                dim,
                spread,
                crate::boxes::VBOX,
                &self.eqtb,
                i32::MAX,
            )
        } else {
            crate::boxes::hpack_add(preamble, dim, spread, crate::boxes::HBOX, &self.eqtb)
        };
        if let Node::Box { list, .. } = &mut res.node {
            list.truncate(preamble_len);
        }
        self.last_badness = res.badness;
        let origin = self.align_origin.clone();
        // tex.web §804: `pack_begin_line:=-mode_line` while the preamble
        // is packaged ("in alignment at lines a--b")
        let align_line = origin.as_ref().map_or(0, |mark| mark.to_context().line as i32);
        let saved_begin = std::mem::replace(&mut self.pack_begin_line, -align_line);
        self.report_pack_warnings_at(&mut res, origin);
        self.pack_begin_line = saved_begin;
        let (p_size, p_sign, p_order, p_set) = match &res.node {
            Node::Box {
                w,
                h,
                glue_sign,
                glue_order,
                glue_set,
                ..
            } => (if valign { *h } else { *w }, *glue_sign, *glue_order, *glue_set),
            _ => (0, 0, 0, 0.0),
        };
        // the amount a tabskip glue contributes under the preamble setting
        let tab_amount = |g: &Glue| -> i64 {
            let mut t = g.width as i64;
            if p_sign == 1 && g.stretch_order == p_order {
                t += (p_set * g.stretch as f64).round() as i64;
            } else if p_sign == 2 && g.shrink_order == p_order {
                t -= (p_set * g.shrink as f64).round() as i64;
            }
            t
        };

        let mut rows: NodeList = Vec::new();
        let bs = self.eqtb.glue_params[GlueParam::BaselineSkip.idx() as usize];
        let ls = self.eqtb.glue_params[GlueParam::LineSkip.idx() as usize];
        let lsl = self.eqtb.dim_params[DimParam::LineSkipLimit.idx() as usize];
        let to_setbox = self.setbox_target.is_some() && self.setbox_depth == self.box_kinds.len();
        // tex.web §800: `if nest[nest_ptr-1].mode_field=mmode then
        // o:=display_indent`; only the rows and the top-level \noalign rules
        // are shifted (§810, §806), other \noalign material stays put.
        // etex.ch §800 also marks those rows `set_box_lr(q)(dlist)` for
        // ship_out.
        let display = !valign && !to_setbox && self.mode == Mode::DisplayMath;
        let o = if display {
            self.eqtb.dim_params[DimParam::DisplayIndent.idx() as usize]
        } else {
            0
        };
        // tex.web init_align: push_nest keeps the enclosing aux, so in a
        // vertical list the first row's interline glue is computed against
        // the enclosing prev_depth (a display uses the depth of the list
        // around the paragraph, already in self.prev_depth here).
        let mut prev: Option<i32> = if !valign
            && !to_setbox
            && matches!(
                self.mode,
                Mode::Vertical | Mode::InternalVertical | Mode::DisplayMath
            )
            && self.prev_depth > self.ignore_depth()
        {
            Some(self.prev_depth)
        } else {
            None
        };
        // luatex: with `append_to_vlist_filter` registered the callback, not
        // `append_to_vlist`, decides about glue and `prev_depth`
        let any_handled = rows_in.iter().any(|r| r.first().is_some_and(|c| c.ctl.handled));
        let mut cb_depth: Option<i32> = None;
        // luatex `fin_align` divides integers when it sets the glue of a
        // `\valign` cell (the `\halign` branch casts to double)
        let int_div = valign && self.engine_kind == crate::engine::EngineKind::LuaTeX;
        let cur_attr = self.eqtb.cur_attr;
        // §806: the running dimensions of a top-level rule extend to the
        // alignment's boundaries; in a display it is boxed and shifted like
        // a row
        let fix_rule = |mut n: Node| -> Node {
            if let Node::Rule { width, height, depth, .. } = &mut n {
                if valign {
                    if *height == crate::build::RULE_FILL {
                        *height = p_size;
                    }
                    if *depth == crate::build::RULE_FILL {
                        *depth = 0;
                    }
                } else if *width == crate::build::RULE_FILL {
                    *width = p_size;
                }
                if o != 0 {
                    let mut b = crate::boxes::hpack(vec![n], None, crate::boxes::HBOX, &self.eqtb).node;
                    if let Node::Box { shift, .. } = &mut b {
                        *shift = o;
                    }
                    return b;
                }
            }
            n
        };
        // tex.web §808-810: set the cell `cell_box` of grid column `c`, which
        // spans `span` more columns, to its final width; the nodes standing
        // in for the covered columns go to `covered`. Returns the last
        // column of the cell.
        let set_cell = |cell_box: &mut Node,
                        c: usize,
                        span: u16,
                        totals: Option<(usize, i64, usize, i64)>,
                        row_a: i32,
                        row_b: i32,
                        covered: &mut NodeList|
         -> usize {
            let end = (c + span as usize).min(ncols - 1);
            let w = widths[c];
            let mut t = w as i64;
            // §809: tabskip glue and an empty box for every covered column
            for k in c + 1..=end {
                let g = tabs[k - 1];
                t += tab_amount(&g) + widths[k] as i64;
                covered.push(Node::Glue(g, cur_attr));
                let mut empty = column(widths[k]);
                if let Node::Box { subtype, .. } = &mut empty {
                    *subtype = crate::boxes::list_subtype::CELL;
                }
                covered.push(empty);
            }
            set_unset_cell(cell_box, w, t, valign, row_a, row_b, totals, int_div);
            end
        };
        let mut bad_box = false;
        for (mut row, adj) in rows_in.into_iter().zip(row_adj) {
            if row.len() == 1 && row[0].span == NOALIGN_SPAN {
                if let Some(Node::Box { list, shift: end_pd, .. }) =
                    row.into_iter().next().and_then(|c| c.packed)
                {
                    if !valign {
                        prev = (end_pd > self.ignore_depth()).then_some(end_pd);
                        if any_handled {
                            cb_depth = Some(end_pd);
                        }
                    }
                    // §811
                    rows.extend(list.into_iter().map(&fix_rule));
                }
                continue;
            }
            let mut ctl = row.first_mut().map_or_else(RowCtl::default, |c| std::mem::take(&mut c.ctl));
            // the unset row's other dimensions: fin_row's natural pack
            let (row_a, row_b) = match &ctl.repl {
                Some(repl) => repl
                    .items
                    .iter()
                    .find_map(|item| match item {
                        RowItem::Row(body) if valign => Some((body.w, body.d)),
                        RowItem::Row(body) => Some((body.h, body.d)),
                        RowItem::Node(_) => None,
                    })
                    .unwrap_or((0, 0)),
                None => row.iter().fold((0, 0), |(a, b), c| match &c.packed {
                    Some(Node::Box { w, h, d, .. }) => {
                        if valign {
                            (a.max(*w), 0)
                        } else {
                            (a.max(*h), b.max(*d))
                        }
                    }
                    _ => (a, b),
                }),
            };
            let row_nodes: NodeList = if let Some(repl) = ctl.repl.take() {
                // Lua's row: tabskip glue and the unset cells it left
                let mut nodes = NodeList::with_capacity(repl.items.len());
                for item in repl.items {
                    match item {
                        RowItem::Node(n) => nodes.push(fix_rule(n)),
                        RowItem::Row(body) => {
                            let mut line: NodeList = Vec::with_capacity(body.list.len() + 2 * ncols);
                            let mut entries = body.list.into_iter().peekable();
                            // the node in front of the first cell stays; the
                            // first cell has to be unset (fin_align "bad box")
                            if let Some(first) = entries.next() {
                                line.push(match first {
                                    RowEntry::Node(n) => n,
                                    RowEntry::Unset(u) => u.node,
                                });
                            }
                            bad_box |= !entries.peek().is_some_and(|e| matches!(e, RowEntry::Unset(_)));
                            let mut col = 0;
                            while let Some(entry) = entries.next() {
                                match entry {
                                    RowEntry::Unset(mut u) if col < ncols => {
                                        let mut covered = NodeList::new();
                                        let totals = (
                                            usize::from(u.stretch_order),
                                            u.stretch,
                                            usize::from(u.shrink_order),
                                            u.shrink,
                                        );
                                        let end =
                                            set_cell(&mut u.node, col, u.span, Some(totals), row_a, row_b, &mut covered);
                                        line.push(u.node);
                                        line.extend(covered);
                                        // the node after the cell is its tabskip glue
                                        if let Some(glue) = entries.next() {
                                            line.push(match glue {
                                                RowEntry::Node(n) => n,
                                                RowEntry::Unset(u) => u.node,
                                            });
                                        }
                                        col = end + 1;
                                    }
                                    RowEntry::Unset(u) => line.push(u.node),
                                    RowEntry::Node(n) => line.push(n),
                                }
                            }
                            nodes.push(Node::Box {
                                kind: if valign { crate::boxes::VBOX } else { crate::boxes::HBOX },
                                w: if valign { row_a } else { p_size },
                                h: if valign { p_size } else { row_a },
                                d: row_b,
                                shift: o,
                                list: line,
                                glue_sign: p_sign,
                                glue_order: p_order,
                                glue_set: p_set,
                                lr: if display { crate::boxes::BOX_LR_DLIST } else { 0 },
                                dir: 0,
                                attr: cur_attr,
                                subtype: crate::boxes::list_subtype::ALIGNMENT,
                            });
                        }
                    }
                }
                nodes
            } else {
                let mut line: NodeList = Vec::with_capacity(2 * row.len() + 1);
                line.push(Node::Glue(t0, cur_attr));
                for (c, cell) in row.into_iter().enumerate() {
                    // grid slots covered by an earlier spanning cell
                    let Some(mut cell_box) = cell.packed else { continue };
                    let mut covered = NodeList::new();
                    let end = set_cell(&mut cell_box, c, cell.span, None, row_a, row_b, &mut covered);
                    line.push(cell_box);
                    line.extend(covered);
                    line.push(Node::Glue(tabs[end], cur_attr));
                }
                vec![Node::Box {
                    kind: if valign { crate::boxes::VBOX } else { crate::boxes::HBOX },
                    w: if valign { row_a } else { p_size },
                    h: if valign { p_size } else { row_a },
                    d: if valign { 0 } else { row_b },
                    shift: o,
                    list: line,
                    glue_sign: p_sign,
                    glue_order: p_order,
                    glue_set: p_set,
                    lr: if display { crate::boxes::BOX_LR_DLIST } else { 0 },
                    dir: 0,
                    attr: cur_attr,
                    subtype: crate::boxes::list_subtype::ALIGNMENT,
                }]
            };
            // pdftex.web fin_row: the row's `\vadjust pre` material joins
            // the vertical list in front of the row (and its interline glue)
            let (pre_adj, adj) = split_pre_adjust(adj);
            rows.extend(pre_adj);
            if ctl.handled {
                if ctl.depth.is_some() {
                    cb_depth = ctl.depth;
                }
            } else if !valign {
                // tex.web append_to_vlist at fin_row time
                let (lead, trail) = self.interline_extents(row_a, row_b);
                if let Some(pd) = prev {
                    let gap = bs.width as i64 - pd as i64 - lead as i64;
                    rows.push(Node::Glue(if gap < lsl as i64 {
                        ls.param(crate::boxes::glue_subtype::LINE_SKIP)
                    } else {
                        Glue {
                            width: gap as i32,
                            subtype: crate::boxes::glue_subtype::BASELINE_SKIP,
                            ..bs.fresh()
                        }
                    }, cur_attr));
                }
                prev = Some(trail);
            }
            rows.extend(row_nodes);
            // tex.web fin_row §15724-5: the row's migrated \vadjust
            // material follows the row box raw (no interline glue before
            // it; prev_depth keeps the row box's depth).
            rows.extend(adj);
        }

        if bad_box {
            self.fatal_error("error:  (alignment): bad box");
        }
        self.align_preamble.clear();
        self.align_cur_row.clear();
        self.align_adjust.clear();
        self.align_done = false;
        self.align_in_noalign = false;
        self.align_everycr_done = false;
        let nested = self.align_has_save();
        self.show.aligns.pop();
        self.align_nested_restore();
        if !nested {
            self.scanner_status = ScannerStatus::Normal;
            self.align_to = None;
            self.align_brace_depth = 0;
            self.align_is_valign = false;
        }
        if to_setbox {
            let packed = if valign {
                crate::boxes::hpack(rows, None, crate::boxes::HBOX, &self.eqtb).node
            } else {
                crate::boxes::vpack(rows, None, crate::boxes::VBOX, &self.eqtb).node
            };
            let idx = self.setbox_target.take().unwrap();
            let g = self.setbox_global;
            self.unpark_setbox();
            self.eqtb.assign_box(idx, Some(packed), g);
            return;
        }
        if valign {
            // fin_row: each \valign row joins the horizontal list directly
            if self.mode.is_h() {
                self.cur_list.extend(rows);
                self.space_factor = 1000;
            } else {
                let hbox = crate::boxes::hpack(rows, None, crate::boxes::HBOX, &self.eqtb).node;
                self.append_box_node(Some(hbox));
            }
            return;
        }
        match self.mode {
            Mode::Vertical => {
                // tex.web fin_align: the rows join the contribution list
                // individually and the page builder runs
                if any_handled {
                    if let Some(d) = cb_depth {
                        self.prev_depth = d;
                    }
                } else if let Some(d) = prev {
                    self.prev_depth = d;
                } else if !rows.is_empty() {
                    self.prev_depth = self.ignore_depth();
                }
                self.page_list.extend(rows);
                self.lua_page_filter(crate::lua_callbacks::page_info::ALIGNMENT, true);
                self.build_page();
            }
            Mode::InternalVertical => {
                if any_handled {
                    if let Some(d) = cb_depth {
                        self.prev_depth = d;
                    }
                } else if let Some(d) = prev {
                    self.prev_depth = d;
                } else if !rows.is_empty() {
                    self.prev_depth = self.ignore_depth();
                }
                self.cur_list.extend(rows);
            }
            Mode::DisplayMath => {
                // tex.web §16078: the alignment rows (already carrying their
                // interline glue, including the leading glue seeded from the
                // outer vlist's prev_depth) plus \noalign material join the
                // display vlist raw. prev_depth := the align level's final
                // prev_depth (last row's depth), tracked by `prev`.
                let final_pd = if any_handled { cb_depth } else { prev }.unwrap_or(self.prev_depth);
                self.prev_depth = final_pd;
                self.display_halign = Some((rows, final_pd));
                if self.engine_kind == crate::engine::EngineKind::LuaTeX {
                    self.finish_display_alignment();
                }
            }
            _ => {
                let vbox = crate::boxes::vpack(rows, None, crate::boxes::VBOX, &self.eqtb).node;
                self.append_box_node(Some(vbox));
            }
        }
    }

    /// What `\showlists` needs to rebuild tex.web's alignment, row and cell
    /// levels: the alignment `depth` levels below the innermost one.
    pub(crate) fn align_view(&self, depth: usize) -> Option<AlignView<'_>> {
        if depth == 0 {
            return Some(AlignView {
                is_valign: self.align_is_valign,
                in_noalign: self.align_in_noalign,
                preamble: &self.align_preamble,
                loop_start: self.align_loop_start,
                t0: self.align_t0,
                rows: &self.align_rows,
                row_adjust: &self.align_row_adjust,
                cur_row: &self.align_cur_row,
            });
        }
        let sv = self
            .align_stack
            .len()
            .checked_sub(depth)
            .and_then(|i| self.align_stack.get(i))?;
        Some(AlignView {
            is_valign: sv.is_valign,
            in_noalign: sv.in_noalign,
            preamble: &sv.preamble,
            loop_start: sv.loop_start,
            t0: sv.t0,
            rows: &sv.rows,
            row_adjust: &sv.row_adjust,
            cur_row: &sv.cur_row,
        })
    }
}

/// tex.web §810: turn a natural-size cell into its final box: size `w`
/// along the alignment axis, glue set as if that size were `t` (the cell
/// plus any spanned columns), other dimensions taken from the row.
fn set_unset_cell(
    cell: &mut Node,
    w: i32,
    t: i64,
    valign: bool,
    row_a: i32,
    row_b: i32,
    totals: Option<(usize, i64, usize, i64)>,
    int_div: bool,
) {
    let Node::Box {
        w: bw,
        h: bh,
        d: bd,
        list,
        glue_sign,
        glue_order,
        glue_set,
        subtype,
        ..
    } = cell
    else {
        return;
    };
    *subtype = crate::boxes::list_subtype::CELL;
    let nat = if valign { *bh } else { *bw } as i64;
    // the order and total of the cell's stretch and shrink: what `fin_col`
    // recorded in the unset node (Lua may have changed them)
    let (stretch_order, stretch, shrink_order, shrink) = match totals {
        Some(totals) => totals,
        None => {
            let (stretch, shrink) = crate::boxes::glue_sums(list);
            let (so, ho) = (crate::boxes::highest_glue_order(&stretch), crate::boxes::highest_glue_order(&shrink));
            (so, stretch[so], ho, shrink[ho])
        }
    };
    if t == nat {
        (*glue_sign, *glue_order, *glue_set) = (0, 0, 0.0);
    } else if t > nat {
        *glue_sign = 1;
        *glue_order = stretch_order as u8;
        *glue_set = if stretch == 0 {
            0.0
        } else if int_div {
            ((t - nat) / stretch) as f64
        } else {
            (t - nat) as f64 / stretch as f64
        };
    } else {
        *glue_sign = 2;
        *glue_order = shrink_order as u8;
        *glue_set = if shrink == 0 {
            0.0
        } else if shrink_order == 0 && nat - t > shrink {
            1.0
        } else if int_div {
            ((nat - t) / shrink) as f64
        } else {
            (nat - t) as f64 / shrink as f64
        };
    }
    if valign {
        *bw = row_a;
        *bh = w;
    } else {
        *bh = row_a;
        *bd = row_b;
        *bw = w;
    }
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

    /// (alignment width, alignment list): the enclosing \vbox's list, or —
    /// for an outer-vertical \halign, whose rows go straight to the page —
    /// the page list after the page builder's \topskip glue
    fn vbox_of(e: &Engine) -> (i32, &[Node]) {
        let start = e
            .page_list
            .iter()
            .position(|n| !matches!(n, Node::Glue(_, _)))
            .expect("alignment material on page list");
        if let Node::Box { kind, w, list, .. } = &e.page_list[start] {
            if *kind == crate::boxes::VBOX {
                return (*w, list);
            }
        }
        let w = e.page_list[start..]
            .iter()
            .find_map(|n| match n {
                Node::Box { kind, w, .. } if *kind == crate::boxes::HBOX => Some(*w),
                _ => None,
            })
            .expect("alignment rows on page list");
        (w, &e.page_list[start..])
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
        // tex.web §809: the spanned cell is a box of its first column's
        // width, followed by tabskip glue and an empty box per covered column
        assert_eq!(r.len(), 7, "T_0, cell, tabskip, span cell, tabskip, empty, T_n: {:?}", r);
        assert_eq!(box_w(&r[3]), w1);
        assert_eq!(box_w(&r[5]), w2);
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
    fn tabular_style_halign_in_vbox() {
        // LaTeX tabular puts its \halign in a \vbox (an \halign directly in
        // restricted horizontal mode is an error in TeX): the rows land in
        // the surrounding box
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\vbox{\\halign{#\\hfil& #\\hfil\\cr a&bb\\cr ccc&d\\cr}}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        let (w, list) = vbox_of(&e);
        let (w0, w1) = (e.align_col_widths[0], e.align_col_widths[1]);
        assert!(w0 > 0 && w1 > 0);
        assert_eq!(w, w0 + w1);
        let rows = list
            .iter()
            .filter(|n| matches!(n, Node::Box { .. }))
            .count();
        assert_eq!(rows, 2, "rows: {:?}", list);
    }

    #[test]
    fn halign_in_restricted_horizontal_mode_closes_the_box() {
        // tex.web head_for_vmode: \halign cannot start in an \hbox, so the
        // box is closed first ("Missing } inserted") and the alignment
        // follows it in the enclosing vertical mode
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\hbox{a\\halign{#\\cr b\\cr}}\n",
        ));
        let messages: Vec<&str> = e.diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(messages, ["Missing } inserted"], "{}", e.term);
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
        // a LaTeX-tabular-shaped alignment inside \vbox: \toprule-like
        // \noalign rule before the first row, a \multicolumn header
        // (\span\omit), body rows, \midrule, and a \bottomrule followed by
        // \crcr before the closing brace (like \endtabular)
        let e = run(concat!(
            "\\font\\cmr=cmr10 \\cmr\n",
            "\\def\\br{\\noalign{\\hrule}}\n",
            "\\vbox{\\halign{\\hfil#& \\hfil#\\hfil& #\\hfil\\cr\n",
            "\\br\n",
            "\\span\\omit \\hfil Header\\hfil\\cr\n",
            "\\br\n",
            "a&bb&ccc\\cr\n",
            "d&e&f\\cr\n",
            "\\br\n",
            "\\crcr}}\n",
        ));
        assert_eq!(e.error_count, 0, "errors:\n{}", e.term);
        // the rows and rules sit directly in the vbox
        let (_, align_box) = vbox_of(&e);
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
            "\\font\\tsy=cmsy10 \\font\\tex=cmex10 \\textfont2=\\tsy \\scriptfont2=\\tsy \\scriptscriptfont2=\\tsy ",
            "\\textfont3=\\tex \\scriptfont3=\\tex \\scriptscriptfont3=\\tex\n",
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
            Node::Glue(g, _) => g.width,
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
        assert!(matches!(l[2], Node::Glue(_, _)), "interline glue after it");
        assert_eq!(
            glue_w(&l[2]),
            glue_w(&p[1]),
            "the splice must not disturb interline glue (prev_depth keeps the row depth)"
        );
        assert!(matches!(l[3], Node::Box { .. }), "row 2 box");
        // row boxes must not contain the adjustment node
        for r in [0usize, 3] {
            assert!(
                row_of(&l[r]).iter().all(|n| !matches!(n, Node::VAdjust(_, _))),
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
        assert!(matches!(l[2], Node::Glue(_, _)));
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
            "\\font\\tsy=cmsy10 \\font\\tex=cmex10 \\textfont2=\\tsy \\scriptfont2=\\tsy \\scriptscriptfont2=\\tsy ",
            "\\textfont3=\\tex \\scriptfont3=\\tex \\scriptscriptfont3=\\tex\n",
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
        // tex.web fin_align: each \valign row joins the enclosing hlist
        let vboxes: Vec<_> = list
            .iter()
            .filter(|n| matches!(n, Node::Box { kind, .. } if *kind == crate::boxes::VBOX))
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

    /// Box dimensions measured with TeX Live 2026 pdflatex for the same
    /// input: fin_align's span merging, nullified empty columns, the
    /// \valign transpose, interline glue against the enclosing list and a
    /// trailing \span that becomes \cr.
    #[test]
    fn alignment_dimensions_match_tex_live() {
        let cases: &[(&str, &str, &str, &str)] = &[
            (
                r"\vbox{\hbox{x}\halign{#\cr a\cr g\cr}\hbox{y}}",
                "5.2778pt",
                "40.30554pt",
                "1.94444pt",
            ),
            (r"\vbox to 60pt{\halign{#\cr a\cr}\vfil\hbox{y}}", "5.2778pt", "60.0pt", "1.94444pt"),
            (r"\vbox{\tabskip=5pt\halign{#&#&#&#\cr a&b\span c&d\cr}}", "40.5556pt", "6.94444pt", "0pt"),
            (r"\vbox{\tabskip=5pt\halign{#&#&#\cr a\cr}}", "15.00002pt", "4.30554pt", "0pt"),
            (
                r"\vbox{\tabskip=1pt\halign{#&#&#\cr aaaa\cr x\span y\span\cr a&\omit\span\omit wwwwwwwwww\cr}}",
                "95.22235pt",
                "28.30554pt",
                "0pt",
            ),
            (
                r"\hbox{\valign{#\vfil\tabskip=2pt&\hbox{#}\cr \hbox{a}&b\cr \noalign{\kern 3pt}\hbox{c}\hbox{d}&e\cr}}",
                "14.11115pt",
                "27.24998pt",
                "0pt",
            ),
            (
                r"\hbox{x\valign to 40pt{\hbox{#}\vfil\tabskip 0pt plus 1fil&\hbox{#}\cr a&b\cr c\span d\cr g\cr}y}",
                "26.66676pt",
                "40.0pt",
                "1.94444pt",
            ),
        ];
        for (body, wd, ht, dp) in cases {
            let e = run(&format!(
                "\\font\\cmr=cmr10 \\cmr \\lineskip=1pt \\lineskiplimit=0pt \\boxmaxdepth=16383.99999pt \\setbox1={body}\
                 \\ifdim\\wd1={wd}\\else\\errmessage{{wd \\the\\wd1}}\\fi\
                 \\ifdim\\ht1={ht}\\else\\errmessage{{ht \\the\\ht1}}\\fi\
                 \\ifdim\\dp1={dp}\\else\\errmessage{{dp \\the\\dp1}}\\fi"
            ));
            assert_eq!(e.error_count, 0, "{body}:\n{}", e.diagnostic_output);
        }

        // a \span after the last column is "Extra alignment tab": the rest
        // of the row starts a new one
        let e = run(
            "\\font\\cmr=cmr10 \\cmr \\setbox1=\\vbox{\\halign{#&#\\cr a&b\\span c\\cr}}\
             \\ifdim\\wd1=10.55559pt\\else\\errmessage{wd \\the\\wd1}\\fi\
             \\ifdim\\ht1=18.94444pt\\else\\errmessage{ht \\the\\ht1}\\fi",
        );
        assert_eq!(e.error_count, 1, "{}", e.diagnostic_output);
        assert!(
            e.diagnostic_output.contains("Extra alignment tab"),
            "{}",
            e.diagnostic_output
        );
    }

    /// tex.web §791 init_col: after `\span\omit\span` the last absorbed
    /// column was not \omit'ted, so its v-part `]` closes the cell. TeX
    /// Live 2026 (cmr10): the row's natural width is 16.11116pt.
    #[test]
    fn span_takes_omit_status_of_the_absorbed_column() {
        let e = run(
            "\\font\\cmr=cmr10 \\cmr \
             \\setbox1\\vbox{\\halign{#&&[#]\\cr a\\span\\omit\\span b\\cr}}\
             \\setbox2\\vbox{\\unvbox1 \\setbox3\\lastbox\\global\\setbox4\\hbox{\\unhbox3}}\
             \\ifdim\\wd4=16.11116pt\\else\\errmessage{wd \\the\\wd4}\\fi",
        );
        assert_eq!(e.error_count, 0, "{}", e.diagnostic_output);
    }
}
