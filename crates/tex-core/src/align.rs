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
//!   pass: \hfil etc. stretch), interleaves tabskip glue between grid
//!   columns and baselineskip/lineskip glue between rows, packs the vbox
//!   and appends it like a box result (or assigns it to \setbox target).
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
use crate::eqtb::LevelType;
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
const PH_CLOSE: i32 = 8; // close stream pushed; sentinel not yet seen

const U_PART_SRC: &str = "<align-u>";
const CELL_SRC: &str = "<align-cell>";
const PEEK_SRC: &str = "<align-peek>";

/// saved outer alignment state for nested \halign (tabular in a p-cell)
#[derive(Default)]
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
            };
            let key = self.engine_key();
            ALIGN_STACK.with(|s| s.borrow_mut().push((key, save)));
        }
        if !self.mode.is_v() {
            self.error("\\halign in horizontal mode");
            self.align_nested_restore();
            return;
        }
        self.scanner_status = ScannerStatus::Aligning;
        self.align_preamble.clear();
        if !self.scan_align_preamble() {
            self.scanner_status = ScannerStatus::Normal;
            self.align_state = PH_IDLE;
            self.align_nested_restore();
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
        self.eqtb.push_level(LevelType::Group);
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
        // inspect what follows the preamble: first row, \noalign or }
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
            let t = self.get_token();
            if t == crate::input::EOF_MARKER {
                self.error("File ended while scanning an alignment preamble");
                self.end_occurred = true;
                return false;
            }
            if t.is_cs() {
                match self.cur_prim {
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
                    _ => {}
                }
            }
            if t.is_char() {
                match t.cc() {
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
                    6 if depth == 0 => {
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
                cur.u_part.push(t);
            } else {
                cur.v_part.push(t);
            }
        }
        // the entry in progress at \cr is always a column, even when its
        // template is empty (#\cr is a legal single bare column)
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
        ));
        self.eqtb.push_level(LevelType::Group);
        self.box_targets.push(None);
        self.box_shifts.push(0);
        self.box_kinds.push(CELL_GROUP_KIND);
        self.mode = mode;
        if mode.is_v() {
            self.prev_depth = -1000 * 65536;
        }
    }

    /// start the cell at `align_cur_col`
    fn align_start_cell(&mut self) {
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
        // \omit may be the very first token of the cell: peek (expanding)
        let t = self.get_token();
        if t == crate::input::EOF_MARKER {
            self.error("File ended while starting an alignment cell");
            self.end_occurred = true;
            return;
        }
        if t.is_cs() && self.cur_prim == Some(Prim::Omit) {
            self.align_state = PH_CONTENT | PH_OMIT;
            return;
        }
        // re-queue the peeked token below the u part
        self.input.push_toks(vec![t], PEEK_SRC);
        if spec.u_part.is_empty() {
            self.align_set_phase(PH_CONTENT);
        } else {
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
            if let Some(spec) = self.align_preamble.get(col) {
                close.extend(spec.v_part.iter().cloned());
            }
        }
        close.push(self.crcr_token());
        self.input.push_toks(close, CELL_SRC);
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
        if self.align_phase() == PH_IDLE {
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
        if self.align_phase() == PH_IDLE {
            // \cr between rows: the row inspection consumes those; a
            // leftover one here is ignored
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
        let t = self.get_token();
        if t == crate::input::EOF_MARKER {
            self.error("File ended after \\span");
            self.end_occurred = true;
            return;
        }
        if t.is_cs() && self.cur_prim == Some(Prim::Omit) {
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
            self.input.push_toks(u, U_PART_SRC);
        }
    }

    // ------------------------------------------------------------------
    // \noalign
    // ------------------------------------------------------------------


    // ------------------------------------------------------------------
    // cell completion (at dispatch level, from the \crcr sentinel)
    // ------------------------------------------------------------------

    /// kept for expand.rs's CELL_END interception (defensive only: close
    /// streams are terminated with a \crcr primitive token, which scanners
    /// push back instead of swallowing mid-scan)
    pub fn finish_cell_typeset(&mut self) {
        self.align_finish_cell_now();
    }

    /// pop this module's cell group context; returns the collected list
    fn align_pop_cell_group(&mut self) -> Option<NodeList> {
        if self.box_kinds.last() != Some(&CELL_GROUP_KIND) || self.saved_lists.is_empty() {
            return None;
        }
        let inner = std::mem::take(&mut self.cur_list);
        let _ = self.box_targets.pop().flatten();
        let _ = self.box_shifts.pop().unwrap_or(0);
        let _ = self.box_kinds.pop();
        self.eqtb.pop_level();
        let (om, ol, pd, sf) = self.saved_lists.pop().unwrap();
        self.cur_list = ol;
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
        let mut toks = self.scan_general_text();
        self.align_in_noalign = true;
        self.align_push_cell_group(Mode::InternalVertical);
        self.align_state |= PH_CLOSE;
        toks.push(self.crcr_token());
        self.input.push_toks(toks, CELL_SRC);
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
                self.align_start_cell();
            }
        } else {
            self.align_finish_row();
        }
    }

    /// pack the open \noalign text into a vertical box and store it
    fn align_finish_noalign_now(&mut self) {
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

    /// after a \cr (or at alignment start): decide between \noalign, another
    /// \cr, the alignment's closing brace, or the next row's first cell.
    fn align_row_inspect(&mut self) {
        loop {
            let t = self.get_token();
            if t == crate::input::EOF_MARKER {
                self.error("File ended during an alignment");
                self.end_occurred = true;
                return;
            }
            if t.is_cs() {
                match self.cur_prim {
                    Some(Prim::NoAlign) => {
                        self.align_noalign(); // re-inspects when its box closes
                        return;
                    }
                    // e.g. \crcr (or \cr\cr) between rows
                    Some(Prim::Cr) | Some(Prim::CrCr) => continue,
                    _ => {}
                }
            }
            if t.is_char() && t.cc() == 2 {
                self.align_done = true;
            }
            self.input.push_toks(vec![t], PEEK_SRC);
            if !self.align_done {
                self.align_start_row();
            }
            return;
        }
    }

    /// start the next row: \everycr, then the first cell
    fn align_start_row(&mut self) {
        self.align_cur_col = 0;
        self.align_start_cell();
        let ec = (*self.eqtb.tok_params[ToksParam::EveryCr.idx() as usize]).clone();
        if !ec.is_empty() {
            // plays before the row's u part
            self.input.push_toks(ec, "<everycr>");
        }
    }

    // ------------------------------------------------------------------
    // final packaging (build.rs end_box, kind 7)
    // ------------------------------------------------------------------

    pub fn finish_halign(&mut self) {
        let rows_in = std::mem::take(&mut self.align_rows);
        let ncols = self.align_preamble.len();
        let tabskip = self.eqtb.glue_params[GlueParam::TabSkip.idx() as usize].clone();
        let mut widths = vec![0i32; ncols];

        // pass 1: single-column cells set the column widths
        let mut spans: Vec<(usize, usize, i32)> = Vec::new(); // (col, span, width)
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
                } else {
                    spans.push((c, cell.span as usize, w));
                }
            }
        }
        // pass 2: spanning cells widen the covered columns if needed
        spans.sort_by_key(|&(_, s, _)| s);
        for (c, s, w) in spans {
            if ncols == 0 || c >= ncols {
                continue;
            }
            let lo = c;
            let hi = (c + s).min(ncols - 1);
            let n = (hi - lo + 1) as i64;
            let avail: i64 = (lo..=hi).map(|i| widths[i] as i64).sum::<i64>()
                + s as i64 * tabskip.width as i64;
            if w as i64 > avail {
                let deficit = w as i64 - avail;
                let share = deficit / n;
                let rem = deficit % n;
                for (k, i) in (lo..=hi).enumerate() {
                    widths[i] += (share + if (k as i64) < rem { 1 } else { 0 }) as i32;
                }
            }
        }
        self.align_col_widths = widths.clone();

        // assemble the rows
        let mut rows: NodeList = Vec::new();
        let bs = self.eqtb.glue_params[GlueParam::BaselineSkip.idx() as usize].clone();
        let ls = self.eqtb.glue_params[GlueParam::LineSkip.idx() as usize].clone();
        let lsl = self.eqtb.dim_params[DimParam::LineSkipLimit.idx() as usize];
        let mut prev: Option<(i32, i32)> = None; // (height, depth) of last box
        for row in rows_in {
            // a \noalign entry: insert its box (or rule) as-is
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
            let mut g = 0usize; // grid column cursor
            for (c, cell) in row.into_iter().enumerate() {
                // tabskip glue for grid columns skipped before this cell
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
                // the "unset box" pass: re-pack to the final column width
                let inner = match cell.packed {
                    Some(Node::Box { list, .. }) => list,
                    Some(other) => vec![other],
                    None => Vec::new(),
                };
                line.push(
                    crate::boxes::hpack(inner, Some(target as i32), crate::boxes::HBOX, &self.eqtb)
                        .node,
                );
                g = c + s + 1;
            }
            // trailing tabs for grid columns with no cell in this row
            while g < ncols {
                if g > 0 {
                    line.push(Node::Glue(tabskip.clone()));
                }
                g += 1;
            }
            let rowbox = crate::boxes::hpack(line, None, crate::boxes::HBOX, &self.eqtb).node;
            let (h, d) = match &rowbox {
                Node::Box { h, d, .. } => (*h, *d),
                _ => (0, 0),
            };
            align_interline(&mut rows, &mut prev, h, d, &bs, &ls, lsl);
            rows.push(rowbox);
        }

        let vbox = crate::boxes::vpack(rows, None, crate::boxes::VBOX, &self.eqtb).node;
        self.align_preamble.clear();
        self.align_rows.clear();
        self.align_cur_row.clear();
        self.align_cur_col = 0;
        self.align_done = false;
        self.align_in_noalign = false;
        self.align_state = PH_IDLE;
        self.scanner_status = ScannerStatus::Normal;
        self.align_nested_restore();
        // append like a box result (setbox0=\halign{...} assigns instead)
        if let Some(idx) = self.setbox_target.take() {
            self.eqtb.assign_box(idx, Some(vbox), self.global_flag);
            self.global_flag = false;
        } else {
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
        let mut gap = bs.width - pd - h;
        if gap < lsl {
            gap = ls.width;
        }
        if gap != 0 {
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
        // row 1 = [cell(a), tabskip, cell(bb)]
        let r1 = row_of(&list[0]);
        assert_eq!(r1.len(), 3);
        assert_eq!(box_w(&r1[0]), w0);
        assert_eq!(box_w(&r1[2]), w1);
        // cell(a) + \quad stretched to the width of 'ccc': fil glue set
        match &r1[0] {
            Node::Box { glue_sign, glue_set, .. } => {
                assert_eq!(*glue_sign, 1);
                assert!(*glue_set > 0.0);
            }
            _ => unreachable!(),
        }
        // row 2 = [cell(ccc), tabskip, cell(d)]
        let r2 = row_of(&list[2]);
        assert_eq!(r2.len(), 3);
        assert_eq!(box_w(&r2[0]), w0);
        assert_eq!(box_w(&r2[2]), w1);
        // 'ccc' at natural width: no glue set
        match &r2[0] {
            Node::Box { glue_sign, .. } => assert_eq!(*glue_sign, 0),
            _ => unreachable!(),
        }
        // column 1 is as wide as 'ccc', column 2 as wide as 'bb'
        let f = &e.eqtb.fonts[e.cur_font as usize];
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
        // the spanning cell was widened over columns 1 and 2
        let (w1, w2) = (e.align_col_widths[1], e.align_col_widths[2]);
        assert!(w1 > 0, "span deficit distributed: {:?}", e.align_col_widths);
        assert!((w1 - w2).abs() <= 1, "equal split: {:?}", e.align_col_widths);
        let r = row_of(&list[0]);
        assert_eq!(r.len(), 3, "cell, tabskip, span cell: {:?}", r);
        assert_eq!(box_w(&r[2]), w1 + w2);
        assert_eq!(w, box_w(&r[0]) + w1 + w2);
    }

    #[test]
    fn valign_errors_gracefully() {
        let e = run("\\valign{#\\cr a\\cr}\n");
        assert!(e.error_count > 0, "valign should report an error");
        assert!(e.end_occurred, "valign stops the job cleanly");
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
        // the alignment vbox lands inside the \vbox, which lands on the page
        let (_, list) = vbox_of(&e);
        assert_eq!(list.len(), 1, "outer vbox: {:?}", list);
        let inner = match &list[0] {
            Node::Box { kind: crate::boxes::VBOX, list, .. } => list,
            other => panic!("expected inner vbox, got {:?}", other),
        };
        assert_eq!(inner.len(), 3, "2 rows + interrow glue: {:?}", inner);
        let w = e.align_col_widths[0];
        assert_eq!(box_w(&inner[0]), w);
        assert_eq!(box_w(&inner[2]), w);
    }
}
