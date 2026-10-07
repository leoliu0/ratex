//! The environment's locale, which makeindex consults in two places: `-L`
//! (and `-T`) sort with `strcoll` under `LC_COLLATE`, and the first letter
//! of a new group is case-converted under `LC_CTYPE`.

/// A locale built from the environment (`setlocale(..., "")`).
pub(crate) struct Locale {
    #[cfg(target_os = "linux")]
    handle: *mut std::ffi::c_void,
}

#[cfg(target_os = "linux")]
mod ffi {
    use std::ffi::{c_char, c_int, c_void};

    // glibc and musl: LC_CTYPE is 0 and LC_COLLATE is 3.
    pub(super) const LC_CTYPE_MASK: c_int = 1 << 0;
    pub(super) const LC_COLLATE_MASK: c_int = 1 << 3;

    extern "C" {
        pub(super) fn newlocale(mask: c_int, locale: *const c_char, base: *mut c_void) -> *mut c_void;
        pub(super) fn freelocale(locale: *mut c_void);
        pub(super) fn strcoll_l(a: *const c_char, b: *const c_char, locale: *mut c_void) -> c_int;
        pub(super) fn isupper_l(c: c_int, locale: *mut c_void) -> c_int;
        pub(super) fn tolower_l(c: c_int, locale: *mut c_void) -> c_int;
        pub(super) fn toupper_l(c: c_int, locale: *mut c_void) -> c_int;
    }
}

impl Locale {
    /// The environment's locale; `None` when it is the "C" locale in effect
    /// (unset, invalid, or on systems where it is not consulted).
    pub(crate) fn from_env() -> Option<Locale> {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: newlocale reads the environment and returns an owned
            // locale object or null.
            let handle = unsafe { ffi::newlocale(ffi::LC_CTYPE_MASK | ffi::LC_COLLATE_MASK, c"".as_ptr(), std::ptr::null_mut()) };
            (!handle.is_null()).then_some(Locale { handle })
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }

    /// `strcoll`.
    pub(crate) fn strcoll(&self, a: &[u8], b: &[u8]) -> i32 {
        #[cfg(target_os = "linux")]
        {
            let c_string = |text: &[u8]| {
                let end = text.iter().position(|&byte| byte == 0).unwrap_or(text.len());
                std::ffi::CString::new(&text[..end]).expect("no interior NUL")
            };
            let (a, b) = (c_string(a), c_string(b));
            // SAFETY: both strings are NUL-terminated and the locale is live.
            unsafe { ffi::strcoll_l(a.as_ptr(), b.as_ptr(), self.handle) }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (a, b);
            0
        }
    }

    /// `TOLOWER`: lower case for an upper-case letter, else unchanged.
    pub(crate) fn to_lower(&self, byte: u8) -> u8 {
        #[cfg(target_os = "linux")]
        {
            let c = std::ffi::c_int::from(byte);
            // SAFETY: plain ctype queries on a live locale.
            unsafe {
                if ffi::isupper_l(c, self.handle) != 0 { ffi::tolower_l(c, self.handle) as u8 } else { byte }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            byte.to_ascii_lowercase()
        }
    }

    /// `TOUPPER`: upper case for a letter that is not upper case already.
    pub(crate) fn to_upper(&self, byte: u8) -> u8 {
        #[cfg(target_os = "linux")]
        {
            let c = std::ffi::c_int::from(byte);
            // SAFETY: plain ctype queries on a live locale.
            unsafe {
                if ffi::isupper_l(c, self.handle) != 0 { byte } else { ffi::toupper_l(c, self.handle) as u8 }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            byte.to_ascii_uppercase()
        }
    }
}

impl Drop for Locale {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        // SAFETY: the handle came from newlocale and is freed once.
        unsafe {
            ffi::freelocale(self.handle)
        }
    }
}
