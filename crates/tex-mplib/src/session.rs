//! MetaPost / mplib session manager.

use crate::interp::Interpreter;
use crate::types::MpFigure;

/// Configuration options for initializing an mplib session.
#[derive(Clone, Debug, Default)]
pub struct MpConfig {
    pub math_mode: Option<String>,
    pub random_seed: Option<u32>,
    pub job_name: Option<String>,
}

/// Result returned from executing a MetaPost code chunk.
#[derive(Clone, Debug)]
pub struct MpResult {
    pub status: i32,
    pub log: String,
    pub term: String,
    pub fig: Vec<MpFigure>,
}

/// An active MetaPost interpreter session (matching mplib in LuaTeX).
pub struct MpSession {
    pub interp: Interpreter,
    pub config: MpConfig,
    pub finished: bool,
}

impl MpSession {
    pub fn new(config: MpConfig) -> Self {
        Self {
            interp: Interpreter::new(),
            config,
            finished: false,
        }
    }

    /// Executes MetaPost code and returns generated figures, logs, and status.
    pub fn execute(&mut self, code: &str) -> MpResult {
        if self.finished {
            return MpResult {
                status: 2,
                log: "Session already finished\n".into(),
                term: "Session already finished\n".into(),
                fig: Vec::new(),
            };
        }

        let run_res = self.interp.run(code);
        let fig = std::mem::take(&mut self.interp.figures);
        let log = std::mem::take(&mut self.interp.log);
        let term = std::mem::take(&mut self.interp.term);

        match run_res {
            Ok(()) => MpResult {
                status: 0,
                log,
                term,
                fig,
            },
            Err(e) => MpResult {
                status: 2,
                log: format!("{log}Error: {e}\n"),
                term: format!("{term}Error: {e}\n"),
                fig,
            },
        }
    }

    /// Finishes the MetaPost session.
    pub fn finish(&mut self) -> MpResult {
        self.finished = true;
        MpResult {
            status: 0,
            log: String::new(),
            term: String::new(),
            fig: Vec::new(),
        }
    }
}
