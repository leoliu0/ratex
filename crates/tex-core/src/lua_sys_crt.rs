//! The `errno` values and `strerror` texts of the Microsoft C runtime.
//!
//! LuaTeX's libraries return `nil, strerror(errno), errno` from the C
//! runtime's point of view. On Windows `std::io::Error` carries the Win32
//! error code and the Win32 message instead ("The system cannot find the file
//! specified." for a missing file where the CRT says "No such file or
//! directory"), so the Windows build maps the Win32 code (or, for errors
//! without one, the `ErrorKind`) to the CRT's `errno` the way `_dosmaperr`
//! does, and shows the CRT's text for it.

use std::io::ErrorKind;

const ENOENT: i32 = 2;
const EINTR: i32 = 4;
const E2BIG: i32 = 7;
const ENOEXEC: i32 = 8;
const EBADF: i32 = 9;
const EAGAIN: i32 = 11;
const ENOMEM: i32 = 12;
const EACCES: i32 = 13;
const EBUSY: i32 = 16;
const EEXIST: i32 = 17;
const EXDEV: i32 = 18;
const ENOTDIR: i32 = 20;
const EISDIR: i32 = 21;
const EINVAL: i32 = 22;
const EMFILE: i32 = 24;
const EFBIG: i32 = 27;
const ENOSPC: i32 = 28;
const EROFS: i32 = 30;
const EMLINK: i32 = 31;
const EPIPE: i32 = 32;
const EDEADLK: i32 = 36;
const ENAMETOOLONG: i32 = 38;
const ENOSYS: i32 = 40;
const ENOTEMPTY: i32 = 41;

/// `_dosmaperr`: the `errno` the CRT sets for a failing Win32 call.
pub(crate) fn errno_from_win32(code: u32) -> i32 {
    match code {
        2 | 3 | 15 | 18 | 53 | 67 | 161 | 206 => ENOENT,
        4 => EMFILE,
        5 | 16 | 33 | 65 | 82 | 83 | 108 | 132 | 158 | 167 => EACCES,
        6 => EBADF,
        7..=9 | 1816 => ENOMEM,
        10 => E2BIG,
        11 => ENOEXEC,
        17 => EXDEV,
        80 | 183 => EEXIST,
        // ERROR_WRITE_PROTECT .. ERROR_SHARING_BUFFER_EXCEEDED
        19..=36 => EACCES,
        // ERROR_INVALID_STARTING_CODESEG .. ERROR_INFLOOP_IN_RELOC_CHAIN
        188..=202 => ENOEXEC,
        109 => EPIPE,
        112 => ENOSPC,
        145 => ENOTEMPTY,
        164 | 215 => EAGAIN,
        267 => ENOTDIR,
        _ => EINVAL,
    }
}

/// The `errno` standing for an error that did not come from the OS.
pub(crate) fn errno_from_kind(kind: ErrorKind) -> i32 {
    match kind {
        ErrorKind::NotFound => ENOENT,
        ErrorKind::PermissionDenied => EACCES,
        ErrorKind::AlreadyExists => EEXIST,
        ErrorKind::InvalidInput | ErrorKind::InvalidData => EINVAL,
        ErrorKind::WouldBlock => EAGAIN,
        ErrorKind::Interrupted => EINTR,
        ErrorKind::BrokenPipe => EPIPE,
        ErrorKind::OutOfMemory => ENOMEM,
        ErrorKind::Unsupported => ENOSYS,
        ErrorKind::NotADirectory => ENOTDIR,
        ErrorKind::IsADirectory => EISDIR,
        ErrorKind::DirectoryNotEmpty => ENOTEMPTY,
        ErrorKind::ReadOnlyFilesystem => EROFS,
        ErrorKind::StorageFull => ENOSPC,
        ErrorKind::FileTooLarge => EFBIG,
        ErrorKind::ResourceBusy => EBUSY,
        ErrorKind::CrossesDevices => EXDEV,
        ErrorKind::TooManyLinks => EMLINK,
        ErrorKind::InvalidFilename => ENAMETOOLONG,
        ErrorKind::ArgumentListTooLong => E2BIG,
        ErrorKind::Deadlock => EDEADLK,
        _ => 0,
    }
}

/// `strerror(errno)` of the Windows C runtime.
pub(crate) fn strerror(errno: i32) -> &'static str {
    match errno {
        0 => "No error",
        1 => "Operation not permitted",
        2 => "No such file or directory",
        3 => "No such process",
        4 => "Interrupted function call",
        5 => "Input/output error",
        6 => "No such device or address",
        7 => "Arg list too long",
        8 => "Exec format error",
        9 => "Bad file descriptor",
        10 => "No child processes",
        11 => "Resource temporarily unavailable",
        12 => "Not enough space",
        13 => "Permission denied",
        14 => "Bad address",
        16 => "Resource device",
        17 => "File exists",
        18 => "Improper link",
        19 => "No such device",
        20 => "Not a directory",
        21 => "Is a directory",
        22 => "Invalid argument",
        23 => "Too many open files in system",
        24 => "Too many open files",
        25 => "Inappropriate I/O control operation",
        27 => "File too large",
        28 => "No space left on device",
        29 => "Invalid seek",
        30 => "Read-only file system",
        31 => "Too many links",
        32 => "Broken pipe",
        33 => "Domain error",
        34 => "Result too large",
        36 => "Resource deadlock avoided",
        38 => "Filename too long",
        39 => "No locks available",
        40 => "Function not implemented",
        41 => "Directory not empty",
        42 => "Illegal byte sequence",
        _ => "Unknown error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win32_codes_map_like_dosmaperr() {
        // ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_ACCESS_DENIED,
        // ERROR_ALREADY_EXISTS, ERROR_DIR_NOT_EMPTY, ERROR_SHARING_VIOLATION
        let mapped: Vec<i32> = [2, 3, 5, 183, 145, 32, 123456].into_iter().map(errno_from_win32).collect();
        assert_eq!(mapped, [ENOENT, ENOENT, EACCES, EEXIST, ENOTEMPTY, EACCES, EINVAL]);
        assert_eq!(strerror(errno_from_win32(2)), "No such file or directory");
        assert_eq!(strerror(errno_from_win32(5)), "Permission denied");
        assert_eq!(strerror(EROFS), "Read-only file system");
    }

    #[test]
    fn error_kinds_without_a_win32_code_map_to_errno() {
        assert_eq!(errno_from_kind(ErrorKind::NotFound), ENOENT);
        assert_eq!(errno_from_kind(ErrorKind::AlreadyExists), EEXIST);
        assert_eq!(errno_from_kind(ErrorKind::Other), 0);
    }
}
