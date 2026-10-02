//! The `pdfe` library. Every expectation is the output of LuaTeX 1.24
//! (TeX Live 2026) for the same Lua script and PDF fixture:
//!
//!   luatex --luaonly run.lua   with
//!   run.lua = FIXDIR="tests/fixtures/pdfe/" OUT="expected/<name>.txt"
//!             [DEPTH=0] arg={"<name>.pdf"} dofile("probe.lua")
//!
//! `probe.lua` calls every function of the library on a PDF and on everything
//! reachable from the trailer, catalog, info and page dictionaries; `edge.lua`
//! feeds each function assorted (wrong) arguments. Calls where LuaTeX itself
//! crashes (a closed document, an array as a stream, `pdfe.new(s, -1)`) are
//! not in the scripts.
use tex_core::engine::{Engine, EngineKind};

fn boot_lua() -> Engine {
    // the probe writes to an absolute scratch path, which TeX Live's default
    // `openout_any = p` (also applied to Lua's io.open) refuses
    std::env::set_var("openout_any", "a");
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    e.eqtb.cat[b'{' as usize] = 1;
    e.eqtb.cat[b'}' as usize] = 2;
    e.eqtb.cat[b'#' as usize] = 6;
    e.eqtb.cat[b' ' as usize] = 10;
    e.eqtb.cat[b'\n' as usize] = 5;
    e.eqtb.cat[b'\r' as usize] = 5;
    e
}

fn fixtures() -> String {
    format!("{}/tests/fixtures/pdfe/", env!("CARGO_MANIFEST_DIR").replace('\\', "/"))
}

/// Run `lua` with `\directlua`; the engine afterwards.
fn run_lua(lua: &str) -> Engine {
    let mut e = boot_lua();
    let src = format!("\\directlua{{{lua}}}\n\\end\n");
    e.input.push_file("t.tex".to_string(), src.into_bytes());
    e.run();
    e
}

fn out_path(name: &str) -> String {
    format!("{}/pdfe-{name}-{}.txt", env!("CARGO_TARGET_TMPDIR").replace('\\', "/"), std::process::id())
}

/// Run the fixture script `script` with `globals` and return what it wrote
/// to OUT together with the engine.
fn run_script(name: &str, globals: &str, script: &str) -> (String, Engine) {
    let dir = fixtures();
    let out = out_path(name);
    let e = run_lua(&format!("FIXDIR=\"{dir}\" OUT=\"{out}\" {globals} dofile(\"{dir}{script}\")"));
    assert_eq!(e.error_count, 0, "errors: {:?}, term: {}", e.diagnostics, e.term);
    let text = String::from_utf8_lossy(&std::fs::read(&out).unwrap_or_else(|err| panic!("{out}: {err}; term: {}", e.term))).into_owned();
    let _ = std::fs::remove_file(&out);
    (text, e)
}

fn warnings(e: &Engine) -> Vec<String> {
    e.term.lines().filter(|l| l.starts_with("warning  (pdfe")).map(str::to_owned).collect()
}

fn assert_same(what: &str, got: &str, expected: &str) {
    if got == expected {
        return;
    }
    let (g, x): (Vec<_>, Vec<_>) = (got.lines().collect(), expected.lines().collect());
    for (i, (a, b)) in g.iter().zip(&x).enumerate() {
        assert_eq!(a, b, "{what}: line {} differs", i + 1);
    }
    assert_eq!(g.len(), x.len(), "{what}: line count");
}

/// `warns`: pplib's own messages luatex prints for the script (none for sound files).
fn probe(name: &str, depth: Option<u32>, warns: &[&str]) {
    let globals = match depth {
        Some(d) => format!("DEPTH={d} arg={{\"{name}.pdf\"}}"),
        None => format!("arg={{\"{name}.pdf\"}}"),
    };
    let (got, e) = run_script(name, &globals, "probe.lua");
    let expected = String::from_utf8_lossy(&std::fs::read(format!("{}expected/{name}.txt", fixtures())).unwrap()).into_owned();
    assert_same(name, &got, &expected);
    assert_eq!(warnings(&e), warns, "{name}");
}

#[test]
fn plain_pdf_matches_luatex() {
    probe("obj", None, &[]);
}

#[test]
fn streams_and_filters_match_luatex() {
    probe("streams", None, &[]);
}

#[test]
fn incremental_update_matches_luatex() {
    probe("incr", Some(0), &[]);
}

#[test]
fn object_streams_match_luatex() {
    probe("objstm", Some(0), &[]);
}

#[test]
fn encrypted_with_empty_user_password_matches_luatex() {
    for name in ["enc-rc4", "enc-aes128", "enc-aes256"] {
        probe(name, Some(0), &[]);
    }
}

#[test]
fn password_protected_documents_match_luatex() {
    for name in ["encu-rc4", "encu-aes128", "encu-aes256"] {
        // a locked document: luatex warns once when the script passes the missing catalog on
        probe(name, Some(0), &["warning  (pdfe lib): lua <pdfe dictionary> expected"]);
    }
}

#[test]
fn argument_handling_matches_luatex() {
    let (got, e) = run_script("edge", "", "edge.lua");
    let expected = String::from_utf8_lossy(&std::fs::read(format!("{}expected/edge.txt", fixtures())).unwrap()).into_owned();
    assert_same("edge", &got, &expected);

    let dir = fixtures();
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for w in warnings(&e) {
        *counts.entry(w.replace(&dir, "<FIXDIR>/")).or_default() += 1;
    }
    let got: Vec<String> = counts.into_iter().map(|(w, n)| format!("{n} {w}")).collect();
    let expected = std::fs::read_to_string(format!("{dir}expected/edge.warnings.txt")).unwrap();
    let mut expected: Vec<&str> = expected.lines().collect();
    expected.sort_by(|a, b| a.split_once(' ').unwrap().1.cmp(b.split_once(' ').unwrap().1));
    assert_eq!(got, expected);
}

#[test]
fn library_and_userdata_members_equal_luatex() {
    let dir = fixtures();
    let out = out_path("members");
    let e = run_lua(&format!(
        "local f=io.open([[{out}]],'wb') \
         local d=pdfe.open([[{dir}obj.pdf]]) \
         local page=pdfe.getpage(d,1) \
         local extra=pdfe.getarray(pdfe.getcatalog(d),'Extra') \
         local s=pdfe.getstream(pdfe.getarray(page,'Contents'),0) or pdfe.getstream(page,'Contents') \
         local _,r=pdfe.getfromdictionary(pdfe.gettrailer(d),'Root') \
         for _,x in ipairs{{d,page,extra,s,r}} do \
           local mt=getmetatable(x) local ks={{}} \
           for k,v in pairs(mt) do ks[\\string#ks+1]=k..':'..type(v) end table.sort(ks) \
           f:write(pdfe.type(x),' ',table.concat(ks,','),'\\string\\n') end \
         local ks={{}} for k in pairs(pdfe) do ks[\\string#ks+1]=k end table.sort(ks) \
         f:write(\\string#ks,' ',table.concat(ks,' '),'\\string\\n') \
         f:write(tostring(package.loaded.pdfe==pdfe),'\\string\\n') f:close()"
    ));
    assert_eq!(e.error_count, 0, "{:?} {}", e.diagnostics, e.term);
    let got = String::from_utf8_lossy(&std::fs::read(&out).unwrap()).into_owned();
    let _ = std::fs::remove_file(&out);
    // luatex --luaonly with the same code
    let expected = "pdfe __gc:function,__index:function,__name:string,__tostring:function\n\
pdfe.dictionary __index:function,__len:function,__name:string,__tostring:function\n\
pdfe.array __index:function,__len:function,__name:string,__tostring:function\n\
pdfe.stream __call:function,__index:function,__len:function,__name:string,__tostring:function\n\
pdfe.reference __name:string,__tostring:function\n\
36 arraytotable close closestream dictionarytotable getarray getboolean getbox getcatalog getdictionary getfromarray getfromdictionary getfromreference getfromstream getinfo getinteger getmemoryusage getname getnofobjects getnofpages getnumber getpage getpages getsize getstatus getstream getstring gettrailer getversion new open openstream pagestotable readfromstream readwholestream type unencrypt\n\
true\n";
    assert_eq!(got, expected);
}

#[test]
fn tostring_shows_type_and_pointer_or_number() {
    let dir = fixtures();
    let out = out_path("tostring");
    let e = run_lua(&format!(
        "local f=io.open([[{out}]],'wb') \
         local d=pdfe.open([[{dir}obj.pdf]]) \
         local t=pdfe.gettrailer(d) local _,r=pdfe.getfromdictionary(t,'Root') \
         local page=pdfe.getpage(d,1) \
         f:write(tostring(d),'\\string\\n',tostring(t),'\\string\\n',tostring(pdfe.getarray(t,'ID')),'\\string\\n',tostring(r),'\\string\\n') \
         pdfe.close(d) f:write(tostring(d),'\\string\\n') f:close()"
    ));
    assert_eq!(e.error_count, 0, "{:?} {}", e.diagnostics, e.term);
    let got = String::from_utf8_lossy(&std::fs::read(&out).unwrap()).into_owned();
    let _ = std::fs::remove_file(&out);
    let lines: Vec<&str> = got.lines().collect();
    let hex = |s: &str, prefix: &str| s.strip_prefix(prefix).and_then(|r| r.strip_suffix('>')).is_some_and(|p| p.starts_with("0x") && p[2..].bytes().all(|b| b.is_ascii_hexdigit()));
    assert!(hex(lines[0], "<pdfe "), "{}", lines[0]);
    assert!(hex(lines[1], "<pdfe.dictionary "), "{}", lines[1]);
    assert!(hex(lines[2], "<pdfe.array "), "{}", lines[2]);
    assert_eq!(lines[3], "<pdfe.reference 1>");
    // LuaTeX prints a closed document's NULL pointer with %p
    assert_eq!(lines[4], "<pdfe (nil)>");
}

/// `pdfe.new(stream, length, id)` names the document for image inclusion:
/// `data:application/pdf,` + id + the djb2 checksum of the bytes (as in
/// luatex: `pdfe.new(obj.pdf, #obj.pdf, "pdf-glyph-00001")`).
#[test]
fn new_with_an_id_returns_the_image_file_name() {
    let dir = fixtures();
    let out = out_path("new");
    let e = run_lua(&format!(
        "local f=io.open([[{out}]],'wb') \
         local h=io.open([[{dir}obj.pdf]],'rb') local s=h:read('a') h:close() \
         f:write(pdfe.new(s,\\string#s,'pdf-glyph-00001'),'\\string\\n',pdfe.new(s,\\string#s,''),'\\string\\n',pdfe.new(s,\\string#s,'a'),'\\string\\n') f:close()"
    ));
    assert_eq!(e.error_count, 0, "{:?} {}", e.diagnostics, e.term);
    let got = String::from_utf8_lossy(&std::fs::read(&out).unwrap()).into_owned();
    let _ = std::fs::remove_file(&out);
    assert_eq!(
        got,
        "data:application/pdf,pdf-glyph-000019066af1c6362dc19\n\
         data:application/pdf,9066af1c6362dc19\n\
         data:application/pdf,a9066af1c6362dc19\n"
    );
}

#[test]
fn collected_documents_do_not_break_live_objects() {
    let dir = fixtures();
    let out = out_path("gc");
    let e = run_lua(&format!(
        "local f=io.open([[{out}]],'wb') \
         local page do local d=pdfe.open([[{dir}obj.pdf]]) page=pdfe.getpage(d,2) end \
         collectgarbage() collectgarbage() \
         f:write(pdfe.getname(page,'Type'),' ',\\string#page,'\\string\\n') f:close()"
    ));
    assert_eq!(e.error_count, 0, "{:?} {}", e.diagnostics, e.term);
    let got = String::from_utf8_lossy(&std::fs::read(&out).unwrap()).into_owned();
    let _ = std::fs::remove_file(&out);
    assert_eq!(got, "Page 11\n");
}
