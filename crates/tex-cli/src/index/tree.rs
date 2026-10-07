//! Index style files from the TeX tree (kpathsea's `ist` format beyond the
//! working directory), shared by the build driver and the engine's
//! `\write18` makeindex.

use std::path::Path;

use tex_kpse::fs::PathExt;

/// The style file `name` (already with its suffix) from the TeX tree or the
/// embedded packages: the path makeindex reports, and the contents.
pub fn style(root: &Path, name: &str) -> Option<(String, Vec<u8>)> {
    let kpse = tex_kpse::Kpse::with_roots(root, &[]);
    if let Some(path) = kpse.find_any(name).filter(|path| path.tex_is_file()) {
        if let Ok(bytes) = tex_kpse::fs::read(&path) {
            return Some((path.to_string_lossy().into_owned(), bytes));
        }
    }
    tex_kpse::get_embedded_package(name).map(|bytes| (name.to_string(), bytes))
}
