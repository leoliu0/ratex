use crate::boxes::{Node, WhatIt};
use crate::build::RULE_FILL;
use crate::engine::Engine;
pub use crate::pdffile::PdfEncryptConfig;
use crate::token::Token;

// PDF document model: pages, annotations, destinations, embedded fonts.
// Serialization lives in `pdffile`; page rendering in `pdfrender`.
/// A link (or generic) annotation attached to a page.
#[derive(Clone, Debug)]
pub struct Annot {
    /// [x0 y0 x1 y1] in PDF user space (y grows upward), in bp
    pub rect: [f64; 4],
    /// external URL: serialized as /A <</S /URI /URI (...)>>
    pub uri: Option<String>,
    /// internal named destination: serialized as a /GoTo action
    pub dest: Option<String>,
    /// extra key/value dict body from \pdfstartlink user{...}
    pub attr: String,
    /// explicit /Subtype value; `None` falls back to /Link (the annots
    /// produced by \pdfstartlink and plain \pdfannot)
    pub subtype: Option<String>,
}

/// Identifier of a `\pdfdest`: `name {<string>}` entries go to the /Dests
/// name tree, `num <n>` entries become standalone destination objects.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DestId {
    Name(String),
    Num(i32),
}

/// A destination anchored at a page point.
#[derive(Clone, Debug)]
pub struct Dest {
    pub id: DestId,
    /// anchor / explicit position in bp, bottom-origin
    pub x: f64,
    pub y: f64,
    /// PDF dest type: 0 /XYZ, 1 /Fit, 2 /FitH, 3 /FitV, 4 /FitB,
    /// 5 /FitBH, 6 /FitBV, 7 /FitR
    pub kind: u8,
    /// /XYZ zoom factor (None = null)
    pub zoom: Option<f64>,
}

/// Target of a pdfTeX `goto` action.
#[derive(Clone, Debug)]
pub enum GotoTarget {
    /// `page <n> {<view>}`: 1-based page number and raw view tokens
    Page(i32, String),
    Dest(DestId),
}

/// pdfTeX action specification (`scan_action`).
#[derive(Clone, Debug)]
pub enum PdfAction {
    /// `user {<action dict>}`, written verbatim
    User(String),
    /// `goto [file {<file>}] page|name|num ... [newwindow|nonewwindow]`
    Goto {
        file: Option<String>,
        new_window: Option<bool>,
        target: GotoTarget,
    },
    /// `thread [file {<file>}] name {..}|num <n>`
    Thread { file: Option<String>, id: DestId },
}

/// One `\pdfoutline [attr {..}] <action> [count <n>] {<title>}` entry, in
/// source order. A nonzero count makes the next |n| entries (recursively)
/// its children; a negative count shows the item closed.
#[derive(Clone, Debug)]
pub struct Outline {
    pub attr: String,
    pub action: Option<PdfAction>,
    pub count: i32,
    /// raw title tokens: a PDF string body, possibly with escapes
    pub title: String,
    /// where the item was declared, for end-of-job diagnostics
    pub source: Option<crate::input::SourceMark>,
}

/// A compiled PDF page awaiting serialization.
pub struct PdfPage {
    pub content: Vec<u8>,
    pub width: i32,
    pub height: i32,
    /// page size in sp, printed as pdfTeX `pdf_print_mag_bp` does
    pub width_sp: i64,
    pub height_sp: i64,
    pub annots: Vec<Annot>,
    /// Existing annotation objects `pdf.registerannot` added to the page
    pub annot_refs: Vec<i32>,
    /// (doc font index, resource number `n` of `/F<n>`) — resolved by
    /// `embed_used_fonts`. pdfTeX names a font resource after the internal
    /// number of the font that owns its dictionary.
    pub fonts: Vec<(usize, u32)>,
    /// named destinations anchored on this page
    pub dests: Vec<Dest>,
    /// raw dict body contributed by \pdfpageattr (copied at shipout)
    pub attr_extra: Vec<u8>,
    /// raw dict entries contributed by \pdfpageresources (copied at shipout)
    pub resources_extra: Vec<u8>,
    pub display_list: Option<crate::boxes::DisplayList>,
    /// pdfTeX "Generate ProcSet if desired" (`\pdfomitprocset` at shipout)
    pub procset: bool,
    /// pdfTeX `pdf_image_procset`: IMAGE_COLOR_* bits of the page's images
    pub image_procset: u8,
    /// pdfTeX `pdf_xform_list`: form objects painted, in first-use order
    pub xforms: Vec<i32>,
    /// pdfTeX `pdf_ximage_list`: image objects painted, in first-use order
    pub ximages: Vec<i32>,
    /// pdfTeX `pdf_page_group_val`: the page's /Group object (0 = none)
    pub group: i32,
    /// LuaTeX `\pdfvariable omitmediabox` was zero at shipout.
    pub media_box: bool,
}

pub struct PdfDoc {
    compression_worker: Option<crate::pdfcompress::Worker>,
    page_compression: Vec<Option<crate::pdfcompress::Pending>>,
    pub pages: Vec<PdfPage>,
    /// LuaTeX `page_order_index` location per page (pages missing here sort
    /// at 0): the `/Kids` of the page tree are ordered by it.
    pub page_order: Vec<i32>,
    /// raw user objects from \pdfobj
    pub objects: Vec<(i32, Vec<u8>)>,
    /// Reserved font dictionaries for forms, with the same font-index
    /// remapping as pages. Each form has its own resource namespace.
    pub form_fonts: Vec<(i32, Vec<(usize, u32)>)>,
    /// Shared descriptors for standard PDF fonts embedded while importing pages.
    pub(crate) imported_base14_fonts: std::collections::BTreeMap<Vec<u8>, i32>,
    pub info: Vec<u8>,
    /// raw dict body contributed by \pdfcatalog
    pub catalog_extra: Vec<u8>,
    /// raw dict entries contributed by \pdfnames (merged into /Names)
    pub names_extra: Vec<u8>,
    /// raw dict body contributed by \pdfpagesattr (written to the page tree)
    pub pages_attr: Vec<u8>,
    /// \pdfoutline entries in source order (tree built at serialization)
    pub outlines: Vec<Outline>,
    /// loaded embedded fonts
    /// open action: (1-based page number, view name) from \pdfcatalog's
    /// keyword form `openaction goto page <n> {view}` (hyperref PDF@SetupDoc)
    pub open_action: Option<(i32, String)>,
    /// loaded embedded fonts
    pub fonts: Vec<EmbedFont>,
    /// Character codes used by each engine font before page/form font ids
    /// are remapped to entries in `fonts`. This lets the serializer retain
    /// only the required Type 1 glyph programs.
    pub font_chars: std::collections::BTreeMap<usize, [u64; 4]>,
    pub encrypt: Option<PdfEncryptConfig>,
    pub pdfa: bool,
    /// PDF header version frozen at the first shipout (`\pdfmajorversion`,
    /// `\pdfminorversion`).
    pub major_version: i32,
    pub minor_version: Option<i32>,
    /// pdfTeX `fixed_decimal_digits`: fractional digits of PDF coordinates.
    pub decimal_digits: u32,
    /// XeTeX native fonts as xdvipdfmx embeds them: glyph metrics per font
    /// program, the PDF font each TeX font belongs to (`xe_fid_rep`) and the
    /// glyphs used per PDF font (`xe_use`, keyed by the representative TeX font).
    pub(crate) xe_metrics: std::collections::HashMap<([u8; 16], u32, bool), std::rc::Rc<crate::dpx_font::NativeMetrics>>,
    pub(crate) xe_groups: std::collections::HashMap<XeGroupKey, u16>,
    pub(crate) xe_fid_rep: std::collections::HashMap<u16, u16>,
    pub(crate) xe_use: std::collections::BTreeMap<u16, std::collections::BTreeSet<u16>>,
    pub native_bindings: std::collections::BTreeMap<usize, Vec<NativeBindingInfo>>,
    pub legacy_bindings: std::collections::BTreeMap<usize, Vec<LegacyBindingInfo>>,
    /// pdfTeX `mag_set`: the magnification frozen by the first page output
    /// (0 = not yet used). Page geometry prints through `pdf_print_mag_bp`.
    pub mag: i32,
    /// pdfTeX `pdf_link_stack`: open \pdfstartlink regions, which persist
    /// across boxes and pages until \pdfendlink.
    pub(crate) link_stack: Vec<OpenLink>,
    /// pdfTeX `gen_running_link` (\pdfrunninglinkoff/on), persistent across pages.
    pub(crate) gen_running_link: bool,
    /// XeTeX: the document is written like xdvipdfmx writes it (information
    /// dictionary without pdfTeX's /Trapped, /ModDate and /PTEX banner).
    pub(crate) xdvipdfmx: bool,
    /// pdfTeX resource names: form XObject number → `n` of `/Fm<n>`
    /// (`pdf_xform_count` when the form was created).
    pub(crate) form_names: std::collections::BTreeMap<i32, i32>,
    /// pdfTeX resource names: image XObject number → `n` of `/Im<n>`
    /// (`pdf_ximage_count` when the image was read).
    pub(crate) image_names: std::collections::BTreeMap<i32, i32>,
    /// pdfTeX `pdf_resname_prefix` (`\pdfuniqueresname`): appended to every
    /// font, form and image resource name.
    pub(crate) resname_prefix: String,
    /// Destinations already shipped (`obj_dest_ptr` set): later ones with
    /// the same identifier are duplicates.
    pub(crate) shipped_dests: std::collections::HashSet<DestId>,
    /// `\pdftrailer` entries for the trailer dictionary.
    pub(crate) trailer_extra: Vec<u8>,
    /// `\pdfomitinfodict`: no document information dictionary.
    pub(crate) omit_info_dict: bool,
    /// pdfTeX `start_time_str`: the job start as a PDF date. It is the
    /// /CreationDate and /ModDate of the Info dictionary and, with the output
    /// file name, the source of the default trailer /ID. Empty = unknown.
    pub(crate) start_time: String,
    /// `\pdfinfoomitdate`: no /CreationDate and /ModDate.
    pub(crate) info_omit_date: bool,
    /// The `/PTEX.Fullbanner` key as `\pdfsuppressptexinfo` and
    /// `\pdfptexuseunderscore` leave it; `None` when bit 1 suppresses it.
    pub(crate) ptex_banner_key: Option<&'static str>,
    /// /Producer and the /PTEX.Fullbanner text of the engine that writes the file.
    pub(crate) producer: &'static str,
    pub(crate) banner: &'static str,
    /// Output file name (`output_file_name`), the second part of the /ID.
    pub(crate) output_name: String,
    /// `\pdftrailerid` text (`pdf_trailer_id_toks`): its MD5 replaces the
    /// default /ID; empty text writes no /ID. `None` = no such command.
    pub(crate) trailer_id_text: Option<Vec<u8>>,
    /// LuaTeX `\pdfvariable trailerid` / `pdf.settrailerid`: the /ID array
    /// text, written verbatim (pdfgen.c `print_ID`); empty = unset.
    pub(crate) trailer_id_raw: Vec<u8>,
    /// `\pdfomitcharset`: no /CharSet in Type 1 font descriptors.
    pub(crate) omit_charset: bool,
    /// `\pdfpageref`: object numbers fixed for pages (0-based index).
    pub(crate) page_objnums: std::collections::BTreeMap<usize, i32>,
    /// Highest object number the engine reserved; the writer numbers its
    /// own objects after it.
    pub(crate) reserved_objects: i32,
    /// epdf.c's font descriptors for fonts of included PDF files that the
    /// `\pdfinclusioncopyfonts` = 0 replacement took over.
    pub imported_fonts: Vec<ImportedFont>,
    /// pdftoepdf.cc `pdfDocuments`: every included PDF file once, with the
    /// objects already copied from it, by file name.
    pub pdf_sources: std::collections::HashMap<String, crate::pdf_images::PdfSource>,
    /// Parsed Type 1 programs of replacement fonts by file name (None when
    /// the file does not exist).
    pub(crate) imported_programs:
        std::collections::HashMap<String, Option<std::rc::Rc<crate::pdffile::Type1Source>>>,
}

/// One `pdf_link_stack` record: the link's box nesting level, its width,
/// height and depth spec (`RULE_FILL` = running) and action, plus its
/// current annotation on the page being shipped and that annotation's raw
/// DVI-space rectangle (for `matrixrecalculate`).
#[derive(Clone, Debug)]
pub(crate) struct OpenLink {
    pub nesting: i32,
    pub dims: (i32, i32, i32),
    pub uri: Option<String>,
    pub dest: Option<String>,
    pub attr: String,
    pub annot: Option<usize>,
    pub raw: [i64; 4],
}

impl PdfDoc {
    /// Local `goto name` targets (link annotations and outline items) that no
    /// `\pdfdest name` defines, in first-reference order. pdfTeX replaces each
    /// with a fixed destination on the first page.
    pub(crate) fn unresolved_dest_names(&self) -> Vec<&str> {
        if self.pages.is_empty() {
            return Vec::new();
        }
        let defined: std::collections::HashSet<&str> = self
            .pages
            .iter()
            .flat_map(|page| &page.dests)
            .filter_map(|dest| match &dest.id {
                DestId::Name(name) => Some(name.as_str()),
                DestId::Num(_) => None,
            })
            .collect();
        let links = self
            .pages
            .iter()
            .flat_map(|page| &page.annots)
            .filter_map(|annot| annot.dest.as_deref());
        let outlines = self.outlines.iter().filter_map(|item| match &item.action {
            Some(PdfAction::Goto {
                file: None,
                target: GotoTarget::Dest(DestId::Name(name)),
                ..
            }) => Some(name.as_str()),
            _ => None,
        });
        let mut seen = std::collections::HashSet::new();
        links
            .chain(outlines)
            .filter(|name| !defined.contains(name) && seen.insert(*name))
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EmbedFontSubtype {
    Type1,
    TrueType,
    Cff,
}

#[derive(Clone, Debug)]
pub struct NativeBindingInfo {
    pub code_map: std::collections::HashMap<u16, smallvec::SmallVec<[usize; 1]>>,
    pub entries: Vec<(u16, u16, String)>,
    pub next_code: u32,
}
#[derive(Clone, Debug)]
pub struct LegacyBindingInfo {
    pub code_map: std::collections::HashMap<u8, smallvec::SmallVec<[usize; 1]>>,
    pub entries: Vec<(u8, u8, String)>,
    pub next_code: u16,
}

/// Binding zero is permanently reserved for a font's original one-byte code
/// space. Semantic remaps and native glyph maps occupy disjoint shards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontBinding(usize);

impl FontBinding {
    pub const RAW: Self = Self(0);

    pub fn remapped(index: usize) -> Self {
        Self(
            index
                .checked_add(1)
                .expect("PDF font binding index overflow"),
        )
    }

    pub fn remapped_index(self) -> Option<usize> {
        self.0.checked_sub(1)
    }

    pub fn resource_key(self, font_id: u16) -> usize {
        (self.0 << u16::BITS) | usize::from(font_id)
    }
}

/// A font prepared for embedding (Type 1 or OpenType/TrueType/CFF).
pub struct EmbedFont {
    pub obj_font: i32,
    pub base_font: String,
    /// full Type 1 program or raw font data
    pub font_file: std::rc::Rc<Vec<u8>>,
    pub length1: usize,
    pub length2: usize,
    pub length3: usize,
    pub is_truetype: bool,
    pub subtype: EmbedFontSubtype,
    pub face_index: u32,
    pub variations: Vec<(ttf_parser::Tag, f32)>,
    pub allow_subsetting: bool,
    pub content_hash: [u8; 16],
    pub units_per_em: u16,
    /// glyph names by slot (None = the font's built-in encoding)
    pub encoding_diff: Option<std::rc::Rc<[String]>>,
    pub first_char: u8,
    pub last_char: u8,
    /// widths in 1/10000 font units, for first_char..=last_char
    pub widths: Vec<i32>,
    pub font_matrix_scale: f64,
    /// FontDescriptor metrics of SFNT fonts (1/1000 font units, degrees for
    /// the angle); Type 1 descriptors use `t1_preset` and `t1_keys`
    pub font_bbox: [f64; 4],
    pub italic_angle: f64,
    pub ascent: f64,
    pub descent: f64,
    pub cap_height: f64,
    pub stem_v: f64,
    pub flags: i32,
    /// /ToUnicode mappings: (code, Unicode string).
    pub to_unicode: Vec<(u8, String)>,
    /// Character codes actually painted with this font.
    pub used_chars: [u64; 4],
    /// CID font indicators and mappings
    pub is_cid: bool,
    pub is_native: bool,
    pub legacy_cids: Vec<(u8, u16, String)>,
    pub native_cids: Vec<(u16, u16, String)>,
    pub used_gids: std::collections::BTreeSet<u16>,
    pub to_unicode_2byte: Vec<(u16, String)>,
    /// `\pdffontattr` text appended to the font dictionary.
    pub font_attr: String,
    /// Type 1 FontDescriptor inputs (writefont.c `preset_fontmetrics` of
    /// the TFM, overridden by the keys the program declares). Fonts of one
    /// program share a descriptor preset from the newest-initialized TFM.
    pub t1_preset: [i32; crate::pdf_fonts::INT_KEYS_NUM],
    pub t1_keys: std::rc::Rc<crate::pdf_fonts::Type1Keys>,
    /// `pdf_init_font` order of the engine font.
    pub init_order: usize,
    /// The object number `font_descriptor_objnum_provider` chose for the
    /// descriptor (0: the writer numbers it).
    pub desc_obj: i32,
    /// pdfTeX's dictionary of an engine font's own code space (None for
    /// remapped code spaces, which have no pdfTeX counterpart).
    pub pdftex: Option<PdfTexFont>,
    /// XeTeX native font data (xdvipdfmx Identity-H/V CID font).
    pub xe: Option<XeFont>,
}

/// What the writer needs of a XeTeX native font beyond the program.
pub struct XeFont {
    pub vertical: bool,
    pub used: std::collections::BTreeSet<u16>,
}

/// A PDF font of XeTeX's native text: xdvipdfmx shares one font between
/// all TeX fonts of the same program, face, direction and synthetic options
/// (`pdf_insert_native_fontmap_record` key).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct XeGroupKey {
    pub hash: [u8; 16],
    pub face_index: u32,
    pub variations: Vec<(u32, u32)>,
    pub vertical: bool,
    pub extend: i32,
    pub slant: i32,
    pub embolden: i32,
}

impl PdfDoc {
    /// The representative TeX font of `fid`'s PDF font.
    pub(crate) fn xe_group_rep(&mut self, fid: u16, key: XeGroupKey) -> u16 {
        if let Some(&rep) = self.xe_fid_rep.get(&fid) {
            return rep;
        }
        let rep = *self.xe_groups.entry(key).or_insert(fid);
        self.xe_fid_rep.insert(fid, rep);
        rep
    }

    #[inline]
    pub(crate) fn xe_note_glyph(&mut self, rep: u16, gid: u16) {
        self.xe_use.entry(rep).or_default().insert(gid);
    }
}

/// writefont.c `fo_entry` data of a Type 1 font dictionary.
pub struct PdfTexFont {
    pub tfm_name: String,
    /// Original container, retained before PFB segment headers are removed.
    pub source_is_pfb: bool,
    /// The map's encoding file: one /Encoding object per file, covering
    /// the codes every font with that encoding uses. None = the program's
    /// builtin encoding, which pdfTeX leaves implicit.
    pub enc_file: Option<String>,
    /// The `/Name` the encoding file declares (xdvipdfmx names the ToUnicode CMap after it).
    pub enc_ps_name: Option<String>,
    /// tounicode.c `write_tounicode` CMap.
    pub tounicode: Option<std::rc::Rc<str>>,
}

/// writefont.c `fd_entry` of a font that epdf's `copyFont` replaced: the
/// map entry's Type 1 program, written for the glyphs the included font's
/// /CharSet names. Fonts of one program, slant and extension share it.
pub struct ImportedFont {
    /// `fm->ff_name`: the program's file name.
    pub ff_name: String,
    /// `fm_slant` and `fm_extend` of the map entry.
    pub slant: i32,
    pub extend: i32,
    pub program: std::rc::Rc<crate::pdffile::Type1Source>,
    /// `fm->ps_name`.
    pub base_font: String,
    /// `is_subsetted(fm)`: the map entry downloads a partial font.
    pub subsettable: bool,
    /// `fd->gl_tree`: glyph names of the /CharSet strings so far.
    pub glyphs: std::collections::BTreeSet<String>,
    /// `fd->all_glyphs` (`embed_whole_font`): a font without /CharSet.
    pub all_glyphs: bool,
    /// `/StemV` of the first included font, rounded.
    pub stem_v: i32,
    /// `fd_objnum`, reserved when the descriptor was created.
    pub desc_obj: i32,
    /// `fn_objnum` (0 until a font dictionary needs it): the object that
    /// holds the tagged /BaseFont name.
    pub name_obj: i32,
    /// Number of document fonts already initialized when the font was first
    /// included: it created the shared descriptor, so those initialized
    /// later find it and preset nothing from their TFM.
    pub init_order: usize,
}

impl PdfDoc {
    pub fn new() -> Self {
        PdfDoc {
            compression_worker: None,
            page_compression: Vec::new(),
            pages: Vec::new(),
            page_order: Vec::new(),
            objects: Vec::new(),
            form_fonts: Vec::new(),
            imported_base14_fonts: std::collections::BTreeMap::new(),
            info: Vec::new(),
            catalog_extra: Vec::new(),
            names_extra: Vec::new(),
            pages_attr: Vec::new(),
            outlines: Vec::new(),
            open_action: None,
            fonts: Vec::new(),
            font_chars: std::collections::BTreeMap::new(),
            encrypt: None,
            pdfa: false,
            major_version: 1,
            minor_version: None,
            decimal_digits: 3,
            native_bindings: std::collections::BTreeMap::new(),
            xe_metrics: std::collections::HashMap::new(),
            xe_groups: std::collections::HashMap::new(),
            xe_fid_rep: std::collections::HashMap::new(),
            xe_use: std::collections::BTreeMap::new(),
            legacy_bindings: std::collections::BTreeMap::new(),
            mag: 0,
            link_stack: Vec::new(),
            form_names: std::collections::BTreeMap::new(),
            image_names: std::collections::BTreeMap::new(),
            resname_prefix: String::new(),
            shipped_dests: std::collections::HashSet::new(),
            trailer_extra: Vec::new(),
            omit_info_dict: false,
            start_time: String::new(),
            info_omit_date: false,
            ptex_banner_key: Some("PTEX.Fullbanner"),
            producer: crate::pdftex::PDFTEX_PRODUCER,
            banner: crate::pdftex::PDFTEX_BANNER,
            output_name: String::new(),
            trailer_id_text: None,
            trailer_id_raw: Vec::new(),
            omit_charset: false,
            page_objnums: std::collections::BTreeMap::new(),
            reserved_objects: 0,
            imported_fonts: Vec::new(),
            pdf_sources: std::collections::HashMap::new(),
            imported_programs: std::collections::HashMap::new(),
            gen_running_link: true,
            xdvipdfmx: false,
        }
    }

    pub fn push_page(&mut self, page: PdfPage) {
        if self.pages.len() == 1 && !crate::debug_flag("TEX_PDF_SERIAL") {
            self.compression_worker = crate::pdfcompress::Worker::new();
        }
        self.page_compression.resize_with(self.pages.len(), || None);
        self.page_compression.push(
            self.compression_worker
                .as_ref()
                .and_then(|worker| worker.submit(&page.content)),
        );
        self.pages.push(page);
    }

    pub(crate) fn compressed_page(&self, index: usize) -> Option<&[u8]> {
        self.page_compression
            .get(index)?
            .as_ref()?
            .get(&self.pages.get(index)?.content)
    }

    #[inline]
    pub fn record_font_char(&mut self, font: usize, character: u8) {
        let words = self.font_chars.entry(font).or_insert([0; 4]);
        words[character as usize / 64] |= 1_u64 << (character as usize % 64);
    }

    pub fn get_or_alloc_native_code(
        &mut self,
        font_id: usize,
        glyph_id: u16,
        text: &str,
    ) -> (FontBinding, u16) {
        let bindings = self.native_bindings.entry(font_id).or_default();
        for (index, binding) in bindings.iter().enumerate() {
            if let Some(indices) = binding.code_map.get(&glyph_id) {
                for &entry in indices {
                    if binding.entries[entry].2 == text {
                        return (FontBinding::remapped(index), binding.entries[entry].0);
                    }
                }
            }
        }
        if bindings
            .last()
            .is_none_or(|binding| binding.next_code > u16::MAX as u32)
        {
            bindings.push(NativeBindingInfo {
                code_map: std::collections::HashMap::new(),
                entries: Vec::new(),
                next_code: 1,
            });
        }
        let binding_index = bindings.len() - 1;
        let binding = &mut bindings[binding_index];
        let code = binding.next_code as u16;
        binding.next_code += 1;
        binding
            .code_map
            .entry(glyph_id)
            .or_default()
            .push(binding.entries.len());
        binding.entries.push((code, glyph_id, text.to_owned()));
        (FontBinding::remapped(binding_index), code)
    }
    pub fn get_or_alloc_legacy_code(
        &mut self,
        font_id: usize,
        base_char: u8,
        text: &str,
    ) -> (FontBinding, u8) {
        let bindings = self.legacy_bindings.entry(font_id).or_default();
        for (index, binding) in bindings.iter().enumerate() {
            if let Some(indices) = binding.code_map.get(&base_char) {
                for &entry in indices {
                    if binding.entries[entry].2 == text {
                        return (FontBinding::remapped(index), binding.entries[entry].0);
                    }
                }
            }
        }
        if bindings
            .last()
            .is_none_or(|binding| binding.next_code > u8::MAX as u16)
        {
            bindings.push(LegacyBindingInfo {
                code_map: std::collections::HashMap::new(),
                entries: Vec::new(),
                next_code: 1,
            });
        }
        let binding_index = bindings.len() - 1;
        let binding = &mut bindings[binding_index];
        let code = binding.next_code as u8;
        binding.next_code += 1;
        binding
            .code_map
            .entry(base_char)
            .or_default()
            .push(binding.entries.len());
        binding.entries.push((code, base_char, text.to_owned()));
        (FontBinding::remapped(binding_index), code)
    }
}

impl Default for PdfDoc {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------------------------------- primitive handling

/// Sentinel \pdfdest coordinate: "use the current position" (pdfTeX uses
/// the value -32768 for this).
pub const PDF_POS_CURRENT: i32 = -32768;

/// Result of scanning an optional \pdfdest positional parameter.
enum DestParam {
    Number,
    Null,
    End,
}
/// Destination type keywords, case-insensitive per pdfTeX's scan_keyword.
fn dest_kind(kw: &[u8]) -> Option<u8> {
    Some(match kw {
        b"xyz" => 0,
        b"fit" => 1,
        b"fith" => 2,
        b"fitv" => 3,
        b"fitb" => 4,
        b"fitbh" => 5,
        b"fitbv" => 6,
        b"fitr" => 7,
        _ => return None,
    })
}

/// Number of optional positional parameters a dest type takes
/// (xyz: left top zoom; fith/fitbh: top; fitv/fitbv: left; fitr: 4).
fn dest_param_count(kind: u8) -> usize {
    match kind {
        0 => 3,
        2 | 5 | 3 | 6 => 1,
        7 => 4,
        _ => 0,
    }
}

/// Extract the /URI (...) action target from a raw \pdfstartlink user{...}
/// spec; returns None when the spec carries no URI action.
pub fn extract_uri(spec: &str) -> Option<String> {
    let b = spec.as_bytes();
    let mut i = 0;
    while i + 4 <= b.len() {
        if &b[i..i + 4] == b"/URI" && b.get(i + 4).is_none_or(|c| !c.is_ascii_alphabetic()) {
            let mut j = i + 4;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == b'(' {
                j += 1;
                let mut out = Vec::new();
                let mut depth = 1;
                while j < b.len() {
                    match b[j] {
                        b'\\' if j + 1 < b.len() => {
                            out.push(b[j + 1]);
                            j += 2;
                        }
                        b'(' => {
                            depth += 1;
                            out.push(b'(');
                            j += 1;
                        }
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                return String::from_utf8_lossy(&out).into_owned().into();
                            }
                            out.push(b')');
                            j += 1;
                        }
                        c => {
                            out.push(c);
                            j += 1;
                        }
                    }
                }
            }
        }
        i += 1;
    }
    None
}

impl Engine {
    /// Peek at the run of letter tokens at the input (pdfTeX-style
    /// keyword: plain letters scanned case-insensitively, e.g. the
    /// `attr`/`goto`/`name`/`XYZ` of hyperref's calls). The input is
    /// always restored.
    fn peek_letters(&mut self) -> String {
        self.skip_spaces_relax();
        let mut letters: Vec<u8> = Vec::new();
        let mut toks: Vec<Token> = Vec::new();
        loop {
            let t = self.get_token();
            if t.is_char() && t.cc() == 11 {
                letters.push(t.chr() as u8);
                toks.push(t);
            } else {
                toks.push(t);
                break;
            }
        }
        for t in toks.iter().rev() {
            self.push_token(t.clone());
        }
        String::from_utf8_lossy(&letters).to_ascii_lowercase()
    }

    /// Consume the keyword `name` (a run of letters, case-insensitive,
    /// spaces skipped). On mismatch the input is fully restored.
    fn take_keyword(&mut self, name: &[u8]) -> bool {
        self.skip_spaces_relax();
        let mut letters: Vec<u8> = Vec::new();
        let mut toks: Vec<Token> = Vec::new();
        loop {
            let t = self.get_token();
            if t.is_char() && t.cc() == 11 {
                letters.push(t.chr() as u8);
                toks.push(t);
            } else {
                toks.push(t);
                break;
            }
        }
        if letters.to_ascii_lowercase().as_slice() == name {
            // keyword consumed: only the terminator token goes back
            if let Some(t) = toks.last() {
                self.push_token(t.clone());
            }
            true
        } else {
            for t in toks.iter().rev() {
                self.push_token(t.clone());
            }
            false
        }
    }

    /// Next optional \pdfdest parameter: a number, the keyword `null`
    /// (keep the current position), or end of the parameter list.
    fn next_dest_param(&mut self) -> DestParam {
        self.skip_spaces_relax();
        let t = self.get_token();
        if t.is_char() && matches!(t.chr() as u8, b'0'..=b'9' | b'+' | b'-') {
            self.push_token(t);
            return DestParam::Number;
        }
        if t.is_char() && t.cc() == 11 {
            let mut letters = vec![t.chr() as u8];
            let mut toks = vec![t];
            loop {
                let tt = self.get_token();
                if tt.is_char() && tt.cc() == 11 {
                    letters.push(tt.chr() as u8);
                    toks.push(tt);
                } else {
                    toks.push(tt);
                    break;
                }
            }
            if letters.to_ascii_lowercase().as_slice() == b"null" {
                if let Some(tt) = toks.last() {
                    self.push_token(tt.clone());
                }
                return DestParam::Null;
            }
            for tt in toks.iter().rev() {
                self.push_token(tt.clone());
            }
            return DestParam::End;
        }
        self.push_token(t);
        DestParam::End
    }

    /// \pdfstartlink [attr{..}|width..|height..|depth..] goto name{..}|user{..}
    /// hyperref forms: `attr{..} goto name{..}\relax`, `attr{..} user{..}\relax`,
    /// `user{..}\relax`.
    pub fn do_pdfstartlink(&mut self) {
        let mut attr = String::new();
        let mut uri: Option<String> = None;
        let mut dest: Option<String> = None;
        // pdfTeX `scan_alt_rule`: unspecified dimensions stay running
        let (mut wd, mut ht, mut dp) = (RULE_FILL, RULE_FILL, RULE_FILL);
        loop {
            match self.peek_letters().as_str() {
                kw @ ("width" | "height" | "depth") => {
                    self.take_keyword(kw.as_bytes());
                    self.scan_optional_equals();
                    let value = self.scan_dimen(false, false);
                    match kw {
                        "width" => wd = value,
                        "height" => ht = value,
                        _ => dp = value,
                    }
                }
                "attr" => {
                    self.take_keyword(b"attr");
                    self.scan_optional_equals();
                    attr = self.scan_pdf_string();
                }
                "user" | "file" => {
                    let kw = self.peek_letters();
                    self.take_keyword(kw.as_bytes());
                    self.scan_optional_equals();
                    let spec = self.scan_pdf_string();
                    match extract_uri(&spec) {
                        Some(u) => uri = Some(u),
                        None => attr.push_str(&spec),
                    }
                    break;
                }
                "goto" => {
                    self.take_keyword(b"goto");
                    match self.peek_letters().as_str() {
                        "name" | "fld" => {
                            let sub = self.peek_letters();
                            self.take_keyword(sub.as_bytes());
                            dest = Some(self.scan_pdf_string());
                        }
                        "num" => {
                            self.take_keyword(b"num");
                            let _ = self.scan_int();
                        }
                        "page" => {
                            self.take_keyword(b"page");
                            let _ = self.scan_int();
                        }
                        _ => {}
                    }
                    break;
                }
                "thread" => {
                    self.take_keyword(b"thread");
                    break;
                }
                _ => break,
            }
        }
        self.append_whatsit(Node::Whatsit(WhatIt::PdfStartLink {
            attr,
            uri,
            name: dest,
            wd,
            ht,
            dp,
        }, self.eqtb.cur_attr));
    }

    /// Destination identifier: `name {<string>}` or `num <n>` (pdfTeX
    /// requires a positive number). Leaves the input untouched when
    /// neither keyword follows.
    fn scan_dest_id(&mut self) -> Option<DestId> {
        match self.peek_letters().as_str() {
            kw @ ("name" | "fld") => {
                self.take_keyword(kw.as_bytes());
                Some(DestId::Name(self.scan_pdf_string()))
            }
            "num" => {
                self.take_keyword(b"num");
                let n = self.scan_int();
                if n <= 0 {
                    self.error("pdfTeX error (ext1): num identifier must be positive");
                    return None;
                }
                Some(DestId::Num(n))
            }
            _ => None,
        }
    }

    /// \pdfdest name{<name>}|num <n> <type> [<params>] — <type> is one of
    /// xyz fit fith fitv fitb fitbh fitbv fitr (case-insensitive), each
    /// optionally followed by integers (sp; `null` keeps the anchor) and
    /// ended by `\relax` or the next non-parameter token.
    pub fn do_pdfdest(&mut self) {
        let Some(id) = self.scan_dest_id() else {
            return;
        };
        let type_kw = self.peek_letters();
        let kind = dest_kind(type_kw.as_bytes()).unwrap_or(0);
        if !type_kw.is_empty() {
            self.take_keyword(type_kw.as_bytes());
        }
        let mut vals = [PDF_POS_CURRENT; 4];
        for slot in vals.iter_mut().take(dest_param_count(kind)) {
            match self.next_dest_param() {
                DestParam::Number => *slot = self.scan_int(),
                DestParam::Null => {}
                DestParam::End => break,
            }
        }
        // pdftex.web: a destination already shipped makes this one a
        // duplicate, dropped right away
        if self.pdf_doc.shipped_dests.contains(&id) {
            self.warn_dest_dup(&id);
            return;
        }
        let node = Node::Whatsit(WhatIt::PdfDest {
            id,
            kind,
            params: vals,
        }, self.eqtb.cur_attr);
        match self.mode {
            crate::engine::Mode::Vertical | crate::engine::Mode::InternalVertical => {
                self.vlist_append(node)
            }
            _ => self.cur_list.push(node),
        }
    }

    /// \pdfannot [width <d>|height <d>|depth <d>] {<dict body>}: a generic
    /// annotation over the given box at the current point.
    pub fn do_pdfannot(&mut self) {
        // pdfTeX `scan_alt_rule`: unspecified dimensions stay running
        let (mut wd, mut ht, mut dp) = (RULE_FILL, RULE_FILL, RULE_FILL);
        loop {
            match self.peek_letters().as_str() {
                "width" => {
                    self.take_keyword(b"width");
                    self.scan_optional_equals();
                    wd = self.scan_dimen(false, false);
                }
                "height" => {
                    self.take_keyword(b"height");
                    self.scan_optional_equals();
                    ht = self.scan_dimen(false, false);
                }
                "depth" => {
                    self.take_keyword(b"depth");
                    self.scan_optional_equals();
                    dp = self.scan_dimen(false, false);
                }
                _ => break,
            }
        }
        let attr = self.scan_pdf_string();
        self.append_whatsit(Node::Whatsit(WhatIt::PdfAnnot { attr, wd, ht, dp }, self.eqtb.cur_attr));
    }

    /// pdfTeX `scan_action`: `user {<dict>}` or
    /// `goto|thread [file {<file>}] page <n> {<view>}|name {..}|num <n>
    /// [newwindow|nonewwindow]` (`page` only with `goto`).
    fn scan_pdf_action(&mut self) -> Option<PdfAction> {
        let kind = self.peek_letters();
        match kind.as_str() {
            "user" => {
                self.take_keyword(b"user");
                return Some(PdfAction::User(self.scan_pdf_string()));
            }
            "goto" | "thread" => {
                self.take_keyword(kind.as_bytes());
            }
            _ => {
                self.error("pdfTeX error (ext1): action type missing");
                return None;
            }
        }
        let goto = kind == "goto";
        let file = if self.peek_letters() == "file" {
            self.take_keyword(b"file");
            Some(self.scan_pdf_string())
        } else {
            None
        };
        let target = if self.peek_letters() == "page" {
            self.take_keyword(b"page");
            let page = self.scan_int();
            let view = self.scan_pdf_string();
            if !goto {
                self.error("pdfTeX error (ext1): only GoTo action can be used with `page'");
                return None;
            }
            if page <= 0 {
                self.error("pdfTeX error (ext1): invalid page number");
                return None;
            }
            GotoTarget::Page(page, view)
        } else {
            match self.scan_dest_id() {
                Some(DestId::Num(_)) if goto && file.is_some() => {
                    self.error(
                        "pdfTeX error (ext1): `goto' option cannot be used with both `file' and `num'",
                    );
                    return None;
                }
                Some(id) => GotoTarget::Dest(id),
                None => {
                    self.error("pdfTeX error (ext1): identifier type missing");
                    return None;
                }
            }
        };
        let new_window = match self.peek_letters().as_str() {
            "newwindow" => {
                self.take_keyword(b"newwindow");
                Some(true)
            }
            "nonewwindow" => {
                self.take_keyword(b"nonewwindow");
                Some(false)
            }
            _ => None,
        };
        if new_window.is_some() && (!goto || file.is_none()) {
            self.error(
                "pdfTeX error (ext1): `newwindow'/`nonewwindow' must be used with `goto' and `file' option",
            );
        }
        Some(if goto {
            PdfAction::Goto {
                file,
                new_window,
                target,
            }
        } else {
            let GotoTarget::Dest(id) = target else {
                unreachable!("`page' is rejected for thread actions")
            };
            PdfAction::Thread { file, id }
        })
    }

    /// \pdfoutline [attr {<dict>}] <action> [count <n>] {<title>}
    pub fn do_pdfoutline(&mut self) {
        let attr = if self.peek_letters() == "attr" {
            self.take_keyword(b"attr");
            self.scan_pdf_string()
        } else {
            String::new()
        };
        let action = self.scan_pdf_action();
        let mut count = 0i32;
        if self.peek_letters() == "count" {
            self.take_keyword(b"count");
            count = self.scan_int();
        }
        let title = self.scan_pdf_string();
        let source = self.input.current_source_mark();
        self.pdf_doc.outlines.push(Outline {
            attr,
            action,
            count,
            title,
            source,
        });
    }

    /// pdfTeX's end-of-file checks for outline targets that the PDF cannot
    /// point at: a `goto num` without a matching `\pdfdest num` is replaced
    /// by a fixed destination on the first page; a `goto page` past the
    /// last page and a local `thread num` (no `\pdfthread` support) leave
    /// the item without an action.
    pub(crate) fn warn_unresolved_outline_targets(&mut self) {
        let doc = &self.pdf_doc;
        if doc.pages.is_empty() {
            return;
        }
        let defined = |n: i32| {
            doc.pages
                .iter()
                .flat_map(|page| &page.dests)
                .any(|dest| dest.id == DestId::Num(n))
        };
        let mut warnings: Vec<(String, Option<crate::input::SourceContext>)> = Vec::new();
        for item in &doc.outlines {
            let message = match &item.action {
                Some(PdfAction::Goto {
                    file: None,
                    target: GotoTarget::Dest(DestId::Num(n)),
                    ..
                }) if !defined(*n) => format!(
                    "\\pdfoutline destination num {n} has been referenced but does not exist, replaced by a fixed one"
                ),
                Some(PdfAction::Goto {
                    file: None,
                    target: GotoTarget::Page(page, _),
                    ..
                }) if *page as usize > doc.pages.len() => format!(
                    "\\pdfoutline page {page} has been referenced but does not exist; the item has no action"
                ),
                Some(PdfAction::Thread {
                    file: None,
                    id: DestId::Num(n),
                }) => format!(
                    "\\pdfoutline thread num {n} needs \\pdfthread, which is not supported; the item has no action"
                ),
                _ => continue,
            };
            if !warnings.iter().any(|(seen, _)| *seen == message) {
                warnings.push((message, item.source.as_ref().map(|s| s.to_context())));
            }
        }
        for (message, source) in warnings {
            self.warning_at(&message, source);
        }
    }

    /// pdfTeX's end-of-file check for named destinations that links or
    /// outline items reference but no `\pdfdest name` defines.
    pub(crate) fn warn_unresolved_dest_names(&mut self) {
        let messages: Vec<String> = self
            .pdf_doc
            .unresolved_dest_names()
            .into_iter()
            .map(|name| {
                format!("name{{{name}}} has been referenced but does not exist, replaced by a fixed one")
            })
            .collect();
        for message in messages {
            self.warning_at(&message, None);
        }
    }

    /// \pdfcatalog {<dict body>} [use {<dict body>}]
    pub fn do_pdfcatalog(&mut self) {
        let body = self.scan_pdf_string();
        self.pdf_doc
            .catalog_extra
            .extend_from_slice(body.as_bytes());
        if self.peek_letters() == "use" {
            self.take_keyword(b"use");
            let extra = self.scan_pdf_string();
            self.pdf_doc
                .catalog_extra
                .extend_from_slice(extra.as_bytes());
        }
        // pdfTeX keyword form: `openaction goto page <n> {<view>}` —
        // hyperref's \PDF@SetupDoc emits it right after the dict body.
        // Unparsed, it leaked into the typeset stream.
        if self.peek_letters() == "openaction" {
            self.take_keyword(b"openaction");
            if self.peek_letters() == "goto" {
                self.take_keyword(b"goto");
                if self.peek_letters() == "page" {
                    self.take_keyword(b"page");
                    let page = self.scan_int();
                    let view = self.scan_pdf_string();
                    self.pdf_doc.open_action = Some((page, view));
                }
            }
        }
    }

    /// \pdfnames {<dict entries>}: appended to the catalog /Names dict.
    pub fn do_pdfnames(&mut self) {
        let body = self.scan_pdf_string();
        self.pdf_doc.names_extra.extend_from_slice(body.as_bytes());
    }

    /// \pdfpageattr {<dict body>}: replaces the per-page attribute body.
    pub fn do_pdfpageattr(&mut self) {
        self.scan_optional_equals();
        let toks = self.scan_token_list();
        self.pdf_page_attr = self.write_tokens_to_string(&toks);
        self.pdf_page_attr_toks = toks;
    }

    /// \pdfpagesattr {<dict body>}: replaces the page-tree attribute body.
    pub fn do_pdfpagesattr(&mut self) {
        self.scan_optional_equals();
        let toks = self.scan_token_list();
        self.pdf_pages_attr = self.write_tokens_to_string(&toks);
        self.pdf_pages_attr_toks = toks;
        self.pdf_doc.pages_attr = self.pdf_pages_attr.clone().into_bytes();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dest_kinds_map_case_insensitively() {
        // case-folding happens in the scanner (peek_letters lowercases)
        assert_eq!(dest_kind(b"xyz"), Some(0));
        assert_eq!(dest_kind(b"XYZ"), None);
        assert_eq!(dest_kind(b"fit"), Some(1));
        assert_eq!(dest_kind(b"fitbh"), Some(5));
        assert_eq!(dest_kind(b"fitbv"), Some(6));
        assert_eq!(dest_kind(b"fitr"), Some(7));
        assert_eq!(dest_kind(b"nonsense"), None);
        assert_eq!(dest_param_count(0), 3);
        assert_eq!(dest_param_count(5), 1);
        assert_eq!(dest_param_count(7), 4);
        assert_eq!(dest_param_count(1), 0);
    }

    #[test]
    fn uri_extraction_from_user_spec() {
        assert_eq!(
            extract_uri("/Subtype/Link/A<</S/URI/URI(https://example.org)>>"),
            Some("https://example.org".to_string())
        );
        // parens and backslashes inside the string survive PDF escaping
        assert_eq!(
            extract_uri("/A<</S/URI/URI(a\\(b\\)c)>>"),
            Some("a(b)c".to_string())
        );
        // no URI action: None, raw spec keeps flowing through as attr
        assert_eq!(extract_uri("/Subtype/Link/A<</S/Named/N/NextPage>>"), None);
    }
    #[test]
    fn font_binding_shards_do_not_alias_raw_or_each_other() {
        let mut doc = PdfDoc::new();
        let font_id = 17;
        let mut allocated = Vec::new();
        for index in 0..=u8::MAX {
            allocated.push(doc.get_or_alloc_legacy_code(
                font_id as usize,
                b'A',
                &format!("semantic-{index}"),
            ));
        }

        assert_eq!(allocated[0].0.remapped_index(), Some(0));
        assert_eq!(allocated[0].1, 1);
        assert_eq!(allocated[usize::from(u8::MAX)].0.remapped_index(), Some(1));
        assert_eq!(allocated[usize::from(u8::MAX)].1, 1);
        let raw_key = FontBinding::RAW.resource_key(font_id);
        let first_key = allocated[0].0.resource_key(font_id);
        let overflow_key = allocated[usize::from(u8::MAX)].0.resource_key(font_id);
        assert_ne!(raw_key, first_key);
        assert_ne!(raw_key, overflow_key);
        assert_ne!(first_key, overflow_key);
    }
}
