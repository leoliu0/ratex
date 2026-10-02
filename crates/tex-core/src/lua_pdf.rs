//! Engine side of LuaTeX's `pdf` library (lpdflib.c): backend parameters,
//! object and font queries and the actions that run the pdfTeX backend
//! commands. `lua_pdf.lua` builds the Lua API on these natives.

use tex_lua::{Lua, LuaApi, LuaBytes, LuaString, LuaTable, Value};

use crate::engine::Engine;
use crate::eqtb::Equiv;
use crate::lua_bridge::{bytes_of, with_engine};
use crate::prim::Prim;
use crate::token::Token;

macro_rules! reg {
    ($lua:expr, $tbl:expr, $name:literal, $f:expr) => {
        $tbl.set($name, $lua.create_function($f).map_err(|e| format!("{}: {e:?}", $name))?)
            .map_err(|e| format!("{}: {e:?}", $name))?
    };
}

/// A `\pdfvariable` key or an engine-side text parameter.
enum Var {
    Int(crate::prim::IntParam),
    Dim(crate::prim::DimParam),
    Toks(crate::prim::ToksParam),
    Text(&'static str),
}

impl Engine {
    /// `pdf.setforcefile`: the PDF is written although no page was shipped out.
    pub fn pdf_force_file(&self) -> bool {
        self.lua_tex.force_file
    }

    fn lua_pdf_var(&self, key: &[u8]) -> Option<Var> {
        match key {
            b"catalog" => return Some(Var::Text("catalog")),
            b"info" => return Some(Var::Text("info")),
            b"names" => return Some(Var::Text("names")),
            b"trailer" => return Some(Var::Text("trailer")),
            b"trailerid" => return Some(Var::Text("trailerid")),
            b"pageattr" => return Some(Var::Text("pageattr")),
            b"pagesattr" => return Some(Var::Text("pagesattr")),
            b"pageresources" => return Some(Var::Text("pageresources")),
            _ => {}
        }
        let (_, target) = crate::luatex::PDF_VARIABLES.iter().find(|(k, _)| *k == key)?;
        let cs = self.backend_cs(target)?;
        match self.eqtb.resolve(cs)? {
            Equiv::Prim(Prim::IntP(p)) => Some(Var::Int(*p)),
            Equiv::Prim(Prim::DimP(p)) => Some(Var::Dim(*p)),
            Equiv::Prim(Prim::ToksP(p)) => Some(Var::Toks(*p)),
            _ => None,
        }
    }

    fn lua_pdf_text_get(&mut self, which: &str) -> Option<Vec<u8>> {
        let bytes = match which {
            "catalog" => self.pdf_doc.catalog_extra.clone(),
            "info" => self.pdf_doc.info.clone(),
            "names" => self.pdf_doc.names_extra.clone(),
            "trailer" => self.pdf_doc.trailer_extra.clone(),
            "trailerid" => self.pdf_doc.trailer_id_raw.clone(),
            "pageattr" => self.pdf_page_attr.clone().into_bytes(),
            "pagesattr" => self.pdf_pages_attr.clone().into_bytes(),
            _ => self.pdf_page_resources.clone(),
        };
        (!bytes.is_empty()).then_some(bytes)
    }

    fn lua_pdf_text_set(&mut self, which: &str, text: Vec<u8>) {
        let toks = Engine::lua_str_toks(&text);
        let string = || String::from_utf8_lossy(&text).into_owned();
        match which {
            "catalog" => self.pdf_doc.catalog_extra = text,
            "info" => self.pdf_doc.info = text,
            "names" => self.pdf_doc.names_extra = text,
            "trailer" => self.pdf_doc.trailer_extra = text,
            "trailerid" => self.pdf_doc.trailer_id_raw = text,
            "pageattr" => {
                self.pdf_page_attr = string();
                self.pdf_page_attr_toks = toks;
            }
            "pagesattr" => {
                self.pdf_pages_attr = string();
                self.pdf_pages_attr_toks = toks;
                self.pdf_doc.pages_attr = text;
            }
            _ => {
                self.pdf_page_resources = text;
                self.pdf_page_resources_toks = toks;
            }
        }
    }

    /// Execute `toks` as TeX commands now (the `tex.runtoks` mechanism).
    pub(crate) fn lua_run_tokens(&mut self, toks: Vec<Token>) {
        let end = self.lua_end_local_control_token();
        self.push_token(end);
        if !toks.is_empty() {
            self.push_tokens_named(toks, "<lua pdf>");
        }
        self.lua_local_control();
    }

    /// Tokens for a Lua argument list: strings are other characters (spaces
    /// stay spaces), `1` and `2` are begin/end group, `{"name"}` is a
    /// control sequence.
    fn lua_pdf_parts(&mut self, parts: &LuaTable) -> Result<Vec<Token>, String> {
        let mut toks = Vec::new();
        let n = parts.raw_len().map_err(|e| format!("{e:?}"))?;
        for i in 1..=n {
            let v: Value = parts.raw_geti(i as i64).map_err(|e| format!("{e:?}"))?;
            if let Some(code) = v.as_integer() {
                toks.push(Token::unicode_char(if code == 1 { 1 } else { 2 }, if code == 1 { '{' } else { '}' } as u32));
            } else if let Some(t) = v.as_table() {
                let kind: Option<LuaString> = t.raw_geti(2).map_err(|e| format!("{e:?}"))?;
                if kind.as_ref().map(bytes_of).as_deref() == Some(b"font") {
                    let f: i64 = t.raw_geti(1).map_err(|e| format!("{e:?}"))?;
                    let f = self.lua_pdf_font(f)?;
                    toks.push(self.lua_pdf_font_token(f));
                    continue;
                }
                let name = bytes_of(&t.raw_geti::<LuaString>(1).map_err(|e| format!("{e:?}"))?);
                let backend = kind;
                let id = if backend.is_some() {
                    self.backend_cs(&name).ok_or_else(|| format!("unknown backend command {}", String::from_utf8_lossy(&name)))?
                } else {
                    self.lua_prim_cs(&name)
                };
                toks.push(Token::from_cs(id));
            } else if let Some(s) = v.as_string() {
                let bytes = s.as_bytes().to_vec();
                for c in String::from_utf8_lossy(&bytes).chars() {
                    toks.push(if c == ' ' { Token::space() } else { Token::unicode_char(12, c as u32) });
                }
            } else {
                return Err("invalid argument".to_string());
            }
        }
        Ok(toks)
    }

    /// A control sequence with the LuaTeX primitive `name` as its meaning,
    /// whatever `\name` is bound to at the moment.
    pub(crate) fn lua_prim_cs(&mut self, name: &[u8]) -> crate::token::CsId {
        let equiv = self.lua_primitives.iter().find(|p| p.name == name).map(|p| p.equiv.clone());
        let Some(equiv) = equiv else {
            return self.cs.intern(name);
        };
        let id = self.cs.intern(&[b"\xFF\x00LUA-prim-", name].concat());
        self.eqtb.assign(id, equiv, true);
        id
    }

    fn lua_pdf_font(&mut self, f: i64) -> Result<u16, String> {
        u16::try_from(f)
            .ok()
            .filter(|f| *f != 0 && (*f as usize) < self.eqtb.fonts.len())
            .ok_or_else(|| "invalid font identifier".to_string())
    }

    /// A control sequence meaning font `f`, to hand to the font scanners.
    fn lua_pdf_font_token(&mut self, f: u16) -> Token {
        let id = self.cs.intern(b"\xFF\x00LUA-font");
        self.eqtb.assign(id, Equiv::FontRef(f), true);
        Token::from_cs(id)
    }

    fn lua_pdf_creation_date(&mut self) -> String {
        self.pdf_creation_date.get_or_insert_with(crate::expand::pdf_creation_date).clone()
    }

    fn lua_pdf_scan_args(&mut self, mut toks: Vec<Token>) {
        toks.push(Token::space());
        self.push_tokens_named(toks, "<lua pdf>");
    }
}

fn digits(n: i64) -> Vec<Token> {
    n.to_string().chars().map(|c| Token::unicode_char(12, c as u32)).collect()
}

pub(crate) fn install(lua: &mut Lua) -> Result<(), String> {
    let p: LuaTable = lua.create_table().map_err(|e| format!("{e:?}"))?;

    // ---- parameters ----
    reg!(lua, p, "var_get", |key: LuaString| -> Result<(Option<String>, i64, Option<LuaBytes>), String> {
        let key = bytes_of(&key);
        with_engine(|e| match e.lua_pdf_var(&key) {
            None => (None, 0, None),
            Some(Var::Int(ip)) => (Some("int".into()), i64::from(e.int_param_value(ip)), None),
            Some(Var::Dim(dp)) => (Some("dim".into()), i64::from(e.dim_param_value(dp)), None),
            Some(Var::Toks(tp)) => {
                let toks = e.eqtb.tok_params[tp.idx() as usize].clone();
                let bytes = e.tokens_to_bytes(&toks);
                (Some("text".into()), 0, (!bytes.is_empty()).then(|| LuaBytes(bytes)))
            }
            Some(Var::Text(which)) => (Some("text".into()), 0, e.lua_pdf_text_get(which).map(LuaBytes)),
        })
    });
    reg!(lua, p, "var_set", |key: LuaString, value: i64, text: Option<LuaString>| -> Result<(), String> {
        let key = bytes_of(&key);
        let text = text.as_ref().map(bytes_of);
        with_engine(|e| {
            match e.lua_pdf_var(&key) {
                Some(Var::Int(ip)) => e.eqtb.assign_int_param(ip, value as i32, true),
                Some(Var::Dim(dp)) => e.eqtb.assign_dim_param(dp, value as i32, true),
                Some(Var::Toks(tp)) => {
                    let toks = Engine::lua_str_toks(&text.unwrap_or_default());
                    e.eqtb.assign_toks_param(tp, std::rc::Rc::new(toks), true);
                }
                Some(Var::Text(which)) => e.lua_pdf_text_set(which, text.unwrap_or_default()),
                None => {}
            }
        })
    });

    // ---- queries ----
    reg!(lua, p, "last", |which: String| -> Result<i64, String> {
        with_engine(|e| match which.as_str() {
            "obj" => i64::from(e.pdf_last_obj),
            "annot" => i64::from(e.pdf_last_annot),
            "link" => i64::from(e.pdf_last_link),
            "retval" => i64::from(e.pdf_retval),
            _ => 0,
        })
    });
    reg!(lua, p, "max_obj", || -> Result<i64, String> { with_engine(|e| i64::from(e.pdf_next_obj - 1)) });
    reg!(lua, p, "pos", || -> Result<(i64, i64), String> {
        with_engine(|e| (i64::from(e.lua_tex.pdf_pos.0), i64::from(e.lua_tex.pdf_pos.1)))
    });
    reg!(lua, p, "obj_type", |n: i64| -> Result<Option<String>, String> {
        with_engine(|e| {
            let n = i32::try_from(n).ok()?;
            if e.pdf_doc.objects.iter().any(|(o, _)| *o == n) || e.pdf_reserved_objnums.contains(&n) {
                Some("obj".to_string())
            } else if e.pdf_backend.page_objs.values().any(|o| *o == n) {
                Some("page".to_string())
            } else if e.pdf_backend.font_objs.values().any(|o| *o == n) {
                Some("font".to_string())
            } else if e.pdf_doc.form_names.contains_key(&n) {
                Some("xform".to_string())
            } else {
                None
            }
        })
    });
    reg!(lua, p, "font_name", |f: i64| -> Result<i64, String> {
        with_engine(|e| {
            let f = e.lua_pdf_font(f)?;
            let tok = e.lua_pdf_font_token(f);
            e.lua_pdf_scan_args(vec![tok]);
            e.pdf_font_name().map(i64::from).ok_or_else(|| "invalid font identifier".to_string())
        })?
    });
    reg!(lua, p, "font_objnum", |f: i64| -> Result<i64, String> {
        with_engine(|e| {
            let f = e.lua_pdf_font(f)?;
            let tok = e.lua_pdf_font_token(f);
            e.lua_pdf_scan_args(vec![tok]);
            e.pdf_font_objnum().map(i64::from).ok_or_else(|| "invalid font identifier".to_string())
        })?
    });
    reg!(lua, p, "font_size", |f: i64| -> Result<Option<i64>, String> {
        with_engine(|e| {
            let f = e.lua_pdf_font(f).ok()?;
            Some(i64::from(e.eqtb.fonts[f as usize].at_size))
        })
    });
    reg!(lua, p, "page_ref", |page: i64| -> Result<i64, String> {
        with_engine(|e| {
            e.lua_pdf_scan_args(digits(page));
            e.pdf_page_ref().map(i64::from).ok_or_else(|| "invalid page number".to_string())
        })?
    });
    reg!(lua, p, "xform_name", |obj: i64| -> Result<i64, String> {
        with_engine(|e| {
            e.lua_pdf_scan_args(digits(obj));
            e.pdf_xform_name().map(i64::from).ok_or_else(|| "cannot find referenced object".to_string())
        })?
    });

    // ---- actions ----
    reg!(lua, p, "run", |parts: LuaTable| -> Result<(), String> {
        with_engine(|e| {
            let toks = e.lua_pdf_parts(&parts)?;
            e.lua_run_tokens(toks);
            Ok::<(), String>(())
        })?
    });
    reg!(lua, p, "backend", |name: String| -> Result<Option<String>, String> {
        // the name of the hidden control sequence running pdfTeX command `name`
        with_engine(|e| {
            let cs = e.backend_cs(name.as_bytes())?;
            Some(String::from_utf8_lossy(e.cs.name(cs)).into_owned())
        })
    });
    reg!(lua, p, "colorstack_init", |parts: LuaTable| -> Result<i64, String> {
        with_engine(|e| {
            let toks = e.lua_pdf_parts(&parts)?;
            e.lua_pdf_scan_args(toks);
            Ok::<i64, String>(i64::from(e.pdf_colorstack_init()))
        })?
    });
    reg!(lua, p, "map", |item: LuaString, file: bool| -> Result<(), String> {
        let item = String::from_utf8_lossy(&bytes_of(&item)).into_owned();
        with_engine(|e| e.process_map_item(&item, file))
    });
    reg!(lua, p, "include_chars", |f: i64, chars: LuaString| -> Result<(), String> {
        let chars = bytes_of(&chars);
        with_engine(|e| {
            let f = e.lua_pdf_font(f)?;
            e.pdf_init_font(f);
            for c in chars {
                e.pdf_doc.record_font_char(f as usize, c);
            }
            Ok::<(), String>(())
        })?
    });
    reg!(lua, p, "creation_date", || -> Result<LuaBytes, String> {
        with_engine(|e| LuaBytes(e.lua_pdf_creation_date().into_bytes()))
    });
    reg!(lua, p, "print", |mode: i64, text: LuaString| -> Result<(), String> {
        let text = bytes_of(&text);
        with_engine(|e| e.lua_tex.pdf_print.push((mode as u8, text)))
    });
    // `pdf.includefont`: pdf_init_font; a second call is fatal
    reg!(lua, p, "include_font", |f: i64| -> Result<(), String> {
        with_engine(|e| {
            if e.lua_tex.included_fonts.contains(&f) {
                e.fatal_error(&format!("error:  (pdf backend): font {f} gets initialized twice"));
                return Ok(());
            }
            e.lua_tex.included_fonts.push(f);
            let ff = e.lua_pdf_font(f)?;
            e.pdf_init_font(ff);
            Ok::<(), String>(())
        })?
    });
    reg!(lua, p, "include_char", |f: i64, c: i64| -> Result<(), String> {
        with_engine(|e| {
            let ff = e.lua_pdf_font(f)?;
            e.pdf_init_font(ff);
            if let Ok(c) = u8::try_from(c) {
                e.pdf_doc.record_font_char(ff as usize, c);
            }
            Ok::<(), String>(())
        })?
    });
    reg!(lua, p, "set_force_file", |on: bool| -> Result<(), String> { with_engine(|e| e.lua_tex.force_file = on) });
    reg!(lua, p, "set_type1_wide_mode", |v: i64| -> Result<(), String> { with_engine(|e| e.lua_tex.type1_wide_mode = v as i32) });
    reg!(lua, p, "in_late_lua", || -> Result<bool, String> { with_engine(|e| e.lua_tex.in_late_lua) });
    reg!(lua, p, "register_annot", |n: i64| -> Result<(), String> {
        with_engine(|e| {
            if let Ok(n) = i32::try_from(n) {
                e.lua_tex.late_annots.push(n);
            }
        })
    });

    lua.set_global("__ratex_pdflib", p).map_err(|e| format!("{e:?}"))?;
    lua.load(include_str!("lua_pdf.lua"))
        .set_name("=[ratex pdf]")
        .exec()
        .map_err(|e| format!("pdf library: {}", lua.get_error_message(e).message()))?;
    crate::lua_img::install(lua)
}
