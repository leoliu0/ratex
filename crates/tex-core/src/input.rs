//! Input sources: files (line-based) and token lists (macros, args, registers).

use crate::token::Token;

pub const EOF_MARKER: Token = Token(0xFFFF_FFFF);
pub const PAR_END: Token = Token(0xFFFF_FFFE); // blank line -> \par

#[derive(Clone, Debug)]
pub enum Source {
    File {
        name: String,
        data: Vec<u8>,
        pos: usize,
        line_no: u32,
        /// tokenizer state: 0 = new line, 1 = mid line, 2 = skip spaces
        state: u8,
        /// set by \endinput: stop at end of current line
        ending: bool,
        /// true when the source has returned EOF already
        done: bool,
        /// pending ParEnd to deliver once
        pending_par: bool,
        at_eof: bool,
    },
    TokList {
        toks: Vec<Token>,
        pos: usize,
        /// macro parameters (only for macro playback)
        params: Vec<Vec<Token>>,
        param_idx: usize,
        param_pos: usize,
        /// inside a param list right now
        in_param: bool,
        name: String,
    },
}

pub struct InputStack {
    pub stack: Vec<Source>,
}

impl InputStack {
    pub fn new() -> Self {
        InputStack { stack: Vec::new() }
    }

    pub fn push_file(&mut self, name: String, data: Vec<u8>) {
        self.stack.push(Source::File {
            name,
            data,
            pos: 0,
            line_no: 0,
            state: 0,
            ending: false,
            done: false,
            pending_par: false,
            at_eof: false,
        });
    }

    pub fn push_toks(&mut self, toks: Vec<Token>, name: &str) {
        self.stack.push(Source::TokList {
            toks,
            pos: 0,
            params: Vec::new(),
            param_idx: 0,
            param_pos: 0,
            in_param: false,
            name: name.to_string(),
        });
    }

    pub fn push_macro(&mut self, toks: Vec<Token>, params: Vec<Vec<Token>>, name: &str) {
        self.stack.push(Source::TokList {
            toks,
            pos: 0,
            params,
            param_idx: 0,
            param_pos: 0,
            in_param: false,
            name: name.to_string(),
        });
    }

    pub fn current_file_name(&self) -> String {
        for s in self.stack.iter().rev() {
            if let Source::File { name, .. } = s {
                return name.clone();
            }
        }
        String::new()
    }

    pub fn current_file_line(&self) -> u32 {
        for s in self.stack.iter().rev() {
            if let Source::File { line_no, .. } = s {
                return *line_no;
            }
        }
        0
    }
}
