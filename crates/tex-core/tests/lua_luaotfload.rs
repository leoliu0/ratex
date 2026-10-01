//! What luaotfload's bootstrap and font names database need from the system
//! libraries, over the embedded archive: a writable per-user cache
//! (`$TEXMFCACHE`/`$TEXMFVAR`) and the bundled fonts as a readable,
//! listable, read-only virtual TDS tree (`/<embedded>/…`).
//!
//! Expected values: TeX Live 2026 luahbtex reports the same file size (111536
//! bytes) and the same 72 files for `fonts/opentype/public/lm`, and resolves
//! `kpse.find_file('lmroman10-regular.otf','opentype fonts')` to that file in
//! its TDS tree.

use tex_core::engine::{Engine, EngineKind};

fn run(script: &str) -> String {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let base = std::env::temp_dir().join(format!("ratex-luaotfload-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let cache = base.join("cache");
    let file = base.join("probe.lua");
    let out = base.join("probe.out");
    let _ = std::fs::remove_file(&out);
    std::env::set_var("TEX_RS_CACHE_DIR", &cache);
    std::env::set_var("TEXMFOUTPUT", &base);
    std::env::set_var("TEX_RS_HERMETIC", "1");
    std::fs::write(
        &file,
        format!(
            "local out = {{}}\nfunction P(...) local t = table.pack(...) for i = 1, t.n do t[i] = tostring(t[i]) end out[#out+1] = table.concat(t, ' | ') end\n{script}\nlocal f = io.open('{}', 'wb') f:write(table.concat(out, '\\n')) f:close()\n",
            out.display()
        ),
    )
    .unwrap();
    let mut e = Engine::new_with_kind(EngineKind::LuaTeX, true);
    e.init_primitives();
    for (c, cat) in [(b'{', 1), (b'}', 2), (b'#', 6), (b' ', 10), (b'\n', 5)] {
        e.eqtb.cat[c as usize] = cat;
    }
    e.input.push_file(
        "t.tex".to_string(),
        format!("\\directlua{{dofile('{}')}}\\end\n", file.display()).into_bytes(),
    );
    e.run();
    assert_eq!(e.error_count, 0, "errors: {:?}, term: {}", e.diagnostics, e.term);
    String::from_utf8_lossy(&std::fs::read(&out).expect("no output")).into_owned()
}

/// `kpse.find_file` hands out paths of bundled files that `io.open`, `fio`
/// and `lfs` read consistently with the file system.
#[test]
fn bundled_font_is_found_statted_and_read() {
    let got = run(r#"
local path = kpse.find_file('lmroman10-regular.otf', 'opentype fonts')
P(path)
P(lfs.attributes(path, 'mode'), lfs.attributes(path, 'size'))
P(lfs.isfile(path), lfs.isdir(path:match('^(.*)/')))
local f = io.open(path, 'rb')
P(io.type(f), fio.readcardinal4(f), f:seek('end'))
f:seek('set', 0)
P(#f:read('a'))
f:close()
P(io.type(f), kpse.readable_file(path) == path)
P(io.open(path, 'wb'))
P(lfs.mkdir(path:match('^(.*)/') .. '/new'))
local n = 0
for name in lfs.dir(path:match('^(.*)/')) do if name ~= '.' and name ~= '..' then n = n + 1 end end
P(n)
"#);
    let lines: Vec<&str> = got.lines().collect();
    assert_eq!(lines[0], "/<embedded>/fonts/opentype/public/lm/lmroman10-regular.otf");
    assert_eq!(lines[1], "file | 111536");
    assert_eq!(lines[2], "true | true");
    assert_eq!(lines[3], "file | 1330926671 | 111536"); // 'OTTO'
    assert_eq!(lines[4], "111536");
    assert_eq!(lines[5], "closed file | true");
    // Restricted shell escape (luatex-core.lua's overlay): writes outside the
    // output areas are refused whatever the file system says.
    assert_eq!(lines[6], "nil");
    assert!(lines[7].starts_with("nil | "), "{}", lines[7]);
    assert_eq!(lines[8], "72");
}

/// The font search paths cover the bundled fonts, and the cache variables
/// luaotfload's `caches` setup reads name a directory it can create.
#[test]
fn search_paths_and_cache_variables() {
    let got = run(r#"
local dirs = kpse.expand_path(kpse.show_path('opentype fonts'))
P(dirs:find('/<embedded>/fonts/opentype/public/lm', 1, true) ~= nil)
local cache = kpse.expand_var('$TEXMFCACHE')
P(cache == kpse.expand_var('$TEXMFVAR'), cache:match('texmf%-var$') ~= nil)
lfs.mkdirp(cache .. '/')
P(lfs.isdir(cache), lfs.isdir(cache .. '/luatex-cache') == false)
P(kpse.out_name_ok_silent_extended(cache .. '/luatex-cache/generic/x'))
"#);
    assert_eq!(got.lines().collect::<Vec<_>>(), ["true", "true | true", "true | true", "true"]);
}
