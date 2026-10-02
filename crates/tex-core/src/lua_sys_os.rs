//! LuaTeX's additions to `os` and `io` (`loslibext.c`, `luatex-core.lua`):
//! `exec`, `spawn`, `execute`, `kpsepopen`, `setenv`, `env`, `selfdir`,
//! `tmpdir`, `uname`, `times`, `gettimeofday`, `sleep`, `socketsleep`,
//! `socketgettime`, the restricted-shell policy and the file-name policy
//! (`openin_any`/`openout_any`) applied to `io.open`, `io.lines`, `os.rename`,
//! `os.remove` and the `lfs` functions.

use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tex_lua::{Lua, LuaApi, LuaBytes, LuaString};

use crate::lua_sys::{bytes_of, os_str, sys_reg};
#[cfg(unix)]
use crate::lua_sys::strerror_no;
use crate::lua_sys_kpse::check_command;

pub(crate) const PRELUDE: &str = include_str!("lua_sys_os.lua");

const INVALID_RET_E2BIG: i64 = 143;
const INVALID_RET_ENOENT: i64 = 144;
const INVALID_RET_ENOEXEC: i64 = 145;
const INVALID_RET_ENOMEM: i64 = 146;
const INVALID_RET_ETXTBSY: i64 = 147;
const INVALID_RET_UNKNOWN: i64 = 148;
const INVALID_RET_INTR: i64 = 149;

/// Message of the failure that `allow` (0 disabled, -1 quoting) stands for.
fn refusal(allow: i32) -> &'static str {
    if allow == 0 {
        "Command execution disabled via shell_escape='p'"
    } else {
        "Quoting error in system command line."
    }
}

/// `do_split_command`: split a command line at spaces honouring quotes and
/// backslash escapes.
fn split_command(cmd: &[u8]) -> Option<Vec<Vec<u8>>> {
    if cmd.is_empty() {
        return None;
    }
    let mut args = Vec::new();
    let mut piece = Vec::new();
    let mut in_string = 0u8;
    let mut quoted = false;
    let mut i = 0;
    while i < cmd.len() && cmd[i] == b' ' {
        i += 1;
    }
    while i <= cmd.len() {
        let c = cmd.get(i).copied().unwrap_or(0);
        let next = cmd.get(i + 1).copied().unwrap_or(0);
        if c == b'\\' && matches!(next, b'\\' | b'\'' | b'"') {
            quoted = true;
            i += 1;
            continue;
        }
        if in_string != 0 && c == in_string && !quoted {
            in_string = 0;
            i += 1;
            continue;
        }
        if (c == b'"' || c == b'\'') && !quoted {
            in_string = c;
            i += 1;
            continue;
        }
        if (in_string == 0 && c == b' ') || c == 0 {
            args.push(std::mem::take(&mut piece));
            while i < cmd.len() && cmd.get(i + 1) == Some(&b' ') {
                i += 1;
            }
            i += 1;
            continue;
        }
        piece.push(c);
        quoted = false;
        i += 1;
    }
    Some(args)
}

fn spawn_error_code(error: &std::io::Error) -> i64 {
    match crate::lua_sys::errno_of(error) {
        7 => INVALID_RET_E2BIG,
        2 => INVALID_RET_ENOENT,
        8 => INVALID_RET_ENOEXEC,
        12 => INVALID_RET_ENOMEM,
        26 => INVALID_RET_ETXTBSY,
        _ => INVALID_RET_UNKNOWN,
    }
}

fn command_for(program: &[u8], args: &[Vec<u8>]) -> Command {
    let mut command = Command::new(os_str(program));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        if let Some(first) = args.first() {
            command.arg0(os_str(first));
        }
    }
    for arg in args.iter().skip(1) {
        command.arg(os_str(arg));
    }
    command
}

pub(crate) fn register(lua: &mut Lua, s: &tex_lua::LuaTable) -> Result<(), String> {
    // luainit.c puts a C locale into the environment and texmfmp.c names the engine.
    for (key, value) in [("LC_CTYPE", "C"), ("LC_COLLATE", "C"), ("LC_NUMERIC", "C"), ("engine", "luatex")] {
        std::env::set_var(key, value);
    }
    sys_reg!(lua, s, "os_environ", || -> Vec<LuaBytes> {
        std::env::vars_os()
            .flat_map(|(k, v)| [LuaBytes(crate::lua_sys::os_bytes(&k)), LuaBytes(crate::lua_sys::os_bytes(&v))])
            .collect()
    });
    sys_reg!(lua, s, "os_setenv", |key: LuaString, value: Option<LuaString>| -> Result<bool, String> {
        let key = os_str(&bytes_of(&key));
        if key.is_empty() || key.to_string_lossy().contains('=') {
            return Err("unable to change environment".to_string());
        }
        match value {
            Some(value) => std::env::set_var(key, os_str(&bytes_of(&value))),
            None => std::env::remove_var(key),
        }
        Ok(true)
    });
    sys_reg!(lua, s, "os_platform", || -> (&'static str, &'static str) {
        if cfg!(windows) {
            ("windows", "windows")
        } else {
            (
                "unix",
                match std::env::consts::OS {
                    "linux" => "linux",
                    "macos" => "macosx",
                    "freebsd" => "freebsd",
                    "openbsd" => "openbsd",
                    "solaris" => "solaris",
                    _ => "generic",
                },
            )
        }
    });
    sys_reg!(lua, s, "os_selfdir", || -> LuaBytes {
        let exe = std::env::current_exe().ok().and_then(|p| std::fs::canonicalize(p).ok()).unwrap_or_default();
        LuaBytes(crate::lua_sys::kpse_path(exe.parent().unwrap_or(std::path::Path::new(""))).into_bytes())
    });
    sys_reg!(lua, s, "os_gettimeofday", || -> f64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
    });
    sys_reg!(lua, s, "os_sleep", |interval: f64, units: Option<f64>| {
        let seconds = interval / units.unwrap_or(1.0);
        if seconds > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(seconds));
        }
    });
    sys_reg!(lua, s, "os_socketsleep", |seconds: f64| {
        let seconds = seconds.clamp(0.0, f64::from(i32::MAX));
        std::thread::sleep(Duration::from_secs_f64(seconds));
    });
    sys_reg!(lua, s, "os_uname", || -> Vec<LuaBytes> {
        #[cfg(unix)]
        {
            let mut uts: libc::utsname = unsafe { std::mem::zeroed() };
            if unsafe { libc::uname(&mut uts) } < 0 {
                return Vec::new();
            }
            let field = |f: &[libc::c_char]| {
                let bytes: Vec<u8> = f.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
                LuaBytes(bytes)
            };
            vec![field(&uts.sysname), field(&uts.machine), field(&uts.release), field(&uts.version), field(&uts.nodename)]
        }
        #[cfg(windows)]
        {
            win32::uname().into_iter().map(LuaBytes).collect()
        }
        #[cfg(not(any(unix, windows)))]
        {
            Vec::new()
        }
    });
    sys_reg!(lua, s, "os_times", || -> Vec<f64> {
        #[cfg(unix)]
        {
            let mut t: libc::tms = unsafe { std::mem::zeroed() };
            unsafe { libc::times(&mut t) };
            let tick = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
            vec![t.tms_utime as f64 / tick, t.tms_stime as f64 / tick, t.tms_cutime as f64 / tick, t.tms_cstime as f64 / tick]
        }
        #[cfg(windows)]
        {
            // Windows has no `times(2)`: user and kernel time of this process
            // in seconds, none for children.
            win32::process_times().to_vec()
        }
        #[cfg(not(any(unix, windows)))]
        {
            Vec::new()
        }
    });
    sys_reg!(lua, s, "os_tmpdir", |template: LuaString| -> (Option<LuaBytes>, Option<LuaBytes>) {
        #[cfg(unix)]
        {
            let mut bytes = bytes_of(&template);
            bytes.push(0);
            let ptr = unsafe { libc::mkdtemp(bytes.as_mut_ptr().cast()) };
            if ptr.is_null() {
                let message = strerror_no(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
                return (None, Some(LuaBytes(message.into_bytes())));
            }
            bytes.pop();
            (Some(LuaBytes(bytes)), None)
        }
        #[cfg(not(unix))]
        {
            // loslibext.c's do_mkdtemp, which builds without mkdtemp(3)
            const REPLACEMENTS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
            let mut name = bytes_of(&template);
            let tail = name.len() - 6;
            let clock = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
            let mut value = clock ^ std::process::id();
            for _ in 0..36 * 36 * 36 {
                let mut v = value;
                for slot in &mut name[tail..] {
                    *slot = REPLACEMENTS[(v % 36) as usize];
                    v /= 36;
                }
                match std::fs::create_dir(os_str(&name)) {
                    Ok(()) => return (Some(LuaBytes(name)), None),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return (None, Some(LuaBytes(crate::lua_sys::strerror(&e).into_bytes()))),
                }
                value = value.wrapping_add(8413);
            }
            (None, Some(LuaBytes(crate::lua_sys::strerror_no(17).into_bytes())))
        }
    });
    // `os.execute(cmd)`: (status | nil, message); without a command, the
    // `status.shell_escape` state.
    sys_reg!(lua, s, "os_execute", |cmd: Option<LuaString>| -> (Option<i64>, Option<String>) {
        let Some(cmd) = cmd else {
            return (
                Some(match crate::lua_sys::shell_escape() {
                    crate::lua_sys::ShellEscape::Disabled => 0,
                    crate::lua_sys::ShellEscape::Enabled => 1,
                    crate::lua_sys::ShellEscape::Restricted => 2,
                }),
                None,
            );
        };
        if crate::lua_sys::shell_escape() == crate::lua_sys::ShellEscape::Disabled {
            return (None, Some("All command execution disabled.".to_string()));
        }
        let text = String::from_utf8_lossy(&bytes_of(&cmd)).into_owned();
        let (allow, run) = check_command(&text);
        if allow <= 0 {
            return (None, Some(refusal(allow).to_string()));
        }
        let status = Command::new("/bin/sh").arg("-c").arg(os_str(run.as_bytes())).status();
        match status {
            Ok(status) => {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    (Some(i64::from(status.into_raw())), None)
                }
                #[cfg(not(unix))]
                {
                    (Some(i64::from(status.code().unwrap_or(-1))), None)
                }
            }
            Err(_) => (Some(-1), None),
        }
    });
    // Replace the process by `argv`; only returns on failure.
    sys_reg!(
        lua,
        s,
        "os_exec",
        |argv: tex_lua::LuaTable, runcmd: Option<LuaString>| -> (Option<LuaBytes>, Option<i64>) {
            let argv: Vec<LuaString> = argv.sequence_values().unwrap_or_default();
            if crate::lua_sys::shell_escape() == crate::lua_sys::ShellEscape::Disabled {
                return (Some(LuaBytes(b"All command execution disabled.".to_vec())), None);
            }
            let args: Vec<Vec<u8>> = argv.iter().map(bytes_of).collect();
            let run = runcmd.map(|r| bytes_of(&r)).or_else(|| args.first().cloned());
            let (Some(run), false) = (run, args.is_empty()) else {
                return (Some(LuaBytes(b"invalid command line passed".to_vec())), None);
            };
            let (allow, program) = if crate::lua_sys::shell_escape() == crate::lua_sys::ShellEscape::Enabled {
                (1, String::from_utf8_lossy(&run).into_owned())
            } else {
                check_command(&String::from_utf8_lossy(&run))
            };
            if allow <= 0 {
                return (Some(LuaBytes(refusal(allow).as_bytes().to_vec())), None);
            }
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                let error = command_for(program.as_bytes(), &args).exec();
                let message = format!("{}: {}", program, crate::lua_sys::strerror(&error));
                (Some(LuaBytes(message.into_bytes())), Some(crate::lua_sys::errno_of(&error)))
            }
            #[cfg(not(unix))]
            {
                let _ = (program, args);
                (Some(LuaBytes(b"Function not implemented".to_vec())), Some(38))
            }
        }
    );
    // `os.spawn`: (status | nil, message, code)
    sys_reg!(
        lua,
        s,
        "os_spawn",
        |argv: tex_lua::LuaTable, runcmd: Option<LuaString>, env: Option<tex_lua::LuaTable>| -> (Option<i64>, Option<LuaBytes>, Option<i64>) {
            let argv: Vec<LuaString> = argv.sequence_values().unwrap_or_default();
            let env: Option<Vec<LuaString>> = env.and_then(|t| t.sequence_values().ok());
            if crate::lua_sys::shell_escape() == crate::lua_sys::ShellEscape::Disabled {
                return (None, Some(LuaBytes(b"All command execution disabled.".to_vec())), None);
            }
            let args: Vec<Vec<u8>> = argv.iter().map(bytes_of).collect();
            let run = runcmd.map(|r| bytes_of(&r)).or_else(|| args.first().cloned());
            let (Some(run), false) = (run, args.is_empty()) else {
                return (None, Some(LuaBytes(b"invalid command line passed".to_vec())), None);
            };
            let enabled = crate::lua_sys::shell_escape() == crate::lua_sys::ShellEscape::Enabled;
            let (allow, program) = if enabled {
                (1, String::from_utf8_lossy(&run).into_owned())
            } else {
                check_command(&String::from_utf8_lossy(&run))
            };
            if allow <= 0 {
                return (None, Some(LuaBytes(refusal(allow).as_bytes().to_vec())), None);
            }
            let mut command = command_for(program.as_bytes(), &args);
            if let (true, Some(pairs)) = (enabled, env) {
                command.env_clear();
                for pair in pairs.chunks(2) {
                    if let [k, v] = pair {
                        command.env(os_str(&bytes_of(k)), os_str(&bytes_of(v)));
                    }
                }
            }
            match command.status() {
                Ok(status) => match status.code() {
                    Some(code) => (Some(i64::from(code)), None, None),
                    None => (None, Some(LuaBytes(format!("{program}: execution interrupted").into_bytes())), Some(INVALID_RET_INTR)),
                },
                Err(error) => {
                    let code = spawn_error_code(&error);
                    let text = if code == INVALID_RET_UNKNOWN { "execution failed".to_string() } else { crate::lua_sys::strerror(&error) };
                    (None, Some(LuaBytes(format!("{program}: {text}").into_bytes())), Some(code))
                }
            }
        }
    );
    Ok(())
}

/// `os.uname()` of the Windows build of LuaTeX (loslibext.c): the system
/// name from the version, `build N`, the processor architecture and the
/// computer name.
#[cfg(windows)]
mod win32 {
    #[repr(C)]
    #[allow(dead_code)]
    struct OsVersionInfoW {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        csd_version: [u16; 128],
    }

    #[repr(C)]
    #[allow(dead_code)]
    struct SystemInfo {
        processor_architecture: u16,
        reserved: u16,
        page_size: u32,
        minimum_application_address: *mut std::ffi::c_void,
        maximum_application_address: *mut std::ffi::c_void,
        active_processor_mask: usize,
        number_of_processors: u32,
        processor_type: u32,
        allocation_granularity: u32,
        processor_level: u16,
        processor_revision: u16,
    }

    #[link(name = "ntdll")]
    extern "system" {
        // GetVersionEx reports 6.2 to programs without a manifest; this does not.
        fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
    }

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn GetProcessTimes(
            process: *mut std::ffi::c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn GetSystemInfo(info: *mut SystemInfo);
        fn GetComputerNameW(buffer: *mut u16, size: *mut u32) -> i32;
    }

    const VER_PLATFORM_WIN32_NT: u32 = 2;
    const PROCESSOR_ARCHITECTURE_INTEL: u16 = 0;
    const PROCESSOR_ARCHITECTURE_ARM: u16 = 5;
    const PROCESSOR_ARCHITECTURE_IA64: u16 = 6;
    const PROCESSOR_ARCHITECTURE_AMD64: u16 = 9;
    const PROCESSOR_ARCHITECTURE_ARM64: u16 = 12;

    fn system_name(info: &OsVersionInfoW) -> &'static str {
        if info.platform != VER_PLATFORM_WIN32_NT {
            return "Windows";
        }
        match (info.major, info.minor) {
            (..=3, _) => "Windows NT 3",
            (4, _) => "Windows NT 4",
            (5, 0) => "Windows 2000",
            (5, 1) => "Windows XP",
            (5, 2) => "Windows XP 64-Bit",
            (6, 0) => "Windows Vista",
            (6, 1) => "Windows 7",
            (6, 2) => "Windows 8",
            (6, 3) => "Windows 8.1",
            (10, _) => "Windows 10",
            _ => "",
        }
    }

    /// `[sysname, machine, release, version, nodename]`
    pub(super) fn uname() -> Vec<Vec<u8>> {
        let mut os: OsVersionInfoW = unsafe { std::mem::zeroed() };
        os.size = std::mem::size_of::<OsVersionInfoW>() as u32;
        unsafe { RtlGetVersion(&mut os) };
        let mut system: SystemInfo = unsafe { std::mem::zeroed() };
        unsafe { GetSystemInfo(&mut system) };

        let csd_len = os.csd_version.iter().position(|&c| c == 0).unwrap_or(os.csd_version.len());
        let csd = String::from_utf16_lossy(&os.csd_version[..csd_len]);
        let mut version = format!("{}.{:02}", os.major, os.minor);
        if !csd.is_empty() {
            version.push(' ');
            version.push_str(&csd);
        }
        let release = format!("build {}", os.build & 0xFFFF);
        let machine = match system.processor_architecture {
            PROCESSOR_ARCHITECTURE_AMD64 => "amd64".to_string(),
            PROCESSOR_ARCHITECTURE_ARM => "arm".to_string(),
            PROCESSOR_ARCHITECTURE_ARM64 => "arm64".to_string(),
            PROCESSOR_ARCHITECTURE_IA64 => "ia64".to_string(),
            PROCESSOR_ARCHITECTURE_INTEL if os.platform == VER_PLATFORM_WIN32_NT => format!("i{}86", system.processor_level),
            _ => "unknown".to_string(),
        };
        let mut name = [0u16; 65];
        let mut length = (name.len() - 1) as u32;
        let nodename = if unsafe { GetComputerNameW(name.as_mut_ptr(), &mut length) } != 0 {
            String::from_utf16_lossy(&name[..length as usize])
        } else {
            String::new()
        };
        vec![
            system_name(&os).as_bytes().to_vec(),
            machine.into_bytes(),
            release.into_bytes(),
            version.into_bytes(),
            nodename.into_bytes(),
        ]
    }

    /// `[utime, stime, cutime, cstime]` in seconds.
    pub(super) fn process_times() -> [f64; 4] {
        let mut creation = FileTime { low: 0, high: 0 };
        let mut exit = FileTime { low: 0, high: 0 };
        let mut kernel = FileTime { low: 0, high: 0 };
        let mut user = FileTime { low: 0, high: 0 };
        let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut creation, &mut exit, &mut kernel, &mut user) };
        if ok == 0 {
            return [0.0; 4];
        }
        // FILETIME counts 100 ns ticks
        let seconds = |t: &FileTime| ((u64::from(t.high) << 32) | u64::from(t.low)) as f64 / 1e7;
        [seconds(&user), seconds(&kernel), 0.0, 0.0]
    }
}
