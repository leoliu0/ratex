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

#[derive(Clone)]
enum Backend {
    Memory(MemoryFs),
    Disk,
}

/// Filesystem and resource policy for one compilation.
///
/// Contexts are installed only on the current thread and restored by `Scope`.
/// Paths are resolved against `cwd`; reads and writes outside their respective
/// roots are rejected before touching the backing filesystem.
#[derive(Clone)]
pub struct ResourceContext {
    backend: Backend,
    cwd: PathBuf,
    input_roots: Vec<PathBuf>,
    write_roots: Vec<PathBuf>,
    output_root: PathBuf,
    aux_root: PathBuf,
    epoch: Option<u64>,
    allow_embedded: bool,
}

impl ResourceContext {
    pub fn memory(
        fs: MemoryFs,
        output_root: &Path,
        aux_root: &Path,
        allow_embedded: bool,
    ) -> io::Result<Self> {
        let output_root = fs.resolve(output_root)?;
        let aux_root = fs.resolve(aux_root)?;
        Ok(Self {
            cwd: fs.cwd.clone(),
            backend: Backend::Memory(fs),
            input_roots: vec![PathBuf::from("/project")],
            write_roots: dedup_roots(vec![output_root.clone(), aux_root.clone()]),
            output_root,
            aux_root,
            epoch: None,
            allow_embedded,
        })
    }

    pub fn disk(
        cwd: &Path,
        allowed_input_roots: &[PathBuf],
        output_root: &Path,
        aux_root: &Path,
        allow_embedded: bool,
        epoch: Option<u64>,
    ) -> io::Result<Self> {
        let cwd = std::fs::canonicalize(cwd)?;
        let output_root = normalize_native(output_root, &cwd)?;
        let aux_root = normalize_native(aux_root, &cwd)?;
        std::fs::create_dir_all(&output_root)?;
        std::fs::create_dir_all(&aux_root)?;
        let output_root = std::fs::canonicalize(output_root)?;
        let aux_root = std::fs::canonicalize(aux_root)?;
        let mut input_roots = Vec::with_capacity(allowed_input_roots.len() + 3);
        input_roots.push(cwd.clone());
        input_roots.push(output_root.clone());
        input_roots.push(aux_root.clone());
        for root in allowed_input_roots {
            input_roots.push(std::fs::canonicalize(normalize_native(root, &cwd)?)?);
        }
        Ok(Self {
            backend: Backend::Disk,
            cwd,
            input_roots: dedup_roots(input_roots),
            write_roots: dedup_roots(vec![output_root.clone(), aux_root.clone()]),
            output_root,
            aux_root,
            epoch,
            allow_embedded,
        })
    }

    pub fn enter(&self) -> Scope {
        Scope(ACTIVE.with(|slot| slot.replace(Some(self.clone()))))
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn output_root(&self) -> &Path {
        &self.output_root
    }

    pub fn aux_root(&self) -> &Path {
        &self.aux_root
    }

    pub fn allows_embedded(&self) -> bool {
        self.allow_embedded
    }

    fn resolve_read(&self, path: &Path) -> io::Result<PathBuf> {
        match &self.backend {
            Backend::Memory(fs) => {
                let path = fs.resolve(path)?;
                ensure_beneath(&path, &self.input_roots)?;
                Ok(path)
            }
            Backend::Disk => {
                let path = std::fs::canonicalize(normalize_native(path, &self.cwd)?)?;
                ensure_beneath(&path, &self.input_roots)?;
                Ok(path)
            }
        }
    }

    fn resolve_write(&self, path: &Path) -> io::Result<PathBuf> {
        match &self.backend {
            Backend::Memory(fs) => {
                let path = fs.resolve(path)?;
                ensure_beneath(&path, &self.write_roots)?;
                Ok(path)
            }
            Backend::Disk => {
                let path = normalize_native(path, &self.cwd)?;
                let existing = existing_ancestor(&path)?;
                let suffix = path.strip_prefix(existing).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidInput, "invalid output path")
                })?;
                let path = std::fs::canonicalize(existing)?.join(suffix);
                ensure_beneath(&path, &self.write_roots)?;
                Ok(path)
            }
        }
    }
}

fn dedup_roots(mut roots: Vec<PathBuf>) -> Vec<PathBuf> {
    roots.sort();
    roots.dedup();
    roots
}

fn ensure_beneath(path: &Path, roots: &[PathBuf]) -> io::Result<()> {
    if roots.iter().any(|root| path.starts_with(root)) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "path is outside the resource context",
        ))
    }
}

fn normalize_native(path: &Path, cwd: &Path) -> io::Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    };
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => result.push(prefix.as_os_str()),
            Component::RootDir => result.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "path escapes the filesystem root",
                    ));
                }
            }
            Component::Normal(part) => result.push(part),
        }
    }
    Ok(result)
}

fn existing_ancestor(path: &Path) -> io::Result<&Path> {
    path.ancestors()
        .find(|candidate| candidate.exists())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no existing path ancestor"))
}

thread_local! {
    static ACTIVE: RefCell<Option<ResourceContext>> = const { RefCell::new(None) };
}

pub struct Scope(Option<ResourceContext>);
impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}

fn active() -> Option<ResourceContext> {
    ACTIVE.with(|slot| slot.borrow().clone())
}

pub fn is_memory() -> bool {
    active().is_some_and(|context| matches!(context.backend, Backend::Memory(_)))
}

pub fn embedded_allowed() -> bool {
    active().is_none_or(|context| context.allow_embedded)
}

pub fn epoch() -> Option<u64> {
    active().and_then(|context| match &context.backend {
        Backend::Memory(fs) => Some(fs.epoch),
        Backend::Disk => context.epoch,
    })
}

pub fn current_dir() -> io::Result<PathBuf> {
    match active() {
        Some(context) => Ok(context.cwd),
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
        ResourceContext::memory(
            self.clone(),
            Path::new("/project"),
            Path::new("/project"),
            true,
        )
        .expect("the virtual project root is valid")
        .enter()
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
        Some(context) => {
            let path = context.resolve_read(path.as_ref())?;
            match &context.backend {
                Backend::Memory(fs) => fs
                    .state
                    .borrow()
                    .files
                    .get(&path)
                    .cloned()
                    .ok_or_else(missing),
                Backend::Disk => std::fs::read(path),
            }
        }
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
        Some(context) => {
            let path = context.resolve_write(path.as_ref())?;
            match &context.backend {
                Backend::Memory(fs) => fs.make_dirs(&path),
                Backend::Disk => std::fs::create_dir_all(path),
            }
        }
        None => std::fs::create_dir_all(path),
    }
}

pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    match active() {
        Some(context) => {
            let path = context.resolve_read(path.as_ref())?;
            if let Backend::Memory(fs) = &context.backend {
                let state = fs.state.borrow();
                if !state.files.contains_key(&path) && !state.directories.contains(&path) {
                    return Err(missing());
                }
            }
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
        Some(context) => {
            let path = context.resolve_read(path.as_ref())?;
            match &context.backend {
                Backend::Memory(fs) => {
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
                Backend::Disk => std::fs::metadata(path).map(Metadata::Native),
            }
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
            } => Ok(UNIX_EPOCH + Duration::from_secs(*epoch) + Duration::from_micros(*generation)),
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
            Some(context) => {
                let path = context.resolve_read(path.as_ref())?;
                match &context.backend {
                    Backend::Memory(fs) => {
                        if !fs.state.borrow().files.contains_key(&path) {
                            return Err(missing());
                        }
                        Ok(Self::Memory {
                            fs: fs.clone(),
                            path,
                            position: 0,
                            writable: false,
                        })
                    }
                    Backend::Disk => std::fs::File::open(path).map(Self::Native),
                }
            }
            None => std::fs::File::open(path).map(Self::Native),
        }
    }
    pub fn create(path: impl AsRef<Path>) -> io::Result<Self> {
        match active() {
            Some(context) => {
                let path = context.resolve_write(path.as_ref())?;
                match &context.backend {
                    Backend::Memory(fs) => {
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
                            fs: fs.clone(),
                            path,
                            position: 0,
                            writable: true,
                        })
                    }
                    Backend::Disk => std::fs::File::create(path).map(Self::Native),
                }
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
        Some(context) => {
            let path = context.resolve_read(path.as_ref())?;
            match &context.backend {
                Backend::Memory(fs) => {
                    let state = fs.state.borrow();
                    if !state.directories.contains(&path) {
                        return Err(missing());
                    }
                    let entries: Vec<_> = state
                        .files
                        .keys()
                        .map(|path| (path, false))
                        .chain(state.directories.iter().map(|path| (path, true)))
                        .filter(|(entry, _)| entry.parent() == Some(path.as_path()))
                        .map(|(entry, directory)| {
                            Ok(DirEntry::Memory {
                                path: entry.clone(),
                                directory,
                            })
                        })
                        .collect();
                    Ok(Box::new(entries.into_iter()))
                }
                Backend::Disk => Ok(Box::new(
                    std::fs::read_dir(path)?.map(|entry| entry.map(DirEntry::Native)),
                )),
            }
        }
        None => Ok(Box::new(
            std::fs::read_dir(path)?.map(|entry| entry.map(DirEntry::Native)),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ratex-resource-context-{}-{label}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        root
    }

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
    #[test]
    fn memory_context_limits_outputs_and_embedded_resources() {
        let fs = MemoryFs::new(Path::new("/project/src"), 7).unwrap();
        fs.insert(Path::new("/project/src/main.tex"), b"input".to_vec())
            .unwrap();
        let context = ResourceContext::memory(
            fs.clone(),
            Path::new("/project/out"),
            Path::new("/project/aux"),
            false,
        )
        .unwrap();
        {
            let _scope = context.enter();
            assert_eq!(current_dir().unwrap(), Path::new("/project/src"));
            assert_eq!(epoch(), Some(7));
            assert!(!embedded_allowed());
            assert!(!crate::has_embedded_package("article.cls"));
            assert_eq!(read("main.tex").unwrap(), b"input");
            create_dir_all("/project/out").unwrap();
            write("/project/out/main.pdf", b"pdf").unwrap();
            assert!(write("/project/src/leak.aux", b"blocked").is_err());
            assert!(read("/etc/passwd").is_err());
        }
        assert!(embedded_allowed());
        assert!(crate::has_embedded_package("article.cls"));
        assert_eq!(fs.outputs()["out/main.pdf"], b"pdf");
    }

    #[test]
    fn disk_context_enforces_read_and_write_roots() {
        let root = temp_root("disk");
        let project = root.join("project");
        let extra = root.join("extra");
        let output = root.join("output");
        let aux = root.join("aux");
        let outside = root.join("outside.txt");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        std::fs::write(project.join("main.tex"), b"project").unwrap();
        std::fs::write(extra.join("font.otf"), b"font").unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        let context = ResourceContext::disk(
            &project,
            std::slice::from_ref(&extra),
            &output,
            &aux,
            false,
            Some(11),
        )
        .unwrap();
        {
            let _scope = context.enter();
            assert_eq!(
                current_dir().unwrap(),
                std::fs::canonicalize(&project).unwrap()
            );
            assert_eq!(epoch(), Some(11));
            assert_eq!(read("main.tex").unwrap(), b"project");
            assert_eq!(read(extra.join("font.otf")).unwrap(), b"font");
            assert!(read(&outside).is_err());
            assert!(File::create("source-output.txt").is_err());
            create_dir_all(output.join("nested")).unwrap();
            write(output.join("nested/result.pdf"), b"pdf").unwrap();
            write(aux.join("main.aux"), b"aux").unwrap();
        }
        assert_eq!(
            std::fs::read(output.join("nested/result.pdf")).unwrap(),
            b"pdf"
        );
        assert_eq!(std::fs::read(aux.join("main.aux")).unwrap(), b"aux");
        std::fs::remove_dir_all(root).unwrap();
    }
}
