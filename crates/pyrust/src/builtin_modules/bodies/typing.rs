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
// - `Any` is a singleton PyInstance within one imported module generation.
//
// - `Optional`, `Union`, `Callable`, `ClassVar`, `Final`, `Literal`, `Type`
//   are special forms — stub `PyClass` values built per module generation with
//   a `__class_getitem__` attr.  `expr.rs`'s sentinel path creates a
//   `GenericAlias` directly on subscript so no extra interpreter plumbing
//   is needed.
//
// - `Generic` and `Protocol` are real `PyClass` values so that they
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
// ## Module generations
//
// `module_class(name)` calls `module()`, and `module()` is itself evaluating
// the constants block when we first call it.  Calling `module()` from inside
// constants would loop forever. A module-owned weak registry therefore tracks
// the synthetic classes of each import generation without keeping discarded
// modules alive. CPython 3.12 keeps `Generic` stable, but recreates `Any`,
// `Protocol`, its internal alias classes, special forms, and legacy aliases.
//
// Reference: <https://docs.python.org/3/library/typing.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::value::{PyInstance, Value, ValueKind};
use pyrust_derive::pyrust_module;

#[path = "typing/aliases.rs"]
mod aliases;
#[path = "typing/generation.rs"]
mod generation;
#[path = "typing/python_members.rs"]
mod python_members;

pub(crate) use aliases::legacy_alias_delegate;
pub(crate) use generation::{is_protocol_marker_class, is_protocol_subclass};
pub(crate) use python_members::inject_python_members;

enum MappingProxySubscriptPolicy {
    Accept,
    Reject(&'static str),
}

/// Return the CPython mapping-slot policy for synthetic bare `typing` values.
///
/// PyRust represents these values differently from CPython, so their visible
/// class shape cannot by itself model `PyMapping_Check` or its error type name
/// at this boundary.
fn mapping_proxy_policy(value: &Value) -> Option<MappingProxySubscriptPolicy> {
    if generation::is_annotated_marker(value) {
        return Some(MappingProxySubscriptPolicy::Reject("type"));
    }
    let ValueKind::PyClass(class) = value.kind() else {
        return None;
    };
    if generation::is_legacy_alias_class(class) || generation::is_special_form_class(class) {
        return Some(MappingProxySubscriptPolicy::Accept);
    }
    None
}

pub(crate) fn mapping_proxy_subscript_policy(value: &Value) -> Option<bool> {
    match mapping_proxy_policy(value) {
        Some(MappingProxySubscriptPolicy::Accept) => Some(true),
        Some(MappingProxySubscriptPolicy::Reject(_)) => Some(false),
        None => None,
    }
}

pub(crate) fn mapping_proxy_rejection_type_name(value: &Value) -> Option<&'static str> {
    match mapping_proxy_policy(value) {
        Some(MappingProxySubscriptPolicy::Reject(type_name)) => Some(type_name),
        Some(MappingProxySubscriptPolicy::Accept) | None => None,
    }
}

pyrust_module! {
    constants {
        // PEP 585: these names are deprecated aliases for built-in types
        // (CPython's `typing._SpecialGenericAlias`).  They are *distinct*
        // objects from the underlying builtins — `typing.List is not list` —
        // with a `typing.`-prefixed repr (`typing.List`, `typing.List[int]`).
        // `isinstance`/`issubclass` delegate to the underlying builtin.
        // The first constant starts a fresh generation; subsequent constants
        // use the same latest registry entry.
        "List"      => aliases::start_generation_legacy_alias_value("List"),
        "Dict"      => aliases::legacy_alias_value("Dict"),
        "Set"       => aliases::legacy_alias_value("Set"),
        "FrozenSet" => aliases::legacy_alias_value("FrozenSet"),
        "Tuple"     => aliases::legacy_alias_value("Tuple"),

        // `Any` — special singleton within this module generation.
        "Any"   => aliases::make_any_instance(),

        // `_GenericAlias` is a public private attribute in CPython and is
        // recreated with its owning module generation. `_TypingAlias` remains
        // internal on both runtimes, but is tracked by the same registry.
        "_GenericAlias" => Value::py_class(generation::current_generic_alias_class()),

        // Subscriptable special forms — also class-based.  For `Optional`,
        // `Union`, etc., expose the PyClass itself; subscripting dispatches
        // through `__class_getitem__` on the class.
        "Optional" => aliases::class_value_for("Optional"),
        "Union"    => aliases::class_value_for("Union"),
        "Callable" => aliases::class_value_for("Callable"),
        "ClassVar" => aliases::class_value_for("ClassVar"),
        "Final"    => aliases::class_value_for("Final"),
        "Literal"  => aliases::class_value_for("Literal"),

        // `Type` is the deprecated alias for the built-in `type`; like the
        // PEP 585 aliases above it reprs as `typing.Type` and delegates
        // `isinstance`/`issubclass` to `type`.
        "Type"     => aliases::legacy_alias_value("Type"),

        // `Generic` and `Protocol` are real PyClass values (not _TypingAlias
        // instances) so they can serve as class bases.
        "Generic"  => Value::py_class(generation::current_generic_class()),
        "Protocol" => Value::py_class(generation::current_protocol_class()),

        // Module-generation markers usable as class bases and functional
        // factories. Their typed weak registry is interpreter-free; generic
        // class/call routing consumes only the adapter result.
        "NamedTuple" => Value::py_class(generation::current_namedtuple_marker_class()),

        "TypedDict" => Value::py_class(generation::current_typeddict_marker_class()),

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
        Ok(aliases::make_typing_alias("_TypingAlias", subscript))
    }

    // ── _GenericAlias dispatch fns ────────────────────────────────────────────

    #[py_name = "_GenericAlias.__init__"]
    fn generic_alias_init(args) -> Result<Value> {
        let _ = (_interp, args);
        Ok(Value::none())
    }

    #[py_name = "_GenericAlias.__call__"]
    fn generic_alias_call(args) -> Result<Value> {
        // `Stack[int](...)` constructs an instance of the origin class, dropping
        // the type arguments (CPython's `_GenericAlias.__call__`).  `args[0]` is
        // the alias instance; the remaining positionals/keywords are forwarded
        // to the origin constructor unchanged.
        let inst = expect_self(args, FN_NAME)?;
        let origin = inst
            .borrow()
            .attrs
            .get("__origin__")
            .cloned()
            .unwrap_or_else(|| Value::py_class(generation::current_generic_class()));
        _interp.call_function_expanded(origin, &args[1..])
    }

    #[py_name = "_GenericAlias.__repr__"]
    fn generic_alias_repr(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let (origin_repr, sub) = {
            let borrow = inst.borrow();
            // The origin is the receiver class.  CPython's `_type_repr` shows
            // `typing.Generic` / `typing.Protocol` for the typing singletons but
            // a bare class name for a user subclass (`Stack[int]`, not
            // `typing.Stack[int]`).
            let origin_repr = borrow
                .attrs
                .get("_origin")
                .map(aliases::generic_origin_repr)
                .unwrap_or_else(|| "typing.Generic".to_string());
            let sub = borrow.attrs.get("_args").cloned().unwrap_or_else(Value::none);
            (origin_repr, sub)
        };
        // The subscript may be a single arg (`Generic[T]`) or a tuple
        // (`Generic[T, U]`).  Each element is rendered via `_type_repr` so a
        // plain class shows its name (`int`, not `<class 'int'>`) and a
        // `TypeVar` shows its variance prefix (`~T`).
        let args_repr = match sub.kind() {
            ValueKind::Tuple(items) => items
                .iter()
                .map(|v| aliases::generic_arg_repr(_interp, v))
                .collect::<Result<Vec<_>>>()?
                .join(", "),
            _ => aliases::generic_arg_repr(_interp, &sub)?,
        };
        Ok(Value::string(format!("{origin_repr}[{args_repr}]")))
    }

    #[py_name = "_GenericAlias.__mro_entries__"]
    fn generic_alias_mro_entries(args) -> Result<Value> {
        let _ = _interp;
        let instance = expect_self(args, FN_NAME)?;
        let user_args = &args[1..];
        let positional_count = user_args
            .iter()
            .filter(|argument| argument.name.is_none())
            .count();
        let has_bases_keyword = user_args
            .iter()
            .any(|argument| argument.name.as_deref() == Some("bases"));

        // `_GenericAlias.__mro_entries__` is an ordinary Python method with
        // signature `(self, bases)`. The runtime supplies `self`, so validate
        // the remaining expanded arguments here before reading the origin.
        // `bases` is intentionally unused by CPython's implementation, but it
        // is still required and may be passed by keyword.
        if user_args
            .iter()
            .any(|argument| argument.name.as_deref() == Some("self"))
        {
            return Err(pyrust_core::type_err!(
                "_GenericAlias.__mro_entries__() got multiple values for argument 'self'"
            ));
        }
        if positional_count > 0 && has_bases_keyword {
            return Err(pyrust_core::type_err!(
                "_GenericAlias.__mro_entries__() got multiple values for argument 'bases'"
            ));
        }
        if let Some(name) = user_args
            .iter()
            .filter_map(|argument| argument.name.as_deref())
            .find(|name| *name != "bases")
        {
            return Err(pyrust_core::type_err!(
                "_GenericAlias.__mro_entries__() got an unexpected keyword argument '{name}'"
            ));
        }
        if positional_count > 1 {
            return Err(pyrust_core::type_err!(
                "_GenericAlias.__mro_entries__() takes 2 positional arguments but {} were given",
                positional_count + 1
            ));
        }
        if positional_count == 0 && !has_bases_keyword {
            return Err(pyrust_core::type_err!(
                "_GenericAlias.__mro_entries__() missing 1 required positional argument: 'bases'"
            ));
        }
        let origin = instance
            .borrow()
            .attrs
            .get("_origin")
            .cloned()
            .ok_or_else(|| {
                PyError::Runtime("typing._GenericAlias has no origin".to_string())
            })?;
        Ok(Value::tuple(vec![origin]))
    }

    // ── Generic.__class_getitem__ ─────────────────────────────────────────────
    //
    // The name "typing._generic_cgi" does NOT contain ".__class_getitem__", so
    // pyrust's expr.rs subscript handler calls our function rather than
    // creating a sentinel GenericAlias.  Both the direct call
    // (`Generic.__class_getitem__(T)`) and the subscript form (`Generic[T]`)
    // route here and build a `_GenericAlias` wrapping the subscript, matching
    // CPython (`typing.Generic[~T]`, type `_GenericAlias`).  The class-base path
    // (`class Stack(Generic[T])`) unwraps the alias back to `Generic` via
    // `generic_alias_origin`.

    #[py_name = "_generic_cgi"]
    fn generic_class_getitem(args) -> Result<Value> {
        // The subscript form `Generic[T]` (or `Stack[T]` inheriting via MRO)
        // calls us with the receiver class prepended (`[Cls, T]`), while the
        // direct call `Generic.__class_getitem__(T)` passes only `[T]`.  The
        // origin of the resulting `_GenericAlias` must be the receiver class so
        // `Stack[int]` reprs as `Stack[int]` (origin = `Stack`) rather than
        // `Generic[int]`.  Fall back to `Generic` for the direct-call form.
        let positionals: Vec<&ExpandedCallArg> =
            args.iter().filter(|a| a.name.is_none()).collect();
        let (origin, subscript) = if positionals.len() >= 2 {
            (
                positionals[0].value.clone(),
                positionals[positionals.len() - 1].value.clone(),
            )
        } else {
            (
                Value::py_class(generation::current_generic_class()),
                positionals
                    .first()
                    .map(|a| a.value.clone())
                    .unwrap_or_else(Value::none),
            )
        };
        aliases::make_generic_alias(_interp, origin, subscript)
    }

    // ── Protocol.__class_getitem__ ────────────────────────────────────────────

    #[py_name = "_protocol_cgi"]
    fn protocol_class_getitem(args) -> Result<Value> {
        // Mirror `_generic_cgi`: use the receiver class as the alias origin so a
        // subclass `class P(Protocol[T])` / `P[int]` keeps `P` as origin.  The
        // direct `Protocol.__class_getitem__(T)` form falls back to `Protocol`.
        let positionals: Vec<&ExpandedCallArg> =
            args.iter().filter(|a| a.name.is_none()).collect();
        let (origin, subscript) = if positionals.len() >= 2 {
            (
                positionals[0].value.clone(),
                positionals[positionals.len() - 1].value.clone(),
            )
        } else {
            (
                Value::py_class(generation::current_protocol_class()),
                positionals
                    .first()
                    .map(|a| a.value.clone())
                    .unwrap_or_else(Value::none),
            )
        };
        aliases::make_generic_alias(_interp, origin, subscript)
    }

    // Special forms own their subscript normalization. Their class slots point
    // at these registered callables, so generic expression dispatch does not
    // need to recognize `typing.Union` or `typing.Optional` by name.

    #[py_name = "_optional_cgi"]
    fn optional_class_getitem(args) -> Result<Value> {
        let _ = _interp;
        aliases::special_form_class_getitem("Optional", args)
    }

    #[py_name = "_union_cgi"]
    fn union_class_getitem(args) -> Result<Value> {
        let _ = _interp;
        aliases::special_form_class_getitem("Union", args)
    }

    #[py_name = "_callable_cgi"]
    fn callable_class_getitem(args) -> Result<Value> {
        let _ = _interp;
        aliases::special_form_class_getitem("Callable", args)
    }

    #[py_name = "_classvar_cgi"]
    fn classvar_class_getitem(args) -> Result<Value> {
        let _ = _interp;
        aliases::special_form_class_getitem("ClassVar", args)
    }

    #[py_name = "_final_cgi"]
    fn final_class_getitem(args) -> Result<Value> {
        let _ = _interp;
        aliases::special_form_class_getitem("Final", args)
    }

    #[py_name = "_literal_cgi"]
    fn literal_class_getitem(args) -> Result<Value> {
        let _ = _interp;
        aliases::special_form_class_getitem("Literal", args)
    }

    #[py_name = "_type_cgi"]
    fn type_class_getitem(args) -> Result<Value> {
        let _ = _interp;
        aliases::special_form_class_getitem("Type", args)
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

    // ── TypeVar class ─────────────────────────────────────────────────────────
    //
    // `TypeVar('T')` returns a TypeVar instance with `__name__ = 'T'`.
    // This mirrors the surface of CPython's typing.TypeVar so that
    // `T = TypeVar('T')` followed by `class Stack(Generic[T]): pass` works.
    //
    // CPython signature: TypeVar(name, *constraints, bound=None,
    //                            covariant=False, contravariant=False,
    //                            infer_variance=False)
    // - args[0]    = self
    // - args[1]    = name (required, positional)
    // - args[2..]  = positional constraint types (e.g. TypeVar('T', int, str))
    // - bound=     = optional keyword argument
    // - variance flags are converted with Python's truth protocol

    class TypeVar {
        fn __init__(args) -> Result<Value> {
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

            // Variance flags drive the variance prefix in repr:
            // explicit covariance/contravariance render as +T/-T, the
            // invariant default as ~T, and inferred variance as plain T.
            let mut kw_flag = |name: &str| -> Result<bool> {
                match args.iter().find(|a| a.name.as_deref() == Some(name)) {
                    Some(arg) => _interp.truthy_value(&arg.value),
                    None => Ok(false),
                }
            };
            let covariant = kw_flag("covariant")?;
            let contravariant = kw_flag("contravariant")?;
            let infer_variance = kw_flag("infer_variance")?;
            // CPython rejects a TypeVar that is both covariant and
            // contravariant (`ValueError: Bivariant types are not supported.`).
            if covariant && contravariant {
                return Err(PyError::named(
                    "ValueError",
                    "Bivariant types are not supported.".to_string(),
                ));
            }
            if infer_variance && (covariant || contravariant) {
                return Err(PyError::named(
                    "ValueError",
                    "Variance cannot be specified with infer_variance.".to_string(),
                ));
            }

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
            borrow
                .attrs
                .insert("__covariant__", Value::bool_(covariant));
            borrow
                .attrs
                .insert("__contravariant__", Value::bool_(contravariant));
            borrow
                .attrs
                .insert("__infer_variance__", Value::bool_(infer_variance));
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
            // CPython prefixes explicit variance: + covariant, - contravariant,
            // ~ invariant; inferred variance is rendered without a prefix.
            let flag = |attr: &str| {
                borrow
                    .attrs
                    .get(attr)
                    .map(|v| v.truthy_raw())
                    .unwrap_or(false)
            };
            let rendered = if flag("__infer_variance__") {
                name
            } else if flag("__covariant__") {
                format!("+{name}")
            } else if flag("__contravariant__") {
                format!("-{name}")
            } else {
                format!("~{name}")
            };
            Ok(Value::string(rendered))
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

#[cfg(test)]
mod ownership_tests {
    const OWNER: &str = include_str!("typing.rs");
    const ALIASES: &str = include_str!("typing/aliases.rs");
    const GENERATION: &str = include_str!("typing/generation.rs");
    const PYTHON_MEMBERS: &str = include_str!("typing/python_members.rs");
    const IMPLEMENTATIONS: &str = concat!(
        include_str!("typing/aliases.rs"),
        include_str!("typing/generation.rs"),
        include_str!("typing/python_members.rs"),
    );

    #[test]
    fn facade_keeps_the_only_registration_surface() {
        let registration_macro = concat!("pyrust_", "module!");
        assert_eq!(
            OWNER.matches(registration_macro).count(),
            1,
            "typing.rs must remain the only registration owner"
        );
        assert!(
            !IMPLEMENTATIONS.contains(registration_macro),
            "private implementation modules must not register Python names"
        );
    }

    #[test]
    fn implementation_dependencies_are_explicit() {
        let wildcard_parent_import = concat!("use super::", "*");
        assert!(!IMPLEMENTATIONS.contains(wildcard_parent_import));
        assert!(!IMPLEMENTATIONS.contains("include!("));
    }

    #[test]
    fn responsibilities_stay_owned_by_one_module() {
        assert!(GENERATION.contains("struct TypingGeneration"));
        assert!(GENERATION.contains("STABLE_GENERIC_CLASS"));
        assert!(!GENERATION.contains("Interpreter"));

        assert!(ALIASES.contains("fn build_union"));
        assert!(ALIASES.contains("fn build_legacy_alias_class"));
        assert!(!ALIASES.contains("TYPING_PY_SOURCE"));

        assert!(PYTHON_MEMBERS.contains("TYPING_PY_SOURCE"));
        assert!(PYTHON_MEMBERS.contains("exec_source"));
        assert!(!PYTHON_MEMBERS.contains("TypingGeneration"));
    }

    #[test]
    fn registration_facade_stays_small() {
        assert!(
            OWNER.lines().count() <= 700,
            "typing registration facade grew beyond its 700-line budget"
        );
    }
}
