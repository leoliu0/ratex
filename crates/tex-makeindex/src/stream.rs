//! Byte input with makeindex's `mk_getc`, which reads `\r\n` as `\n`.

pub(crate) const EOF: i32 = -1;

pub(crate) struct Stream<'a> {
    bytes: &'a [u8],
    pos: usize,
    /// The byte read after a `\r` that did not start `\r\n`.
    lookahead: Option<i32>,
}

impl<'a> Stream<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Stream { bytes, pos: 0, lookahead: None }
    }

    /// `getc`: the underlying file, bypassing `mk_getc`'s lookahead (as
    /// `fscanf` does).
    pub(crate) fn raw_getc(&mut self) -> i32 {
        match self.bytes.get(self.pos) {
            Some(&byte) => {
                self.pos += 1;
                i32::from(byte)
            }
            None => EOF,
        }
    }

    pub(crate) fn peek_raw(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// `mk_getc`.
    pub(crate) fn getc(&mut self) -> i32 {
        let ch = match self.lookahead.take() {
            Some(ch) => ch,
            None => self.raw_getc(),
        };
        if ch == i32::from(b'\r') {
            let next = self.raw_getc();
            if next == i32::from(b'\n') {
                return next;
            }
            self.lookahead = Some(next);
        }
        ch
    }
}
