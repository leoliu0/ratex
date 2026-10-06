//! Large generated tables are embedded as zlib-compressed little-endian blobs
//! and inflated on first use, keeping them out of rustc and the linker.
use std::io::Read;

pub(crate) fn inflate(compressed: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(compressed).read_to_end(&mut out).expect("bundled table is valid zlib data");
    out
}

/// Sequential reader over an inflated blob; generator and reader agree on the layout.
pub(crate) struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self { Reader(data) }
    pub(crate) fn bytes(&mut self, n: usize) -> &'a [u8] {
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        head
    }
    pub(crate) fn u16(&mut self) -> u16 { u16::from_le_bytes(self.bytes(2).try_into().unwrap()) }
    pub(crate) fn u32(&mut self) -> u32 { u32::from_le_bytes(self.bytes(4).try_into().unwrap()) }
    pub(crate) fn len(&mut self) -> usize { self.u32() as usize }
    pub(crate) fn str(&mut self) -> &'a str {
        let n = self.len();
        std::str::from_utf8(self.bytes(n)).expect("bundled table holds UTF-8")
    }
}
