//! In-process, self-contained pdfLaTeX + BibTeX document builds.
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use tex_core::{driver, engine::InteractionMode};
use tex_kpse::fs::{self, MemoryFs};

const FORMAT: &[u8] = include_bytes!("../../tex-cli/assets/default.fmt.zst");
pub const MAX_PASSES: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Status {
    Success = 0,
    CompilationError = 1,
    InvalidInput = 2,
    NoConvergence = 3,
    InternalError = 4,
}

pub struct Compilation {
    pub status: Status,
    pub pdf: Vec<u8>,
    pub log: String,
    pub diagnostics: String,
    pub files: BTreeMap<String, Vec<u8>>,
    pub passes: u32,
    pub bibtex_runs: u32,
}

impl Compilation {
    pub fn error(status: Status, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            status,
            pdf: Vec::new(),
            log: message.clone(),
            diagnostics: message,
            files: BTreeMap::new(),
            passes: 0,
            bibtex_runs: 0,
        }
    }
}

/// Inputs persist between calls; generated files belong only to the result.
/// A session is used by one caller at a time. Independent sessions may run on
/// separate native threads, without changing cwd, environment, or panic hooks.
#[derive(Default)]
pub struct Session {
    inputs: BTreeMap<String, Vec<u8>>,
    epoch: Option<u64>,
}

pub fn project_path(name: &str) -> Result<String, String> {
    if name.is_empty()
        || name.contains('\0')
        || name.contains('\\')
        || name.contains(':')
        || Path::new(name).is_absolute()
    {
        return Err("expected a relative UTF-8 project path using / separators".into());
    }
    let mut result = PathBuf::new();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(part) => result.push(part),
            Component::CurDir => {}
            Component::ParentDir if result.pop() => {}
            _ => return Err("path escapes the project".into()),
        }
    }
    if result.as_os_str().is_empty() {
        return Err("expected a file path".into());
    }
    Ok(result.to_string_lossy().replace('\\', "/"))
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn add_file(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let name = project_path(name)?;
        self.inputs.insert(name, bytes.to_vec());
        Ok(())
    }
    pub fn remove_file(&mut self, name: &str) -> Result<(), String> {
        self.inputs.remove(&project_path(name)?);
        Ok(())
    }
    /// UTC Unix seconds, or None for the host clock at the next compile.
    pub fn set_epoch(&mut self, epoch: Option<u64>) -> Result<(), String> {
        if epoch.is_some_and(|n| n > 253_402_300_799) {
            return Err("timestamp exceeds year 9999".into());
        }
        self.epoch = epoch;
        Ok(())
    }
    pub fn compile(&self, entry: &str) -> Compilation {
        self.compile_at(entry, host_epoch())
    }
    /// JS supplies Date.now() explicitly; the core has no JavaScript imports.
    pub fn compile_at(&self, entry: &str, now: u64) -> Compilation {
        let entry = match project_path(entry) {
            Ok(entry) if self.inputs.contains_key(&entry) => entry,
            Ok(entry) => {
                return Compilation::error(
                    Status::InvalidInput,
                    format!("entry file not found: {entry}"),
                )
            }
            Err(error) => return Compilation::error(Status::InvalidInput, error),
        };
        let path = Path::new("/project").join(&entry);
        let cwd = path.parent().unwrap();
        let job = match path.file_stem().and_then(|s| s.to_str()) {
            Some(job) if !job.is_empty() => job,
            _ => return Compilation::error(Status::InvalidInput, "entry file has no job name"),
        };
        let memory = match MemoryFs::new(cwd, self.epoch.unwrap_or(now).min(253_402_300_799)) {
            Ok(memory) => memory,
            Err(error) => return Compilation::error(Status::InvalidInput, error.to_string()),
        };
        for (name, bytes) in &self.inputs {
            if let Err(error) = memory.insert(&Path::new("/project").join(name), bytes.clone()) {
                return Compilation::error(Status::InvalidInput, format!("{name}: {error}"));
            }
        }
        let _scope = memory.enter();
        let mut result = Compilation::error(Status::NoConvergence, "");
        let mut previous = BTreeMap::new();
        let mut bibliography = None;
        let mut final_engine = None;
        for pass in 1..=MAX_PASSES {
            result.passes = pass;
            let mut engine = match tex_core::format::load_format_from(FORMAT) {
                Ok(engine) => engine,
                Err(error) => {
                    return Compilation::error(
                        Status::InternalError,
                        format!("embedded LaTeX format: {error}"),
                    )
                }
            };
            driver::finalize_format_load(&mut engine);
            driver::prepare_latex_job(&mut engine);
            engine.set_interaction_mode(InteractionMode::Nonstop);
            engine.halt_on_error = true;
            engine.allow_missing_main_aux = true;
            engine.job_name = job.to_owned();
            engine.main_dir = Some(cwd.to_owned());
            engine.aux_dir = Some(cwd.to_owned());
            engine.out_dir = format!("{}/", cwd.display());
            engine.synctex_enabled = false;
            if engine.input_file(path.to_str().unwrap()) {
                driver::insert_everyjob(&mut engine);
                engine.run();
                engine.finish_job_diagnostics();
            }
            // Close streams before BibTeX or a following engine reads them.
            for stream in &mut engine.write_streams {
                stream.take();
            }
            result
                .log
                .push_str(&format!("--- TeX pass {pass} ---\n{}", engine.log));
            for diagnostic in &engine.diagnostics {
                result.diagnostics.push_str(&diagnostic.render());
            }
            if engine.error_count > 0 || engine.stopped_on_error {
                result.status = Status::CompilationError;
                break;
            }
            let outputs = memory.outputs();
            if outputs.keys().any(|name| name.ends_with(".bcf")) {
                result.status = Status::CompilationError;
                result.diagnostics.push_str("Biber is not available in the in-process library; use a BibTeX bibliography.\n");
                break;
            }
            let aux = fs::read_to_string(cwd.join(format!("{job}.aux"))).unwrap_or_default();
            let aux_files: BTreeMap<_, _> = outputs
                .iter()
                .filter(|(name, _)| name.ends_with(".aux"))
                .map(|(n, b)| (n.clone(), b.clone()))
                .collect();
            let needs_bib = aux.contains("\\bibdata{")
                || aux_files
                    .values()
                    .any(|b| String::from_utf8_lossy(b).contains("\\bibdata{"));
            let mut ran_bibtex = false;
            if needs_bib && bibliography.as_ref() != Some(&aux_files) {
                let status = tex_bibtex::driver::run(
                    &[cwd.join(job).to_string_lossy().into_owned()],
                    env!("CARGO_PKG_VERSION"),
                );
                result.bibtex_runs += 1;
                let log = fs::read_to_string(cwd.join(format!("{job}.blg"))).unwrap_or_default();
                result.log.push_str(&format!("--- BibTeX ---\n{log}"));
                if status != 0 {
                    result.status = Status::CompilationError;
                    result.diagnostics.push_str(&log);
                    break;
                }
                bibliography = Some(aux_files);
                ran_bibtex = true;
            }
            let current = auxiliary_state(&memory.outputs());
            let stable = current == previous;
            previous = current;
            final_engine = Some(engine);
            if stable && !ran_bibtex {
                result.status = Status::Success;
                break;
            }
        }
        if result.status == Status::NoConvergence {
            result
                .diagnostics
                .push_str("auxiliary files did not converge after five TeX passes\n");
        }
        if result.status == Status::Success {
            if let Some(mut engine) = final_engine {
                if engine.pdf_doc.pages.is_empty() {
                    result.status = Status::CompilationError;
                    result
                        .diagnostics
                        .push_str("document produced no PDF pages\n");
                } else {
                    match driver::finish_pdf(&mut engine, false) {
                        Ok(pdf) => {
                            result.pdf = pdf;
                        }
                        Err(error) => {
                            result.status = Status::CompilationError;
                            result.diagnostics.push_str(&error);
                        }
                    }
                }
            }
        }
        let _ = fs::write(cwd.join(format!("{job}.log")), result.log.as_bytes());
        if result.status == Status::Success {
            let _ = fs::write(cwd.join(format!("{job}.pdf")), &result.pdf);
        }
        result.files = memory.outputs();
        result
    }
}

fn auxiliary_state(files: &BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Vec<u8>> {
    files
        .iter()
        .filter(|(name, _)| {
            !matches!(
                Path::new(name).extension().and_then(|s| s.to_str()),
                Some("pdf" | "log" | "blg" | "synctex" | "gz")
            )
        })
        .map(|(name, bytes)| (name.clone(), bytes.clone()))
        .collect()
}

fn host_epoch() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}
