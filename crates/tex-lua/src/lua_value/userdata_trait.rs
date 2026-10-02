// Trait-based Userdata system for Lua-rs
//
// Instead of using Lua's traditional metatable-based approach for userdata access,
// we leverage Rust's trait system for direct, type-safe dispatch of field access,
// method calls, and metamethods.
//
// Key design principles:
// 1. Trait-based dispatch (no metatable lookup for known operations)
// 2. Backward compatibility via `as_any()` downcasting
// 3. Metatables still work as fallback for Lua-level customization

use std::any::Any;
use std::fmt;

use crate::lua_vm::CFunction;
use crate::{LuaResult, LuaState, LuaUserdata, LuaValue};

/// A crate-provided native function usable as a userdata method or `__call`.
///
/// Opaque on purpose: native functions operate on raw, unrooted VM values and
/// can only be written inside this crate.
#[derive(Clone, Copy)]
pub struct LuaCFunction(pub(crate) CFunction);

/// Collector callback handed to [`UserDataTrait::trace_lua_values`].
///
/// Only crate-internal userdata types own Lua values; the type is opaque so
/// that host code cannot obtain or forge raw VM values through it.
pub struct LuaValueVisitor<'a>(pub(crate) &'a mut dyn FnMut(LuaValue));

/// Intermediate value type for userdata field/method returns.
///
/// Since `LuaValue` requires GC-allocated strings, trait methods return `UdValue`
/// which the VM converts to proper `LuaValue` (interning strings as needed).
pub enum UdValue {
    Nil,
    Boolean(bool),
    Integer(i64),
    Number(f64),
    /// A Rust string — will be interned by the VM when converting to LuaValue
    Str(String),
    /// An 8-bit clean Lua string (any bytes, not necessarily UTF-8).
    Bytes(Vec<u8>),
    /// A userdata value seen from `set_field`/`set_int_field`: the
    /// [`UserDataTrait::handle_id`] of the assigned userdata.
    Handle(i64),
    /// A light C function — used for returning methods from `get_field`
    Function(LuaCFunction),
    /// Owned userdata value (as return from arithmetic trait methods).
    /// The VM allocates this as a new GC-managed userdata.
    UserdataOwned(Box<dyn UserDataTrait>),
}

impl Clone for UdValue {
    fn clone(&self) -> Self {
        match self {
            UdValue::Nil => UdValue::Nil,
            UdValue::Boolean(b) => UdValue::Boolean(*b),
            UdValue::Integer(i) => UdValue::Integer(*i),
            UdValue::Number(n) => UdValue::Number(*n),
            UdValue::Str(s) => UdValue::Str(s.clone()),
            UdValue::Bytes(b) => UdValue::Bytes(b.clone()),
            UdValue::Handle(h) => UdValue::Handle(*h),
            UdValue::Function(f) => UdValue::Function(*f),
            UdValue::UserdataOwned(_) => UdValue::Nil,
        }
    }
}

impl UdValue {
    #[inline]
    pub fn is_nil(&self) -> bool {
        matches!(self, UdValue::Nil)
    }

    /// Wrap a value that implements `UserDataTrait` into an owned `UdValue`.
    ///
    /// Use this as the return value from arithmetic trait methods when the
    /// result is a new userdata (e.g., `Vec2 + Vec2 → Vec2`).
    #[inline]
    pub fn from_userdata<T: UserDataTrait>(value: T) -> Self {
        UdValue::UserdataOwned(Box::new(value))
    }
}

/// Describes how a userdata type can be accessed from Lua.
///
/// This trait provides rich, typed access to struct fields, methods, and standard
/// operations. Methods are exposed by returning `UdValue::Function(cfunction)`
/// from `get_field`.
///
/// # Dispatch priority (when Lua accesses `obj.key`):
/// 1. `get_field(key)` — field or method access (fields return value, methods return CFunction)
/// 2. Metatable `__index` — traditional Lua fallback
///
/// # Example (manual implementation)
/// ```ignore
/// struct Point { x: f64, y: f64 }
///
/// impl UserDataTrait for Point {
///     fn type_name(&self) -> &'static str { "Point" }
///
///     fn get_field(&self, key: &str) -> Option<UdValue> {
///         match key {
///             "x" => Some(UdValue::Number(self.x)),
///             "y" => Some(UdValue::Number(self.y)),
///             _ => None,
///         }
///     }
///
///     fn set_field(&mut self, key: &str, value: UdValue) -> Option<Result<(), String>> {
///         match key {
///             "x" => match value {
///                 UdValue::Number(n) => { self.x = n; Some(Ok(())) }
///                 _ => Some(Err("x must be a number".into()))
///             }
///             _ => None,
///         }
///     }
///
///     fn as_any(&self) -> &dyn Any { self }
///     fn as_any_mut(&mut self) -> &mut dyn Any { self }
/// }
/// ```
pub trait UserDataTrait: 'static {
    // ==================== Identity ====================

    /// Returns the type name displayed in error messages and `type()` calls.
    fn type_name(&self) -> &'static str;
    /// Registry key of the metatable every userdata created from this value (a
    /// `UserdataOwned` return) gets, when the registry holds a table under it.
    fn metatable_name(&self) -> Option<&'static str> {
        None
    }
    /// Visit embedded Lua values so the collector can retain userdata-owned references.
    fn trace_lua_values(&self, _visit: &mut LuaValueVisitor<'_>) {}

    // ==================== Field Access ====================

    /// Get a field value by name.
    /// Returns `Some(value)` if the field exists, `None` to fall through to metatable.
    fn get_field(&self, _key: &str) -> Option<UdValue> {
        None
    }

    /// Get a field value by integer key (`obj[1]`). `None` falls through to
    /// the metatable.
    fn get_int_field(&self, _key: i64) -> Option<UdValue> {
        None
    }

    /// Set a field value by integer key; same contract as [`Self::set_field`].
    fn set_int_field(&mut self, _key: i64, _value: UdValue) -> Option<Result<(), String>> {
        None
    }

    /// Identity of this userdata for host code that stores handles to host
    /// objects: when `Some(h)`, assigning this userdata through `set_field`
    /// arrives as `UdValue::Handle(h)`.
    fn handle_id(&self) -> Option<i64> {
        None
    }

    /// Set a field value by name.
    /// Returns:
    /// - `Some(Ok(()))` — field was set successfully
    /// - `Some(Err(msg))` — field exists but value is invalid (type mismatch, etc.)
    /// - `None` — field not found, fall through to metatable `__newindex`
    fn set_field(&mut self, _key: &str, _value: UdValue) -> Option<Result<(), String>> {
        None
    }

    // ==================== Metamethods ====================
    // Return None = not supported.

    /// `__tostring`: String representation.
    fn lua_tostring(&self) -> Option<String> {
        None
    }

    /// `__eq`: Equality comparison.
    /// `other` is guaranteed to be a `&dyn UserDataTrait` — downcast via `as_any()`.
    fn lua_eq(&self, _other: &dyn UserDataTrait) -> Option<bool> {
        None
    }

    /// `__lt`: Less-than comparison.
    fn lua_lt(&self, _other: &dyn UserDataTrait) -> Option<bool> {
        None
    }

    /// `__le`: Less-or-equal comparison.
    fn lua_le(&self, _other: &dyn UserDataTrait) -> Option<bool> {
        None
    }

    /// `__len`: Length operator (`#obj`).
    fn lua_len(&self) -> Option<UdValue> {
        None
    }

    /// `__unm`: Unary minus (`-obj`).
    fn lua_unm(&self) -> Option<UdValue> {
        None
    }

    /// `__bnot`: Bitwise NOT (`~obj`).
    fn lua_bnot(&self) -> Option<UdValue> {
        None
    }

    /// `__add`: Addition (`obj + other`).
    fn lua_add(&self, _other: &UdValue) -> Option<UdValue> {
        None
    }

    /// `__sub`: Subtraction (`obj - other`).
    fn lua_sub(&self, _other: &UdValue) -> Option<UdValue> {
        None
    }

    /// `__mul`: Multiplication (`obj * other`).
    fn lua_mul(&self, _other: &UdValue) -> Option<UdValue> {
        None
    }

    /// `__div`: Division (`obj / other`).
    fn lua_div(&self, _other: &UdValue) -> Option<UdValue> {
        None
    }

    /// `__mod`: Modulo (`obj % other`).
    fn lua_mod(&self, _other: &UdValue) -> Option<UdValue> {
        None
    }

    /// `__pow`: Exponentiation (`obj ^ other`).
    fn lua_pow(&self, _other: &UdValue) -> Option<UdValue> {
        None
    }

    /// `__idiv`: Integer division (`obj // other`).
    fn lua_idiv(&self, _other: &UdValue) -> Option<UdValue> {
        None
    }

    /// `__band`: Bitwise AND (`obj & other`).
    fn lua_band(&self, _other: &UdValue) -> Option<UdValue> {
        None
    }

    /// `__bor`: Bitwise OR (`obj | other`).
    fn lua_bor(&self, _other: &UdValue) -> Option<UdValue> {
        None
    }

    /// `__bxor`: Bitwise XOR (`obj ~ other`).
    fn lua_bxor(&self, _other: &UdValue) -> Option<UdValue> {
        None
    }

    /// `__shl`: Left shift (`obj << other`).
    fn lua_shl(&self, _other: &UdValue) -> Option<UdValue> {
        None
    }

    /// `__shr`: Right shift (`obj >> other`).
    fn lua_shr(&self, _other: &UdValue) -> Option<UdValue> {
        None
    }

    /// `__concat`: Concatenation (`obj .. other`).
    fn lua_concat(&self, _other: &UdValue) -> Option<UdValue> {
        None
    }

    /// `__close`: Called when a to-be-closed variable goes out of scope.
    fn lua_close(&mut self) {}

    /// `__call`: Makes the userdata callable like a function.
    ///
    /// Return `Some(cfunction)` to make `obj(args...)` work from Lua.
    /// The CFunction receives `self` (the userdata) as arg 1, followed by
    /// the caller's arguments.
    ///
    /// This is checked before the metatable `__call` fallback.
    fn lua_call(&self) -> Option<LuaCFunction> {
        None
    }

    // ==================== Iteration ====================

    /// Stateless iterator: given the current control variable, return the next
    /// `(control, value)` pair, or `None` to stop.
    ///
    /// This follows Lua's generic-for protocol:
    /// ```lua
    /// for k, v in pairs(ud) do ... end
    /// ```
    ///
    /// The control variable starts as `UdValue::Nil`. Implementors decide
    /// what it represents (e.g., an integer index for sequences).
    ///
    /// # Example (Vec-like)
    /// ```ignore
    /// fn lua_next(&self, control: &UdValue) -> Option<(UdValue, UdValue)> {
    ///     let idx = match control {
    ///         UdValue::Nil => 0,
    ///         UdValue::Integer(i) => *i as usize,
    ///         _ => return None,
    ///     };
    ///     self.items.get(idx).map(|v| (
    ///         UdValue::Integer((idx + 1) as i64),
    ///         UdValue::Integer(*v as i64),
    ///     ))
    /// }
    /// ```
    fn lua_next(&self, _control: &UdValue) -> Option<(UdValue, UdValue)> {
        None
    }

    // ==================== Reflection ====================

    /// List available field names (for debugging, iteration, auto-completion).
    fn field_names(&self) -> &'static [&'static str] {
        &[]
    }

    // ==================== Downcasting ====================

    /// Downcast to `&dyn Any` for type-specific access.
    fn as_any(&self) -> &dyn Any;

    /// Downcast to `&mut dyn Any` for mutable type-specific access.
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

// ==================== UdValue ↔ Rust type conversions ====================

impl From<bool> for UdValue {
    fn from(b: bool) -> Self {
        UdValue::Boolean(b)
    }
}

impl From<i64> for UdValue {
    fn from(i: i64) -> Self {
        UdValue::Integer(i)
    }
}

impl From<i32> for UdValue {
    fn from(i: i32) -> Self {
        UdValue::Integer(i as i64)
    }
}

impl From<f64> for UdValue {
    fn from(n: f64) -> Self {
        UdValue::Number(n)
    }
}

impl From<f32> for UdValue {
    fn from(n: f32) -> Self {
        UdValue::Number(n as f64)
    }
}

impl From<String> for UdValue {
    fn from(s: String) -> Self {
        UdValue::Str(s)
    }
}

impl From<&str> for UdValue {
    fn from(s: &str) -> Self {
        UdValue::Str(s.to_owned())
    }
}

impl<T: Into<UdValue>> From<Option<T>> for UdValue {
    fn from(opt: Option<T>) -> Self {
        match opt {
            Some(v) => v.into(),
            None => UdValue::Nil,
        }
    }
}

// ==================== UdValue → Rust type extraction ====================

impl UdValue {
    /// Extract as bool. Follows Lua truthiness: nil and false are false, everything else is true.
    pub fn to_bool(&self) -> bool {
        match self {
            UdValue::Nil => false,
            UdValue::Boolean(b) => *b,
            _ => true,
        }
    }

    /// Extract as i64; floats convert only when they have an exact integer value.
    pub fn to_integer(&self) -> Option<i64> {
        match self {
            UdValue::Integer(i) => Some(*i),
            UdValue::Number(n) => LuaValue::float(*n).as_integer(),
            _ => None,
        }
    }

    /// Extract as f64 (with optional int→float coercion).
    pub fn to_number(&self) -> Option<f64> {
        match self {
            UdValue::Number(n) => Some(*n),
            UdValue::Integer(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// Extract as string reference.
    pub fn to_str(&self) -> Option<&str> {
        match self {
            UdValue::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

impl fmt::Debug for UdValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UdValue::Nil => write!(f, "Nil"),
            UdValue::Boolean(b) => write!(f, "Boolean({})", b),
            UdValue::Integer(i) => write!(f, "Integer({})", i),
            UdValue::Number(n) => write!(f, "Number({})", n),
            UdValue::Str(s) => write!(f, "Str({:?})", s),
            UdValue::Bytes(b) => write!(f, "Bytes({:?})", b),
            UdValue::Handle(h) => write!(f, "Handle({})", h),
            UdValue::Function(_) => write!(f, "Function(<cfunction>)"),
            UdValue::UserdataOwned(ud) => write!(f, "UserdataOwned({})", ud.type_name()),
        }
    }
}

impl fmt::Display for UdValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UdValue::Nil => write!(f, "nil"),
            UdValue::Boolean(b) => write!(f, "{}", b),
            UdValue::Integer(i) => write!(f, "{}", i),
            UdValue::Number(n) => write!(f, "{}", n),
            UdValue::Str(s) => write!(f, "{}", s),
            UdValue::Bytes(b) => write!(f, "{}", String::from_utf8_lossy(b)),
            UdValue::Handle(h) => write!(f, "userdata:{}", h),
            UdValue::Function(_) => write!(f, "function"),
            UdValue::UserdataOwned(ud) => write!(f, "{}", ud.type_name()),
        }
    }
}

// ==================== UdValue ↔ LuaValue conversion ====================

/// Convert a `UdValue` to a `LuaValue`.
///
/// Most variants are zero-cost. `UdValue::Str` requires GC allocation via `LuaState`.
/// This is the bridge between trait-based dispatch (which returns `UdValue`) and the
/// VM's internal representation (`LuaValue`).
pub fn udvalue_to_lua_value(lua_state: &mut LuaState, udv: UdValue) -> LuaResult<LuaValue> {
    match udv {
        UdValue::Nil => Ok(LuaValue::nil()),
        UdValue::Boolean(b) => Ok(LuaValue::boolean(b)),
        UdValue::Integer(i) => Ok(LuaValue::integer(i)),
        UdValue::Number(n) => Ok(LuaValue::float(n)),
        UdValue::Str(s) => lua_state.create_string(&s),
        UdValue::Bytes(b) => lua_state.create_bytes(&b),
        UdValue::Handle(_) => Ok(LuaValue::nil()),
        UdValue::Function(f) => Ok(LuaValue::cfunction(f.0)),
        UdValue::UserdataOwned(ud) => {
            let meta_name = ud.metatable_name();
            let userdata = LuaUserdata::from_boxed(ud);
            let value = lua_state.create_userdata(userdata)?;
            if let Some(name) = meta_name
                && let Some(meta) = lua_state.global_state_mut().registry_get(name)?
                && meta.as_table_ptr().is_some()
                && let Some(ptr) = value.as_userdata_ptr()
            {
                ptr.as_mut_ref().data.set_metatable(meta);
            }
            Ok(value)
        }
    }
}

/// Convert a `LuaValue` to a `UdValue`.
///
/// Lossless for nil, bool, int, float, string. Other types (table, function, etc.)
/// become `UdValue::Nil` since they can't be represented in the trait world.
pub fn lua_value_to_udvalue(value: &LuaValue) -> UdValue {
    if value.is_nil() {
        UdValue::Nil
    } else if let Some(b) = value.as_boolean() {
        UdValue::Boolean(b)
    } else if let Some(i) = value.as_integer() {
        UdValue::Integer(i)
    } else if let Some(n) = value.as_float() {
        UdValue::Number(n)
    } else if let Some(s) = value.as_str() {
        UdValue::Str(s.to_owned())
    } else if let Some(b) = value.as_bytes() {
        UdValue::Bytes(b.to_vec())
    } else if let Some(h) = value
        .as_userdata_mut()
        .and_then(|ud| ud.get_trait().handle_id())
    {
        UdValue::Handle(h)
    } else {
        UdValue::Nil
    }
}

// ==================== Convenience macro for simple types ====================

/// Implement `UserDataTrait` for types that only need type name and downcast support.
/// These types use metatables for their Lua-visible API (e.g., IO file handles).
///
/// ```ignore
/// impl_simple_userdata!(LuaFile, "FILE*");
/// ```
#[macro_export]
macro_rules! impl_simple_userdata {
    ($ty:ty, $name:expr) => {
        impl $crate::lua_value::userdata_trait::UserDataTrait for $ty {
            fn type_name(&self) -> &'static str {
                $name
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }

            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
        }
    };
}

// ==================== OpaqueUserData ====================

/// Wraps any `T: 'static` as an opaque Lua userdata.
///
/// No fields, methods, or metamethods are exposed — the value is a "black box"
/// in Lua. From Rust you can recover the original type via `downcast_ref::<T>()`.
///
/// Create one with [`LuaApi::create_userdata`](crate::LuaApi::create_userdata).
///
/// # Example
///
/// ```ignore
/// // Third-party type you don't control
/// let client = reqwest::Client::new();
/// let ud = lua.create_userdata(OpaqueUserData::new(client))?;
/// vm.set_global("http_client", ud)?;
///
/// // Later, in a Rust callback:
/// let client = ud_value.downcast_ref::<reqwest::Client>().unwrap();
/// ```
pub struct OpaqueUserData<T: 'static> {
    value: T,
}

impl<T: 'static> OpaqueUserData<T> {
    /// Wrap a value.
    pub fn new(value: T) -> Self {
        OpaqueUserData { value }
    }

    /// Get a reference to the inner value.
    pub fn inner(&self) -> &T {
        &self.value
    }

    /// Get a mutable reference to the inner value.
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl<T: 'static> UserDataTrait for OpaqueUserData<T> {
    fn type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }

    fn as_any(&self) -> &dyn Any {
        // Downcast to T (not OpaqueUserData<T>) for ergonomic access
        &self.value
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        &mut self.value
    }
}
