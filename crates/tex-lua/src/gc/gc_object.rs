use crate::{
    LuaProto, LuaRawFunction, LuaRawTable,
    gc::{GcObjectKind, Pooled},
    lua_value::{CClosureFunction, LuaString, LuaUpvalue, LuaUserdata, RClosureFunction},
    lua_vm::LuaState,
};
use std::cell::Cell;

// ============ GC Constants (from Lua 5.5 lgc.h) ============
// Object ages for generational GC
// Uses 3 bits (0-7) - stored in bits 0-2 of marked field
pub const G_NEW: u8 = 0; // Created in current cycle
pub const G_SURVIVAL: u8 = 1; // Created in previous cycle (survived one minor)
pub const G_OLD0: u8 = 2; // Marked old by forward barrier in this cycle
pub const G_OLD1: u8 = 3; // First full cycle as old
pub const G_OLD: u8 = 4; // Really old object (not to be visited in minor)
pub const G_TOUCHED1: u8 = 5; // Old object touched this cycle
pub const G_TOUCHED2: u8 = 6; // Old object touched in previous cycle

// Color bit positions in marked field
pub const WHITE0BIT: u8 = 3; // Object is white (type 0)
pub const WHITE1BIT: u8 = 4; // Object is white (type 1)
pub const BLACKBIT: u8 = 5; // Object is black
pub const FINALIZEDBIT: u8 = 6; // Object has been marked for finalization
pub const SHAREDBIT: u8 = 7; // Object is shared across VMs and never collected

// Bit masks
pub const WHITEBITS: u8 = (1 << WHITE0BIT) | (1 << WHITE1BIT);
pub const AGEBITS: u8 = 0x07; // Mask for age bits (bits 0-2: 0b00000111)
pub const MASKCOLORS: u8 = (1 << BLACKBIT) | WHITEBITS;

/// GC object header - embedded in every GC-managed object
/// Port of Lua 5.5's CommonHeader (lgc.h)
///
/// Compact header layout. Layout:
///
///   `marked: u8` — color + age + finalized/shared bits
///       - Bits 0-2: Age (G_NEW=0 .. G_TOUCHED2=6)
///       - Bit 3: WHITE0
///       - Bit 4: WHITE1
///       - Bit 5: BLACK
///       - Bit 6: FINALIZEDBIT
///       - Bit 7: SHAREDBIT
///   `touched: bool` — Lua 5.3 open-upvalue `touched` flag (see `is_touched`)
///   `_padding: [u8; 2]` — keeps `index` naturally aligned
///   `index: u32` — position in GcList (32-bit, max ~4.29B objects)
///
///   `size: u32` — allocation-time memory size estimate (for GC pacing).
///     Set once at creation, never updated. This ensures consistent
///     accounting between trace_object (allocation) and sweep (deallocation).
///
/// **Tri-color invariant**: Gray is implicit - an object is gray iff it has no white bits AND no black bit.
#[repr(C)]
pub struct GcHeader {
    marked: Cell<u8>,
    touched: Cell<bool>,
    _padding: [u8; 2],
    index: Cell<u32>,
    size: Cell<u32>,
}

impl std::fmt::Debug for GcHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcHeader")
            .field("marked", &self.marked())
            .field("index", &self.index())
            .field("size", &self.size())
            .finish()
    }
}

impl GcHeader {
    const INDEX_MAX: u32 = u32::MAX;

    // ============ Raw field access ============

    #[inline(always)]
    pub fn marked(&self) -> u8 {
        self.marked.get()
    }

    #[inline(always)]
    fn set_marked_bits(&self, m: u8) {
        self.marked.set(m);
    }

    #[inline(always)]
    pub fn index(&self) -> usize {
        self.index.get() as usize
    }

    #[inline(always)]
    pub fn set_index(&self, idx: usize) {
        assert!(
            idx <= Self::INDEX_MAX as usize,
            "GcList index overflow: {idx} > {}",
            Self::INDEX_MAX
        );
        self.index.set(idx as u32);
    }

    #[inline(always)]
    pub fn size(&self) -> u32 {
        self.size.get()
    }

    #[inline(always)]
    pub fn set_size(&self, size: u32) {
        self.size.set(size);
    }

    /// Lua 5.3's `UpVal.u.open.touched` (meaningful for open upvalues only): set on
    /// creation and when a closure traversal outside the atomic phase reaches the upvalue;
    /// `remark_upvalues` marks the values of touched upvalues of unmarked threads once.
    #[inline(always)]
    pub fn is_touched(&self) -> bool {
        self.touched.get()
    }

    #[inline(always)]
    pub fn set_touched(&self, touched: bool) {
        self.touched.set(touched);
    }
}

impl Default for GcHeader {
    fn default() -> Self {
        // WARNING: Default creates a GRAY object (no color bits set)
        // This is INCORRECT for new objects - they should be WHITE
        // Use GcHeader::with_white(current_white) instead when creating GC objects
        GcHeader {
            marked: Cell::new(G_NEW),
            touched: Cell::new(true),
            _padding: [0; 2],
            index: Cell::new(0),
            size: Cell::new(0),
        }
    }
}

impl GcHeader {
    /// Create a new header with given white bit and age G_NEW, index=0
    ///
    /// **CRITICAL**: All new GC objects MUST use this constructor with current_white from GC
    #[inline(always)]
    pub fn with_white(current_white: u8) -> Self {
        debug_assert!(
            current_white == 0 || current_white == 1,
            "current_white must be 0 or 1"
        );
        GcHeader {
            marked: Cell::new((1 << (WHITE0BIT + current_white)) | G_NEW),
            touched: Cell::new(true),
            _padding: [0; 2],
            index: Cell::new(0),
            size: Cell::new(0),
        }
    }

    // ============ Age Operations (generational GC) ============

    /// Get object age (bits 0-2)
    #[inline(always)]
    pub fn age(&self) -> u8 {
        self.marked() & AGEBITS
    }

    /// Set object age (preserves color bits and index)
    #[inline(always)]
    pub fn set_age(&self, age: u8) {
        debug_assert!(age <= G_TOUCHED2, "Invalid age value");
        let m = (self.marked() & !AGEBITS) | (age & AGEBITS);
        self.set_marked_bits(m);
    }

    /// Check if object is old (age > G_SURVIVAL)
    #[inline(always)]
    pub fn is_old(&self) -> bool {
        self.age() > G_SURVIVAL
    }

    // ============ Color Operations (tri-color marking) ============

    #[inline(always)]
    pub fn is_white(&self) -> bool {
        (self.marked() & WHITEBITS) != 0
    }

    #[inline(always)]
    pub fn is_current_white(&self, current_white: u8) -> bool {
        debug_assert!(
            current_white == 0 || current_white == 1,
            "current_white must be 0 or 1"
        );
        (self.marked() & (1 << (WHITE0BIT + current_white))) != 0
    }

    #[inline(always)]
    pub fn is_black(&self) -> bool {
        (self.marked() & (1 << BLACKBIT)) != 0
    }

    #[inline(always)]
    pub fn is_gray(&self) -> bool {
        (self.marked() & (WHITEBITS | (1 << BLACKBIT))) == 0
    }

    // ============ Special Flags ============

    #[inline(always)]
    pub fn to_finalize(&self) -> bool {
        (self.marked() & (1 << FINALIZEDBIT)) != 0
    }

    #[inline(always)]
    pub fn is_shared(&self) -> bool {
        (self.marked() & (1 << SHAREDBIT)) != 0
    }

    #[inline(always)]
    pub fn set_finalized(&self) {
        self.set_marked_bits(self.marked() | (1 << FINALIZEDBIT));
    }

    #[inline(always)]
    pub fn clear_finalized(&self) {
        self.set_marked_bits(self.marked() & !(1 << FINALIZEDBIT));
    }

    #[inline(always)]
    pub fn make_shared(&self) {
        self.set_marked_bits(self.marked() | (1 << SHAREDBIT));
    }

    // ============ Color Transitions ============

    #[inline(always)]
    pub fn make_white(&self, current_white: u8) {
        debug_assert!(
            current_white == 0 || current_white == 1,
            "current_white must be 0 or 1"
        );
        let m = (self.marked() & !MASKCOLORS) | (1 << (WHITE0BIT + current_white));
        self.set_marked_bits(m);
    }

    #[inline(always)]
    pub fn make_gray(&self) {
        self.set_marked_bits(self.marked() & !MASKCOLORS);
    }

    #[inline(always)]
    pub fn make_black(&self) {
        let m = (self.marked() & !WHITEBITS) | (1 << BLACKBIT);
        self.set_marked_bits(m);
    }

    #[inline(always)]
    pub fn nw2black(&self) {
        debug_assert!(!self.is_white(), "nw2black called on white object");
        self.set_marked_bits(self.marked() | (1 << BLACKBIT));
    }

    // ============ Death Detection ============

    #[inline(always)]
    pub fn is_dead(&self, other_white: u8) -> bool {
        debug_assert!(
            other_white == 0 || other_white == 1,
            "other_white must be 0 or 1"
        );
        if self.is_shared() {
            return false;
        }
        (self.marked() & (1 << (WHITE0BIT + other_white))) != 0
    }

    #[inline(always)]
    pub fn otherwhite(current_white: u8) -> u8 {
        current_white ^ 1
    }

    #[inline(always)]
    pub fn change_white(&self) {
        self.set_marked_bits(self.marked() ^ WHITEBITS);
    }

    // ============ Generational GC Age Transitions ============

    #[inline(always)]
    pub fn make_old0(&self) {
        self.set_age(G_OLD0);
    }

    #[inline(always)]
    pub fn make_old1(&self) {
        self.set_age(G_OLD1);
    }

    #[inline(always)]
    pub fn make_old(&self) {
        self.set_age(G_OLD);
    }

    #[inline(always)]
    pub fn make_touched1(&self) {
        self.set_age(G_TOUCHED1);
    }

    #[inline(always)]
    pub fn make_touched2(&self) {
        self.set_age(G_TOUCHED2);
    }

    #[inline(always)]
    pub fn make_survival(&self) {
        self.set_age(G_SURVIVAL);
    }

    // ============ Utility Methods ============

    #[inline(always)]
    pub fn is_marked(&self) -> bool {
        !self.is_white()
    }
}

pub trait HasGcHeader {
    fn header(&self) -> &GcHeader;
}

#[repr(C)]
pub struct Gc<T> {
    pub header: GcHeader,
    pub data: T,
}

impl<T: std::fmt::Debug> std::fmt::Debug for Gc<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gc")
            .field("header", &self.header)
            .field("data", &self.data)
            .finish()
    }
}

impl<T> Gc<T> {
    pub fn new(data: T, current_white: u8, size: u32) -> Self {
        let header = GcHeader::with_white(current_white);
        header.set_size(size);
        Gc { header, data }
    }
}

impl<T> HasGcHeader for Gc<T> {
    fn header(&self) -> &GcHeader {
        &self.header
    }
}

pub type GcString = Gc<LuaString>;
pub type GcTable = Gc<LuaRawTable>;
pub type GcFunction = Gc<LuaRawFunction>;
pub type GcCClosure = Gc<CClosureFunction>;
pub type GcRClosure = Gc<RClosureFunction>;
pub type GcUpvalue = Gc<LuaUpvalue>;
pub type GcThread = Gc<LuaState>;
pub type GcUserdata = Gc<LuaUserdata>;
pub type GcProto = Gc<LuaProto>;

#[derive(Debug)]
pub struct GcPtr<T: HasGcHeader> {
    ptr: *const T,
    _marker: std::marker::PhantomData<*const T>,
}

impl<T: HasGcHeader> std::hash::Hash for GcPtr<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (self.ptr as u64).hash(state);
    }
}

// Manual implementation of Clone and Copy to avoid trait bound requirements on T
// GcPtr is always Copy regardless of T, since it only stores a raw pointer
impl<T: HasGcHeader> Clone for GcPtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: HasGcHeader> Copy for GcPtr<T> {}

impl<T: HasGcHeader> Eq for GcPtr<T> {}

impl<T: HasGcHeader> PartialEq for GcPtr<T> {
    fn eq(&self, other: &Self) -> bool {
        (self.ptr as u64) == (other.ptr as u64)
    }
}

impl<T: HasGcHeader> GcPtr<T> {
    pub fn new(ptr: *const T) -> Self {
        Self {
            ptr,
            _marker: std::marker::PhantomData,
        }
    }

    /// Construct from raw u64 (used by GcObjectPtr tagged-pointer unpacking).
    #[inline(always)]
    pub fn from_raw(raw: u64) -> Self {
        Self {
            ptr: raw as *const T,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn null() -> Self {
        Self {
            ptr: std::ptr::null(),
            _marker: std::marker::PhantomData,
        }
    }

    /// Get the raw u64 pointer value (used by GcObjectPtr tagged-pointer packing)
    #[inline(always)]
    pub fn as_u64(&self) -> u64 {
        self.ptr as u64
    }

    #[inline(always)]
    pub fn as_ptr(&self) -> *const T {
        self.ptr
    }

    #[inline(always)]
    pub fn as_mut_ptr(&self) -> *mut T {
        self.ptr as *mut T
    }

    #[allow(clippy::mut_from_ref)]
    #[inline(always)]
    pub fn as_mut_ref(&self) -> &mut T {
        unsafe { &mut *(self.ptr as *mut T) }
    }

    #[inline(always)]
    pub fn as_ref(&self) -> &T {
        unsafe { &*self.ptr }
    }

    pub fn is_null(&self) -> bool {
        self.ptr.is_null()
    }
}

pub type UpvaluePtr = GcPtr<GcUpvalue>;
pub type TablePtr = GcPtr<GcTable>;
pub type StringPtr = GcPtr<GcString>;
pub type FunctionPtr = GcPtr<GcFunction>;
pub type CClosurePtr = GcPtr<GcCClosure>;
pub type RClosurePtr = GcPtr<GcRClosure>;
pub type UserdataPtr = GcPtr<GcUserdata>;
pub type ThreadPtr = GcPtr<GcThread>;
pub type ProtoPtr = GcPtr<GcProto>;

/// Compressed GcObjectPtr — tagged pointer in a single `u64` (8 bytes, was 16).
///
/// x86-64 user-space pointers use at most 48 bits. We store a 4-bit type tag
/// in bits 60-63, leaving bits 0-47 for the pointer. This is safe because:
/// - Windows user addresses < 0x0000_7FFF_FFFF_FFFF
/// - Linux user addresses < 0x0000_7FFF_FFFF_F000
/// - Tag values 0-8 in bits 60-63 never collide with valid addresses.
///
/// Because all `Gc<T>` are `#[repr(C)]` with `header: GcHeader` at offset 0,
/// `header()` / `header_mut()` are direct pointer casts — no match dispatch.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct GcObjectPtr(u64);

impl std::fmt::Debug for GcObjectPtr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GcObjectPtr({:?}, 0x{:012x})",
            self.kind(),
            self.raw_ptr()
        )
    }
}

impl std::hash::Hash for GcObjectPtr {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl GcObjectPtr {
    const TAG_SHIFT: u32 = 60;
    const PTR_MASK: u64 = (1u64 << 48) - 1; // low 48 bits

    // Tag values — must match GcObjectKind repr(u8)
    pub const TAG_STRING: u64 = 0;
    pub const TAG_TABLE: u64 = 1;
    pub const TAG_FUNCTION: u64 = 2;
    pub const TAG_CCLOSURE: u64 = 3;
    pub const TAG_RCLOSURE: u64 = 4;
    pub const TAG_UPVALUE: u64 = 5;
    pub const TAG_THREAD: u64 = 6;
    pub const TAG_USERDATA: u64 = 7;
    pub const TAG_PROTO: u64 = 8;
    pub const TAG_NONE: u64 = 9;
    #[inline(always)]
    pub fn new_tagged(ptr: u64, tag: u64) -> Self {
        debug_assert!(
            ptr & !Self::PTR_MASK == 0,
            "pointer exceeds 48 bits: 0x{ptr:016x}"
        );
        Self(ptr | (tag << Self::TAG_SHIFT))
    }

    #[inline(always)]
    pub fn null() -> Self {
        Self(0)
    }

    #[inline(always)]
    fn tag(&self) -> u8 {
        (self.0 >> Self::TAG_SHIFT) as u8
    }

    #[inline(always)]
    fn raw_ptr(&self) -> u64 {
        self.0 & Self::PTR_MASK
    }

    // ============ Header access — zero-cost via #[repr(C)] guarantee ============

    /// Access the GcHeader at offset 0 of the pointed-to Gc<T>.
    /// Safe because all Gc<T> are #[repr(C)] with header as first field.
    #[inline(always)]
    pub fn header(&self) -> Option<&GcHeader> {
        let p = self.raw_ptr();
        if p == 0 {
            None
        } else {
            Some(unsafe { &*(p as *const GcHeader) })
        }
    }

    #[inline(always)]
    #[allow(clippy::mut_from_ref)]
    pub fn header_mut(&self) -> Option<&GcHeader> {
        let p = self.raw_ptr();
        if p == 0 {
            None
        } else {
            Some(unsafe { &*(p as *const GcHeader) })
        }
    }

    #[inline(always)]
    pub fn kind(&self) -> GcObjectKind {
        // Safety: tag values 0-8 match repr(u8) of GcObjectKind
        match GcObjectKind::from_u8(self.tag()) {
            Some(k) => k,
            None => unreachable!(),
        }
    }

    #[inline(always)]
    pub(crate) fn index(&self) -> usize {
        // Reads index directly from header
        self.header().map(|h| h.index()).unwrap_or(0)
    }

    pub fn fix_gc_object(&mut self) {
        if let Some(header) = self.header_mut() {
            header.set_age(G_OLD);
            header.make_gray(); // Gray forever, like Lua 5.5
        }
    }

    // ============ Typed pointer extraction ============

    #[inline(always)]
    pub fn as_string_ptr(&self) -> StringPtr {
        debug_assert!(self.tag() == Self::TAG_STRING as u8);
        StringPtr::from_raw(self.raw_ptr())
    }

    #[inline(always)]
    pub fn as_table_ptr(&self) -> TablePtr {
        debug_assert!(self.tag() == Self::TAG_TABLE as u8);
        TablePtr::from_raw(self.raw_ptr())
    }

    #[inline(always)]
    pub fn as_function_ptr(&self) -> FunctionPtr {
        debug_assert!(self.tag() == Self::TAG_FUNCTION as u8);
        FunctionPtr::from_raw(self.raw_ptr())
    }

    #[inline(always)]
    pub fn as_cclosure_ptr(&self) -> CClosurePtr {
        debug_assert!(self.tag() == Self::TAG_CCLOSURE as u8);
        CClosurePtr::from_raw(self.raw_ptr())
    }

    #[inline(always)]
    pub fn as_rclosure_ptr(&self) -> RClosurePtr {
        debug_assert!(self.tag() == Self::TAG_RCLOSURE as u8);
        RClosurePtr::from_raw(self.raw_ptr())
    }

    #[inline(always)]
    pub fn as_upvalue_ptr(&self) -> UpvaluePtr {
        debug_assert!(self.tag() == Self::TAG_UPVALUE as u8);
        UpvaluePtr::from_raw(self.raw_ptr())
    }

    #[inline(always)]
    pub fn as_thread_ptr(&self) -> ThreadPtr {
        debug_assert!(self.tag() == Self::TAG_THREAD as u8);
        ThreadPtr::from_raw(self.raw_ptr())
    }

    #[inline(always)]
    pub fn as_userdata_ptr(&self) -> UserdataPtr {
        debug_assert!(self.tag() == Self::TAG_USERDATA as u8);
        UserdataPtr::from_raw(self.raw_ptr())
    }

    #[inline(always)]
    pub fn as_proto_ptr(&self) -> ProtoPtr {
        debug_assert!(self.tag() == Self::TAG_PROTO as u8);
        ProtoPtr::from_raw(self.raw_ptr())
    }

    // ============ Pattern matching helpers (for code that still uses if-let) ============

    #[inline(always)]
    pub fn is_string(&self) -> bool {
        self.tag() == Self::TAG_STRING as u8
    }

    #[inline(always)]
    pub fn is_table(&self) -> bool {
        self.tag() == Self::TAG_TABLE as u8
    }

    #[inline(always)]
    pub fn is_upvalue(&self) -> bool {
        self.tag() == Self::TAG_UPVALUE as u8
    }

    #[inline(always)]
    pub fn is_thread(&self) -> bool {
        self.tag() == Self::TAG_THREAD as u8
    }

    #[inline(always)]
    pub fn is_function(&self) -> bool {
        self.tag() == Self::TAG_FUNCTION as u8
    }

    #[inline(always)]
    pub fn is_cclosure(&self) -> bool {
        self.tag() == Self::TAG_CCLOSURE as u8
    }

    #[inline(always)]
    pub fn is_rclosure(&self) -> bool {
        self.tag() == Self::TAG_RCLOSURE as u8
    }

    #[inline(always)]
    pub fn is_userdata(&self) -> bool {
        self.tag() == Self::TAG_USERDATA as u8
    }

    #[inline(always)]
    pub fn is_proto(&self) -> bool {
        self.tag() == Self::TAG_PROTO as u8
    }
}

impl From<StringPtr> for GcObjectPtr {
    #[inline(always)]
    fn from(ptr: StringPtr) -> Self {
        Self::new_tagged(ptr.as_u64(), Self::TAG_STRING)
    }
}

impl From<TablePtr> for GcObjectPtr {
    #[inline(always)]
    fn from(ptr: TablePtr) -> Self {
        Self::new_tagged(ptr.as_u64(), Self::TAG_TABLE)
    }
}

impl From<FunctionPtr> for GcObjectPtr {
    #[inline(always)]
    fn from(ptr: FunctionPtr) -> Self {
        Self::new_tagged(ptr.as_u64(), Self::TAG_FUNCTION)
    }
}

impl From<UpvaluePtr> for GcObjectPtr {
    #[inline(always)]
    fn from(ptr: UpvaluePtr) -> Self {
        Self::new_tagged(ptr.as_u64(), Self::TAG_UPVALUE)
    }
}

impl From<ThreadPtr> for GcObjectPtr {
    #[inline(always)]
    fn from(ptr: ThreadPtr) -> Self {
        Self::new_tagged(ptr.as_u64(), Self::TAG_THREAD)
    }
}

impl From<UserdataPtr> for GcObjectPtr {
    #[inline(always)]
    fn from(ptr: UserdataPtr) -> Self {
        Self::new_tagged(ptr.as_u64(), Self::TAG_USERDATA)
    }
}

impl From<CClosurePtr> for GcObjectPtr {
    #[inline(always)]
    fn from(ptr: CClosurePtr) -> Self {
        Self::new_tagged(ptr.as_u64(), Self::TAG_CCLOSURE)
    }
}

impl From<RClosurePtr> for GcObjectPtr {
    #[inline(always)]
    fn from(ptr: RClosurePtr) -> Self {
        Self::new_tagged(ptr.as_u64(), Self::TAG_RCLOSURE)
    }
}

impl From<ProtoPtr> for GcObjectPtr {
    #[inline(always)]
    fn from(ptr: ProtoPtr) -> Self {
        Self::new_tagged(ptr.as_u64(), Self::TAG_PROTO)
    }
}

// ============ GC-managed Objects ============
/// Owns one GC allocation and, for a slot allocation, its exact strong pool Rc.
/// Only typed conversions can couple the allocation kind with its pool type.
pub struct GcObjectOwner {
    pointer: GcObjectPtr,
    pool: *const (),
}

macro_rules! pooled_gc_owner {
    ($object:ty, $pointer:ident) => {
        impl From<Pooled<$object>> for GcObjectOwner {
            fn from(value: Pooled<$object>) -> Self {
                // Validate pointer tagging while the typed allocation still
                // owns its destruction if a debug assertion fails.
                let pointer = GcObjectPtr::from($pointer::new(value.as_ptr()));
                let (value, pool) = value.into_gc_parts();
                Self {
                    pointer: GcObjectPtr::new_tagged(value as u64, pointer.tag() as u64),
                    pool,
                }
            }
        }
    };
}

pooled_gc_owner!(GcString, StringPtr);
pooled_gc_owner!(GcTable, TablePtr);
pooled_gc_owner!(GcFunction, FunctionPtr);
pooled_gc_owner!(GcUpvalue, UpvaluePtr);
pooled_gc_owner!(GcUserdata, UserdataPtr);
pooled_gc_owner!(GcCClosure, CClosurePtr);
pooled_gc_owner!(GcRClosure, RClosurePtr);
pooled_gc_owner!(GcProto, ProtoPtr);

impl From<Box<GcThread>> for GcObjectOwner {
    fn from(value: Box<GcThread>) -> Self {
        let pointer = GcObjectPtr::from(ThreadPtr::new(value.as_ref() as *const GcThread));
        let value = Box::into_raw(value);
        Self {
            pointer: GcObjectPtr::new_tagged(value as u64, pointer.tag() as u64),
            pool: std::ptr::null(),
        }
    }
}

impl GcObjectOwner {
    /// Return the stored allocation-time size (from header.size).
    #[inline]
    pub fn size(&self) -> usize {
        self.header().size() as usize
    }

    #[inline(always)]
    fn raw_header_ptr(&self) -> *mut GcHeader {
        self.pointer.raw_ptr() as *mut GcHeader
    }

    pub fn header(&self) -> &GcHeader {
        unsafe { &*self.raw_header_ptr() }
    }

    pub fn header_mut(&self) -> &GcHeader {
        unsafe { &*self.raw_header_ptr() }
    }

    #[inline(always)]
    pub fn as_str_ptr(&self) -> Option<StringPtr> {
        self.pointer.is_string().then(|| self.pointer.as_string_ptr())
    }

    pub fn as_table_ptr(&self) -> Option<TablePtr> {
        self.pointer.is_table().then(|| self.pointer.as_table_ptr())
    }

    pub fn as_function_ptr(&self) -> Option<FunctionPtr> {
        self.pointer.is_function().then(|| self.pointer.as_function_ptr())
    }

    pub fn as_upvalue_ptr(&self) -> Option<UpvaluePtr> {
        self.pointer.is_upvalue().then(|| self.pointer.as_upvalue_ptr())
    }

    pub fn as_thread_ptr(&self) -> Option<ThreadPtr> {
        self.pointer.is_thread().then(|| self.pointer.as_thread_ptr())
    }

    pub fn as_userdata_ptr(&self) -> Option<UserdataPtr> {
        self.pointer.is_userdata().then(|| self.pointer.as_userdata_ptr())
    }

    pub fn as_closure_ptr(&self) -> Option<CClosurePtr> {
        self.pointer.is_cclosure().then(|| self.pointer.as_cclosure_ptr())
    }

    pub fn as_rclosure_ptr(&self) -> Option<RClosurePtr> {
        self.pointer.is_rclosure().then(|| self.pointer.as_rclosure_ptr())
    }

    pub fn as_proto_ptr(&self) -> Option<ProtoPtr> {
        self.pointer.is_proto().then(|| self.pointer.as_proto_ptr())
    }

    pub fn as_gc_ptr(&self) -> GcObjectPtr {
        self.pointer
    }

    pub fn as_thread_mut(&mut self) -> Option<&mut LuaState> {
        if !self.pointer.is_thread() {
            return None;
        }
        // The allocation remains owned here, and the exclusive owner borrow
        // provides the same mutable access as the former Box variant.
        Some(unsafe { &mut (*self.pointer.as_thread_ptr().as_mut_ptr()).data })
    }

    pub fn size_of_data(&self) -> usize {
        self.header().size() as usize
    }
}

impl Drop for GcObjectOwner {
    fn drop(&mut self) {
        // SAFETY: the private typed From implementations couple this pointer's
        // kind with its concrete allocation and Rc type. Each consumes its
        // allocation once; this non-Copy owner reconstructs it exactly once.
        // Pooled's existing drop still releases slots, and boxed feature paths
        // and threads still drop their original Boxes.
        unsafe {
            match self.pointer.kind() {
                GcObjectKind::String => drop(Pooled::<GcString>::from_gc_parts(self.pointer.as_string_ptr().as_mut_ptr(), self.pool)),
                GcObjectKind::Table => drop(Pooled::<GcTable>::from_gc_parts(self.pointer.as_table_ptr().as_mut_ptr(), self.pool)),
                GcObjectKind::Function => drop(Pooled::<GcFunction>::from_gc_parts(self.pointer.as_function_ptr().as_mut_ptr(), self.pool)),
                GcObjectKind::Upvalue => drop(Pooled::<GcUpvalue>::from_gc_parts(self.pointer.as_upvalue_ptr().as_mut_ptr(), self.pool)),
                GcObjectKind::Userdata => drop(Pooled::<GcUserdata>::from_gc_parts(self.pointer.as_userdata_ptr().as_mut_ptr(), self.pool)),
                GcObjectKind::CClosure => drop(Pooled::<GcCClosure>::from_gc_parts(self.pointer.as_cclosure_ptr().as_mut_ptr(), self.pool)),
                GcObjectKind::RClosure => drop(Pooled::<GcRClosure>::from_gc_parts(self.pointer.as_rclosure_ptr().as_mut_ptr(), self.pool)),
                GcObjectKind::Proto => drop(Pooled::<GcProto>::from_gc_parts(self.pointer.as_proto_ptr().as_mut_ptr(), self.pool)),
                GcObjectKind::Thread => drop(Box::from_raw(self.pointer.as_thread_ptr().as_mut_ptr())),
            }
        }
    }
}

/// High-performance Vec-based pool for GC objects
/// - O(1) allocation: direct push to Vec, returns GcPtr
/// - O(1) deallocation: swap_remove using tracked pool_index  
/// - O(live_objects) iteration: always compact, no holes!
/// - No free_list needed: objects are truly removed via swap_remove
/// - GcPtr-based: external references use pointers, not indices
pub struct GcList {
    gc_list: Vec<GcObjectOwner>,
}

#[allow(unused)]
impl GcList {
    #[inline]
    pub fn new() -> Self {
        Self {
            gc_list: Vec::new(),
        }
    }

    #[inline]
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            gc_list: Vec::with_capacity(cap),
        }
    }

    /// Allocate a new object and return a GcPtr to it
    /// O(1) allocation: push to Vec, track index in header, return pointer to Box contents
    #[inline]
    pub fn add(&mut self, mut value: GcObjectOwner) {
        let index = self.gc_list.len();
        value.header_mut().set_index(index);
        self.gc_list.push(value);
    }

    /// Free an object using its pointer
    /// O(1) via swap_remove: moves last object to removed position, updates its index
    #[inline]
    pub fn remove(&mut self, gc_ptr: GcObjectPtr) -> GcObjectOwner {
        let index = gc_ptr.index();
        let last_index = self.gc_list.len() - 1;
        if index != last_index {
            // Update moved object's index
            let moved_obj = &self.gc_list[last_index];
            moved_obj
                .as_gc_ptr()
                .header_mut()
                .expect("moved object must have a valid GC header")
                .set_index(index);
        }

        // swap_remove: O(1) removal by moving last element to this position
        self.gc_list.swap_remove(index)
    }

    /// Current number of live objects (always equals Vec length, no holes!)
    #[inline]
    pub fn len(&self) -> usize {
        self.gc_list.len()
    }

    /// Check if pool is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.gc_list.is_empty()
    }

    /// Iterate over all live objects (always compact, O(live_objects))
    pub fn iter(&self) -> impl Iterator<Item = &GcObjectOwner> + '_ {
        self.gc_list.iter()
    }

    /// Iterate over all live objects mutably
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut GcObjectOwner> + '_ {
        self.gc_list.iter_mut()
    }

    /// Shrink internal storage to fit current objects
    pub fn shrink_to_fit(&mut self) {
        self.gc_list.shrink_to_fit();
    }

    /// Clear all objects
    pub fn clear(&mut self) {
        self.gc_list.clear();
    }

    /// Get Vec capacity (for diagnostics)
    #[inline]
    pub fn capacity(&self) -> usize {
        self.gc_list.capacity()
    }

    pub fn get(&self, index: usize) -> Option<&GcObjectOwner> {
        self.gc_list.get(index)
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut GcObjectOwner> {
        self.gc_list.get_mut(index)
    }

    pub fn iter_ptrs(&self) -> impl Iterator<Item = GcObjectPtr> + '_ {
        self.gc_list.iter().map(|obj| obj.as_gc_ptr())
    }

    /// Check if an object is in this list by checking its index
    /// O(1) check using the object's stored index
    #[inline]
    pub fn contains(&self, gc_ptr: GcObjectPtr) -> bool {
        let index = gc_ptr.index();
        if index < self.gc_list.len() {
            // Verify it's actually the same object (not just same index)
            self.gc_list[index].as_gc_ptr() == gc_ptr
        } else {
            false
        }
    }

    /// Try to remove an object, returning Some(owner) if found, None otherwise
    #[inline]
    pub fn try_remove(&mut self, gc_ptr: GcObjectPtr) -> Option<GcObjectOwner> {
        if self.contains(gc_ptr) {
            Some(self.remove(gc_ptr))
        } else {
            None
        }
    }

    /// Get GcObjectOwner by index (for iteration with ownership)
    /// This method panics if index is out of bounds
    #[inline]
    pub fn get_owner(&self, index: usize) -> &GcObjectOwner {
        &self.gc_list[index]
    }

    /// Take all objects out and return as Vec, leaving self empty
    #[inline]
    pub fn take_all(&mut self) -> Vec<GcObjectOwner> {
        std::mem::take(&mut self.gc_list)
    }

    /// Add multiple objects (used when moving between generation lists)
    #[inline]
    pub fn add_all(&mut self, objects: Vec<GcObjectOwner>) {
        for obj in objects {
            self.add(obj);
        }
    }
}

impl Default for GcList {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua_value::userdata_trait::OpaqueUserData;
    use std::rc::Rc;

    struct OwnedResource {
        drops: Rc<Cell<usize>>,
        value: u32,
    }

    impl Drop for OwnedResource {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    fn owned_userdata(drops: &Rc<Cell<usize>>, value: u32) -> GcUserdata {
        GcUserdata::new(
            LuaUserdata::new(OpaqueUserData::new(OwnedResource {
                drops: Rc::clone(drops),
                value,
            })),
            0,
            std::mem::size_of::<LuaUserdata>() as u32,
        )
    }

    #[test]
    fn gc_owner_reuses_released_slots_and_outlives_its_pool() {
        let drops = Rc::new(Cell::new(0));
        let mut pool = crate::gc::PagedPool::new(1);
        let first = pool.alloc(owned_userdata(&drops, 17));
        #[cfg(not(any(miri, tex_lua_boxed_pool)))]
        let first_ptr = first.as_ptr();
        drop(GcObjectOwner::from(first));
        assert_eq!(drops.get(), 1);

        let reused = pool.alloc(owned_userdata(&drops, 23));
        #[cfg(not(any(miri, tex_lua_boxed_pool)))]
        assert_eq!(first_ptr, reused.as_ptr());
        let owner = GcObjectOwner::from(reused);
        drop(pool);
        let pointer = owner.as_userdata_ptr().unwrap();
        let resource = pointer.as_ref().data.get_trait().as_any().downcast_ref::<OwnedResource>().unwrap();
        assert_eq!(resource.value, 23);
        assert_eq!(drops.get(), 1);
        drop(owner);
        assert_eq!(drops.get(), 2);
    }

    #[cfg(any(miri, tex_lua_boxed_pool, feature = "shared-proto"))]
    #[test]
    fn boxed_gc_owner_preserves_payload_and_drops_once() {
        let drops = Rc::new(Cell::new(0));
        let owner = GcObjectOwner::from(Pooled::boxed(owned_userdata(&drops, 31)));
        let pointer = owner.as_userdata_ptr().unwrap();
        let resource = pointer.as_ref().data.get_trait().as_any().downcast_ref::<OwnedResource>().unwrap();
        assert_eq!(resource.value, 31);
        assert_eq!(drops.get(), 0);
        drop(owner);
        assert_eq!(drops.get(), 1);
    }


    #[test]
    fn gc_header_large_index_round_trip() {
        let header = GcHeader::with_white(0);
        let idx = (1usize << 24) + 123;
        header.set_index(idx);
        assert_eq!(header.index(), idx);
    }
}
