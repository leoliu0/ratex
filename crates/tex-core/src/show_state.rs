//! Bookkeeping that only `\showlists` reads. Ratex builds math lists and
//! alignments with Rust recursion and side tables where tex.web pushes
//! semantic nest levels; these records say which levels tex.web would have
//! had, so `show_activities` can print them (tex.web §218-§219).

use crate::boxes::NodeList;

/// What tex.web had pushed a math level for (the `push_math` caller).
pub(crate) enum ScanKind {
    /// a subformula `{...}` (tex.web appends an empty ord noad first)
    Brace,
    /// the argument of `^` or `_`; `limits` is a `\limits` request that
    /// applies to the tail operator (tex.web `math_limit_switch`)
    Script { sup: bool, limits: Option<u8> },
    /// `\mathaccent`: the accent noad is appended before its nucleus
    Accent { fam: u8, c: u8 },
    /// `\radical`: the radical noad (with its delimiter code) comes first
    Radical { delim: i32 },
    /// `\mathord`...`\mathinner`: a noad of this class
    Class(u8),
    /// `\overline`
    Over,
    /// `\underline`
    Under,
    /// one of the four `\mathchoice` groups
    Choice,
    /// the rest of the group after `\over` and friends (tex.web's
    /// `incompleat_noad`): the numerator is what was split off
    Denominator(PendingFrac),
}

pub(crate) struct PendingFrac {
    pub(crate) num: NodeList,
    pub(crate) thickness: i32,
    pub(crate) left: i32,
    pub(crate) right: i32,
}

/// One entry of `Engine::math_lists` that is not backed by a semantic
/// nest frame (those are formulas and `\left` groups).
pub(crate) struct MathScan {
    /// index into `math_lists`
    pub(crate) index: usize,
    pub(crate) kind: ScanKind,
    /// tex.web `mode_line` of the level
    pub(crate) line: i32,
}

/// One active alignment, outermost first.
pub(crate) struct AlignNest {
    /// level index (`saved_lists.len()` after the alignment's frame was
    /// pushed) of the alignment level tex.web's `init_align` pushes
    pub(crate) level: usize,
    /// its `mode_line`
    pub(crate) line: i32,
    /// `mode_line` of the current row level (`init_row`)
    pub(crate) row_line: i32,
}

#[derive(Default)]
pub(crate) struct ShowState {
    pub(crate) math_scans: Vec<MathScan>,
    /// `mode_line` of every open `{` group in `Engine::math_group_marks`
    pub(crate) brace_lines: Vec<i32>,
    /// set by the caller of `scan_math_group_or_token` for the level it opens
    pub(crate) scan_owner: Option<ScanKind>,
    pub(crate) aligns: Vec<AlignNest>,
    /// `mode_line` of the `\eqno` level
    pub(crate) eqno_line: i32,
}

impl crate::engine::Engine {
    /// Open a `math_lists` entry that has no semantic nest frame of its own
    /// (a script or group argument, a denominator) and record why.
    pub(crate) fn begin_math_scan(&mut self, kind: ScanKind) {
        let index = self.math_lists.len();
        self.math_lists.push(NodeList::new());
        let line = self.nest_line();
        self.show.math_scans.push(MathScan { index, kind, line });
    }

    /// Close the entry opened by [`Self::begin_math_scan`].
    pub(crate) fn end_math_scan(&mut self) -> NodeList {
        self.show.math_scans.pop();
        self.math_lists.pop().unwrap_or_default()
    }

    /// Record the parameters of the fraction whose denominator is scanned.
    pub(crate) fn set_pending_fraction(&mut self, thickness: i32, left: i32, right: i32) {
        if let Some(MathScan {
            kind: ScanKind::Denominator(p),
            ..
        }) = self.show.math_scans.last_mut()
        {
            p.thickness = thickness;
            p.left = left;
            p.right = right;
        }
    }

    /// Close the denominator's record and hand back the numerator.
    pub(crate) fn take_pending_numerator(&mut self) -> NodeList {
        match self.show.math_scans.pop() {
            Some(MathScan {
                kind: ScanKind::Denominator(p),
                ..
            }) => p.num,
            _ => NodeList::new(),
        }
    }

    /// tex.web §1181: `\over` while `incompleat_noad` is set at this level.
    pub(crate) fn fraction_is_ambiguous(&self) -> bool {
        let depth = self.math_lists.len();
        matches!(
            self.show.math_scans.last(),
            Some(MathScan { index, kind: ScanKind::Denominator(_), .. }) if index + 1 == depth
        ) && !self.math_group_marks.iter().any(|(d, _, _)| *d == depth)
    }

    /// tex.web sub_sup: the tail noad already has a script of this kind.
    pub(crate) fn script_repeats(&self, sup: bool) -> bool {
        let depth = self.math_lists.len();
        let boundary = self
            .math_group_marks
            .iter()
            .rev()
            .find(|(d, _, _)| *d == depth)
            .map_or(0, |m| m.1);
        let Some(list) = self.math_lists.last().filter(|l| l.len() > boundary) else {
            return false;
        };
        match list.last() {
            Some(crate::boxes::Node::Scripts { sup: s, sub: x, .. }) => {
                if sup {
                    s.is_some()
                } else {
                    x.is_some()
                }
            }
            Some(crate::boxes::Node::OpLimits { above, below, .. }) => {
                if sup {
                    above.is_some()
                } else {
                    below.is_some()
                }
            }
            _ => false,
        }
    }
}
