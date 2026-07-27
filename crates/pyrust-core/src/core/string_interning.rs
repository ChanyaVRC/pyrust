use std::cell::RefCell;
use std::collections::HashMap;

use crate::object_model::{STR_INLINE_MAX, Value, make_inline_str};

/// Maximum byte length of a string eligible for interning.
///
/// Covers all identifier strings, most dict keys, and all dunder names
/// while excluding long user-visible strings that are unlikely to repeat.
/// Mirrors Lua 5.4's short-string threshold.
const INTERN_MAX_BYTES: usize = 40;

/// Maximum number of entries in the per-thread intern table.
///
/// Caps memory usage for programs that use many unique short strings
/// (e.g. programs that generate lots of distinct short keys).
const INTERN_MAX_ENTRIES: usize = 1024;

thread_local! {
    /// Per-thread cache mapping short string byte slices to their `Value`.
    ///
    /// Holding a strong `Value` reference is safe: `Value` is NaN-boxed, and
    /// string Values carry an Rc-like refcount in their heap header.  The intern
    /// table keeps exactly one extra reference alive per cached string.  Each
    /// `intern_string` call returns a clone of the cached value (a cheap
    /// refcount bump).
    static INTERN: RefCell<HashMap<Box<str>, Value>> = RefCell::new(HashMap::new());
}

/// Return a `Value::string` for `s`, reusing a cached allocation when `s`
/// is short (≤ [`INTERN_MAX_BYTES`] bytes) and the table has not yet hit
/// [`INTERN_MAX_ENTRIES`].
///
/// **Only call this for immutable, constant-pool strings.**  Never intern
/// strings produced by concatenation, `input()`, or user code — those are
/// not reused and polluting the table wastes memory.
///
/// Strings longer than `INTERN_MAX_BYTES` are not interned (identity is not
/// preserved across loads of the same long constant).  Use
/// [`intern_string_value`] when a pre-built `Value::string` is already
/// available to avoid a redundant allocation on the long-string path.
pub fn intern_string(s: &str) -> Value {
    // Inline (SSO, #2832): ≤ 5-byte strings carry their bytes in the NaN-box —
    // there is no heap allocation to dedup, and identical content already maps
    // to identical bits, so interning is pure overhead.  Skip the table.
    if s.len() <= STR_INLINE_MAX {
        return make_inline_str(s);
    }
    if s.len() > INTERN_MAX_BYTES {
        // Long string: interning would bloat the table without meaningful
        // reuse.  Caller must allocate a fresh Value here.
        return Value::string(s);
    }
    INTERN.with(|cache| {
        let mut map = cache.borrow_mut();
        if let Some(v) = map.get(s) {
            return v.clone();
        }
        let v = Value::string(s);
        if map.len() < INTERN_MAX_ENTRIES {
            map.insert(s.into(), v.clone());
        }
        v
    })
}

/// Like [`intern_string`] but takes a pre-built `Value::string` to avoid
/// a redundant allocation on the long-string fast-exit path.
///
/// - Short strings (≤ [`INTERN_MAX_BYTES`]): looked up / inserted in the
///   intern table; the pre-built value is used as the initial allocation
///   if the string is not yet cached.
/// - Long strings (> `INTERN_MAX_BYTES`): `val` is returned as-is — no
///   new `Value::string` allocation is needed (issue #845).
///
/// **Call site contract**: `val` must already be a `Value::string` whose
/// content equals `s`.  The const-pool `LoadConst` path satisfies this
/// by passing both the borrowed `&str` from `cv.kind()` and `cv` itself.
pub fn intern_string_value(s: &str, val: &Value) -> Value {
    // Inline (SSO, #2832): `val` is already an inline value for ≤ 5 bytes; a
    // clone is a bit-copy and interning would only pollute the table.
    if s.len() <= STR_INLINE_MAX {
        return val.clone();
    }
    if s.len() > INTERN_MAX_BYTES {
        // Long string: not interned; return a cheap clone of the existing
        // const-pool Value rather than allocating a second copy.
        return val.clone();
    }
    INTERN.with(|cache| {
        let mut map = cache.borrow_mut();
        if let Some(v) = map.get(s) {
            return v.clone();
        }
        // Use the caller's pre-built Value as the canonical copy so the
        // const-pool allocation doubles as the intern-table entry.
        let v = val.clone();
        if map.len() < INTERN_MAX_ENTRIES {
            map.insert(s.into(), v.clone());
        }
        v
    })
}
