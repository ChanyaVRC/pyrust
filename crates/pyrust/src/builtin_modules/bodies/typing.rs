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
use crate::value::{PyClass, PyInstance, Value, ValueKind};
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
        Rc::new(RefCell::new(PyClass {
            name: "_Any".to_string(),
            qualname: "_Any".to_string(),
            base: None,
            extra_bases: vec![],
            attrs,
            mutation_version: std::cell::Cell::new(0),
        }))
    };

    static TYPING_ALIAS_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        for (method, reg_name) in TYPING_ALIAS_METHODS {
            attrs.insert((*method).to_string(), Value::builtin_function(reg_name));
        }
        Rc::new(RefCell::new(PyClass {
            name: "_TypingAlias".to_string(),
            qualname: "_TypingAlias".to_string(),
            base: None,
            extra_bases: vec![],
            attrs,
            mutation_version: std::cell::Cell::new(0),
        }))
    };
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
        // PEP 585: these names are deprecated aliases for built-in types.
        // Using the actual primitive classes means `List[int]` dispatches
        // through the existing `__class_getitem__` mechanism.
        "List"  => primitive_class_value("list"),
        "Dict"  => primitive_class_value("dict"),
        "Set"   => primitive_class_value("set"),
        "Tuple" => primitive_class_value("tuple"),

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
        "Type"     => class_value_for("Type"),
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
        Ok(Value::string("typing.Any".to_string()))
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
            .map(|v| v.repr())
            .unwrap_or_else(|| "...".to_string());
        Ok(Value::string(format!("typing.{form}[{args_repr}]")))
    }

    #[py_name = "_TypingAlias.__class_getitem__"]
    fn typing_alias_class_getitem(args) -> Result<Value> {
        let _ = _interp;
        let subscript = args.get(1).map(|a| a.value.clone()).unwrap_or_else(Value::none);
        Ok(make_typing_alias("_TypingAlias", subscript))
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn expect_self(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<RefCell<PyInstance>>> {
    match args.first().map(|a| a.value.kind()) {
        Some(ValueKind::PyInstance(rc)) => Ok(Rc::clone(&rc)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

/// Construct a `_TypingAlias` instance carrying `form` and `subscript`.
fn make_typing_alias(form: &str, subscript: Value) -> Value {
    TYPING_ALIAS_CLASS.with(|class| {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        attrs.insert("_form".to_string(), Value::string(form));
        attrs.insert("_args".to_string(), subscript);
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(class),
            attrs,
        })))
    })
}

/// Construct the `Any` singleton as a PyInstance of `_Any`.
fn make_any_instance() -> Value {
    ANY_CLASS.with(|class| {
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(class),
            attrs: IndexMap::new(),
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
            let class = Rc::new(RefCell::new(PyClass {
                name: (*name).to_string(),
                qualname: (*name).to_string(),
                base: None,
                extra_bases: vec![],
                attrs,
                mutation_version: std::cell::Cell::new(0),
            }));
            map.insert(*name, class);
        }
        map
    };
}

/// Helper to get a primitive-class Value by name.
fn primitive_class_value(name: &str) -> Value {
    match crate::interpreter::primitive_class_by_name(name) {
        Some(rc) => Value::py_class(rc),
        None => Value::none(),
    }
}
