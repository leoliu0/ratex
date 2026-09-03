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
//! * nested \halign is supported through a thread-local state stack keyed
//!   by the engine address (the outer preamble/rows survive an inner
//!   \halign used e.g. inside a p-column cell).

use std::cell::RefCell;

use crate::boxes::{Glue, Node, NodeList};
use crate::engine::{Engine, Mode, ScannerStatus};
use crate::eqtb::{Equiv, LevelType};
use crate::prim::{DimParam, GlueParam, Prim, ToksParam};
use crate::token::Token;

/// sentinel appended to a cell's close stream; intercepted by the expansion
/// loop (expand.rs get_token_inner) and routed to `finish_cell_typeset`.
pub const CELL_END_TOKEN: Token = Token(0xE000_0002);

#[derive(Clone, Default)]
pub struct ColSpec {
    /// tokens played before the cell content
    pub u_part: Vec<Token>,
    /// tokens played after the cell content
    pub v_part: Vec<Token>,
    /// extra grid columns absorbed by preamble \span
    pub span: u16,
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
const PH_IDLE: i32 = 0; // no cell open
const PH_U: i32 = 1; // u part of the template is playing
const PH_CONTENT: i32 = 2; // cell content phase
const PH_OMIT: i32 = 4; // template omitted for the current cell
pub(crate) const PH_CLOSE: i32 = 8; // close stream pushed; sentinel not yet seen

const U_PART_SRC: &str = "<align-u>";
const CELL_SRC: &str = "<align-cell>";
const PEEK_SRC: &str = "<align-peek>";

/// saved outer alignment state for nested \halign (tabular in a p-cell)
struct AlignSave {
    preamble: Vec<ColSpec>,
    rows: Vec<Vec<Cell>>,
    cur_row: Vec<Cell>,
    cur_col: i32,
    widths: Vec<i32>,
    scanning_cell: bool,
    in_noalign: bool,
    state: i32,
    done: bool,
    to: Option<(i32, bool)>,
    pushed_base: usize,
    noalign_save_base: usize,
}

thread_local! {
    static ALIGN_STACK: RefCell<Vec<(usize, AlignSave)>> = const { RefCell::new(Vec::new()) };
}

impl Engine {
    fn align_phase(&self) -> i32 {
        self.align_state & (PH_U | PH_CONTENT)
    }

    fn align_omitted(&self) -> bool {
        self.align_state & PH_OMIT != 0
    }

    fn align_set_phase(&mut self, phase: i32) {
        self.align_state = (self.align_state & PH_OMIT) | phase;
    }

    fn engine_key(&self) -> usize {
        self as *const Engine as usize
    }

    /// true when an outer alignment state is saved for this engine (we are
    /// the inner \halign of a nesting)
    fn align_has_save(&self) -> bool {
        let key = self.engine_key();
        ALIGN_STACK.with(|s| s.borrow().iter().any(|(k, _)| *k == key))
    }

    // ------------------------------------------------------------------
    // \halign
    // ------------------------------------------------------------------

    pub fn begin_halign(&mut self) {


        if self.scanner_status == ScannerStatus::Aligning {
            // nested \halign (e.g. tabular inside a p-column cell): save the
            // outer alignment so the inner one can use the global fields
            let save = AlignSave {
                preamble: std::mem::take(&mut self.align_preamble),
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
                noalign_save_base: self.align_noalign_save_base,
            };
            let key = self.engine_key();
            ALIGN_STACK.with(|s| s.borrow_mut().push((key, save)));
        }
        // tex.web: \halign inside a display formula is legal only as the
        // whole formula (\eqalign-style); other modes enter the alignment
        if matches!(self.mode, Mode::DisplayMath) {
            self.error("Improper \\halign inside $$'s");
            self.align_nested_restore();
            return;
        }
        if self.mode == Mode::Horizontal {
            // tex.web negates unrestricted horizontal mode (giving the
            // alignment valign-like semantics); LaTeX never uses this form,
            // tabular wraps its \halign in \hbox (restricted mode)
            self.error("\\halign in horizontal mode");
            self.align_nested_restore();
            return;
        }
        self.scanner_status = ScannerStatus::Aligning;
        self.align_preamble.clear();
        // \halign to <dimen> / \halign spread <dimen> (tex.web scan_spec)
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
        ));
        self.eqtb.push_level(LevelType::Box);
        self.box_targets.push(None);
        self.box_shifts.push(0);
        self.box_kinds.push(7);
        self.mode = Mode::InternalVertical;
        self.prev_depth = -1000 * 65536;
        self.align_rows.clear();
        self.align_cur_row.clear();
        self.align_cur_col = 0;
        self.align_in_noalign = false;
        self.align_state = PH_IDLE;
        self.align_done = false;
        self.align_row_inspect();
    }

    fn align_nested_restore(&mut self) {
        let key = self.engine_key();
        let outer = ALIGN_STACK.with(|s| {
            let mut s = s.borrow_mut();
            // the innermost saved state for this engine
            s.iter()
                .rposition(|(k, _)| *k == key)
                .map(|i| s.remove(i).1)
        });
        if let Some(sv) = outer {
            self.align_preamble = sv.preamble;
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
            self.align_noalign_save_base = sv.noalign_save_base;
        }
    }

    /// scan `{ <u # v>... \cr`; the preamble ends at \cr/\crcr. Returns
    /// false (after reporting) on any fatal error.
    fn scan_align_preamble(&mut self) -> bool {
        // tex.web scan_left_brace
        self.skip_spaces_relax();
        let t = self.get_token();
        if t == crate::input::EOF_MARKER {
            self.error("File ended while scanning an alignment preamble");
            self.end_occurred = true;
            return false;
        }
        if !t.is_char() || t.cc() != 1 {
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
                self.error("File ended while scanning an alignment preamble");
                self.end_occurred = true;
                return false;
            }
            let t = if t == crate::input::PAR_END {
                Token::from_cs(self.ids.par)
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
                    // \span joins this entry with the next one: the entry
                    // keeps accumulating and covers span+1 grid columns
                    cur.span = cur.span.saturating_add(1);
                    in_u = true;
                    continue;
                }
                Some(Prim::Omit) => {
                    self.error("\\omit is not allowed in an alignment preamble");
                    continue;
                }
                Some(Prim::GlueP(crate::prim::GlueParam::TabSkip)) => {
                    // tex.web §782: inter-column \\tabskip glue is scanned
                    // during the preamble and never stored in the template;
                    // pack time reads the \\tabskip param (align glue code).
                    let _ = self.scan_glue(false);
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

                    // LaTeX tabular emits & between column templates: it is
                    // the entry separator, so finalize this entry and start
                    // the next one with a fresh u part
                    4 if depth == 0 => {
                        entries.push(std::mem::take(&mut cur));
                        in_u = true;
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
        entries.push(cur);
        self.align_preamble = entries;
        if crate::debug_flag("ALIGNTRACE") {
            for (i, e) in self.align_preamble.iter().enumerate() {
                eprintln!(
                    "PREAMBLE col{} span={} u=[{}] v=[{}]",
                    i,
                    e.span,
                    self.tokens_to_string(&e.u_part),
                    self.tokens_to_string(&e.v_part)
                );
            }
        }

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
        ));
        self.eqtb.push_level(LevelType::Box);

        self.box_targets.push(None);
        self.box_shifts.push(0);
        self.box_kinds.push(CELL_GROUP_KIND);
        self.mode = mode;
        if mode.is_v() {
            self.prev_depth = -1000 * 65536;
        }
    }

    /// start the cell at `align_cur_col`
    /// start the cell at `align_cur_col`. `first` is a token already
    /// looked at by align_row_inspect (omit / content); `None` peeks.
    fn align_start_cell(&mut self, first: Option<crate::token::Token>) {
        let col = self.align_cur_col as usize;
        if col >= self.align_preamble.len() {
            return; // empty preamble: rows have no cells
        }
        let spec = self.align_preamble[col].clone();
        self.align_push_cell_group(Mode::RestrictedHorizontal);
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
                    self.error("File ended while starting an alignment cell");
                    self.end_occurred = true;
                    return;
                }
                t
            }
        };
        let is_omit = t.is_cs() && matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Prim(Prim::Omit)));
        if is_omit {
            self.align_state = PH_CONTENT | PH_OMIT;
            return;
        }
        // content below the u part
        self.align_pushed_base = self.pushed.len();
        self.input.push_toks(vec![t], PEEK_SRC);
        if spec.u_part.is_empty() {
            self.align_set_phase(PH_CONTENT);
        } else {
            self.align_pushed_base = self.pushed.len();
            self.input.push_toks(spec.u_part, U_PART_SRC);
        }


    }


    /// discard the not-yet-played remainder of the u part source (on \omit,
    /// & or \cr arriving while the u part is still playing)
    fn align_discard_u_part(&mut self) {
        let is_u = matches!(
            self.input.stack.last(),
            Some(crate::input::Source::TokList { name, .. }) if *name == U_PART_SRC
        );
        if is_u {
            self.input.stack.pop();
        }
    }
    /// token terminating a close stream. The \crcr primitive token is used
    /// (not CELL_END_TOKEN): scanners may look ahead across the close
    /// stream (e.g. \hskip's fill check), and a scanner swallowing
    /// CELL_END_TOKEN would pack the cell mid-scan. A \crcr cs token is
    /// pushed back by scanners and only acted on at dispatch level, like
    /// tex.web's frozen \cr.
    fn crcr_token(&self) -> Token {
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
    fn align_push_close(&mut self, row_continues: bool) {
        let col = self.align_cur_col as usize;
        self.align_scanning_cell = row_continues;
        let omit = self.align_omitted();
        self.align_set_phase(PH_CONTENT);
        self.align_state |= PH_CLOSE;
        let mut close: Vec<Token> = Vec::new();
        if !omit {
            // tex.web fin_col: the v part played at the end of a cell is
            // that of the LAST entry the cell covers (cur_align advances
            // through spanned columns)
            let last = col + self.align_cur_row.get(col).map(|c| c.span as usize).unwrap_or(0);
            if let Some(spec) = self.align_preamble.get(last) {
                close.extend(spec.v_part.iter().cloned());
            }
        }
        close.push(self.crcr_token());
        self.align_pushed_base = self.pushed.len();
        self.input.push_toks(close, CELL_SRC);


    }


    pub fn align_tab(&mut self) {
        if crate::debug_flag("ALIGN2") {
            eprintln!(
                "ATAB col={} st={:?} phase={:#x} kinds_last={:?} ss_len={} mode={:?}",
                self.align_cur_col,
                self.scanner_status,
                self.align_state,
                self.box_kinds.last(),
                self.eqtb.save_stack.len(),
                self.mode
            );
        }
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
        if self.align_phase() == PH_IDLE
            || !self.box_kinds.contains(&CELL_GROUP_KIND)
        {
            // tex.web: & reaching main_control inside a nested group of a
            // cell (braces hide it from the row) is "Misplaced alignment
            // tab"; PH_IDLE covers & between rows and at the row boundary
            self.error("Misplaced alignment tab character &");
            return;
        }
        let col = self.align_cur_col as usize;
        let cur_span = self.align_cur_row.get(col).map(|c| c.span as usize).unwrap_or(0);
        if col + 1 + cur_span >= self.align_preamble.len() {
            self.error("Extra alignment tab has been changed to \\cr");
            if self.align_phase() == PH_U {
                self.align_discard_u_part();
            }
            self.align_push_close(false);
            return;
        }
        if self.align_phase() == PH_U {
            self.align_discard_u_part();
        }
        self.align_push_close(true);
    }

    pub fn align_cr(&mut self) {
        if crate::debug_flag("ALIGN2") {
            eprintln!(
                "ACR col={} st={:?} phase={:#x} kinds_last={:?} close_flag={} in_noalign={} ss_len={} mode={:?}",
                self.align_cur_col,
                self.scanner_status,
                self.align_state,
                self.box_kinds.last(),
                self.align_state & PH_CLOSE != 0,
                self.align_in_noalign,
                self.eqtb.save_stack.len(),
                self.mode
            );
        }
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
        // A template u part may open box groups inside the cell (LaTeX
        // p-columns: `\@startpbox` = `\vtop\bgroup ...`), so the CELL group
        // is not necessarily the top of box_kinds. Only a \cr between rows
        // or fully outside a cell is misplaced; otherwise let the close
        // stream's v part unwind the template groups and finish the cell.
        if self.align_phase() == PH_IDLE || !self.box_kinds.contains(&CELL_GROUP_KIND) {
            self.error("Misplaced \\cr");
            return;
        }
        if self.align_phase() == PH_U {
            self.align_discard_u_part();
        }
        self.align_push_close(false);
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
    }

    /// \span in a row: the current cell absorbs the next grid column; the
    /// absorbed column's u part plays next (skipped when \omit follows).
    pub fn align_span(&mut self) {
        if self.scanner_status != ScannerStatus::Aligning || self.align_phase() == PH_IDLE {
            self.error("\\span outside alignment");
            return;
        }
        let col = self.align_cur_col as usize;
        let span = {
            while self.align_cur_row.len() <= col {
                self.align_cur_row.push(Cell::default());
            }
            let cell = &mut self.align_cur_row[col];
            cell.span = cell.span.saturating_add(1);
            cell.span as usize
        };
        if self.align_phase() == PH_U {
            self.align_discard_u_part();
        }
        let t = self.align_peek_expanding();
        if t == crate::input::EOF_MARKER {
            self.error("File ended after \\span");
            self.end_occurred = true;
            return;
        }
        let is_omit = t.is_cs() && matches!(self.eqtb.resolve(t.cs_id()), Some(Equiv::Prim(Prim::Omit)));
        if is_omit {
            self.align_state = (self.align_state & PH_OMIT) | PH_CONTENT | PH_OMIT;
            return;
        }
        self.align_state = (self.align_state & PH_OMIT) | PH_U;
        // re-queue the peeked token below the absorbed column's u part
        self.input.push_toks(vec![t], PEEK_SRC);
        let u = self
            .align_preamble
            .get(col + span)
            .map(|s| s.u_part.clone())
            .unwrap_or_default();
        if u.is_empty() {
            self.align_state = (self.align_state & PH_OMIT) | PH_CONTENT;
        } else {
            self.align_pushed_base = self.pushed.len();
            self.input.push_toks(u, U_PART_SRC);
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

    fn align_pop_cell_group(&mut self) -> Option<NodeList> {
        if self.box_kinds.last() != Some(&CELL_GROUP_KIND) || self.saved_lists.is_empty() {
            return None;
        }
        let inner = std::mem::take(&mut self.cur_list);
        let _ = self.box_targets.pop().flatten();
        let _ = self.box_shifts.pop().unwrap_or(0);
        let _ = self.box_kinds.pop();
        self.pop_group();
        let (om, ol, pd, sf) = self.saved_lists.pop().unwrap();
        self.cur_list = ol;
        self.mode = om; // tex.web unsave: the enclosing level's mode returns
        self.prev_depth = pd;
        self.space_factor = sf;
        Some(inner)
    }

    pub fn align_noalign(&mut self) {
        if self.scanner_status != ScannerStatus::Aligning
            || self.align_phase() != PH_IDLE
            || self.align_in_noalign
        {
            self.error("Misplaced \\noalign");
            return;
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
        self.align_push_cell_group(Mode::InternalVertical);
        // group-depth watermark: the noalign body ends when the save stack
        // returns here (i.e. the body's `{`-group just closed). The cell
        // group pushed above is LevelType::Box — without an explicit
        // Simple level for the consumed `{`, the body's `}` would close
        self.align_noalign_save_base = self.eqtb.save_stack.len();
        self.align_pushed_base = self.pushed.len();
        self.eqtb.push_level(LevelType::Simple);
    }

    /// pack the open cell into the current row and start the next cell /
    /// finish the row
    fn align_finish_cell_now(&mut self) {
        let Some(inner) = self.align_pop_cell_group() else {
            return;
        };
        let col = self.align_cur_col as usize;
        let span = self.align_cur_row.get(col).map(|c| c.span).unwrap_or(0);
        let packed = crate::boxes::hpack(inner, None, crate::boxes::HBOX, &self.eqtb).node;
        while self.align_cur_row.len() <= col {
            self.align_cur_row.push(Cell::default());
        }
        self.align_cur_row[col] = Cell { packed: Some(packed), span };
        self.align_state = PH_IDLE;
        if self.align_scanning_cell {
            // & closed this cell: start the next one
            let next = col + 1 + span as usize;
            if next >= self.align_preamble.len() {
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

    /// pack the open \noalign text into a vertical box and store it
    pub(crate) fn align_finish_noalign_now(&mut self) {
        let Some(inner) = self.align_pop_cell_group() else {
            return;
        };
        self.align_in_noalign = false;
        self.align_state = PH_IDLE;
        let packed = crate::boxes::vpack(inner, None, crate::boxes::VBOX, &self.eqtb).node;
        self.align_rows
            .push(vec![Cell { packed: Some(packed), span: NOALIGN_SPAN }]);
        // \noalign may be followed by another \noalign, \cr or }
        self.align_row_inspect();
    }

    /// pack up the finished row and inspect what follows it
    fn align_finish_row(&mut self) {
        let row = std::mem::take(&mut self.align_cur_row);
        if !row.is_empty() {
            self.align_rows.push(row);
        }
        self.align_cur_col = 0;
        self.align_row_inspect();
    }

    /// peek one token, expanding macros and skipping spaces first:
    /// tex.web's align_peek uses get_x_token, so row boundaries see through
    /// macros like a \noalign wrapper defined as \def\br{\noalign{\hrule}}
    fn align_peek_expanding(&mut self) -> Token {
        loop {
            let t = self.get_x_raw();
            if t == crate::input::EOF_MARKER {
                return t;
            }
            if t.is_char() && t.cc() == 10 {
                continue;
            }
            return t;
        }
    }

    /// after a \cr (or at alignment start): decide between \noalign, another
    /// \cr, the alignment's closing brace, or the next row's first cell.
    fn align_row_inspect(&mut self) {
        loop {
            let t = self.align_peek_expanding();
            if t == crate::input::EOF_MARKER {
                self.error("File ended during an alignment");
                self.end_occurred = true;
                return;
            }
            if t.is_cs() {
                match self.cur_prim {
                    Some(Prim::NoAlign) => {
                        self.align_noalign();
                        return;
                    }
                    Some(Prim::Cr) | Some(Prim::CrCr) => continue,
                    _ => {}
                }
            }
            if t.is_char() && t.cc() == 2 {
                self.align_done = true;
                self.align_pushed_base = self.pushed.len();
                self.input.push_toks(vec![t], PEEK_SRC);
                return;
            }
            self.align_start_row(Some(t));
            return;
        }
    }

    fn align_start_row(&mut self, first: Option<crate::token::Token>) {
        self.align_cur_col = 0;
        let ec = (*self.eqtb.tok_params[ToksParam::EveryCr.idx() as usize]).clone();
        if !ec.is_empty() && !self.align_everycr_done {
            // tex.web endv: after \\cr the \\everycr tokens are inserted
            // BEFORE the next row is peeked, so a leading \\noalign (longtable
            // header machinery) is legal at PH_IDLE. Park the already-peeked
            // first content token below everycr and re-enter inspection.
            if let Some(t) = first {
                self.align_pushed_base = self.pushed.len();
                self.input.push_toks(vec![t], PEEK_SRC);
            }
            self.align_everycr_done = true;
            self.align_pushed_base = self.pushed.len();
            self.input.push_toks(ec, "<everycr>");
            self.align_row_inspect();
            return;
        }
        self.align_everycr_done = false;
        self.align_start_cell(first);
    }
    // ------------------------------------------------------------------
    // final packaging (build.rs end_box, kind 7)
    // ------------------------------------------------------------------

    pub fn finish_halign(&mut self) {
        let rows_in = std::mem::take(&mut self.align_rows);
        let ncols = self.align_preamble.len();
        let tabskip = self.eqtb.glue_params[GlueParam::TabSkip.idx() as usize].clone();
        let mut widths = vec![0i32; ncols];
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
        if crate::debug_flag("ALIGNW") {
            for (ri, row) in rows_in.iter().enumerate() {
                for (c, cell) in row.iter().enumerate() {
                    let w = match &cell.packed { Some(Node::Box { w, .. }) => *w, _ => 0 };
                    eprintln!("ALIGNW row{} cell{} span={} natw={:.1}", ri, c, cell.span, w as f64 / 65536.0);
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
                let pre: i64 =
                    (c..j).map(|k| widths[k] as i64 + tabskip.width as i64).sum();
                w = w.max(nat as i64 - pre);
            }
            widths[j] = w.max(0) as i32;
        }
        if crate::debug_flag("ALIGNW") {
            eprintln!("ALIGNW widths={:?}", widths.iter().map(|w| *w as f64 / 65536.0).collect::<Vec<_>>());
        }
        self.align_col_widths = widths.clone();
        let mut rows: NodeList = Vec::new();
        let bs = self.eqtb.glue_params[GlueParam::BaselineSkip.idx() as usize].clone();
        let ls = self.eqtb.glue_params[GlueParam::LineSkip.idx() as usize].clone();
        let lsl = self.eqtb.dim_params[DimParam::LineSkipLimit.idx() as usize];
        let mut prev: Option<(i32, i32)> = None;
        for row in rows_in {
            if row.len() == 1 && row[0].span == NOALIGN_SPAN {
                if let Some(node) = row.into_iter().next().and_then(|c| c.packed) {
                    let (h, d) = match &node {
                        Node::Box { h, d, .. } => (*h, *d),
                        Node::Rule { height, depth, .. } => (*height, *depth),
                        _ => (0, 0),
                    };
                    align_interline(&mut rows, &mut prev, h, d, &bs, &ls, lsl);
                    rows.push(node);
                }
                continue;
            }
            let mut line: NodeList = Vec::new();
            line.push(Node::Glue(tabskip.clone()));
            let mut g = 0usize;
            for (c, cell) in row.into_iter().enumerate() {
                while g < c {
                    if g > 0 {
                        line.push(Node::Glue(tabskip.clone()));
                    }
                    g += 1;
                }
                if c > 0 {
                    line.push(Node::Glue(tabskip.clone()));
                }
                let s = cell.span as usize;
                let target: i64 = if ncols == 0 {
                    0
                } else {
                    let lo = c;
                    let hi = (c + s).min(ncols - 1);
                    (lo..=hi).map(|i| widths[i] as i64).sum::<i64>()
                        + s as i64 * tabskip.width as i64
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
                    line.push(Node::Glue(tabskip.clone()));
                }
                g += 1;
            }
            line.push(Node::Glue(tabskip.clone()));
            let rowbox = match self.align_to {
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
            let (h, d) = match &rowbox {
                Node::Box { h, d, .. } => (*h, *d),
                _ => (0, 0),
            };
            align_interline(&mut rows, &mut prev, h, d, &bs, &ls, lsl);
            rows.push(rowbox);
        }
        self.align_preamble.clear();
        self.align_rows.clear();
        self.align_cur_row.clear();
        self.align_cur_col = 0;
        self.align_done = false;
        self.align_in_noalign = false;
        self.align_state = PH_IDLE;
        let nested = self.align_has_save();
        self.align_nested_restore();
        if !nested {
            self.scanner_status = ScannerStatus::Normal;
            self.align_to = None;
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

    fn run(src: &str) -> Engine {
        let mut e = Engine::new(true);
        e.init_primitives();
        e.add_nullfont();
        // unbooted engine: catcodes default to plain-like INITEX values only
        // after \catcode assignments, so set the alignment-relevant ones
        let full = format!(
            "\\catcode`\\{{=1 \\catcode`\\}}=2 \\catcode`\\#=6 \\catcode`\\&=4 \\baselineskip=12pt {}\n",
            src
        );
        e.input.push_file("driver.tex".to_string(), full.as_bytes().to_vec());
        e.run();
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
            Node::Box { glue_sign, glue_set, .. } => {
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
        // (655362su), which the v part of every column-0 cell contributes
        assert_eq!(w0, wc * 3 + 655362);
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
        let (_, list) = vbox_of(&e);
        let boxes = list.iter().filter(|n| matches!(n, Node::Box { .. })).count();
        // the \noalign text is packed into its own vbox holding the rule;
        // find it among the interrow glue / row boxes
        let noalign = list
            .iter()
            .find(|n| matches!(n, Node::Box { kind: crate::boxes::VBOX, .. }))
            .expect("noalign vbox inside the alignment vbox");
        match noalign {
            Node::Box { list: inner, .. } => assert_eq!(
                inner.iter().filter(|n| matches!(n, Node::Rule { .. })).count(),
                1,
                "rule inside noalign vbox: {:?}",
                inner
            ),
            _ => unreachable!(),
        }
        // row / rule / row order
        assert!(matches!(list[0], Node::Box { .. }));
        assert!(matches!(list[list.len() - 1], Node::Box { .. }));
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
        assert_eq!(widths[1] as i64, expect1, "deficit on column 1 (last column of 2-col span)");
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
            Node::Box { list: inner_cell, .. } => {
                assert!(matches!(inner_cell[0], Node::Box { .. }), "{:?}", inner_cell)
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
        assert!(e.term.contains("alignment tab"), "{}", e.term);
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
            Node::Box { glue_sign, glue_set, .. } => {
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
        // 3 rules + 3 rows, interleaved with glue; every noalign rule is a
        // vbox wrapping one hrule
        let boxes: Vec<&Node> = align_box
            .iter()
            .filter(|n| matches!(n, Node::Box { .. }))
            .collect();
        assert_eq!(boxes.len(), 6, "rules + rows: {:?}", align_box);
        let rules = align_box
            .iter()
            .filter_map(|n| match n {
                Node::Box { kind: crate::boxes::VBOX, list, .. } => Some(list),
                _ => None,
            })
            .flat_map(|l| l.iter())
            .filter(|n| matches!(n, Node::Rule { .. }))
            .count();
        assert_eq!(rules, 3, "top/mid/bottom rules");
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
    fn misplaced_align_tokens_error() {
        // stray alignment tokens outside any alignment
        let e = run("x & y \\cr\n");
        assert!(
            e.term.contains("alignment tab") || e.term.contains("Misplaced \\cr"),
            "& and \\cr outside alignment: {}",
            e.term
        );
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

}
