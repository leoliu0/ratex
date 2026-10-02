//! Behaviour of the LuaTeX system libraries (`lfs`, `fio`, `md5`, `sha2`,
//! `zlib`, `gzip`, `zip`, `unicode`, `ltn12`, `mime`, `texconfig`, `status`,
//! `kpse`, `os`, `io`, `lua`), cross-checked against TeX Live 2026 luatex.

use tex_core::engine::{Engine, EngineKind};

fn boot_lua() -> Engine {
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

/// Runs a fixture script from `tests/lua_sys/NAME.lua` in the engine and
/// returns what it printed with `P(...)`. The scripts chdir into their own
/// scratch directory below `TEXMFOUTPUT`; the working directory is
/// process-global, hence the lock.
fn run_fixture(name: &str) -> String {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lua_sys");
    let base = std::env::temp_dir().join(format!("ratex-lua-sys-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    std::env::set_var("TEX_RS_HERMETIC", "1");
    let work = base.join(name);
    let _ = std::fs::remove_dir_all(&work);
    std::env::set_var("TEXMFOUTPUT", &base);
    let fixture = std::fs::read_to_string(root.join(format!("{name}.lua"))).unwrap();
    let script = format!(
        "lfs.chdir('{}')\nlocal out = {{}}\nfunction P(...) local t = table.pack(...) for i = 1, t.n do t[i] = tostring(t[i]) end out[#out+1] = table.concat(t, ' | ') end\n{fixture}\nlocal f = io.open('probe.out', 'wb') f:write(table.concat(out, '\\n')) f:close()\n",
        base.display().to_string().replace('\\', "/")
    );
    let file = base.join(format!("{name}.script.lua"));
    std::fs::write(&file, script).unwrap();
    let cwd = std::env::current_dir().unwrap();
    let mut e = boot_lua();
    e.input.push_file(
        "t.tex".to_string(),
        format!("\\directlua{{dofile('{}')}}\\end\n", file.display().to_string().replace('\\', "/")).into_bytes(),
    );
    e.run();
    std::env::set_current_dir(cwd).unwrap();
    assert_eq!(e.error_count, 0, "errors: {:?}, term: {}", e.diagnostics, e.term);
    String::from_utf8_lossy(&std::fs::read(work.join("probe.out")).expect("no output")).into_owned()
}

/// Compares against the output TeX Live 2026 `luatex --ini` produced for the
/// same script (`tests/lua_sys/NAME.expected`).
fn check(name: &str) {
    let expected = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/lua_sys/{name}.expected"))).unwrap();
    let got = run_fixture(name);
    for (n, (a, b)) in expected.lines().zip(got.lines()).enumerate() {
        assert_eq!(a, b, "{name}: line {}", n + 1);
    }
    assert_eq!(expected.lines().count(), got.lines().count(), "{name}: line count");
}

/// Member names and value types of every library luatex exports.
#[test]
fn library_members_match_luatex() {
    check("members");
}

/// POSIX only: hard and symbolic links, `utime`, `fcntl` locks and errno
/// texts, which the Windows build of `lfs` reports differently or not at all.
#[cfg(unix)]
#[test]
fn lfs_matches_luatex() {
    check("lfs");
}

#[test]
fn fio_unicode_mime_match_luatex() {
    check("fio_unicode_mime");
}

#[test]
fn md5_sha2_zlib_gzip_match_luatex() {
    check("hash_zlib");
}

#[test]
fn string_extensions_match_luatex() {
    check("string_ext");
}

#[test]
fn status_texconfig_os_io_match_luatex() {
    check("status_os");
}

#[test]
fn kpse_matches_luatex() {
    check("kpse");
}

#[test]
fn luaotfload_startup_check_passes() {
    // luaotfload-main.lua: `if status.safer_option ~= 0 then abort`
    let out = run_fixture("luaotfload_check");
    assert_eq!(out, "0");
}
