//! Index generation for the build driver: finds the `.idx` files a pass
//! wrote, works out the makeindex command that processes each (the one
//! imakeidx announces in the transcript, or latexmk's `makeindex -o X.ind
//! X.idx`) and runs the embedded makeindex on it.

use std::path::{Path, PathBuf};

use tex_kpse::fs::PathExt;

/// Width at which the engine wraps transcript lines (`max_print_line`).
const LOG_LINE_WIDTH: usize = 79;

/// One makeindex invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// Input index, in the auxiliary directory.
    pub idx: PathBuf,
    /// Formatted index the engine reads back.
    pub ind: PathBuf,
    /// Style file name as the command names it.
    pub style: Option<String>,
    /// The options that govern the run, without file names.
    pub options: tex_makeindex::Options,
}

/// File system and style lookup for the embedded makeindex.
pub struct Host<'a> {
    /// Directory of the main source; styles live next to it.
    pub source_dir: &'a Path,
    pub aux_dir: &'a Path,
}

impl Host<'_> {
    fn style_file(&self, name: &str) -> Option<(PathBuf, Vec<u8>)> {
        let with_extension = if name.ends_with(".ist") { name.to_string() } else { format!("{name}.ist") };
        let mut candidates = Vec::new();
        for directory in [self.source_dir, self.aux_dir] {
            candidates.push(directory.join(name));
            candidates.push(directory.join(&with_extension));
        }
        if Path::new(name).is_absolute() {
            candidates.insert(0, PathBuf::from(name));
        }
        for candidate in candidates {
            if candidate.tex_is_file() {
                if let Ok(bytes) = tex_kpse::fs::read(&candidate) {
                    return Some((candidate, bytes));
                }
            }
        }
        let kpse = tex_kpse::Kpse::with_roots(self.source_dir, &[]);
        for file in [name, with_extension.as_str()] {
            if let Some(path) = kpse.find_any(file).filter(|path| path.tex_is_file()) {
                if let Ok(bytes) = tex_kpse::fs::read(&path) {
                    return Some((path, bytes));
                }
            }
        }
        tex_kpse::get_embedded_package(&with_extension)
            .map(|bytes| (PathBuf::from(&with_extension), bytes))
    }
}

impl tex_makeindex::Host for Host<'_> {
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        tex_kpse::fs::read(path)
    }

    fn write(&self, path: &str, bytes: &[u8]) -> std::io::Result<()> {
        tex_kpse::fs::write(path, bytes)
    }

    fn exists(&self, path: &str) -> bool {
        Path::new(path).tex_is_file()
    }

    fn find_style(&self, name: &str) -> Option<(String, Vec<u8>)> {
        self.style_file(name).map(|(path, bytes)| (path.to_string_lossy().into_owned(), bytes))
    }

    fn read_stdin(&self) -> std::io::Result<Vec<u8>> {
        Ok(Vec::new())
    }
}

/// Splits a command line the way a shell would for the simple cases imakeidx
/// produces: blanks separate words and double quotes group.
fn split_command(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quoted = false;
    for character in text.chars() {
        match character {
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            c => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(current);
    }
    words
}

/// The commands imakeidx asks to have run, from its "Remember to run ...
/// after calling `command'" warnings in the transcript.
fn announced_commands(log: &str) -> Vec<String> {
    let lines: Vec<&str> = log.lines().collect();
    let mut commands = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        if !lines[at].starts_with("Package imakeidx Warning: Remember to run") {
            at += 1;
            continue;
        }
        at += 1;
        // The command is quoted `like this'; the engine may wrap it anywhere.
        let mut text = String::new();
        let mut started = false;
        while at < lines.len() {
            let line = lines[at];
            let (body, wrapped_from_previous) = match line.strip_prefix("(imakeidx)") {
                Some(rest) => (rest.trim_start(), false),
                None => (line, true),
            };
            if !started && !body.contains('`') {
                if wrapped_from_previous {
                    break;
                }
                at += 1;
                continue;
            }
            let mut piece = body;
            if !started {
                piece = &piece[piece.find('`').map_or(0, |index| index + 1)..];
                started = true;
            }
            if let Some(end) = piece.find('\'') {
                text.push_str(&piece[..end]);
                commands.push(text);
                at += 1;
                break;
            }
            text.push_str(piece);
            // A physical line that fills the width continues on the next one
            // without a break in the text; a shorter one ended at a blank.
            if lines[at].len() < LOG_LINE_WIDTH {
                text.push(' ');
            }
            at += 1;
        }
    }
    commands
}

/// Reads the makeindex options out of an announced command; `None` when the
/// command is not a makeindex call this driver can run.
fn parse_command(words: &[String]) -> Option<(tex_makeindex::Options, String)> {
    let (program, rest) = words.split_first()?;
    let name = Path::new(program).file_stem()?.to_string_lossy().to_ascii_lowercase();
    if name != "makeindex" {
        return None;
    }
    let options = tex_makeindex::parse_args(rest).ok()?;
    let input = options.inputs.first()?.clone();
    Some((options, input))
}

/// The `.idx` files an engine pass wrote into `aux_dir` (from its recorder
/// file), in order of first appearance.
fn written_indexes(fls: &str, aux_dir: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for line in fls.lines() {
        let Some(path) = line.strip_prefix("OUTPUT ") else { continue };
        let path = cwd.join(path);
        if path.extension().and_then(|extension| extension.to_str()) != Some("idx") {
            continue;
        }
        if path.parent() == Some(aux_dir) && !found.contains(&path) {
            found.push(path);
        }
    }
    found
}

/// Makeindex jobs for the indexes a pass wrote. Each follows latexmk's
/// `makeindex -o X.ind X.idx` unless imakeidx announced a command for it.
pub fn plan(fls: &str, log: &str, aux_dir: &Path, cwd: &Path) -> Vec<Job> {
    let announced: Vec<(tex_makeindex::Options, String)> = announced_commands(log)
        .iter()
        .filter_map(|command| parse_command(&split_command(command)))
        .collect();
    let mut jobs = Vec::new();
    for idx in written_indexes(fls, aux_dir, cwd) {
        let file = idx.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let mut options = announced
            .iter()
            .find(|(options, input)| {
                input == &file || options.inputs.first().is_some_and(|name| Path::new(name).file_name().is_some_and(|n| n.to_string_lossy() == file))
            })
            .map(|(options, _)| options.clone())
            .unwrap_or_default();
        let style = options.style.clone();
        let ind = match &options.output {
            Some(name) => aux_dir.join(name),
            None => idx.with_extension("ind"),
        };
        options.quiet = true;
        options.inputs = vec![idx.to_string_lossy().into_owned()];
        options.output = Some(ind.to_string_lossy().into_owned());
        options.log = Some(
            match &options.log {
                Some(name) => aux_dir.join(name),
                None => idx.with_extension("ilg"),
            }
            .to_string_lossy()
            .into_owned(),
        );
        jobs.push(Job { idx, ind, style, options });
    }
    jobs
}

/// Everything the output of a job depends on (options, input, style), as
/// bytes to be hashed.
pub fn signature_input(job: &Job, host: &Host) -> Vec<u8> {
    let mut text = Vec::new();
    text.extend_from_slice(b"makeindex-embedded-1\n");
    let options = &job.options;
    for (name, set) in [
        ("l", options.letter_ordering),
        ("c", options.compress_blanks),
        ("r", options.no_ranges),
        ("g", options.german),
    ] {
        text.extend_from_slice(format!("{name}={set}\n").as_bytes());
    }
    text.extend_from_slice(format!("p={:?}\n", options.start_page).as_bytes());
    text.extend_from_slice(format!("idx={}\n", job.idx.file_name().unwrap_or_default().to_string_lossy()).as_bytes());
    text.extend_from_slice(format!("ind={}\n", job.ind.file_name().unwrap_or_default().to_string_lossy()).as_bytes());
    if let Some(style) = &job.style {
        text.extend_from_slice(format!("style={style}\n").as_bytes());
        if let Some((_, bytes)) = host.style_file(style) {
            text.extend_from_slice(&bytes);
        }
    }
    text.push(0);
    text.extend_from_slice(&tex_kpse::fs::read(&job.idx).unwrap_or_default());
    text
}

/// Runs a job; an error is makeindex's exit status.
pub fn run(job: &Job, host: &Host) -> Result<(), i32> {
    match tex_makeindex::run(&job.options, host) {
        0 => Ok(()),
        status => Err(status),
    }
}
