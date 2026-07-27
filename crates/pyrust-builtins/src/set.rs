use pyrust_core::{PyError, PyKey, PySet, Result, Value, ValueKind};

use crate::method_signature::{KeywordPolicy, PositionalArity};

pub const TYPE_NAME: &str = "set";

/// Canonical list of method names dispatched by `call`.
pub const METHODS: &[&str] = &[
    "__iter__",
    "add",
    "remove",
    "discard",
    "pop",
    "clear",
    "update",
    "intersection_update",
    "difference_update",
    "symmetric_difference_update",
    "copy",
    "union",
    "intersection",
    "difference",
    "symmetric_difference",
    "issubset",
    "issuperset",
    "isdisjoint",
];

pub const CLASS_ATTRS: crate::primitive_class_attrs::PrimitiveClassAttrs =
    crate::primitive_class_attrs::PrimitiveClassAttrs::new(TYPE_NAME, METHODS).with_flags(
        crate::primitive_class_attrs::PrimitiveClassFlags::NONE
            .with_init()
            .with_class_getitem(),
    );

/// Returns `true` if `method` is the name of a built-in `set` method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Method {
    Iter,
    Add,
    Remove,
    Discard,
    Pop,
    Clear,
    Update,
    IntersectionUpdate,
    DifferenceUpdate,
    SymmetricDifferenceUpdate,
    Copy,
    Union,
    Intersection,
    Difference,
    SymmetricDifference,
    IsSubset,
    IsSuperset,
    IsDisjoint,
    Contains,
}

impl Method {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Iter => "__iter__",
            Self::Add => "add",
            Self::Remove => "remove",
            Self::Discard => "discard",
            Self::Pop => "pop",
            Self::Clear => "clear",
            Self::Update => "update",
            Self::IntersectionUpdate => "intersection_update",
            Self::DifferenceUpdate => "difference_update",
            Self::SymmetricDifferenceUpdate => "symmetric_difference_update",
            Self::Copy => "copy",
            Self::Union => "union",
            Self::Intersection => "intersection",
            Self::Difference => "difference",
            Self::SymmetricDifference => "symmetric_difference",
            Self::IsSubset => "issubset",
            Self::IsSuperset => "issuperset",
            Self::IsDisjoint => "isdisjoint",
            Self::Contains => "__contains__",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MethodSpec {
    method: Method,
    arity: PositionalArity,
    keywords: KeywordPolicy,
}

impl MethodSpec {
    #[inline(always)]
    pub const fn method(self) -> Method {
        self.method
    }

    #[inline]
    pub fn validate_positional_arity(self, given: usize) -> Result<()> {
        if self.arity.accepts(given) {
            return Ok(());
        }
        self.arity
            .reject_excess(TYPE_NAME, self.method.name(), given)
    }

    #[inline]
    pub fn validate_keywords(self, has_keywords: bool) -> Result<()> {
        if self.keywords.accepts(has_keywords) {
            return Ok(());
        }
        self.keywords
            .validate(TYPE_NAME, self.method.name(), has_keywords)
    }

    #[inline(always)]
    pub const fn keyword_policy(self) -> KeywordPolicy {
        self.keywords
    }
}

/// Resolve a set method once into its semantic route and positional policy.
#[inline]
pub fn method_spec(method: &str) -> Option<MethodSpec> {
    let (method, arity) = match method {
        "__iter__" => (Method::Iter, PositionalArity::exact(0)),
        "add" => (Method::Add, PositionalArity::exact(1)),
        "remove" => (Method::Remove, PositionalArity::exact(1)),
        "discard" => (Method::Discard, PositionalArity::exact(1)),
        "pop" => (Method::Pop, PositionalArity::exact(0)),
        "clear" => (Method::Clear, PositionalArity::exact(0)),
        "update" => (Method::Update, PositionalArity::variadic(0)),
        "intersection_update" => (Method::IntersectionUpdate, PositionalArity::variadic(0)),
        "difference_update" => (Method::DifferenceUpdate, PositionalArity::variadic(0)),
        "symmetric_difference_update" => {
            (Method::SymmetricDifferenceUpdate, PositionalArity::exact(1))
        }
        "copy" => (Method::Copy, PositionalArity::exact(0)),
        "union" => (Method::Union, PositionalArity::variadic(0)),
        "intersection" => (Method::Intersection, PositionalArity::variadic(0)),
        "difference" => (Method::Difference, PositionalArity::variadic(0)),
        "symmetric_difference" => (Method::SymmetricDifference, PositionalArity::exact(1)),
        "issubset" => (Method::IsSubset, PositionalArity::exact(1)),
        "issuperset" => (Method::IsSuperset, PositionalArity::exact(1)),
        "isdisjoint" => (Method::IsDisjoint, PositionalArity::exact(1)),
        "__contains__" => (Method::Contains, PositionalArity::exact(1)),
        _ => return None,
    };
    Some(MethodSpec {
        method,
        arity,
        keywords: KeywordPolicy::Reject,
    })
}

/// Positional signature for set methods, including the interpreter-owned
/// object-key routes.
pub fn positional_arity(method: &str) -> Option<PositionalArity> {
    method_spec(method).map(|spec| spec.arity)
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

#[inline]
pub fn validate_method_keywords(method: &str, has_keywords: bool) -> Result<()> {
    if !has_keywords {
        return Ok(());
    }
    match method_spec(method) {
        Some(spec) => spec.validate_keywords(true),
        None => Ok(()),
    }
}

pub fn keyword_policy(method: &str) -> Option<KeywordPolicy> {
    method_spec(method).map(MethodSpec::keyword_policy)
}

/// Dispatch a `set` method.  Receiver is `&Value`; each method body
/// either reads via `set_with` or writes via `set_with_mut`, with
/// argument iteration happening *outside* the scoped borrow so
/// self-aliased calls (`s.update(s)`) never simultaneously borrow the
/// same storage (#448).
pub fn call(method: &str, receiver: &Value, args: Vec<Value>) -> Result<Value> {
    let Some(spec) = method_spec(method) else {
        return Err(PyError::named(
            "AttributeError",
            format!("'set' object has no attribute '{method}'"),
        ));
    };
    spec.validate_positional_arity(args.len())?;
    call_resolved(spec.method(), receiver, args)
}

/// Dispatch a method whose name and arity were resolved by [`method_spec`].
#[doc(hidden)]
pub fn call_resolved(method: Method, receiver: &Value, args: Vec<Value>) -> Result<Value> {
    let args = args.as_slice();
    let not_set = || PyError::named("TypeError", "set method receiver is not a set".to_string());
    match method {
        // ── Mutating ───────────────────────────────────────────────
        Method::Add => receiver
            .set_with_mut(|items| add(items, args))
            .ok_or_else(not_set)?,
        Method::Remove => receiver
            .set_with_mut(|items| remove(items, args))
            .ok_or_else(not_set)?,
        Method::Discard => receiver
            .set_with_mut(|items| discard(items, args))
            .ok_or_else(not_set)?,
        Method::Pop => receiver.set_with_mut(pop).ok_or_else(not_set)?,
        Method::Clear => {
            receiver.set_clear()?;
            Ok(Value::none())
        }
        // Iterating + mutating methods: materialise all arg
        // iterables BEFORE borrow_mut so a self-aliased call
        // (`s.update(s)`) doesn't take a `&` to the same storage
        // we're about to `&mut`.
        // For the `*_update` family we collect ONE iterable, apply it,
        // then move to the next.  CPython does the same — an error
        // collecting the 2nd arg leaves the 1st arg's effect on the
        // receiver visible (`s.update([1], object())` leaves `1` in
        // `s` before raising TypeError).  Collecting all args upfront
        // would make these all-or-nothing, diverging from CPython.
        Method::Update => {
            for arg in args {
                let snap = snapshot_iterable(receiver, arg)?;
                receiver
                    .set_with_mut(|items| items.extend(snap))
                    .ok_or_else(not_set)?;
            }
            Ok(Value::none())
        }
        Method::IntersectionUpdate => {
            for arg in args {
                let snap = snapshot_iterable(receiver, arg)?;
                receiver
                    .set_with_mut(|items| items.retain(|k| snap.contains(k)))
                    .ok_or_else(not_set)?;
            }
            Ok(Value::none())
        }
        Method::DifferenceUpdate => {
            for arg in args {
                let snap = snapshot_iterable(receiver, arg)?;
                receiver
                    .set_with_mut(|items| subtract_snapshot(items, &snap))
                    .ok_or_else(not_set)?;
            }
            Ok(Value::none())
        }
        Method::SymmetricDifferenceUpdate => {
            let other = args
                .first()
                .ok_or_else(|| {
                    PyError::Runtime(
                        "set.symmetric_difference_update() requires 1 argument".to_string(),
                    )
                })
                .and_then(|v| snapshot_iterable(receiver, v))?;
            receiver
                .set_with_mut(|items| {
                    let mut to_add: Vec<PyKey> = Vec::new();
                    for k in &other {
                        if !items.contains(k) {
                            to_add.push(k.clone());
                        }
                    }
                    items.retain(|k| !other.contains(k));
                    for k in to_add {
                        items.insert(k);
                    }
                })
                .ok_or_else(not_set)?;
            Ok(Value::none())
        }
        // ── Non-mutating ───────────────────────────────────────────
        // Scoped read borrow + clone is enough; no `&mut` ever taken.
        Method::Copy => receiver
            .set_with(|items| Value::set(items.clone()))
            .ok_or_else(not_set),
        Method::Union => union(receiver, args),
        Method::Intersection => intersection(receiver, args),
        Method::Difference => difference(receiver, args),
        Method::SymmetricDifference => symmetric_difference(receiver, args),
        Method::IsSubset => issubset(receiver, args),
        Method::IsSuperset => issuperset(receiver, args),
        Method::IsDisjoint => isdisjoint(receiver, args),
        // Intercepted by the interpreter's iteration domain; drift sentinel.
        Method::Iter => Err(PyError::named(
            "TypeError",
            "'set' __iter__ must be dispatched by the interpreter",
        )),
        Method::Contains => Err(PyError::named(
            "AttributeError",
            "'set' object has no attribute '__contains__'",
        )),
    }
}

/// Materialise each arg into an owned `PySet`.  Performed
/// before any `borrow_mut` on the receiver so a self-aliased call
/// (`s.update(s)`) reads its own pre-update snapshot, exactly matching
/// CPython's iterate-then-mutate semantics.
fn collect_iterables(receiver: &Value, args: &[Value]) -> Result<Vec<PySet>> {
    args.iter()
        .map(|arg| snapshot_iterable(receiver, arg))
        .collect()
}

/// Same as `collect_iterable` but takes a snapshot of the receiver
/// via `set_with` when the arg aliases it, avoiding the
/// `as_set`/`borrow` reentry on the same storage.
fn snapshot_iterable(receiver: &Value, arg: &Value) -> Result<PySet> {
    if std::ptr::eq(receiver as *const Value, arg as *const Value)
        || receiver.value_id() == arg.value_id() && receiver.value_id().is_some()
    {
        // Aliased: snapshot via the scoped borrow.
        return receiver.set_with(|items| items.clone()).ok_or_else(|| {
            PyError::named(
                "TypeError",
                "set snapshot receiver is not a set".to_string(),
            )
        });
    }
    collect_iterable(arg)
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Walk `v` recursively (into tuples and slice components) to find the leaf
/// unhashable value and return its type name.
///
/// This mirrors CPython's `PyObject_Hash` behaviour: when a tuple or slice
/// contains an unhashable element, the error names the leaf type (e.g.
/// `'list'`), not the container (`'tuple'` or `'slice'`).
pub fn leaf_unhashable_type_name(v: &Value) -> String {
    // Tuple: recurse into elements.
    if let ValueKind::Tuple(items) = v.kind() {
        for item in items {
            if item.to_key().is_none() {
                return leaf_unhashable_type_name(item);
            }
        }
        // All elements hashable — the tuple itself is the culprit (shouldn't
        // happen if caller verified to_key() == None, but be safe).
        return pyrust_core::builtin_type_name(v).into_owned();
    }
    // Slice: recurse into components.
    if let ValueKind::BuiltinObject { ops, state } = v.kind()
        && crate::slice::is_slice_ops(ops)
    {
        let borrow = state.borrow();
        if let Some(s) = borrow.downcast_ref::<crate::slice::SliceState>() {
            for component in [&s.start, &s.stop, &s.step] {
                if component.to_key().is_none() {
                    return leaf_unhashable_type_name(component);
                }
            }
        }
    }
    // Leaf unhashable value (list, dict, set, …).
    pyrust_core::builtin_type_name(v).into_owned()
}

/// Convert `v` to a `PyKey`, producing a precise "unhashable type: 'X'" error.
///
/// When `v` is a `slice` or a `tuple` whose `to_key()` returns `None`, the
/// components are inspected recursively to surface the actual unhashable leaf
/// (e.g. a `list` argument to `slice()`) rather than blaming the container.
/// This matches CPython 3.12 behaviour where `s.update([slice([1,2], 3)])`
/// raises `TypeError: unhashable type: 'list'`, not `'slice'`.
fn to_key(v: &Value) -> Result<PyKey> {
    if let Some(key) = v.to_key() {
        return Ok(key);
    }
    Err(PyError::named(
        "TypeError",
        format!("unhashable type: '{}'", leaf_unhashable_type_name(v)),
    ))
}

/// Collect an iterable `Value` into a set of `PyKey`s.
fn collect_iterable(v: &Value) -> Result<PySet> {
    let mut out = PySet::default();
    match v.kind() {
        ValueKind::Set(s) => {
            for k in s.iter() {
                out.insert(k.clone());
            }
        }
        _ if crate::frozenset::as_items(v).is_some() => {
            let rc = crate::frozenset::as_items(v).unwrap();
            for k in rc.iter() {
                out.insert(k.clone());
            }
        }
        ValueKind::List(items) => {
            for item in items.iter() {
                out.insert(to_key(item)?);
            }
        }
        ValueKind::Tuple(items) => {
            for item in items.iter() {
                out.insert(to_key(item)?);
            }
        }
        ValueKind::Dict(d) => {
            for k in d.keys() {
                out.insert(k.clone());
            }
        }
        ValueKind::Str(s) => {
            for ch in s.chars() {
                out.insert(PyKey::str_from(ch.encode_utf8(&mut [0u8; 4])));
            }
        }
        ValueKind::Range { start, stop, step } => {
            if step != 0 {
                let mut cur = start;
                loop {
                    if step > 0 && cur >= stop {
                        break;
                    }
                    if step < 0 && cur <= stop {
                        break;
                    }
                    out.insert(PyKey::Int(cur));
                    let Some(next) = cur.checked_add(step) else {
                        // A monotonic i64 range can only overflow after its
                        // final in-domain value; wrapping would re-enter the
                        // opposite side and make materialisation effectively
                        // unbounded.
                        break;
                    };
                    cur = next;
                }
            }
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "'{}' object is not iterable",
                    pyrust_core::builtin_type_name(v)
                ),
            ));
        }
    }
    Ok(out)
}

/// Remove all keys in `snapshot` while preserving survivor order.
///
/// `retain` is linear in the receiver and avoids `IndexSet::shift_remove`'s
/// repeated element shifts for multi-key differences.  Keep the single-key
/// lookup path, though: a missing key remains O(1) instead of forcing a full
/// receiver scan.
#[inline]
fn subtract_snapshot(items: &mut PySet, snapshot: &PySet) {
    match snapshot.len() {
        0 => {}
        1 => {
            items.shift_remove(snapshot.iter().next().unwrap());
        }
        _ => items.retain(|key| !snapshot.contains(key)),
    }
}

// ── mutating methods ──────────────────────────────────────────────────────────

fn add(items: &mut PySet, args: &[Value]) -> Result<Value> {
    let elem = args
        .first()
        .ok_or_else(|| PyError::Runtime("set.add() requires 1 argument".to_string()))?;
    items.insert(to_key(elem)?);
    Ok(Value::none())
}

fn remove(items: &mut PySet, args: &[Value]) -> Result<Value> {
    let elem = args
        .first()
        .ok_or_else(|| PyError::Runtime("set.remove() requires 1 argument".to_string()))?;
    let key = to_key(elem)?;
    if items.shift_remove(&key) {
        Ok(Value::none())
    } else {
        Err(PyError::key_error(elem.clone()))
    }
}

fn discard(items: &mut PySet, args: &[Value]) -> Result<Value> {
    let elem = args
        .first()
        .ok_or_else(|| PyError::Runtime("set.discard() requires 1 argument".to_string()))?;
    let key = to_key(elem)?;
    items.shift_remove(&key);
    Ok(Value::none())
}

fn pop(items: &mut PySet) -> Result<Value> {
    match items.pop() {
        Some(k) => Ok(key_to_value(k)),
        None => Err(PyError::named(
            "KeyError",
            "pop from an empty set".to_string(),
        )),
    }
}

// ── non-mutating methods ──────────────────────────────────────────────────────
//
// Each helper takes `&Value` and uses a scoped `set_with` borrow.
// Argument iterables are collected before the borrow opens, so
// self-aliased calls (`s.union(s)`) read a stable snapshot.

fn union(receiver: &Value, args: &[Value]) -> Result<Value> {
    let snapshots = collect_iterables(receiver, args)?;
    receiver
        .set_with(|items| {
            let mut result = items.clone();
            for snap in snapshots {
                for k in snap {
                    result.insert(k);
                }
            }
            Value::set(result)
        })
        .ok_or_else(|| PyError::named("TypeError", "set.union receiver is not a set".to_string()))
}

fn intersection(receiver: &Value, args: &[Value]) -> Result<Value> {
    let snapshots = collect_iterables(receiver, args)?;
    receiver
        .set_with(|items| {
            let mut result = items.clone();
            for snap in &snapshots {
                result.retain(|k| snap.contains(k));
            }
            Value::set(result)
        })
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "set.intersection receiver is not a set".to_string(),
            )
        })
}

fn difference(receiver: &Value, args: &[Value]) -> Result<Value> {
    let snapshots = collect_iterables(receiver, args)?;
    receiver
        .set_with(|items| {
            let mut result = items.clone();
            for snap in &snapshots {
                subtract_snapshot(&mut result, snap);
            }
            Value::set(result)
        })
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "set.difference receiver is not a set".to_string(),
            )
        })
}

fn symmetric_difference(receiver: &Value, args: &[Value]) -> Result<Value> {
    let other = args
        .first()
        .ok_or_else(|| {
            PyError::Runtime("set.symmetric_difference() requires 1 argument".to_string())
        })
        .and_then(|v| snapshot_iterable(receiver, v))?;
    receiver
        .set_with(|items| {
            let mut result: PySet = PySet::default();
            for k in items {
                if !other.contains(k) {
                    result.insert(k.clone());
                }
            }
            for k in &other {
                if !items.contains(k) {
                    result.insert(k.clone());
                }
            }
            Value::set(result)
        })
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "set.symmetric_difference receiver is not a set".to_string(),
            )
        })
}

fn issubset(receiver: &Value, args: &[Value]) -> Result<Value> {
    let other = args
        .first()
        .ok_or_else(|| PyError::Runtime("set.issubset() requires 1 argument".to_string()))
        .and_then(|v| snapshot_iterable(receiver, v))?;
    receiver
        .set_with(|items| Value::bool_(items.iter().all(|k| other.contains(k))))
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "set.issubset receiver is not a set".to_string(),
            )
        })
}

fn issuperset(receiver: &Value, args: &[Value]) -> Result<Value> {
    let other = args
        .first()
        .ok_or_else(|| PyError::Runtime("set.issuperset() requires 1 argument".to_string()))
        .and_then(|v| snapshot_iterable(receiver, v))?;
    receiver
        .set_with(|items| Value::bool_(other.iter().all(|k| items.contains(k))))
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "set.issuperset receiver is not a set".to_string(),
            )
        })
}

fn isdisjoint(receiver: &Value, args: &[Value]) -> Result<Value> {
    let other = args
        .first()
        .ok_or_else(|| PyError::Runtime("set.isdisjoint() requires 1 argument".to_string()))
        .and_then(|v| snapshot_iterable(receiver, v))?;
    receiver
        .set_with(|items| Value::bool_(!items.iter().any(|k| other.contains(k))))
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "set.isdisjoint receiver is not a set".to_string(),
            )
        })
}

// ── key → Value conversion ────────────────────────────────────────────────────

fn key_to_value(k: PyKey) -> Value {
    match k {
        PyKey::Int(v) => Value::int(v),
        PyKey::BigInt(v) => Value::bigint(*v),
        PyKey::Float(bits) => Value::float(f64::from_bits(bits)),
        PyKey::Str(s) => s,
        PyKey::Bool(b) => Value::bool_(b),
        PyKey::None => Value::none(),
        PyKey::Ellipsis => Value::ellipsis(),
        PyKey::FrozenSet(key) => crate::frozenset::frozenset_key(key),
        PyKey::Tuple(items) => Value::tuple(items.into_iter().map(key_to_value).collect()),
        PyKey::Bytes(rc) => Value::bytes((*rc).clone()),
        PyKey::Complex(re, im) => Value::complex(re, im),
        PyKey::Object { value, .. } => value,
    }
}
