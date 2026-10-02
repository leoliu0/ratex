//! Page level driver of the XeTeX output: page size and origin as
//! xdvipdfmx derives them from the `pdf:pagesize` special XeTeX writes at the
//! start of every page (xetex.web `ship_out`) and the `papersize`/`landscape`
//! specials, then the page itself.

use super::bp_to_sp;
use crate::boxes::{LeaderBody, Node, WhatIt, HBOX};
use crate::engine::Engine;
use crate::pdfout::PdfPage;
use crate::prim::DimParam;

/// dvipdfmx.cfg `p a4`: ISO A4 in bp.
const A4_WIDTH_BP: f64 = 595.2755905511812;
const A4_HEIGHT_BP: f64 = 841.8897637795276;

/// Page geometry in bp: size and the offsets of the DVI origin from the
/// upper left corner.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PageGeometry {
    pub width: f64,
    pub height: f64,
    pub x_offset: f64,
    pub y_offset: f64,
}

/// `print_scaled` followed by dvipdfmx's `pt` conversion.
fn pt_to_bp(sp: i32) -> f64 {
    let pt = (f64::from(sp) / 65536.0 * 1e5).round() / 1e5;
    pt * 72.0 / 72.27
}

/// The specials of a box tree in DVI order.
fn collect_specials<'n>(list: &'n [Node], out: &mut Vec<&'n str>) {
    for node in list {
        match node {
            Node::Whatsit(WhatIt::Special(s), _) => out.push(s.as_ref()),
            Node::Box { list, .. } => collect_specials(list, out),
            Node::Leaders { body: LeaderBody::Box(b), .. } => {
                if let Node::Box { list, .. } = &**b {
                    collect_specials(list, out);
                }
            }
            Node::Disc(d) => {
                collect_specials(&d.pre_break, out);
                collect_specials(&d.post_break, out);
                collect_specials(&d.no_break, out);
            }
            _ => {}
        }
    }
}

/// One `scan_special` of dvi.c restricted to the page geometry.
fn scan_geometry(text: &str, g: &mut PageGeometry, landscape: &mut bool, paper: &mut (f64, f64)) {
    use super::dpx::{c_ident, read_length, skip_blank};
    use crate::dpx_obj::Parser;
    let mut p = Parser::new(text.as_bytes());
    skip_blank(&mut p);
    let mut q = c_ident(&mut p);
    let mut ns_pdf = false;
    if matches!(q.as_deref(), Some("pdf" | "x" | "dvipdfmx")) {
        skip_blank(&mut p);
        if p.peek() == Some(b':') {
            ns_pdf = q.as_deref() == Some("pdf");
            p.pos += 1;
            skip_blank(&mut p);
            q = c_ident(&mut p);
        }
    }
    skip_blank(&mut p);
    match q.as_deref() {
        Some("landscape") => *landscape = true,
        Some("pagesize") if ns_pdf => {
            while let Some(key) = c_ident(&mut p) {
                skip_blank(&mut p);
                match key.as_str() {
                    "width" => {
                        if let Ok(v) = read_length(&mut p) {
                            g.width = v;
                        }
                    }
                    "height" => {
                        if let Ok(v) = read_length(&mut p) {
                            g.height = v;
                        }
                    }
                    "xoffset" => {
                        if let Ok(v) = read_length(&mut p) {
                            g.x_offset = v;
                        }
                    }
                    "yoffset" => {
                        if let Ok(v) = read_length(&mut p) {
                            g.y_offset = v;
                        }
                    }
                    "default" => {
                        g.width = paper.0;
                        g.height = paper.1;
                        g.x_offset = 72.0;
                        g.y_offset = 72.0;
                    }
                    _ => break,
                }
                skip_blank(&mut p);
            }
        }
        Some("papersize") => {
            if p.peek() == Some(b'=') {
                p.pos += 1;
            }
            skip_blank(&mut p);
            let quote = matches!(p.peek(), Some(b'\'' | b'"')).then(|| p.s[p.pos]);
            if quote.is_some() {
                p.pos += 1;
                skip_blank(&mut p);
            }
            if let Ok(w) = read_length(&mut p) {
                skip_blank(&mut p);
                if p.peek() == Some(b',') {
                    p.pos += 1;
                    skip_blank(&mut p);
                }
                if let Ok(h) = read_length(&mut p) {
                    g.width = w;
                    g.height = h;
                    *paper = (w, h);
                }
            }
        }
        _ => {}
    }
}

impl Engine {
    /// The size and origin of the page being shipped (dvipdfmx.c
    /// `do_dvi_pages` + `dvi_scan_specials`).
    pub(crate) fn dpx_page_geometry(&mut self, page_box: &Node) -> PageGeometry {
        let dim = |e: &Self, p: DimParam| e.eqtb.dim_params[p.idx() as usize];
        let (pw, ph) = (dim(self, DimParam::PdfPageWidth), dim(self, DimParam::PdfPageHeight));
        let mut paper = self.dpx.paper.unwrap_or((A4_WIDTH_BP, A4_HEIGHT_BP));
        let mut g = PageGeometry { width: paper.0, height: paper.1, x_offset: 72.0, y_offset: 72.0 };
        let mut landscape = self.dpx.landscape;
        // xetex.web `ship_out`: the implicit pdf:pagesize special
        if pw > 0 && ph > 0 {
            g.width = pt_to_bp(pw);
            g.height = pt_to_bp(ph);
        }
        let mut specials = Vec::new();
        if let Node::Box { list, .. } = page_box {
            collect_specials(list, &mut specials);
        }
        for s in specials {
            let text = crate::tex_bytes::text_to_display(s);
            if text.contains("pagesize") || text.contains("papersize") || text.contains("landscape") {
                scan_geometry(&text, &mut g, &mut landscape, &mut paper);
            }
        }
        self.dpx.paper = Some(paper);
        if landscape != self.dpx.landscape {
            std::mem::swap(&mut g.width, &mut g.height);
            self.dpx.landscape = landscape;
        }
        g
    }

    /// `render_page` for XeTeX: the same box traversal as pdfTeX's, with the
    /// xdvipdfmx page geometry and special interpreter.
    pub(super) fn render_page_xetex(&mut self, page_box: &Node) -> PdfPage {
        self.init_pdf_output();
        let geo = self.dpx_page_geometry(page_box);
        let width_sp = bp_to_sp(geo.width);
        let height_sp = bp_to_sp(geo.height);
        if self.synctex_active() {
            let page = (self.pdf_doc.pages.len() + 1) as u32;
            self.synctex.record_page_size(page, i64::from(width_sp), i64::from(height_sp));
        }
        let mag = self.prepare_mag();
        self.pdf_page_group_val = 0;
        let h_offset = i64::from(self.eqtb.dim_params[DimParam::HOffset.idx() as usize]);
        let v_offset = i64::from(self.eqtb.dim_params[DimParam::VOffset.idx() as usize]);
        let x0 = h_offset;
        let y0 = v_offset;
        // positions are DVI coordinates: x to the right, y = -v (the page
        // origin is installed by the `cm` that starts the content)
        let mut ctx = self.new_ctx(0);
        ctx.dpx_begin_page(f64::from(mag) / 1000.0, geo.x_offset, geo.height - geo.y_offset);
        if let Node::Box { w, h, d, lr, .. } = page_box {
            (ctx.box_w_sp, ctx.box_h_sp, ctx.box_d_sp) = (i64::from(*w), i64::from(*h), i64::from(*d));
            ctx.box_lr = *lr;
        }
        ctx.ship_ref_sp = (x0, y0 + ctx.box_h_sp);
        if let Node::Box { list, kind, h, glue_sign, glue_order, glue_set, .. } = page_box {
            if *kind == HBOX {
                ctx.ship_hlist(list, x0, y0 + i64::from(*h), *glue_sign, *glue_order, *glue_set);
            } else {
                ctx.ship_vlist(list, x0, y0, *glue_sign, *glue_order, *glue_set);
            }
        }
        if ctx.lr_problems > 0 {
            let problems = std::mem::take(&mut ctx.lr_problems);
            ctx.eng.report_lr_problems(problems, None);
        }
        let group = ctx.eng.pdf_page_group_val;
        ctx.write_pending_images();
        let image_procset = ctx.image_procset();
        let resources = ctx.dpx_end_page(geo.width, geo.height);
        let mut resources_extra = Vec::new();
        for (cat, value) in resources {
            resources_extra.extend_from_slice(format!("/{cat} ").as_bytes());
            resources_extra.extend_from_slice(&value.to_bytes());
            resources_extra.push(b' ');
        }
        let media_box = format!(
            "/MediaBox [0 0 {} {}]",
            super::dpx::fmt_prec((geo.width / 0.01 + 0.5).floor() * 0.01, 2),
            super::dpx::fmt_prec((geo.height / 0.01 + 0.5).floor() * 0.01, 2)
        );
        PdfPage {
            content: {
                ctx.end_text();
                std::mem::take(&mut ctx.content).into_bytes()
            },
            width: geo.width.round() as i32,
            height: geo.height.round() as i32,
            width_sp: i64::from(width_sp),
            height_sp: i64::from(height_sp),
            annots: std::mem::take(&mut ctx.annots),
            annot_refs: Vec::new(),
            fonts: std::mem::take(&mut ctx.page_fonts),
            dests: Vec::new(),
            attr_extra: media_box.into_bytes(),
            resources_extra,
            display_list: Some(std::mem::take(&mut ctx.display_list)),
            procset: true,
            image_procset,
            xforms: std::mem::take(&mut ctx.xform_list),
            ximages: std::mem::take(&mut ctx.ximage_list),
            group,
            media_box: false,
        }
    }
}
