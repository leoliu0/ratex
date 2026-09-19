//! Filesystem boundary shared by TeX, font loading and BibTeX.
//!
//! Normal CLI calls delegate to std::fs. A library compilation installs a
//! thread-local memory filesystem for its duration, including unwinding. No
//! memory lookup ever falls through to the host filesystem. Compression workers
//! only receive bytes; engine and file operations stay on the calling thread.
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Default)]
struct State {
    files: BTreeMap<PathBuf, Vec<u8>>,
    directories: BTreeSet<PathBuf>,
    written: BTreeSet<PathBuf>,
    generation: u64,
}

#[derive(Clone)]
pub struct MemoryFs {
    state: Rc<RefCell<State>>,
    cwd: PathBuf,
    epoch: u64,
}

thread_local! {
    static ACTIVE: RefCell<Option<MemoryFs>> = const { RefCell::new(None) };
}

pub struct Scope(Option<MemoryFs>);
impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}

fn active() -> Option<MemoryFs> {
    ACTIVE.with(|slot| slot.borrow().clone())
}

pub fn is_memory() -> bool {
    ACTIVE.with(|slot| slot.borrow().is_some())
}

pub fn epoch() -> Option<u64> {
    ACTIVE.with(|slot| slot.borrow().as_ref().map(|fs| fs.epoch))
}

pub fn current_dir() -> io::Result<PathBuf> {
    match active() {
        Some(fs) => Ok(fs.cwd),
        None => std::env::current_dir(),
    }
}

fn missing() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "file not found")
}

impl MemoryFs {
    pub fn new(cwd: &Path, epoch: u64) -> io::Result<Self> {
        let fs = Self {
            state: Rc::default(),
            cwd: PathBuf::from("/project"),
            epoch,
        };
        let cwd = fs.resolve(cwd)?;
        fs.make_dirs(&cwd)?;
        Ok(Self { cwd, ..fs })
    }

    pub fn enter(&self) -> Scope {
        Scope(ACTIVE.with(|slot| slot.replace(Some(self.clone()))))
    }

    fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            self.cwd.join(path)
        };
        let mut result = PathBuf::new();
        for component in path.components() {
            match component {
                Component::RootDir => result.push("/"),
                Component::Normal(name) => result.push(name),
                Component::CurDir => {}
                Component::ParentDir => {
                    result.pop();
                }
                Component::Prefix(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid virtual path",
                    ))
                }
            }
        }
        if !result.starts_with("/project") {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "path escapes the project",
            ));
        }
        Ok(result)
    }

    fn make_dirs(&self, path: &Path) -> io::Result<()> {
        let path = self.resolve(path)?;
        let mut state = self.state.borrow_mut();
        for parent in path.ancestors().take_while(|p| p.starts_with("/project")) {
            if state.files.contains_key(parent) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "a file occupies the directory path",
                ));
            }
            state.directories.insert(parent.to_owned());
        }
        state.generation += 1;
        Ok(())
    }

    /// Insert an input without marking it as a compiler-generated output.
    pub fn insert(&self, path: &Path, bytes: Vec<u8>) -> io::Result<()> {
        let path = self.resolve(path)?;
        self.make_dirs(path.parent().ok_or_else(missing)?)?;
        let mut state = self.state.borrow_mut();
        if state.directories.contains(&path) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a directory occupies the file path",
            ));
        }
        state.files.insert(path, bytes);
        state.generation += 1;
        Ok(())
    }

    pub fn outputs(&self) -> BTreeMap<String, Vec<u8>> {
        let state = self.state.borrow();
        state
            .written
            .iter()
            .filter_map(|path| {
                let name = path
                    .strip_prefix("/project")
                    .ok()?
                    .to_str()?
                    .replace('\\', "/");
                Some((name, state.files.get(path)?.clone()))
            })
            .collect()
    }
}

pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    match active() {
        Some(fs) => fs
            .state
            .borrow()
            .files
            .get(&fs.resolve(path.as_ref())?)
            .cloned()
            .ok_or_else(missing),
        None => std::fs::read(path),
    }
}

pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    String::from_utf8(read(path)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    File::create(path)?.write_all(bytes.as_ref())
}

pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    match active() {
        Some(fs) => fs.make_dirs(path.as_ref()),
        None => std::fs::create_dir_all(path),
    }
}

pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    match active() {
        Some(fs) => {
            let path = fs.resolve(path.as_ref())?;
            metadata(&path)?;
            Ok(path)
        }
        None => std::fs::canonicalize(path),
    }
}

pub trait PathExt {
    fn tex_is_file(&self) -> bool;
    fn tex_is_dir(&self) -> bool;
    fn tex_exists(&self) -> bool;
    fn tex_canonicalize(&self) -> io::Result<PathBuf>;
}
impl<T: AsRef<Path> + ?Sized> PathExt for T {
    fn tex_is_file(&self) -> bool {
        metadata(self).is_ok_and(|m| m.is_file())
    }
    fn tex_is_dir(&self) -> bool {
        metadata(self).is_ok_and(|m| m.is_dir())
    }
    fn tex_exists(&self) -> bool {
        metadata(self).is_ok()
    }
    fn tex_canonicalize(&self) -> io::Result<PathBuf> {
        canonicalize(self)
    }
}

pub enum Metadata {
    Native(std::fs::Metadata),
    Memory {
        len: u64,
        directory: bool,
        epoch: u64,
        generation: u64,
    },
}

pub fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    match active() {
        Some(fs) => {
            let path = fs.resolve(path.as_ref())?;
            let state = fs.state.borrow();
            let directory = state.directories.contains(&path);
            let len = match state.files.get(&path) {
                Some(bytes) => bytes.len() as u64,
                None if directory => 0,
                None => return Err(missing()),
            };
            Ok(Metadata::Memory {
                len,
                directory,
                epoch: fs.epoch,
                generation: state.generation,
            })
        }
        None => std::fs::metadata(path).map(Metadata::Native),
    }
}

impl Metadata {
    pub fn is_file(&self) -> bool {
        match self {
            Self::Native(m) => m.is_file(),
            Self::Memory { directory, .. } => !directory,
        }
    }
    pub fn is_dir(&self) -> bool {
        match self {
            Self::Native(m) => m.is_dir(),
            Self::Memory { directory, .. } => *directory,
        }
    }
    pub fn len(&self) -> u64 {
        match self {
            Self::Native(m) => m.len(),
            Self::Memory { len, .. } => *len,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn modified(&self) -> io::Result<SystemTime> {
        match self {
            Self::Native(m) => m.modified(),
            Self::Memory {
                epoch, generation, ..
            } => Ok(UNIX_EPOCH + Duration::from_secs(*epoch) + Duration::from_nanos(*generation)),
        }
    }
    pub fn permissions(&self) -> Permissions {
        Permissions(match self {
            Self::Native(m) => m.permissions().readonly(),
            Self::Memory { .. } => false,
        })
    }
}

pub struct Permissions(bool);
impl Permissions {
    pub fn readonly(&self) -> bool {
        self.0
    }
}

// Preserve native directory identity checks without pretending that virtual
// files have host inode numbers. Their mutation generation invalidates caches.
#[cfg(unix)]
macro_rules! native_metadata {
    ($($name:ident: $ty:ty),*) => { impl Metadata { $(pub fn $name(&self) -> $ty {
        use std::os::unix::fs::MetadataExt;
        match self { Self::Native(m) => m.$name(), Self::Memory { generation, .. } => *generation as $ty }
    })* } };
}
#[cfg(unix)]
native_metadata!(dev: u64, ino: u64, mode: u32, nlink: u64, mtime: i64, mtime_nsec: i64, ctime: i64, ctime_nsec: i64);

pub enum File {
    Native(std::fs::File),
    Memory {
        fs: MemoryFs,
        path: PathBuf,
        position: u64,
        writable: bool,
    },
}

impl File {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        match active() {
            Some(fs) => {
                let path = fs.resolve(path.as_ref())?;
                if !fs.state.borrow().files.contains_key(&path) {
                    return Err(missing());
                }
                Ok(Self::Memory {
                    fs,
                    path,
                    position: 0,
                    writable: false,
                })
            }
            None => std::fs::File::open(path).map(Self::Native),
        }
    }
    pub fn create(path: impl AsRef<Path>) -> io::Result<Self> {
        match active() {
            Some(fs) => {
                let path = fs.resolve(path.as_ref())?;
                {
                    let mut state = fs.state.borrow_mut();
                    if !path.parent().is_some_and(|p| state.directories.contains(p)) {
                        return Err(missing());
                    }
                    if state.directories.contains(&path) {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "path is a directory",
                        ));
                    }
                    state.files.insert(path.clone(), Vec::new());
                    state.written.insert(path.clone());
                    state.generation += 1;
                }
                Ok(Self::Memory {
                    fs,
                    path,
                    position: 0,
                    writable: true,
                })
            }
            None => std::fs::File::create(path).map(Self::Native),
        }
    }
    pub fn sync_all(&self) -> io::Result<()> {
        match self {
            Self::Native(f) => f.sync_all(),
            Self::Memory { .. } => Ok(()),
        }
    }
}
impl Read for File {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Native(f) => f.read(buffer),
            Self::Memory {
                fs, path, position, ..
            } => {
                let state = fs.state.borrow();
                let bytes = state.files.get(path).ok_or_else(missing)?;
                let start = (*position).min(bytes.len() as u64) as usize;
                let len = buffer.len().min(bytes.len() - start);
                buffer[..len].copy_from_slice(&bytes[start..start + len]);
                *position += len as u64;
                Ok(len)
            }
        }
    }
}
impl Write for File {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            Self::Native(f) => f.write(buffer),
            Self::Memory {
                fs,
                path,
                position,
                writable,
            } => {
                if !*writable {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "read-only stream",
                    ));
                }
                let start = usize::try_from(*position).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidInput, "file offset overflow")
                })?;
                let end = start.checked_add(buffer.len()).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "file size overflow")
                })?;
                let mut state = fs.state.borrow_mut();
                let bytes = state.files.get_mut(path).ok_or_else(missing)?;
                if end > bytes.len() {
                    bytes.resize(end, 0);
                }
                bytes[start..end].copy_from_slice(buffer);
                *position = end as u64;
                state.generation += 1;
                Ok(buffer.len())
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Native(f) => f.flush(),
            Self::Memory { .. } => Ok(()),
        }
    }
}
impl Seek for File {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        match self {
            Self::Native(f) => f.seek(from),
            Self::Memory {
                fs, path, position, ..
            } => {
                let next = match from {
                    SeekFrom::Start(n) => i128::from(n),
                    SeekFrom::Current(n) => i128::from(*position) + i128::from(n),
                    SeekFrom::End(n) => {
                        fs.state.borrow().files.get(path).ok_or_else(missing)?.len() as i128
                            + i128::from(n)
                    }
                };
                *position = u64::try_from(next)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid seek"))?;
                Ok(*position)
            }
        }
    }
}

pub enum DirEntry {
    Native(std::fs::DirEntry),
    Memory { path: PathBuf, directory: bool },
}
pub enum FileType {
    Native(std::fs::FileType),
    Memory(bool),
}
impl DirEntry {
    pub fn path(&self) -> PathBuf {
        match self {
            Self::Native(e) => e.path(),
            Self::Memory { path, .. } => path.clone(),
        }
    }
    pub fn file_name(&self) -> OsString {
        match self {
            Self::Native(e) => e.file_name(),
            Self::Memory { path, .. } => path.file_name().unwrap_or_default().to_owned(),
        }
    }
    pub fn file_type(&self) -> io::Result<FileType> {
        match self {
            Self::Native(e) => e.file_type().map(FileType::Native),
            Self::Memory { directory, .. } => Ok(FileType::Memory(*directory)),
        }
    }
}
impl FileType {
    pub fn is_file(&self) -> bool {
        match self {
            Self::Native(t) => t.is_file(),
            Self::Memory(dir) => !dir,
        }
    }
    pub fn is_dir(&self) -> bool {
        match self {
            Self::Native(t) => t.is_dir(),
            Self::Memory(dir) => *dir,
        }
    }
    pub fn is_symlink(&self) -> bool {
        match self {
            Self::Native(t) => t.is_symlink(),
            Self::Memory(_) => false,
        }
    }
}
pub fn read_dir(
    path: impl AsRef<Path>,
) -> io::Result<Box<dyn Iterator<Item = io::Result<DirEntry>>>> {
    match active() {
        Some(fs) => {
            let path = fs.resolve(path.as_ref())?;
            let state = fs.state.borrow();
            if !state.directories.contains(&path) {
                return Err(missing());
            }
            let entries: Vec<_> = state
                .files
                .keys()
                .map(|p| (p, false))
                .chain(state.directories.iter().map(|p| (p, true)))
                .filter(|(p, _)| p.parent() == Some(path.as_path()))
                .map(|(p, directory)| {
                    Ok(DirEntry::Memory {
                        path: p.clone(),
                        directory,
                    })
                })
                .collect();
            Ok(Box::new(entries.into_iter()))
        }
        None => Ok(Box::new(
            std::fs::read_dir(path)?.map(|e| e.map(DirEntry::Native)),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_output_streams_and_aliases_share_bytes() {
        let fs = MemoryFs::new(Path::new("/project"), 0).unwrap();
        let _scope = fs.enter();
        let mut file = File::create("out.aux").unwrap();
        file.write_all(b"first").unwrap();
        assert_eq!(read("./out.aux").unwrap(), b"first");
        let stamp = metadata("out.aux").unwrap().modified().unwrap();
        file.write_all(b" second").unwrap();
        assert_ne!(metadata("out.aux").unwrap().modified().unwrap(), stamp);
        assert_eq!(read("/project/out.aux").unwrap(), b"first second");
        let mut input = File::open("out.aux").unwrap();
        input.seek(SeekFrom::Start(6)).unwrap();
        let mut tail = String::new();
        input.read_to_string(&mut tail).unwrap();
        assert_eq!(tail, "second");
        assert!(read("../etc/passwd").is_err());
        assert!(read("/etc/passwd").is_err());
    }

    #[test]
    fn nested_scope_restores_the_previous_filesystem_after_panic() {
        let outer = MemoryFs::new(Path::new("/project"), 1).unwrap();
        let inner = MemoryFs::new(Path::new("/project"), 2).unwrap();
        let _scope = outer.enter();
        write("file", b"outer").unwrap();
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _inner = inner.enter();
            assert!(read("file").is_err());
            write("file", b"inner").unwrap();
            panic!("simulate engine panic");
        }));
        assert!(failure.is_err());
        assert_eq!(read("file").unwrap(), b"outer");
        assert_eq!(epoch(), Some(1));
    }
}
