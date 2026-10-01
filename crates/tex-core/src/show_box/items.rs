//! Display items: the node and noad shapes of tex.web §184 (unset nodes) and
//! §690-§698 (noads) that Ratex keeps in other representations. The views
//! below rebuild tex.web's shapes from Ratex's math lists (flat
//! `\left...\right` markers, fam-255 group markers, scripts wrapping their
//! nucleus) so that `\showlists` prints exactly what tex.web prints.

use super::{invisible, BoxDisplay, DEFAULT_CODE};
use crate::boxes::{Glue, Node};

const CL_OP: u8 = 1;

pub(super) enum Item<'a> {
    /// an ordinary node printed by `display_node`
    Node(&'a Node),
    /// a glue node made up for the display (tabskip, interline glue)
    Glue(Glue),
    /// an alignment cell or row before `fin_align` sets it (tex.web unset_node)
    Unset(Box<Unset<'a>>),
    Noad(Box<Noad<'a>>),
    Frac(Box<FracItem<'a>>),
    /// the display, text, script and scriptscript mlists of a choice node
    Choice(Box<[Vec<Item<'a>>; 4]>),
}

pub(super) struct Unset<'a> {
    pub(super) h: i32,
    pub(super) d: i32,
    pub(super) w: i32,
    /// tex.web `span_count` (extra columns)
    pub(super) span: u16,
    pub(super) stretch: Option<(i32, u8)>,
    pub(super) shrink: Option<(i32, u8)>,
    pub(super) list: Vec<Item<'a>>,
}

#[derive(Clone, Copy)]
pub(super) enum NoadKind {
    Ord,
    Op,
    Bin,
    Rel,
    Open,
    Close,
    Punct,
    Inner,
    Radical(u32),
    Over,
    Under,
    VCenter,
    Accent(u8, u32),
    Left(u32),
    Right(u32),
    Middle(u32),
}

pub(super) struct Noad<'a> {
    kind: NoadKind,
    /// `limits` (1) or `nolimits` (2) of an operator
    subtype: u8,
    nucleus: Field<'a>,
    sup: Field<'a>,
    sub: Field<'a>,
}

pub(super) struct FracItem<'a> {
    pub(super) thickness: i32,
    pub(super) left: u32,
    pub(super) right: u32,
    pub(super) num: Field<'a>,
    pub(super) den: Field<'a>,
}

/// tex.web `math_type` of a noad field.
pub(super) enum Field<'a> {
    Empty,
    Char(u8, u32),
    /// `sub_mlist` or `sub_box`; an empty list is a `sub_mlist` with a null
    /// `info`, displayed as `{}`
    List(Vec<Item<'a>>),
}

fn kind_of_class(class: u8) -> NoadKind {
    match class {
        1 => NoadKind::Op,
        2 => NoadKind::Bin,
        3 => NoadKind::Rel,
        4 => NoadKind::Open,
        5 => NoadKind::Close,
        6 => NoadKind::Punct,
        7 => NoadKind::Inner,
        _ => NoadKind::Ord,
    }
}

/// The 24-bit value tex.web `print_delimiter` shows.
fn delim24(small: (u8, u8), large: (u8, u8)) -> u32 {
    ((u32::from(small.0) << 8 | u32::from(small.1)) << 12)
        | (u32::from(large.0) << 8 | u32::from(large.1))
}

pub(super) fn code24(code: i32) -> u32 {
    let c = code as u32;
    delim24(
        (((c >> 20) & 0xF) as u8, ((c >> 12) & 0xFF) as u8),
        (((c >> 8) & 0xF) as u8, (c & 0xFF) as u8),
    )
}

pub(super) fn noad<'a>(kind: NoadKind, nucleus: Field<'a>, sup: Field<'a>, sub: Field<'a>) -> Item<'a> {
    noad_sub(kind, 0, nucleus, sup, sub)
}

fn noad_sub<'a>(
    kind: NoadKind,
    subtype: u8,
    nucleus: Field<'a>,
    sup: Field<'a>,
    sub: Field<'a>,
) -> Item<'a> {
    Item::Noad(Box::new(Noad {
        kind,
        subtype,
        nucleus,
        sup,
        sub,
    }))
}

/// An empty noad of math class `class` (what tex.web appends before it scans
/// the nucleus).
pub(super) fn empty_noad<'a>(class: u8) -> Item<'a> {
    noad(kind_of_class(class), Field::Empty, Field::Empty, Field::Empty)
}

pub(super) fn empty_noad_kind<'a>(kind: NoadKind) -> Item<'a> {
    noad(kind, Field::Empty, Field::Empty, Field::Empty)
}

/// tex.web `scripts_allowed`: the tail is a noad that can take scripts.
pub(super) fn scripts_allowed(tail: Option<&Item<'_>>) -> bool {
    match tail {
        Some(Item::Noad(n)) => !matches!(
            n.kind,
            NoadKind::Left(_) | NoadKind::Right(_) | NoadKind::Middle(_)
        ),
        Some(Item::Frac(_)) => true,
        _ => false,
    }
}

/// tex.web `math_limit_switch` on the tail noad `item`: `req` is the
/// `\nolimits` (0), `\limits` (1) or `\displaylimits` (2) request.
pub(super) fn set_op_subtype(item: &mut Item<'_>, req: u8) {
    if let Item::Noad(n) = item {
        if matches!(n.kind, NoadKind::Op) {
            n.subtype = match req {
                0 => 2,
                1 => 1,
                _ => 0,
            };
        }
    }
}

fn field_opt<'a>(list: Option<&'a Vec<Node>>) -> Field<'a> {
    match list {
        None => Field::Empty,
        Some(l) => field_of(l),
    }
}

/// The field a brace group or single token scanned into `list`
/// (tex.web §1186: a group holding one scriptless ord noad is that noad's
/// nucleus).
pub(super) fn field_of<'a>(list: &'a [Node]) -> Field<'a> {
    match list {
        [Node::MathChar {
            fam, c, class: 0, ..
        }] if *fam != 255 => Field::Char(*fam, *c),
        [Node::Scripts {
            nucleus,
            sup: None,
            sub: None,
        }] if matches!(
            nucleus.first(),
            Some(Node::MathChar {
                fam: 255,
                class: 0,
                ..
            })
        ) =>
        {
            Field::List(view_list(&nucleus[1..]))
        }
        [b @ Node::Box { .. }] => Field::List(vec![Item::Node(b)]),
        _ => Field::List(view_list(list)),
    }
}

/// The scripts of a `\right` marker, when `node` closes a `\left...\right`
/// group (the marker may be wrapped by the scripts that follow it).
fn lr_close<'a>(node: &'a Node) -> Option<(u32, Field<'a>, Field<'a>)> {
    let delim = |n: &Node| match n {
        Node::DelimBox {
            small, large, size: 1, ..
        } => Some(delim24(*small, *large)),
        _ => None,
    };
    match node {
        Node::DelimBox { .. } => delim(node).map(|d| (d, Field::Empty, Field::Empty)),
        Node::Scripts { nucleus, sup, sub } => match nucleus.as_slice() {
            [n] => delim(n).map(|d| (d, field_opt(sup.as_ref()), field_opt(sub.as_ref()))),
            _ => None,
        },
        Node::OpLimits { op, above, below } => match op.as_slice() {
            [n] => delim(n).map(|d| (d, field_opt(above.as_ref()), field_opt(below.as_ref()))),
            _ => None,
        },
        _ => None,
    }
}

/// The noad a single Ratex atom stands for, with the given scripts.
fn atom_noad<'a>(
    node: &'a Node,
    sup: Field<'a>,
    sub: Field<'a>,
) -> Result<Item<'a>, (Field<'a>, Field<'a>)> {
    Ok(match node {
        Node::MathChar {
            fam, c, class, ..
        } if *fam != 255 => noad(kind_of_class(*class), Field::Char(*fam, *c), sup, sub),
        Node::Box { .. } => noad(NoadKind::Ord, Field::List(vec![Item::Node(node)]), sup, sub),
        Node::VCenter { box_node } => noad(
            NoadKind::VCenter,
            Field::List(vec![Item::Node(box_node)]),
            sup,
            sub,
        ),
        Node::Accent { fam, c, body, .. } => noad(
            NoadKind::Accent(*fam, u32::from(*c)),
            field_of(body),
            sup,
            sub,
        ),
        Node::Radical {
            body, thickness, ..
        } => {
            let code = if *thickness <= -1 { -1 - *thickness } else { 0 };
            noad(NoadKind::Radical(code24(code)), field_of(body), sup, sub)
        }
        Node::Overline { body, under, .. } => noad(
            if *under {
                NoadKind::Under
            } else {
                NoadKind::Over
            },
            field_of(body),
            sup,
            sub,
        ),
        _ => return Err((sup, sub)),
    })
}

/// A `Scripts` or `OpLimits` node. `bare_op_subtype` is the subtype of an
/// operator character without a recorded one (a plain `Scripts` node is the
/// `nolimits` state, an `OpLimits` node the `normal` one).
fn scripts_items<'a>(
    nucleus: &'a [Node],
    sup: Field<'a>,
    sub: Field<'a>,
    bare_op_subtype: u8,
    out: &mut Vec<Item<'a>>,
) {
    if let Some((
        Node::MathChar {
            fam: 255,
            class,
            c,
            origin,
        },
        rest,
    )) = nucleus.split_first()
    {
        let subtype = if *class == CL_OP { *c as u8 } else { 0 };
        let field = match rest {
            [Node::MathChar {
                fam,
                c,
                class: CL_OP,
                ..
            }] if *class == CL_OP && *fam != 255 && origin.id != u64::MAX => {
                Field::Char(*fam, *c)
            }
            _ => field_of(rest),
        };
        out.push(noad_sub(kind_of_class(*class), subtype, field, sup, sub));
        return;
    }
    match nucleus {
        [] => out.push(noad(NoadKind::Ord, Field::Empty, sup, sub)),
        [Node::Choice, ..] => {
            out.extend(view_list(nucleus));
            out.push(noad(NoadKind::Ord, Field::Empty, sup, sub));
        }
        [Node::MathChar {
            fam,
            c,
            class: CL_OP,
            ..
        }] if *fam != 255 => out.push(noad_sub(
            NoadKind::Op,
            bare_op_subtype,
            Field::Char(*fam, *c),
            sup,
            sub,
        )),
        [atom] => match atom_noad(atom, sup, sub) {
            Ok(item) => out.push(item),
            Err((sup, sub)) => {
                view_node(atom, out);
                out.push(noad(NoadKind::Ord, Field::Empty, sup, sub));
            }
        },
        _ => out.push(noad(NoadKind::Ord, Field::List(view_list(nucleus)), sup, sub)),
    }
}

fn view_node<'a>(node: &'a Node, out: &mut Vec<Item<'a>>) {
    match node {
        Node::MathChar {
            fam: 255, class, ..
        } => out.push(noad(
            kind_of_class(*class),
            Field::List(Vec::new()),
            Field::Empty,
            Field::Empty,
        )),
        Node::Scripts { nucleus, sup, sub } => scripts_items(
            nucleus,
            field_opt(sup.as_ref()),
            field_opt(sub.as_ref()),
            2,
            out,
        ),
        Node::OpLimits { op, above, below } => scripts_items(
            op,
            field_opt(above.as_ref()),
            field_opt(below.as_ref()),
            0,
            out,
        ),
        Node::Frac {
            num,
            den,
            thickness,
            left,
            right,
            ..
        } => out.push(Item::Frac(Box::new(FracItem {
            thickness: *thickness,
            left: left.map_or(0, code24),
            right: right.map_or(0, code24),
            num: Field::List(view_list(num)),
            den: Field::List(view_list(den)),
        }))),
        _ => match atom_noad(node, Field::Empty, Field::Empty) {
            Ok(item) => out.push(item),
            Err(_) => out.push(Item::Node(node)),
        },
    }
}

/// tex.web's view of a Ratex math list (or an ordinary list: other nodes pass
/// through).
pub(super) fn view_list<'a>(list: &'a [Node]) -> Vec<Item<'a>> {
    let mut frames: Vec<Vec<Item<'a>>> = vec![Vec::new()];
    let mut i = 0;
    while i < list.len() {
        let node = &list[i];
        i += 1;
        if invisible(node) {
            continue;
        }
        match node {
            Node::Choice => {
                let mut parts: [Vec<Item<'a>>; 4] = Default::default();
                for part in parts.iter_mut() {
                    if let Some(Node::ChoiceAlt { body }) = list.get(i) {
                        *part = view_list(body);
                        i += 1;
                    } else {
                        break;
                    }
                }
                frames.last_mut().unwrap().push(Item::Choice(Box::new(parts)));
            }
            Node::ChoiceAlt { .. } => {}
            Node::DelimBox {
                small,
                large,
                size: 0,
                ..
            } => frames.push(vec![empty_noad_kind(NoadKind::Left(delim24(*small, *large)))]),
            Node::DelimBox {
                small,
                large,
                size: 3,
                ..
            } => frames.last_mut().unwrap().push(empty_noad_kind(NoadKind::Middle(delim24(
                *small, *large,
            )))),
            _ => {
                if let Some((d, sup, sub)) = lr_close(node) {
                    let right = empty_noad_kind(NoadKind::Right(d));
                    if frames.len() > 1 {
                        let mut inner = frames.pop().unwrap();
                        inner.push(right);
                        frames
                            .last_mut()
                            .unwrap()
                            .push(noad(NoadKind::Inner, Field::List(inner), sup, sub));
                    } else {
                        frames.last_mut().unwrap().push(right);
                    }
                } else {
                    view_node(node, frames.last_mut().unwrap());
                }
            }
        }
    }
    while frames.len() > 1 {
        let inner = frames.pop().unwrap();
        frames.last_mut().unwrap().extend(inner);
    }
    frames.pop().unwrap()
}

impl<'a> BoxDisplay<'a> {
    /// tex.web show_box over display items.
    pub(super) fn show_items_box(&mut self, items: &[Item<'_>]) {
        self.show_items(items);
        self.print_ln();
    }

    /// tex.web show_node_list over display items.
    pub(super) fn show_items(&mut self, items: &[Item<'_>]) {
        if self.prefix.len() as i64 > self.depth_threshold {
            if !items.is_empty() {
                self.print(" []");
            }
            return;
        }
        let mut n = 0i64;
        for item in items {
            self.print_ln();
            self.out.extend_from_slice(&self.prefix);
            n += 1;
            if n > self.breadth_max {
                self.print("etc.");
                return;
            }
            self.display_item(item);
        }
    }

    fn items_display(&mut self, items: &[Item<'_>]) {
        self.prefix.push(b'.');
        self.show_items(items);
        self.prefix.pop();
    }

    /// Show a node as the items it stands for (a math node reached through
    /// `display_node`).
    pub(super) fn display_node_as_items(&mut self, node: &Node) {
        let items = view_list(std::slice::from_ref(node));
        for (k, item) in items.iter().enumerate() {
            if k > 0 {
                self.print_ln();
                self.out.extend_from_slice(&self.prefix);
            }
            self.display_item(item);
        }
    }

    fn display_item(&mut self, item: &Item<'_>) {
        match item {
            Item::Node(n) => self.display_node(n),
            Item::Glue(g) => self.display_node(&Node::Glue(*g)),
            Item::Unset(u) => {
                self.print_esc("unsetbox(");
                self.print_scaled(u.h);
                self.out.push(b'+');
                self.print_scaled(u.d);
                self.print(")x");
                self.print_scaled(u.w);
                if u.span != 0 {
                    self.print(" (");
                    self.print_int(i64::from(u.span) + 1);
                    self.print(" columns)");
                }
                if let Some((v, order)) = u.stretch {
                    self.print(", stretch ");
                    self.print_glue(v, order, "");
                }
                if let Some((v, order)) = u.shrink {
                    self.print(", shrink ");
                    self.print_glue(v, order, "");
                }
                self.items_display(&u.list);
            }
            Item::Noad(n) => self.display_noad(n),
            Item::Frac(f) => {
                self.print_esc("fraction, thickness ");
                if f.thickness == DEFAULT_CODE {
                    self.print("= default");
                } else {
                    self.print_scaled(f.thickness);
                }
                if f.left != 0 {
                    self.print(", left-delimiter ");
                    self.print_delimiter(f.left);
                }
                if f.right != 0 {
                    self.print(", right-delimiter ");
                    self.print_delimiter(f.right);
                }
                self.subsidiary_field(&f.num, b'\\');
                self.subsidiary_field(&f.den, b'/');
            }
            Item::Choice(parts) => {
                self.print_esc("mathchoice");
                for (part, c) in parts.iter().zip([b'D', b'T', b'S', b's']) {
                    self.prefix.push(c);
                    self.show_items(part);
                    self.prefix.pop();
                }
            }
        }
    }

    fn print_delimiter(&mut self, a: u32) {
        self.print(&format!("\"{a:X}"));
    }

    fn display_noad(&mut self, n: &Noad<'_>) {
        match n.kind {
            NoadKind::Ord => self.print_esc("mathord"),
            NoadKind::Op => self.print_esc("mathop"),
            NoadKind::Bin => self.print_esc("mathbin"),
            NoadKind::Rel => self.print_esc("mathrel"),
            NoadKind::Open => self.print_esc("mathopen"),
            NoadKind::Close => self.print_esc("mathclose"),
            NoadKind::Punct => self.print_esc("mathpunct"),
            NoadKind::Inner => self.print_esc("mathinner"),
            NoadKind::Over => self.print_esc("overline"),
            NoadKind::Under => self.print_esc("underline"),
            NoadKind::VCenter => self.print_esc("vcenter"),
            NoadKind::Radical(d) => {
                self.print_esc("radical");
                self.print_delimiter(d);
            }
            NoadKind::Accent(fam, c) => {
                self.print_esc("accent");
                self.print_fam_and_char(fam, c);
            }
            NoadKind::Left(d) => {
                self.print_esc("left");
                self.print_delimiter(d);
            }
            NoadKind::Right(d) => {
                self.print_esc("right");
                self.print_delimiter(d);
            }
            NoadKind::Middle(d) => {
                self.print_esc("middle");
                self.print_delimiter(d);
            }
        }
        if !matches!(
            n.kind,
            NoadKind::Left(_) | NoadKind::Right(_) | NoadKind::Middle(_)
        ) {
            if n.subtype != 0 {
                self.print_esc(if n.subtype == 1 { "limits" } else { "nolimits" });
            }
            self.subsidiary_field(&n.nucleus, b'.');
        }
        self.subsidiary_field(&n.sup, b'^');
        self.subsidiary_field(&n.sub, b'_');
    }

    /// tex.web print_subsidiary_data.
    fn subsidiary_field(&mut self, field: &Field<'_>, c: u8) {
        if self.prefix.len() as i64 >= self.depth_threshold {
            if !matches!(field, Field::Empty) {
                self.print(" []");
            }
            return;
        }
        self.prefix.push(c);
        match field {
            Field::Empty => {}
            Field::Char(fam, ch) => {
                self.print_ln();
                self.out.extend_from_slice(&self.prefix);
                self.print_fam_and_char(*fam, *ch);
            }
            Field::List(items) if items.is_empty() => {
                self.print_ln();
                self.out.extend_from_slice(&self.prefix);
                self.print("{}");
            }
            Field::List(items) => self.show_items(items),
        }
        self.prefix.pop();
    }
}
