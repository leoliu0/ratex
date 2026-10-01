//! Tokens, control-sequence interning, and catcode tables.

/// Packed token. Char tokens: (cc<<24)|char  (cc<16, char<0x110000 for xetex; bytex char<256).
/// CS tokens: 0x8000_0000 | cs_id.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Token(pub u32);

impl Token {
    #[inline]
    pub fn is_cs(&self) -> bool {
        self.0 >= 0x8000_0000 && self.0 < 0xFFFF_0000
    }
    #[inline]
    pub fn cs_id(&self) -> u32 {
        // 0x8000_0000 | id (plain CS), 0xC000_0000 | id (\noexpand),
        // and 0xE000_0000 | id (\unexpanded) must all resolve to the same
        // intern id. Mask off bits 31, 30, and 29.
        self.0 & 0x1FFF_FFFF
    }
    #[inline]
    pub fn from_cs(cs: u32) -> Token {
        debug_assert!(cs < 0x4000_0000, "cs id overflow");
        Token(0x8000_0000 | cs)
    }
    #[inline]
    pub fn char(cc: u8, c: u32) -> Token {
        debug_assert!(cc < 16);
        Token(((cc as u32) << 24) | c | if c > 255 { 0x0080_0000 } else { 0 })
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
        self.0 & 0x007F_FFFF
    }
    /// A decoded scalar, distinguished from legacy UTF-8 source bytes.
    #[inline]
    pub fn unicode_char(cc: u8, scalar: u32) -> Token {
        let mut token = Self::char(cc, scalar);
        if scalar > 127 {
            token.0 |= 0x0080_0000;
        }
        token
    }

    #[inline]
    pub fn is_unicode_char(self) -> bool {
        self.is_char() && self.0 & 0x0080_0000 != 0
    }

    pub(crate) fn append_character_bytes(self, bytes: &mut Vec<u8>) {
        if self.is_unicode_char() {
            if let Some(character) = char::from_u32(self.chr()) {
                bytes.extend_from_slice(character.encode_utf8(&mut [0u8; 4]).as_bytes());
            }
        } else {
            bytes.push(self.chr() as u8);
        }
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
    /// Drop one-shot expansion guards. Control sequences use the high-bit
    /// noexpand encoding; parameter characters use bit 28 so their literal
    /// identity survives macro argument scanning without a global counter.
    #[inline]
    pub fn unfreeze(self) -> Token {
        if self.0 >= 0xE000_0000 && self.0 < 0xFFFF_0000 {
            // \unexpanded control-sequence marker: bit 29 is part of the
            // marker, not the interned control-sequence id.
            Token::from_cs(self.0 & 0x1FFF_FFFF)
        } else if self.0 >= 0xC000_0000 && self.0 < 0xE000_0000 {
            Token::from_cs(self.0 & 0x3FFF_FFFF)
        } else if self.0 >= 0x1000_0000 && self.0 < 0x2000_0000 {
            Token(self.0 & !0x1000_0000)
        } else {
            self
        }
    }
}

pub type CsId = u32;

pub const MAX_HASH_NAMES: usize = 2_097_152;

/// FxHash over whole words. Control-sequence names are hashed on every
/// tokenized control word and `\csname`; the generic byte-at-a-time hasher
/// spends a multiply per byte and mixes poorly.
#[derive(Default)]
pub struct NameHasher(u64);

impl std::hash::Hasher for NameHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        const K: u64 = 0x517c_c1b7_2722_0a95;
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            let word = u64::from_le_bytes(chunk.try_into().expect("eight bytes"));
            self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(K);
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut word = [0u8; 8];
            word[..rest.len()].copy_from_slice(rest);
            self.0 = (self.0.rotate_left(5) ^ u64::from_le_bytes(word)).wrapping_mul(K);
        }
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.0 = (self.0.rotate_left(5) ^ i as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

/// No id has been cached for this active character.
const NO_ACTIVE_ID: CsId = CsId::MAX;

/// Interning table for control sequence names (byte strings).
pub struct CsTable {
    names: Vec<Vec<u8>>,
    map: std::collections::HashMap<Vec<u8>, CsId, std::hash::BuildHasherDefault<NameHasher>>,
    capacity_exceeded: bool,
    /// Ids of the 8-bit active characters, filled on first use.
    active_ids: Box<[CsId; 256]>,
    /// One bit per id: the name is an active-character placeholder. Token
    /// list scanners test this per control-sequence token.
    active_names: Vec<u64>,
    /// tex.web frozen control sequences: ids that print with their name but
    /// cannot be reached by name. The value is `true` for pdfTeX's hidden
    /// primitive copies (`prim_eqtb`), which `frozen_lookup` finds by name.
    frozen: std::collections::BTreeMap<CsId, bool>,
    frozen_by_name: std::collections::HashMap<Vec<u8>, CsId, std::hash::BuildHasherDefault<NameHasher>>,
}

impl CsTable {
    pub fn new() -> Self {
        CsTable {
            names: Vec::new(),
            map: Default::default(),
            capacity_exceeded: false,
            active_ids: Box::new([NO_ACTIVE_ID; 256]),
            active_names: Vec::new(),
            frozen: Default::default(),
            frozen_by_name: Default::default(),
        }
    }

    /// A new frozen id named `name`. `by_name` entries are found again by
    /// [`CsTable::frozen_lookup`].
    pub fn push_frozen(&mut self, name: &[u8], by_name: bool) -> CsId {
        let id = self.names.len() as CsId;
        self.names.push(name.to_vec());
        self.frozen.insert(id, by_name);
        if by_name {
            self.frozen_by_name.insert(name.to_vec(), id);
        }
        id
    }

    pub fn frozen_lookup(&self, name: &[u8]) -> Option<CsId> {
        self.frozen_by_name.get(name).copied()
    }

    /// `None` for an ordinary id, else whether it is found by name.
    pub fn frozen_kind(&self, id: CsId) -> Option<bool> {
        self.frozen.get(&id).copied()
    }

    /// The cached id of 8-bit active character `c`, if known.
    #[inline]
    pub(crate) fn cached_active(&self, c: u32) -> Option<CsId> {
        let id = *self.active_ids.get(c as usize)?;
        (id != NO_ACTIVE_ID).then_some(id)
    }

    pub(crate) fn cache_active(&mut self, c: u32, id: CsId) {
        if let Some(slot) = self.active_ids.get_mut(c as usize) {
            *slot = id;
        }
    }

    pub fn intern(&mut self, name: &[u8]) -> CsId {
        self.intern_with_limit(name, MAX_HASH_NAMES)
    }

    fn intern_with_limit(&mut self, name: &[u8], limit: usize) -> CsId {
        if let Some(&id) = self.map.get(name) {
            return id;
        }
        // Main control reports the latched error at its next safe boundary.
        // Until then, reuse the one valid overflow id so an expandable scan
        // cannot allocate an unbounded number of additional names.
        if self.capacity_exceeded {
            return self.names.len().saturating_sub(1) as CsId;
        }
        if self.names.len() >= limit {
            self.capacity_exceeded = true;
        }
        let id = self.names.len() as CsId;
        if crate::engine::Engine::active_cs_scalar(name).is_some() {
            let word = id as usize / 64;
            if self.active_names.len() <= word {
                self.active_names.resize(word + 1, 0);
            }
            self.active_names[word] |= 1 << (id % 64);
        }
        self.names.push(name.to_vec());
        self.map.insert(name.to_vec(), id);
        id
    }

    /// True when `id` names an active character.
    #[inline]
    pub(crate) fn is_active(&self, id: CsId) -> bool {
        self.active_names
            .get(id as usize / 64)
            .is_some_and(|word| word & (1 << (id % 64)) != 0)
    }

    /// The table deliberately accepts the first entry beyond TeX's logical
    /// limit. This keeps the returned id valid until the engine reaches its
    /// next safe diagnostic boundary instead of panicking inside tokenization.
    #[inline]
    pub(crate) fn capacity_exceeded(&self) -> bool {
        self.capacity_exceeded
    }

    #[cfg(test)]
    pub(crate) fn force_capacity_exceeded_for_test(&mut self) {
        self.capacity_exceeded = true;
    }

    pub fn lookup(&self, name: &[u8]) -> Option<CsId> {
        self.map.get(name).copied()
    }

    pub fn name(&self, id: CsId) -> &[u8] {
        self.names
            .get(id as usize)
            .map(|v| v.as_slice())
            .unwrap_or(b"??")
    }
    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn all_ids(&self) -> impl Iterator<Item = CsId> {
        0..self.names.len() as CsId
    }
}

#[cfg(test)]
mod capacity_tests {
    use super::CsTable;

    #[test]
    fn interning_past_a_logical_limit_keeps_the_new_id_valid() {
        let mut table = CsTable::new();
        assert_eq!(table.intern_with_limit(b"first", 1), 0);
        let overflow = table.intern_with_limit(b"second", 1);
        let repeated_overflow = table.intern_with_limit(b"third", 1);

        assert!(table.capacity_exceeded());
        assert_eq!(table.lookup(b"second"), Some(overflow));
        assert_eq!(table.name(overflow), b"second");
        assert_eq!(repeated_overflow, overflow);
        assert_eq!(table.len(), 2);
        assert_eq!(table.lookup(b"third"), None);
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
        t[b'\t' as usize] = CAT_SPACE;
        t[b'%' as usize] = CAT_COMMENT;
        // Knuth tex.web §232: specials ({}, $, &, #, ^, _) are other_char in INITEX
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
