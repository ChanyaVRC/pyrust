//! `bytearray` built-in type.
//!
//! `bytearray` is the mutable counterpart to `bytes`.  It shares all of
//! `bytes`'s read methods and additionally supports item/slice assignment,
//! `append`, `extend`, `insert`, `pop`, `remove`, `reverse`, `clear`, and
//! `copy`.  Internally backed by `Rc<RefCell<Vec<u8>>>` so that clones share
//! mutable state (Python reference semantics: `b = a; b.append(1)` mutates `a`
//! too).

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::{BuiltinState, BuiltinTypeOps, PyDict, PyError, PyKey, Result, Value, ValueKind};

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

/// Returns `true` if `method` is exposed by `bytearray`.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
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
            } if ops.type_name() == TYPE_NAME => {
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
            ValueKind::Bytes(rc) => {
                Ok(crate::bytes::find_subsequence(&data, rc.as_slice()).is_some())
            }
            ValueKind::BuiltinObject {
                ops,
                state: item_state,
            } if ops.type_name() == TYPE_NAME => {
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
            && ops.type_name() == crate::slice::TYPE_NAME
        {
            let sb = slice_state.borrow();
            let sl = sb
                .downcast_ref::<crate::slice::SliceState>()
                .expect("slice state");
            let len = data.len() as i64;
            let (start, stop, step) = resolve_slice_indices(len, &sl.start, &sl.stop, &sl.step);
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
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<ByteArrayState>()
            .expect("bytearray state");
        let mut data = s.data.borrow_mut();
        // Slice assignment.
        if let ValueKind::BuiltinObject {
            ops,
            state: slice_state,
        } = key.kind()
            && ops.type_name() == crate::slice::TYPE_NAME
        {
            let sb = slice_state.borrow();
            let sl = sb
                .downcast_ref::<crate::slice::SliceState>()
                .expect("slice state");
            let len = data.len() as i64;
            let (start, stop, step) = resolve_slice_indices(len, &sl.start, &sl.stop, &sl.step);
            drop(sb);
            let replacement = bytes_from_value(&value, "bytearray slice assignment")?;
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
            && ops.type_name() == crate::slice::TYPE_NAME
        {
            let sb = slice_state.borrow();
            let sl = sb
                .downcast_ref::<crate::slice::SliceState>()
                .expect("slice state");
            let len = data.len() as i64;
            let (start, stop, step) = resolve_slice_indices(len, &sl.start, &sl.stop, &sl.step);
            drop(sb);
            if step == 1 {
                // For step == 1 both bounds are forward, clamped to [0, len].
                // An empty/reversed range (start > stop) deletes nothing.
                let s2 = (start.max(0) as usize).min(data.len());
                let e2 = (stop.max(0) as usize).min(data.len()).max(s2);
                data.drain(s2..e2);
            } else {
                // Extended slice deletion: collect indices in reverse and remove.
                let mut indices: Vec<usize> = slice_indices(start, stop, step)
                    .filter(|&i| i < data.len())
                    .collect();
                // Remove from back to front to keep indices valid.
                indices.sort_unstable_by(|a, b| b.cmp(a));
                for i in indices {
                    data.remove(i);
                }
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
        // Extract the byte slice; needed for read methods.
        let data_snapshot: Vec<u8> = {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<ByteArrayState>()
                .expect("bytearray state");
            s.data.borrow().clone()
        };
        // For mutable methods we need the Rc to mutate through.
        let data_rc: Rc<RefCell<Vec<u8>>> = {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<ByteArrayState>()
                .expect("bytearray state");
            Rc::clone(&s.data)
        };

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
            | "center" | "ljust" | "rjust" | "zfill" | "translate" => {
                let result = crate::bytes::call_on_slice(method, &data_snapshot, &args, &empty_kw)?;
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
                let merged =
                    crate::bytes::merge_single_kwarg_str(method, "keepends", &args, kwargs)?;
                let result =
                    crate::bytes::call_on_slice(method, &data_snapshot, &merged, &empty_kw)?;
                return Ok(bytes_list_to_bytearray_list(result));
            }
            // join: like bytes.join but accepts bytearray as separator and elements.
            "join" => {
                return bytearray_join(&data_snapshot, &args);
            }
            // Mutable methods.
            "append" => {
                let byte_val = args.into_iter().next().ok_or_else(|| {
                    PyError::named(
                        "TypeError",
                        "append() takes exactly one argument (0 given)".to_string(),
                    )
                })?;
                let byte = value_to_byte(&byte_val, "bytearray.append")?;
                data_rc.borrow_mut().push(byte);
                return Ok(Value::none());
            }
            "extend" => {
                let iterable = args.into_iter().next().ok_or_else(|| {
                    PyError::named(
                        "TypeError",
                        "extend() takes exactly one argument (0 given)".to_string(),
                    )
                })?;
                let bytes = bytes_from_value(&iterable, "bytearray.extend")?;
                data_rc.borrow_mut().extend_from_slice(&bytes);
                return Ok(Value::none());
            }
            "insert" => {
                if args.len() < 2 {
                    return Err(PyError::named(
                        "TypeError",
                        format!("insert() takes exactly 2 arguments ({} given)", args.len()),
                    ));
                }
                let idx = match args[0].kind() {
                    ValueKind::Int(n) => n,
                    ValueKind::Bool(b) => b as i64,
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "integer argument expected".to_string(),
                        ));
                    }
                };
                let byte = value_to_byte(&args[1], "bytearray.insert")?;
                let mut data = data_rc.borrow_mut();
                let len = data.len();
                let pos = if idx < 0 {
                    let from_end = (-idx) as usize;
                    len.saturating_sub(from_end)
                } else {
                    (idx as usize).min(len)
                };
                data.insert(pos, byte);
                return Ok(Value::none());
            }
            "pop" => {
                let mut data = data_rc.borrow_mut();
                if data.is_empty() {
                    return Err(PyError::named(
                        "IndexError",
                        "pop from empty bytearray".to_string(),
                    ));
                }
                let idx = match args.first() {
                    None => data.len() - 1,
                    Some(v) => match v.kind() {
                        ValueKind::Int(n) => {
                            let len = data.len();

                            if n < 0 {
                                let from_end = (-n) as usize;
                                len.checked_sub(from_end).ok_or_else(|| {
                                    PyError::named(
                                        "IndexError",
                                        "bytearray index out of range".to_string(),
                                    )
                                })?
                            } else {
                                let ui = n as usize;
                                if ui >= len {
                                    return Err(PyError::named(
                                        "IndexError",
                                        "bytearray index out of range".to_string(),
                                    ));
                                }
                                ui
                            }
                        }
                        ValueKind::Bool(b) => b as usize,
                        _ => {
                            return Err(PyError::named(
                                "TypeError",
                                "integer argument expected".to_string(),
                            ));
                        }
                    },
                };
                let byte = data.remove(idx);
                return Ok(Value::int(byte as i64));
            }
            "remove" => {
                let val = args.into_iter().next().ok_or_else(|| {
                    PyError::named(
                        "TypeError",
                        "remove() takes exactly one argument (0 given)".to_string(),
                    )
                })?;
                let byte = value_to_byte(&val, "bytearray.remove")?;
                let mut data = data_rc.borrow_mut();
                let pos = data.iter().position(|&b| b == byte).ok_or_else(|| {
                    PyError::named("ValueError", "value not found in bytearray".to_string())
                })?;
                data.remove(pos);
                return Ok(Value::none());
            }
            "reverse" => {
                data_rc.borrow_mut().reverse();
                return Ok(Value::none());
            }
            "clear" => {
                data_rc.borrow_mut().clear();
                return Ok(Value::none());
            }
            "copy" => {
                let snapshot = data_rc.borrow().clone();
                return Ok(bytearray(snapshot));
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
    if ops.type_name() != TYPE_NAME {
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

// ─────────────────────────────────────────────────────────────────────────────
// Private helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve a slice object's start/stop/step against a sequence of length
/// `len`. Returns `(start, stop, step)` as signed `i64` values matching CPython
/// slice semantics: `stop` is the *exclusive* boundary, and for a backward
/// slice both `start` and `stop` may be `-1` (an empty slice / a walk down to
/// and including index 0). For forward (`step >= 0`) slices both land in
/// `[0, len]`, so step==1 callers can cast them back to `usize`.
fn resolve_slice_indices(len: i64, start: &Value, stop: &Value, step: &Value) -> (i64, i64, i64) {
    let step_val: i64 = match step.kind() {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => b as i64,
        _ => 1,
    };
    let step_val = if step_val == 0 { 1 } else { step_val };

    let clamp = |v: i64, lo: i64, hi: i64| v.max(lo).min(hi);

    let (default_start, default_stop) = if step_val > 0 {
        (0i64, len)
    } else {
        (len - 1, -1i64)
    };

    let start_val: i64 = match start.kind() {
        ValueKind::None => default_start,
        ValueKind::Int(n) => {
            if step_val > 0 {
                if n < 0 {
                    clamp(n + len, 0, len)
                } else {
                    clamp(n, 0, len)
                }
            } else if n < 0 {
                // Backward slice: a `start` below `-len` clamps to -1 (empty);
                // otherwise to at most `len - 1` (the last valid index).
                clamp(n + len, -1, len - 1)
            } else {
                clamp(n, -1, len - 1)
            }
        }
        ValueKind::Bool(b) => b as i64,
        _ => default_start,
    };
    let stop_val: i64 = match stop.kind() {
        ValueKind::None => default_stop,
        ValueKind::Int(n) => {
            if step_val > 0 {
                if n < 0 {
                    clamp(n + len, 0, len)
                } else {
                    clamp(n, 0, len)
                }
            } else if n < 0 {
                clamp(n + len, -1, len - 1)
            } else {
                clamp(n, -1, len - 1)
            }
        }
        ValueKind::Bool(b) => b as i64,
        _ => default_stop,
    };

    // Both `start` and `stop` are kept signed. For a backward slice `start`
    // may be -1 (the slice is empty) and `stop` may be -1 (iterate down to and
    // including index 0); round-tripping either through `usize` would corrupt
    // these boundary cases. For forward slices both land in [0, len], so the
    // step==1 callers can cast back to `usize` safely.
    (start_val, stop_val, step_val)
}

/// Generate index sequence for a slice (start, stop, step) over a sequence.
fn slice_indices(start: i64, stop: i64, step: i64) -> impl Iterator<Item = usize> {
    struct SliceIter {
        current: i64,
        stop: i64,
        step: i64,
    }
    impl Iterator for SliceIter {
        type Item = usize;
        fn next(&mut self) -> Option<usize> {
            // Forward (step > 0) and backward (step < 0) slices share the same
            // advance step; only the bound test differs.
            let in_range = (self.step > 0 && self.current < self.stop)
                || (self.step < 0 && self.current > self.stop);
            if in_range {
                let c = self.current as usize;
                self.current += self.step;
                Some(c)
            } else {
                None
            }
        }
    }
    // `stop` is already the exclusive boundary (CPython slice semantics):
    // forward slices stop before `stop`, backward slices stop after `stop`.
    SliceIter {
        current: start,
        stop,
        step,
    }
}

/// Convert a subscript `Value` to a concrete `usize` index into a slice of
/// length `len`.  Raises `IndexError` if out of range.
fn value_to_index(key: &Value, len: usize, type_name: &str) -> Result<usize> {
    let idx: i64 = match key.kind() {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => b as i64,
        ValueKind::BigInt(_) => {
            return Err(PyError::named(
                "IndexError",
                format!("{type_name} index out of range"),
            ));
        }
        _ => {
            // CPython 3.12 uses the bare (unquoted) type name here, matching
            // bytes: `bytearray indices must be integers or slices, not float`.
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{type_name} indices must be integers or slices, not {}",
                    pyrust_core::builtin_type_name(key)
                ),
            ));
        }
    };
    let i = if idx < 0 {
        let from_end = (-idx) as usize;
        len.checked_sub(from_end).ok_or_else(|| {
            PyError::named("IndexError", format!("{type_name} index out of range"))
        })?
    } else {
        let ui = idx as usize;
        if ui >= len {
            return Err(PyError::named(
                "IndexError",
                format!("{type_name} index out of range"),
            ));
        }
        ui
    };
    Ok(i)
}

/// Convert a `Value` to a single byte (0..255).  Used for item assignment
/// and `append`.
fn value_to_byte(v: &Value, _context: &str) -> Result<u8> {
    match v.kind() {
        ValueKind::Int(n) => {
            if (0..=255).contains(&n) {
                Ok(n as u8)
            } else {
                Err(PyError::named(
                    "ValueError",
                    "byte must be in range(0, 256)".to_string(),
                ))
            }
        }
        ValueKind::Bool(b) => Ok(b as u8),
        // A BigInt is a valid int but always outside 0..=255 — CPython raises
        // ValueError, not the "cannot be interpreted as an integer" TypeError
        // used for non-int types.
        ValueKind::BigInt(_) => Err(PyError::named(
            "ValueError",
            "byte must be in range(0, 256)".to_string(),
        )),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                pyrust_core::builtin_type_name(v)
            ),
        )),
    }
}

/// Extract a `Vec<u8>` from a bytes-like value (bytes or bytearray) or an
/// iterable of integers.
fn bytes_from_value(v: &Value, context: &str) -> Result<Vec<u8>> {
    match v.kind() {
        ValueKind::Bytes(rc) => Ok(rc.as_slice().to_vec()),
        ValueKind::BuiltinObject { ops, state } if ops.type_name() == TYPE_NAME => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<ByteArrayState>()
                .expect("bytearray state");
            Ok(s.data.borrow().clone())
        }
        ValueKind::List(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items.iter() {
                out.push(value_to_byte(item, context)?);
            }
            Ok(out)
        }
        ValueKind::Tuple(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items.iter() {
                out.push(value_to_byte(item, context)?);
            }
            Ok(out)
        }
        // CPython rejects str in slice assignment with a special message;
        // for extend() it iterates the string and fails per-character.
        ValueKind::Str(_) if context != "bytearray.extend" => Err(PyError::named(
            "TypeError",
            "can assign only bytes, buffers, or iterables of ints in range(0, 256)".to_string(),
        )),
        _ => {
            let type_name = pyrust_core::builtin_type_name(v);
            // Try materialising via the registered iter callback.
            let items = pyrust_core::iter_values_via_registry(v).map_err(|_| {
                // Mirror CPython wording for the two call sites:
                // extend() → "can't extend bytearray with <type>"
                // slice assignment → "can assign only bytes, buffers, or iterables of ints in range(0, 256)"
                if context == "bytearray.extend" {
                    PyError::named(
                        "TypeError",
                        format!("can't extend bytearray with {type_name}"),
                    )
                } else {
                    PyError::named(
                        "TypeError",
                        "can assign only bytes, buffers, or iterables of ints in range(0, 256)"
                            .to_string(),
                    )
                }
            })?;
            let mut out = Vec::with_capacity(items.len());
            for item in &items {
                out.push(value_to_byte(item, context)?);
            }
            Ok(out)
        }
    }
}

/// Convert a `Value::bytes(...)` result to `bytearray(...)`.  Panics if
/// `v` is not a bytes value — callers guarantee this.
fn bytes_val_to_bytearray(v: Value) -> Value {
    match v.kind() {
        ValueKind::Bytes(rc) => bytearray(rc.as_slice().to_vec()),
        _ => panic!("bytes_val_to_bytearray: expected bytes value"),
    }
}

/// Convert a `Value::list` of `Value::bytes` items to a `Value::list` of
/// `bytearray` items (for split/rsplit/splitlines return types).
fn bytes_list_to_bytearray_list(v: Value) -> Value {
    // Collect into a snapshot first to avoid holding a kind() borrow.
    let snapshot: Option<Vec<Value>> = match v.kind() {
        ValueKind::List(items) => Some(
            items
                .iter()
                .map(|item| match item.kind() {
                    ValueKind::Bytes(rc) => bytearray(rc.as_slice().to_vec()),
                    _ => item.clone(),
                })
                .collect(),
        ),
        _ => None,
    };
    match snapshot {
        Some(out) => Value::list(out),
        None => v,
    }
}

/// `bytearray.partition` / `bytearray.rpartition` — returns a 3-tuple of
/// bytearray values.
fn bytearray_partition(bytes: &[u8], args: &[Value], reverse: bool) -> Result<Value> {
    let sep_val = args.first().ok_or_else(|| {
        let name = if reverse { "rpartition" } else { "partition" };
        PyError::named(
            "TypeError",
            format!("bytearray.{name}() requires exactly 1 argument"),
        )
    })?;
    let sep: Vec<u8> = match sep_val.kind() {
        ValueKind::Bytes(rc) => rc.as_slice().to_vec(),
        ValueKind::BuiltinObject { ops, state } if ops.type_name() == TYPE_NAME => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<ByteArrayState>()
                .expect("bytearray state");
            s.data.borrow().clone()
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(sep_val)
                ),
            ));
        }
    };
    if sep.is_empty() {
        return Err(PyError::named("ValueError", "empty separator".to_string()));
    }
    let found = if reverse {
        crate::bytes::rfind_subsequence(bytes, &sep)
    } else {
        crate::bytes::find_subsequence(bytes, &sep)
    };
    let parts = match found {
        Some(pos) => {
            let before = bytearray(bytes[..pos].to_vec());
            let mid = bytearray(sep.clone());
            let after = bytearray(bytes[pos + sep.len()..].to_vec());
            vec![before, mid, after]
        }
        None => {
            if reverse {
                vec![
                    bytearray(vec![]),
                    bytearray(vec![]),
                    bytearray(bytes.to_vec()),
                ]
            } else {
                vec![
                    bytearray(bytes.to_vec()),
                    bytearray(vec![]),
                    bytearray(vec![]),
                ]
            }
        }
    };
    Ok(Value::tuple(parts))
}

/// `bytearray.join(iterable)` — join elements of an iterable of bytes-like
/// objects using this bytearray as separator.
fn bytearray_join(sep: &[u8], args: &[Value]) -> Result<Value> {
    let iterable = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "join() takes exactly one argument (0 given)".to_string(),
        )
    })?;
    // Collect elements from the iterable.
    let items = pyrust_core::iter_values_via_registry(iterable).map_err(|_| {
        PyError::named(
            "TypeError",
            format!(
                "can only join an iterable, not '{}'",
                pyrust_core::builtin_type_name(iterable)
            ),
        )
    })?;
    let mut out: Vec<u8> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(sep);
        }
        match item.kind() {
            ValueKind::Bytes(rc) => out.extend_from_slice(rc.as_slice()),
            ValueKind::BuiltinObject { ops, state } if ops.type_name() == TYPE_NAME => {
                let borrow = state.borrow();
                let s = borrow
                    .downcast_ref::<ByteArrayState>()
                    .expect("bytearray state");
                out.extend_from_slice(&s.data.borrow());
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "sequence item {}: expected a bytes-like object, {} found",
                        i,
                        pyrust_core::builtin_type_name(item)
                    ),
                ));
            }
        }
    }
    Ok(bytearray(out))
}

/// Title-case a byte slice (same logic as bytes).
fn bytes_title(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut prev_was_alpha = false;
    for &b in bytes {
        if b.is_ascii_alphabetic() {
            if prev_was_alpha {
                out.push(b.to_ascii_lowercase());
            } else {
                out.push(b.to_ascii_uppercase());
            }
            prev_was_alpha = true;
        } else {
            out.push(b);
            prev_was_alpha = false;
        }
    }
    out
}

/// Capitalize a byte slice (same logic as bytes).
fn bytes_capitalize(bytes: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = bytes.iter().map(|b| b.to_ascii_lowercase()).collect();
    if let Some(first) = out.first_mut() {
        *first = first.to_ascii_uppercase();
    }
    out
}

/// Render a byte slice as a Python bytes literal (`b'...'`).  Used for
/// `bytearray.__repr__`, which wraps this in `bytearray(...)`.
fn bytearray_bytes_repr(bytes: &[u8]) -> String {
    let has_single = bytes.contains(&b'\'');
    let has_double = bytes.contains(&b'"');
    let q = if has_single && !has_double { '"' } else { '\'' };
    let mut out = String::with_capacity(bytes.len() + 3);
    out.push('b');
    out.push(q);
    for &b in bytes {
        match b {
            0x09 => out.push_str("\\t"),
            0x0a => out.push_str("\\n"),
            0x0d => out.push_str("\\r"),
            0x5c => out.push_str("\\\\"),
            b'\'' if q == '\'' => out.push_str("\\'"),
            b'"' if q == '"' => out.push_str("\\\""),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push(q);
    out
}
