//! Index generation for the build driver, as latexmk does it: every index
//! file a pass announces with `Writing index file X.idx` is processed by
//! `makeindex -o X.ind X.idx` (the embedded port), run in the directory that
//! holds the index.

use std::path::{Path, PathBuf};

pub mod tree;

/// Width at which the engine wraps transcript lines (`max_print_line`).
const LOG_LINE_WIDTH: usize = 79;

/// One makeindex invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// Input index.
    pub idx: PathBuf,
    /// Formatted index the engine reads back.
    pub ind: PathBuf,
    /// The command line, with names relative to the index's directory.
    pub options: tex_makeindex::Options,
}

impl Job {
    fn directory(&self) -> &Path {
        self.idx.parent().unwrap_or(Path::new("."))
    }
}

/// The host a job runs with: the index's directory as working directory
/// for its files, the source directory (and the TeX tree) for style files.
pub fn with_host<T>(job: &Job, source_dir: &Path, run: impl FnOnce(&tex_makeindex::DirHost) -> T) -> T {
    let tree = |name: &str| tree::style(source_dir, name);
    let host = tex_makeindex::DirHost {
        work_dir: source_dir,
        output_dir: Some(job.directory()),
        tree: &tree,
        stdin: false,
    };
    run(&host)
}

/// The index files a pass announced, in order of first mention: latexmk's
/// patterns for makeidx/multind/imakeidx and index.sty, with transcript
/// lines the engine wrapped joined again. Only `.idx` files have a rule.
fn announced_indexes(log: &str) -> Vec<String> {
    let mut logical: Vec<String> = Vec::new();
    let mut continued = false;
    for line in log.lines() {
        match logical.last_mut() {
            Some(last) if continued => last.push_str(line),
            _ => logical.push(line.to_string()),
        }
        continued = line.len() == LOG_LINE_WIDTH || line.chars().count() == LOG_LINE_WIDTH;
    }
    let mut found: Vec<String> = Vec::new();
    for line in &logical {
        let name = if let Some(name) = line.strip_prefix("Writing index file ") {
            name
        } else if let Some(name) = line.strip_prefix("index.sty> Writing index file ") {
            name
        } else if let Some(rest) = line.strip_prefix("Package ") {
            let Some((package, rest)) = rest.split_once(' ') else { continue };
            let Some(rest) = rest.strip_prefix("Info: Writing index file ") else { continue };
            let Some((name, _)) = rest.split_once(" on input line") else { continue };
            if package.is_empty() {
                continue;
            }
            name
        } else {
            continue;
        };
        let name = name.trim_end().to_string();
        if name.ends_with(".idx") && !found.contains(&name) {
            found.push(name);
        }
    }
    found
}

/// Makeindex jobs for the indexes a pass announced in its transcript `log`.
pub fn plan(log: &str, aux_dir: &Path) -> Vec<Job> {
    let mut jobs = Vec::new();
    for name in announced_indexes(log) {
        let idx = aux_dir.join(&name);
        if !idx.is_file() {
            continue;
        }
        let ind = idx.with_extension("ind");
        let file_name = |path: &Path| path.file_name().map(|name| name.to_string_lossy().into_owned());
        let (Some(idx_name), Some(ind_name)) = (file_name(&idx), file_name(&ind)) else { continue };
        let options = tex_makeindex::Options {
            quiet: true,
            output: Some(ind_name),
            inputs: vec![idx_name],
            ..Default::default()
        };
        jobs.push(Job { idx, ind, options });
    }
    jobs
}

/// Everything the output of a job depends on (input, style), as bytes to
/// be hashed.
pub fn signature_input(job: &Job, source_dir: &Path) -> Vec<u8> {
    let mut text = Vec::new();
    text.extend_from_slice(b"makeindex-embedded-2\n");
    text.extend_from_slice(format!("{:?}\n", job.options).as_bytes());
    // makeindex applies `X.mst` by itself to a lone `X.idx`.
    let mst = job.idx.with_extension("mst");
    let mst_name = mst.file_name().unwrap_or_default().to_string_lossy().into_owned();
    with_host(job, source_dir, |host| {
        use tex_makeindex::Host;
        if host.exists(&mst_name) {
            if let Some((path, bytes)) = host.find_style(&mst_name) {
                text.extend_from_slice(format!("style={path}\n").as_bytes());
                text.extend_from_slice(&bytes);
            }
        }
    });
    text.push(0);
    text.extend_from_slice(&tex_kpse::fs::read(&job.idx).unwrap_or_default());
    text
}

/// Runs a job; an error is makeindex's exit status.
pub fn run(job: &Job, source_dir: &Path) -> Result<(), i32> {
    match with_host(job, source_dir, |host| tex_makeindex::run(&job.options, host)) {
        0 => Ok(()),
        status => Err(status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_are_the_ones_latexmk_recognizes() {
        let log = concat!(
            "Writing index file main.idx\n",
            "Writing index file names.idx \n",
            "Package index Info: Writing index file main.adx on input line 7.\n",
            "Package index Info: Writing index file topics.idx on input line 8.\n",
            "index.sty> Writing index file old.idx\n",
            "Started index file main\n",
            "Writing index file main.idx\n",
        );
        assert_eq!(announced_indexes(log), ["main.idx", "names.idx", "topics.idx", "old.idx"]);
        let long = format!("Writing index file {}.idx", "x".repeat(70));
        let (head, tail) = long.split_at(LOG_LINE_WIDTH);
        assert_eq!(announced_indexes(&format!("{head}\n{tail}\n")), [long["Writing index file ".len()..].to_string()]);
    }
}
