//! Display items: the node and noad shapes of tex.web §184 (unset nodes) and
//! §690-§698 (noads) that Ratex keeps in other representations. The views
//! below rebuild tex.web's shapes from Ratex's math lists (flat
//! `\left...\right` markers, fam-255 group markers, scripts wrapping their
//! nucleus) so that `\showlists` prints exactly what tex.web prints.

use super::{invisible, BoxDisplay, DEFAULT_CODE};
use crate::boxes::{noad_option, AccentSpec, Delim, Glue, Node};

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
    Radical {
        subtype: u8,
        delim: Delim,
        width: i32,
        options: u16,
    },
    Over,
    Under,
    VCenter,
    Accent(AccentSpec),
    Left(Delim),
    Right(Delim),
    Middle(Delim),
}

pub(super) struct Noad<'a> {
    kind: NoadKind,
    /// `limits` (1) or `nolimits` (2) of an operator
    subtype: u8,
    nucleus: Field<'a>,
    sup: Field<'a>,
    sub: Field<'a>,
    /// the root degree of a `\Uroot` radical
    degree: Field<'a>,
}

pub(super) struct FracItem<'a> {
    pub(super) thickness: i32,
    pub(super) left: Delim,
    pub(super) right: Delim,
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

/// The delimiter of a `DelimBox` marker.
pub(super) fn delim_of(small: (u8, u32), large: (u8, u32)) -> Delim {
    Delim {
        small_fam: small.0,
        small_char: small.1,
        large_fam: large.0,
        large_char: large.1,
    }
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
        degree: Field::Empty,
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

/// The noad field luatex is scanning into: it hands `scan_math` a fresh
/// `math_char_node` (family 0, character 0), which `\showlists` prints as
/// `\fam0 ` while the field is in progress.
#[derive(Clone, Copy)]
pub(super) enum PendingField {
    Nucleus,
    Sup,
    Sub,
    Degree,
}

pub(super) fn set_pending_field(item: &mut Item<'_>, which: PendingField) {
    set_field(item, which, Field::Char(0, 0));
}

pub(super) fn set_field<'a>(item: &mut Item<'a>, which: PendingField, field: Field<'a>) {
    if let Item::Noad(n) = item {
        let slot = match which {
            PendingField::Nucleus => &mut n.nucleus,
            PendingField::Sup => &mut n.sup,
            PendingField::Sub => &mut n.sub,
            PendingField::Degree => &mut n.degree,
        };
        *slot = field;
    }
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
            ..
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
fn lr_close<'a>(node: &'a Node) -> Option<(Delim, Field<'a>, Field<'a>)> {
    let delim = |n: &Node| match n {
        Node::DelimBox {
            small, large, size: 1, ..
        } => Some(delim_of(*small, *large)),
        _ => None,
    };
    match node {
        Node::DelimBox { .. } => delim(node).map(|d| (d, Field::Empty, Field::Empty)),
        Node::Scripts { nucleus, sup, sub, .. } => match nucleus.as_slice() {
            [n] => delim(n).map(|d| (d, field_opt(sup.as_ref()), field_opt(sub.as_ref()))),
            _ => None,
        },
        Node::OpLimits { op, above, below, .. } => match op.as_slice() {
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
        Node::Accent { spec, body, .. } => noad(NoadKind::Accent(*spec), field_of(body), sup, sub),
        Node::Radical {
            body,
            delim,
            subtype,
            width,
            options,
            degree,
            ..
        } => {
            let kind = NoadKind::Radical {
                subtype: *subtype,
                delim: *delim,
                width: *width,
                options: *options,
            };
            // `\Uhextensible`'s nucleus is an empty `sub_box`: nothing shows
            let nucleus = if *subtype == 7 { Field::Empty } else { field_of(body) };
            let degree = degree.as_ref().map_or(Field::Empty, |d| field_of(d));
            Item::Noad(Box::new(Noad {
                kind,
                subtype: 0,
                nucleus,
                sup,
                sub,
                degree,
            }))
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
            origin, .. },
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
        Node::Scripts { nucleus, sup, sub, .. } => scripts_items(
            nucleus,
            field_opt(sup.as_ref()),
            field_opt(sub.as_ref()),
            2,
            out,
        ),
        Node::OpLimits { op, above, below, .. } => scripts_items(
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
            left: *left,
            right: *right,
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
                    if let Some(Node::ChoiceAlt { body, .. }) = list.get(i) {
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
            } => frames.push(vec![empty_noad_kind(NoadKind::Left(delim_of(*small, *large)))]),
            Node::DelimBox {
                small,
                large,
                size: 3 | 4,
                ..
            } => frames.last_mut().unwrap().push(empty_noad_kind(NoadKind::Middle(delim_of(
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
            Item::Glue(g) => self.display_node(&Node::Glue(*g, crate::boxes::Attr::NONE)),
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
                if !f.left.is_null() {
                    self.print(", left-delimiter ");
                    self.print_delimiter(&f.left);
                }
                if !f.right.is_null() {
                    self.print(", right-delimiter ");
                    self.print_delimiter(&f.right);
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

    /// texmath.c `print_delimiter` (without the `\Uleft` options).
    fn print_delimiter(&mut self, d: &Delim) {
        if d.small_fam < 16 && d.large_fam < 16 && d.small_char < 256 && d.large_char < 256 {
            // traditional tex style
            let a = ((u32::from(d.small_fam) * 256 + d.small_char) << 12)
                + u32::from(d.large_fam) * 256
                + d.large_char;
            self.print(&format!("\"{a:X}"));
        } else if (d.large_fam == 0 && d.large_char == 0) || d.small_char > 65535 || d.large_char > 65535 {
            // luatex style
            self.print(&format!("\"{:X}\"{:X}", d.small_fam, d.small_char));
        }
    }

    /// texmath.c `display_normal_noad`, accent noads; the traditional
    /// engines print tex.web's `\accent<fam><char>`.
    fn display_accent(&mut self, spec: &AccentSpec) {
        if self.e.engine_kind != crate::engine::EngineKind::LuaTeX {
            self.print_esc("accent");
            if let Some((fam, c)) = spec.top {
                self.print_fam_and_char(fam, c);
            }
            return;
        }
        let (top, bottom, overlay) = (spec.top, spec.bottom, spec.overlay);
        self.print_esc(match (top.is_some(), bottom.is_some()) {
            (true, true) => "Umathaccent both",
            (true, false) => "Umathaccent",
            (false, true) => "Umathaccent bottom",
            (false, false) => "Umathaccent overlay",
        });
        if spec.fraction != 0 {
            self.print(" fraction=");
            self.print_int(i64::from(spec.fraction));
            self.print(" ");
        }
        let fc = |d: &mut Self, a: Option<(u8, u32)>| {
            let (fam, c) = a.unwrap_or((0, 0));
            d.print_fam_and_char(fam, c);
        };
        match spec.subtype {
            0 => {
                if top.is_some() {
                    fc(self, top);
                    if bottom.is_some() {
                        fc(self, bottom);
                    }
                } else if bottom.is_some() {
                    fc(self, bottom);
                } else {
                    fc(self, overlay);
                }
            }
            1 => {
                self.print(" fixed ");
                fc(self, top);
                if bottom.is_some() {
                    fc(self, bottom);
                }
            }
            2 => {
                if top.is_some() {
                    fc(self, top);
                }
                self.print(" fixed ");
                fc(self, bottom);
            }
            _ => {
                self.print(" fixed ");
                fc(self, top);
                self.print(" fixed ");
                fc(self, bottom);
            }
        }
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
            NoadKind::Radical {
                subtype,
                delim,
                width,
                options,
            } => {
                self.print_esc(match subtype {
                    7 => "Uhextensible",
                    6 => "Udelimiterover",
                    5 => "Udelimiterunder",
                    4 => "Uoverdelimiter",
                    3 => "Uunderdelimiter",
                    2 => "Uroot",
                    _ => "radical",
                });
                self.print_delimiter(&delim);
                // `degree(p) != null` only for \Uroot
                if subtype == 2 {
                    self.subsidiary_field(&n.degree, b'/');
                }
                if width != 0 {
                    self.print("width=");
                    self.print_scaled(width);
                    self.print(" ");
                }
                if options & noad_option::SET == noad_option::SET {
                    self.print(" [ ");
                    if noad_option::has(options, noad_option::EXACT) {
                        self.print("exact ");
                    }
                    if noad_option::has(options, noad_option::LEFT) {
                        self.print("left ");
                    }
                    if noad_option::has(options, noad_option::MIDDLE) {
                        self.print("middle ");
                    }
                    if noad_option::has(options, noad_option::RIGHT) {
                        self.print("right ");
                    }
                    self.print("]");
                }
            }
            NoadKind::Accent(spec) => self.display_accent(&spec),
            NoadKind::Left(d) => {
                self.print_esc("left");
                self.print_delimiter(&d);
            }
            NoadKind::Right(d) => {
                self.print_esc("right");
                self.print_delimiter(&d);
            }
            NoadKind::Middle(d) => {
                self.print_esc("middle");
                self.print_delimiter(&d);
            }
        }
        if !matches!(
            n.kind,
            NoadKind::Left(_) | NoadKind::Right(_) | NoadKind::Middle(_)
        ) {
            if n.subtype != 0 {
                self.print_esc(if n.subtype == 1 { "limits" } else { "nolimits" });
            }
            // luatex's `sub_sup` gives a nucleus-less noad an empty
            // `sub_mlist` nucleus (`{}`); tex.web leaves it empty
            let simple = matches!(
                n.kind,
                NoadKind::Ord
                    | NoadKind::Op
                    | NoadKind::Bin
                    | NoadKind::Rel
                    | NoadKind::Open
                    | NoadKind::Close
                    | NoadKind::Punct
                    | NoadKind::Inner
            );
            if simple && matches!(n.nucleus, Field::Empty) && self.e.engine_kind == crate::engine::EngineKind::LuaTeX {
                self.subsidiary_field(&Field::List(Vec::new()), b'.');
            } else {
                self.subsidiary_field(&n.nucleus, b'.');
            }
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
