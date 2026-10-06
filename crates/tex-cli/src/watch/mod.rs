//! Watch mode (`-pvc`): rebuild whenever a file the last build read changes.
//!
//! Change detection is polling, in pure Rust: every [`POLL_INTERVAL`] each
//! tracked file's metadata (size, modification time, inode) is compared with
//! what was recorded when the file was last read by a build. Only when the
//! metadata differs, or cannot prove the file unchanged, is the content
//! hashed, so a save that rewrites identical bytes (or a bare `touch`) is
//! ignored. Changes are debounced: a rebuild starts once the tracked files
//! have been quiet for [`DEBOUNCE`], so an editor's truncate-and-write,
//! backup-and-rename, or write-temp-and-rename save yields one rebuild.
//!
//! Polling needs no platform backend, behaves the same on network and
//! container filesystems, and follows a file that is deleted and recreated
//! (its path stays tracked while it is missing).

use super::strict_file_hash;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// Interval between metadata scans of the tracked files.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Quiet time after the last observed change before a rebuild starts.
const DEBOUNCE: Duration = Duration::from_millis(250);
/// Granularity at which a sleeping poll loop notices an interrupt.
const SLEEP_SLICE: Duration = Duration::from_millis(25);
/// A metadata match proves nothing while the file's modification time is this
/// close to the moment its content was recorded: a second write inside one
/// timestamp tick would leave size and time unchanged. Such entries are
/// re-hashed on every scan until they age past the window.
const RACY_WINDOW: Duration = Duration::from_secs(2);

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Whether Ctrl-C was pressed since [`install_interrupt_handler`].
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// Make Ctrl-C set the [`interrupted`] flag instead of killing the process,
/// so the watch loop can unwind, release the cache lock, and exit with 0.
/// Child processes get the default disposition back on `exec`.
#[cfg(unix)]
pub fn install_interrupt_handler() {
    extern "C" fn on_interrupt(_signal: libc::c_int) {
        INTERRUPTED.store(true, Ordering::SeqCst);
    }
    // SAFETY: `action` is fully initialized before use and the handler only
    // performs an atomic store, which is async-signal-safe.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::sigemptyset(&mut action.sa_mask);
        action.sa_flags = 0;
        libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
    }
}

#[cfg(windows)]
pub fn install_interrupt_handler() {
    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }
    unsafe extern "system" fn on_interrupt(event: u32) -> i32 {
        // CTRL_C_EVENT and CTRL_BREAK_EVENT
        if event == 0 || event == 1 {
            INTERRUPTED.store(true, Ordering::SeqCst);
            1
        } else {
            0
        }
    }
    // SAFETY: registers a handler that only performs an atomic store.
    unsafe {
        SetConsoleCtrlHandler(Some(on_interrupt), 1);
    }
}

/// Local wall-clock time as `HH:MM:SS` for status lines.
#[cfg(unix)]
pub fn clock() -> String {
    // SAFETY: `time` accepts a null pointer and `localtime_r` fills `parts`.
    let parts = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut parts: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut parts).is_null() {
            return utc_clock();
        }
        parts
    };
    format!("{:02}:{:02}:{:02}", parts.tm_hour, parts.tm_min, parts.tm_sec)
}

#[cfg(not(unix))]
pub fn clock() -> String {
    utc_clock()
}

fn utc_clock() -> String {
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
        % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

/// Sleep for `total`; `true` when interrupted before it elapsed.
fn sleep_interruptibly(total: Duration) -> bool {
    let deadline = Instant::now() + total;
    loop {
        if interrupted() {
            return true;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        std::thread::sleep(remaining.min(SLEEP_SLICE));
    }
}

/// File metadata that changes whenever an editor replaces or rewrites a file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    /// Inode where the platform has one: an atomic rename installs a new one.
    id: u64,
}

impl Stamp {
    /// `None` when `path` is missing or not a regular file.
    fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        metadata.is_file().then(|| Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            id: file_id(&metadata),
        })
    }
}

#[cfg(unix)]
fn file_id(metadata: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::ino(metadata)
}

#[cfg(not(unix))]
fn file_id(_metadata: &std::fs::Metadata) -> u64 {
    0
}

fn is_racy(stamp: Option<Stamp>, now: SystemTime) -> bool {
    let Some(stamp) = stamp else {
        return false;
    };
    // An unknown, future, or recent modification time proves nothing.
    stamp
        .modified
        .is_none_or(|modified| now.duration_since(modified).map_or(true, |age| age < RACY_WINDOW))
}

fn hash_file(path: &Path) -> Option<u64> {
    strict_file_hash(path).ok()
}

/// The recorded state of one tracked path.
struct Entry {
    /// Metadata when `hash` was taken; `None` when the path was missing.
    stamp: Option<Stamp>,
    /// Content hash; `None` when the path was missing or unreadable.
    hash: Option<u64>,
    /// `stamp` cannot yet prove the content unchanged; see [`RACY_WINDOW`].
    racy: bool,
    /// Possibly changed before the tracker saw it (a file first found by a
    /// build that was already running); a rebuild is due regardless of content.
    forced: bool,
    /// The latest differing state a scan observed: the change is pending
    /// until it stops moving, then a rebuild consumes it.
    pending: Option<(Option<Stamp>, Option<u64>)>,
}

impl Entry {
    fn record(path: &Path) -> Self {
        // Metadata first: a write between the two reads leaves a stale stamp,
        // which the next scan notices and resolves by hashing.
        let stamp = Stamp::of(path);
        let hash = stamp.and_then(|_| hash_file(path));
        Self {
            stamp,
            hash,
            racy: is_racy(stamp, SystemTime::now()),
            forced: false,
            pending: None,
        }
    }
}

/// The files whose change means the document is out of date.
#[derive(Default)]
pub struct Tracker {
    files: BTreeMap<PathBuf, Entry>,
}

impl Tracker {
    /// Files that existed when last recorded; paths tracked only so that their
    /// creation is noticed are not counted.
    pub fn present(&self) -> usize {
        self.files
            .values()
            .filter(|entry| entry.stamp.is_some())
            .count()
    }

    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.files.keys().map(PathBuf::as_path)
    }

    /// Track exactly `paths`. Files tracked before keep the state recorded when
    /// the build started, so an edit made while the build ran is still seen.
    /// A newly tracked file modified since `build_started` may have been read
    /// before that edit, so it is marked as changed.
    pub fn replace(&mut self, paths: BTreeSet<PathBuf>, build_started: SystemTime) {
        self.files.retain(|path, _| paths.contains(path));
        for path in paths {
            self.files.entry(path).or_insert_with_key(|path| {
                let mut entry = Entry::record(path);
                entry.forced = entry
                    .stamp
                    .and_then(|stamp| stamp.modified)
                    .is_some_and(|modified| modified >= build_started);
                entry
            });
        }
    }

    /// One scan of every tracked file. `true` when a change appeared or moved
    /// since the previous scan.
    fn scan(&mut self) -> bool {
        let now = SystemTime::now();
        let mut moved = false;
        for (path, entry) in &mut self.files {
            let stamp = Stamp::of(path);
            if entry.pending.is_none() && !entry.forced && !entry.racy && stamp == entry.stamp {
                continue;
            }
            let hash = stamp.and_then(|_| hash_file(path));
            if !entry.forced && hash == entry.hash {
                // A touch, an identical rewrite, or a change already undone.
                entry.stamp = stamp;
                entry.racy = is_racy(stamp, now);
                entry.pending = None;
                continue;
            }
            let observed = Some((stamp, hash));
            if entry.pending != observed {
                moved = true;
                entry.pending = observed;
            }
        }
        moved
    }

    /// Accept the pending changes as the new baseline, as the build about to
    /// start will read them; returns the files whose content really changed.
    fn accept_pending(&mut self) -> Vec<PathBuf> {
        let mut changed = Vec::new();
        for (path, entry) in &mut self.files {
            if entry.pending.is_none() && !entry.forced {
                continue;
            }
            let fresh = Entry::record(path);
            if entry.forced || fresh.hash != entry.hash {
                changed.push(path.clone());
            }
            *entry = fresh;
        }
        changed
    }

    /// Block until tracked content changes and then stays quiet for the
    /// debounce interval; returns the changed files, or `None` on Ctrl-C.
    pub fn wait_for_change(&mut self) -> Option<Vec<PathBuf>> {
        let mut last_motion: Option<Instant> = None;
        loop {
            if sleep_interruptibly(POLL_INTERVAL) {
                return None;
            }
            if self.scan() {
                last_motion = Some(Instant::now());
            }
            let pending = self
                .files
                .values()
                .any(|entry| entry.pending.is_some() || entry.forced);
            if !pending {
                last_motion = None;
                continue;
            }
            if last_motion.is_none_or(|moved| moved.elapsed() >= DEBOUNCE) {
                let changed = self.accept_pending();
                if !changed.is_empty() {
                    return Some(changed);
                }
                last_motion = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("texmk-watch-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tracking(paths: &[&PathBuf]) -> Tracker {
        let mut tracker = Tracker::default();
        tracker.replace(
            paths.iter().map(|path| (*path).clone()).collect(),
            SystemTime::now() + Duration::from_secs(60),
        );
        tracker
    }

    #[test]
    fn touch_and_identical_rewrite_are_not_changes() {
        let dir = Dir::new("touch");
        let path = dir.file("a.tex", "one");
        let mut tracker = tracking(&[&path]);
        std::fs::write(&path, "one").unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(5)).unwrap();
        assert!(!tracker.scan());
        assert!(tracker.accept_pending().is_empty());
    }

    #[test]
    fn same_size_rewrite_inside_one_timestamp_tick_is_seen() {
        let dir = Dir::new("racy");
        let path = dir.file("a.tex", "one");
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let mut tracker = tracking(&[&path]);
        std::fs::write(&path, "two").unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(modified).unwrap();
        assert!(tracker.scan());
        assert_eq!(tracker.accept_pending(), vec![path]);
    }

    #[test]
    fn content_change_is_pending_until_accepted_once() {
        let dir = Dir::new("change");
        let path = dir.file("a.tex", "one");
        let mut tracker = tracking(&[&path]);
        std::fs::write(&path, "two!").unwrap();
        assert!(tracker.scan());
        assert!(!tracker.scan(), "an unmoving change is not new activity");
        assert_eq!(tracker.accept_pending(), vec![path]);
        assert!(!tracker.scan());
        assert!(tracker.accept_pending().is_empty());
    }

    #[test]
    fn atomic_rename_save_is_one_change() {
        let dir = Dir::new("rename");
        let path = dir.file("a.tex", "one");
        let mut tracker = tracking(&[&path]);
        let temporary = dir.file(".a.tex.swp", "new text");
        std::fs::rename(&temporary, &path).unwrap();
        assert!(tracker.scan());
        assert_eq!(tracker.accept_pending(), vec![path]);
    }

    #[test]
    fn deleted_then_recreated_file_stays_tracked() {
        let dir = Dir::new("recreate");
        let path = dir.file("a.tex", "one");
        let mut tracker = tracking(&[&path]);
        std::fs::remove_file(&path).unwrap();
        assert!(tracker.scan());
        std::fs::write(&path, "one").unwrap();
        assert!(
            !tracker.scan() && tracker.accept_pending().is_empty(),
            "identical content after the delete is no change"
        );
        std::fs::remove_file(&path).unwrap();
        assert!(tracker.scan());
        assert_eq!(tracker.accept_pending(), vec![path.clone()]);
        std::fs::write(&path, "other").unwrap();
        assert!(tracker.scan());
        assert_eq!(tracker.accept_pending(), vec![path]);
    }

    #[test]
    fn replace_keeps_baselines_drops_removed_and_flags_files_written_during_the_build() {
        let dir = Dir::new("replace");
        let kept = dir.file("kept.tex", "one");
        let dropped = dir.file("dropped.tex", "one");
        let mut tracker = tracking(&[&kept, &dropped]);
        std::fs::write(&kept, "edited while the build ran").unwrap();
        let during = dir.file("during.tex", "new");
        tracker.replace(
            [kept.clone(), during.clone()].into_iter().collect(),
            SystemTime::now() - Duration::from_secs(1),
        );
        assert_eq!(tracker.present(), 2);
        assert!(tracker.scan());
        let mut changed = tracker.accept_pending();
        changed.sort();
        assert_eq!(changed, vec![during, kept]);
    }
}
