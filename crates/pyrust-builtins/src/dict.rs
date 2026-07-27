use pyrust_core::{PyDict, PyError, PyKey, Result, Value, ValueKind};

use crate::method_signature::{KeywordPolicy, PositionalArity};

pub const TYPE_NAME: &str = "dict";

/// Canonical list of method names exposed for `dict`.
///
/// **Note** (#425): of these, `get`, `pop`, `setdefault`, and `__contains__`
/// are NOT dispatched by `call` below — `Interpreter::call_dict_method`
/// (`crates/pyrust/src/interpreter/runtime/expr.rs`) intercepts those four
/// before delegating, because they need to fire user-defined `__hash__` /
/// `__eq__` (#368) which an interpreter-free dispatcher can't do.
/// `has_method` still must report them (instance-attr `d.pop` resolution
/// goes through `builtin_has_method` → this list), so they stay listed
/// here.  The unreachable function bodies that used to live below them in
/// this file are gone (only the `match` arms in `call` were dead — see #425).
pub const METHODS: &[&str] = &[
    "__iter__",
    "get",
    "keys",
    "values",
    "items",
    "update",
    "pop",
    "popitem",
    "clear",
    "setdefault",
    "copy",
];

pub const CLASS_ATTRS: crate::primitive_class_attrs::PrimitiveClassAttrs =
    crate::primitive_class_attrs::PrimitiveClassAttrs::new(TYPE_NAME, METHODS)
        .with_native_class_methods(&["fromkeys"])
        .with_flags(
            crate::primitive_class_attrs::PrimitiveClassFlags::NONE
                .with_init()
                .with_class_getitem(),
        );

/// Returns `true` if `method` is the name of a built-in `dict` method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Method {
    Iter,
    Get,
    Keys,
    Values,
    Items,
    Update,
    Pop,
    Popitem,
    Clear,
    SetDefault,
    Copy,
    Contains,
    FromKeys,
}

impl Method {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Iter => "__iter__",
            Self::Get => "get",
            Self::Keys => "keys",
            Self::Values => "values",
            Self::Items => "items",
            Self::Update => "update",
            Self::Pop => "pop",
            Self::Popitem => "popitem",
            Self::Clear => "clear",
            Self::SetDefault => "setdefault",
            Self::Copy => "copy",
            Self::Contains => "__contains__",
            Self::FromKeys => "fromkeys",
        }
    }

    /// Whether a successful or partially-successful call can mutate the
    /// receiver. Interpreter adapters use this to synchronize special live
    /// namespace dictionaries after the canonical dict implementation returns.
    #[inline(always)]
    pub const fn mutates_receiver(self) -> bool {
        matches!(
            self,
            Self::Update | Self::Pop | Self::Popitem | Self::Clear | Self::SetDefault
        )
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

    #[inline(always)]
    pub const fn view_method(self) -> Option<ViewMethod> {
        match self.method {
            Method::Keys => Some(ViewMethod::Keys),
            Method::Values => Some(ViewMethod::Values),
            Method::Items => Some(ViewMethod::Items),
            _ => None,
        }
    }
}

/// Resolve a dict method once into its semantic route and positional policy.
#[inline]
pub fn method_spec(method: &str) -> Option<MethodSpec> {
    let (method, arity) = match method {
        "__iter__" => (Method::Iter, PositionalArity::exact(0)),
        "get" => (Method::Get, PositionalArity::range(1, 2)),
        "keys" => (Method::Keys, PositionalArity::exact(0)),
        "values" => (Method::Values, PositionalArity::exact(0)),
        "items" => (Method::Items, PositionalArity::exact(0)),
        "update" => (Method::Update, PositionalArity::range(0, 1)),
        "pop" => (Method::Pop, PositionalArity::range(1, 2)),
        "popitem" => (Method::Popitem, PositionalArity::exact(0)),
        "clear" => (Method::Clear, PositionalArity::exact(0)),
        "setdefault" => (Method::SetDefault, PositionalArity::range(1, 2)),
        "copy" => (Method::Copy, PositionalArity::exact(0)),
        "__contains__" => (Method::Contains, PositionalArity::exact(1)),
        "fromkeys" => (Method::FromKeys, PositionalArity::range(1, 2)),
        _ => return None,
    };
    let keywords = if method == Method::Update {
        KeywordPolicy::Accept
    } else {
        KeywordPolicy::Reject
    };
    Some(MethodSpec {
        method,
        arity,
        keywords,
    })
}

/// Positional signature for dict methods, including the interpreter-owned key
/// routes and the `fromkeys` classmethod.
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

/// Enforce dict's keyword policy. `update` consumes arbitrary keyword names as
/// entries; every other known method is positional-only.
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

/// Dict methods that produce a live view sharing the source backing storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewMethod {
    Keys,
    Values,
    Items,
}

/// Classify a method that produces a live dict view.
///
/// These three cannot go through `call` below — the interpreter-free
/// signature only sees a `Vec<Value>` snapshot, whereas a live view needs the
/// `Rc<RefCell<IndexMap>>`.
pub fn view_method(method: &str) -> Option<ViewMethod> {
    method_spec(method).and_then(MethodSpec::view_method)
}

/// Compatibility predicate for callers that have not migrated to the typed
/// [`ViewMethod`] route yet.
#[deprecated(since = "0.1.0", note = "use view_method(method).is_some()")]
pub fn needs_rc(method: &str) -> bool {
    view_method(method).is_some()
}

/// Dispatch a `dict` method.  Receiver is `&Value`; each branch
/// opens a scoped `dict_with` / `dict_with_mut` borrow.  Iterating
/// methods (`update`) snapshot the mapping arg via the receiver's
/// own scoped borrow when the arg aliases the receiver, so
/// `d.update(d)` never simultaneously borrows the same `IndexMap`
/// (#448).
pub fn call(method: &str, receiver: &Value, args: Vec<Value>, kwargs: &PyDict) -> Result<Value> {
    let Some(spec) = method_spec(method) else {
        return Err(PyError::named(
            "AttributeError",
            format!("'dict' object has no attribute '{method}'"),
        ));
    };
    spec.validate_keywords(!kwargs.is_empty())?;
    spec.validate_positional_arity(args.len())?;
    call_resolved(spec.method(), receiver, args, kwargs)
}

/// Dispatch a method whose name and arity were resolved by [`method_spec`].
#[doc(hidden)]
pub fn call_resolved(
    method: Method,
    receiver: &Value,
    args: Vec<Value>,
    kwargs: &PyDict,
) -> Result<Value> {
    let not_dict = || {
        PyError::named(
            "TypeError",
            "dict method receiver is not a dict".to_string(),
        )
    };
    match method {
        Method::Keys => receiver
            .dict_with(|dict| Value::list(dict.keys().cloned().map(key_to_value).collect()))
            .ok_or_else(not_dict),
        Method::Values => receiver
            .dict_with(|dict| Value::list(dict.values().cloned().collect()))
            .ok_or_else(not_dict),
        Method::Items => receiver
            .dict_with(|dict| {
                Value::list(
                    dict.iter()
                        .map(|(k, v)| Value::tuple(vec![key_to_value(k.clone()), v.clone()]))
                        .collect(),
                )
            })
            .ok_or_else(not_dict),
        Method::Update => {
            // CPython: `dict.update([mapping_or_iterable], **kwargs)` —
            // at most one positional arg.  >1 positional → TypeError.
            if args.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("update expected at most 1 argument, got {}", args.len()),
                ));
            }
            // Materialise the mapping snapshot BEFORE borrow_mut so
            // a self-aliased call (`d.update(d)`) reads its pre-
            // update state and doesn't `&` the storage we'd `&mut`.
            let mut snapshot = snapshot_update_arg(receiver, &args)?;
            // Keyword arguments are inserted after the positional arg,
            // matching CPython's order: positional mapping first, then kwargs.
            snapshot.extend(
                kwargs
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
            receiver.dict_extend(snapshot)?;
            Ok(Value::none())
        }
        Method::Popitem => receiver.dict_with_mut(popitem).ok_or_else(not_dict)?,
        Method::Clear => {
            receiver.dict_clear()?;
            Ok(Value::none())
        }
        Method::Copy => receiver
            .dict_with(|dict| Value::dict(dict.clone()))
            .ok_or_else(not_dict),
        _ => Err(PyError::named(
            "AttributeError",
            format!("'dict' object has no attribute '{}'", method.name()),
        )),
    }
}

/// Materialise the `update()` argument(s) into `(PyKey, Value)`
/// pairs.  When the arg aliases the receiver we snapshot the
/// receiver's contents via its own scoped read borrow; otherwise we
/// drain the mapping arg directly.
fn snapshot_update_arg(receiver: &Value, args: &[Value]) -> Result<Vec<(PyKey, Value)>> {
    let mut out = Vec::new();
    for arg in args {
        let aliased = arg.value_id() == receiver.value_id() && arg.value_id().is_some();
        if aliased {
            let snap = receiver
                .dict_with(|dict| {
                    dict.iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<Vec<_>>()
                })
                .ok_or_else(|| {
                    PyError::named(
                        "TypeError",
                        "dict.update receiver is not a dict".to_string(),
                    )
                })?;
            out.extend(snap);
            continue;
        }
        // Helper: handle a single key/value pair from the iterable form
        // (`[(k1, v1), (k2, v2), ...]`).  Each `pair` must itself be a
        // length-2 sequence.  Factored out so the `List`, `Tuple`, and
        // `Str` arms below can share it without combining their patterns.
        // `idx` is the 0-based element position within the outer iterable,
        // used to match CPython's error messages.
        fn push_pair(pair: &Value, idx: usize, out: &mut Vec<(PyKey, Value)>) -> Result<()> {
            let (len, kv): (usize, Vec<Value>) = match pair.kind() {
                ValueKind::List(items) => (items.len(), items.clone()),
                ValueKind::Tuple(items) => (items.len(), items.to_vec()),
                ValueKind::Str(s) => {
                    let chars: Vec<Value> =
                        s.chars().map(|c| Value::string(c.to_string())).collect();
                    (chars.len(), chars)
                }
                _ => {
                    // Non-sequence element: CPython raises TypeError here.
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "cannot convert dictionary update sequence element #{idx} to a sequence"
                        ),
                    ));
                }
            };
            if len != 2 {
                return Err(PyError::named(
                    "ValueError",
                    format!(
                        "dictionary update sequence element #{idx} has length {len}; 2 is required"
                    ),
                ));
            }
            let k = kv[0].to_key().ok_or_else(|| {
                PyError::named(
                    "TypeError",
                    format!(
                        "unhashable type: '{}'",
                        pyrust_core::builtin_type_name(&kv[0])
                    ),
                )
            })?;
            out.push((k, kv[1].clone()));
            Ok(())
        }
        match arg.kind() {
            ValueKind::Dict(other_map) => {
                for (k, v) in other_map.iter() {
                    out.push((k.clone(), v.clone()));
                }
            }
            ValueKind::List(items) => {
                for (idx, pair) in items.iter().enumerate() {
                    push_pair(pair, idx, &mut out)?;
                }
            }
            ValueKind::Tuple(items) => {
                for (idx, pair) in items.iter().enumerate() {
                    push_pair(pair, idx, &mut out)?;
                }
            }
            ValueKind::Str(s) => {
                // Strings are iterable (yield 1-char strings), but each
                // char is length 1, so push_pair will raise the
                // CPython-matching ValueError on element #0.
                for (idx, ch) in s.chars().enumerate() {
                    let char_val = Value::string(ch.to_string());
                    push_pair(&char_val, idx, &mut out)?;
                }
            }
            ValueKind::Bytes(rc) => {
                // Bytes are iterable (yield integers 0-255), but integers
                // are not sequences, so push_pair raises TypeError element #0.
                for (idx, b) in rc.iter().enumerate() {
                    let byte_val = Value::int(*b as i64);
                    push_pair(&byte_val, idx, &mut out)?;
                }
            }
            _ => {
                // Non-iterable argument: CPython propagates the TypeError
                // from the iterator protocol — `'X' object is not iterable`.
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object is not iterable",
                        pyrust_core::builtin_type_name(arg)
                    ),
                ));
            }
        }
    }
    Ok(out)
}

fn popitem(dict: &mut PyDict) -> Result<Value> {
    match dict.pop() {
        Some((k, v)) => Ok(Value::tuple(vec![key_to_value(k), v])),
        None => Err(PyError::named(
            "KeyError",
            "popitem(): dictionary is empty".to_string(),
        )),
    }
}

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
