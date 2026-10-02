//! LuaTeX's file and resource callbacks (`find_font_file`, `read_font_file`,
//! `find_vf_file`, `read_vf_file`, `find_map_file`, `read_map_file`,
//! `find_enc_file`, `read_enc_file`, `find_type1_file`, `read_type1_file`,
//! `find_truetype_file`, `find_opentype_file`, `read_opentype_file`,
//! `find_data_file`, `read_data_file`, `find_image_file`,
//! `find_output_file`, `find_format_file`).
//!
//! Every expectation below was produced by running the same source with
//! TeX Live 2026 `luatex -ini -interaction=nonstopmode -jobname=job t.tex`
//! (`t.tex`: the `\directlua` line that enables the primitives, the `LIB`
//! below, the case's `\outputmode=1` setup when it writes a PDF, the body
//! and `\end`). The callbacks log `CB ...` and `MSG ...` lines; paths are
//! reduced to `DIR/<file name>` because the trees differ, and the
//! `start_file`/`stop_file` lines of the main input file are dropped.

use tex_core::engine::{Engine, EngineKind};

const LIB: &str = r##"
function NORM(s) if s:find('/', 1, true) then return 'DIR/' .. s:match('[^/]*$') end return s end
function LOG(...) local t = table.pack(...) for i = 1, t.n do t[i] = tostring(t[i]) end texio.write_nl('CB ' .. table.concat(t, ' ') .. '@@') end
function MSG(...) local t = table.pack(...) for i = 1, t.n do t[i] = tostring(t[i]) end texio.write_nl('MSG ' .. table.concat(t, ' ') .. '@@') end
function READALL(n) local f = io.open(n, 'rb') if not f then return nil end local d = f:read('a') f:close() return d end
function FIND(k, kind, pick) callback.register('find_' .. k .. '_file', function(n) LOG('find_' .. k .. '_file', NORM(n)) if pick then return pick(n) end return kpse.find_file(n, kind) or n end) end
function READ(k, kind, serve) callback.register('read_' .. k .. '_file', function(n) LOG('read_' .. k .. '_file', NORM(n)) local d if serve then d = serve(n) else d = READALL(kpse.find_file(n, kind) or n) end if d then return true, d, d:len() end return false, '', 0 end) end
function FILES() callback.register('start_file', function(c, n) LOG('start_file', c, NORM(n)) end) callback.register('stop_file', function(c) LOG('stop_file', c) end) end
function PAGES() callback.register('start_page_number', function() LOG('start_page') end) callback.register('stop_page_number', function() LOG('stop_page') end) callback.register('finish_pdffile', function() LOG('finish_pdffile') end) end
function OUT(pick) callback.register('find_output_file', function(n) LOG('find_output_file', NORM(n)) if pick then return pick(n) end return n end) end
"##;
const PDF: &str = r##"\outputmode=1 \pagewidth=100pt \pageheight=50pt \pdfvariable compresslevel=0 \pdfvariable objcompresslevel=0 "##;

struct Run {
    e: Engine,
    events: Vec<String>,
    pdf: Option<String>,
}

impl Run {
    /// The PDF with all white space removed.
    fn pdf_flat(&self) -> String {
        self.pdf.as_deref().expect("a PDF was written").split_whitespace().collect()
    }

    fn has_error(&self, text: &str) -> bool {
        self.e.diagnostics.iter().any(|d| d.message.contains(text))
    }
}

fn run(pdf: bool, body: &str) -> Run {
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.add_nullfont();
    for (c, cat) in [(b'{', 1), (b'}', 2), (b'#', 6), (b' ', 10), (b'\n', 5)] {
        e.eqtb.cat[c as usize] = cat;
    }
    e.set_interaction_mode(tex_core::engine::InteractionMode::Nonstop);
    e.job_name = "job".to_string();
    let setup = if pdf { PDF } else { "" };
    e.input.push_file(
        "t.tex".to_string(),
        format!("\\directlua{{tex.enableprimitives('',tex.extraprimitives())}}\n\\directlua{{{LIB}}}\n{setup}\n{body}\n\\end\n")
            .into_bytes(),
    );
    e.run();
    e.finish_job_diagnostics();
    let events = e
        .term
        .lines()
        .filter(|l| l.starts_with("CB ") || l.starts_with("MSG "))
        .filter_map(|l| l.find("@@").map(|end| l[..end].to_string()))
        .filter(|l| l != "CB stop_file 1" && !l.starts_with("CB start_file 1 "))
        .collect();
    let pdf = (e.error_count == 0 && !e.pdf_doc.pages.is_empty())
        .then(|| tex_core::driver::finish_pdf(&mut e, false).expect("PDF"))
        .map(|bytes| bytes.iter().map(|&b| b as char).collect::<String>());
    Run { e, events, pdf }
}

/// `\font` asks `find_font_file`/`read_font_file` and then `find_vf_file`/`read_vf_file` every time, even for a font loaded before.
#[test]
fn tfm_and_vf_callbacks_run_for_every_fresh_font() {
    let r = run(false, r##"\directlua{FIND('font', 'tfm') READ('font', 'tfm') FIND('vf', 'vf') READ('vf', 'vf')}
\font\a=cmr12 \font\b=cmr12 at 11pt \font\c=cmr12 \setbox0\hbox{}
\directlua{MSG('max', font.max())}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_font_file cmr12"##,
            r##"CB read_font_file DIR/cmr12.tfm"##,
            r##"CB find_vf_file cmr12"##,
            r##"CB read_vf_file cmr12"##,
            r##"CB find_font_file cmr12"##,
            r##"CB read_font_file DIR/cmr12.tfm"##,
            r##"CB find_vf_file cmr12"##,
            r##"CB read_vf_file cmr12"##,
            r##"CB find_font_file cmr12"##,
            r##"CB read_font_file DIR/cmr12.tfm"##,
            r##"CB find_vf_file cmr12"##,
            r##"CB read_vf_file cmr12"##,
            r##"MSG max 3"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
}

/// the bytes `read_font_file` returns are the font: cmr10's metrics under the name cmr12.
#[test]
fn read_font_file_replaces_the_tfm() {
    let r = run(false, r##"\directlua{FIND('font', 'tfm') READ('font', 'tfm', function(n) return READALL(kpse.find_file('cmr10.tfm', 'tfm')) end)}
\font\a=cmr12 \setbox0\hbox{\a A} \setbox0\hbox{}
\directlua{MSG('wd', tex.getbox(0).width)}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_font_file cmr12"##,
            r##"CB read_font_file DIR/cmr12.tfm"##,
            r##"MSG wd 0"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
}

/// `find_font_file` names another file; `read_font_file` is asked for that name.
#[test]
fn find_font_file_redirects_to_another_tfm() {
    let r = run(false, r##"\directlua{FIND('font', 'tfm', function(n) return kpse.find_file('cmr10.tfm', 'tfm') end) READ('font', 'tfm')}
\font\a=cmr12 \setbox0\hbox{\a A} \setbox0\hbox{}
\directlua{MSG('wd', tex.getbox(0).width)}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_font_file cmr12"##,
            r##"CB read_font_file DIR/cmr10.tfm"##,
            r##"MSG wd 0"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
}

/// a `find_font_file` that returns nil leaves the font unloadable and never reaches `read_font_file`.
#[test]
fn font_not_found_when_find_font_file_returns_nothing() {
    let r = run(false, r##"\directlua{FIND('font', 'tfm', function(n) return nil end) READ('font', 'tfm')}
\font\a=cmr12 \font\b=cmr12 at 11pt \setbox0\hbox{}
\directlua{MSG('max', font.max())}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_font_file cmr12"##,
            r##"CB find_font_file cmr12"##,
            r##"MSG max 0"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 2, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.has_error(r##"Font \a=cmr12 not loadable: metric data not found or bad"##), "{:?}", r.e.diagnostics);
}

/// `read_font_file` returning false: the font is not loadable, `find_vf_file` is not asked.
#[test]
fn read_font_file_refusal_makes_the_font_unloadable() {
    let r = run(false, r##"\directlua{FIND('font', 'tfm') callback.register('read_font_file', function(n) LOG('read_font_file', NORM(n)) return false end) FIND('vf', 'vf')}
\font\a=cmr12 \setbox0\hbox{}
\directlua{MSG('max', font.max())}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_font_file cmr12"##,
            r##"CB read_font_file DIR/cmr12.tfm"##,
            r##"MSG max 0"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 1, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.has_error(r##"Font \a=cmr12 not loadable: metric data not found or bad"##), "{:?}", r.e.diagnostics);
}

/// a virtual font found through `read_vf_file`: its local fonts are loaded with the same callbacks.
#[test]
fn virtual_font_loads_its_local_fonts_through_the_callbacks() {
    let r = run(false, r##"\directlua{FIND('font', 'tfm') READ('font', 'tfm') FIND('vf', 'vf') READ('vf', 'vf')}
\font\a=ptmr8t \setbox0\hbox{}
\directlua{MSG('max', font.max())}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_font_file ptmr8t"##,
            r##"CB read_font_file DIR/ptmr8t.tfm"##,
            r##"CB find_vf_file ptmr8t"##,
            r##"CB read_vf_file DIR/ptmr8t.vf"##,
            r##"CB find_font_file ptmr8r"##,
            r##"CB read_font_file DIR/ptmr8r.tfm"##,
            r##"CB find_vf_file ptmr8r"##,
            r##"CB read_vf_file ptmr8r"##,
            r##"MSG max 2"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
}

/// `find_vf_file` returning nil: no `read_vf_file`, no local fonts.
#[test]
fn find_vf_file_nothing_keeps_the_font_real() {
    let r = run(false, r##"\directlua{FIND('font', 'tfm') READ('font', 'tfm') FIND('vf', 'vf', function(n) return nil end) READ('vf', 'vf')}
\font\a=ptmr8t \setbox0\hbox{}
\directlua{MSG('max', font.max())}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_font_file ptmr8t"##,
            r##"CB read_font_file DIR/ptmr8t.tfm"##,
            r##"CB find_vf_file ptmr8t"##,
            r##"MSG max 1"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
}

/// `read_vf_file` returning false: the font stays real.
#[test]
fn read_vf_file_refusal_keeps_the_font_real() {
    let r = run(false, r##"\directlua{FIND('font', 'tfm') READ('font', 'tfm') FIND('vf', 'vf') READ('vf', 'vf', function(n) return nil end)}
\font\a=ptmr8t \setbox0\hbox{}
\directlua{MSG('max', font.max())}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_font_file ptmr8t"##,
            r##"CB read_font_file DIR/ptmr8t.tfm"##,
            r##"CB find_vf_file ptmr8t"##,
            r##"CB read_vf_file DIR/ptmr8t.vf"##,
            r##"MSG max 1"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
}

/// the VF bytes a callback serves for another name are the packets of the font.
#[test]
fn read_vf_file_data_makes_the_font_virtual() {
    let r = run(false, r##"\directlua{FIND('font', 'tfm') FIND('vf', 'vf', function(n) if n == 'ptmr8t' then return 'ptmr8t.vf' end return nil end) READ('vf', 'vf', function(n) return READALL(kpse.find_file('ptmr8t.vf', 'vf')) end)}
\font\a=ptmr8t \setbox0\hbox{}
\directlua{MSG('max', font.max())}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_font_file ptmr8t"##,
            r##"CB find_vf_file ptmr8t"##,
            r##"CB read_vf_file ptmr8t.vf"##,
            r##"CB find_font_file ptmr8r"##,
            r##"CB find_vf_file ptmr8r"##,
            r##"MSG max 2"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
}

/// `font.read_tfm` goes through `find_font_file`/`read_font_file` (no map, no virtual font).
#[test]
fn font_read_tfm_uses_the_callbacks_without_vf() {
    let r = run(false, r##"\directlua{FIND('font', 'tfm') READ('font', 'tfm') FIND('vf', 'vf') READ('vf', 'vf')}
\directlua{local t = font.read_tfm('cmr10', 655360) MSG('cs', t.checksum, t.characters[65].width) local v = font.read_vf('ptmr8t', 655360) MSG('vf', v and v.characters[65] and 1 or 0)}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_font_file cmr10"##,
            r##"CB read_font_file DIR/cmr10.tfm"##,
            r##"MSG cs 1274110073 491521"##,
            r##"CB find_vf_file ptmr8t"##,
            r##"CB read_vf_file DIR/ptmr8t.vf"##,
            r##"MSG vf 1"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
}

/// `do_vf` runs for a `define_font` table of unknown type (and only then).
#[test]
fn define_font_table_without_type_asks_for_a_vf() {
    let r = run(false, r##"\directlua{FIND('font', 'tfm') READ('font', 'tfm') FIND('vf', 'vf') READ('vf', 'vf')
callback.register('define_font', function(name, size, id) LOG('define_font', name, size, id) local f = font.read_tfm(name, size) f.name = name return f end)}
\font\a=cmr10 \font\b=ptmr8t \setbox0\hbox{}
\directlua{MSG('max', font.max())}"##);
    assert_eq!(
        r.events,
        [
            r##"CB define_font cmr10 -1000 1"##,
            r##"CB find_font_file cmr10"##,
            r##"CB read_font_file DIR/cmr10.tfm"##,
            r##"CB find_vf_file cmr10"##,
            r##"CB read_vf_file cmr10"##,
            r##"CB define_font ptmr8t -1000 2"##,
            r##"CB find_font_file ptmr8t"##,
            r##"CB read_font_file DIR/ptmr8t.tfm"##,
            r##"CB find_vf_file ptmr8t"##,
            r##"CB read_vf_file DIR/ptmr8t.vf"##,
            r##"CB define_font ptmr8r 655360 3"##,
            r##"CB find_font_file ptmr8r"##,
            r##"CB read_font_file DIR/ptmr8r.tfm"##,
            r##"CB find_vf_file ptmr8r"##,
            r##"CB read_vf_file ptmr8r"##,
            r##"MSG max 3"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
}

/// default map at `\pdfextension mapfile`, output file at the first shipout, encodings and Type 1 programs once the PDF is finished.
#[test]
fn map_enc_and_type1_callbacks_follow_luatex_order() {
    let r = run(true, r##"\directlua{FIND('font', 'tfm') READ('font', 'tfm') FIND('map', 'map') READ('map', 'map') FIND('enc', 'enc files') READ('enc', 'enc files') FIND('type1', 'type1 fonts') READ('type1', 'type1 fonts') FILES() PAGES()}
\font\a=cmr10 \pdfextension mapfile{+lm.map} \font\b=ec-lmr10 \font\c=cmr10 at 12pt \setbox0\hbox{}
\shipout\hbox{\a abc\b abc\c a}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_font_file cmr10"##,
            r##"CB read_font_file DIR/cmr10.tfm"##,
            r##"CB find_map_file pdftex.map"##,
            r##"CB read_map_file DIR/pdftex.map"##,
            r##"CB start_file 2 DIR/pdftex.map"##,
            r##"CB stop_file 2"##,
            r##"CB find_map_file lm.map"##,
            r##"CB read_map_file DIR/lm.map"##,
            r##"CB start_file 2 DIR/lm.map"##,
            r##"CB stop_file 2"##,
            r##"CB find_font_file ec-lmr10"##,
            r##"CB read_font_file DIR/ec-lmr10.tfm"##,
            r##"CB find_font_file cmr10"##,
            r##"CB read_font_file DIR/cmr10.tfm"##,
            r##"CB start_page"##,
            r##"CB stop_page"##,
            r##"CB finish_pdffile"##,
            r##"CB find_enc_file lm-ec.enc"##,
            r##"CB read_enc_file DIR/lm-ec.enc"##,
            r##"CB start_file 2 DIR/lm-ec.enc"##,
            r##"CB stop_file 2"##,
            r##"CB find_type1_file cmr10.pfb"##,
            r##"CB find_type1_file DIR/cmr10.pfb"##,
            r##"CB read_type1_file DIR/cmr10.pfb"##,
            r##"CB start_file 4 DIR/cmr10.pfb"##,
            r##"CB stop_file 4"##,
            r##"CB find_type1_file lmr10.pfb"##,
            r##"CB find_type1_file DIR/lmr10.pfb"##,
            r##"CB read_type1_file DIR/lmr10.pfb"##,
            r##"CB start_file 4 DIR/lmr10.pfb"##,
            r##"CB stop_file 4"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.pdf_flat().contains(r##"/Differences[97/a/b/c]"##));
}

/// without `\pdfmapfile` pdftex.map is requested while the page is shipped, before `finish_pdfpage`.
#[test]
fn default_map_is_read_when_the_first_font_is_initialized() {
    let r = run(true, r##"\directlua{FIND('font', 'tfm') FIND('map', 'map') READ('map', 'map') FIND('type1', 'type1 fonts') READ('type1', 'type1 fonts') FILES() PAGES()}
\font\a=cmr10 \setbox0\hbox{}
\shipout\hbox{\a ab}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_font_file cmr10"##,
            r##"CB start_page"##,
            r##"CB find_map_file pdftex.map"##,
            r##"CB read_map_file DIR/pdftex.map"##,
            r##"CB start_file 2 DIR/pdftex.map"##,
            r##"CB stop_file 2"##,
            r##"CB stop_page"##,
            r##"CB finish_pdffile"##,
            r##"CB find_type1_file cmr10.pfb"##,
            r##"CB find_type1_file DIR/cmr10.pfb"##,
            r##"CB read_type1_file DIR/cmr10.pfb"##,
            r##"CB start_file 4 DIR/cmr10.pfb"##,
            r##"CB stop_file 4"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
}

/// the encoding vector comes from the bytes `read_enc_file` delivers.
#[test]
fn read_enc_file_data_is_the_encoding() {
    let r = run(true, r##"\directlua{FIND('enc', 'enc files') READ('enc', 'enc files', function(n) local d = READALL(kpse.find_file(n, 'enc files') or n) local nl = string.char(10) return (d:gsub(nl .. '/b' .. nl, nl .. '/Foo' .. nl)) end)}
\font\b=ec-lmr10 \pdfextension mapfile{+lm.map} \setbox0\hbox{}
\shipout\hbox{\b abc}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_enc_file lm-ec.enc"##,
            r##"CB read_enc_file DIR/lm-ec.enc"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.pdf_flat().contains(r##"/Differences[97/a/Foo/c]"##));
    assert!(!r.pdf_flat().contains(r##"/Differences[97/a/b/c]"##));
}

/// the font program is what `read_type1_file` delivers: lmr10 under the name cmr10.pfb.
#[test]
fn read_type1_file_data_is_the_program() {
    let r = run(true, r##"\directlua{FIND('type1', 'type1 fonts') READ('type1', 'type1 fonts', function(n) return READALL(kpse.find_file('lmr10.pfb', 'type1 fonts')) end)}
\font\a=cmr10 \setbox0\hbox{}
\shipout\hbox{\a abc}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_type1_file cmr10.pfb"##,
            r##"CB find_type1_file DIR/cmr10.pfb"##,
            r##"CB read_type1_file DIR/cmr10.pfb"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.pdf_flat().contains(r##"LMRoman10-Regular"##));
    assert!(!r.pdf_flat().contains(r##"CMR10"##));
}

/// a map line served by `read_map_file` maps cmr10 to the Latin Modern program.
#[test]
fn read_map_file_data_is_the_map() {
    let r = run(true, r##"\directlua{FIND('map', 'map', function(n) return 'x.map' end) READ('map', 'map', function(n) if n == 'x.map' then return 'cmr10 LMRoman10-Regular <lmr10.pfb' .. string.char(10) end return READALL(kpse.find_file(n, 'map') or n) end)}
\font\a=cmr10 \pdfextension mapfile{x.map} \setbox0\hbox{}
\shipout\hbox{\a abc}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_map_file x.map"##,
            r##"CB read_map_file x.map"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.pdf_flat().contains(r##"LMRoman10-Regular"##));
    assert!(!r.pdf_flat().contains(r##"CMR10"##));
}

/// no encoding file: `error:  (type 1): cannot find encoding file 'lm-ec.enc' for reading`, no PDF.
#[test]
fn find_enc_file_nothing_is_a_fatal_error() {
    let r = run(true, r##"\directlua{FIND('enc', 'enc files', function(n) return nil end) READ('enc', 'enc files')}
\font\b=ec-lmr10 \pdfextension mapfile{+lm.map} \setbox0\hbox{}
\shipout\hbox{\b abc}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_enc_file lm-ec.enc"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 1, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.has_error(r##"error:  (type 1): cannot find encoding file 'lm-ec.enc' for reading"##), "{:?}", r.e.diagnostics);
}

/// `check_ff_exist` finds nothing: `cannot open file for reading 'cmr10.pfb'`.
#[test]
fn find_type1_file_nothing_is_a_fatal_error() {
    let r = run(true, r##"\directlua{FIND('type1', 'type1 fonts', function(n) return nil end) READ('type1', 'type1 fonts')}
\font\a=cmr10 \setbox0\hbox{}
\shipout\hbox{\a abc}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_type1_file cmr10.pfb"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 1, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.has_error(r##"error:  (type 1): cannot open file for reading 'cmr10.pfb'"##), "{:?}", r.e.diagnostics);
}

/// `read_type1_file` delivering nothing: the program cannot be read.
#[test]
fn empty_type1_data_is_unexpected_end_of_file() {
    let r = run(true, r##"\directlua{FIND('type1', 'type1 fonts') READ('type1', 'type1 fonts', function(n) return '' end) FILES()}
\font\a=cmr10 \setbox0\hbox{}
\shipout\hbox{\a abc}"##);
    assert_eq!(
        r.events,
        [
            r##"CB start_file 2 DIR/pdftex.map"##,
            r##"CB stop_file 2"##,
            r##"CB find_type1_file cmr10.pfb"##,
            r##"CB find_type1_file DIR/cmr10.pfb"##,
            r##"CB read_type1_file DIR/cmr10.pfb"##,
            r##"CB start_file 4 DIR/cmr10.pfb"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 1, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.has_error(r##"(type 1): unexpected end of file"##), "{:?}", r.e.diagnostics);
}

/// two fonts, one program: encodings and programs are asked for once.
#[test]
fn repeated_font_files_are_read_once() {
    let r = run(true, r##"\directlua{FIND('enc', 'enc files') READ('enc', 'enc files') FIND('type1', 'type1 fonts') READ('type1', 'type1 fonts')}
\pdfextension mapfile{+lm.map}
\font\a=ec-lmr10 \font\b=ec-lmr10 at 12pt \font\c=ec-lmr10 at 14pt \setbox0\hbox{}
\shipout\hbox{\a a\b b\c c}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_enc_file lm-ec.enc"##,
            r##"CB read_enc_file DIR/lm-ec.enc"##,
            r##"CB find_type1_file lmr10.pfb"##,
            r##"CB find_type1_file DIR/lmr10.pfb"##,
            r##"CB read_type1_file DIR/lmr10.pfb"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
}

/// an OpenType Lua font (subset): `writetype0` asks `find_opentype_file`, then `read_opentype_file`; subset files are category 4.
#[test]
fn opentype_cid_font_goes_through_find_and_read_opentype_file() {
    let r = run(true, r##"\directlua{FINDPICK = nil READPICK = nil FILENAME = 'lmroman10-regular.otf' FORMAT = 'opentype' EMBEDDING = 'subset' }
\directlua{FIND('opentype', 'opentype fonts', FINDPICK) READ('opentype', 'opentype fonts', READPICK) FIND('truetype', 'truetype fonts') READ('truetype', 'truetype fonts') FILES() PAGES()
callback.register('define_font', function(name, size, id)
 return {name = name, size = size > 0 and size or 655360, designsize = 655360, filename = FILENAME, format = FORMAT, embedding = EMBEDDING,
  encodingbytes = 2, psname = 'LMRoman10-Regular', fullname = 'LMRoman10-Regular', type = 'real',
  parameters = {slant = 0, space = 200000, space_stretch = 100000, space_shrink = 50000, x_height = 300000, quad = 655360, extra_space = 0},
  characters = {[97] = {width = 300000, height = 400000, depth = 0, index = 68, unicode = 97}, [98] = {width = 300000, height = 400000, depth = 0, index = 69, unicode = 98}}}
end)}
\font\a=lmroman10-regular at 10pt \setbox0\hbox{}
\shipout\hbox{\a ab}"##);
    assert_eq!(
        r.events,
        [
            r##"CB start_page"##,
            r##"CB start_file 2 DIR/pdftex.map"##,
            r##"CB stop_file 2"##,
            r##"CB stop_page"##,
            r##"CB finish_pdffile"##,
            r##"CB find_opentype_file lmroman10-regular.otf"##,
            r##"CB read_opentype_file DIR/lmroman10-regular.otf"##,
            r##"CB start_file 4 DIR/lmroman10-regular.otf"##,
            r##"CB stop_file 4"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.pdf_flat().contains(r##"LMRoman10-Regular"##));
}

/// a font embedded in full is reported to `start_file` with category 5.
#[test]
fn full_embedding_reports_category_5() {
    let r = run(true, r##"\directlua{FINDPICK = nil READPICK = nil FILENAME = 'lmroman10-regular.otf' FORMAT = 'opentype' EMBEDDING = 'full' }
\directlua{FIND('opentype', 'opentype fonts', FINDPICK) READ('opentype', 'opentype fonts', READPICK) FIND('truetype', 'truetype fonts') READ('truetype', 'truetype fonts') FILES() PAGES()
callback.register('define_font', function(name, size, id)
 return {name = name, size = size > 0 and size or 655360, designsize = 655360, filename = FILENAME, format = FORMAT, embedding = EMBEDDING,
  encodingbytes = 2, psname = 'LMRoman10-Regular', fullname = 'LMRoman10-Regular', type = 'real',
  parameters = {slant = 0, space = 200000, space_stretch = 100000, space_shrink = 50000, x_height = 300000, quad = 655360, extra_space = 0},
  characters = {[97] = {width = 300000, height = 400000, depth = 0, index = 68, unicode = 97}, [98] = {width = 300000, height = 400000, depth = 0, index = 69, unicode = 98}}}
end)}
\font\a=lmroman10-regular at 10pt \setbox0\hbox{}
\shipout\hbox{\a ab}"##);
    assert_eq!(
        r.events,
        [
            r##"CB start_page"##,
            r##"CB start_file 2 DIR/pdftex.map"##,
            r##"CB stop_file 2"##,
            r##"CB stop_page"##,
            r##"CB finish_pdffile"##,
            r##"CB find_opentype_file lmroman10-regular.otf"##,
            r##"CB read_opentype_file DIR/lmroman10-regular.otf"##,
            r##"CB start_file 5 DIR/lmroman10-regular.otf"##,
            r##"CB stop_file 5"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
}

/// `writetype0` asks `find_truetype_file` when `find_opentype_file` finds nothing.
#[test]
fn find_opentype_file_nothing_falls_back_to_truetype() {
    let r = run(true, r##"\directlua{FINDPICK = nil READPICK = nil FILENAME = 'lmroman10-regular.otf' FORMAT = 'opentype' EMBEDDING = 'subset' FINDPICK = function(n) return nil end}
\directlua{FIND('opentype', 'opentype fonts', FINDPICK) READ('opentype', 'opentype fonts', READPICK) FIND('truetype', 'truetype fonts') READ('truetype', 'truetype fonts') FILES() PAGES()
callback.register('define_font', function(name, size, id)
 return {name = name, size = size > 0 and size or 655360, designsize = 655360, filename = FILENAME, format = FORMAT, embedding = EMBEDDING,
  encodingbytes = 2, psname = 'LMRoman10-Regular', fullname = 'LMRoman10-Regular', type = 'real',
  parameters = {slant = 0, space = 200000, space_stretch = 100000, space_shrink = 50000, x_height = 300000, quad = 655360, extra_space = 0},
  characters = {[97] = {width = 300000, height = 400000, depth = 0, index = 68, unicode = 97}, [98] = {width = 300000, height = 400000, depth = 0, index = 69, unicode = 98}}}
end)}
\font\a=lmroman10-regular at 10pt \setbox0\hbox{}
\shipout\hbox{\a ab}"##);
    assert_eq!(
        r.events,
        [
            r##"CB start_page"##,
            r##"CB start_file 2 DIR/pdftex.map"##,
            r##"CB stop_file 2"##,
            r##"CB stop_page"##,
            r##"CB finish_pdffile"##,
            r##"CB find_opentype_file lmroman10-regular.otf"##,
            r##"CB find_truetype_file lmroman10-regular.otf"##,
            r##"CB read_opentype_file DIR/lmroman10-regular.otf"##,
            r##"CB start_file 4 DIR/lmroman10-regular.otf"##,
            r##"CB stop_file 4"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
}

/// the program is what `read_opentype_file` delivers (`lmroman10-regular.otf` served under another name).
#[test]
fn read_opentype_file_data_is_the_font_program() {
    let r = run(true, r##"\directlua{FINDPICK = nil READPICK = nil FILENAME = 'lmroman10-regular.otf' FORMAT = 'opentype' EMBEDDING = 'subset' FILENAME = 'nothere.otf' FINDPICK = function(n) return n end READPICK = function(n) return READALL(kpse.find_file('lmroman10-regular.otf', 'opentype fonts')) end}
\directlua{FIND('opentype', 'opentype fonts', FINDPICK) READ('opentype', 'opentype fonts', READPICK) FIND('truetype', 'truetype fonts') READ('truetype', 'truetype fonts') FILES() PAGES()
callback.register('define_font', function(name, size, id)
 return {name = name, size = size > 0 and size or 655360, designsize = 655360, filename = FILENAME, format = FORMAT, embedding = EMBEDDING,
  encodingbytes = 2, psname = 'LMRoman10-Regular', fullname = 'LMRoman10-Regular', type = 'real',
  parameters = {slant = 0, space = 200000, space_stretch = 100000, space_shrink = 50000, x_height = 300000, quad = 655360, extra_space = 0},
  characters = {[97] = {width = 300000, height = 400000, depth = 0, index = 68, unicode = 97}, [98] = {width = 300000, height = 400000, depth = 0, index = 69, unicode = 98}}}
end)}
\font\a=lmroman10-regular at 10pt \setbox0\hbox{}
\shipout\hbox{\a ab}"##);
    assert_eq!(
        r.events,
        [
            r##"CB start_page"##,
            r##"CB start_file 2 DIR/pdftex.map"##,
            r##"CB stop_file 2"##,
            r##"CB stop_page"##,
            r##"CB finish_pdffile"##,
            r##"CB find_opentype_file nothere.otf"##,
            r##"CB read_opentype_file nothere.otf"##,
            r##"CB start_file 4 nothere.otf"##,
            r##"CB stop_file 4"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.pdf_flat().contains(r##"LMRoman10-Regular"##));
}

/// `read_opentype_file` refusing: `error:  (file ...) (type 0): cannot find file`.
#[test]
fn unreadable_opentype_program_is_a_fatal_error() {
    let r = run(true, r##"\directlua{FINDPICK = nil READPICK = nil FILENAME = 'lmroman10-regular.otf' FORMAT = 'opentype' EMBEDDING = 'subset' READPICK = function(n) return nil end}
\directlua{FIND('opentype', 'opentype fonts', FINDPICK) READ('opentype', 'opentype fonts', READPICK) FIND('truetype', 'truetype fonts') READ('truetype', 'truetype fonts') FILES() PAGES()
callback.register('define_font', function(name, size, id)
 return {name = name, size = size > 0 and size or 655360, designsize = 655360, filename = FILENAME, format = FORMAT, embedding = EMBEDDING,
  encodingbytes = 2, psname = 'LMRoman10-Regular', fullname = 'LMRoman10-Regular', type = 'real',
  parameters = {slant = 0, space = 200000, space_stretch = 100000, space_shrink = 50000, x_height = 300000, quad = 655360, extra_space = 0},
  characters = {[97] = {width = 300000, height = 400000, depth = 0, index = 68, unicode = 97}, [98] = {width = 300000, height = 400000, depth = 0, index = 69, unicode = 98}}}
end)}
\font\a=lmroman10-regular at 10pt \setbox0\hbox{}
\shipout\hbox{\a ab}"##);
    assert_eq!(
        r.events,
        [
            r##"CB start_page"##,
            r##"CB start_file 2 DIR/pdftex.map"##,
            r##"CB stop_file 2"##,
            r##"CB stop_page"##,
            r##"CB finish_pdffile"##,
            r##"CB find_opentype_file lmroman10-regular.otf"##,
            r##"CB read_opentype_file DIR/lmroman10-regular.otf"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 1, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.has_error(r##"(type 0): cannot find file"##), "{:?}", r.e.diagnostics);
}

/// `find_image_file` runs for every `\saveimageresource`; PNG and JPEG files are reported to `start_file` with category 3 when the page writes them.
#[test]
fn find_image_file_names_the_image() {
    let r = run(true, r##"\directlua{FIND('image', nil) FILES() PAGES()}
\saveimageresource{example-image-a.pdf}
\saveimageresource{example-image-b.png}
\saveimageresource{example-image-a.jpg}
\saveimageresource{example-image-a.jpg}
\shipout\hbox{\useimageresource\lastsavedimageresourceindex}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_image_file example-image-a.pdf"##,
            r##"CB find_image_file example-image-b.png"##,
            r##"CB find_image_file example-image-a.jpg"##,
            r##"CB find_image_file example-image-a.jpg"##,
            r##"CB start_page"##,
            r##"CB start_file 3 DIR/example-image-a.jpg"##,
            r##"CB stop_file 3"##,
            r##"CB stop_page"##,
            r##"CB finish_pdffile"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
}

/// a `find_image_file` result that is not a string: `cannot find image file`.
#[test]
fn find_image_file_nothing_is_fatal() {
    let r = run(true, r##"\directlua{callback.register('find_image_file', function(n) LOG('find_image_file', NORM(n)) return nil end)}
\saveimageresource{example-image-a.pdf}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_image_file example-image-a.pdf"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 1, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.has_error(r##"(pdf backend): cannot find image file 'example-image-a.pdf'"##), "{:?}", r.e.diagnostics);
}

/// the image is read from the path the callback returned: `reading image file 'nothere.pdf' failed`.
#[test]
fn find_image_file_nonexistent_path_fails_reading() {
    let r = run(true, r##"\directlua{callback.register('find_image_file', function(n) LOG('find_image_file', NORM(n)) return 'nothere.pdf' end)}
\saveimageresource{example-image-a.pdf}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_image_file example-image-a.pdf"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 1, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.has_error(r##"(pdf backend): reading image file 'nothere.pdf' failed"##), "{:?}", r.e.diagnostics);
}

/// `\pdfextension obj file`: the output file opens first, then `find_data_file` gets the typed name and `read_data_file` the name it returned; the data is in the PDF.
#[test]
fn find_data_file_and_read_data_file_serve_pdfobj_files() {
    let r = run(true, r##"\directlua{OUT() FIND('data', 'tex', function(n) return 'found:' .. n end) READ('data', 'tex', function(n) return 'SERVED DATA' end)}
\immediate\pdfextension obj file {dummy.dat}
\shipout\hbox{x}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_output_file job.pdf"##,
            r##"CB find_data_file dummy.dat"##,
            r##"CB read_data_file found:dummy.dat"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.pdf_flat().contains(r##"SERVEDDATA"##));
}

/// a refusing `read_data_file`: `cannot open file for embedding`.
#[test]
fn read_data_file_refusal_is_fatal() {
    let r = run(true, r##"\directlua{FIND('data', 'tex', function(n) return n end) READ('data', 'tex', function(n) return nil end)}
\immediate\pdfextension obj file {dummy.dat}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_data_file dummy.dat"##,
            r##"CB read_data_file dummy.dat"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 1, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.has_error(r##"(pdf backend): cannot open file for embedding"##), "{:?}", r.e.diagnostics);
}

/// `read_data_file` with an empty file: `empty file for embedding`.
#[test]
fn empty_data_is_an_error() {
    let r = run(true, r##"\directlua{FIND('data', 'tex', function(n) return n end) READ('data', 'tex', function(n) return '' end)}
\immediate\pdfextension obj file {dummy.dat}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_data_file dummy.dat"##,
            r##"CB read_data_file dummy.dat"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 1, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.has_error(r##"(pdf backend): empty file for embedding"##), "{:?}", r.e.diagnostics);
}

/// `find_output_file` gets `jobname.pdf` once, before `start_page_number` of the first page.
#[test]
fn find_output_file_runs_before_the_first_page() {
    let r = run(true, r##"\directlua{OUT() PAGES()}
\shipout\hbox{x}\shipout\hbox{y}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_output_file job.pdf"##,
            r##"CB start_page"##,
            r##"CB stop_page"##,
            r##"CB start_page"##,
            r##"CB stop_page"##,
            r##"CB finish_pdffile"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
}

/// a `find_output_file` that returns nil: `I can't write on file`, no output.
#[test]
fn find_output_file_nothing_is_a_write_error() {
    let r = run(true, r##"\directlua{OUT(function(n) return nil end) PAGES()}
\shipout\hbox{x}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_output_file job.pdf"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 1, "{:?}\n{}", r.e.diagnostics, r.e.term);
    assert!(r.has_error(r##"I can't write on file `job.pdf'"##), "{:?}", r.e.diagnostics);
}

/// an immediate PDF object writes to the output file, so the file is opened for it.
#[test]
fn find_output_file_immediate_object_opens_the_file() {
    let r = run(true, r##"\directlua{OUT() PAGES()}
\immediate\pdfextension obj{(x)}
\shipout\hbox{x}"##);
    assert_eq!(
        r.events,
        [
            r##"CB find_output_file job.pdf"##,
            r##"CB start_page"##,
            r##"CB stop_page"##,
            r##"CB finish_pdffile"##,
        ],
        "{}",
        r.e.term
    );
    assert_eq!(r.e.error_count, 0, "{:?}\n{}", r.e.diagnostics, r.e.term);
}

/// luatex `lua_b_open_out`: the file `find_output_file` returns is the one the
/// PDF is written to (`pdf_output_file_override`); luatex wrote `renamed.pdf`
/// for this source (`-jobname=job`) and no `job.pdf`.
#[test]
fn find_output_file_result_names_the_pdf() {
    let r = run(true, r##"\directlua{OUT(function(n) return 'renamed.pdf' end)}
\shipout\hbox{x}"##);
    assert_eq!(r.e.error_count, 0, "{:?}", r.e.diagnostics);
    assert_eq!(r.e.pdf_output_file_override(), Some("renamed.pdf"));
}

/// luatex `zopen_w_input`: `find_format_file("fm1.fmt")` is asked for the
/// format name; a string names the file, nil or false leave no format.
/// The callback runs before any format exists, so it is tested through the
/// engine entry point the format loader uses; the ratex command line has no
/// Lua initialization script (`--lua`) that could register it earlier.
#[test]
fn find_format_file_maps_results_like_luatex() {
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    assert_eq!(e.lua_find_format_file("fm1.fmt"), None);
    for (code, expect) in [
        ("return n", Some(Some("fm1.fmt"))),
        ("return 'other.fmt'", Some(Some("other.fmt"))),
        ("return nil", Some(None)),
        ("return false", Some(None)),
        ("return ''", Some(None)),
    ] {
        e.execute_directlua(format!("callback.register('find_format_file', function(n) {code} end)").as_bytes()).unwrap();
        assert_eq!(e.lua_find_format_file("fm1.fmt"), expect.map(|o| o.map(String::from)), "{code}");
    }
}
