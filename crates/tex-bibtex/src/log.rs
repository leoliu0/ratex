//! Terminal and `.blg` output plus the job history (bibtex.web's
//! `print`, `log_pr`, `mark_warning`, `mark_error`, `mark_fatal`).

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum History {
    Spotless,
    Warning,
    Error,
    Fatal,
}

pub struct Log {
    /// everything printed, i.e. the `.blg` contents
    pub blg: Vec<u8>,
    /// the subset that also goes to the terminal
    pub term: Vec<u8>,
    pub history: History,
    pub err_count: u32,
}

impl Log {
    pub fn new() -> Self {
        Log {
            blg: Vec::new(),
            term: Vec::new(),
            history: History::Spotless,
            err_count: 0,
        }
    }

    /// `print`: terminal and log.
    pub fn print(&mut self, s: impl AsRef<[u8]>) {
        let s = s.as_ref();
        self.blg.extend_from_slice(s);
        self.term.extend_from_slice(s);
    }

    /// `print_ln`.
    pub fn println(&mut self, s: impl AsRef<[u8]>) {
        self.print(s);
        self.print(b"\n");
    }

    /// `log_pr`: log file only.
    pub fn log_only(&mut self, s: impl AsRef<[u8]>) {
        self.blg.extend_from_slice(s.as_ref());
    }

    pub fn mark_warning(&mut self) {
        match self.history {
            History::Warning => self.err_count += 1,
            History::Spotless => {
                self.history = History::Warning;
                self.err_count = 1;
            }
            _ => {}
        }
    }

    pub fn mark_error(&mut self) {
        if self.history < History::Error {
            self.history = History::Error;
            self.err_count = 1;
        } else {
            self.err_count += 1;
        }
    }

    pub fn mark_fatal(&mut self) {
        self.history = History::Fatal;
    }

    /// `<Print the job history>`.
    pub fn print_history(&mut self) {
        let n = self.err_count;
        match self.history {
            History::Spotless => {}
            History::Warning if n == 1 => self.println("(There was 1 warning)"),
            History::Warning => self.println(format!("(There were {n} warnings)")),
            History::Error if n == 1 => self.println("(There was 1 error message)"),
            History::Error => self.println(format!("(There were {n} error messages)")),
            History::Fatal => self.println("(That was a fatal error)"),
        }
    }
}
