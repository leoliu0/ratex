//! In-process, self-contained pdfLaTeX + BibTeX document builds.
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use tex_core::{
    driver,
    engine::{Engine, EngineKind, InteractionMode},
};
use tex_kpse::fs::{self, MemoryFs, ResourceContext};

pub mod convergence;
pub mod engine_selection;

const FORMAT_PDFLATEX: &[u8] = include_bytes!("../../tex-cli/assets/default.fmt.zst");
const FORMAT_XELATEX: &[u8] = include_bytes!("../../tex-cli/assets/xelatex.fmt.zst");
const FORMAT_LUALATEX: &[u8] = include_bytes!("../../tex-cli/assets/lualatex.fmt.zst");

pub fn format_for_engine(kind: EngineKind) -> &'static [u8] {
    match kind {
        EngineKind::PdfTeX => FORMAT_PDFLATEX,
        EngineKind::XeTeX => FORMAT_XELATEX,
        EngineKind::LuaTeX => FORMAT_LUALATEX,
    }
}

pub const MAX_PASSES: u32 = 5;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineChoice {
    #[default]
    Auto,
    Explicit(EngineKind),
}

impl EngineChoice {
    pub const fn selected(self) -> EngineKind {
        match self {
            Self::Auto => EngineKind::PdfTeX,
            Self::Explicit(kind) => kind,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PassPolicy {
    #[default]
    Auto,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ResourcePolicy {
    #[default]
    Isolated,
    ScopedDisk {
        project_root: PathBuf,
        allowed_input_roots: Vec<PathBuf>,
        allow_embedded: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileRequest {
    pub entry: String,
    pub job_name: Option<String>,
    pub engine: EngineChoice,
    pub passes: PassPolicy,
    pub interaction: InteractionMode,
    pub halt_on_error: bool,
    pub output_root: Option<String>,
    pub aux_root: Option<String>,
    pub resources: ResourcePolicy,
    pub timestamp: Option<u64>,
    pub synctex: bool,
    pub optimize_pdf: bool,
}

impl CompileRequest {
    pub fn new(entry: impl Into<String>) -> Self {
        Self {
            entry: entry.into(),
            job_name: None,
            engine: EngineChoice::Auto,
            passes: PassPolicy::Auto,
            interaction: InteractionMode::Nonstop,
            halt_on_error: true,
            output_root: None,
            aux_root: None,
            resources: ResourcePolicy::Isolated,
            timestamp: None,
            synctex: false,
            optimize_pdf: false,
        }
    }
}

pub struct PassOutcome {
    pub selected_engine: EngineKind,
    pub status: Status,
    pub log: String,
    pub diagnostics: String,
    pub artifacts: BTreeMap<String, Vec<u8>>,
    pub auxiliary_observations: BTreeMap<String, Vec<u8>>,
    pub bibliography_inputs: BTreeMap<String, Vec<u8>>,
    pub bibliography_required: bool,
    final_engine: Option<Engine>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Status {
    Success = 0,
    CompilationError = 1,
    InvalidInput = 2,
    NoConvergence = 3,
    InternalError = 4,
    UnsupportedEngine = 5,
}

pub struct Compilation {
    pub selected_engine: EngineKind,
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
        Self::error_for(EngineKind::PdfTeX, status, message)
    }

    pub fn error_for(
        selected_engine: EngineKind,
        status: Status,
        message: impl Into<String>,
    ) -> Self {
        let message = message.into();
        Self {
            selected_engine,
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
        self.compile_request(CompileRequest::new(entry))
    }

    /// JS supplies Date.now() explicitly; the core has no JavaScript imports.
    pub fn compile_at(&self, entry: &str, now: u64) -> Compilation {
        let mut request = CompileRequest::new(entry);
        request.timestamp = Some(now);
        self.compile_request(request)
    }

    pub fn compile_request(&self, request: CompileRequest) -> Compilation {
        let (project_root, allowed_input_roots, allow_embedded, is_disk) = match &request.resources {
            ResourcePolicy::Isolated => (PathBuf::from("/project"), Vec::new(), true, false),
            ResourcePolicy::ScopedDisk {
                project_root,
                allowed_input_roots,
                allow_embedded,
            } => (project_root.clone(), allowed_input_roots.clone(), *allow_embedded, true),
        };

        let timestamp = self.epoch.or(request.timestamp).unwrap_or_else(host_epoch);
        if timestamp > 253_402_300_799 {
            return Compilation::error_for(
                request.engine.selected(),
                Status::InvalidInput,
                "timestamp exceeds year 9999",
            );
        }

        let entry = match project_path(&request.entry) {
            Ok(entry) if !is_disk && !self.inputs.contains_key(&entry) => {
                return Compilation::error_for(
                    request.engine.selected(),
                    Status::InvalidInput,
                    format!("entry file not found: {entry}"),
                )
            }
            Ok(entry) => entry,
            Err(error) => {
                return Compilation::error_for(request.engine.selected(), Status::InvalidInput, error)
            }
        };

        let path = if is_disk {
            project_root.join(&entry)
        } else {
            Path::new("/project").join(&entry)
        };
        let cwd = path.parent().unwrap();
        let default_job = match path.file_stem().and_then(|name| name.to_str()) {
            Some(job) if !job.is_empty() => job,
            _ => {
                return Compilation::error_for(
                    request.engine.selected(),
                    Status::InvalidInput,
                    "entry file has no job name",
                )
            }
        };
        let job = request.job_name.as_deref().unwrap_or(default_job);
        if let Err(error) = validate_job_name(job) {
            return Compilation::error_for(request.engine.selected(), Status::InvalidInput, error);
        }
        let output_dir = match request_directory(request.output_root.as_deref(), cwd) {
            Ok(path) => path,
            Err(error) => {
                return Compilation::error_for(request.engine.selected(), Status::InvalidInput, error)
            }
        };
        let aux_dir = match request_directory(request.aux_root.as_deref(), cwd) {
            Ok(path) => path,
            Err(error) => {
                return Compilation::error_for(request.engine.selected(), Status::InvalidInput, error)
            }
        };

        let memory = match MemoryFs::new(cwd, timestamp) {
            Ok(memory) => memory,
            Err(error) => {
                return Compilation::error_for(
                    request.engine.selected(),
                    Status::InvalidInput,
                    error.to_string(),
                )
            }
        };
        for (name, bytes) in &self.inputs {
            if let Err(error) = memory.insert(&Path::new("/project").join(name), bytes.clone()) {
                return Compilation::error_for(
                    request.engine.selected(),
                    Status::InvalidInput,
                    format!("{name}: {error}"),
                );
            }
        }

        let context = if is_disk {
            match ResourceContext::disk(
                cwd,
                &allowed_input_roots,
                &output_dir,
                &aux_dir,
                allow_embedded,
                Some(timestamp),
            ) {
                Ok(context) => context,
                Err(error) => {
                    return Compilation::error_for(
                        request.engine.selected(),
                        Status::InvalidInput,
                        error.to_string(),
                    )
                }
            }
        } else {
            match ResourceContext::memory(memory.clone(), &output_dir, &aux_dir, allow_embedded) {
                Ok(context) => context,
                Err(error) => {
                    return Compilation::error_for(
                        request.engine.selected(),
                        Status::InvalidInput,
                        error.to_string(),
                    )
                }
            }
        };

        let _scope = context.enter();
        for directory in [&output_dir, &aux_dir] {
            if let Err(error) = fs::create_dir_all(directory) {
                return Compilation::error_for(
                    request.engine.selected(),
                    Status::InvalidInput,
                    error.to_string(),
                );
            }
        }

        // Automatic engine selection
        let initial_engine = match request.engine {
            EngineChoice::Explicit(kind) => kind,
            EngineChoice::Auto => {
                let source_str = if is_disk {
                    std::fs::read_to_string(&path).unwrap_or_default()
                } else {
                    self.inputs.get(&entry).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or_default().to_string()
                };
                engine_selection::detect_required_engine_from_source(&source_str).unwrap_or(EngineKind::PdfTeX)
            }
        };

        let mut selected_engine = initial_engine;
        if selected_engine == EngineKind::XeTeX {
            return Compilation::error_for(
                selected_engine,
                Status::UnsupportedEngine,
                format!("{} semantics are not implemented", selected_engine.command_name()),
            );
        }
        let mut attempted_engines = vec![selected_engine];
        let mut result = Compilation::error_for(selected_engine, Status::NoConvergence, "");
        let mut previous = BTreeMap::new();
        let mut bibliography = None;
        let mut final_engine = None;
        let max_passes = MAX_PASSES;
        for pass in 1..=max_passes {
            result.passes = pass;
            let mut outcome = self.run_pass(
                selected_engine,
                &request,
                &memory,
                &path,
                cwd,
                &aux_dir,
                &output_dir,
                job,
            );
            result
                .log
                .push_str(&format!("--- TeX pass {pass} ---\n{}", outcome.log));
            result.diagnostics.push_str(&outcome.diagnostics);
            result.files = outcome.artifacts.clone();
            if outcome.status != Status::Success {
                if request.engine == EngineChoice::Auto {
                    if let Some(next_engine) = engine_selection::detect_engine_switch_need(
                        selected_engine,
                        &outcome.log,
                        &outcome.diagnostics,
                    ) {
                        if !attempted_engines.contains(&next_engine) {
                            attempted_engines.push(next_engine);
                            selected_engine = next_engine;
                            result = Compilation::error_for(selected_engine, Status::NoConvergence, "");
                            previous.clear();
                            bibliography = None;
                            final_engine = None;
                            continue;
                        }
                    }
                }
                result.status = outcome.status;
                break;
            }
            if outcome.artifacts.keys().any(|name| name.ends_with(".bcf")) {
                result.status = Status::CompilationError;
                result.diagnostics.push_str(
                    "Biber is not available in the in-process library; use a BibTeX bibliography.\n",
                );
                break;
            }
            final_engine = outcome.final_engine.take();

            let has_bib_inputs = !outcome.bibliography_inputs.is_empty();
            let mut ran_bibtex = false;
            if outcome.bibliography_required
                && bibliography.as_ref() != Some(&outcome.bibliography_inputs)
            {
                let status = tex_bibtex::driver::run(
                    &[aux_dir.join(job).to_string_lossy().into_owned()],
                    env!("CARGO_PKG_VERSION"),
                );
                result.bibtex_runs += 1;
                let log =
                    fs::read_to_string(aux_dir.join(format!("{job}.blg"))).unwrap_or_default();
                result.log.push_str(&format!("--- BibTeX ---\n{log}"));
                if status != 0 {
                    result.status = Status::CompilationError;
                    result.diagnostics.push_str(&log);
                    break;
                }
                bibliography = Some(outcome.bibliography_inputs);
                ran_bibtex = true;
            }
            let sound_one_pass = pass == 1
                && convergence::ConvergenceState::is_sound_single_pass(
                    &fs::read_to_string(aux_dir.join(format!("{job}.aux"))).unwrap_or_default(),
                    &outcome.log,
                    has_bib_inputs,
                    outcome.bibliography_required,
                );
            let stable = sound_one_pass || (outcome.auxiliary_observations == previous && !ran_bibtex);
            previous = outcome.auxiliary_observations;
            if stable {
                result.status = Status::Success;
                break;
            }
        }
        if result.status == Status::NoConvergence {
            result.diagnostics.push_str(&format!(
                "auxiliary files did not converge after {max_passes} TeX passes\n"
            ));
        }
        if result.status == Status::Success {
            if let Some(mut engine) = final_engine {
                if engine.pdf_doc.pages.is_empty() {
                    result.status = Status::CompilationError;
                    result
                        .diagnostics
                        .push_str("document produced no PDF pages\n");
                } else {
                    match driver::finish_pdf(&mut engine, request.optimize_pdf) {
                        Ok(pdf) => result.pdf = pdf,
                        Err(error) => {
                            result.status = Status::CompilationError;
                            result.diagnostics.push_str(&error);
                        }
                    }
                }
            }
        }
        let _ = fs::write(aux_dir.join(format!("{job}.log")), result.log.as_bytes());
        if result.status == Status::Success {
            let _ = fs::write(output_dir.join(format!("{job}.pdf")), &result.pdf);
        }
        result.files = memory.outputs();
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn run_pass(
        &self,
        selected_engine: EngineKind,
        request: &CompileRequest,
        memory: &MemoryFs,
        path: &Path,
        cwd: &Path,
        aux_dir: &Path,
        output_dir: &Path,
        job: &str,
    ) -> PassOutcome {
        let format_bytes = format_for_engine(selected_engine);
        let mut engine = match tex_core::format::load_format_from(format_bytes) {
            Ok(engine) => engine,
            Err(error) => {
                return PassOutcome {
                    selected_engine,
                    status: Status::InternalError,
                    log: String::new(),
                    diagnostics: format!("embedded LaTeX format: {error}"),
                    artifacts: memory.outputs(),
                    auxiliary_observations: BTreeMap::new(),
                    bibliography_inputs: BTreeMap::new(),
                    bibliography_required: false,
                    final_engine: None,
                }
            }
        };
        if engine.engine_kind != selected_engine {
            return PassOutcome {
                selected_engine,
                status: Status::InternalError,
                log: String::new(),
                diagnostics: format!(
                    "embedded {} format cannot execute {} semantics",
                    engine.engine_kind.command_name(),
                    selected_engine.command_name()
                ),
                artifacts: memory.outputs(),
                auxiliary_observations: BTreeMap::new(),
                bibliography_inputs: BTreeMap::new(),
                bibliography_required: false,
                final_engine: None,
            };
        }
        driver::finalize_format_load(&mut engine);
        driver::prepare_latex_job(&mut engine);
        engine.set_interaction_mode(request.interaction);
        engine.halt_on_error = request.halt_on_error;
        engine.allow_missing_main_aux = true;
        engine.job_name = job.to_owned();
        engine.main_dir = Some(cwd.to_owned());
        engine.aux_dir = Some(aux_dir.to_owned());
        engine.out_dir = format!("{}/", output_dir.display());
        engine.synctex_enabled = request.synctex;
        if engine.input_file(path.to_str().unwrap()) {
            driver::insert_everyjob(&mut engine);
            engine.run();
            engine.finish_job_diagnostics();
        }
        for stream in &mut engine.write_streams {
            stream.take();
        }
        let mut diagnostics = String::new();
        for diagnostic in &engine.diagnostics {
            diagnostics.push_str(&diagnostic.render());
        }
        let status = if engine.error_count > 0 || engine.stopped_on_error {
            Status::CompilationError
        } else {
            Status::Success
        };
        let artifacts = memory.outputs();
        let bibliography_inputs: BTreeMap<_, _> = artifacts
            .iter()
            .filter(|(name, _)| name.ends_with(".aux"))
            .map(|(name, bytes)| (name.clone(), bytes.clone()))
            .collect();
        let main_aux = fs::read_to_string(aux_dir.join(format!("{job}.aux"))).unwrap_or_default();
        let bibliography_required = main_aux.contains("\\bibdata{")
            || bibliography_inputs
                .values()
                .any(|bytes| String::from_utf8_lossy(bytes).contains("\\bibdata{"));
        let auxiliary_observations = auxiliary_state(&artifacts);
        PassOutcome {
            selected_engine,
            status,
            log: engine.log.clone(),
            diagnostics,
            artifacts,
            auxiliary_observations,
            bibliography_inputs,
            bibliography_required,
            final_engine: Some(engine),
        }
    }
}

fn validate_job_name(job: &str) -> Result<(), String> {
    if job.is_empty()
        || job.contains('\0')
        || job.contains('/')
        || job.contains('\\')
        || job == "."
        || job == ".."
    {
        Err("job name must be one non-empty path component".into())
    } else {
        Ok(())
    }
}

fn request_directory(value: Option<&str>, default: &Path) -> Result<PathBuf, String> {
    match value {
        None => Ok(default.to_owned()),
        Some(".") => Ok(PathBuf::from("/project")),
        Some(value) => project_path(value).map(|path| Path::new("/project").join(path)),
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
