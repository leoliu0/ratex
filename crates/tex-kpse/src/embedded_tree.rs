//! The embedded package archive as a read-only virtual TDS tree.
//!
//! The archive is looked up by basename, but every member also remembers its
//! directory below the TDS root. Paths below [`ROOT`] (`/<embedded>/fonts/
//! opentype/public/lm/lmroman10-regular.otf`, …) address that tree: it can be
//! statted, read and listed like a directory hierarchy, so programs that scan
//! font directories (luaotfload's names database) or open a file reported by
//! `kpse.find_file` need not know the files live inside the executable. The
//! root is an absolute path (no directory of that name exists on any real
//! system) so that code resolving relative names against the working
//! directory never mistakes it for one.

use super::*;
use std::sync::LazyLock;

/// Root of the virtual tree.
pub const ROOT: &str = "/<embedded>";

/// Modification time (seconds since the epoch) every embedded entry reports:
/// the archive's pinned `generated_at` stamp, so that anything derived from
/// timestamps (font name databases) is reproducible.
pub const MTIME: i64 = 1_704_067_200;

/// What a virtual path names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbeddedKind {
    File { size: u64 },
    Directory,
}

fn dir_name(id: usize) -> &'static str {
    let [offset, length] = PACKAGE_DIRS.get(id).unwrap_or_default();
    let (offset, length) = (offset as usize, length as usize);
    std::str::from_utf8(&PACKAGE_DIR_NAMES[offset..offset + length]).unwrap_or("")
}

fn dir_id(path: &str) -> Option<usize> {
    PACKAGE_DIRS.binary_search_by(|[offset, length]| {
        let (offset, length) = (offset as usize, length as usize);
        PACKAGE_DIR_NAMES[offset..offset + length].cmp(path.as_bytes())
    })
}

/// Index of the member called exactly `name`.
fn exact_entry(name: &str) -> Option<usize> {
    PACKAGE_INDEX.binary_search_by(|entry| package_name(entry).cmp(name.as_bytes()))
}

/// `path` below [`ROOT`] as a normalized `/`-separated relative path (no
/// empty, `.` or `..` components); `None` for paths outside the tree.
pub fn relative(path: &str) -> Option<String> {
    let rest = path.strip_prefix(ROOT)?;
    if !rest.is_empty() && !rest.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// Whether `path` lies in the virtual tree (whether or not it exists).
pub fn is_embedded_path(path: &str) -> bool {
    relative(path).is_some()
}

fn split(relative: &str) -> (&str, &str) {
    relative.rsplit_once('/').unwrap_or(("", relative))
}

/// The member at `relative` (a normalized relative path), as its index.
fn file_entry(relative: &str) -> Option<usize> {
    let (directory, name) = split(relative);
    let entry = exact_entry(name)?;
    let [id] = PACKAGE_MEMBER_DIRS.get(entry)?;
    (dir_name(id as usize) == directory).then_some(entry)
}

/// Exact virtual-file membership for metadata-only callers. Normal paths
/// borrow their relative spelling; unusual paths use the same normalization
/// as the virtual filesystem. The caller must check `embedded_allowed`.
pub(super) fn path_file_entry(path: &str) -> Option<usize> {
    let rest = path.strip_prefix(ROOT)?.strip_prefix('/')?;
    let normalized;
    let relative = if rest.split('/').all(|part| !matches!(part, "" | "." | "..")) {
        rest
    } else {
        normalized = relative(path)?;
        &normalized
    };
    if dir_id(relative).is_some() {
        return None;
    }
    file_entry(relative)
}

/// `stat` of a virtual path.
pub fn stat(path: &str) -> Option<EmbeddedKind> {
    if !fs::embedded_allowed() {
        return None;
    }
    let relative = relative(path)?;
    if dir_id(&relative).is_some() {
        return Some(EmbeddedKind::Directory);
    }
    let entry = file_entry(&relative)?;
    let [_, _, _, _, length, _] = PACKAGE_INDEX.get(entry)?;
    Some(EmbeddedKind::File { size: u64::from(length) })
}

/// Contents of a virtual file.
pub fn read(path: &str) -> Option<Vec<u8>> {
    if !fs::embedded_allowed() {
        return None;
    }
    let relative = relative(path)?;
    file_entry(&relative)?;
    get_embedded_package(split(&relative).1)
}

/// Members grouped by directory id (compressed rows).
static MEMBERS_BY_DIR: LazyLock<(Vec<u32>, Vec<u32>)> = LazyLock::new(|| {
    let dirs = PACKAGE_DIRS.len();
    let mut starts = vec![0u32; dirs + 1];
    for entry in 0..PACKAGE_MEMBER_DIRS.len() {
        if let Some([id]) = PACKAGE_MEMBER_DIRS.get(entry) {
            starts[id as usize + 1] += 1;
        }
    }
    for id in 0..dirs {
        starts[id + 1] += starts[id];
    }
    let mut fill = starts.clone();
    let mut members = vec![0u32; PACKAGE_MEMBER_DIRS.len()];
    for entry in 0..PACKAGE_MEMBER_DIRS.len() {
        if let Some([id]) = PACKAGE_MEMBER_DIRS.get(entry) {
            members[fill[id as usize] as usize] = entry as u32;
            fill[id as usize] += 1;
        }
    }
    (starts, members)
});

/// Entries of a virtual directory: `(name, is_directory)`, sorted by name.
pub fn read_dir(path: &str) -> Option<Vec<(String, bool)>> {
    if !fs::embedded_allowed() {
        return None;
    }
    let relative = relative(path)?;
    let id = dir_id(&relative)?;
    let mut entries: Vec<(String, bool)> = Vec::new();
    let (starts, members) = &*MEMBERS_BY_DIR;
    for &entry in &members[starts[id] as usize..starts[id + 1] as usize] {
        let name = package_name(PACKAGE_INDEX.get(entry as usize)?);
        entries.push((String::from_utf8_lossy(name).into_owned(), false));
    }
    let prefix = if relative.is_empty() { String::new() } else { format!("{relative}/") };
    // The directories below `relative` are contiguous in the sorted table.
    let (mut low, mut high) = (0, PACKAGE_DIRS.len());
    while low < high {
        let middle = low + (high - low) / 2;
        if dir_name(middle) < prefix.as_str() {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    for candidate in low..PACKAGE_DIRS.len() {
        let name = dir_name(candidate);
        let Some(rest) = name.strip_prefix(prefix.as_str()) else { break };
        if !rest.is_empty() && !rest.contains('/') {
            entries.push((rest.to_owned(), true));
        }
    }
    entries.sort();
    Some(entries)
}

/// The virtual path of the archive member that `filename` (a basename or a
/// path whose last component names a member) resolves to, spelled with the
/// member's real name.
pub fn member_path(filename: &str) -> Option<String> {
    if !fs::embedded_allowed() {
        return None;
    }
    let entry = package_entry(filename)?;
    let [id] = PACKAGE_MEMBER_DIRS.get(entry)?;
    let name = String::from_utf8_lossy(package_name(PACKAGE_INDEX.get(entry)?));
    let directory = dir_name(id as usize);
    Some(if directory.is_empty() {
        format!("{ROOT}/{name}")
    } else {
        format!("{ROOT}/{directory}/{name}")
    })
}
