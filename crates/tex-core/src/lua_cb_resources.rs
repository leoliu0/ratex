//! LuaTeX's resource file callbacks: `find_font_file`/`read_font_file`
//! (TFM and OFM), `find_vf_file`/`read_vf_file`, `find_map_file`/
//! `read_map_file`, `find_enc_file`/`read_enc_file`, `find_type1_file`/
//! `read_type1_file`, `find_truetype_file`/`read_truetype_file`,
//! `find_opentype_file`/`read_opentype_file`, `find_data_file`/
//! `read_data_file`, `find_image_file`, `find_output_file` and
//! `find_format_file` (`tfmofm.c`, `vfovf.c`, `mapfile.c`, `writet1.c`,
//! `writettf.c`, `writetype0.c`, `writetype2.c`, `pdfobj.c`,
//! `texfileio.c`).
//!
//! luatex asks the callbacks at the moment it needs a file: a TFM (and its
//! VF) for every fresh `\font`, the default map when the first font is
//! initialized for the PDF page or `\pdfmapfile` runs, and the encodings
//! and font programs once the PDF is finished. Once any of these callbacks
//! (or `start_file`) is registered the engine follows that model
//! ("resource mode"): no cache stands between TeX and the callbacks, a TFM
//! is read independently of the map, and the map entry of a font is
//! applied when its PDF font is initialized. Without such a callback the
//! classic loaders run untouched.

use std::rc::Rc;

use tex_kpse::Format;

use crate::engine::{Engine, EngineKind};
use crate::lua_callbacks::Cb;
use crate::lua_cb_files::{filetype, ReadFile};
use crate::lua_font::{FontEmbedding, FontFormat, LuaPdfKind};
use crate::tfm::{parse_tfm, Font};

/// The callbacks that switch the font loaders to luatex's resource model.
const RESOURCE_CBS: [Cb; 16] = [
    Cb::FindFontFile,
    Cb::ReadFontFile,
    Cb::FindVfFile,
    Cb::ReadVfFile,
    Cb::FindMapFile,
    Cb::ReadMapFile,
    Cb::FindEncFile,
    Cb::ReadEncFile,
    Cb::FindType1File,
    Cb::ReadType1File,
    Cb::FindTruetypeFile,
    Cb::ReadTruetypeFile,
    Cb::FindOpentypeFile,
    Cb::ReadOpentypeFile,
    Cb::StartFile,
    Cb::StopFile,
];

/// What was found and read for one resource file.
pub(crate) enum Got {
    /// `find_*_file` returned nothing (luatex: `NULL`).
    NotFound,
    /// The file was found but could not be opened.
    NotOpened(String),
    /// The file was opened and holds no data.
    Empty(String),
    /// The file name used and its contents.
    Data(String, Vec<u8>),
}

/// State of the resource callbacks for one job.
#[derive(Default)]
pub(crate) struct LuaResources {
    /// Fonts loaded in resource mode whose map entry is applied when the
    /// PDF font is initialized (`pdf_init_font` → `getfontmap`).
    pub(crate) pending_map: crate::FxHashSet<u16>,
    /// The map files were taken over by the callbacks (`create_avl_trees`).
    map_started: bool,
    /// luatex `mitem->line`: the default map is still to be read.
    default_pending: bool,
    /// Encodings read through the callbacks, by name (`fe_tree`).
    encs: crate::FxHashMap<String, Rc<[String]>>,
    /// Font programs already requested of the callbacks (`ff_tree`).
    programs: crate::FxHashSet<String>,
    /// `find_output_file` ran for this job (`ensure_output_file_open`).
    output_opened: bool,
    /// The file name `find_output_file` chose for the PDF.
    output_name: Option<String>,
    /// Images whose file was reported through `start_file`.
    images_reported: crate::FxHashSet<i32>,
    /// The checksum word of the TFM files read, by font name.
    pub(crate) checksums: crate::FxHashMap<String, u32>,
    /// The virtual font `do_vf` fetched through the callbacks, handed to
    /// `font.read_vf` when it merges the packets into the font table.
    pub(crate) vf_override: Option<Vec<u8>>,
    /// The map file being scanned (luatex `cur_file_name`).
    pub(crate) map_file: Option<String>,
    /// `fd_objnum` that `font_descriptor_objnum_provider` chose for the
    /// descriptor of each font.
    pub(crate) descriptor_objnums: crate::FxHashMap<u16, i32>,
}

/// `do_vf` on a font table of unknown type: the name to look for.
const VF_NAME: &str = r#"
local t = ...
local ty = t.type
if ty == nil or ty == "unknown" then return t.name end
"#;

/// `do_vf`: a virtual font replaces the packets of the characters the
/// table has and supplies the local fonts.
const VF_MERGE: &str = r#"
local t = ...
local v = font.read_vf(t.name, t.size or t.designsize or 0)
if not v then return end
t.type = "virtual"
t.fonts = v.fonts
local chars = t.characters
if chars and v.characters then
  for c, vc in pairs(v.characters) do
    local tc = chars[c]
    if tc then tc.commands = vc.commands end
  end
end
"#;

impl crate::fontload::FontLoader {
    /// Turn a virtual font file into the base of `font`: its glyphs come
    /// from the VF's local fonts, so the map's program is dropped.
    pub(crate) fn adopt_vf(&mut self, font: &mut Font, name: &str, resolved_name: &str, data: &[u8]) -> bool {
        let Some(vf) = self.parse_vf(data, font.at_size) else {
            return false;
        };
        font.map_fontname = None;
        font.type1_path = None;
        font.enc_name = None;
        font.encoding = None;
        self.vf_fonts.insert((name.to_string(), font.at_size), vf.clone());
        self.vf_fonts.insert((resolved_name.to_string(), font.at_size), vf);
        true
    }

    /// Read `path` from disk (or the embedded tree) and record the file as
    /// a dependency of the job.
    fn read_path_dependency(&mut self, path: &str) -> Option<Vec<u8>> {
        if tex_kpse::embedded_tree::is_embedded_path(path) {
            return tex_kpse::embedded_tree::read(path);
        }
        let data = tex_kpse::fs::read(path).ok()?;
        let path = std::path::PathBuf::from(path);
        self.dependency_file_digests.push((
            path.clone(),
            data.len() as u64,
            crate::fontload::dependency_content_hash(&data),
        ));
        self.dependency_files.push(path);
        Some(data)
    }
}

/// The font name of a TFM file name (`read_tfm_info`: the base name
/// without a `.tfm`/`.ofm` suffix).
fn tfm_base(name: &str) -> &str {
    let base = name.rsplit('/').next().unwrap_or(name);
    base.strip_suffix(".tfm").or_else(|| base.strip_suffix(".ofm")).unwrap_or(base)
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Which luatex writer embeds a font program.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Program {
    /// `writet1`
    Type1,
    /// `writettf`
    TrueType,
    /// `writeotf`
    OpenType,
    /// `writetype0`: a CID-keyed OpenType (or TrueType) font
    CidOpenType,
    /// `writetype2`: a CID-keyed TrueType font
    CidTrueType,
}

impl Engine {
    /// Whether luatex's resource model is in force: a LuaTeX engine with a
    /// resource callback (or `start_file`/`stop_file`) registered.
    #[inline]
    pub(crate) fn lua_res_mode(&self) -> bool {
        self.engine_kind == EngineKind::LuaTeX && RESOURCE_CBS.iter().any(|&cb| self.cb_defined(cb))
    }

    /// luatex `formatted_error(what, ...)`: a fatal error, prefixed with the
    /// file being processed when there is one.
    pub(crate) fn lua_res_error(&mut self, file: Option<&str>, what: &str, message: &str) {
        let text = match file {
            Some(file) => format!("error:  (file {file}) ({what}): {message}"),
            None => format!("error:  ({what}): {message}"),
        };
        self.fatal_error(&text);
    }

    // ---- finding and reading ------------------------------------------

    /// `luatex_find_file`: the `find_*_file` callback `find`, else the
    /// kpathsea search for `name` in `formats`.
    fn res_find(&mut self, find: Cb, name: &str, formats: &[Format]) -> Option<String> {
        if self.cb_defined(find) {
            return self.lua_find_file(find, None, name.as_bytes()).flatten().map(|n| lossy(&n));
        }
        for &fmt in formats {
            let resolved = self.font_loader.kpse.find(name, fmt);
            self.font_loader.record_lookup_dependency(name, fmt, resolved.as_deref());
            if let Some(path) = self.lua_kpse_find(name, fmt) {
                // kpathsea reports a file of the working directory as `./name`
                let local = !path.starts_with('/') && !path.starts_with("./") && !path.starts_with("../");
                return Some(if local { format!("./{path}") } else { path });
            }
        }
        None
    }

    /// The `read_*_file` callback `read`, else the file itself.
    fn res_read(&mut self, read: Cb, fname: &str, formats: &[Format]) -> Got {
        match self.lua_read_file_ex(read, fname.as_bytes()) {
            Some(ReadFile::Data(d)) => Got::Data(fname.to_string(), d),
            Some(ReadFile::NotOpened) => Got::NotOpened(fname.to_string()),
            Some(ReadFile::Empty) => Got::Empty(fname.to_string()),
            None => {
                let mut data = self.font_loader.read_path_dependency(fname);
                if data.is_none() {
                    data = formats.iter().find_map(|&fmt| self.font_loader.read_dependency(fname, fmt));
                }
                match data {
                    Some(d) if !d.is_empty() => Got::Data(fname.to_string(), d),
                    Some(_) => Got::Empty(fname.to_string()),
                    None => Got::NotOpened(fname.to_string()),
                }
            }
        }
    }

    /// `find_*_file` followed by `read_*_file`.
    fn res_open(&mut self, find: Cb, read: Cb, name: &str, formats: &[Format]) -> Got {
        match self.res_find(find, name, formats) {
            Some(fname) => self.res_read(read, &fname, formats),
            None => Got::NotFound,
        }
    }

    // ---- TFM and VF -----------------------------------------------------

    /// tfmofm.c `open_tfm_file` and the parse of `read_tfm_info`: the font
    /// `name` at `s` (`luatex` convention: an `at` size, `-1000` for the
    /// design size, `-n` for `scaled n`), with the bytes it was made from.
    pub(crate) fn lua_tfm_parse(&mut self, name: &str, s: i32) -> Option<(Font, String, Vec<u8>)> {
        let Got::Data(_, data) = self.res_open(Cb::FindFontFile, Cb::ReadFontFile, name, &[Format::Tfm]) else {
            return None;
        };
        let base = tfm_base(name).to_string();
        let at = if s >= 0 {
            s
        } else if s == -1000 {
            0
        } else {
            let dsize = parse_tfm(&data, &base, 0).ok()?.dsize;
            crate::scaled::xn_over_d(dsize, -s, 1000)
        };
        let font = parse_tfm(&data, &base, at).ok()?;
        let checksum = data.get(24..28).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
        self.lua_res.checksums.insert(base.clone(), checksum);
        Some((font, base, data))
    }

    /// vfovf.c `open_vf_file`: the bytes of the virtual font `name`.
    pub(crate) fn lua_vf_data(&mut self, name: &str) -> Option<Vec<u8>> {
        let fname = self.res_find(Cb::FindVfFile, name, &[Format::Vf])?;
        if fname.is_empty() {
            return None;
        }
        match self.res_read(Cb::ReadVfFile, &fname, &[Format::Vf]) {
            Got::Data(_, data) => Some(data),
            _ => None,
        }
    }

    /// `do_define_font` without a `define_font` callback in resource mode:
    /// `read_tfm_info` then `do_vf`, and no cache between them and TeX.
    pub(crate) fn lua_define_tfm(&mut self, name: &str, s: i32) -> Option<Rc<Font>> {
        let (mut font, base, _) = self.lua_tfm_parse(name, s)?;
        self.lua_attach_vf(&mut font, name, &base);
        Some(Rc::new(font))
    }

    /// vfovf.c `do_vf`: a virtual font found for `base` replaces the real
    /// font.
    fn lua_attach_vf(&mut self, font: &mut Font, name: &str, base: &str) {
        let adopted = match self.lua_vf_data(base) {
            Some(data) => self.font_loader.adopt_vf(font, name, base, &data),
            None => false,
        };
        if !adopted {
            self.font_loader.vf_fonts.remove(&(name.to_string(), font.at_size));
            self.font_loader.vf_fonts.remove(&(base.to_string(), font.at_size));
        }
    }

    /// vfovf.c `do_vf` for a font table the `define_font` callback returned
    /// without a `type`: `find_vf_file` and `read_vf_file` run for the font's
    /// name, a virtual font found turns the table into a virtual font.
    pub(crate) fn lua_do_vf_table(&mut self, t: &tex_lua::LuaTable) {
        if !self.lua_res_mode() {
            return;
        }
        let name = self.lua_run(|lua| {
            use tex_lua::LuaApi;
            let found: Option<tex_lua::Value> =
                lua.lua.load(VF_NAME).call(t.clone()).map_err(|e| lua.lua.get_error_message(e).message().to_string())?;
            Ok(found.and_then(|v| crate::lua_node_lib::value_bytes(&v)))
        });
        let Some(name) = name.ok().flatten() else {
            return;
        };
        let Some(data) = self.lua_vf_data(&lossy(&name)) else {
            return;
        };
        self.lua_res.vf_override = Some(data);
        let merged = self.lua_run(|lua| {
            use tex_lua::LuaApi;
            lua.lua.load(VF_MERGE).call::<_, ()>(t.clone()).map_err(|e| lua.lua.get_error_message(e).message().to_string())
        });
        self.lua_res.vf_override = None;
        if let Err(err) = merged {
            self.error(&format!("LuaTeX error {err}"));
        }
    }

    /// The local font of a virtual font at an `at` size
    /// (`FontLoader::load_tfm` or, in resource mode, the callbacks).
    pub(crate) fn lua_load_tfm_at(&mut self, name: &str, at: i32) -> Option<Rc<Font>> {
        if self.lua_res_mode() {
            self.lua_define_tfm(name, at.max(0))
        } else {
            self.font_loader.load_tfm(name, at)
        }
    }

    /// Append a freshly loaded TFM font; in resource mode its map entry
    /// follows with `pdf_init_font`.
    pub(crate) fn lua_push_tfm_font(&mut self, font: Rc<Font>, cs: crate::token::CsId) -> u16 {
        let id = self.push_engine_font(font, cs);
        if self.lua_res_mode() {
            self.lua_res.pending_map.insert(id);
            // do_vf loads the local fonts of a virtual font right away
            self.ensure_vf_bases(id);
        }
        id
    }

    // ---- font maps ------------------------------------------------------

    /// The first map operation hands the map over to the callbacks: luatex
    /// starts from an empty `tfm_tree` with `pdftex.map` queued.
    fn lua_res_map_start(&mut self) {
        if !self.lua_res.map_started {
            self.lua_res.map_started = true;
            self.lua_res.default_pending = true;
            self.font_loader.reset_map_for_callbacks();
        }
    }

    /// `process_map_item`: an item without `+`, `=` or `-` drops the
    /// default map file, any other reads it first.
    pub(crate) fn lua_res_map_item(&mut self, flush_default: bool) {
        self.lua_res_map_start();
        if flush_default {
            self.lua_res.default_pending = false;
        } else {
            self.lua_res_map_default();
        }
    }

    /// `getfontmap`: read the default map if that is still to be done.
    pub(crate) fn lua_res_map_default(&mut self) {
        self.lua_res_map_start();
        if std::mem::replace(&mut self.lua_res.default_pending, false) {
            if let Some(text) = self.lua_map_text("pdftex.map") {
                self.font_loader.map.add_layer(text, crate::fontmap::MapMode::DupIgnore, &|_| false, false);
                self.lua_map_text_done();
            }
        }
    }

    /// mapfile.c `fm_read_info` for a map file: `find_map_file`,
    /// `read_map_file`, then the start of the `start_file` report (the
    /// caller closes it with `stop_file` once the lines are processed).
    pub(crate) fn lua_map_text(&mut self, name: &str) -> Option<String> {
        let cur = self.res_find(Cb::FindMapFile, name, &[Format::Map])?;
        match self.res_read(Cb::ReadMapFile, &cur, &[Format::Map]) {
            Got::Data(_, data) => {
                self.lua_report_start_file(filetype::MAP, cur.as_bytes());
                self.lua_res.map_file = Some(cur);
                Some(lossy(&data))
            }
            Got::NotOpened(_) => {
                self.warning_at(&format!("(file {cur}) (map file): cannot open font map file '{cur}'"), None);
                None
            }
            Got::Empty(_) | Got::NotFound => None,
        }
    }

    /// The end of a map file's lines: `stop_file`.
    pub(crate) fn lua_map_text_done(&mut self) {
        self.lua_res.map_file = None;
        self.lua_report_stop_file(filetype::MAP);
    }

    /// A warning of the map file scanner (`formatted_warning("map file")`),
    /// prefixed with the file being read.
    pub(crate) fn lua_map_warning(&mut self, message: &str) {
        let text = match &self.lua_res.map_file {
            Some(file) => format!("(file {file}) (map file): {message}"),
            None => format!("(map file): {message}"),
        };
        self.warning_at(&text, None);
    }

    /// `pdf_init_font` → `getfontmap`: the font's map entry is found in the
    /// map the callbacks read.
    pub(crate) fn lua_res_init_font(&mut self, f: u16) {
        self.lua_res_map_default();
        if !self.lua_res.pending_map.remove(&f) {
            return;
        }
        self.lua_apply_font_map(f);
    }

    fn lua_apply_font_map(&mut self, f: u16) {
        let Some(font) = self.eqtb.fonts.get(usize::from(f)) else {
            return;
        };
        if font.lua.as_ref().is_some_and(|lua| lua.filename.is_some())
            || self.font_loader.vf_fonts.contains_key(&(font.tfm_name.clone(), font.at_size))
        {
            return;
        }
        let Some(entry) = self.font_loader.map.get(&font.tfm_name) else {
            return;
        };
        let font = Rc::make_mut(&mut self.eqtb.fonts[usize::from(f)]);
        font.map_fontname = (!entry.fontname.is_empty()).then(|| entry.fontname.clone());
        font.enc_name = entry.enc_file.clone();
        font.encoding = None;
        font.type1_path = entry.pfb.clone().filter(|pfb| !pfb.ends_with(".vf"));
    }

    // ---- encodings and font programs -----------------------------------

    /// writet1.c `load_enc_file`.
    fn lua_load_enc(&mut self, name: &str) -> Option<Rc<[String]>> {
        if let Some(enc) = self.lua_res.encs.get(name) {
            return Some(enc.clone());
        }
        let Some(cur) = self.res_find(Cb::FindEncFile, name, &[Format::Enc]) else {
            self.lua_res_error(None, "type 1", &format!("cannot find encoding file '{name}' for reading"));
            return None;
        };
        let data = match self.res_read(Cb::ReadEncFile, &cur, &[Format::Enc]) {
            Got::Data(_, data) => data,
            _ => {
                self.lua_res_error(Some(&cur), "type 1", &format!("cannot open encoding file '{cur}' for reading"));
                return None;
            }
        };
        self.lua_report_start_file(filetype::MAP, cur.as_bytes());
        let text = lossy(&data);
        let parsed = crate::fontload::parse_enc_names(&text);
        self.lua_report_stop_file(filetype::MAP);
        let Some(names) = parsed else {
            let line = text.lines().find(|l| !l.starts_with('%') && !l.is_empty()).unwrap_or("");
            self.lua_res_error(Some(&cur), "type 1", &format!("invalid encoding vector (a name or '[' missing): '{line}'"));
            return None;
        };
        let enc: Rc<[String]> = names.into();
        self.lua_res.encs.insert(name.to_string(), enc.clone());
        Some(enc)
    }

    /// The program of `font`, the writer that embeds it and whether it is
    /// subsetted; `None` for fonts without an embedded program.
    fn lua_font_program_job(&self, font: &Font) -> Option<(String, Program, bool)> {
        if let Some(lua) = font.lua.as_ref().filter(|lua| lua.filename.is_some()) {
            let name = lossy(lua.filename.as_ref()?);
            if lua.embedding == FontEmbedding::No {
                return None;
            }
            let subset = lua.embedding != FontEmbedding::Full;
            if lua.pdf_kind() == LuaPdfKind::Cid {
                let truetype = match lua.format {
                    FontFormat::TrueType => true,
                    FontFormat::OpenType => false,
                    _ => name.to_ascii_lowercase().ends_with(".ttf") || name.to_ascii_lowercase().ends_with(".ttc"),
                };
                return Some((name, if truetype { Program::CidTrueType } else { Program::CidOpenType }, subset));
            }
            return Some((name, Program::Type1, subset));
        }
        let name = font.type1_path.clone().filter(|p| !p.ends_with(".vf"))?;
        let lower = name.to_ascii_lowercase();
        let program = if lower.ends_with(".ttf") || lower.ends_with(".ttc") {
            Program::TrueType
        } else if lower.ends_with(".otf") {
            Program::OpenType
        } else {
            Program::Type1
        };
        let subset = self.font_loader.map.get(&font.tfm_name).is_none_or(|entry| !entry.full_download);
        Some((name, program, subset))
    }

    /// `write_fontfile` for one program, with luatex's callbacks and errors.
    /// The bytes are left where the PDF writer looks for them.
    fn lua_load_program(&mut self, name: &str, program: Program, subset: bool) {
        let category = if subset { filetype::SUBSET } else { filetype::FONT };
        let data = match program {
            Program::Type1 => {
                // check_ff_exist asks for the bare name once; the writer asks
                // again for the path that came back
                let found = if self.cb_defined(Cb::FindType1File) {
                    self.lua_find_file(Cb::FindType1File, None, name.as_bytes())
                        .flatten()
                        .filter(|p| !p.is_empty())
                        .map(|p| lossy(&p))
                } else {
                    self.res_find(Cb::FindType1File, name, &[Format::Type1])
                };
                let Some(ff_path) = found else {
                    self.lua_res_error(None, "type 1", &format!("cannot open file for reading '{name}'"));
                    return;
                };
                let cur = if self.cb_defined(Cb::FindType1File) {
                    self.lua_find_file(Cb::FindType1File, None, ff_path.as_bytes()).flatten().map(|p| lossy(&p))
                } else {
                    Some(ff_path.clone())
                };
                let Some(cur) = cur else {
                    self.lua_res_error(None, "type 1", &format!("cannot open file for reading '{ff_path}'"));
                    return;
                };
                match self.res_read(Cb::ReadType1File, &cur, &[Format::Type1]) {
                    Got::Data(_, data) => {
                        self.lua_report_start_file(category, cur.as_bytes());
                        self.lua_report_stop_file(category);
                        data
                    }
                    Got::NotOpened(_) | Got::Empty(_) => {
                        self.lua_report_start_file(category, cur.as_bytes());
                        self.lua_res_error(Some(&cur), "type 1", "unexpected end of file");
                        return;
                    }
                    Got::NotFound => return,
                }
            }
            Program::TrueType => {
                let Some(cur) = self.res_find(Cb::FindTruetypeFile, name, &[Format::Truetype]) else {
                    self.lua_res_error(None, "ttf font", &format!("cannot find font file for reading '{name}'"));
                    return;
                };
                match self.res_read(Cb::ReadTruetypeFile, &cur, &[Format::Truetype]) {
                    Got::Data(_, data) => data,
                    _ => {
                        self.lua_res_error(Some(&cur), "ttf font", &format!("cannot open font file for reading '{cur}'"));
                        return;
                    }
                }
            }
            Program::OpenType => {
                let Some(cur) = self.res_find_opentype(name) else {
                    self.lua_res_error(None, "otf font", &format!("cannot find font file for reading '{name}'"));
                    return;
                };
                match self.res_read(Cb::ReadOpentypeFile, &cur, &[Format::Otf, Format::Truetype]) {
                    Got::Data(_, data) => data,
                    _ => {
                        self.lua_res_error(Some(&cur), "otf font", &format!("cannot open font file for reading '{cur}'"));
                        return;
                    }
                }
            }
            Program::CidOpenType | Program::CidTrueType => {
                let (what, fallback) = if program == Program::CidOpenType { ("type 0", true) } else { ("type 2", false) };
                let mut found = self.res_find_opentype(name);
                if found.is_none() && fallback {
                    found = self.res_find(Cb::FindTruetypeFile, name, &[Format::Truetype]);
                }
                let Some(cur) = found else {
                    self.lua_res_error(None, what, &format!("cannot find file '{name}'"));
                    return;
                };
                match self.res_read(Cb::ReadOpentypeFile, &cur, &[Format::Otf, Format::Truetype]) {
                    Got::Data(_, data) => {
                        self.lua_report_start_file(category, cur.as_bytes());
                        self.lua_report_stop_file(category);
                        data
                    }
                    _ => {
                        self.lua_res_error(Some(&cur), what, &format!("cannot find file '{cur}'"));
                        return;
                    }
                }
            }
        };
        self.font_loader.file_bytes_cache.insert(name.to_string(), Rc::new(data));
    }

    /// `find_opentype_file`; without the callback the OpenType and then
    /// the TrueType search path.
    fn res_find_opentype(&mut self, name: &str) -> Option<String> {
        self.res_find(Cb::FindOpentypeFile, name, &[Format::Otf, Format::Truetype])
    }

    /// The fonts the PDF embeds (the set `embed_used_fonts` walks): every
    /// font a page or form shows plus the fonts `\pdfincludechars` marked.
    fn lua_embedded_fonts(&self) -> std::collections::BTreeSet<u16> {
        let mut used = std::collections::BTreeSet::new();
        for fonts in self.pdf_doc.pages.iter().map(|p| &p.fonts).chain(self.pdf_doc.form_fonts.iter().map(|(_, fonts)| fonts)) {
            used.extend(fonts.iter().map(|&(key, _)| key as u16));
        }
        for fid in self.pdf_backend.font_ff.keys().copied() {
            let marked = self.pdf_doc.font_chars.get(&usize::from(fid)).is_some_and(|chars| chars.iter().any(|&word| word != 0));
            if marked {
                used.insert(fid);
            }
        }
        used
    }

    /// What luatex does with `finish_pdffile` behind it: the encodings
    /// (`create_fontdictionary`) and the font programs (`write_fontfile`)
    /// of every PDF font are read through the callbacks, font by font.
    /// Their bytes replace what the loaders would find.
    pub(crate) fn lua_resolve_font_resources(&mut self) -> bool {
        let resources = self.lua_res_mode();
        let provider = self.engine_kind == EngineKind::LuaTeX && self.cb_defined(Cb::FontDescriptorObjnumProvider);
        if !(resources || provider) || self.stopped_on_error {
            return true;
        }
        let used = self.lua_embedded_fonts();
        if used.is_empty() {
            return true;
        }
        if resources {
            for &fid in &used {
                self.lua_res_init_font(fid);
            }
        }
        // do_pdf_font: the encoding of each font, and the program of every
        // font that is not a Type 1 font; their descriptors are written
        // right away
        let mut type1_jobs: Vec<(String, bool, u16)> = Vec::new();
        let mut type1_fonts: crate::FxHashMap<String, Vec<u16>> = crate::FxHashMap::default();
        for &fid in &used {
            let Some(font) = self.eqtb.fonts.get(usize::from(fid)).cloned() else {
                continue;
            };
            if resources {
                if let Some(enc_name) = font.enc_name.clone() {
                    let Some(enc) = self.lua_load_enc(&enc_name) else {
                        return false;
                    };
                    Rc::make_mut(&mut self.eqtb.fonts[usize::from(fid)]).encoding = Some(enc);
                }
            }
            let job = self.lua_font_program_job(&font);
            if let Some((name, Program::Type1, subset)) = &job {
                type1_fonts.entry(name.clone()).or_default().push(fid);
                if self.lua_res.programs.insert(name.clone()) {
                    type1_jobs.push((name.clone(), *subset, fid));
                }
                continue;
            }
            if job.is_some() || font.map_fontname.is_some() {
                // writefont.c `create_fontdictionary`: a font file of another
                // kind, or no file at all (a built-in font)
                self.lua_descriptor_objnum(fid, &font);
            }
            let Some((name, program, subset)) = job else {
                continue;
            };
            if resources && self.lua_res.programs.insert(name.clone()) {
                self.lua_load_program(&name, program, subset);
                if self.stopped_on_error {
                    return false;
                }
            }
        }
        // write_fontdescriptors: the Type 1 programs, by the name of the
        // font file (the `fd_tree` order)
        type1_jobs.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        for (name, subset, fid) in type1_jobs {
            if let Some(font) = self.eqtb.fonts.get(usize::from(fid)).cloned() {
                let number = self.lua_descriptor_objnum(fid, &font);
                if number != 0 {
                    for &other in &type1_fonts[&name] {
                        self.lua_res.descriptor_objnums.insert(other, number);
                    }
                }
            }
            if resources {
                self.lua_load_program(&name, Program::Type1, subset);
                if self.stopped_on_error {
                    return false;
                }
            }
        }
        true
    }

    /// writefont.c `write_fontdescriptor`: `font_descriptor_objnum_provider`
    /// (`"S->d"`) is asked for the object number of the descriptor of a
    /// font, named like `preset_fontname` does. A number is the object the
    /// descriptor is written as (the callback's own responsibility); a
    /// result of another type is reported and, like 0, leaves the number to
    /// luatex.
    fn lua_descriptor_objnum(&mut self, fid: u16, font: &Font) -> i32 {
        if self.engine_kind != EngineKind::LuaTeX || !self.cb_defined(Cb::FontDescriptorObjnumProvider) {
            return 0;
        }
        let name = match (&font.map_fontname, font.lua.as_ref().and_then(|lua| lua.fullname.as_ref())) {
            (Some(ps), _) => ps.clone().into_bytes(),
            (None, Some(full)) => full.clone(),
            (None, None) => font.tfm_name.clone().into_bytes(),
        };
        let rets = self.lua_cb_call(
            Cb::FontDescriptorObjnumProvider,
            "font_descriptor_objnum_provider",
            vec![crate::lua_callbacks::CbArg::Str(name)],
        );
        let number = match rets.as_deref().map(|r| r.first()) {
            Some(Some(crate::lua_callbacks::CbRet::Int(n))) => *n as i32,
            // lua_tointeger: a float is a number only when it holds an integer
            Some(Some(crate::lua_callbacks::CbRet::Num(n))) => {
                if n.fract() == 0.0 && n.abs() < f64::from(i32::MAX) { *n as i32 } else { 0 }
            }
            Some(other) => {
                eprintln!("callback should return a number, not: {}", other.map_or("nil", |r| r.type_name()));
                0
            }
            None => 0,
        };
        if number != 0 {
            self.lua_res.descriptor_objnums.insert(fid, number);
        }
        number
    }

    // ---- data files, images, the output file, formats -------------------

    /// pdfobj.c `\pdfobj file`: the contents of the data file `name`
    /// through `find_data_file`/`read_data_file`. `None`: neither is
    /// registered, the classic search applies; `Some(Err(name))`: open
    /// `name` the classic way; `Some(Ok(bytes))`: the callback's data.
    pub(crate) fn lua_data_file(&mut self, name: &str) -> Option<Result<Vec<u8>, String>> {
        if self.engine_kind != EngineKind::LuaTeX || !(self.cb_defined(Cb::FindDataFile) || self.cb_defined(Cb::ReadDataFile)) {
            return None;
        }
        self.lua_open_output();
        let found = self.res_find(Cb::FindDataFile, name, &[Format::Tex]);
        let fnam = match found {
            Some(fnam) if self.cb_defined(Cb::ReadDataFile) => fnam,
            Some(fnam) => return Some(Err(fnam)),
            None => return Some(Err(name.to_string())),
        };
        match self.lua_read_file_ex(Cb::ReadDataFile, fnam.as_bytes()) {
            Some(ReadFile::Data(data)) => Some(Ok(data)),
            Some(ReadFile::Empty) => {
                self.lua_res_error(None, "pdf backend", "empty file for embedding");
                Some(Ok(Vec::new()))
            }
            _ => {
                self.lua_res_error(None, "pdf backend", "cannot open file for embedding");
                Some(Ok(Vec::new()))
            }
        }
    }

    /// `find_image_file` (`"S->S"`): the path luatex reads the image
    /// `name` from. `None`: no callback; `Some(None)`: it found nothing,
    /// the error was reported.
    pub(crate) fn lua_image_file(&mut self, name: &str) -> Option<Option<String>> {
        if self.engine_kind != EngineKind::LuaTeX || !self.cb_defined(Cb::FindImageFile) {
            return None;
        }
        let rets = self.lua_cb_call(
            Cb::FindImageFile,
            "find_image_file",
            vec![crate::lua_callbacks::CbArg::Str(name.as_bytes().to_vec())],
        );
        match rets.as_deref().and_then(|r| r.first()) {
            Some(crate::lua_callbacks::CbRet::Str(path)) => Some(Some(lossy(path))),
            other => {
                if rets.is_some() {
                    let t = other.map_or("nil", |r| r.type_name());
                    eprintln!("callback should return a string, not: {t}");
                }
                self.lua_res_error(Some(name), "pdf backend", &format!("cannot find image file '{name}'"));
                Some(None)
            }
        }
    }

    /// The image files a page writes are reported when the page ends
    /// (`start_file`/`stop_file` with the image category).
    pub(crate) fn lua_page_image_files(&mut self) {
        if !(self.cb_defined(Cb::StartFile) || self.cb_defined(Cb::StopFile)) {
            return;
        }
        let Some(page) = self.pdf_doc.pages.last() else {
            return;
        };
        let objects = page.ximages.clone();
        for obj in objects {
            let Some(image) = self.pdf_images.get(&obj) else {
                continue;
            };
            if image.kind == crate::engine::ImageKind::Pdf || !self.lua_res.images_reported.insert(obj) {
                continue;
            }
            let path = image.path.clone();
            self.lua_report_start_file(filetype::IMAGE, path.as_bytes());
            self.lua_report_stop_file(filetype::IMAGE);
        }
    }

    /// pdfgen.c `ensure_output_file_open`: `find_output_file` names the
    /// PDF the first time the job writes to it; a result that is not a
    /// non-empty string leaves luatex unable to write the file.
    pub(crate) fn lua_open_output(&mut self) {
        if self.engine_kind != EngineKind::LuaTeX || std::mem::replace(&mut self.lua_res.output_opened, true) {
            return;
        }
        if !self.cb_defined(Cb::FindOutputFile) {
            return;
        }
        let name = format!("{}.pdf", self.job_name);
        match self.lua_find_file(Cb::FindOutputFile, None, name.as_bytes()).flatten().filter(|n| !n.is_empty()) {
            Some(chosen) => self.lua_res.output_name = Some(lossy(&chosen)),
            None => {
                let text = format!("I can't write on file `{name}'.\nPlease type another file name for output");
                self.fatal_error(&text);
            }
        }
    }

    /// The file `find_output_file` chose for the PDF, when it chose one.
    pub fn pdf_output_file_override(&self) -> Option<&str> {
        self.lua_res.output_name.as_deref()
    }

    /// texfileio.c `zopen_w_input`: `find_format_file` names the format
    /// file to load. `None`: no callback; `Some(None)`: it found no file.
    pub fn lua_find_format_file(&mut self, name: &str) -> Option<Option<String>> {
        if self.engine_kind != EngineKind::LuaTeX {
            return None;
        }
        self.lua_find_file(Cb::FindFormatFile, None, name.as_bytes())
            .map(|found| found.filter(|n| !n.is_empty()).map(|n| lossy(&n)))
    }
}
