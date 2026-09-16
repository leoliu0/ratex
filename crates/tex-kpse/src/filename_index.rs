//! Optional installation metadata. Stale or malformed indexes fall back to ls-R.
use super::LsR;
use std::io;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"TXLSR001";
const STAMP_WORDS: usize = 7;
const HEADER: usize = 8 + STAMP_WORDS * 8 + 8;
const ENTRY: usize = 24;

fn stamp(path: &Path) -> io::Result<[u64; STAMP_WORDS]> {
    let m = std::fs::metadata(path)?;
    let t = m
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok([
            m.len(),
            t.as_secs(),
            t.subsec_nanos() as u64,
            m.dev(),
            m.ino(),
            m.ctime() as u64,
            m.ctime_nsec() as u64,
        ])
    }
    #[cfg(not(unix))]
    {
        Ok([m.len(), t.as_secs(), t.subsec_nanos() as u64, 0, 0, 0, 0])
    }
}

fn index_path(source: &Path, directory: &Path) -> Option<PathBuf> {
    let canonical = source.canonicalize().ok()?;
    // Hashing only names the index. Its header checks the source identity.
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    canonical.hash(&mut h);
    Some(directory.join(format!("{:016x}.txlsr", h.finish())))
}

pub(super) struct Packed {
    bytes: Vec<u8>,
    text: String,
    count: usize,
}

impl Packed {
    pub(super) fn source_bytes(&self) -> &[u8] {
        self.text.as_bytes()
    }

    fn u32(&self, entry: usize, offset: usize) -> usize {
        let at = HEADER + entry * ENTRY + offset;
        u32::from_le_bytes(self.bytes[at..at + 4].try_into().unwrap()) as usize
    }

    fn key(&self, entry: usize) -> u64 {
        let at = HEADER + entry * ENTRY;
        u64::from_le_bytes(self.bytes[at..at + 8].try_into().unwrap())
    }

    pub(super) fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub(super) fn get(&self, name: &str) -> Option<Vec<PathBuf>> {
        let key = LsR::fingerprint(name);
        let (mut low, mut high) = (0, self.count);
        while low < high {
            let mid = low + (high - low) / 2;
            if self.key(mid) < key {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        let mut hits = Vec::new();
        for i in low..self.count {
            if self.key(i) != key {
                break;
            }
            let stored = &self.text[self.u32(i, 8)..self.u32(i, 12)];
            let matches = if stored.is_ascii() && name.is_ascii() {
                stored.eq_ignore_ascii_case(name)
            } else {
                stored.to_lowercase() == name.to_lowercase()
            };
            if matches {
                hits.push((i, stored));
            }
        }
        if hits.is_empty() {
            return None;
        }
        let exact = hits.iter().any(|(_, n)| *n == name);
        Some(
            hits.into_iter()
                .filter_map(|(i, stored)| {
                    if exact && stored != name {
                        return None;
                    }
                    Some(Path::new(&self.text[self.u32(i, 16)..self.u32(i, 20)]).join(stored))
                })
                .collect(),
        )
    }

    fn decode(mut bytes: Vec<u8>, expected: [u64; STAMP_WORDS]) -> Option<Self> {
        if bytes.get(..8)? != MAGIC {
            return None;
        }
        for (i, expected) in expected.into_iter().enumerate() {
            let at = 8 + i * 8;
            if u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?) != expected {
                return None;
            }
        }
        let count = usize::try_from(u64::from_le_bytes(
            bytes.get(HEADER - 8..HEADER)?.try_into().ok()?,
        ))
        .ok()?;
        let end = HEADER.checked_add(count.checked_mul(ENTRY)?)?;
        if end > bytes.len() {
            return None;
        }
        let text = String::from_utf8(bytes.split_off(end)).ok()?;
        if text.len() as u64 != expected[0] {
            return None;
        }
        let packed = Self { bytes, text, count };
        let mut previous = 0;
        for i in 0..count {
            let key = packed.key(i);
            if key < previous {
                return None;
            }
            previous = key;
            for (a, b) in [(8, 12), (16, 20)] {
                packed.text.get(packed.u32(i, a)..packed.u32(i, b))?;
            }
        }
        Some(packed)
    }

    pub(super) fn load(source: &Path) -> Option<Self> {
        let directory = match std::env::var_os("TEX_INDEX_DIR") {
            Some(s) if s.is_empty() => return None,
            Some(s) => PathBuf::from(s),
            None => std::env::current_exe()
                .ok()?
                .parent()?
                .join("tex-index-data"),
        };
        let before = stamp(source).ok()?;
        let bytes = std::fs::read(index_path(source, &directory)?).ok()?;
        let packed = Self::decode(bytes, before)?;
        (stamp(source).ok()? == before).then_some(packed)
    }
}

/// Generate a reusable filename index without modifying the TeX installation.
/// Lookup always falls back to the source database when its identity changes.
pub fn build_filename_index(root: &Path, directory: &Path) -> io::Result<PathBuf> {
    let source = ["ls-R", "ls-R.lua"]
        .into_iter()
        .map(|n| root.join(n))
        .find(|p| p.exists())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no ls-R database"))?;
    let before = stamp(&source)?;
    let db = LsR::parse(std::fs::read_to_string(&source)?);
    if stamp(&source)? != before {
        return Err(io::Error::other("ls-R changed while indexing"));
    }
    if db.text.len() > u32::MAX as usize {
        return Err(io::Error::other("ls-R exceeds index range"));
    }
    let mut entries: Vec<_> = db
        .entries
        .iter()
        .map(|e| (LsR::fingerprint(&db.text[e.name.clone()]), e))
        .collect();
    // Stable sort retains source order for duplicate names and collisions.
    entries.sort_by_key(|(key, _)| *key);
    let mut bytes = Vec::with_capacity(HEADER + entries.len() * ENTRY + db.text.len());
    bytes.extend_from_slice(MAGIC);
    for v in before {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for (key, e) in entries {
        bytes.extend_from_slice(&key.to_le_bytes());
        for v in [e.name.start, e.name.end, e.directory.start, e.directory.end] {
            bytes.extend_from_slice(&(v as u32).to_le_bytes());
        }
    }
    bytes.extend_from_slice(db.text.as_bytes());
    std::fs::create_dir_all(directory)?;
    let path = index_path(&source, directory)
        .ok_or_else(|| io::Error::other("cannot resolve ls-R path"))?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_index_preserves_order_case_and_rejects_stale_or_invalid_data() {
        let dir = std::env::temp_dir().join(format!("tex-index-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("ls-R");
        let text = "% head\r\n./one:\r\nMix.sty\r\nÉcole.sty\n./two:\nMIX.sty\nMix.sty\n";
        std::fs::write(&source, text).unwrap();
        let path = build_filename_index(&dir, &dir).unwrap();
        let bytes = std::fs::read(path).unwrap();
        let expected = stamp(&source).unwrap();
        let packed = Packed::decode(bytes.clone(), expected).unwrap();
        let parsed = LsR::parse(text.into());
        for name in ["Mix.sty", "MIX.sty", "mix.sty", "école.sty", "absent"] {
            assert_eq!(packed.get(name), parsed.get(name));
        }
        for n in [0, 7, HEADER - 1, HEADER + ENTRY - 1, bytes.len() - 1] {
            assert!(Packed::decode(bytes[..n].to_vec(), expected).is_none());
        }
        let mut stale = expected;
        stale[1] += 1;
        assert!(Packed::decode(bytes.clone(), stale).is_none());
        let mut corrupt = bytes;
        corrupt[HEADER + 8..HEADER + 12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Packed::decode(corrupt, expected).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
