//! Construction and normalization for `typing` aliases and special forms.
//!
//! The generation registry owns identity and lifetime. This module owns the
//! value shapes, public alias delegation, repr support, and Union/Optional
//! normalization rules.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value, ValueKind};
use indexmap::IndexMap;

use super::generation;

const GENERIC_ALIAS_CLASS_CACHE: crate::interpreter::ModuleClassCacheSlot =
    crate::interpreter::ModuleClassCacheSlot::new(0);

/// Construct a `_TypingAlias` instance carrying `form` and `subscript`.
pub(super) fn make_typing_alias(form: &str, subscript: Value) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_form", Value::string(form));
    attrs.insert("_args", subscript);
    Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: generation::current_typing_alias_class(),
        attrs,
    })))
}

#[inline(always)]
fn active_generic_alias_class(interp: &mut Interpreter) -> Result<Rc<RefCell<PyClass>>> {
    interp.cached_module_class(GENERIC_ALIAS_CLASS_CACHE, "typing", "_GenericAlias")
}

fn is_paramspec_value(value: &Value) -> bool {
    matches!(
        value.kind(),
        ValueKind::PyInstance(inst)
            if crate::interpreter::is_paramspec_class(&inst.borrow().class)
    )
}

/// Extract declared type parameters for a generic class from `__type_params__`
/// (PEP 695) or from `Generic[...]` in `__orig_bases__`.
fn class_declared_type_params(origin: &Value) -> Vec<Value> {
    let ValueKind::PyClass(class) = origin.kind() else {
        return Vec::new();
    };
    if Rc::ptr_eq(class, &generation::stable_generic_class())
        || generation::is_protocol_marker_class(class)
    {
        return Vec::new();
    }
    let borrowed = class.borrow();
    if let Some(tp) = borrowed.attrs.get("__type_params__")
        && let ValueKind::Tuple(items) = tp.kind()
        && !items.is_empty()
    {
        return items.to_vec();
    }
    if let Some(orig_bases) = borrowed.attrs.get("__orig_bases__")
        && let ValueKind::Tuple(bases) = orig_bases.kind()
    {
        for base in bases.iter() {
            if let ValueKind::PyInstance(inst) = base.kind()
                && let Some(base_origin) = inst.borrow().attrs.get("__origin__")
                && let ValueKind::PyClass(base_cls) = base_origin.kind()
                && Rc::ptr_eq(base_cls, &generation::stable_generic_class())
                && let Some(base_args) = inst.borrow().attrs.get("__args__")
                && let ValueKind::Tuple(args) = base_args.kind()
            {
                return args.to_vec();
            }
        }
    }
    Vec::new()
}

/// Normalize subscript arguments for a generic class, applying CPython 3.12's
/// `ParamSpec` preparation rules when the class declares `ParamSpec` parameters.
fn normalize_generic_subscript(
    interp: &mut Interpreter,
    origin: &Value,
    subscript: Value,
) -> Result<Value> {
    let params = class_declared_type_params(origin);
    if params.len() == 1 && is_paramspec_value(&params[0]) {
        return match subscript.kind() {
            ValueKind::Tuple([]) => {
                let origin_repr = crate::interpreter::render_value_repr(interp, origin)?;
                Err(pyrust_core::type_err!(
                    "Too few arguments for {origin_repr}"
                ))
            }
            ValueKind::Ellipsis => Ok(Value::tuple(vec![subscript.clone()])),
            _ if is_paramspec_value(&subscript) => Ok(Value::tuple(vec![subscript.clone()])),
            ValueKind::List(items) => Ok(Value::tuple(vec![Value::tuple(items.to_vec())])),
            ValueKind::Tuple(_) => Ok(Value::tuple(vec![subscript.clone()])),
            _ => Ok(Value::tuple(vec![Value::tuple(vec![subscript.clone()])])),
        };
    }
    if params.iter().any(is_paramspec_value) {
        let raw: Vec<Value> = match subscript.kind() {
            ValueKind::Tuple(items) => items.to_vec(),
            _ => vec![subscript.clone()],
        };
        let normalized = raw
            .into_iter()
            .map(|item| {
                let list_items = match item.kind() {
                    ValueKind::List(items) => Some(items.to_vec()),
                    _ => None,
                };
                match list_items {
                    Some(items) => Value::tuple(items),
                    None => item,
                }
            })
            .collect();
        return Ok(Value::tuple(normalized));
    }
    if matches!(subscript.kind(), ValueKind::Tuple(_)) {
        Ok(subscript)
    } else {
        Ok(Value::tuple(vec![subscript]))
    }
}

/// Construct a `_GenericAlias` instance wrapping `origin` and `subscript`.
///
/// `Generic` is intentionally process-stable (matching CPython), so its
/// receiver cannot identify which root Interpreter is executing. Resolve the
/// alias class through that Interpreter's authoritative `sys.modules` entry
/// instead of the thread-global latest-generation registry.
#[inline]
pub(super) fn make_generic_alias(
    interp: &mut Interpreter,
    origin: Value,
    subscript: Value,
) -> Result<Value> {
    let class = active_generic_alias_class(interp)?;
    let args_tuple = normalize_generic_subscript(interp, &origin, subscript)?;
    let mut attrs = InstanceAttrs::with_capacity(4);
    attrs.insert("_origin", origin.clone());
    attrs.insert("_args", args_tuple.clone());
    attrs.insert("__origin__", origin);
    attrs.insert("__args__", args_tuple);
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs,
    }))))
}

/// Render a `_GenericAlias`'s argument tuple matching CPython 3.12's
/// `_GenericAlias.__repr__` (including `ParamSpec` tuple args as `[int, str]`,
/// empty tuples as `()`, and `Ellipsis` as `...`).
pub(super) fn generic_alias_args_repr(interp: &mut Interpreter, sub: &Value) -> Result<String> {
    match sub.kind() {
        ValueKind::Tuple([]) => Ok("()".to_string()),
        ValueKind::Tuple(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items.iter() {
                match item.kind() {
                    ValueKind::Tuple(inner) => {
                        let rendered = inner
                            .iter()
                            .map(|v| generic_arg_repr(interp, v))
                            .collect::<Result<Vec<_>>>()?
                            .join(", ");
                        parts.push(format!("[{rendered}]"));
                    }
                    _ => parts.push(generic_arg_repr(interp, item)?),
                }
            }
            Ok(parts.join(", "))
        }
        _ => generic_arg_repr(interp, sub),
    }
}

/// Resolve `__mro_entries__(self, bases)` for `typing._GenericAlias`, enforcing
/// CPython 3.12's duplicate-`Generic` and implicit PEP 695 `Protocol` /
/// `NamedTuple` / `TypedDict` rules.
pub(super) fn resolve_generic_alias_mro_entries(
    instance: &Rc<RefCell<PyInstance>>,
    bases_arg: Option<&Value>,
) -> Result<Value> {
    let (origin, is_pep695_implicit) = {
        let borrow = instance.borrow();
        let origin =
            borrow.attrs.get("_origin").cloned().ok_or_else(|| {
                PyError::runtime("typing._GenericAlias has no origin".to_string())
            })?;
        let implicit = borrow
            .attrs
            .get("_pep695_implicit")
            .is_some_and(Value::truthy_raw);
        (origin, implicit)
    };
    let origin_is_generic = matches!(
        origin.kind(),
        ValueKind::PyClass(cls) if Rc::ptr_eq(cls, &generation::stable_generic_class())
    );
    if origin_is_generic
        && let Some(bases_val) = bases_arg
        && let ValueKind::Tuple(bases) = bases_val.kind()
    {
        for base in bases.iter() {
            if let ValueKind::PyInstance(other) = base.kind()
                && Rc::ptr_eq(other, instance)
            {
                continue;
            }
            if let ValueKind::PyClass(cls) = base.kind() {
                if Rc::ptr_eq(cls, &generation::stable_generic_class()) {
                    return Err(pyrust_core::type_err!("duplicate base class Generic"));
                }
                if is_pep695_implicit
                    && (generation::is_protocol_marker_class(cls)
                        || pyrust_builtins::typing_marker::classify(cls).is_some())
                {
                    return Ok(Value::tuple(vec![]));
                }
            } else if let ValueKind::PyInstance(other) = base.kind()
                && let Some(other_origin) = other.borrow().attrs.get("__origin__").cloned()
                && let ValueKind::PyClass(other_cls) = other_origin.kind()
            {
                if Rc::ptr_eq(other_cls, &generation::stable_generic_class()) {
                    return Err(pyrust_core::type_err!(
                        "Cannot inherit from Generic[...] multiple times."
                    ));
                }
                if is_pep695_implicit && generation::is_protocol_marker_class(other_cls) {
                    return Ok(Value::tuple(vec![]));
                }
            }
        }
    }
    Ok(Value::tuple(vec![origin]))
}

/// Render an alias origin with CPython's `_type_repr` spelling.
pub(super) fn generic_origin_repr(origin: &Value) -> String {
    if let ValueKind::PyClass(class) = origin.kind() {
        let is_typing_singleton = Rc::ptr_eq(class, &generation::stable_generic_class())
            || generation::protocol_classes()
                .iter()
                .any(|protocol| Rc::ptr_eq(class, protocol));
        let borrow = class.borrow();
        let qualname = borrow.qualname.clone();
        if is_typing_singleton {
            return format!("typing.{qualname}");
        }
        let module = borrow
            .attrs
            .get("__module__")
            .and_then(|value| value.as_str().map(str::to_owned));
        return match module.as_deref() {
            Some("builtins") | None => qualname,
            Some(module) => format!("{module}.{qualname}"),
        };
    }
    "typing.Generic".to_string()
}

/// Render one `_GenericAlias` argument with CPython's `_type_repr` spelling.
pub(super) fn generic_arg_repr(interp: &mut Interpreter, value: &Value) -> Result<String> {
    match value.kind() {
        ValueKind::Ellipsis => return Ok("...".to_string()),
        ValueKind::PyClass(class) => {
            return pyrust_builtins::generic_alias::repr_class_type_arg_with(
                class,
                pyrust_builtins::generic_alias::ClassTypeArgReprStyle::Typing,
                interp,
                Interpreter::render_value_as_str,
            );
        }
        ValueKind::List(items) => {
            let inner = items
                .iter()
                .map(|v| generic_arg_repr(interp, v))
                .collect::<Result<Vec<_>>>()?
                .join(", ");
            return Ok(format!("[{inner}]"));
        }
        _ => {}
    }
    if let Some(rendered) = pyrust_builtins::generic_alias::typevar_arg_repr(value) {
        return Ok(rendered);
    }
    crate::interpreter::render_value_repr(interp, value)
}

/// Construct the generation-local `Any` singleton value.
pub(super) fn make_any_instance() -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert("__module__", Value::string("typing"));
    attrs.insert("__name__", Value::string("Any"));
    attrs.insert("__qualname__", Value::string("Any"));
    Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: generation::current_any_class(),
        attrs,
    })))
}

fn special_form_spec(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "Optional" => ("Optional", "typing._optional_cgi"),
        "Union" => ("Union", "typing._union_cgi"),
        "Callable" => ("Callable", "typing._callable_cgi"),
        "ClassVar" => ("ClassVar", "typing._classvar_cgi"),
        "Final" => ("Final", "typing._final_cgi"),
        "Literal" => ("Literal", "typing._literal_cgi"),
        "Type" => ("Type", "typing._type_cgi"),
        _ => return None,
    })
}

fn build_special_form_class(name: &'static str, reg_name: &'static str) -> Rc<RefCell<PyClass>> {
    let mut attrs: IndexMap<String, Value> = IndexMap::new();
    attrs.insert(
        "__class_getitem__".to_string(),
        Value::builtin_function(reg_name),
    );
    attrs.insert("__module__".to_string(), Value::string("typing"));
    let mut pyclass = PyClass::new(name, name, None, attrs);
    pyclass.override_repr = Some(format!("typing.{name}").into_boxed_str());
    Rc::new(RefCell::new(pyclass))
}

fn current_special_form_class(name: &str) -> Option<Rc<RefCell<PyClass>>> {
    let (name, reg_name) = special_form_spec(name)?;
    Some(generation::current_special_form_class(name, || {
        build_special_form_class(name, reg_name)
    }))
}

/// Build or retrieve a special form without recursively calling `module()`.
pub(super) fn class_value_for(name: &str) -> Value {
    current_special_form_class(name)
        .map(Value::py_class)
        .unwrap_or_else(Value::none)
}

pub(super) fn special_form_class_getitem(form: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    if args.iter().any(|argument| argument.name.is_some()) {
        return Err(pyrust_core::type_err!(
            "typing.{form}.__class_getitem__() takes no keyword arguments"
        ));
    }
    let positionals = args
        .iter()
        .filter(|argument| argument.name.is_none())
        .collect::<Vec<_>>();
    let index = positionals
        .last()
        .map(|argument| argument.value.clone())
        .ok_or_else(|| {
            pyrust_core::type_err!("typing.{form}.__class_getitem__() needs an argument")
        })?;
    let origin = if positionals.len() >= 2 {
        positionals[0].value.clone()
    } else {
        class_value_for(form)
    };
    if matches!(form, "Union" | "Optional") {
        let union_origin = if form == "Union" {
            origin
        } else {
            generation::paired_special_form_class(&origin, "Union")
                .map(Value::py_class)
                .unwrap_or_else(|| class_value_for("Union"))
        };
        return union_or_optional_getitem(form, index, union_origin);
    }

    let type_arguments = if matches!(index.kind(), ValueKind::Tuple(_)) {
        index
    } else {
        Value::tuple(vec![index])
    };
    Ok(pyrust_builtins::generic_alias::generic_alias(
        origin,
        type_arguments,
    ))
}

/// Normalize and construct a `Union`/`Optional` alias.
fn union_or_optional_getitem(form: &str, index: Value, union_origin: Value) -> Result<Value> {
    let none_type = || primitive_class_value("NoneType");
    let raw = match index.kind() {
        ValueKind::Tuple(items) => {
            if form == "Optional" {
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
        let mut args = raw;
        args.push(Value::none());
        return build_union(args, none_type(), union_origin);
    }
    if raw.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "Cannot take a Union of no types.".to_string(),
        ));
    }
    build_union(raw, none_type(), union_origin)
}

/// Flatten, lower `None`, de-duplicate, and collapse Union arguments.
fn build_union(raw: Vec<Value>, none_type: Value, union_origin: Value) -> Result<Value> {
    let mut flat = Vec::with_capacity(raw.len());
    for arg in raw {
        let arg = if arg.is_none() {
            none_type.clone()
        } else {
            arg
        };
        if let Some((origin, nested_args)) =
            pyrust_builtins::generic_alias::as_generic_alias_origin_args(&arg)
            && generation::is_union_class(&origin)
            && let ValueKind::Tuple(items) = nested_args.kind()
        {
            for inner in items.iter() {
                push_unique(&mut flat, inner.clone());
            }
            continue;
        }
        push_unique(&mut flat, arg);
    }

    if flat.len() == 1 {
        return Ok(flat.into_iter().next().unwrap());
    }
    Ok(pyrust_builtins::generic_alias::typing_union_alias(
        union_origin,
        Value::tuple(flat),
    ))
}

fn push_unique(values: &mut Vec<Value>, value: Value) {
    if !values.contains(&value) {
        values.push(value);
    }
}

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

fn legacy_alias_spec(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "List" => ("List", "list"),
        "Dict" => ("Dict", "dict"),
        "Set" => ("Set", "set"),
        "FrozenSet" => ("FrozenSet", "frozenset"),
        "Tuple" => ("Tuple", "tuple"),
        "Type" => ("Type", "type"),
        _ => return None,
    })
}

fn build_legacy_alias_class(name: &'static str, builtin: &'static str) -> Rc<RefCell<PyClass>> {
    let mut attrs: IndexMap<String, Value> = IndexMap::new();
    attrs.insert(
        "__class_getitem__".to_string(),
        Value::builtin_function(legacy_alias_cgi_name(name)),
    );
    attrs.insert("__module__".to_string(), Value::string("typing"));
    attrs.insert(
        "__pyrust_legacy_alias_of__".to_string(),
        legacy_builtin_class_value(builtin),
    );
    let mut pyclass = PyClass::new(name, name, None, attrs);
    pyclass.override_repr = Some(format!("typing.{name}").into_boxed_str());
    Rc::new(RefCell::new(pyclass))
}

fn current_legacy_alias_class(name: &str) -> Option<Rc<RefCell<PyClass>>> {
    let (name, builtin) = legacy_alias_spec(name)?;
    Some(generation::current_legacy_alias_class(name, || {
        build_legacy_alias_class(name, builtin)
    }))
}

/// Return the module-constant value for a legacy alias.
pub(super) fn legacy_alias_value(name: &str) -> Value {
    current_legacy_alias_class(name)
        .map(Value::py_class)
        .unwrap_or_else(Value::none)
}

pub(super) fn start_generation_legacy_alias_value(name: &str) -> Value {
    generation::start_typing_generation();
    legacy_alias_value(name)
}

fn legacy_builtin_class_value(builtin: &str) -> Value {
    if builtin == "type" {
        return Value::py_class(crate::interpreter::type_class_singleton());
    }
    crate::interpreter::primitive_class_by_name(builtin)
        .map(Value::py_class)
        .unwrap_or_else(Value::none)
}

/// Return the primitive class delegated to by a live legacy typing alias.
pub(crate) fn legacy_alias_delegate(class: &Rc<RefCell<PyClass>>) -> Option<Value> {
    if !generation::is_legacy_alias_class(class) {
        return None;
    }
    class
        .borrow()
        .attrs
        .get("__pyrust_legacy_alias_of__")
        .cloned()
}

fn primitive_class_value(name: &str) -> Value {
    crate::interpreter::primitive_class_by_name(name)
        .map(Value::py_class)
        .unwrap_or_else(Value::none)
}
