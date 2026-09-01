//! PDF page rendering: traverses box trees into content streams, tracks
//! used fonts, emits rules, raw literals, colors, leaders, link
//! annotations, named destinations, and \pdfsavepos position recording.

use crate::boxes::{leader_dims, LeaderBody, Node, NodeList, HBOX};
use crate::build::RULE_FILL;
use crate::engine::Engine;
use crate::pdfout::{Annot, PdfPage};
use crate::prim::DimParam;

/// TeX sp to PDF bp
#[inline]
pub fn sp_to_bp(sp: i64) -> f64 {
    sp as f64 * 72.0 / (72.27 * 65536.0)
}

/// PDF bp to TeX sp
#[inline]
pub fn bp_to_sp(bp: f64) -> i32 {
    (bp * 72.27 * 65536.0 / 72.0).round() as i32
}

/// An open \pdfstartlink .. \pdfendlink region accumulating its extent.
struct LinkFrame {
    uri: Option<String>,
    dest: Option<String>,
    attr: String,
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

pub struct RenderCtx<'a> {
    pub eng: &'a mut Engine,
    pub content: String,
    pub used_fonts: Vec<(u16, u16)>, // (engine font id, pdf font resource num)
    pub page_height_bp: f64,
    pub cur_font: u16,
    pub cur_pdf_font: u16,
    links: Vec<LinkFrame>,
    pub annots: Vec<Annot>,
    pub dests: Vec<crate::pdfout::Dest>,
    pub page_fonts: Vec<(u16, u16)>, // (engine font id, resource num)
    // containing-box context for leaders grids and null-rule sentinels (sp)
    pub left_edge_sp: i64,
    pub box_w_sp: i64,
    pub box_h_sp: i64,
    pub box_d_sp: i64,
}

impl Engine {
    /// Render a shipped page box into a PdfPage. Also copies the outline
    /// list into the document and records \pdfsavepos results
    /// (\pdflastxpos/\pdflastypos) from the last SavePos node on the page.
    pub fn render_page(&mut self, page_box: &Node) -> PdfPage {
        let width_sp = self.eqtb.dim_params[DimParam::PdfPageWidth.idx() as usize];
        let height_sp = self.eqtb.dim_params[DimParam::PdfPageHeight.idx() as usize];
        let w_bp = sp_to_bp(width_sp as i64);
        let h_bp = sp_to_bp(height_sp as i64);
        let mut ctx = RenderCtx {
            eng: self,
            content: String::new(),
            used_fonts: Vec::new(),
            page_height_bp: h_bp,
            cur_font: 0,
            cur_pdf_font: 0,
            links: Vec::new(),
            annots: Vec::new(),
            dests: Vec::new(),
            page_fonts: Vec::new(),
            left_edge_sp: 0,
            box_w_sp: width_sp as i64,
            box_h_sp: height_sp as i64,
            box_d_sp: 0,
        };
        let horigin_bp = sp_to_bp(ctx.eng.eqtb.dim_params[DimParam::PdfHOrigin.idx() as usize] as i64);
        let vorigin_bp = sp_to_bp(ctx.eng.eqtb.dim_params[DimParam::PdfVOrigin.idx() as usize] as i64);
        let x0 = horigin_bp;
        // TeX y grows down from the page top; emit_char applies y_pdf.
        let y0 = vorigin_bp;
        if let Node::Box { list, kind, glue_sign, glue_order, glue_set, w, h, d, .. } = page_box {
            ctx.left_edge_sp = bp_to_sp(x0) as i64;
            (ctx.box_w_sp, ctx.box_h_sp, ctx.box_d_sp) = (*w as i64, *h as i64, *d as i64);
            if *kind == HBOX {
                // shipped hbox: baseline sits at the top-left origin
                ctx.ship_hlist(list, x0, y0, *glue_sign, *glue_order, *glue_set);
            } else {
                // vbox/vtop: the top of the page material sits at the origin
                ctx.ship_vlist(list, x0, y0, *glue_sign, *glue_order, *glue_set);
            }
        }
        // close any links left open at the end of the page
        while let Some(fr) = ctx.links.pop() {
            ctx.close_link(fr);
        }
        // engine-level results
        ctx.eng.pdf_doc.outlines = ctx.eng.pdf_outlines.clone();
        ctx.eng.pdf_doc.pages_attr = ctx.eng.pdf_pages_attr.clone().into_bytes();
        PdfPage {
            content: std::mem::take(&mut ctx.content).into_bytes(),
            width: w_bp as i32,
            height: h_bp as i32,
            annots: std::mem::take(&mut ctx.annots),
            fonts: std::mem::take(&mut ctx.page_fonts)
                .into_iter()
                .map(|(id, num)| (id as usize, num))
                .collect(),
            dests: std::mem::take(&mut ctx.dests),
            attr_extra: ctx.eng.pdf_page_attr.as_bytes().to_vec(),
        }
    }
}

/// Glue advance in bp under the box's glue setting.
fn glue_advance(width: i32, stretch: i32, shrink: i32, stretch_order: u8, shrink_order: u8, sign: u8, order: u8, set: f64) -> f64 {
    match sign {
        1 => {
            let w = sp_to_bp(width as i64);
            if stretch_order == order {
                w + set * sp_to_bp(stretch as i64)
            } else {
                w // lower-order stretch does not participate
            }
        }
        2 => {
            let w = sp_to_bp(width as i64);
            if shrink_order == order {
                (w - set * sp_to_bp(shrink as i64)).max(0.0)
            } else if shrink_order < order {
                w
            } else {
                0.0
            }
        }
        _ => sp_to_bp(width as i64),
    }
}

impl<'a> RenderCtx<'a> {
    fn y_pdf(&self, tex_y_bp: f64) -> f64 {
        self.page_height_bp - tex_y_bp
    }

    /// grow every open link region to include the point/extent at (x, y)
    fn note_point(&mut self, x: f64, y: f64) {
        for fr in self.links.iter_mut() {
            if x < fr.min_x {
                fr.min_x = x;
            }
            if x > fr.max_x {
                fr.max_x = x;
            }
            if y < fr.min_y {
                fr.min_y = y;
            }
            if y > fr.max_y {
                fr.max_y = y;
            }
        }
    }

    fn close_link(&mut self, fr: LinkFrame) {
        // PDF rect: y grows upward; tex y grows upward within the page
        let y0 = self.y_pdf(fr.max_y);
        let y1 = self.y_pdf(fr.min_y);
        self.annots.push(Annot {
            rect: [fr.min_x, y0, fr.max_x, y1],
            uri: fr.uri,
            dest: fr.dest,
            attr: fr.attr,
            subtype: Some("/Link".to_string()),
        });
    }

    /// ship a vertical list with its top edge at y
    pub fn ship_vlist(&mut self, list: &NodeList, x: f64, y: f64, sign: u8, order: u8, set: f64) {
        let mut cur_y = y;
        for n in list {
            match n {
                Node::Box { h, d, w, shift, glue_sign, glue_order, glue_set, list: inner, kind, .. } => {
                    let (bh, bd, sh) =
                        (sp_to_bp(*h as i64), sp_to_bp(*d as i64), sp_to_bp(*shift as i64));
                    // thread containing-box context for the inner list
                    let saved = (self.left_edge_sp, self.box_w_sp, self.box_h_sp, self.box_d_sp);
                    self.left_edge_sp = bp_to_sp(x + sh) as i64;
                    (self.box_w_sp, self.box_h_sp, self.box_d_sp) = (*w as i64, *h as i64, *d as i64);
                    if *kind == HBOX {
                        // hbox: the vertical shift moves the baseline down
                        let baseline = cur_y + bh + sh;
                        self.ship_hlist(inner, x, baseline, *glue_sign, *glue_order, *glue_set);
                    } else {
                        // vbox/vtop: the shift is horizontal
                        self.ship_vlist(inner, x + sh, cur_y, *glue_sign, *glue_order, *glue_set);
                    }
                    self.left_edge_sp = saved.0;
                    self.box_w_sp = saved.1;
                    self.box_h_sp = saved.2;
                    self.box_d_sp = saved.3;
                    cur_y += bh + bd;
                }
                Node::Rule { width, height, depth } => {
                    // hrule in a vlist: null width fills the containing box
                    let w_sp = if *width == RULE_FILL { self.box_w_sp } else { *width as i64 };
                    let (rw, rh, rd) =
                        (sp_to_bp(w_sp), sp_to_bp(*height as i64), sp_to_bp(*depth as i64));
                    let y1 = cur_y + rh; // top of rule
                    self.emit_rect(x, self.y_pdf(y1 + rd), rw, rh + rd);
                    cur_y += rh + rd;
                }
                Node::Glue(g) => {
                    cur_y += glue_advance(g.width, g.stretch, g.shrink, g.stretch_order, g.shrink_order, sign, order, set);
                }
                Node::Leaders { glue, kind, body } => {
                    let adv = glue_advance(glue.width, glue.stretch, glue.shrink, glue.stretch_order, glue.shrink_order, sign, order, set);
                    let (lw, lh, ld) = leader_dims(body);
                    match body {
                        // rule body: one rect spanning the whole advance;
                        // null width fills the containing box
                        LeaderBody::Rule { width, .. } => {
                            let w_sp = if *width == RULE_FILL { self.box_w_sp } else { *width as i64 };
                            let rw = sp_to_bp(w_sp);
                            if rw > 0.0 && adv > 0.0 {
                                self.emit_rect(x, self.y_pdf(cur_y + adv), rw, adv);
                            }
                        }
                        LeaderBody::Box(b) => {
                            // leader_wd = height + depth of the body box
                            let (positions, _, _) = crate::boxes::leader_layout(
                                *kind,
                                (lh + ld) as i64,
                                bp_to_sp(adv) as i64,
                                self.left_edge_sp,
                                bp_to_sp(cur_y) as i64,
                            );
                            for pos in positions {
                                self.ship_leader_copy(b, x, sp_to_bp(pos), true);
                            }
                        }
                    }
                    let _ = lw;
                    cur_y += adv;
                }
                Node::Kern(k) | Node::ExplicitKern(k) => cur_y += sp_to_bp(*k as i64),
                Node::Penalty(_) | Node::Mark { .. } => {}
                Node::Whatsit(w) => {
                    self.note_point(x, cur_y);
                    self.emit_whatsit(w, x, cur_y);
                }
                Node::Ins { box_node, .. } => {
                    if let Node::Box { list: inner, .. } = &**box_node {
                        self.ship_vlist(inner, x, cur_y, 0, 0, 0.0);
                    }
                }
                Node::VAdjust(items) => {
                    self.ship_vlist(items, x, cur_y, 0, 0, 0.0);
                }
                _ => {}
            }
        }
    }

    /// ship a horizontal list with baseline at y
    pub fn ship_hlist(&mut self, list: &NodeList, x: f64, y: f64, sign: u8, order: u8, set: f64) {
        let mut cur_x = x;
        for n in list {
            match n {
                Node::Char { c, font } => {
                    let adv = sp_to_bp(self.font_char_width(*font, *c) as i64);
                    self.emit_char(*font, *c, cur_x, y);
                    cur_x += adv;
                }
                Node::Ligature { c, font, lig_width, .. } => {
                    self.emit_char(*font, *c, cur_x, y);
                    cur_x += sp_to_bp(*lig_width as i64);
                }
                Node::Glue(g) => {
                    cur_x += glue_advance(g.width, g.stretch, g.shrink, g.stretch_order, g.shrink_order, sign, order, set);
                }
                Node::Kern(k) | Node::ExplicitKern(k) => cur_x += sp_to_bp(*k as i64),
                Node::Penalty(_) => {}
                Node::Rule { width, height, depth } => {
                    // vrule in an hlist: null height/depth fill the containing box
                    let h_sp = if *height == RULE_FILL { self.box_h_sp } else { *height as i64 };
                    let d_sp = if *depth == RULE_FILL { self.box_d_sp } else { *depth as i64 };
                    let (rw, rh, rd) = (sp_to_bp(*width as i64), sp_to_bp(h_sp), sp_to_bp(d_sp));
                    self.emit_rect(cur_x, self.y_pdf(y + rh), rw, rh + rd);
                    cur_x += rw;
                }
                Node::Box { w, h, d, shift, glue_sign, glue_order, glue_set, list: inner, kind, .. } => {
                    let (bw, bh, sh) =
                        (sp_to_bp(*w as i64), sp_to_bp(*h as i64), sp_to_bp(*shift as i64));
                    // thread containing-box context for the inner list
                    let saved = (self.left_edge_sp, self.box_w_sp, self.box_h_sp, self.box_d_sp);
                    self.left_edge_sp = bp_to_sp(if *kind == HBOX { cur_x } else { cur_x + sh }) as i64;
                    (self.box_w_sp, self.box_h_sp, self.box_d_sp) = (*w as i64, *h as i64, *d as i64);
                    if *kind == HBOX {
                        // hbox: the shift is vertical (baseline moves down)
                        let baseline = y + sh;
                        self.ship_hlist(inner, cur_x, baseline, *glue_sign, *glue_order, *glue_set);
                    } else {
                        // vbox/vtop: the shift is horizontal; baseline bh below top
                        self.ship_vlist(inner, cur_x + sh, y - bh, *glue_sign, *glue_order, *glue_set);
                    }
                    self.left_edge_sp = saved.0;
                    self.box_w_sp = saved.1;
                    self.box_h_sp = saved.2;
                    self.box_d_sp = saved.3;
                    cur_x += bw;
                    let _ = (bh, d);
                }
                Node::Disc(dc) => {
                    for nn in &dc.no_break {
                        match nn {
                            Node::Char { c, font } => {
                                let adv = sp_to_bp(self.font_char_width(*font, *c) as i64);
                                self.emit_char(*font, *c, cur_x, y);
                                cur_x += adv;
                            }
                            other => {
                                let single: NodeList = vec![other.clone()];
                                self.ship_hlist(&single, cur_x, y, sign, order, set);
                            }
                        }
                    }
                }
                Node::Leaders { glue, kind, body } => {
                    let adv = glue_advance(glue.width, glue.stretch, glue.shrink, glue.stretch_order, glue.shrink_order, sign, order, set);
                    let (lw, lh, ld) = leader_dims(body);
                    match body {
                        // rule body: one rect over the whole advance; null
                        // height/depth fill the containing box
                        LeaderBody::Rule { height, depth, .. } => {
                            let h_sp = if *height == RULE_FILL { self.box_h_sp } else { *height as i64 };
                            let d_sp = if *depth == RULE_FILL { self.box_d_sp } else { *depth as i64 };
                            let (rh, rd) = (sp_to_bp(h_sp), sp_to_bp(d_sp));
                            if adv > 0.0 && rh + rd > 0.0 {
                                self.emit_rect(cur_x, self.y_pdf(y + rh), adv, rh + rd);
                            }
                        }
                        LeaderBody::Box(b) => {
                            let (positions, _, _) = crate::boxes::leader_layout(
                                *kind,
                                lw as i64,
                                bp_to_sp(adv) as i64,
                                self.left_edge_sp,
                                bp_to_sp(cur_x) as i64,
                            );
                            for pos in positions {
                                self.ship_leader_copy(b, sp_to_bp(pos), y, false);
                            }
                        }
                    }
                    let _ = (lh, ld);
                    cur_x += adv;
                }
                Node::Whatsit(w) => {
                    self.note_point(cur_x, y);
                    self.emit_whatsit(w, cur_x, y);
                }
                Node::Mark { .. } | Node::Ins { .. } => {}
                _ => {}
            }
        }
    }

    /// ship one leader body copy. Horizontal (`vertical == false`): the
    /// copy's baseline sits at `at`. Vertical: the copy's top edge sits
    /// at `at` (a vlist item position).
    fn ship_leader_copy(&mut self, b: &Node, x: f64, at: f64, vertical: bool) {
        let Node::Box { w, h, d, shift, glue_sign, glue_order, glue_set, list: inner, kind, .. } = b
        else {
            return;
        };
        let bh = sp_to_bp(*h as i64);
        let sh = sp_to_bp(*shift as i64);
        let saved = (self.left_edge_sp, self.box_w_sp, self.box_h_sp, self.box_d_sp);
        self.left_edge_sp = bp_to_sp(x) as i64;
        (self.box_w_sp, self.box_h_sp, self.box_d_sp) = (*w as i64, *h as i64, *d as i64);
        if vertical {
            if *kind == HBOX {
                // hbox as a vlist item: top edge at `at`, baseline below
                self.ship_hlist(inner, x, at + bh + sh, *glue_sign, *glue_order, *glue_set);
            } else {
                self.ship_vlist(inner, x + sh, at, *glue_sign, *glue_order, *glue_set);
            }
        } else if *kind == HBOX {
            self.ship_hlist(inner, x, at + sh, *glue_sign, *glue_order, *glue_set);
        } else {
            self.ship_vlist(inner, x + sh, at - bh, *glue_sign, *glue_order, *glue_set);
        }
        self.left_edge_sp = saved.0;
        self.box_w_sp = saved.1;
        self.box_h_sp = saved.2;
        self.box_d_sp = saved.3;
    }

    fn font_char_width(&self, f: u16, c: u8) -> i32 {
        self.eng.eqtb.fonts.get(f as usize).map(|ff| ff.char_width(c)).unwrap_or(0)
    }

    fn ensure_font(&mut self, f: u16) -> u16 {
        if self.cur_font == f && self.cur_pdf_font != 0 {
            return self.cur_pdf_font;
        }
        if let Some((_, num)) = self.used_fonts.iter().find(|(id, _)| *id == f) {
            self.cur_font = f;
            self.cur_pdf_font = *num;
            return *num;
        }
        let num = (self.used_fonts.len() + 1) as u16;
        self.used_fonts.push((f, num));
        self.page_fonts.push((f, num));
        self.cur_font = f;
        self.cur_pdf_font = num;
        num
    }

    fn emit_char(&mut self, f: u16, c: u8, x: f64, y: f64) {
        let size_bp = self
            .eng
            .eqtb
            .fonts
            .get(f as usize)
            .map(|ff| sp_to_bp(ff.at_size as i64))
            .unwrap_or(0.0);
        // approximate glyph extent feeds link rectangles
        self.note_point(x, y + 0.75 * size_bp);
        self.note_point(x + 0.5 * size_bp, y - 0.25 * size_bp);
        if size_bp <= 0.0 {
            return; // nullfont: nothing to draw
        }
        // Virtual font: expand the glyph into its mapped steps in the base
        // fonts (kerns included as offsets). The VF font itself is never
        // registered as a page resource.
        if let Some(bases) = self.eng.font_loader.vf_bases.get(&f).cloned() {
            let key = self
                .eng
                .eqtb
                .fonts
                .get(f as usize)
                .map(|ff| (ff.tfm_name.clone(), ff.at_size));
            let steps = key
                .and_then(|k| self.eng.font_loader.vf_fonts.get(&k).cloned())
                .and_then(|vf| vf.chars.get(c as usize).cloned().flatten());
            if let Some(steps) = steps {
                for st in steps.iter() {
                    let Some(&bfid) = bases.get(st.base as usize) else { continue };
                    if bfid == u16::MAX {
                        continue; // base TFM missing at load time
                    }
                    let bnum = self.ensure_font(bfid);
                    let bsize_bp = self
                        .eng
                        .eqtb
                        .fonts
                        .get(bfid as usize)
                        .map(|ff| sp_to_bp(ff.at_size as i64))
                        .unwrap_or(0.0);
                    self.content.push_str(&format!(
                        "BT /F{} {:.4} Tf 1 0 0 1 {:.4} {:.4} Tm <{:02x}> Tj ET\n",
                        bnum,
                        bsize_bp,
                        x + sp_to_bp(st.dx as i64),
                        self.y_pdf(y - sp_to_bp(st.dy as i64)),
                        st.ch
                    ));
                }
            }
            return; // VF font without a packet for this char: nothing to draw
        }
        let num = self.ensure_font(f);
        self.content.push_str(&format!(
            "BT /F{} {:.4} Tf 1 0 0 1 {:.4} {:.4} Tm <{:02x}> Tj ET\n",
            num,
            size_bp,
            x,
            self.y_pdf(y),
            c
        ));
    }

    fn emit_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let (w, h) = (w.max(0.0), h.max(0.0));
        if w == 0.0 || h == 0.0 {
            return;
        }
        self.note_point(x, y);
        self.note_point(x + w, y + h);
        self.content
            .push_str(&format!("{:.4} {:.4} {:.4} {:.4} re f\n", x, y, w, h));
    }

    fn emit_whatsit(&mut self, w: &crate::boxes::WhatIt, x: f64, y: f64) {
        use crate::boxes::WhatIt::*;
        match w {
            PdfLiteral { data, .. } => {
                self.content.push_str(data);
                self.content.push('\n');
            }
            PdfColorPush(color) => {
                self.content.push_str("q\n");
                self.content.push_str(color);
                self.content.push('\n');
            }
            PdfColorPop => {
                self.content.push_str("Q\n");
            }
            PdfSave => self.content.push_str("q\n"),
            PdfRestore => self.content.push_str("Q\n"),
            PdfDest { name, kind, params } => {
                // first definition of a name wins
                if !self.dests.iter().any(|d| &d.name == name) {
                    // explicit coordinates are page-absolute sp from the
                    // bottom-left corner; the sentinel -32768 keeps the
                    // anchor position
                    let pv = |i: usize, anchor: f64| {
                        if params[i] == crate::pdfout::PDF_POS_CURRENT {
                            anchor
                        } else {
                            sp_to_bp(params[i] as i64)
                        }
                    };
                    // anchor position: x from the pen, y from the baseline
                    let ax = x;
                    let ay = self.y_pdf(y);
                    // XYZ: left top zoom; FitH/FitBH: top; FitV/FitBV: left;
                    // FitR: left bottom right top
                    let (px, py) = match kind {
                        2 | 5 => (ax, pv(0, ay)),
                        3 | 6 => (pv(0, ax), ay),
                        7 => (pv(0, ax), pv(3, ay)),
                        _ => (pv(0, ax), pv(1, ay)),
                    };
                    let zm = if *kind == 0 && params[2] > 0 {
                        Some(params[2] as f64 / 1000.0)
                    } else {
                        None
                    };
                    self.dests.push(crate::pdfout::Dest {
                        name: name.clone(),
                        x: px,
                        y: py,
                        kind: *kind,
                        zoom: zm,
                    });
                }
            }
            PdfAnnot { attr, wd, ht, dp } => {
                let x1 = x + sp_to_bp(*wd as i64);
                let y0 = self.y_pdf(y + sp_to_bp(*ht as i64));
                let y1 = self.y_pdf(y - sp_to_bp(*dp as i64));
                self.annots.push(Annot {
                    rect: [x, y0, x1, y1],
                    uri: None,
                    dest: None,
                    attr: attr.clone(),
                    subtype: None,
                });
            }
            PdfStartLink { attr, uri, name } => {
                self.links.push(LinkFrame {
                    uri: uri.clone(),
                    dest: name.clone(),
                    attr: attr.clone(),
                    min_x: x,
                    min_y: y,
                    max_x: x,
                    max_y: y,
                });
            }
            PdfEndLink => {
                if let Some(fr) = self.links.pop() {
                    // include the pen position at closing time
                    let mut fr = fr;
                    if x < fr.min_x {
                        fr.min_x = x;
                    }
                    if x > fr.max_x {
                        fr.max_x = x;
                    }
                    self.close_link(fr);
                }
            }
            Special(_) => {
                // DVI \special has no direct PDF meaning; ignored
            }
            SavePos { .. } => {
                // position is relative to the page edges, in sp
                self.eng.pdf_last_x = bp_to_sp(x);
                self.eng.pdf_last_y = bp_to_sp(self.y_pdf(y));
            }
            _ => {}
        }
    }
}
