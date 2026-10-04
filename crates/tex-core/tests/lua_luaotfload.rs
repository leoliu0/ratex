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
            out.display().to_string().replace('\\', "/")
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
        format!("\\directlua{{dofile('{}')}}\\end\n", file.display().to_string().replace('\\', "/")).into_bytes(),
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

/// Scanning Kpathsea's native path list must discover each face, including
/// mixed-case filenames that luaotfload lowercases on Windows. Names below
/// are from TeX Live 2026 `fontloader.info` on these same bundled OTF files.
#[test]
fn bundled_font_scan_preserves_name_and_style_metadata() {
    let got = run(r#"
require('lualibs')
local realpath = require('luaotfload-realpath').realpath
local wanted = {
  ['lmroman10-regular.otf'] = true, ['lmroman10-bold.otf'] = true,
  ['lmroman10-italic.otf'] = true, ['lmroman10-bolditalic.otf'] = true,
  ['lmmono10-regular.otf'] = true, ['FandolSong-Regular.otf'] = true,
}
local found = {}
for _, dir in ipairs(file.splitpath(kpse.expand_path(kpse.show_path('opentype fonts')))) do
  if lfs.isdir(dir) then
    dir = assert(realpath(dir))
    for name in lfs.dir(dir) do
      if wanted[name] then
        local path = dir .. '/' .. name
        if os.type == 'windows' then path = path:lower() end
        local meta = fontloader.info(path)
        local f = assert(io.open(path, 'rb'))
        assert(f:read(4) == 'OTTO')
        f:close()
        found[name] = meta.familyname .. ' | ' .. meta.fontname .. ' | ' .. meta.fullname
      end
    end
  end
end
for _, name in ipairs({
  'lmroman10-regular.otf', 'lmroman10-bold.otf', 'lmroman10-italic.otf',
  'lmroman10-bolditalic.otf', 'lmmono10-regular.otf', 'FandolSong-Regular.otf',
}) do
  P(name, (assert(found[name], 'undiscovered font: ' .. name)))
end
"#);
    assert_eq!(got.lines().collect::<Vec<_>>(), [
        "lmroman10-regular.otf | LM Roman 10 | LMRoman10-Regular | LMRoman10-Regular",
        "lmroman10-bold.otf | LM Roman 10 | LMRoman10-Bold | LMRoman10-Bold",
        "lmroman10-italic.otf | LM Roman 10 | LMRoman10-Italic | LMRoman10-Italic",
        "lmroman10-bolditalic.otf | LM Roman 10 | LMRoman10-BoldItalic | LMRoman10-BoldItalic",
        "lmmono10-regular.otf | LM Mono 10 | LMMono10-Regular | LMMono10-Regular",
        "FandolSong-Regular.otf | FandolSong | FandolSong-Regular | FandolSong",
    ]);
}

/// Native drive paths must survive expansion and directory-restricted
/// lookups rather than being split into `C` and `/...` search elements.
#[cfg(windows)]
#[test]
fn windows_drive_paths_survive_kpse_expansion_and_lookup() {
    let got = run(r#"
local root = kpse.expand_var('$TEXMFOUTPUT')
local first, second = root .. '/first', root .. '/second'
assert(lfs.mkdir(first) or lfs.isdir(first))
assert(lfs.mkdir(second) or lfs.isdir(second))
local f = assert(io.open(second .. '/font-path-probe.otf', 'wb'))
f:write('search result')
f:close()
local paths = kpse.expand_path(kpse.expand_braces(root .. '/{first,second}'))
local path = assert(kpse.lookup('font-path-probe.otf', { path = paths }))
local input = assert(io.open(path, 'rb'))
P(input:read('a'))
input:close()
"#);
    assert_eq!(got, "search result");
}
