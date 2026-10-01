// Tests for I/O library functions
use crate::*;
use std::env;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

// Helper to get the test data directory path
fn get_test_data_dir() -> String {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    format!("{}/src/test/test_data", manifest_dir).replace("\\", "/")
}

#[test]
fn test_io_open_read() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local f = io.open("{}/sample.txt", "r")
        assert(f ~= nil, "Failed to open file")
        local content = f:read("*a")
        f:close()
        assert(content:find("Hello, World!") ~= nil)
        "#,
        test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_open_nonexistent() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local f, err = io.open("nonexistent_file_12345.txt", "r")
        assert(f == nil)
        assert(err ~= nil)
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_lines_file() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local lines = {{}}
        for line in io.lines("{}/lines.txt") do
            table.insert(lines, line)
        end
        assert(#lines == 5)
        assert(lines[1] == "line1")
        assert(lines[5] == "line5")
        "#,
        test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_read_line() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local f = io.open("{}/sample.txt", "r")
        local line1 = f:read("*l")
        local line2 = f:read("*l")
        f:close()
        assert(line1 == "Hello, World!")
        assert(line2 == "This is line 2.")
        "#,
        test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_read_number() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local f = io.open("{}/binary.dat", "r")
        local n = f:read("*n")
        f:close()
        -- Should read the number at the start
        assert(type(n) == "number" or n == nil)
        "#,
        test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_read_bytes() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local f = io.open("{}/binary.dat", "r")
        local bytes = f:read(4)
        f:close()
        assert(bytes == "0123")
        "#,
        test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_write_temp() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local path = "{}/temp_write.txt"
        local f = io.open(path, "w")
        assert(f ~= nil)
        f:write("Test write")
        f:close()
        
        local f2 = io.open(path, "r")
        local content = f2:read("*a")
        f2:close()
        assert(content == "Test write")
        
        os.remove(path)
        "#,
        test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_seek_operations() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local f = io.open("{}/binary.dat", "r")
        
        -- seek to position 5
        local pos = f:seek("set", 5)
        assert(pos == 5)
        
        -- read from position 5
        local char = f:read(1)
        assert(char == "5")
        
        -- seek relative
        f:seek("cur", 2)
        char = f:read(1)
        assert(char == "8")
        
        -- seek to end
        local size = f:seek("end", 0)
        assert(size == 16)
        
        f:close()
        "#,
        test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_type_function() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local f = io.open("{}/sample.txt", "r")
        assert(io.type(f) == "file")
        f:close()
        assert(io.type(f) == "closed file")
        assert(io.type("not a file") == nil)
        assert(io.type(123) == nil)
        "#,
        test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_flush() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local path = "{}/temp_flush.txt"
        local f = io.open(path, "w")
        f:write("flush test")
        f:flush()
        f:close()
        os.remove(path)
        "#,
        test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_tmpfile() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local f = io.tmpfile()
        if f then
            f:write("temp content")
            f:seek("set", 0)
            local content = f:read("*a")
            assert(content == "temp content")
            f:close()
        end
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_read_all() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local f = io.open("{}/sample.txt", "r")
        local all = f:read("*a")
        f:close()
        assert(#all > 0)
        assert(all:find("End of file") ~= nil)
        "#,
        test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_file_setvbuf() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local path = "{}/temp_buf.txt"
        local f = io.open(path, "w")
        -- set buffering mode
        f:setvbuf("full", 1024)
        f:write("buffered")
        f:close()
        os.remove(path)
        "#,
        test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_multiple_reads() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local f = io.open("{}/binary.dat", "r")
        local a = f:read(2)
        local b = f:read(2)
        local c = f:read(2)
        f:close()
        assert(a == "01")
        assert(b == "23")
        assert(c == "45")
        "#,
        test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_append_mode() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local path = "{}/temp_append.txt"
        
        -- Write initial content
        local f = io.open(path, "w")
        f:write("first")
        f:close()
        
        -- Append
        f = io.open(path, "a")
        f:write("second")
        f:close()
        
        -- Verify
        f = io.open(path, "r")
        local content = f:read("*a")
        f:close()
        assert(content == "firstsecond")
        
        os.remove(path)
        "#,
        test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_read_eof() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local f = io.open("{}/binary.dat", "r")
        -- Read all content
        local all = f:read("*a")
        -- Try to read more - should return empty string or nil
        local more = f:read("*a")
        f:close()
        assert(all == "0123456789ABCDEF")
        assert(more == "" or more == nil)
        "#,
        test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_read_stops_after_first_failure() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        io.input("{}/lines.txt")
        local t = table.pack(io.read("*l", "*l", "*l", "*l", "*l", "*l"))
        assert(t.n == 6)
        assert(t[1] == "line1")
        assert(t[5] == "line5")
        assert(t[6] == nil)

        io.input("{}/sample.txt")
        local f = io.open("{}/one_line.txt", "w")
        f:write("only-line\n")
        f:close()
        io.input("{}/one_line.txt")
        local t2 = table.pack(io.read("*l", "*l", "*l"))
        assert(t2.n == 2)
        assert(t2[1] == "only-line")
        assert(t2[2] == nil)
        os.remove("{}/one_line.txt")
        "#,
        test_dir, test_dir, test_dir, test_dir, test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_file_read_stops_after_first_failure() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let test_dir = get_test_data_dir();

    let result = vm.main_state().execute(&format!(
        r#"
        local path = "{}/one_line_file_read.txt"
        local wf = io.open(path, "w")
        wf:write("only-line\n")
        wf:close()

        local f = io.open(path, "r")
        local t = table.pack(f:read("*l", "*l", "*l"))
        f:close()
        os.remove(path)

        assert(t.n == 2)
        assert(t[1] == "only-line")
        assert(t[2] == nil)
        "#,
        test_dir
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[test]
fn test_io_lines_uses_initialized_default_input() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local iter = io.lines(nil)
        assert(iter ~= nil)
        assert(io.type(io.input()) == "file")
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_io_popen_read_mode() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local f = assert(io.popen("echo hello", "r"))
        local line = f:read("*l")
        local ok, how, code = f:close()
        assert(line == "hello")
        assert(ok == true)
        assert(how == "exit")
        assert(code == 0)
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_io_popen_invalid_mode_raises() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let result = vm.main_state().execute(
        r#"
        local ok, err = pcall(io.popen, "cat", "r+")
        assert(ok == false)
        assert(type(err) == "string")
        assert(err:find("invalid mode", 1, true) ~= nil)
        "#,
    );

    assert!(result.is_ok(), "Error: {:?}", result);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_io_popen_write_mode() {
    let mut vm = GlobalState::new(SafeOption::default());
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();

    let path = PathBuf::from("popen_write_output.txt");
    let path_str = path.to_string_lossy().to_string();

    let command = if cfg!(target_os = "windows") {
        format!("sort > {}", path_str)
    } else {
        format!("cat > {}", path_str)
    };

    let result = vm.main_state().execute(&format!(
        r#"
        local f = assert(io.popen("{command}", "w"))
        f:write("pipe-data\n")
        local ok, how, code = f:close()
        assert(ok == true)
        assert(how == "exit")
        assert(code == 0)

        local rf = assert(io.open("{path}", "r"))
        local content = rf:read("*a")
        rf:close()
        assert(content:find("pipe%-data", 1) == 1)
        os.remove("{path}")
        "#,
        command = command.replace('\\', "\\\\").replace('"', "\\\""),
        path = path_str.replace('\\', "\\\\").replace('"', "\\\"")
    ));

    assert!(result.is_ok(), "Error: {:?}", result);
}

/// Run `code` in a fresh state of the given language level with `PATH`
/// bound to a scratch file path (a Lua string literal).
#[cfg(not(target_arch = "wasm32"))]
fn run_io_script(level: LuaLanguageLevel, name: &str, code: &str) {
    let path = std::env::temp_dir().join(format!("tex_lua_io_{}_{name}", std::process::id()));
    let mut vm = GlobalState::new_with_language(SafeOption::default(), level);
    vm.open_stdlib(crate::stdlib::Stdlib::All).unwrap();
    let script = format!("local PATH = {:?}\n{code}", path.to_string_lossy());
    let result = vm.main_state().execute(&script);
    let _ = std::fs::remove_file(&path);
    assert!(result.is_ok(), "{level:?}: {:?}", result);
}

#[cfg(not(target_arch = "wasm32"))]
fn run_io_script_all(name: &str, code: &str) {
    run_io_script(LuaLanguageLevel::Lua53, name, code);
    run_io_script(LuaLanguageLevel::Lua55, name, code);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_io_lines_keep_carriage_returns() {
    run_io_script_all(
        "crlf",
        r##"
        local f = assert(io.open(PATH, "wb")) f:write("a\r\nb\r\n\r\nc") f:close()
        local got = {}
        for line in io.lines(PATH) do got[#got + 1] = line end
        assert(#got == 4 and got[1] == "a\r" and got[2] == "b\r" and got[3] == "\r" and got[4] == "c")
        f = io.open(PATH, "rb")
        assert(f:read("l") == "a\r" and f:read("L") == "b\r\n")
        f:close()
        "##,
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_io_read_write_share_one_position() {
    run_io_script_all(
        "rw",
        r##"
        local f = assert(io.open(PATH, "w+"))
        f:write("abcdef")
        assert(f:seek("set", 2) == 2)
        assert(f:read(2) == "cd")
        f:write("XY")
        assert(f:seek() == 6)
        f:seek("set")
        assert(f:read("a") == "abcdXY")
        f:close()
        f = assert(io.open(PATH, "a+"))
        assert(f:read("a") == "abcdXY")
        f:write("Z")
        f:seek("set")
        assert(f:read("a") == "abcdXYZ")
        f:close()
        "##,
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_io_closed_file_errors() {
    run_io_script_all(
        "closed",
        r##"
        local f = assert(io.open(PATH, "w")) f:close()
        for _, op in ipairs{"read", "write", "lines", "seek", "flush", "close", "setvbuf"} do
          local ok, err = pcall(f[op], f, "no")
          assert(not ok and err:find("attempt to use a closed file", 1, true), op)
        end
        assert(io.type(f) == "closed file" and tostring(f) == "file (closed)")
        local g = assert(io.open(PATH))
        local it = g:lines() g:close()
        local ok, err = pcall(it)
        assert(not ok and err:find("file is already closed", 1, true))
        "##,
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_io_read_argument_errors() {
    run_io_script_all(
        "readargs",
        r##"
        local f = assert(io.open(PATH, "w")) f:write("abc\ndef") f:close()
        f = assert(io.open(PATH))
        for _, fmt in ipairs{"", "x", "*x", 1.5} do
          assert(not pcall(f.read, f, fmt), tostring(fmt))
        end
        local ok, err = pcall(f.read, f, -1)
        assert(not ok and type(err) == "string")
        assert(f:read(0) == "" and f:read("a") == "abc\ndef")
        assert(f:read(0) == nil and f:read("a") == "" and f:read("l") == nil and f:read(1) == nil)
        f:close()
        "##,
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_io_read_number_then_line() {
    run_io_script_all(
        "numbers",
        r##"
        local f = assert(io.open(PATH, "w"))
        f:write("12 0x1F -3.5e2 .5 0x 1e5x\nrest\n", ("1"):rep(201))
        f:close()
        f = assert(io.open(PATH))
        local a, b, c, d = f:read("n", "n", "n", "n")
        assert(a == 12 and math.type(a) == "integer" and b == 31 and c == -350.0 and d == 0.5)
        assert(f:read("n") == nil)
        assert(f:read("n") == 1e5 and f:read("l") == "x")
        assert(f:read("l") == "rest")
        assert(f:read("n") == nil)
        f:close()
        "##,
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_io_write_errors_and_results() {
    run_io_script_all(
        "write",
        r##"
        local f = assert(io.open(PATH, "w"))
        assert(f:write("a", 1, "b") == f)
        assert(not pcall(f.write, f, {}))
        f:close()
        f = assert(io.open(PATH, "r"))
        local r, msg, code = f:write("x")
        assert(r == nil and msg == "Bad file descriptor" and code == 9)
        local data, msg2 = f:read("a")
        assert(data == "a1b")
        f:close()
        f = assert(io.open(PATH, "w"))
        assert(f:read("a") == nil)
        f:close()
        assert(io.flush() == true)
        local nf, err, errno = io.open("/nonexistent-dir-tex-lua/x")
        assert(nf == nil and err == "/nonexistent-dir-tex-lua/x: No such file or directory" and errno == 2)
        assert(not pcall(io.open, PATH, "rw"))
        assert(not pcall(io.open, PATH, "r+x"))
        assert(io.open(PATH, "r+b")):close()
        "##,
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_io_seek_and_setvbuf_options() {
    run_io_script_all(
        "seek",
        r##"
        local f = assert(io.open(PATH, "w")) f:write("0123456789") f:close()
        f = assert(io.open(PATH))
        assert(f:read(3) == "012" and f:seek("cur", -1) == 2 and f:read(1) == "2")
        local r, msg, code = f:seek("set", -5)
        assert(r == nil and msg == "Invalid argument" and code == 22)
        assert(not pcall(f.seek, f, "bad"))
        assert(not pcall(f.seek, f, "set", 1.5))
        assert(f:setvbuf("no") == true and f:setvbuf("line") == true)
        assert(not pcall(f.setvbuf, f, "bad"))
        f:close()
        "##,
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_io_default_files() {
    run_io_script_all(
        "defaults",
        r##"
        assert(getmetatable(io.stdout) == getmetatable(io.stderr))
        local ok, msg = io.close()
        assert(ok == nil and msg == "cannot close standard file")
        local old = io.output()
        io.output(PATH)
        io.write("q")
        assert(io.close() == true)
        assert(not pcall(io.write, "x"))
        io.output(old)
        io.input(PATH)
        assert(io.read("a") == "q")
        io.input():close()
        local ok2, err = pcall(io.read)
        assert(not ok2 and err:find("input file is closed", 1, true))
        assert(not pcall(io.output, "/nonexistent-dir-tex-lua/sub/x"))
        "##,
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_io_lines_results() {
    run_io_script(
        LuaLanguageLevel::Lua53,
        "lines53",
        r##"
        local f = assert(io.open(PATH, "w")) f:write("ab\ncd") f:close()
        assert(select("#", io.lines(PATH)) == 1)
        local it = io.lines(PATH, 1, "l")
        local a, b = it()
        assert(a == "a" and b == "b")
        assert(select("#", it()) == 2)
        assert(select("#", it()) == 0)
        assert(not pcall(it))
        "##,
    );
    run_io_script(
        LuaLanguageLevel::Lua55,
        "lines55",
        r##"
        local f = assert(io.open(PATH, "w")) f:write("x") f:close()
        local it, s, c, closer = io.lines(PATH)
        assert(s == nil and c == nil and io.type(closer) == "file")
        assert(it() == "x" and select("#", it()) == 0 and io.type(closer) == "closed file")
        "##,
    );
}
