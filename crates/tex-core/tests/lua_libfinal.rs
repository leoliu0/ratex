//! Lua library gaps against TeX Live 2026 `luatex`: every `tests/lua_libfinal/NAME.expected`
//! was produced by `tests/lua_libfinal/gen.sh NAME` (luatex --ini) from the fixture
//! `NAME.lua`, which the engine runs here with `P(...)` collecting lines.

use tex_core::engine::{Engine, EngineKind};
use tex_core::{set_shell_escape, ShellEscape};

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

fn run_fixture(name: &str) -> String {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lua_libfinal");
    let base = std::env::temp_dir().join(format!("ratex-lua-libfinal-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let fixture = std::fs::read_to_string(root.join(format!("{name}.lua"))).unwrap();
    // luatex refuses absolute output paths (openout_any=p): write relative to TEXMFOUTPUT
    std::env::set_var("TEXMFOUTPUT", &base);
    let out = base.join(format!("{name}.out"));
    let script = format!(
        "lfs.chdir('{}')\nlocal out = {{}}\nfunction P(...) local t = table.pack(...) for i = 1, t.n do t[i] = tostring(t[i]) end out[#out+1] = table.concat(t, ' | ') end\n{fixture}\nlocal f = io.open('{name}.out', 'wb') f:write(table.concat(out, '\\n')) f:close()\n",
        base.display()
    );
    let file = base.join(format!("{name}.script.lua"));
    std::fs::write(&file, script).unwrap();
    let cwd = std::env::current_dir().unwrap();
    let mut e = boot_lua();
    e.input.push_file("t.tex".to_string(), format!("\\directlua{{dofile('{}')}}\\end\n", file.display()).into_bytes());
    e.run();
    std::env::set_current_dir(cwd).unwrap();
    assert_eq!(e.error_count, 0, "errors: {:?}, term: {}", e.diagnostics, e.term);
    String::from_utf8_lossy(&std::fs::read(&out).expect("no output")).into_owned()
}

fn check(name: &str) {
    let expected = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/lua_libfinal/{name}.expected"))).unwrap();
    let got = run_fixture(name);
    for (n, (a, b)) in expected.lines().zip(got.lines()).enumerate() {
        assert_eq!(a, b, "{name}: line {}", n + 1);
    }
    assert_eq!(expected.lines().count(), got.lines().count(), "{name}: line count");
}

// luatex --shell-escape
#[test]
fn os_execute_returns_the_wait_status() {
    set_shell_escape(ShellEscape::Enabled);
    check("os_execute");
}

// luatex (restricted default): the module function insists on a string
#[test]
fn kpse_find_file_needs_a_file_name() {
    check("kpse_find_file");
}

#[test]
fn status_reports_the_destination_table_size() {
    check("status_dest");
}

// luatex default policy (shell_escape=p): only the shell_escape_commands list runs
#[test]
fn os_commands_follow_the_restricted_policy() {
    set_shell_escape(ShellEscape::Restricted);
    check("os_restricted");
}

// Lua 5.3 llex.c / lparser.c message texts as luatex prints them
#[test]
fn parser_errors_use_lua53_texts() {
    check("parser_errors");
}
