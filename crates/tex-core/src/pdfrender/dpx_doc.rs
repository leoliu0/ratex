//! `pdf:` specials of xdvipdfmx (spc_pdfm.c) that build document structure:
//! annotations with line breaking, destinations, outlines, named objects,
//! streams, form XObjects, images, document information and view settings,
//! and the end-of-job writer for everything the interpreter collected.

use super::dpx::*;
use super::dpx::{fmt_prec, mat_mul, read_ti, round_to, ImageOpts, SOURCE_BCOLOR};
use super::RenderCtx;
use crate::dpx_obj::{dict_merge, dict_set, Obj, Parser};
use crate::engine::Engine;

fn skip_ws(p: &mut Parser) {
    p.skip_white();
}

// -------------------------------------------------------------- outlines

/// One `pdf:outline` item: normalised level (1 = top), dictionary and
/// open state (-1 = default).
#[derive(Clone)]
pub(crate) struct OutlineItem {
    pub level: i32,
    pub dict: Vec<(String, Obj)>,
    pub open: i32,
}

/// Bookmarks as `pdf_doc` collects them.
#[derive(Default)]
pub(crate) struct Outlines {
    pub items: Vec<OutlineItem>,
    /// `outlines.current_depth`
    pub depth: i32,
}

impl Engine {
    /// Reserve (or look up) the object number of page `n` (1-based), as
    /// `\pdfpageref` does for pdfTeX.
    pub(crate) fn dpx_page_ref(&mut self, n: usize) -> i32 {
        let key = n as i32;
        if let Some(&obj) = self.pdf_backend.page_objs.get(&key) {
            return obj;
        }
        let obj = self.alloc_pdf_obj();
        self.pdf_backend.page_objs.insert(key, obj);
        obj
    }

    /// The object number behind `@name`, reserving it when unknown.
    pub(crate) fn dpx_name_obj(&mut self, name: &str) -> i32 {
        if let Some(&n) = self.dpx.names.get(name) {
            return n;
        }
        let n = self.alloc_pdf_obj();
        self.dpx.names.insert(name.to_string(), n);
        n
    }

    /// Read a file for `pdf:fstream` the way `\pdfobj file` does.
    fn dpx_read_file(&mut self, name: &str) -> Option<Vec<u8>> {
        if let Some(bytes) = tex_kpse::get_embedded_package(name) {
            return Some(bytes);
        }
        let path = self
            .resolve_input_path(name)
            .or_else(|| self.font_loader.kpse.find_any(name))?;
        let bytes = tex_kpse::fs::read(&path).ok()?;
        self.record_loaded_bytes(&path, &bytes);
        self.loaded_files.push(path);
        Some(bytes)
    }
}

fn dict_bytes(d: &[(String, Obj)]) -> Vec<u8> {
    Obj::Dict(d.to_vec()).to_bytes()
}

/// `/Key value` pairs of a dictionary without the surrounding brackets.
fn dict_body(d: &[(String, Obj)]) -> Vec<u8> {
    let b = dict_bytes(d);
    // "<< /A 1 /B 2 >>" -> "/A 1 /B 2"
    let inner = &b[2..b.len() - 2];
    String::from_utf8_lossy(inner).trim().as_bytes().to_vec()
}

impl<'a> RenderCtx<'a> {
    // ---------------------------------------------------- parsing helpers

    /// `parse_pdf_object_extended` with `@name` references resolved.
    fn dpx_parse_obj(&mut self, p: &mut Parser, env: Env) -> Option<Obj> {
        let s = p.s;
        let mut lookup = |name: &str| self.dpx_lookup_ref(name, env);
        let mut q = Parser::with_lookup(s, &mut lookup);
        q.pos = p.pos;
        let obj = q.object();
        p.pos = q.pos;
        obj
    }

    /// `parse_pdf_dict_with_tounicode`
    fn dpx_parse_dict(&mut self, p: &mut Parser, env: Env) -> Option<Vec<(String, Obj)>> {
        let obj = self.dpx_parse_obj(p, env)?;
        match obj {
            Obj::Dict(mut d) => {
                let taint = self.eng.dpx.taint_keys.clone();
                for (k, v) in d.iter_mut() {
                    crate::dpx_obj::reencode_text_strings(v, Some(k), &taint);
                }
                Some(d)
            }
            _ => {
                self.dpx_warn("Dictionary type object expected but non-dictionary type found.");
                None
            }
        }
    }

    /// `spc_lookup_reference`
    fn dpx_lookup_ref(&mut self, name: &str, env: Env) -> Option<Obj> {
        match name {
            "xpos" => Some(Obj::Num(round_to(self.dpx_transform(env.x, env.y).0, 0.01))),
            "ypos" => Some(Obj::Num(round_to(self.dpx_transform(env.x, env.y).1, 0.01))),
            "thispage" => {
                let n = self.eng.dpx.page_no;
                Some(Obj::Ref(self.eng.dpx_page_ref(n)))
            }
            "prevpage" => {
                let n = self.eng.dpx.page_no;
                if n <= 1 {
                    self.dpx_warn("Could not find the named reference (@prevpage).");
                    None
                } else {
                    Some(Obj::Ref(self.eng.dpx_page_ref(n - 1)))
                }
            }
            "nextpage" => {
                let n = self.eng.dpx.page_no;
                Some(Obj::Ref(self.eng.dpx_page_ref(n + 1)))
            }
            _ => {
                if let Some(num) = name.strip_prefix("page").and_then(|d| d.parse::<usize>().ok()) {
                    if num > 0 {
                        return Some(Obj::Ref(self.eng.dpx_page_ref(num)));
                    }
                }
                match self.eng.dpx.names.get(name) {
                    Some(&n) => Some(Obj::Ref(n)),
                    None => {
                        // a forward reference: the object is reserved
                        if matches!(name, "resources" | "pages" | "names" | "catalog" | "docinfo") {
                            self.dpx_warn(&format!("Could not find the named reference (@{name})."));
                            None
                        } else {
                            Some(Obj::Ref(self.eng.dpx_name_obj(name)))
                        }
                    }
                }
            }
        }
    }

    // -------------------------------------------------------- the pdf: table

    pub(super) fn dpx_pdfm(
        &mut self,
        cmd: &str,
        p: &mut Parser,
        env: Env,
        _cur_h: i64,
        _cur_v: i64,
    ) -> Result<(), String> {
        match cmd {
            "annotation" | "annotate" | "annot" | "ann" => self.dpx_annot(p, env),
            "outline" | "out" => self.dpx_outline(p, env),
            "dest" | "destination" => self.dpx_dest(p, env),
            "names" => self.dpx_names(p, env),
            "object" | "obj" => self.dpx_object(p, env),
            "docinfo" => {
                let d = self
                    .dpx_parse_dict(p, env)
                    .ok_or("Dictionary object expected but not found.")?;
                dict_merge(&mut self.eng.dpx.docinfo, &d);
                Ok(())
            }
            "docview" => {
                let mut d = self
                    .dpx_parse_dict(p, env)
                    .ok_or("Dictionary object expected but not found.")?;
                // avoid overriding the whole ViewerPreferences
                let key = "ViewerPreferences";
                if let (Some(old), Some(new)) = (
                    self.eng.dpx.catalog.iter_mut().find(|(k, _)| k == key),
                    d.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()),
                ) {
                    if let (Obj::Dict(o), Obj::Dict(n)) = (&mut old.1, new) {
                        dict_merge(o, &n);
                        d.retain(|(k, _)| k != key);
                    }
                }
                dict_merge(&mut self.eng.dpx.catalog, &d);
                Ok(())
            }
            "content" => {
                if p.pos < p.s.len() {
                    let (cx, cy) = self.dpx_current_point(env);
                    let payload = String::from_utf8_lossy(&p.s[p.pos..]).into_owned();
                    self.dpx_emit(&format!("q 1 0 0 1 {} {} cm", fmt_g(cx), fmt_g(cy)));
                    self.dpx_emit(&payload);
                    self.dpx_emit("Q");
                }
                Ok(())
            }
            "put" => self.dpx_put(p, env),
            "close" => Ok(()),
            "bop" => {
                self.eng.dpx.bop = String::from_utf8_lossy(&p.s[p.pos..]).into_owned();
                Ok(())
            }
            "eop" => {
                self.eng.dpx.eop = String::from_utf8_lossy(&p.s[p.pos..]).into_owned();
                Ok(())
            }
            "image" | "img" | "epdf" => self.dpx_image(p, env),
            "link" => {
                self.eng.dpx.annot.link_annot = true;
                Ok(())
            }
            "nolink" => {
                self.eng.dpx.annot.link_annot = false;
                Ok(())
            }
            "begincolor" | "bcolor" | "bc" | "begingray" | "bgray" | "bg" => self.dpx_bcolor(p, false),
            "setcolor" | "scolor" | "sc" => self.dpx_bcolor(p, true),
            "endcolor" | "ecolor" | "ec" | "endgray" | "egray" | "eg" => {
                self.dpx_color_pop(SOURCE_BCOLOR);
                Ok(())
            }
            "bgcolor" | "bgc" | "bbc" | "bbg" => {
                let c = self
                    .dpx_read_color_pdf(p)
                    .map_err(|()| "No valid color specified?".to_string())?;
                self.eng.dpx.bgcolor = Some(c);
                Ok(())
            }
            "pagesize" => Ok(()),
            "bannot" | "beginann" | "bann" => self.dpx_bann(p, env),
            "eannot" | "endann" | "eann" => self.dpx_eann(),
            "btrans" | "begintransform" | "begintrans" | "bt" => self.dpx_btrans(p, env),
            "etrans" | "endtransform" | "endtrans" | "et" => {
                self.dpx_etrans();
                Ok(())
            }
            "bform" | "beginxobj" | "bxobj" => self.dpx_bxobj(p, env),
            "eform" | "endxobj" | "exobj" => self.dpx_exobj(p, env),
            "usexobj" | "uxobj" => self.dpx_uxobj(p, env),
            "literal" => {
                let mut direct = false;
                skip_ws(p);
                loop {
                    let rest = &p.s[p.pos..];
                    if rest.starts_with(b"reverse") {
                        p.pos += 7;
                    } else if rest.starts_with(b"direct") {
                        direct = true;
                        p.pos += 6;
                    } else {
                        break;
                    }
                    skip_ws(p);
                }
                self.dpx_literal_body(p, env, direct, false)
            }
            "stream" => self.dpx_stream(p, env, false),
            "fstream" => self.dpx_stream(p, env, true),
            "mapline" | "mapfile" => {
                let rest = String::from_utf8_lossy(&p.s[p.pos..]).into_owned();
                self.eng.process_map_item(rest.trim(), cmd == "mapfile");
                Ok(())
            }
            "bcontent" => {
                self.dpx_bcontent(env);
                Ok(())
            }
            "econtent" => {
                self.dpx_econtent();
                Ok(())
            }
            "code" => {
                skip_ws(p);
                if p.pos < p.s.len() {
                    let payload = String::from_utf8_lossy(&p.s[p.pos..]).into_owned();
                    self.dpx_emit(&payload);
                }
                Ok(())
            }
            "majorversion" | "minorversion" => {
                let v = p.number().ok_or("version number expected")? as i32;
                if cmd == "majorversion" {
                    self.eng.dpx.version.0 = Some(v);
                } else {
                    self.eng.dpx.version.1 = Some(v);
                }
                Ok(())
            }
            "pageresources" => {
                let o = self.dpx_parse_obj(p, env).ok_or("Dictionary object expected but not found.")?;
                if let Obj::Dict(_) = o {
                    self.eng.dpx.page_resources = Some(o);
                }
                Ok(())
            }
            "xannot" | "extendann" | "xann" => {
                if !self.dpx_tracking() {
                    return Ok(());
                }
                let ti = dpx_read_ti(p)?;
                let rect = self.dpx_annot_rect(env, &ti);
                self.dpx_expand_box(rect);
                Ok(())
            }
            "encrypt" | "trailerid" | "tounicode" | "article" | "art" | "bead" | "thread" | "bxgstate" | "exgstate" => {
                self.dpx_warn_once(cmd, &format!("pdf:{cmd} is not supported by this PDF writer."));
                Ok(())
            }
            _ => Err(format!("unknown pdf: special {cmd}")),
        }
    }

    // ------------------------------------------------------------ colours

    /// `pdf:bcolor` / `pdf:scolor`
    fn dpx_bcolor(&mut self, p: &mut Parser, set_only: bool) -> Result<(), String> {
        p.skip_white();
        let (cur_s, cur_f) = {
            let (s, f) = self.eng.dpx.colors.last().map(|c| (c.0.clone(), c.1.clone())).unwrap();
            (s, f)
        };
        let (mut sc, mut fc);
        let first = p.peek();
        if matches!(first, Some(b'f') | Some(b's')) {
            sc = cur_s.clone();
            fc = cur_f.clone();
            while p.pos < p.s.len() {
                let rest = &p.s[p.pos..];
                if rest.starts_with(b"fill") {
                    p.pos += 4;
                    p.skip_white();
                    fc = self.dpx_read_pdfcolor(p, &cur_f);
                } else if rest.starts_with(b"stroke") {
                    p.pos += 6;
                    p.skip_white();
                    sc = self.dpx_read_pdfcolor(p, &cur_s);
                } else {
                    break;
                }
                p.skip_white();
            }
        } else {
            fc = self
                .dpx_read_pdfcolor_opt(p, &cur_f)
                .ok_or("Invalid color specification?")?;
            sc = if p.pos < p.s.len() {
                self.dpx_read_pdfcolor_opt(p, &cur_s).ok_or("Invalid color specification?")?
            } else {
                fc.clone()
            };
        }
        if set_only {
            // pdf_color_set: replaces the current entry, keeps the stack
            let top = self.eng.dpx.colors.last_mut().unwrap();
            top.0 = sc;
            top.1 = fc;
            self.dpx_reset_color();
        } else {
            self.dpx_color_push(sc, fc, SOURCE_BCOLOR);
        }
        Ok(())
    }

    // -------------------------------------------------------- annotations

    /// `set_rect_for_annot`: the annotation rectangle of a `pdf:ann`-style
    /// dimension spec at the current point, through the CTM.
    fn dpx_annot_rect(&mut self, env: Env, ti: &TransformInfo) -> [f64; 4] {
        let (cx, cy) = self.dpx_current_point(env);
        let pts = if ti.flags & INFO_HAS_USER_BBOX != 0 {
            [
                (cx + ti.bbox[0], cy + ti.bbox[1]),
                (cx + ti.bbox[2], cy + ti.bbox[1]),
                (cx + ti.bbox[2], cy + ti.bbox[3]),
                (cx + ti.bbox[0], cy + ti.bbox[3]),
            ]
        } else {
            [
                (cx, cy - ti.depth),
                (cx + ti.width, cy - ti.depth),
                (cx + ti.width, cy + ti.height),
                (cx, cy + ti.height),
            ]
        };
        let mut r = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for (x, y) in pts {
            let (tx, ty) = self.dpx_transform(x, y);
            r = [r[0].min(tx), r[1].min(ty), r[2].max(tx), r[3].max(ty)];
        }
        r
    }

    /// `pdf_doc_add_annot`: record the annotation on the current page.
    fn dpx_add_annot(&mut self, rect: [f64; 4], dict: &[(String, Obj)]) {
        let subtype = match dict.iter().find(|(k, _)| k == "Subtype") {
            Some((_, Obj::Name(n))) => Some(n.clone()),
            _ => None,
        };
        let rest: Vec<(String, Obj)> = dict
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "Type" | "Subtype" | "Rect"))
            .cloned()
            .collect();
        let attr = String::from_utf8_lossy(&dict_body(&rest)).into_owned();
        let r = [
            round_to(rect[0], 0.001),
            round_to(rect[1], 0.001),
            round_to(rect[2], 0.001),
            round_to(rect[3], 0.001),
        ];
        self.annots.push(crate::pdfout::Annot { rect: r, uri: None, dest: None, attr, subtype });
    }

    /// `pdf:ann`
    fn dpx_annot(&mut self, p: &mut Parser, env: Env) -> Result<(), String> {
        skip_ws(p);
        if p.peek() == Some(b'@') {
            let _ident = p.opt_ident();
            skip_ws(p);
        }
        let ti = dpx_read_ti(p)?;
        if ti.flags & INFO_HAS_USER_BBOX != 0 && ti.flags & (INFO_HAS_WIDTH | INFO_HAS_HEIGHT) != 0 {
            return Err("You can't specify both bbox and width/height.".to_string());
        }
        let dict = self.dpx_parse_dict(p, env).ok_or("Could not find dictionary object.")?;
        let rect = self.dpx_annot_rect(env, &ti);
        self.dpx_add_annot(rect, &dict);
        Ok(())
    }

    /// `pdf:bann`
    fn dpx_bann(&mut self, p: &mut Parser, env: Env) -> Result<(), String> {
        if self.eng.dpx.annot.dict.is_some() {
            return Err("Can't begin an annotation when one is pending.".to_string());
        }
        skip_ws(p);
        if p.peek() == Some(b'@') {
            let _ident = p.opt_ident();
            skip_ws(p);
        }
        let dict = self
            .dpx_parse_dict(p, env)
            .ok_or("Ignoring annotation with invalid dictionary.")?;
        let a = &mut self.eng.dpx.annot;
        a.dict = Some(dict);
        a.broken = false;
        a.rect = None;
        // dvi_tag_depth
        a.tagged_depth = a.marked_depth;
        a.compute_boxes = true;
        Ok(())
    }

    /// `pdf:eann`
    fn dpx_eann(&mut self) -> Result<(), String> {
        if self.eng.dpx.annot.dict.is_none() {
            return Err("Tried to end an annotation without starting one!".to_string());
        }
        // dvi_untag_depth
        {
            let a = &mut self.eng.dpx.annot;
            a.tagged_depth = -1;
            a.compute_boxes = false;
        }
        self.dpx_break_annot();
        self.eng.dpx.annot.dict = None;
        Ok(())
    }

    /// `pdf_doc_break_annot`
    pub(super) fn dpx_break_annot(&mut self) {
        let (rect, dict) = {
            let a = &mut self.eng.dpx.annot;
            (a.rect.take(), a.dict.clone())
        };
        if let (Some(rect), Some(dict)) = (rect, dict) {
            self.dpx_add_annot(rect, &dict);
            self.eng.dpx.annot.broken = true;
        }
    }

    // ------------------------------------------------------- destinations

    /// `pdf:dest`
    fn dpx_dest(&mut self, p: &mut Parser, env: Env) -> Result<(), String> {
        skip_ws(p);
        let name = match p.object() {
            Some(Obj::Str(s)) => s,
            _ => return Err("PDF string expected for destination name but not found.".to_string()),
        };
        let array = self
            .dpx_parse_obj(p, env)
            .ok_or("No destination specified for pdf:dest.")?;
        if !matches!(array, Obj::Arr(_)) {
            return Err("Destination not specified as an array object!".to_string());
        }
        let dests = &mut self.eng.dpx.dests;
        if dests.iter().any(|(k, _)| *k == name) {
            self.dpx_warn(&format!("Object @{} already defined.", String::from_utf8_lossy(&name)));
            return Ok(());
        }
        dests.push((name, array));
        Ok(())
    }

    /// `pdf:names category key value` / `category [key value ...]`
    fn dpx_names(&mut self, p: &mut Parser, env: Env) -> Result<(), String> {
        let category = match p.object() {
            Some(Obj::Name(n)) => n,
            _ => return Err("PDF name expected but not found.".to_string()),
        };
        let first = self
            .dpx_parse_obj(p, env)
            .ok_or("PDF object expected but not found.")?;
        let mut entries = Vec::new();
        match first {
            Obj::Arr(items) => {
                if items.len() % 2 != 0 {
                    return Err("Array size not multiple of 2 for pdf:names.".to_string());
                }
                let mut it = items.into_iter();
                while let (Some(k), Some(v)) = (it.next(), it.next()) {
                    match k {
                        Obj::Str(k) => entries.push((k, v)),
                        _ => return Err("Name tree key must be string.".to_string()),
                    }
                }
            }
            Obj::Str(key) => {
                let value = self.dpx_parse_obj(p, env).ok_or("PDF object expected but not found.")?;
                entries.push((key, value));
            }
            _ => return Err("Invalid object type for pdf:names.".to_string()),
        }
        let tree = if category == "Dests" {
            &mut self.eng.dpx.dests
        } else {
            self.eng.dpx.name_trees.entry(category).or_default()
        };
        for (k, v) in entries {
            if !tree.iter().any(|(ek, _)| *ek == k) {
                tree.push((k, v));
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ outlines

    /// `pdf:outline [open] level <<item>>`
    fn dpx_outline(&mut self, p: &mut Parser, env: Env) -> Result<(), String> {
        skip_ws(p);
        let mut open = -1;
        if p.pos + 3 < p.s.len() && p.peek() == Some(b'[') {
            p.pos += 1;
            if p.peek() == Some(b'-') {
                p.pos += 1;
                open = 0;
            } else {
                open = 1;
            }
            p.pos += 1;
        }
        skip_ws(p);
        let level = match p.object() {
            Some(Obj::Num(n)) => n as i32,
            Some(_) => return Err("Expecting number for outline item depth.".to_string()),
            None => return Err("Missing number for outline item depth.".to_string()),
        };
        let dpx = &mut self.eng.dpx;
        dpx.lowest_level = dpx.lowest_level.min(level);
        let level = level + 1 - dpx.lowest_level;
        let dict = self.dpx_parse_dict(p, env).ok_or("Ignoring invalid dictionary.")?;
        let current = self.eng.dpx.outlines.depth.max(1);
        // jumping down more than one level creates empty parents
        if level > current + 1 {
            for missing in current + 1..level {
                self.dpx_warn("Empty bookmark node!");
                let dummy = vec![
                    ("Title".to_string(), Obj::Str(b"<No Title>".to_vec())),
                    ("C".to_string(), Obj::Arr(vec![Obj::Num(1.0), Obj::Num(0.0), Obj::Num(0.0)])),
                    ("F".to_string(), Obj::Num(1.0)),
                    (
                        "A".to_string(),
                        Obj::Dict(vec![
                            ("S".to_string(), Obj::Name("JavaScript".to_string())),
                            (
                                "JS".to_string(),
                                Obj::Str(
                                    b"app.alert(\"The author of this document made this bookmark item empty!\", 3, 0)"
                                        .to_vec(),
                                ),
                            ),
                        ]),
                    ),
                ];
                self.eng.dpx.outlines.items.push(OutlineItem { level: missing, dict: dummy, open: 0 });
            }
        }
        // BMOPEN: closed beyond the open depth (0 from dvipdfmx.cfg)
        let open = if open < 0 { i32::from(level <= 0) } else { open };
        self.eng.dpx.outlines.items.push(OutlineItem { level: level.max(1), dict, open });
        self.eng.dpx.outlines.depth = level.max(1);
        Ok(())
    }

    // -------------------------------------------------------------- objects

    /// `pdf:obj @name object`
    fn dpx_object(&mut self, p: &mut Parser, env: Env) -> Result<(), String> {
        skip_ws(p);
        let name = p.opt_ident().ok_or("Could not find a object identifier.")?;
        let obj = self
            .dpx_parse_obj(p, env)
            .ok_or_else(|| format!("Could not find an object definition for \"{name}\"."))?;
        let num = self.eng.dpx_name_obj(&name);
        if self.eng.dpx.objs.contains_key(&num) {
            self.dpx_warn(&format!("Object @{name} already defined."));
            return Ok(());
        }
        self.eng.dpx.objs.insert(num, obj);
        Ok(())
    }

    /// `pdf:put @name object...`
    fn dpx_put(&mut self, p: &mut Parser, env: Env) -> Result<(), String> {
        skip_ws(p);
        let ident = p.opt_ident().ok_or("Missing object identifier.")?;
        skip_ws(p);
        let obj2 = self
            .dpx_parse_obj(p, env)
            .ok_or_else(|| format!("Missing (an) object(s) to put into \"{ident}\"!"))?;
        match ident.as_str() {
            "resources" => match obj2 {
                Obj::Dict(d) => {
                    for (cat, v) in d {
                        self.dpx_put_resource(&cat, v);
                    }
                    Ok(())
                }
                _ => Err("Inconsistent object type for \"put\" (expecting DICT): resources".to_string()),
            },
            "catalog" | "docinfo" => match obj2 {
                Obj::Dict(d) => {
                    let target = if ident == "catalog" {
                        &mut self.eng.dpx.catalog
                    } else {
                        &mut self.eng.dpx.docinfo
                    };
                    dict_merge(target, &d);
                    Ok(())
                }
                _ => Err(format!("Inconsistent object type for \"put\" (expecting DICT): {ident}")),
            },
            _ => {
                let num = *self
                    .eng
                    .dpx
                    .names
                    .get(&ident)
                    .ok_or_else(|| format!("Specified object not exist: {ident}"))?;
                // further objects of an array put
                let mut more = Vec::new();
                if matches!(self.eng.dpx.objs.get(&num), Some(Obj::Arr(_))) {
                    skip_ws(p);
                    while p.pos < p.s.len() {
                        match self.dpx_parse_obj(p, env) {
                            Some(o) => more.push(o),
                            None => break,
                        }
                        skip_ws(p);
                    }
                }
                let target = self
                    .eng
                    .dpx
                    .objs
                    .get_mut(&num)
                    .ok_or_else(|| format!("Specified object not exist: {ident}"))?;
                match (target, obj2) {
                    (Obj::Dict(d), Obj::Dict(n)) => {
                        dict_merge(d, &n);
                        Ok(())
                    }
                    (Obj::Stream(d, _), Obj::Dict(n)) => {
                        dict_merge(d, &n);
                        Ok(())
                    }
                    (Obj::Stream(..), Obj::Stream(..)) => {
                        Err(format!("\"put\" operation not supported for STREAM <- STREAM: {ident}"))
                    }
                    (Obj::Arr(a), o) => {
                        a.push(o);
                        a.extend(more);
                        Ok(())
                    }
                    (Obj::Dict(_) | Obj::Stream(..), _) => Err(format!("Invalid type: expecting a DICT or STREAM: {ident}")),
                    _ => Err(format!("Can't \"put\" object into non-DICT/STREAM/ARRAY type object: {ident}")),
                }
            }
        }
    }

    /// `pdf:stream` / `pdf:fstream`
    fn dpx_stream(&mut self, p: &mut Parser, env: Env, file: bool) -> Result<(), String> {
        skip_ws(p);
        let name = p.opt_ident().ok_or("Missing objname for pdf:(f)stream.")?;
        skip_ws(p);
        let input = match p.object() {
            Some(Obj::Str(s)) => s,
            _ => return Err("Missing input string for pdf:(f)stream.".to_string()),
        };
        let data = if file {
            let fname = String::from_utf8_lossy(&input).into_owned();
            self.eng
                .dpx_read_file(&fname)
                .ok_or_else(|| format!("File \"{fname}\" not found."))?
        } else {
            input
        };
        let mut dict: Vec<(String, Obj)> = Vec::new();
        skip_ws(p);
        if p.peek() == Some(b'<') {
            match self.dpx_parse_obj(p, env) {
                Some(Obj::Dict(mut d)) => {
                    if d.iter().any(|(k, _)| k == "Length") {
                        d.retain(|(k, _)| k != "Length");
                    } else if d.iter().any(|(k, _)| k == "Filter") {
                        d.retain(|(k, _)| k != "Filter");
                    }
                    dict_merge(&mut dict, &d);
                }
                _ => return Err("Parsing dictionary failed.".to_string()),
            }
        }
        let num = self.eng.dpx_name_obj(&name);
        self.eng.dpx.objs.insert(num, Obj::Stream(dict, data));
        Ok(())
    }

    // ---------------------------------------------------------------- forms

    /// `pdf:bxobj @name <dims>`
    fn dpx_bxobj(&mut self, p: &mut Parser, env: Env) -> Result<(), String> {
        skip_ws(p);
        let name = p.opt_ident().ok_or("A form XObject must have name.")?;
        let ti = dpx_read_ti(p)?;
        let bbox = if ti.flags & INFO_HAS_USER_BBOX != 0 {
            if ti.bbox[2] - ti.bbox[0] == 0.0 || ti.bbox[3] - ti.bbox[1] == 0.0 {
                return Err("Bounding box has a zero dimension.".to_string());
            }
            ti.bbox
        } else {
            if ti.width == 0.0 || ti.depth + ti.height == 0.0 {
                return Err("Bounding box has a zero dimension.".to_string());
            }
            [0.0, -ti.depth, ti.width, ti.height]
        };
        let (cx, cy) = self.dpx_current_point(env);
        let obj = self.eng.dpx_name_obj(&name);
        if self.eng.dpx.forms.get(&name).is_none() {
            self.eng.pdf_xform_count += 1;
            let n = self.eng.pdf_xform_count;
            self.eng.pdf_doc.form_names.insert(obj, n);
        }
        self.dpx_begin_form(name, obj, (cx, cy), bbox);
        Ok(())
    }

    fn dpx_begin_form(&mut self, name: String, obj: i32, ref_pt: (f64, f64), bbox: [f64; 4]) {
        self.end_text();
        let frame = FormFrame::capture(self, obj, ref_pt, bbox);
        self.eng.dpx.forms.insert(name, FormInfo { obj, defined: false, bbox });
        self.eng.dpx.form_stack.push(frame);
        // the form is self-contained: colours are installed again
        self.dpx_reset_color();
    }

    /// `pdf:exobj [<attr>]`
    fn dpx_exobj(&mut self, p: &mut Parser, env: Env) -> Result<(), String> {
        if self.eng.dpx.form_stack.is_empty() {
            return Err("Tried to close a nonexistent form XOject.".to_string());
        }
        skip_ws(p);
        let mut attr: Option<Vec<(String, Obj)>> = None;
        if p.pos < p.s.len() {
            if let Some(Obj::Dict(d)) = self.dpx_parse_obj(p, env) {
                attr = Some(d);
            }
        }
        // pageresources here too
        if let Some(Obj::Dict(cats)) = self.eng.dpx.page_resources.clone() {
            for (cat, v) in cats {
                self.dpx_put_resource(&cat, v);
            }
        }
        self.dpx_end_form(attr);
        Ok(())
    }

    fn dpx_end_form(&mut self, attr: Option<Vec<(String, Obj)>>) {
        self.end_text();
        let frame = self.eng.dpx.form_stack.pop().expect("form frame");
        self.dpx_grestore_to(frame.q_depth);
        let obj = frame.obj;
        let ref_pt = frame.ref_pt;
        let bbox = frame.bbox;
        let (data, res, fonts, ximages, xforms) = frame.restore(self);
        // resources: fonts, nested forms and images, user categories
        let mut resources: Vec<(String, Obj)> = res;
        let font_object = self.eng.alloc_pdf_obj();
        self.eng.pdf_doc.objects.push((font_object, b"<< >>".to_vec()));
        self.eng.pdf_doc.form_fonts.push((font_object, fonts));
        if !resources.iter().any(|(k, _)| k == "Font") {
            resources.push(("Font".to_string(), Obj::Ref(font_object)));
        }
        let mut xobjects: Vec<(String, Obj)> = Vec::new();
        let prefix = self.eng.pdf_doc.resname_prefix.clone();
        for o in &xforms {
            let n = self.eng.pdf_doc.form_names.get(o).copied().unwrap_or(*o);
            xobjects.push((format!("Fm{n}{prefix}"), Obj::Ref(*o)));
        }
        for o in &ximages {
            let n = self.eng.pdf_doc.image_names.get(o).copied().unwrap_or(*o);
            xobjects.push((format!("Im{n}{prefix}"), Obj::Ref(*o)));
        }
        if !xobjects.is_empty() {
            match resources.iter_mut().find(|(k, _)| k == "XObject") {
                Some((_, Obj::Dict(d))) => dict_merge(d, &xobjects),
                Some(_) => {}
                None => resources.push(("XObject".to_string(), Obj::Dict(xobjects))),
            }
        }
        resources.push((
            "ProcSet".to_string(),
            Obj::Arr(["PDF", "Text", "ImageC", "ImageB", "ImageI"].iter().map(|n| Obj::Name(n.to_string())).collect()),
        ));
        let r3 = |v: f64| Obj::Num(round_to(v, 0.001));
        let mut dict: Vec<(String, Obj)> = vec![
            ("Type".to_string(), Obj::Name("XObject".to_string())),
            ("Subtype".to_string(), Obj::Name("Form".to_string())),
            ("FormType".to_string(), Obj::Num(1.0)),
            (
                "BBox".to_string(),
                Obj::Arr(vec![
                    r3(ref_pt.0 + bbox[0]),
                    r3(ref_pt.1 + bbox[1]),
                    r3(ref_pt.0 + bbox[2]),
                    r3(ref_pt.1 + bbox[3]),
                ]),
            ),
            (
                "Matrix".to_string(),
                Obj::Arr(vec![Obj::Num(1.0), Obj::Num(0.0), Obj::Num(0.0), Obj::Num(1.0), r3(-ref_pt.0), r3(-ref_pt.1)]),
            ),
        ];
        if let Some(a) = attr {
            dict_merge(&mut dict, &a);
        }
        dict_set(&mut dict, "Resources", Obj::Dict(resources));
        self.eng.dpx.objs.insert(obj, Obj::Stream(dict, data.into_bytes()));
        // forms keep a record of their box for later `uxobj`
        for info in self.eng.dpx.forms.values_mut() {
            if info.obj == obj {
                info.defined = true;
            }
        }
        self.dpx_reset_color();
    }

    /// `pdf:uxobj @name [<dims>]`
    fn dpx_uxobj(&mut self, p: &mut Parser, env: Env) -> Result<(), String> {
        skip_ws(p);
        let name = p.opt_ident().ok_or("No object identifier given.")?;
        let mut ti = TransformInfo::default();
        if p.pos < p.s.len() {
            ti = dpx_read_ti(p)?;
        }
        let obj = self.eng.dpx_name_obj(&name);
        let info = match self.eng.dpx.forms.get(&name).copied() {
            Some(i) => i,
            None => {
                self.eng.pdf_xform_count += 1;
                let n = self.eng.pdf_xform_count;
                self.eng.pdf_doc.form_names.insert(obj, n);
                let info = FormInfo { obj, defined: false, bbox: [0.0, 0.0, 1.0, 1.0] };
                self.eng.dpx.forms.insert(name, info);
                info
            }
        };
        let (xoff, yoff) = self.eng.dpx.coords.last().copied().unwrap_or((0.0, 0.0));
        let natural = Natural::Form { bbox: info.bbox };
        self.dpx_put_xobject(obj, natural, ti, env.x - xoff, env.y - yoff, false);
        Ok(())
    }

    // ---------------------------------------------------------------- images

    /// `pdf:image [@name] <options> (file) [<<dict>>]`
    fn dpx_image(&mut self, p: &mut Parser, env: Env) -> Result<(), String> {
        skip_ws(p);
        let mut ident = None;
        if p.peek() == Some(b'@') {
            ident = p.opt_ident();
            skip_ws(p);
        }
        let mut opts = ImageOpts { page: 1, ..Default::default() };
        let ti = dpx_read_ti_image(p, &mut opts)?;
        skip_ws(p);
        let file = match p.object() {
            Some(Obj::Str(s)) => String::from_utf8_lossy(&s).into_owned(),
            _ => return Err("Missing filename string for pdf:image.".to_string()),
        };
        skip_ws(p);
        if p.pos < p.s.len() {
            let _dict = self.dpx_parse_obj(p, env);
        }
        let img = match self.eng.dpx_load_image(&file, opts.page, opts.pagebox) {
            Ok(img) => img,
            Err(e) => return Err(format!("Could not find image resource... {e}")),
        };
        if let Some(name) = ident {
            self.eng.dpx.image_names.insert(name, img.obj);
        }
        if ti.flags & INFO_DO_HIDE == 0 {
            let (xoff, yoff) = self.eng.dpx.coords.last().copied().unwrap_or((0.0, 0.0));
            let natural = if img.kind == crate::engine::ImageKind::Pdf {
                Natural::Form { bbox: img.bbox }
            } else {
                Natural::Raster { width: f64::from(img.px_w), height: f64::from(img.px_h), xdensity: img.xdensity, ydensity: img.ydensity }
            };
            self.dpx_put_xobject(img.obj, natural, ti, env.x - xoff, env.y - yoff, true);
        }
        Ok(())
    }

    /// `pdf_dev_put_image`: place an XObject with the transformation
    /// `scale_to_fit_I`/`scale_to_fit_F` derive from the dimension spec.
    fn dpx_put_xobject(&mut self, obj: i32, natural: Natural, ti: TransformInfo, ref_x: f64, ref_y: f64, is_image: bool) {
        let mut m = ti.matrix;
        m[4] += ref_x;
        m[5] += ref_y;
        let (t, clip) = scale_to_fit(&natural, &ti);
        let m = mat_mul(&t, &m);
        self.end_text();
        self.dpx_gsave();
        self.dpx_concat(&m);
        if ti.flags & INFO_DO_CLIP != 0 {
            let text = format!(
                "{} {} {} {} re W n",
                fmt_g(clip[0]),
                fmt_g(clip[1]),
                fmt_g(clip[2] - clip[0]),
                fmt_g(clip[3] - clip[1])
            );
            self.dpx_emit(&text);
        }
        let prefix = self.eng.pdf_doc.resname_prefix.clone();
        let res_name = if is_image {
            let n = self.eng.pdf_doc.image_names.get(&obj).copied().unwrap_or(obj);
            if let Some(image) = self.eng.pdf_images.get_mut(&obj) {
                image.used = true;
            }
            if !self.ximage_list.contains(&obj) {
                self.ximage_list.push(obj);
            }
            self.dpx_page_group(obj);
            format!("Im{n}{prefix}")
        } else {
            let n = self.eng.pdf_doc.form_names.get(&obj).copied().unwrap_or(obj);
            if !self.xform_list.contains(&obj) {
                self.xform_list.push(obj);
            }
            format!("Fm{n}{prefix}")
        };
        self.dpx_emit(&format!("/{res_name} Do"));
        // is_drawable: the clip box (unit square / bbox) through the CTM
        if self.dpx_tracking() {
            let c = clip;
            let pts = [(c[0], c[1]), (c[2], c[1]), (c[2], c[3]), (c[0], c[3])];
            let mut r = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            for (x, y) in pts {
                let (tx, ty) = self.dpx_transform(x, y);
                r = [r[0].min(tx), r[1].min(ty), r[2].max(tx), r[3].max(ty)];
            }
            self.dpx_expand_box(r);
        }
        self.dpx_grestore();
    }

    /// An included PDF page's /Group becomes the page group, as with
    /// pdfTeX's `out_image`.
    fn dpx_page_group(&mut self, obj: i32) {
        use crate::engine::ImageKind;
        let Some(image) = self.eng.pdf_images.get(&obj) else { return };
        let (kind, group_ref) = (image.kind, image.group_ref);
        if kind == ImageKind::Png && group_ref > 0 && self.eng.pdf_page_group_val == 0 {
            self.eng.pdf_page_group_val = group_ref;
        } else if kind == ImageKind::Pdf && group_ref != 0 && self.eng.pdf_page_group_val == 0 {
            self.eng.pdf_page_group_val = if group_ref == -1 {
                let group = self.eng.alloc_pdf_obj();
                if let Some(image) = self.eng.pdf_images.get_mut(&obj) {
                    image.group_ref = group;
                }
                group
            } else {
                group_ref
            };
        }
    }
}

/// The natural size of an XObject placed by `dpx_put_xobject`.
pub(crate) enum Natural {
    Raster { width: f64, height: f64, xdensity: f64, ydensity: f64 },
    Form { bbox: [f64; 4] },
}

fn fmt_g(v: f64) -> String {
    fmt_prec(v, 5)
}

/// `pdf_ximage_scale_image`: the matrix `T` and the clip rectangle.
fn scale_to_fit(n: &Natural, p: &TransformInfo) -> ([f64; 6], [f64; 4]) {
    let has_w = p.flags & INFO_HAS_WIDTH != 0;
    let has_h = p.flags & INFO_HAS_HEIGHT != 0;
    let user_bbox = p.flags & INFO_HAS_USER_BBOX != 0;
    match n {
        Natural::Raster { width, height, xdensity, ydensity } => {
            let (mut wd0, mut ht0, xscale, yscale, d_x, d_y);
            if user_bbox {
                wd0 = p.bbox[2] - p.bbox[0];
                ht0 = p.bbox[3] - p.bbox[1];
                xscale = width * xdensity / wd0;
                yscale = height * ydensity / ht0;
                d_x = -p.bbox[0] / wd0;
                d_y = -p.bbox[1] / ht0;
            } else {
                wd0 = width * xdensity;
                ht0 = height * ydensity;
                xscale = 1.0;
                yscale = 1.0;
                d_x = 0.0;
                d_y = 0.0;
            }
            if wd0 == 0.0 {
                wd0 = 1.0;
            }
            if ht0 == 0.0 {
                ht0 = 1.0;
            }
            let (s_x, s_y, dp);
            if has_w && has_h {
                s_x = p.width * xscale;
                s_y = (p.height + p.depth) * yscale;
                dp = p.depth * yscale;
            } else if has_w {
                s_x = p.width * xscale;
                s_y = s_x * (height / width);
                dp = 0.0;
            } else if has_h {
                s_y = (p.height + p.depth) * yscale;
                s_x = s_y * (width / height);
                dp = p.depth * yscale;
            } else {
                s_x = wd0;
                s_y = ht0;
                dp = 0.0;
            }
            let t = [s_x, 0.0, 0.0, s_y, d_x * s_x / xscale, d_y * s_y / yscale - dp];
            let r = if user_bbox {
                [
                    p.bbox[0] / (width * xdensity),
                    p.bbox[1] / (height * ydensity),
                    p.bbox[2] / (width * xdensity),
                    p.bbox[3] / (height * ydensity),
                ]
            } else {
                [0.0, 0.0, 1.0, 1.0]
            };
            (t, r)
        }
        Natural::Form { bbox } => {
            let (mut wd0, mut ht0, d_x, d_y);
            if user_bbox {
                wd0 = p.bbox[2] - p.bbox[0];
                ht0 = p.bbox[3] - p.bbox[1];
                d_x = -p.bbox[0];
                d_y = -p.bbox[1];
            } else {
                wd0 = bbox[2] - bbox[0];
                ht0 = bbox[3] - bbox[1];
                d_x = 0.0;
                d_y = 0.0;
            }
            if wd0 == 0.0 {
                wd0 = 1.0;
            }
            if ht0 == 0.0 {
                ht0 = 1.0;
            }
            let (s_x, s_y, dp);
            if has_w && has_h {
                s_x = p.width / wd0;
                s_y = (p.height + p.depth) / ht0;
                dp = p.depth;
            } else if has_w {
                s_x = p.width / wd0;
                s_y = s_x;
                dp = 0.0;
            } else if has_h {
                s_y = (p.height + p.depth) / ht0;
                s_x = s_y;
                dp = p.depth;
            } else {
                s_x = 1.0;
                s_y = 1.0;
                dp = 0.0;
            }
            let t = [s_x, 0.0, 0.0, s_y, s_x * d_x, s_y * d_y - dp];
            let r = if user_bbox { p.bbox } else { *bbox };
            (t, r)
        }
    }
}

fn dpx_read_ti(p: &mut Parser) -> Result<TransformInfo, String> {
    read_ti(p, None)
}

fn dpx_read_ti_image(p: &mut Parser, opts: &mut ImageOpts) -> Result<TransformInfo, String> {
    read_ti(p, Some(opts))
}


// ------------------------------------------------------- form frame state

impl FormFrame {
    /// `pdf_doc_begin_grabbing`: swap the page state for a fresh form state.
    fn capture(ctx: &mut RenderCtx, obj: i32, ref_pt: (f64, f64), bbox: [f64; 4]) -> FormFrame {
        let frame = FormFrame {
            obj,
            ref_pt,
            bbox,
            saved_content: std::mem::take(&mut ctx.content),
            saved_gs: std::mem::replace(&mut ctx.dpx.gs, vec![Gs::initial()]),
            saved_res: std::mem::take(&mut ctx.dpx.res),
            saved_used_fonts: std::mem::take(&mut ctx.used_fonts),
            saved_page_fonts: std::mem::take(&mut ctx.page_fonts),
            saved_ximages: std::mem::take(&mut ctx.ximage_list),
            saved_xforms: std::mem::take(&mut ctx.xform_list),
            saved_cur_font: std::mem::replace(&mut ctx.cur_font, 0),
            saved_cur_pdf_font: std::mem::replace(&mut ctx.cur_pdf_font, 0),
            q_depth: 0,
        };
        frame
    }

    /// Put the page state back and hand over what the form produced:
    /// content, resource categories, fonts, images, nested forms.
    fn restore(self, ctx: &mut RenderCtx) -> (String, Vec<(String, Obj)>, Vec<(usize, u32)>, Vec<i32>, Vec<i32>) {
        let data = std::mem::replace(&mut ctx.content, self.saved_content);
        ctx.dpx.gs = self.saved_gs;
        let res = std::mem::replace(&mut ctx.dpx.res, self.saved_res);
        let form_used = std::mem::replace(&mut ctx.used_fonts, self.saved_used_fonts);
        let form_fonts = std::mem::replace(&mut ctx.page_fonts, self.saved_page_fonts);
        // fonts first used inside the form are known to the page too
        for entry in form_used {
            if !ctx.used_fonts.iter().any(|(k, _)| *k == entry.0) {
                ctx.used_fonts.push(entry);
            }
        }
        let ximages = std::mem::replace(&mut ctx.ximage_list, self.saved_ximages);
        let xforms = std::mem::replace(&mut ctx.xform_list, self.saved_xforms);
        ctx.cur_font = self.saved_cur_font;
        ctx.cur_pdf_font = self.saved_cur_pdf_font;
        (data, res, form_fonts, ximages, xforms)
    }
}

// ----------------------------------------------------------- end of job

impl Engine {
    /// Write everything the special interpreter collected into the PDF
    /// document before it is serialised: user objects and streams, the
    /// destination name tree, bookmarks, catalog entries, the information
    /// dictionary, PDF version.
    pub(crate) fn dpx_finish(&mut self) {
        if self.engine_kind != crate::engine::EngineKind::XeTeX {
            return;
        }
        // objects referenced but never defined are null
        let undefined: Vec<(String, i32)> = self
            .dpx
            .names
            .iter()
            .filter(|(_, n)| !self.dpx.objs.contains_key(n))
            .map(|(k, n)| (k.clone(), *n))
            .collect();
        for (name, num) in undefined {
            self.warning_at(&format!("xdvipdfmx warning: Object @{name} used, but not defined. Replaced by null."), None);
            self.dpx.objs.insert(num, Obj::Null);
        }
        let objs = std::mem::take(&mut self.dpx.objs);
        for (num, obj) in objs {
            let bytes = match obj {
                Obj::Stream(mut dict, data) => {
                    if !dict.iter().any(|(k, _)| k == "Filter") && !data.is_empty() {
                        let packed = crate::pdffile::flate(&data);
                        dict_set(&mut dict, "Filter", Obj::Name("FlateDecode".to_string()));
                        Obj::Stream(dict, packed).to_bytes()
                    } else {
                        Obj::Stream(dict, data).to_bytes()
                    }
                }
                other => other.to_bytes(),
            };
            self.pdf_doc.objects.push((num, bytes));
        }

        let mut catalog = std::mem::take(&mut self.dpx.catalog);

        // destination name tree
        let mut tree_entries: Vec<(&str, Vec<(Vec<u8>, Obj)>)> = Vec::new();
        let dests = std::mem::take(&mut self.dpx.dests);
        if !dests.is_empty() {
            tree_entries.push(("Dests", dests));
        }
        let others = std::mem::take(&mut self.dpx.name_trees);
        let others: Vec<(String, Vec<(Vec<u8>, Obj)>)> = others.into_iter().collect();
        let mut names_dict: Vec<(String, Obj)> = Vec::new();
        let mut trees: Vec<(String, Vec<(Vec<u8>, Obj)>)> =
            tree_entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        trees.extend(others);
        for (category, mut entries) in trees {
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut flat = Vec::with_capacity(entries.len() * 2);
            for (k, v) in entries {
                flat.push(Obj::Str(k));
                flat.push(v);
            }
            let tree = Obj::Dict(vec![("Names".to_string(), Obj::Arr(flat))]);
            let num = self.alloc_pdf_obj();
            self.pdf_doc.objects.push((num, tree.to_bytes()));
            names_dict.push((category, Obj::Ref(num)));
        }
        if !names_dict.is_empty() {
            let num = self.alloc_pdf_obj();
            self.pdf_doc.objects.push((num, Obj::Dict(names_dict).to_bytes()));
            dict_set(&mut catalog, "Names", Obj::Ref(num));
        }

        // bookmarks
        let items = std::mem::take(&mut self.dpx.outlines.items);
        if !items.is_empty() {
            if let Some(root) = self.dpx_write_outlines(items) {
                dict_set(&mut catalog, "Outlines", Obj::Ref(root));
            }
        }
        let extra = dict_body(&catalog);
        if !extra.is_empty() {
            if !self.pdf_doc.catalog_extra.is_empty() {
                self.pdf_doc.catalog_extra.push(b' ');
            }
            self.pdf_doc.catalog_extra.extend_from_slice(&extra);
        }

        // information dictionary
        let mut info = std::mem::take(&mut self.dpx.docinfo);
        if !info.iter().any(|(k, _)| k == "Creator") {
            use crate::prim::IntParam::{Day, Month, Time, Year};
            let int = |e: &Self, p: crate::prim::IntParam| e.eqtb.int_params[p.idx() as usize];
            let t = int(self, Time);
            let creator = format!(
                " XeTeX output {:04}.{:02}.{:02}:{:02}{:02}",
                int(self, Year),
                int(self, Month),
                int(self, Day),
                t / 60,
                t % 60
            );
            info.push(("Creator".to_string(), Obj::Str(creator.into_bytes())));
        }
        if !info.iter().any(|(k, _)| k == "Producer") {
            info.push(("Producer".to_string(), Obj::Str(b"xdvipdfmx (20260113)".to_vec())));
        }
        self.pdf_doc.info = dict_body(&info);
        self.pdf_doc.xdvipdfmx = true;
        if let Some(major) = self.dpx.version.0 {
            self.pdf_doc.major_version = major;
        }
        if let Some(minor) = self.dpx.version.1 {
            self.pdf_doc.minor_version = Some(minor);
        }
    }

    /// `pdf_doc_close_bookmarks`: link the items into the outline tree.
    fn dpx_write_outlines(&mut self, items: Vec<OutlineItem>) -> Option<i32> {
        let n = items.len();
        let nums: Vec<i32> = (0..n).map(|_| self.alloc_pdf_obj()).collect();
        let root = self.alloc_pdf_obj();
        // parent / children from the levels
        let mut parent: Vec<Option<usize>> = vec![None; n];
        let mut stack: Vec<usize> = Vec::new();
        for (i, item) in items.iter().enumerate() {
            while stack.last().is_some_and(|&top| items[top].level >= item.level) {
                stack.pop();
            }
            parent[i] = stack.last().copied();
            stack.push(i);
        }
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut top: Vec<usize> = Vec::new();
        for (i, p) in parent.iter().enumerate() {
            match p {
                Some(p) => children[*p].push(i),
                None => top.push(i),
            }
        }
        // visible descendants (flush_bookmarks' return value)
        fn visible(i: usize, items: &[OutlineItem], children: &[Vec<usize>]) -> i32 {
            let mut count = 0;
            for &c in &children[i] {
                count += 1;
                if items[c].open != 0 {
                    count += visible(c, items, children);
                }
            }
            count
        }
        for i in 0..n {
            let mut dict = items[i].dict.clone();
            let par = parent[i].map_or(root, |p| nums[p]);
            dict_set(&mut dict, "Parent", Obj::Ref(par));
            let siblings: &Vec<usize> = match parent[i] {
                Some(p) => &children[p],
                None => &top,
            };
            let pos = siblings.iter().position(|&s| s == i).unwrap();
            if pos > 0 {
                dict_set(&mut dict, "Prev", Obj::Ref(nums[siblings[pos - 1]]));
            }
            if pos + 1 < siblings.len() {
                dict_set(&mut dict, "Next", Obj::Ref(nums[siblings[pos + 1]]));
            }
            if let (Some(&first), Some(&last)) = (children[i].first(), children[i].last()) {
                dict_set(&mut dict, "First", Obj::Ref(nums[first]));
                dict_set(&mut dict, "Last", Obj::Ref(nums[last]));
                let count = visible(i, &items, &children);
                let count = if items[i].open != 0 { count } else { -count };
                dict_set(&mut dict, "Count", Obj::Num(f64::from(count)));
            }
            self.pdf_doc.objects.push((nums[i], Obj::Dict(dict).to_bytes()));
        }
        let (first, last) = (*top.first()?, *top.last()?);
        let mut count = top.len() as i32;
        for &t in &top {
            if items[t].open != 0 {
                count += visible(t, &items, &children);
            }
        }
        let root_dict = vec![
            ("Type".to_string(), Obj::Name("Outlines".to_string())),
            ("First".to_string(), Obj::Ref(nums[first])),
            ("Last".to_string(), Obj::Ref(nums[last])),
            ("Count".to_string(), Obj::Num(f64::from(count))),
        ];
        self.pdf_doc.objects.push((root, Obj::Dict(root_dict).to_bytes()));
        Some(root)
    }
}
