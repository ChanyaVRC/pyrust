use pyrust_core::{
    PyDict, PyError, Result, SortKind, StrKey, Value, ValueKind, classify_sort,
    compare_values_via_registry,
};

use crate::method_signature::{KeywordPolicy, PositionalArity};
use crate::mutable_sequence as ms;
use crate::sequence;

pub const TYPE_NAME: &str = "list";

/// Canonical list of method names dispatched by `call`.
pub const METHODS: &[&str] = &[
    "__iter__", "index", "count", "append", "clear", "copy", "extend", "insert", "pop", "remove",
    "reverse", "sort",
];

pub const CLASS_ATTRS: crate::primitive_class_attrs::PrimitiveClassAttrs =
    crate::primitive_class_attrs::PrimitiveClassAttrs::new(TYPE_NAME, METHODS).with_flags(
        crate::primitive_class_attrs::PrimitiveClassFlags::NONE
            .with_init()
            .with_class_getitem(),
    );

/// Returns `true` if `method` is the name of a built-in `list` method.
pub fn has_method(method: &str) -> bool {
    method_spec(method).is_some()
}

/// Typed list dispatch target. Name resolution, arity policy, and
/// interpreter-routing classification all derive from one [`MethodSpec`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Method {
    Iter,
    Index,
    Count,
    Append,
    Clear,
    Copy,
    Extend,
    Insert,
    Pop,
    Remove,
    Reverse,
    Sort,
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
    pub const fn interpreter_method(self) -> Option<InterpreterMethod> {
        match self.method {
            Method::Sort => Some(InterpreterMethod::Sort),
            Method::Index => Some(InterpreterMethod::Index),
            Method::Count => Some(InterpreterMethod::Count),
            Method::Remove => Some(InterpreterMethod::Remove),
            Method::Extend => Some(InterpreterMethod::Extend),
            _ => None,
        }
    }
}

impl Method {
    const fn name(self) -> &'static str {
        match self {
            Self::Iter => "__iter__",
            Self::Index => "index",
            Self::Count => "count",
            Self::Append => "append",
            Self::Clear => "clear",
            Self::Copy => "copy",
            Self::Extend => "extend",
            Self::Insert => "insert",
            Self::Pop => "pop",
            Self::Remove => "remove",
            Self::Reverse => "reverse",
            Self::Sort => "sort",
        }
    }
}

/// Resolve a list method exactly once into its semantic route and signature.
#[inline]
pub fn method_spec(method: &str) -> Option<MethodSpec> {
    let (method, arity, keywords) = match method {
        "__iter__" => (
            Method::Iter,
            PositionalArity::exact(0),
            KeywordPolicy::Reject,
        ),
        "index" => (
            Method::Index,
            PositionalArity::range(1, 3),
            KeywordPolicy::Reject,
        ),
        "count" => (
            Method::Count,
            PositionalArity::exact(1),
            KeywordPolicy::Reject,
        ),
        "append" => (
            Method::Append,
            PositionalArity::exact(1),
            KeywordPolicy::Reject,
        ),
        "clear" => (
            Method::Clear,
            PositionalArity::exact(0),
            KeywordPolicy::Reject,
        ),
        "copy" => (
            Method::Copy,
            PositionalArity::exact(0),
            KeywordPolicy::Reject,
        ),
        "extend" => (
            Method::Extend,
            PositionalArity::exact(1),
            KeywordPolicy::Reject,
        ),
        "insert" => (
            Method::Insert,
            PositionalArity::exact(2),
            KeywordPolicy::Reject,
        ),
        "pop" => (
            Method::Pop,
            PositionalArity::range(0, 1),
            KeywordPolicy::Reject,
        ),
        "remove" => (
            Method::Remove,
            PositionalArity::exact(1),
            KeywordPolicy::Reject,
        ),
        "reverse" => (
            Method::Reverse,
            PositionalArity::exact(0),
            KeywordPolicy::Reject,
        ),
        "sort" => (
            Method::Sort,
            PositionalArity::no_positional(),
            KeywordPolicy::Accept,
        ),
        _ => return None,
    };
    Some(MethodSpec {
        method,
        arity,
        keywords,
    })
}

/// Positional signature for every public list method.
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

/// Enforce list's keyword policy before positional conversion or method-body
/// work. Unknown names are left to attribute resolution.
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

/// Typed keyword slots accepted by `list.sort`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SortKeyword {
    Key,
    Reverse,
}

/// Resolve one `list.sort` keyword with CPython's Argument Clinic diagnostic.
pub fn sort_keyword(name: &str) -> Result<SortKeyword> {
    match name {
        "key" => Ok(SortKeyword::Key),
        "reverse" => Ok(SortKeyword::Reverse),
        _ => Err(PyError::named(
            "TypeError",
            format!("'{name}' is an invalid keyword argument for sort()"),
        )),
    }
}

pub fn validate_sort_keyword_count(given: usize) -> Result<()> {
    if given <= 2 {
        return Ok(());
    }
    Err(PyError::named(
        "TypeError",
        format!("sort() takes at most 2 keyword arguments ({given} given)"),
    ))
}

/// Interpreter-owned route for list methods that can invoke Python code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterpreterMethod {
    Sort,
    Index,
    Count,
    Remove,
    Extend,
}

/// Classify a list method that needs mutable interpreter access.
///
/// Returning a semantic route instead of a boolean keeps the Python names in
/// this built-in module; the VM does not need to compare the same name again
/// after deciding that the interpreter-owned path is required.
pub fn interpreter_method(method: &str) -> Option<InterpreterMethod> {
    method_spec(method).and_then(MethodSpec::interpreter_method)
}

/// Compatibility predicate for callers that have not migrated to the typed
/// [`InterpreterMethod`] route yet.
#[deprecated(since = "0.1.0", note = "use interpreter_method(method).is_some()")]
pub fn requires_interpreter(method: &str) -> bool {
    interpreter_method(method).is_some()
}

pub fn call(method: &str, receiver: &Value, args: Vec<Value>, kwargs: &PyDict) -> Result<Value> {
    let Some(spec) = method_spec(method) else {
        return Err(PyError::named(
            "AttributeError",
            format!("'list' object has no attribute '{method}'"),
        ));
    };
    spec.validate_keywords(!kwargs.is_empty())?;
    spec.validate_positional_arity(args.len())?;
    call_resolved(spec.method(), receiver, args, kwargs)
}

/// Dispatch a method whose name and signature were resolved together by
/// [`method_spec`].
#[doc(hidden)]
pub fn call_resolved(
    method: Method,
    receiver: &Value,
    args: Vec<Value>,
    kwargs: &PyDict,
) -> Result<Value> {
    match method {
        // Read-only sequence operations — borrow scoped to the call.
        Method::Index => receiver
            .list_with(|items| sequence::seq_index(items, &args, "list"))
            .ok_or_else(|| {
                PyError::named("TypeError", "list.index receiver is not a list".to_string())
            })?,
        Method::Count => receiver
            .list_with(|items| sequence::seq_count(items, &args, "list"))
            .ok_or_else(|| {
                PyError::named("TypeError", "list.count receiver is not a list".to_string())
            })?,
        // Mutable Sequence Operations — each ms::* takes &Value and
        // scopes its own borrow_mut().
        Method::Append => ms::append(receiver, args),
        Method::Clear => ms::clear(receiver, args),
        Method::Copy => ms::copy(receiver, args),
        Method::Extend => ms::extend(receiver, args),
        Method::Insert => ms::insert(receiver, args),
        Method::Pop => ms::pop(receiver, args),
        Method::Remove => ms::remove(receiver, args),
        Method::Reverse => ms::reverse(receiver, args),
        // List-specific
        Method::Sort => sort(receiver, &args, kwargs),
        // Intercepted by the interpreter's iteration domain; drift sentinel.
        Method::Iter => Err(PyError::named(
            "TypeError",
            "'list' __iter__ must be dispatched by the interpreter",
        )),
    }
}

fn sort(receiver: &Value, args: &[Value], kwargs: &PyDict) -> Result<Value> {
    let reverse_flag = extract_reverse(args, kwargs)?;
    sort_by_cmp(receiver, reverse_flag)
}

fn extract_reverse(args: &[Value], kwargs: &PyDict) -> Result<bool> {
    // StrKey probe (issue #506): zero-alloc borrowed-str lookup — no heap
    // allocation on every list.sort() call.
    Ok(
        match (
            args.first().map(|v| v.kind()),
            kwargs.get(&StrKey("reverse")).map(|v| v.kind()),
        ) {
            (_, Some(ValueKind::Bool(b))) => b,
            (_, Some(ValueKind::Int(0))) => false,
            (_, Some(v)) => {
                matches!(v, ValueKind::Int(n) if n != 0)
                    || matches!(v, ValueKind::Bool(true))
                    || matches!(v, ValueKind::Float(f) if f != 0.0)
            }
            (Some(ValueKind::Bool(b)), _) => b,
            _ => false,
        },
    )
}

/// Sort a key-less list with the `reverse` flag already resolved to a `bool`
/// by the interpreter (issue #2126).  The receiver-only `call("sort", …)` path
/// re-parses `reverse` via `extract_reverse`, which recognises only
/// Bool/Int/Float; CPython applies `bool(reverse)` to any object.  The
/// interpreter computes that truthiness (honouring user `__bool__`) and calls
/// here directly, matching `sorted()`.
pub fn sort_no_key(receiver: &Value, reverse: bool) -> Result<Value> {
    sort_by_cmp(receiver, reverse)
}

fn sort_by_cmp(receiver: &Value, reverse: bool) -> Result<Value> {
    let (kind, len) = receiver
        .list_with(|items| (classify_sort(items.iter()), items.len()))
        .ok_or_else(|| {
            PyError::named("TypeError", "list.sort receiver is not a list".to_string())
        })?;
    if len < 2 {
        return Ok(Value::none());
    }
    match kind {
        // These native comparators cannot call Python or raise, so sorting the
        // receiver in place is safe and avoids cloning the entire Vec.
        SortKind::AllInt => {
            receiver.list_with_mut(|items| {
                items.sort_by(|a, b| {
                    let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
                    lhs.as_int().unwrap_or(0).cmp(&rhs.as_int().unwrap_or(0))
                });
            });
            return Ok(Value::none());
        }
        SortKind::AllStr => {
            receiver.list_with_mut(|items| {
                items.sort_by(|a, b| {
                    let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
                    lhs.as_str().unwrap_or("").cmp(rhs.as_str().unwrap_or(""))
                });
            });
            return Ok(Value::none());
        }
        SortKind::HasInstance | SortKind::General => {}
    }

    // Snapshot the items into an owned Vec.  The comparator may call
    // user `__lt__` which can re-enter the same list — by working on
    // a snapshot we keep the receiver's borrow unscoped during the
    // sort, then write the result back inside a `list_with_mut`
    // borrow_mut window.  Matches the previous `items.clone() →
    // sort_by → restore on err` shape.
    let mut snapshot = receiver.list_with(|items| items.clone()).ok_or_else(|| {
        PyError::named("TypeError", "list.sort receiver is not a list".to_string())
    })?;
    let mut err: Option<PyError> = None;
    snapshot.sort_by(|a, b| {
        if err.is_some() {
            return std::cmp::Ordering::Equal;
        }
        let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
        match compare_values_via_registry(lhs, rhs) {
            Ok(ord) => ord,
            Err(e) => {
                err = Some(e);
                std::cmp::Ordering::Equal
            }
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    receiver.list_with_mut(|items| *items = snapshot);
    Ok(Value::none())
}

/// Sort items using precomputed keys (one key per item in the same order).
/// Called by the VM after evaluating each key function call.
pub fn sort_with_precomputed_keys(
    receiver: &Value,
    keys: Vec<Value>,
    reverse: bool,
) -> Result<Value> {
    let snapshot = receiver.list_with(|items| items.clone()).ok_or_else(|| {
        PyError::named("TypeError", "list.sort receiver is not a list".to_string())
    })?;
    debug_assert_eq!(snapshot.len(), keys.len());
    let mut keyed: Vec<(Value, Value)> = keys.into_iter().zip(snapshot).collect();
    // Classify by key: homogeneous all-int / all-str keys sort with a native
    // comparator; everything else keeps the general registry comparator.
    match classify_sort(keyed.iter().map(|(k, _)| k)) {
        SortKind::AllInt => keyed.sort_by(|(ka, _), (kb, _)| {
            let (lhs, rhs) = if reverse { (kb, ka) } else { (ka, kb) };
            lhs.as_int().unwrap_or(0).cmp(&rhs.as_int().unwrap_or(0))
        }),
        SortKind::AllStr => keyed.sort_by(|(ka, _), (kb, _)| {
            let (lhs, rhs) = if reverse { (kb, ka) } else { (ka, kb) };
            lhs.as_str().unwrap_or("").cmp(rhs.as_str().unwrap_or(""))
        }),
        _ => {
            let mut sort_err: Option<PyError> = None;
            keyed.sort_by(|(ka, _), (kb, _)| {
                if sort_err.is_some() {
                    return std::cmp::Ordering::Equal;
                }
                let (lhs, rhs) = if reverse { (kb, ka) } else { (ka, kb) };
                match compare_values_via_registry(lhs, rhs) {
                    Ok(ord) => ord,
                    Err(e) => {
                        sort_err = Some(e);
                        std::cmp::Ordering::Equal
                    }
                }
            });
            if let Some(e) = sort_err {
                return Err(e);
            }
        }
    }
    let new_items: Vec<Value> = keyed.into_iter().map(|(_, v)| v).collect();
    receiver.list_with_mut(|items| *items = new_items);
    Ok(Value::none())
}
