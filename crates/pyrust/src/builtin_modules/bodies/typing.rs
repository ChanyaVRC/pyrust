// `typing` module — runtime stubs for the most-used typing names.
//
// pyrust does not perform static type checking.  These stubs exist so that
// `from typing import List, Dict, Optional, ...` followed by annotated
// assignments (`x: List[int] = [1, 2, 3]`) does not raise at runtime.
//
// ## Design
//
// - `List`, `Dict`, `Set`, `Tuple` are PEP 585 deprecated aliases for the
//   built-in primitive classes (`list`, `dict`, `set`, `tuple`).  These
//   already have `__class_getitem__` registered, so `List[int]` produces a
//   `types.GenericAlias` exactly like `list[int]` does.
//
// - `Any` is a singleton PyInstance.  The class is built via a thread-local
//   singleton (like `os.rs`'s `_Environ`) to avoid infinite recursion
//   during `module()` construction.
//
// - `Optional`, `Union`, `Callable`, `ClassVar`, `Final`, `Literal`, `Type`
//   are special forms — stub `PyClass` values built via thread-locals with
//   a `__class_getitem__` attr.  `expr.rs`'s sentinel path creates a
//   `GenericAlias` directly on subscript so no extra interpreter plumbing
//   is needed.
//
// - `Generic` and `Protocol` are real `PyClass` singletons so that they
//   can be used as class bases (`class Stack(Generic[T]): pass`).  pyrust's
//   `MakeClass` instruction requires every base to be `ValueKind::PyClass`;
//   the subscript `Generic[T]` returns the `Generic` class itself (not a
//   `_TypingAlias` instance) to satisfy that constraint.
//
// - `TypeVar` is a real class defined via the `class { … }` block so that
//   `TypeVar('T')` creates a `PyInstance` with `__name__ = 'T'`.
//
// - `cast(typ, val)` is a runtime no-op: returns `val`.
//
// - `overload` is a no-op decorator: returns the function unchanged.
//
// ## Why thread-locals instead of `module_class()`?
//
// `module_class(name)` calls `module()`, and `module()` is itself evaluating
// the constants block when we first call it.  Calling `module()` from inside
// constants would loop forever.  Instead we build the internal class singletons
// independently via `thread_local!` (same pattern as `os.rs::ENVIRON_CLASS`).
//
// Reference: <https://docs.python.org/3/library/typing.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::Interpreter;
use crate::value::{InstanceAttrs, PyClass, PyInstance, PyKey, Value, ValueKind};
use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

// ── thread-local class singletons ─────────────────────────────────────────────
//
// We build `_Any` and `_TypingAlias` as independent class singletons rather
// than going through `module().attrs["_Any"]` — the `module()` call during a
// constants-block evaluation would recurse infinitely.

thread_local! {
    static ANY_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        // FN_PREFIX for this module resolves to "typing." so method registration
        // names are "typing._Any.__repr__" etc.  We use the same convention.
        for (method, reg_name) in ANY_METHODS {
            attrs.insert((*method).to_string(), Value::builtin_function(reg_name));
        }
        Rc::new(RefCell::new(PyClass::new("_Any", "_Any", None, attrs)))
    };

    static TYPING_ALIAS_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        for (method, reg_name) in TYPING_ALIAS_METHODS {
            attrs.insert((*method).to_string(), Value::builtin_function(reg_name));
        }
        Rc::new(RefCell::new(PyClass::new(
            "_TypingAlias",
            "_TypingAlias",
            None,
            attrs,
        )))
    };

    // `Generic` must be a real `PyClass` so that `class Stack(Generic[T]): pass`
    // can use it as a class base.  The `__class_getitem__` is registered under
    // the name "typing._generic_cgi" which does NOT contain ".__class_getitem__",
    // so expr.rs calls our function rather than creating a sentinel GenericAlias.
    // Our function returns the Generic class itself, making `Generic[T]` a valid
    // class base.
    static GENERIC_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        attrs.insert(
            "__class_getitem__".to_string(),
            Value::builtin_function("typing._generic_cgi"),
        );
        Rc::new(RefCell::new(PyClass::new("Generic", "Generic", None, attrs)))
    };

    // `Protocol` follows the same pattern as `Generic`.
    static PROTOCOL_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        attrs.insert(
            "__class_getitem__".to_string(),
            Value::builtin_function("typing._protocol_cgi"),
        );
        Rc::new(RefCell::new(PyClass::new("Protocol", "Protocol", None, attrs)))
    };

    // `NamedTuple` is a real `PyClass` so it can serve as a class base
    // (`class Point(NamedTuple): x: int`).  Class creation detects the marker
    // attr `__pyrust_namedtuple_marker__` and rebuilds the class as a
    // `collections.namedtuple`; the functional call form is routed to the
    // Python helper `typing._namedtuple_functional`.  See
    // `calls.rs::exec_make_class` / `call_class_expanded`.
    static NAMEDTUPLE_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        attrs.insert(
            "__pyrust_namedtuple_marker__".to_string(),
            Value::bool_(true),
        );
        Rc::new(RefCell::new(PyClass::new(
            "NamedTuple",
            "NamedTuple",
            None,
            attrs,
        )))
    };
}

/// Identity check: is `class` the `typing.NamedTuple` marker?  Used by the
/// class-creation machinery (`calls.rs`) to detect `class X(NamedTuple): ...`
/// and the functional `NamedTuple('X', ...)` call.
pub(crate) fn is_namedtuple_marker(class: &Rc<RefCell<PyClass>>) -> bool {
    NAMEDTUPLE_CLASS.with(|nt| Rc::ptr_eq(class, nt))
}

/// The `typing.NamedTuple` marker class value (for the module constant).
pub(crate) fn namedtuple_marker_value() -> Value {
    NAMEDTUPLE_CLASS.with(|c| Value::py_class(Rc::clone(c)))
}

/// True if `class` is the `typing.Protocol` singleton or a subclass of it.
/// Used by `isinstance` to apply structural protocol checks (issue #2526).
pub(crate) fn is_protocol_subclass(class: &Rc<RefCell<PyClass>>) -> bool {
    PROTOCOL_CLASS.with(|p| crate::interpreter::class_is_subclass_of(class, p))
}

/// True if `class` is the bare `typing.Protocol` marker singleton itself
/// (not a user subclass).  `isinstance` skips the structural check for it so
/// `Protocol` keeps ordinary class semantics (issue #2526).
pub(crate) fn is_protocol_marker_class(class: &Rc<RefCell<PyClass>>) -> bool {
    PROTOCOL_CLASS.with(|p| Rc::ptr_eq(class, p))
}

/// Shared `__class_getitem__` body for the `Generic` / `Protocol` marker
/// classes (issue #2698).  `args[0]` is the receiver class (`cls`), `args[1]`
/// the subscript.
///
///   * If `cls` *is* the marker singleton (`Generic[T]` / `Protocol[T]`),
///     return the marker class unchanged so it remains a valid class base
///     (pyrust's `MakeClass` requires every base to be a `ValueKind::PyClass`).
///   * Otherwise `cls` is a user subclass (`class Stack(Generic[T])`) being
///     subscripted (`Stack[int]`); build a `GenericAlias` over the subclass so
///     `type(Stack[int]).__name__` is a generic-alias and its repr is
///     `__main__.Stack[int]`, matching CPython.
fn special_class_getitem(
    args: &[ExpandedCallArg],
    marker: Rc<RefCell<PyClass>>,
) -> Result<Value> {
    let cls = args.first().map(|a| a.value.clone());
    if let Some(cls_val) = &cls
        && let ValueKind::PyClass(cls_rc) = cls_val.kind()
        && Rc::ptr_eq(cls_rc, &marker)
    {
        return Ok(Value::py_class(Rc::clone(&marker)));
    }
    // Subclass subscript → GenericAlias over the subclass.
    let cls_val = cls.unwrap_or_else(|| Value::py_class(Rc::clone(&marker)));
    let index = args
        .get(1)
        .map(|a| a.value.clone())
        .unwrap_or_else(Value::none);
    let type_args = if matches!(index.kind(), ValueKind::Tuple(_)) {
        index
    } else {
        Value::tuple(vec![index])
    };
    Ok(pyrust_builtins::generic_alias::generic_alias(
        cls_val, type_args,
    ))
}

/// (method-short, registry-name) pairs for `_Any`.
const ANY_METHODS: &[(&str, &str)] = &[
    ("__repr__", "typing._Any.__repr__"),
    ("__init__", "typing._Any.__init__"),
];

/// (method-short, registry-name) pairs for `_TypingAlias`.
const TYPING_ALIAS_METHODS: &[(&str, &str)] = &[
    ("__repr__", "typing._TypingAlias.__repr__"),
    ("__init__", "typing._TypingAlias.__init__"),
    ("__class_getitem__", "typing._TypingAlias.__class_getitem__"),
];

pyrust_module! {
    constants {
        // PEP 585: these names are deprecated aliases for built-in types
        // (CPython's `typing._SpecialGenericAlias`).  They are *distinct*
        // objects from the underlying builtins — `typing.List is not list` —
        // with a `typing.`-prefixed repr (`typing.List`, `typing.List[int]`).
        // `isinstance`/`issubclass` delegate to the underlying builtin.
        "List"      => legacy_alias_value("List"),
        "Dict"      => legacy_alias_value("Dict"),
        "Set"       => legacy_alias_value("Set"),
        "FrozenSet" => legacy_alias_value("FrozenSet"),
        "Tuple"     => legacy_alias_value("Tuple"),

        // `Any` — special singleton.  Built via thread-local class to avoid
        // recursion during `module()` construction.
        "Any"   => make_any_instance(),

        // Subscriptable special forms — also class-based.  For `Optional`,
        // `Union`, etc., expose the PyClass itself; subscripting dispatches
        // through `__class_getitem__` on the class.
        "Optional" => class_value_for("Optional"),
        "Union"    => class_value_for("Union"),
        "Callable" => class_value_for("Callable"),
        "ClassVar" => class_value_for("ClassVar"),
        "Final"    => class_value_for("Final"),
        "Literal"  => class_value_for("Literal"),

        // `Type` is the deprecated alias for the built-in `type`; like the
        // PEP 585 aliases above it reprs as `typing.Type` and delegates
        // `isinstance`/`issubclass` to `type`.
        "Type"     => legacy_alias_value("Type"),

        // `Generic` and `Protocol` are real PyClass values (not _TypingAlias
        // instances) so they can serve as class bases.
        "Generic"  => GENERIC_CLASS.with(|c| Value::py_class(Rc::clone(c))),
        "Protocol" => PROTOCOL_CLASS.with(|c| Value::py_class(Rc::clone(c))),

        // `NamedTuple` marker class — usable as a class base and callable as a
        // factory.  Class creation rebuilds subclasses as namedtuples.
        "NamedTuple" => namedtuple_marker_value(),

        // `TYPE_CHECKING` is always `False` at runtime (PEP 563 / typing docs):
        // it is `True` only for static type checkers.  This lets the common
        // `if TYPE_CHECKING: import ...` guard import nothing at runtime.
        "TYPE_CHECKING" => Value::bool_(false),
    }

    // ── _Any dispatch fns ─────────────────────────────────────────────────────
    //
    // These are module-level fns registered under the names listed in
    // `ANY_METHODS`; they implement `_Any` instance dispatch.

    #[py_name = "_Any.__init__"]
    fn any_init(args) -> Result<Value> {
        let _ = (_interp, args);
        Ok(Value::none())
    }

    #[py_name = "_Any.__repr__"]
    fn any_repr(args) -> Result<Value> {
        let _ = (_interp, args);
        Ok(Value::string("typing.Any"))
    }

    // ── _TypingAlias dispatch fns ─────────────────────────────────────────────

    #[py_name = "_TypingAlias.__init__"]
    fn typing_alias_init(args) -> Result<Value> {
        let _ = (_interp, args);
        Ok(Value::none())
    }

    #[py_name = "_TypingAlias.__repr__"]
    fn typing_alias_repr(args) -> Result<Value> {
        let _ = _interp;
        let inst = expect_self(args, FN_NAME)?;
        let borrow = inst.borrow();
        let form = borrow
            .attrs
            .get("_form")
            .and_then(|v| match v.kind() {
                ValueKind::Str(s) => Some(s.to_string()),
                _ => None,
            })
            .unwrap_or_else(|| "_TypingAlias".to_string());
        let args_repr = borrow
            .attrs
            .get("_args")
            .map(|v| v.repr_raw())
            .unwrap_or_else(|| "...".to_string());
        Ok(Value::string(format!("typing.{form}[{args_repr}]")))
    }

    #[py_name = "_TypingAlias.__class_getitem__"]
    fn typing_alias_class_getitem(args) -> Result<Value> {
        let _ = _interp;
        let subscript = args.get(1).map(|a| a.value.clone()).unwrap_or_else(Value::none);
        Ok(make_typing_alias("_TypingAlias", subscript))
    }

    // ── Generic.__class_getitem__ ─────────────────────────────────────────────
    //
    // The name "typing._generic_cgi" does NOT contain ".__class_getitem__", so
    // pyrust's expr.rs subscript handler calls our function rather than
    // creating a sentinel GenericAlias.  `args[0]` is the receiver class (`cls`)
    // and `args[1]` is the subscript.
    //
    // Two cases (issue #2698):
    //   * `Generic[T]` — `cls` is `Generic` itself.  Return the `Generic` class
    //     so it stays a valid class base (`class Stack(Generic[T])`); pyrust's
    //     `MakeClass` requires every base to be a `ValueKind::PyClass`.
    //   * `Stack[int]` — a *subclass* of `Generic` inherits this method, so
    //     `cls` is `Stack`.  Subscripting a user generic must yield a
    //     `GenericAlias` (`__main__.Stack[int]`), matching CPython.

    #[py_name = "_generic_cgi"]
    fn generic_class_getitem(args) -> Result<Value> {
        let _ = _interp;
        special_class_getitem(args, GENERIC_CLASS.with(Rc::clone))
    }

    // ── Protocol.__class_getitem__ ────────────────────────────────────────────

    #[py_name = "_protocol_cgi"]
    fn protocol_class_getitem(args) -> Result<Value> {
        let _ = _interp;
        special_class_getitem(args, PROTOCOL_CLASS.with(Rc::clone))
    }

    // ── cast ─────────────────────────────────────────────────────────────────
    //
    // `cast(typ, val)` is a runtime no-op in CPython — it simply returns
    // `val` unchanged.  Type checkers use the annotation to narrow types.

    fn cast(args) -> Result<Value> {
        let _ = _interp;
        match args.get(1).map(|a| a.value.clone()) {
            Some(v) => Ok(v),
            None => Err(PyError::named(
                "TypeError",
                "cast() requires 2 positional arguments: 'typ' and 'val'".to_string(),
            )),
        }
    }

    // ── overload ─────────────────────────────────────────────────────────────
    //
    // `@overload` is a no-op decorator at runtime.  CPython registers the
    // decorated function internally but the last plain definition wins.
    // Returning the function unchanged preserves that behaviour.

    fn overload(args) -> Result<Value> {
        let _ = _interp;
        match args.first().map(|a| a.value.clone()) {
            Some(v) => Ok(v),
            None => Err(PyError::named(
                "TypeError",
                "overload() requires 1 argument".to_string(),
            )),
        }
    }

    // ── TypedDict ─────────────────────────────────────────────────────────────
    //
    // Minimal stub: raises NotImplementedError.  Sufficient to allow imports
    // without crashing when the module is imported but TypedDict is not called.

    #[py_name = "TypedDict"]
    fn typed_dict(args) -> Result<Value> {
        let _ = (_interp, args);
        Err(PyError::named(
            "NotImplementedError",
            "TypedDict is not yet fully implemented in pyrust".to_string(),
        ))
    }

    // ── TypeVar class ─────────────────────────────────────────────────────────
    //
    // `TypeVar('T')` returns a TypeVar instance with `__name__ = 'T'`.
    // This mirrors the surface of CPython's typing.TypeVar so that
    // `T = TypeVar('T')` followed by `class Stack(Generic[T]): pass` works.
    //
    // CPython signature: TypeVar(name, *constraints, bound=None,
    //                            covariant=False, contravariant=False)
    // - args[0]    = self
    // - args[1]    = name (required, positional)
    // - args[2..]  = positional constraint types (e.g. TypeVar('T', int, str))
    // - bound=     = optional keyword argument
    // - covariant=/contravariant= accepted and ignored (runtime no-op)

    class TypeVar {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            let inst = expect_self(args, FN_NAME)?;
            let name_val = args.get(1).map(|a| a.value.clone()).ok_or_else(|| {
                PyError::named(
                    "TypeError",
                    "TypeVar() requires at least 1 argument (the name)".to_string(),
                )
            })?;
            let name_str = match name_val.kind() {
                ValueKind::Str(s) => s.to_string(),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "TypeVar() name must be a string".to_string(),
                    ));
                }
            };

            // Collect positional constraints (args[2..] without a keyword name).
            let constraints: Vec<Value> = args[2..]
                .iter()
                .filter(|a| a.name.is_none())
                .map(|a| a.value.clone())
                .collect();

            // Find the `bound=` keyword argument.
            let bound = args
                .iter()
                .find(|a| a.name.as_deref() == Some("bound"))
                .map(|a| a.value.clone())
                .unwrap_or_else(Value::none);

            let mut borrow = inst.borrow_mut();
            borrow
                .attrs
                .insert("__name__", Value::string(name_str));
            borrow
                .attrs
                .insert("__constraints__", Value::tuple(constraints));
            borrow.attrs.insert("__bound__", bound);
            // CPython captures the *caller's* module on the TypeVar instance, so
            // `T = TypeVar('T')` at top level has `T.__module__ == '__main__'`.
            // The `TypeVar` *class* carries `__module__ == 'typing'` (#2745), so
            // without an instance-level override the instance would inherit
            // `'typing'`.  pyrust seeds every user class's `__module__` as
            // `'__main__'` (see `run_class_body`), so mirror that here.
            borrow
                .attrs
                .insert("__module__", Value::string("__main__"));
            Ok(Value::none())
        }

        fn __repr__(args) -> Result<Value> {
            let _ = _interp;
            let inst = expect_self(args, FN_NAME)?;
            let borrow = inst.borrow();
            let name = borrow
                .attrs
                .get("__name__")
                .and_then(|v| match v.kind() {
                    ValueKind::Str(s) => Some(s.to_string()),
                    _ => None,
                })
                .unwrap_or_else(|| "T".to_string());
            Ok(Value::string(name))
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn expect_self(args: &[ExpandedCallArg], fn_name: &str) -> Result<Rc<RefCell<PyInstance>>> {
    match args.first().map(|a| a.value.kind()) {
        Some(ValueKind::PyInstance(rc)) => Ok(Rc::clone(rc)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

/// Construct a `_TypingAlias` instance carrying `form` and `subscript`.
fn make_typing_alias(form: &str, subscript: Value) -> Value {
    TYPING_ALIAS_CLASS.with(|class| {
        let mut attrs = InstanceAttrs::new();
        attrs.insert("_form", Value::string(form));
        attrs.insert("_args", subscript);
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(class),
            attrs,
        })))
    })
}

/// Construct the `Any` singleton as a PyInstance of `_Any`.
fn make_any_instance() -> Value {
    ANY_CLASS.with(|class| {
        let mut attrs = InstanceAttrs::new();
        // `typing.Any.__module__ == "typing"` (issue #2745).  `Any` is a
        // singleton instance, so seed `__module__` on the instance rather than
        // on the `_Any` class.
        attrs.insert("__module__", Value::string("typing"));
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(class),
            attrs,
        })))
    })
}

// Build a PyClass with `__class_getitem__` for a special form.  Uses a
// thread-local rather than `module()` to avoid re-entrant calls during the
// constants block.  `expr.rs` treats any BuiltinFunction whose name contains
// `".__class_getitem__"` as a sentinel and creates a GenericAlias directly,
// so the function body is never actually invoked at runtime.
fn class_value_for(name: &str) -> Value {
    SPECIAL_FORM_CLASSES.with(|map| {
        map.get(name).cloned().map(Value::py_class).unwrap_or_else(Value::none)
    })
}

/// Build a `Union`/`Optional` alias from a subscript, normalising it the way
/// CPython's `_SpecialForm.__getitem__` does (issue #2524):
///
/// 1. `Optional[X]` is `Union[X, None]` — the lone arg is collected and
///    `NoneType` is appended.  `Optional` rejects a multi-element subscript
///    with the same `TypeError` CPython raises.
/// 2. Each arg that is itself a `Union`/`Optional` alias is *flattened* —
///    its `__args__` are spliced in rather than the nested alias surviving.
/// 3. `None` arguments are lowered to the `NoneType` class.
/// 4. Duplicate args are dropped, preserving first-seen order.
/// 5. A union of a single unique type collapses to that type
///    (`Union[int]` → `int`); an empty union is a `TypeError`.
///
/// The resulting alias always carries the `Union` class as its origin, so
/// `get_origin` reports `Union` and the repr reconstructs the `Optional[X]`
/// spelling for the two-arg `X | None` case.
pub(crate) fn union_or_optional_getitem(form: &str, index: Value) -> Result<Value> {
    let none_type = || primitive_class_value("NoneType");

    // Collect the raw subscript arguments.
    let raw: Vec<Value> = match index.kind() {
        ValueKind::Tuple(items) => {
            if form == "Optional" {
                // `Optional[int, str]` is a TypeError in CPython.
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "typing.Optional requires a single type. Got {}.",
                        index.repr_raw()
                    ),
                ));
            }
            items.to_vec()
        }
        _ => vec![index.clone()],
    };

    if form == "Optional" {
        // `Optional[X]` == `Union[X, None]`.
        let mut args = raw;
        args.push(Value::none());
        return build_union(args, none_type());
    }

    if raw.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "Cannot take a Union of no types.".to_string(),
        ));
    }
    build_union(raw, none_type())
}

/// Flatten, lower-`None`, de-duplicate, and collapse a list of `Union` args.
fn build_union(raw: Vec<Value>, none_type: Value) -> Result<Value> {
    let mut flat: Vec<Value> = Vec::with_capacity(raw.len());
    for arg in raw {
        // Lower `None` to `NoneType` first so nested-vs-bare `None` dedup
        // consistently.
        let arg = if arg.is_none() { none_type.clone() } else { arg };
        // Splice in the args of a nested `Union`/`Optional` alias.
        if let Some((origin, nested_args)) =
            pyrust_builtins::generic_alias::as_generic_alias_origin_args(&arg)
            && is_union_class(&origin)
            && let ValueKind::Tuple(items) = nested_args.kind()
        {
            for inner in items.iter() {
                push_unique(&mut flat, inner.clone());
            }
            continue;
        }
        push_unique(&mut flat, arg);
    }

    // A single-type union collapses to the type itself (`Union[int]` → `int`).
    if flat.len() == 1 {
        return Ok(flat.into_iter().next().unwrap());
    }

    Ok(pyrust_builtins::generic_alias::generic_alias(
        class_value_for("Union"),
        Value::tuple(flat),
    ))
}

/// Append `arg` to `acc` unless an equal value is already present, preserving
/// first-seen order (CPython de-dups union members).
fn push_unique(acc: &mut Vec<Value>, arg: Value) {
    if !acc.contains(&arg) {
        acc.push(arg);
    }
}

/// True if `v` is the `Union` special-form class singleton (the origin every
/// flattened union alias carries).
fn is_union_class(v: &Value) -> bool {
    if let ValueKind::PyClass(rc) = v.kind() {
        return SPECIAL_FORM_CLASSES
            .with(|map| map.get("Union").map(|u| Rc::ptr_eq(rc, u)).unwrap_or(false));
    }
    false
}

thread_local! {
    /// Map from special-form name → PyClass with `__class_getitem__` registered.
    static SPECIAL_FORM_CLASSES: std::collections::HashMap<&'static str, Rc<RefCell<PyClass>>> = {
        let names: &[(&str, &str)] = &[
            ("Optional", "typing.Optional.__class_getitem__"),
            ("Union",    "typing.Union.__class_getitem__"),
            ("Callable", "typing.Callable.__class_getitem__"),
            ("ClassVar", "typing.ClassVar.__class_getitem__"),
            ("Final",    "typing.Final.__class_getitem__"),
            ("Literal",  "typing.Literal.__class_getitem__"),
            ("Type",     "typing.Type.__class_getitem__"),
        ];
        let mut map = std::collections::HashMap::new();
        for (name, reg_name) in names {
            let mut attrs: IndexMap<String, Value> = IndexMap::new();
            attrs.insert(
                "__class_getitem__".to_string(),
                Value::builtin_function(reg_name),
            );
            // Every special-form subscript reprs with a `typing.` prefix
            // (`typing.Union[int, str]`, `typing.Final[int]`,
            // `typing.Callable[[int], str]`, …).  The `GenericAlias` repr
            // qualifies a class origin with its `__module__`, so seed it here
            // for all special forms.  (The flatten helper always uses the
            // `Union` class as the union alias origin.)
            attrs.insert("__module__".to_string(), Value::string("typing"));
            let mut pyclass = PyClass::new(*name, *name, None, attrs);
            // The bare special form reprs as `typing.<name>` (e.g.
            // `repr(typing.Union) == "typing.Union"`), not the default
            // `<class 'typing.Union'>`.  Set on the dedicated `override_repr`
            // field, not in `attrs`, so it mirrors CPython's `_SpecialForm`
            // repr without being hijackable via `__dict__` (issue #2608).
            pyclass.override_repr = Some(format!("typing.{name}").into_boxed_str());
            let class = Rc::new(RefCell::new(pyclass));
            map.insert(*name, class);
        }
        map
    };
}

thread_local! {
    /// Map from legacy-alias name (`List`, `Dict`, …) → its dedicated
    /// `_SpecialGenericAlias`-style `PyClass` singleton.  Each carries:
    ///   * `qualname`/`name` = the alias name (`List`),
    ///   * `__module__` = `"typing"` so the `GenericAlias` repr qualifies a
    ///     subscript as `typing.List[int]`,
    ///   * `override_repr` = `"typing.List"` so the *bare* class reprs as
    ///     `typing.List` (not `<class 'typing.List'>`).  This is a dedicated
    ///     `PyClass` field, not a `__dict__` attr, so a user class cannot hijack
    ///     its own repr (issue #2608),
    ///   * `__class_getitem__` sentinel so `List[int]` builds a `GenericAlias`
    ///     with this class as origin,
    ///   * `__pyrust_legacy_alias_of__` = the underlying builtin name
    ///     (`"list"`) so `isinstance`/`issubclass` delegate to that builtin.
    static LEGACY_ALIAS_CLASSES: std::collections::HashMap<&'static str, Rc<RefCell<PyClass>>> = {
        // (alias name, underlying builtin name)
        let names: &[(&str, &str)] = &[
            ("List", "list"),
            ("Dict", "dict"),
            ("Set", "set"),
            ("FrozenSet", "frozenset"),
            ("Tuple", "tuple"),
            ("Type", "type"),
        ];
        let mut map = std::collections::HashMap::new();
        for (name, builtin) in names {
            let name = *name;
            let mut attrs: IndexMap<String, Value> = IndexMap::new();
            attrs.insert(
                "__class_getitem__".to_string(),
                Value::builtin_function(legacy_alias_cgi_name(name)),
            );
            attrs.insert("__module__".to_string(), Value::string("typing"));
            // The underlying builtin this alias delegates to, stored as the
            // class value itself so both `legacy_alias_delegate` (native) and
            // `get_origin` (Python) can normalise to it directly.
            attrs.insert(
                "__pyrust_legacy_alias_of__".to_string(),
                legacy_builtin_class_value(builtin),
            );
            let mut pyclass = PyClass::new(name, name, None, attrs);
            // Verbatim bare-class repr (`typing.List`).  Set on the dedicated
            // field, not in `attrs`, so user classes can't hijack it (#2608).
            pyclass.override_repr = Some(format!("typing.{name}").into_boxed_str());
            let class = Rc::new(RefCell::new(pyclass));
            map.insert(name, class);
        }
        map
    };
}

/// Registry name for a legacy alias's `__class_getitem__` sentinel.  The name
/// contains `.__class_getitem__`, which makes `expr.rs`'s subscript handler
/// build a `GenericAlias` (origin = this class) directly rather than invoking
/// the function — so the function body is never called.
fn legacy_alias_cgi_name(name: &str) -> &'static str {
    match name {
        "List" => "typing.List.__class_getitem__",
        "Dict" => "typing.Dict.__class_getitem__",
        "Set" => "typing.Set.__class_getitem__",
        "FrozenSet" => "typing.FrozenSet.__class_getitem__",
        "Tuple" => "typing.Tuple.__class_getitem__",
        "Type" => "typing.Type.__class_getitem__",
        _ => "typing._legacy.__class_getitem__",
    }
}

/// The module-constant `Value` for a legacy alias (`typing.List`, …).
fn legacy_alias_value(name: &str) -> Value {
    LEGACY_ALIAS_CLASSES.with(|map| {
        map.get(name)
            .cloned()
            .map(Value::py_class)
            .unwrap_or_else(Value::none)
    })
}

/// Resolve a legacy-alias builtin name (`"list"`, `"type"`, …) to its
/// `PyClass` value.  `type` lives in the metaclass singleton; the rest are
/// primitive classes.
fn legacy_builtin_class_value(builtin: &str) -> Value {
    if builtin == "type" {
        return Value::py_class(crate::interpreter::type_class_singleton());
    }
    crate::interpreter::primitive_class_by_name(builtin)
        .map(Value::py_class)
        .unwrap_or_else(Value::none)
}

/// If `class` is one of the legacy typing aliases (`typing.List`, …), return
/// the `PyClass` value of the underlying builtin it delegates to (`list`, …).
/// Used by `isinstance`/`issubclass` so checks against the alias behave like
/// checks against the builtin.  Returns `None` for any other class.
pub(crate) fn legacy_alias_delegate(class: &Rc<RefCell<PyClass>>) -> Option<Value> {
    // Confirm pointer identity against a singleton (not just the marker attr)
    // so a user class that happens to set the attr cannot hijack delegation.
    let is_ours = LEGACY_ALIAS_CLASSES.with(|map| map.values().any(|c| Rc::ptr_eq(c, class)));
    if !is_ours {
        return None;
    }
    class.borrow().attrs.get("__pyrust_legacy_alias_of__").cloned()
}

/// Helper to get a primitive-class Value by name.
fn primitive_class_value(name: &str) -> Value {
    match crate::interpreter::primitive_class_by_name(name) {
        Some(rc) => Value::py_class(rc),
        None => Value::none(),
    }
}

// ── Python-source members (issue #2516) ───────────────────────────────────────
//
// Members that are most naturally expressed in Python (`get_type_hints`,
// `get_origin`, `get_args`, `runtime_checkable`, `reveal_type`, the
// `Self`/`Never`/`Annotated`/… special-form markers, `ParamSpec`,
// `TypeVarTuple`, …) live in `typing_py.py`.  They are exec'd once into a
// throwaway namespace at first import of `typing`, and the resulting public
// names are copied onto the module — mirroring `collections::inject_python_members`.

/// Python-source definitions for the runtime helpers and special-form markers.
const TYPING_PY_SOURCE: &str = include_str!("typing_py.py");

/// Public names defined by `TYPING_PY_SOURCE` to export onto the module.
/// Private helpers (`_resolve`, `_namedtuple_functional`,
/// `_build_namedtuple_class`, `_SpecialMarker`) are intentionally omitted.
const TYPING_PY_EXPORTS: &[&str] = &[
    "get_origin",
    "get_args",
    "get_type_hints",
    "runtime_checkable",
    "final",
    "no_type_check",
    "reveal_type",
    "assert_never",
    "assert_type",
    "dataclass_transform",
    "get_overloads",
    "clear_overloads",
    "Self",
    "Never",
    "LiteralString",
    "Annotated",
    "TypeAlias",
    "Concatenate",
    "Unpack",
    "Required",
    "NotRequired",
    "TypeGuard",
    "ParamSpec",
    "TypeVarTuple",
];

/// Exec `TYPING_PY_SOURCE` once and copy its public names onto the `typing`
/// module.  The native special forms (`Optional`, `Union`, `Type`, …) are
/// pre-seeded into the exec namespace so the Python helpers can reference them
/// by identity (e.g. `get_origin` normalising `Optional`/`Union` origins).
/// `_namedtuple_functional` and `_build_namedtuple_class` are also retained on
/// the module under their private names for the native `NamedTuple` paths.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = Value::dict(crate::value::PyDict::default());
    // Seed the exec namespace with the native special-form names the helpers
    // reference by identity.
    for name in [
        "Optional", "Union", "Type", "Callable", "ClassVar", "Final", "Literal",
    ] {
        let v = module.borrow().attrs.get(name).cloned();
        if let Some(v) = v {
            ns.dict_insert(PyKey::str_from(name), v)?;
        }
    }
    // `typing.TypeVar` is a macro-built `PyClass` whose attrs only carry its
    // methods; the macro has no facility to seed `__module__`.  Set it here so
    // `typing.TypeVar.__module__ == "typing"` and the bare class reprs as
    // `<class 'typing.TypeVar'>` (issue #2745), mirroring Generic/Protocol.
    let type_var = module.borrow().attrs.get("TypeVar").cloned();
    if let Some(tv) = type_var
        && let ValueKind::PyClass(class) = tv.kind()
    {
        class
            .borrow_mut()
            .attrs
            .insert("__module__".to_string(), Value::string("typing"));
    }
    interp.exec_source(TYPING_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("typing: exec namespace not a dict".into()))?;
    let mut exports: Vec<&str> = TYPING_PY_EXPORTS.to_vec();
    // Private helpers consumed by the native NamedTuple paths.
    exports.push("_namedtuple_functional");
    exports.push("_build_namedtuple_class");
    for name in exports {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(())
}
