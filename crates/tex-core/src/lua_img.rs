//! Engine side of LuaTeX's `img` library (limglib.c): image files are read
//! by the pdfTeX `\pdfximage` machinery (`pdf_images.rs`); the Lua side
//! keeps the image tables.

use std::any::Any;

use tex_lua::{Lua, LuaApi, LuaString, LuaTable, UserDataTrait};

use crate::engine::{Engine, ImageKind};
use crate::lua_bridge::{bytes_of, with_engine};
use crate::token::Token;

macro_rules! reg {
    ($lua:expr, $tbl:expr, $name:literal, $f:expr) => {
        $tbl.set($name, $lua.create_function($f).map_err(|e| format!("{}: {e:?}", $name))?)
            .map_err(|e| format!("{}: {e:?}", $name))?
    };
}

/// An image object (limglib.c `luatex.image`): a bare userdata whose fields
/// the Lua side keeps; the metatable holds the accessors.
struct ImageUd;

impl UserDataTrait for ImageUd {
    fn type_name(&self) -> &'static str {
        "luatex.image"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn text_tokens(s: &str) -> Vec<Token> {
    s.chars().map(|c| if c == ' ' { Token::space() } else { Token::unicode_char(12, c as u32) }).collect()
}

impl Engine {
    /// The `img` index of the image object `obj` (limglib.c `idict_array`
    /// position, from 1); the first use assigns the next one.
    pub(crate) fn lua_image_index(&mut self, obj: i32) -> i32 {
        let objs = &mut self.lua_tex.image_objs;
        match objs.iter().position(|o| *o == obj) {
            Some(i) => i as i32 + 1,
            None => {
                objs.push(obj);
                objs.len() as i32
            }
        }
    }

    /// The image object of `img` index `index`; `index` itself when it names
    /// no image.
    pub(crate) fn lua_image_obj(&self, index: i32) -> i32 {
        usize::try_from(index - 1).ok().and_then(|i| self.lua_tex.image_objs.get(i)).copied().unwrap_or(index)
    }

    /// Read the image `spec` names with `\pdfximage`; the object number of
    /// the image, `None` when the image could not be read.
    fn lua_img_scan(&mut self, spec: &LuaTable) -> Result<Option<i64>, String> {
        let get_i = |k: &str| -> Result<Option<i64>, String> { spec.get(k).map_err(|e| format!("{e:?}")) };
        let get_s = |k: &str| -> Result<Option<String>, String> {
            let v: Option<LuaString> = spec.get(k).map_err(|e| format!("{e:?}"))?;
            Ok(v.map(|v| String::from_utf8_lossy(&bytes_of(&v)).into_owned()))
        };
        let filename = get_s("filename")?.ok_or("img.scan: no filename given")?;
        let mut text = String::new();
        for (key, word) in [("width", "width"), ("height", "height"), ("depth", "depth")] {
            if let Some(v) = get_i(key)? {
                text.push_str(&format!("{word} {v}sp "));
            }
        }
        if let Some(attr) = get_s("attr")? {
            text.push_str(&format!("attr {{{attr}}} "));
        }
        if let Some(page) = get_i("page")? {
            text.push_str(&format!("page {page} "));
        }
        if let Some(cs) = get_i("colorspace")? {
            text.push_str(&format!("colorspace {cs} "));
        }
        match get_s("pagebox")?.as_deref() {
            Some("media") => text.push_str("mediabox "),
            Some("crop") => text.push_str("cropbox "),
            Some("bleed") => text.push_str("bleedbox "),
            Some("trim") => text.push_str("trimbox "),
            Some("art") => text.push_str("artbox "),
            _ => {}
        }
        let cs = self.lua_prim_cs(b"saveimageresource");
        let mut toks = vec![Token::from_cs(cs)];
        toks.extend(text_tokens(&text));
        toks.push(Token::unicode_char(1, '{' as u32));
        toks.extend(text_tokens(&filename));
        toks.push(Token::unicode_char(2, '}' as u32));
        let before = self.pdf_last_ximage;
        self.pdf_last_ximage = 0;
        self.lua_run_tokens(toks);
        let obj = self.pdf_last_ximage;
        if obj == 0 {
            self.pdf_last_ximage = before;
            return Ok(None);
        }
        Ok(Some(i64::from(obj)))
    }
}

pub(crate) fn install(lua: &mut Lua) -> Result<(), String> {
    let t: LuaTable = lua.create_table().map_err(|e| format!("{e:?}"))?;

    let meta: LuaTable = lua.create_table().map_err(|e| format!("{e:?}"))?;
    t.set("meta", meta.clone()).map_err(|e| format!("{e:?}"))?;
    let newud = lua
        .create_callback(move |cx| {
            let ud = cx.create_userdata(ImageUd)?;
            let value = cx.pack(&ud)?;
            value.set_metatable(Some(&meta))?;
            cx.push(value)
        })
        .map_err(|e| format!("{e:?}"))?;
    t.set("newud", newud).map_err(|e| format!("{e:?}"))?;
    reg!(lua, t, "fatal", |message: String| -> Result<(), String> {
        with_engine(|e| e.fatal_error(&message))
    });
    reg!(lua, t, "scan", |spec: LuaTable| -> Result<Option<i64>, String> {
        with_engine(|e| e.lua_img_scan(&spec))?
    });
    reg!(lua, t, "info", |obj: i64, field: String| -> Result<(Option<String>, i64, i64, i64, i64), String> {
        with_engine(|e| {
            let Some(info) = i32::try_from(obj).ok().and_then(|o| e.pdf_images.get(&o)) else {
                return (None, 0, 0, 0, 0);
            };
            let n = |v: i32| (None, i64::from(v), 0, 0, 0);
            match field.as_str() {
                "path" => (Some(info.path.clone()), 0, 0, 0, 0),
                "type" => (
                    Some(match info.kind {
                        ImageKind::Pdf => "pdf",
                        ImageKind::Png | ImageKind::Svg => "png",
                        ImageKind::Jpeg => "jpg",
                    }.to_string()),
                    0, 0, 0, 0,
                ),
                "width" => n(info.width),
                "height" => n(info.height),
                "depth" => n(info.depth),
                "xsize" => n(info.image_width),
                "ysize" => n(info.image_height),
                "rotation" => n(info.rotate),
                "bbox" => (
                    None,
                    i64::from(info.bbox[0]),
                    i64::from(info.bbox[1]),
                    i64::from(info.bbox[2]),
                    i64::from(info.bbox[3]),
                ),
                "pages" => n(e.pdf_last_ximage_pages),
                "colordepth" => n(e.pdf_backend.last_ximage_colordepth),
                _ => (None, 0, 0, 0, 0),
            }
        })
    });
    reg!(lua, t, "write_now", |obj: i64| -> Result<(), String> { with_engine(|e| e.write_ximage(obj as i32)) });
    reg!(lua, t, "index_of", |obj: i64| -> Result<i64, String> { with_engine(|e| i64::from(e.lua_image_index(obj as i32))) });
    reg!(lua, t, "obj_of", |index: i64| -> Result<Option<i64>, String> {
        with_engine(|e| usize::try_from(index - 1).ok().and_then(|i| e.lua_tex.image_objs.get(i)).map(|o| i64::from(*o)))
    });
    reg!(lua, t, "ref", |obj: i64| -> Result<(), String> {
        // \pdfrefximage <obj>: the image box joins the current list
        with_engine(|e| {
            let cs = e.lua_prim_cs(b"useimageresource");
            let mut toks = vec![Token::from_cs(cs)];
            toks.extend(text_tokens(&format!("{obj} ")));
            e.lua_run_tokens(toks);
            Ok::<(), String>(())
        })?
    });

    lua.set_global("__ratex_imglib", t).map_err(|e| format!("{e:?}"))?;
    lua.load(include_str!("lua_img.lua"))
        .set_name("=[ratex img]")
        .exec()
        .map_err(|e| format!("img library: {}", lua.get_error_message(e).message()))?;
    Ok(())
}
