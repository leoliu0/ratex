//! C ABI. See include/tex.h for ownership and pointer validity requirements.
use std::panic::{catch_unwind, AssertUnwindSafe};
use tex_runtime::{Compilation, Session, Status};

pub struct TexSession {
    session: Session,
    error: String,
}
pub struct TexResult(Compilation);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TexBytes {
    pub data: *const u8,
    pub len: usize,
}
impl TexBytes {
    fn new(bytes: &[u8]) -> Self {
        Self {
            data: if bytes.is_empty() {
                std::ptr::null()
            } else {
                bytes.as_ptr()
            },
            len: bytes.len(),
        }
    }
    fn empty() -> Self {
        Self::new(&[])
    }
}

unsafe fn bytes<'a>(data: *const u8, len: usize) -> Result<&'a [u8], String> {
    if len == 0 {
        return Ok(&[]);
    }
    if data.is_null() || len > isize::MAX as usize {
        return Err("invalid byte buffer".into());
    }
    Ok(unsafe { std::slice::from_raw_parts(data, len) })
}
unsafe fn text<'a>(data: *const u8, len: usize) -> Result<&'a str, String> {
    std::str::from_utf8(unsafe { bytes(data, len)? }).map_err(|_| "path is not UTF-8".into())
}

fn guarded<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}

unsafe fn update(
    session: *mut TexSession,
    f: impl FnOnce(&mut Session) -> Result<(), String>,
) -> u32 {
    let Some(session) = (unsafe { session.as_mut() }) else {
        return Status::InvalidInput as u32;
    };
    match catch_unwind(AssertUnwindSafe(|| f(&mut session.session))) {
        Ok(Ok(())) => {
            session.error.clear();
            Status::Success as u32
        }
        Ok(Err(error)) => {
            session.error = error;
            Status::InvalidInput as u32
        }
        Err(_) => {
            session.error = "internal Rust panic".into();
            Status::InternalError as u32
        }
    }
}

#[no_mangle]
pub extern "C" fn tex_abi_version() -> u32 {
    1
}

#[no_mangle]
pub extern "C" fn tex_session_new() -> *mut TexSession {
    guarded(std::ptr::null_mut(), || {
        Box::into_raw(Box::new(TexSession {
            session: Session::new(),
            error: String::new(),
        }))
    })
}

#[no_mangle]
pub unsafe extern "C" fn tex_session_free(session: *mut TexSession) {
    guarded((), || {
        if !session.is_null() {
            drop(unsafe { Box::from_raw(session) });
        }
    });
}

#[no_mangle]
pub unsafe extern "C" fn tex_session_add_file(
    session: *mut TexSession,
    name: *const u8,
    name_len: usize,
    data: *const u8,
    data_len: usize,
) -> u32 {
    unsafe {
        update(session, |s| {
            s.add_file(text(name, name_len)?, bytes(data, data_len)?)
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn tex_session_remove_file(
    session: *mut TexSession,
    name: *const u8,
    name_len: usize,
) -> u32 {
    unsafe { update(session, |s| s.remove_file(text(name, name_len)?)) }
}

#[no_mangle]
pub unsafe extern "C" fn tex_session_set_epoch(session: *mut TexSession, epoch: i64) -> u32 {
    unsafe {
        update(session, |s| {
            if epoch < -1 {
                return Err("epoch must be -1 (host clock) or nonnegative Unix seconds".into());
            }
            s.set_epoch((epoch >= 0).then_some(epoch as u64))
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn tex_session_last_error(session: *const TexSession) -> TexBytes {
    guarded(TexBytes::empty(), || {
        unsafe { session.as_ref() }.map_or(TexBytes::empty(), |s| TexBytes::new(s.error.as_bytes()))
    })
}

#[no_mangle]
pub unsafe extern "C" fn tex_compile(
    session: *const TexSession,
    entry: *const u8,
    entry_len: usize,
) -> *mut TexResult {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let Some(session) = (unsafe { session.as_ref() }) else {
            return Compilation::error(Status::InvalidInput, "null session");
        };
        match unsafe { text(entry, entry_len) } {
            Ok(entry) => session.session.compile(entry),
            Err(error) => Compilation::error(Status::InvalidInput, error),
        }
    }))
    .unwrap_or_else(|_| {
        Compilation::error(
            Status::InternalError,
            "internal Rust panic during compilation",
        )
    });
    Box::into_raw(Box::new(TexResult(outcome)))
}

#[no_mangle]
pub unsafe extern "C" fn tex_result_free(result: *mut TexResult) {
    guarded((), || {
        if !result.is_null() {
            drop(unsafe { Box::from_raw(result) });
        }
    });
}

macro_rules! result_number {
    ($name:ident, $ty:ty, $fallback:expr, $get:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(result: *const TexResult) -> $ty {
            guarded($fallback, || {
                unsafe { result.as_ref() }.map_or($fallback, |r| ($get)(&r.0))
            })
        }
    };
}
result_number!(
    tex_result_status,
    u32,
    Status::InvalidInput as u32,
    |r: &Compilation| r.status as u32
);
result_number!(tex_result_passes, u32, 0, |r: &Compilation| r.passes);
result_number!(tex_result_bibtex_runs, u32, 0, |r: &Compilation| r
    .bibtex_runs);
result_number!(tex_result_file_count, usize, 0, |r: &Compilation| r
    .files
    .len());

macro_rules! result_bytes {
    ($name:ident, $get:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(result: *const TexResult) -> TexBytes {
            guarded(TexBytes::empty(), || {
                unsafe { result.as_ref() }
                    .map_or(TexBytes::empty(), |r| TexBytes::new(($get)(&r.0)))
            })
        }
    };
}
fn pdf(r: &Compilation) -> &[u8] {
    &r.pdf
}
fn log(r: &Compilation) -> &[u8] {
    r.log.as_bytes()
}
fn diagnostics(r: &Compilation) -> &[u8] {
    r.diagnostics.as_bytes()
}
result_bytes!(tex_result_pdf, pdf);
result_bytes!(tex_result_log, log);
result_bytes!(tex_result_diagnostics, diagnostics);

#[no_mangle]
pub unsafe extern "C" fn tex_result_file_name(result: *const TexResult, index: usize) -> TexBytes {
    guarded(TexBytes::empty(), || {
        unsafe { result.as_ref() }
            .and_then(|r| r.0.files.keys().nth(index))
            .map_or(TexBytes::empty(), |s| TexBytes::new(s.as_bytes()))
    })
}

#[no_mangle]
pub unsafe extern "C" fn tex_result_file_data(result: *const TexResult, index: usize) -> TexBytes {
    guarded(TexBytes::empty(), || {
        unsafe { result.as_ref() }
            .and_then(|r| r.0.files.values().nth(index))
            .map_or(TexBytes::empty(), |bytes| TexBytes::new(bytes))
    })
}
