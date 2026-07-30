//! `bytearray` built-in type.
//!
//! `bytearray` is the mutable counterpart to `bytes`.  It shares all of
//! `bytes`'s read methods and additionally supports item/slice assignment,
//! `append`, `extend`, `insert`, `pop`, `remove`, `reverse`, `clear`, and
//! `copy`.  Internally backed by `Rc<RefCell<Vec<u8>>>` so that clones share
//! mutable state (Python reference semantics: `b = a; b.append(1)` mutates `a`
//! too).

mod indexing;
mod read_ops;
mod repr;
mod storage;
#[cfg(test)]
mod tests;

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::{BuiltinState, BuiltinTypeOps, PyDict, PyError, PyKey, Result, Value, ValueKind};

use indexing::{
    bytes_from_value, resolve_slice_indices, slice_indices, value_to_byte, value_to_index,
};
use read_ops::{
    bytearray_join, bytearray_partition, bytes_capitalize, bytes_list_to_bytearray_list,
    bytes_title, bytes_val_to_bytearray,
};
use repr::bytearray_bytes_repr;
use storage::call_storage_method;

use crate::method_signature::{KeywordPolicy, PositionalArity};

pub const TYPE_NAME: &str = "bytearray";
pub const BYTEARRAY_OPS: &ByteArrayOps = &ByteArrayOps;

/// Canonical list of method names dispatched by `call_method`.
/// Kept in sync with the `match method` in `ByteArrayOps::call_method`.
pub const METHODS: &[&str] = &[
    "__iter__",
    // Shared read methods (same semantics as bytes)
    "hex",
    "decode",
    "startswith",
    "endswith",
    "find",
    "rfind",
    "index",
    "rindex",
    "count",
    "upper",
    "lower",
    "replace",
    "strip",
    "lstrip",
    "rstrip",
    "removeprefix",
    "removesuffix",
    "split",
    "rsplit",
    "splitlines",
    "join",
    "title",
    "capitalize",
    "isdigit",
    "isalpha",
    "isalnum",
    "isupper",
    "islower",
    "isspace",
    "center",
    "ljust",
    "rjust",
    "zfill",
    "translate",
    "partition",
    "rpartition",
    "swapcase",
    "isascii",
    "istitle",
    "expandtabs",
    // Mutable-only methods
    "append",
    "extend",
    "insert",
    "pop",
    "remove",
    "reverse",
    "clear",
    "copy",
    // classmethod — exposed as attribute on the type class, not on instances
    "fromhex",
];

pub const CLASS_ATTRS: crate::primitive_class_attrs::PrimitiveClassAttrs =
    crate::primitive_class_attrs::PrimitiveClassAttrs::new(TYPE_NAME, METHODS)
        .with_native_class_methods(&["fromhex"])
        .with_native_static_methods(&["maketrans"]);

/// Returns `true` if `method` is exposed by `bytearray`.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

/// Positional signature for every public bytearray method.
///
/// Read-only operations deliberately reuse the bytes signature table, while
/// the owner passed to diagnostics remains `bytearray`.
pub fn positional_arity(method: &str) -> Option<PositionalArity> {
    match method {
        "append" | "extend" | "remove" => Some(PositionalArity::exact(1)),
        "insert" => Some(PositionalArity::exact(2)),
        "pop" => Some(PositionalArity::range(0, 1)),
        "reverse" | "clear" | "copy" => Some(PositionalArity::exact(0)),
        // Shared read methods plus fromhex/maketrans use bytes-compatible
        // signatures. bytearray intentionally does not expose
        // bytes.__getnewargs__.
        "__getnewargs__" => None,
        _ => crate::bytes::positional_arity(method),
    }
}

#[inline]
pub fn validate_method_positional_arity(method: &str, given: usize) -> Result<()> {
    if given == 0 {
        return Ok(());
    }
    match positional_arity(method) {
        Some(arity) => arity.reject_excess(TYPE_NAME, method, given),
        None => Ok(()),
    }
}

pub fn keyword_policy(method: &str) -> Option<KeywordPolicy> {
    match method {
        "append" | "extend" | "insert" | "pop" | "remove" | "reverse" | "clear" | "copy" => {
            Some(KeywordPolicy::Reject)
        }
        "__getnewargs__" => None,
        _ => crate::bytes::keyword_policy(method),
    }
}

#[inline]
pub fn validate_method_keywords(method: &str, has_keywords: bool) -> Result<()> {
    if !has_keywords {
        return Ok(());
    }
    match keyword_policy(method) {
        Some(policy) => policy.validate(TYPE_NAME, method, true),
        None => Ok(()),
    }
}

/// Internal bytearray state.  `Rc<RefCell<Vec<u8>>>` so that cloning a
/// `Value::bytearray` shares the backing storage (Python reference semantics).
pub struct ByteArrayState {
    pub data: Rc<RefCell<Vec<u8>>>,
}

pub struct ByteArrayOps;

impl BuiltinTypeOps for ByteArrayOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn canonical_class_tag(&self) -> Option<pyrust_core::CanonicalClassTag> {
        Some(pyrust_core::CanonicalClassTag::Bytearray)
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<ByteArrayState>()
            .expect("bytearray state");
        let data = s.data.borrow();
        format!("bytearray({})", bytearray_bytes_repr(&data))
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<ByteArrayState>()
            .expect("bytearray state");
        !s.data.borrow().is_empty()
    }

    /// `copy.copy(bytearray(...))` — a new bytearray over its own buffer.
    /// The payload is bytes, so there is nothing for `deepcopy` to recurse
    /// into and the same copy serves both.
    fn copy_storage(&self, state: &BuiltinState) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<ByteArrayState>()?;
        let bytes = s.data.borrow().clone();
        Some(bytearray(bytes))
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<ByteArrayState>()
            .expect("bytearray state");
        let lhs = s.data.borrow();
        match other.kind() {
            ValueKind::Bytes(rc) => lhs.as_slice() == rc.as_slice(),
            ValueKind::BuiltinObject {
                ops,
                state: rhs_state,
            } if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) => {
                let rhs_borrow = rhs_state.borrow();
                let rhs = rhs_borrow
                    .downcast_ref::<ByteArrayState>()
                    .expect("bytearray state");
                let rhs_data = rhs.data.borrow();
                Rc::ptr_eq(&s.data, &rhs.data) || lhs.as_slice() == rhs_data.as_slice()
            }
            _ => false,
        }
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<ByteArrayState>()
            .expect("bytearray state");
        Some(s.data.borrow().len())
    }

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<ByteArrayState>()
            .expect("bytearray state");
        let data = s.data.borrow();
        // CPython: `n in bytearray` requires n to be an integer 0..255,
        // OR a bytes-like object (sub-sequence search).
        match item.kind() {
            ValueKind::Int(n) => {
                if !(0..=255).contains(&n) {
                    return Err(PyError::named(
                        "ValueError",
                        "byte must be in range(0, 256)".to_string(),
                    ));
                }
                Ok(data.contains(&(n as u8)))
            }
            ValueKind::Bool(b) => Ok(data.contains(&(b as u8))),
            ValueKind::BigInt(_) => Err(PyError::named(
                "ValueError",
                "byte must be in range(0, 256)".to_string(),
            )),
            ValueKind::Bytes(rc) => {
                Ok(crate::bytes::find_subsequence(&data, rc.as_slice()).is_some())
            }
            ValueKind::BuiltinObject {
                ops,
                state: item_state,
            } if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) => {
                let ib = item_state.borrow();
                let item_s = ib
                    .downcast_ref::<ByteArrayState>()
                    .expect("bytearray state");
                let item_data = item_s.data.borrow();
                Ok(crate::bytes::find_subsequence(&data, &item_data).is_some())
            }
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(item)
                ),
            )),
        }
    }

    fn is_iterable(&self) -> bool {
        true
    }

    /// Each call to `iter_next` advances an internal position stored in the
    /// state.  We model the iterator state as the `ByteArrayState` itself with
    /// a separate position tracked via a second `usize` field.  Since
    /// `BuiltinTypeOps::iter_next` only has access to the state (no separate
    /// cursor), we materialise the full iteration up front: pyrust's
    /// `iter_values` path collects everything via `iter_next` for
    /// `is_iterable() == true` objects, but `bytearray` is also handled
    /// specially in `iter_values` (see `expr.rs`).
    ///
    /// For the `iter_next` path (used when the object is wrapped in a
    /// `NativeIterFrame` by `__iter__`), we return the elements one at a time
    /// by maintaining the position in the state.  However, `ByteArrayState`
    /// does not store a cursor.  To keep the design simple, we do not implement
    /// `iter_next` here; instead, `bytearray` is handled in the `iter_values`
    /// fast path (analogous to frozenset/dict-views).  The `is_iterable()`
    /// flag tells the VM to try `iter_next`, so we must implement it — we
    /// delegate by materialising the full slice.
    fn iter_next(&self, _state: &BuiltinState) -> Result<Option<Value>> {
        // This impl is not called: bytearray is intercepted in `iter_values`.
        // Return None to signal exhaustion if somehow called directly.
        Ok(None)
    }

    fn get_item(&self, state: &BuiltinState, key: &Value) -> Result<Value> {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<ByteArrayState>()
            .expect("bytearray state");
        let data = s.data.borrow();
        // Slice subscript: key is a slice object.
        if let ValueKind::BuiltinObject {
            ops,
            state: slice_state,
        } = key.kind()
            && crate::slice::is_slice_ops(ops)
        {
            let sb = slice_state.borrow();
            let sl = sb
                .downcast_ref::<crate::slice::SliceState>()
                .expect("slice state");
            let len = data.len() as i64;
            let (start, stop, step) = resolve_slice_indices(len, &sl.start, &sl.stop, &sl.step)?;
            let result: Vec<u8> = slice_indices(start, stop, step)
                .filter_map(|i| data.get(i).copied())
                .collect();
            drop(sb);
            return Ok(bytearray(result));
        }
        // Integer subscript.
        let idx = value_to_index(key, data.len(), "bytearray")?;
        Ok(Value::int(data[idx] as i64))
    }

    fn set_item(&self, state: &BuiltinState, key: &Value, value: Value) -> Result<()> {
        let data_rc = {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<ByteArrayState>()
                .expect("bytearray state");
            Rc::clone(&s.data)
        };
        // Slice assignment.
        if let ValueKind::BuiltinObject {
            ops,
            state: slice_state,
        } = key.kind()
            && crate::slice::is_slice_ops(ops)
        {
            let sb = slice_state.borrow();
            let sl = sb
                .downcast_ref::<crate::slice::SliceState>()
                .expect("slice state");
            let len = data_rc.borrow().len() as i64;
            let (start, stop, step) = resolve_slice_indices(len, &sl.start, &sl.stop, &sl.step)?;
            drop(sb);
            // Materialise the complete RHS before taking the mutable borrow.
            // Besides keeping iterator/pre-pass work outside the mutation
            // critical section, this is required for aliasing assignments such
            // as `data[:] = data`: `bytes_from_value` must be able to read the
            // same RefCell without colliding with our eventual write borrow.
            let replacement = bytes_from_value(&value, "bytearray slice assignment")?;
            let mut data = data_rc.borrow_mut();
            if step == 1 {
                // Simple slice: replace range [start, stop) with replacement.
                // For step == 1 both bounds are forward, clamped to [0, len].
                // An empty/reversed range (start > stop) inserts at `start`,
                // matching CPython (e.g. `ba[5:2] = b"XY"` inserts at 5).
                let s2 = (start.max(0) as usize).min(data.len());
                let e2 = (stop.max(0) as usize).min(data.len()).max(s2);
                data.splice(s2..e2, replacement);
            } else {
                // Extended slice: replacement must have the exact same length.
                let indices: Vec<usize> = slice_indices(start, stop, step)
                    .filter(|&i| i < data.len())
                    .collect();
                if indices.len() != replacement.len() {
                    return Err(PyError::named(
                        "ValueError",
                        format!(
                            "attempt to assign bytes of size {} to extended slice of size {}",
                            replacement.len(),
                            indices.len()
                        ),
                    ));
                }
                for (&pos, &byte) in indices.iter().zip(replacement.iter()) {
                    data[pos] = byte;
                }
            }
            return Ok(());
        }
        // Integer item assignment: value must be an integer 0..255.
        let mut data = data_rc.borrow_mut();
        let idx = value_to_index(key, data.len(), "bytearray")?;
        let byte = value_to_byte(&value, "bytearray item assignment")?;
        data[idx] = byte;
        Ok(())
    }

    fn delete_item(&self, state: &BuiltinState, key: &Value) -> Result<()> {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<ByteArrayState>()
            .expect("bytearray state");
        let mut data = s.data.borrow_mut();
        // Slice deletion.
        if let ValueKind::BuiltinObject {
            ops,
            state: slice_state,
        } = key.kind()
            && crate::slice::is_slice_ops(ops)
        {
            let sb = slice_state.borrow();
            let sl = sb
                .downcast_ref::<crate::slice::SliceState>()
                .expect("slice state");
            let len = data.len() as i64;
            let (start, stop, step) = resolve_slice_indices(len, &sl.start, &sl.stop, &sl.step)?;
            drop(sb);
            if step == 1 {
                // For step == 1 both bounds are forward, clamped to [0, len].
                // An empty/reversed range (start > stop) deletes nothing.
                let s2 = (start.max(0) as usize).min(data.len());
                let e2 = (stop.max(0) as usize).min(data.len()).max(s2);
                data.drain(s2..e2);
            } else {
                // Extended slice deletion. `slice_indices` is already monotonic:
                // positive steps yield ascending indices and negative steps
                // yield descending indices. Normalise that list to ascending,
                // then compact the Vec once. Repeated `Vec::remove` shifted the
                // remaining tail for every selected byte, making `del b[::2]`
                // quadratic.
                let mut indices: Vec<usize> = slice_indices(start, stop, step)
                    .filter(|&i| i < data.len())
                    .collect();
                if step < 0 {
                    indices.reverse();
                }
                let mut indices = indices.into_iter().peekable();
                let mut old_index = 0usize;
                data.retain(|_| {
                    let should_delete = indices.peek().copied() == Some(old_index);
                    if should_delete {
                        indices.next();
                    }
                    old_index += 1;
                    !should_delete
                });
            }
            return Ok(());
        }
        // Integer deletion.
        let idx = value_to_index(key, data.len(), "bytearray")?;
        data.remove(idx);
        Ok(())
    }

    fn has_method(&self, name: &str) -> bool {
        // "fromhex" is exposed via getattr (as a BuiltinFunction sentinel)
        // rather than as a bound method, so exclude it from has_method to
        // avoid creating a bound_method wrapper that can't dispatch it.
        if name == "fromhex" {
            return false;
        }
        has_method(name)
    }

    fn getattr(&self, _state: &BuiltinState, name: &str) -> Option<Value> {
        // Expose `fromhex` as a BuiltinFunction sentinel on instances too
        // (matching CPython: `bytearray(b'').fromhex("deadbeef")` works).
        if name == "fromhex" {
            return Some(Value::builtin_function("bytearray.fromhex"));
        }
        None
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        method: &str,
        args: Vec<Value>,
        kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        validate_method_keywords(method, !kwargs.is_empty())?;
        validate_method_positional_arity(method, args.len())?;
        // Resolve the shared backing storage without copying it. Mutating
        // methods operate directly on this Rc and return before read-only
        // dispatch takes its snapshot, so `append`/`pop`/etc. stay O(1) where
        // their underlying Vec operation is O(1).
        let data_rc: Rc<RefCell<Vec<u8>>> = {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<ByteArrayState>()
                .expect("bytearray state");
            Rc::clone(&s.data)
        };
        if let Some(result) = call_storage_method(&data_rc, method, &args)? {
            return Ok(result);
        }

        // Shared bytes operations work from an immutable snapshot. Take it only
        // after storage-local methods have been dispatched so those methods do
        // not clone the entire bytearray before mutating a single byte.
        let data_snapshot = data_rc.borrow().clone();
        // Build an empty kwargs map for bytes::call_on_slice.
        let empty_kw: PyDict = PyDict::default();

        // Methods that return a new bytearray (wrapping the bytes result).
        match method {
            "upper" => {
                let out: Vec<u8> = data_snapshot
                    .iter()
                    .map(|b| b.to_ascii_uppercase())
                    .collect();
                return Ok(bytearray(out));
            }
            "lower" => {
                let out: Vec<u8> = data_snapshot
                    .iter()
                    .map(|b| b.to_ascii_lowercase())
                    .collect();
                return Ok(bytearray(out));
            }
            "swapcase" => {
                let out: Vec<u8> = data_snapshot
                    .iter()
                    .map(|&b| {
                        if b.is_ascii_uppercase() {
                            b.to_ascii_lowercase()
                        } else if b.is_ascii_lowercase() {
                            b.to_ascii_uppercase()
                        } else {
                            b
                        }
                    })
                    .collect();
                return Ok(bytearray(out));
            }
            "title" => {
                return Ok(bytearray(bytes_title(&data_snapshot)));
            }
            "capitalize" => {
                return Ok(bytearray(bytes_capitalize(&data_snapshot)));
            }
            // These delegate to the shared bytes impl with the same method name and
            // wrap the resulting bytes object as a new bytearray.
            "replace" | "strip" | "lstrip" | "rstrip" | "removeprefix" | "removesuffix"
            | "center" | "ljust" | "rjust" | "zfill" => {
                let result = crate::bytes::call_on_slice(method, &data_snapshot, &args, &empty_kw)?;
                return Ok(bytes_val_to_bytearray(result));
            }
            "translate" => {
                let pk_kwargs: PyDict = kwargs
                    .iter()
                    .map(|(k, v)| (PyKey::str_from(k), v.clone()))
                    .collect();
                let result =
                    crate::bytes::call_on_slice(method, &data_snapshot, &args, &pk_kwargs)?;
                return Ok(bytes_val_to_bytearray(result));
            }
            // expandtabs accepts `tabsize` by keyword; merge it into the
            // positional slot before delegating.
            "expandtabs" => {
                let merged =
                    crate::bytes::merge_single_kwarg_str(method, "tabsize", &args, kwargs)?;
                let result =
                    crate::bytes::call_on_slice(method, &data_snapshot, &merged, &empty_kw)?;
                return Ok(bytes_val_to_bytearray(result));
            }
            // partition / rpartition return tuples of bytearray elements.
            "partition" => {
                return bytearray_partition(&data_snapshot, &args, false);
            }
            "rpartition" => {
                return bytearray_partition(&data_snapshot, &args, true);
            }
            // split / rsplit accept `sep`/`maxsplit` by keyword; splitlines
            // accepts `keepends` by keyword. Merge them into positional slots
            // before delegating.
            "split" | "rsplit" => {
                let merged = crate::bytes::merge_split_kwargs_str(method, &args, kwargs)?;
                let result =
                    crate::bytes::call_on_slice(method, &data_snapshot, &merged, &empty_kw)?;
                return Ok(bytes_list_to_bytearray_list(result));
            }
            "splitlines" => {
                let result = if kwargs.is_empty() {
                    // Interpreter adapters already bind and truth-normalize
                    // keepends before BuiltinTypeOps dispatch. Keep the core
                    // Bool-only contract without allocating a duplicate Vec.
                    crate::bytes::call_on_slice(method, &data_snapshot, &args, &empty_kw)?
                } else {
                    let merged =
                        crate::bytes::merge_single_kwarg_str(method, "keepends", &args, kwargs)?;
                    crate::bytes::call_on_slice(method, &data_snapshot, &merged, &empty_kw)?
                };
                return Ok(bytes_list_to_bytearray_list(result));
            }
            // join: like bytes.join but accepts bytearray as separator and elements.
            "join" => {
                return bytearray_join(&data_snapshot, &args);
            }
            _ => {}
        }

        // Read-only methods that return scalars or bytes objects unchanged.
        // Delegate to the shared bytes implementation.
        match method {
            // `decode` honours `encoding`/`errors` keyword arguments and `hex`
            // honours `sep`/`bytes_per_sep`, so the caller's kwargs must be
            // threaded through to the shared bytes impl (mirroring `bytes`). The
            // other delegated methods below take only positional arguments.
            "decode" | "hex" => {
                let pk_kwargs: PyDict = kwargs
                    .iter()
                    .map(|(k, v)| (PyKey::str_from(k), v.clone()))
                    .collect();
                crate::bytes::call_on_slice(method, &data_snapshot, &args, &pk_kwargs)
            }
            "startswith" | "endswith" | "find" | "rfind" | "index" | "rindex" | "count"
            | "isdigit" | "isalpha" | "isalnum" | "isupper" | "islower" | "isspace" | "isascii"
            | "istitle" => crate::bytes::call_on_slice(method, &data_snapshot, &args, &empty_kw),
            _ => Err(PyError::named(
                "AttributeError",
                format!("'bytearray' object has no attribute '{method}'"),
            )),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public constructor
// ─────────────────────────────────────────────────────────────────────────────

/// Construct a `bytearray` Value from a `Vec<u8>`.
pub fn bytearray(data: Vec<u8>) -> Value {
    let state: Box<dyn Any> = Box::new(ByteArrayState {
        data: Rc::new(RefCell::new(data)),
    });
    Value::builtin_object(BYTEARRAY_OPS, state)
}

/// Return the backing `Rc<RefCell<Vec<u8>>>` if `v` is a `bytearray`.
pub fn as_bytearray_rc(v: &Value) -> Option<Rc<RefCell<Vec<u8>>>> {
    let ValueKind::BuiltinObject { ops, state } = v.kind() else {
        return None;
    };
    if ops.canonical_class_tag() != Some(pyrust_core::CanonicalClassTag::Bytearray) {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<ByteArrayState>()?;
    Some(Rc::clone(&s.data))
}

/// Extract a byte slice from a value if it is bytearray.  Returns `None`
/// for non-bytearray values.
pub fn as_bytearray_snapshot(v: &Value) -> Option<Vec<u8>> {
    let rc = as_bytearray_rc(v)?;
    Some(rc.borrow().clone())
}

// ─────────────────────────────────────────────────────────────────────────────
// Repr helper (also used by iter_values in expr.rs)
// ─────────────────────────────────────────────────────────────────────────────

/// Materialise a `bytearray` into its iteration elements (one `Value::int`
/// per byte).  Called by `iter_values` in the interpreter.
pub fn iter_elements(v: &Value) -> Option<Vec<Value>> {
    let rc = as_bytearray_rc(v)?;
    let data = rc.borrow();
    Some(data.iter().map(|&b| Value::int(b as i64)).collect())
}
