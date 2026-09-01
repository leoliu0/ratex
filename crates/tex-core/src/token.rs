//! Tokens, control-sequence interning, and catcode tables.

use std::collections::HashMap;

/// Packed token. Char tokens: (cc<<24)|char  (cc<16, char<0x110000 for xetex; bytex char<256).
/// CS tokens: 0x8000_0000 | cs_id.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct Token(pub u32);

pub const EOF_TOKEN: Token = Token(0xFFFF_FFFF);

impl Token {
    #[inline]
    pub fn is_cs(&self) -> bool {
        self.0 >= 0x8000_0000
    }
    #[inline]
    pub fn cs_id(&self) -> u32 {
        // 0x8000_0000 | id  (plain CS) and 0xC000_0000 | id (\\noexpand)
        // must resolve to the same intern id. Keep only the low 30 bits.
        self.0 & 0x3FFF_FFFF
    }
    #[inline]
    pub fn from_cs(cs: u32) -> Token {
        debug_assert!(cs < 0x4000_0000, "cs id overflow");
        Token(0x8000_0000 | cs)
    }
    #[inline]
    pub fn char(cc: u8, c: u32) -> Token {
        debug_assert!(cc < 16);
        Token(((cc as u32) << 24) | c)
    }
    #[inline]
    pub fn is_char(&self) -> bool {
        self.0 < 0x8000_0000
    }
    /// catcode, valid only for char tokens
    #[inline]
    pub fn cc(&self) -> u8 {
        (self.0 >> 24) as u8 & 0xF
    }
    /// character code, valid only for char tokens
    #[inline]
    pub fn chr(&self) -> u32 {
        self.0 & 0x00FF_FFFF
    }
    #[inline]
    pub fn other(c: u8) -> Token {
        Token::char(12, c as u32)
    }
    #[inline]
    pub fn letter(c: u8) -> Token {
        Token::char(11, c as u32)
    }
    #[inline]
    pub fn space() -> Token {
        Token::char(10, 32)
    }
    #[inline]
    pub fn is_space(&self) -> bool {
        self.is_char() && self.cc() == 10
    }
    pub fn is_right_brace(&self) -> bool {
        self.is_char() && self.cc() == 2
    }
    pub fn is_left_brace(&self) -> bool {
        self.is_char() && self.cc() == 1
    }
    /// Drop a one-shot \\noexpand freeze. Knuth's dont_expand lives only in
    /// the input stream; macro bodies must store ordinary CS tokens.
    #[inline]
    pub fn unfreeze(self) -> Token {
        if self.0 >= 0xC000_0000 && self.0 < 0xFFFF_0000 {
            Token::from_cs(self.cs_id())
        } else {
            self
        }
    }

}

pub type CsId = u32;

/// Interning table for control sequence names (byte strings).
pub struct CsTable {
    names: Vec<Vec<u8>>,
    map: HashMap<Vec<u8>, CsId>,
    /// pre-created ids for names needed internally
    pub prim_ids: HashMap<&'static str, CsId>,
}

impl CsTable {
    pub fn new() -> Self {
        CsTable { names: Vec::new(), map: HashMap::new(), prim_ids: HashMap::new() }
    }

    pub fn intern(&mut self, name: &[u8]) -> CsId {
        if let Some(&id) = self.map.get(name) {
            return id;
        }
        let id = self.names.len() as CsId;
        self.names.push(name.to_vec());
        self.map.insert(name.to_vec(), id);
        id
    }

    pub fn lookup(&self, name: &[u8]) -> Option<CsId> {
        self.map.get(name).copied()
    }

    pub fn name(&self, id: CsId) -> &[u8] {
        self.names.get(id as usize).map(|v| v.as_slice()).unwrap_or(b"??")
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn all_ids(&self) -> impl Iterator<Item = CsId> {
        0..self.names.len() as CsId
    }
}

pub const CAT_ESCAPE: u8 = 0;
pub const CAT_BGROUP: u8 = 1;
pub const CAT_EGROUP: u8 = 2;
pub const CAT_MATH: u8 = 3;
pub const CAT_ALIGN: u8 = 4;
pub const CAT_EOL: u8 = 5;
pub const CAT_PARAM: u8 = 6;
pub const CAT_SUPER: u8 = 7;
pub const CAT_SUB: u8 = 8;
pub const CAT_IGNORED: u8 = 9;
pub const CAT_SPACE: u8 = 10;
pub const CAT_LETTER: u8 = 11;
pub const CAT_OTHER: u8 = 12;
pub const CAT_ACTIVE: u8 = 13;
pub const CAT_COMMENT: u8 = 14;
pub const CAT_INVALID: u8 = 15;

/// Catcode table for 8-bit chars.
#[derive(Clone)]
pub struct CatTable(pub [u8; 256]);

impl CatTable {
    /// INITEX defaults (tex.web): letters cat 11, everything else 12,
    /// except backslash 0, null 9, space 10, CR 5, comment 14, delete 15
    pub fn initex() -> Self {
        let mut t = [CAT_OTHER; 256];
        t[b'\\' as usize] = CAT_ESCAPE;
        t[0] = CAT_IGNORED;
        t[b'\r' as usize] = CAT_EOL;
        t[b' ' as usize] = CAT_SPACE;
        t[b'%' as usize] = CAT_COMMENT;
        t[b'^' as usize] = CAT_SUPER;
        t[0x7F] = CAT_INVALID;
        for c in b'a'..=b'z' {
            t[c as usize] = CAT_LETTER;
        }
        for c in b'A'..=b'Z' {
            t[c as usize] = CAT_LETTER;
        }
        CatTable(t)
    }

    pub fn new() -> Self {
        let mut t = [CAT_OTHER; 256];
        for c in b'a'..=b'z' {
            t[c as usize] = CAT_LETTER;
        }
        for c in b'A'..=b'Z' {
            t[c as usize] = CAT_LETTER;
        }
        t[b'\\' as usize] = CAT_ESCAPE;
        t[0x00] = CAT_IGNORED;
        t[b'\t' as usize] = CAT_SPACE;
        t[b' ' as usize] = CAT_SPACE;
        t[b'%' as usize] = CAT_COMMENT;
        t[0x7F] = CAT_INVALID;
        t[b'\r' as usize] = CAT_EOL; // end-of-line char
        t[b'~' as usize] = CAT_ACTIVE;
        t[b'$' as usize] = CAT_MATH;
        t[b'#' as usize] = CAT_PARAM;
        t[b'^' as usize] = CAT_SUPER;
        t[b'_' as usize] = CAT_SUB;
        t[b'&' as usize] = CAT_ALIGN;
        t[b'{' as usize] = CAT_BGROUP;
        t[b'}' as usize] = CAT_EGROUP;
        CatTable(t)
    }
}
