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
    std::fs::copy(root.join("a.png"), base.join("a.png")).unwrap();
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

fn ship(source: &str) -> Engine {
    let mut e = boot_lua();
    e.input.push_file("t.tex".to_string(), source.as_bytes().to_vec());
    e.run();
    assert_eq!(e.error_count, 0, "errors: {:?}, term: {}", e.diagnostics, e.term);
    e
}

// luatex --ini: the same source printed LL1 655360 11796480 / LL2 true / LL3 983040 11796480
// and wrote `q Q DBT\nTRET\n1 0 0 1 14.944 179.328 cm\nOZ` into the page
#[test]
fn latelua_runs_at_shipout_with_position_and_literals() {
    let e = ship(concat!(
        "\\directlua{tex.enableprimitives('', tex.extraprimitives()) tex.set('pagewidth', 200*65536) tex.set('pageheight', 200*65536) pdf.setorigin(0)}\n",
        "\\setbox0\\hbox to 100pt{\\kern 10pt\\latelua{texio.write_nl('LL1 '..table.concat({pdf.getpos()}, ' ')) pdf.print('page','q Q ') ",
        "texio.write_nl('LL2 '..tostring(pcall(pdf.print,'direct','D')))}\\kern 5pt",
        "\\latelua{texio.write_nl('LL3 '..pdf.gethpos()..' '..pdf.getvpos()) pdf.print('text','T') pdf.print('raw','R') ",
        "pdf.print('origin','O') pdf.print('Z')}}\n",
        "\\setbox1\\vbox{\\kern 20pt\\box0}\n\\shipout\\box1\n\\end\n"
    ));
    let term = e.term.replace('\n', "");
    assert!(term.contains("LL1 655360 11796480"), "{term}");
    assert!(term.contains("LL2 true"), "{term}");
    assert!(term.contains("LL3 983040 11796480"), "{term}");
    let content = String::from_utf8_lossy(&e.pdf_doc.pages[0].content).into_owned();
    assert_eq!(content, "q Q DBT\nTRET\n1 0 0 1 14.944 179.328 cm\nOZ");
}

/// Runs `tests/lua_libfinal/NAME.tex` and compares its `@@` log lines with the
/// ones luatex wrote for the same file.
fn check_tex(name: &str) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lua_libfinal");
    let source = std::fs::read_to_string(dir.join(format!("{name}.tex"))).unwrap();
    let expected = std::fs::read_to_string(dir.join(format!("{name}.expected"))).unwrap();
    let e = ship(&source);
    let got: Vec<String> = e.log.lines().filter_map(|l| l.strip_prefix("@@")).map(str::to_string).collect();
    let want: Vec<String> = expected.lines().map(str::to_string).collect();
    assert_eq!(got, want, "{name}");
}

// luatex: macro-definition scanning keeps match, end-match and out_param tokens
#[test]
fn scan_toks_reads_a_macro_definition() {
    check_tex("scan_toks");
}

// luatex: tex.finish stops the running chunk and the run
#[test]
fn tex_finish_aborts_the_chunk() {
    check_tex("finish");
}

// luatex: the image library (limglib.c); image objects are userdata
#[test]
fn img_library_matches_luatex() {
    check("img");
}
