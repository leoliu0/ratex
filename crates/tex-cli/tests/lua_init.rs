//! `lualatex -lua=FILE`: luatex runs the script before it looks for the
//! format, and the script's Lua state (callbacks) outlives the format load.
//! Expected traces are what `luatex --lua=init.lua` (LuaTeX 1.24.0) prints.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("luainit-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_pdflatex"), path.join("lualatex")).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.0.join(name), text).unwrap();
    }
    fn lualatex(&self, args: &[&str]) -> Output {
        Command::new(self.0.join("lualatex"))
            .args(args)
            .current_dir(&self.0)
            .env("TEX_RS_HERMETIC", "1")
            .env("TEX_RS_CACHE_DIR", self.0.join("cache"))
            .output()
            .unwrap()
    }
    /// A small format (`\hello` = HELLO) named `m.fmt`.
    fn make_format(&self) {
        self.write("mk.tex", "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\def\\hello{HELLO}\\dump\n");
        self.lualatex(&["-ini", "-jobname=m", "mk.tex"]);
        assert!(Path::new(&self.0.join("m.fmt")).is_file());
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn trace_lines(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.starts_with("TRACE ") || l.contains("format") || l.contains("callback should"))
        .map(str::to_string)
        .collect()
}

const INIT: &str = r#"
local function tr(...) io.stderr:write("TRACE ", table.concat({...}, " "), "\n") end
callback.register("start_run", function() tr("start_run") end)
callback.register("find_format_file", function(n) tr("find_format_file", n) return FOUND(n) end)
callback.register("find_read_file", function(id, n) tr("find_read_file", id, n) return n end)
"#;

/// The callbacks registered by the script run in luatex's order: `start_run`
/// (the banner), `find_format_file` (the format), then the job's
/// `find_read_file`. The format they load is the one the callback named.
#[test]
fn init_script_callbacks_survive_format_load() {
    let d = Dir::new("survive");
    d.make_format();
    d.write("init.lua", &format!("FOUND = function(n) return '{}/m.fmt' end\n{INIT}", d.0.display()));
    d.write("u.tex", "\\message{[\\hello]}\\end\n");
    let out = d.lualatex(&["-lua=init.lua", "-fmt=other", "-interaction=nonstopmode", "u.tex"]);
    assert_eq!(
        trace_lines(&out),
        ["TRACE start_run", "TRACE find_format_file other.fmt", "TRACE find_read_file 0 u.tex"]
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[HELLO]"), "{stdout}");
    assert!(!stdout.contains("This is LuaTeX"), "the start_run callback replaces the banner: {stdout}");
}

/// llualib.c dumps the bytecode registers into the format, including ones
/// a `pre_dump` callback sets (expl3's `register_luadata` stores the
/// Unicode data there). `luatex -ini` + `luatex -fmt` (LuaTeX 1.24.0)
/// prints `BC=dumped,direct,nil`.
#[test]
fn bytecode_registers_survive_dump() {
    let d = Dir::new("bytecode");
    d.write(
        "bc.tex",
        concat!(
            "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6\n",
            "\\directlua{lua.bytecode[8] = function() return 'direct' end\n",
            "callback.register('pre_dump', function() lua.bytecode[7] = load('return \"dumped\"') end)}\n",
            "\\dump\n",
        ),
    );
    let out = d.lualatex(&["-ini", "-jobname=bcf", "bc.tex"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    d.write(
        "use.tex",
        "\\directlua{texio.write_nl('BC=' .. lua.bytecode[7]() .. ',' .. lua.bytecode[8]() .. ',' .. tostring(lua.bytecode[9]))}\\end\n",
    );
    let out = d.lualatex(&["-fmt=bcf", "-interaction=nonstopmode", "use.tex"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("BC=dumped,direct,nil"), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
}

/// A `find_format_file` that gives nothing usable (nil, false, "", a name
/// that is no file, a number) ends the run with luatex's message; `&NAME`
/// asks twice.
#[test]
fn find_format_file_without_a_file_ends_the_run() {
    for (result, wrong_type) in [("nil", false), ("false", false), ("''", false), ("'nonexistent'", false), ("5", true)] {
        let d = Dir::new("nofile");
        d.write("init.lua", &format!("FOUND = function(n) return {result} end\n{INIT}"));
        d.write("u.tex", "\\end\n");
        let out = d.lualatex(&["-lua=init.lua", "-fmt=pl", "u.tex"]);
        let mut expected = vec!["TRACE start_run".to_string(), "TRACE find_format_file pl.fmt".to_string()];
        if wrong_type {
            expected.push("callback should return a string, false or nil, not: number".into());
        }
        expected.push("I can't find the format file `pl.fmt'!".into());
        assert_eq!(trace_lines(&out), expected, "result {result}");
        assert_eq!(out.status.code(), Some(1), "result {result}");

        let out = d.lualatex(&["-lua=init.lua", "&pl", "u.tex"]);
        let mut expected = vec!["TRACE start_run".to_string()];
        for last in [false, true] {
            expected.push("TRACE find_format_file pl.fmt".into());
            if wrong_type {
                expected.push("callback should return a string, false or nil, not: number".into());
            }
            expected.push(if last {
                "I can't find the format file `pl.fmt'!".into()
            } else {
                "Sorry, I can't find the format `pl.fmt'; will try `pl.fmt'.".to_string()
            });
        }
        assert_eq!(trace_lines(&out), expected, "result {result} with &pl");
    }
}
