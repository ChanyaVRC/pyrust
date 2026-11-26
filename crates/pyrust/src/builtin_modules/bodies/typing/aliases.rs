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

/// CPython 3.12 `_generic_init_subclass`: record `__parameters__` on every new
/// `Generic` subclass and reject plain / inconsistent `Generic` bases.
///
/// Classes that do not derive from `typing.Generic` return immediately, so
/// ordinary class creation pays only one MRO walk.
pub(super) fn init_generic_subclass(
    interp: &mut Interpreter,
    class: &Rc<RefCell<PyClass>>,
) -> Result<()> {
    let generic = generation::stable_generic_class();
    if Rc::ptr_eq(class, &generic) || !crate::interpreter::class_is_subclass_of(class, &generic) {
        return Ok(());
    }
    let is_plain_generic = |value: &Value| matches!(value.kind(), ValueKind::PyClass(base) if Rc::ptr_eq(base, &generic));
    let orig_bases = class.borrow().attrs.get("__orig_bases__").cloned();
    let parameters = match orig_bases {
        Some(orig_bases) => {
            let bases = match orig_bases.kind() {
                ValueKind::Tuple(items) => items.to_vec(),
                _ => Vec::new(),
            };
            if bases.iter().any(is_plain_generic) {
                return Err(pyrust_core::type_err!("Cannot inherit from plain Generic"));
            }
            let collected = collect_type_parameters(interp, &orig_bases)?;
            let mut generic_vars: Option<Value> = None;
            for base in &bases {
                let Some(instance) = base.as_py_instance_rc() else {
                    continue;
                };
                if !generation::is_generic_alias_class(&instance.borrow().class) {
                    continue;
                }
                let origin = instance.borrow().attrs.get("__origin__").cloned();
                if !origin.as_ref().is_some_and(is_plain_generic) {
                    continue;
                }
                if generic_vars.is_some() {
                    return Err(pyrust_core::type_err!(
                        "Cannot inherit from Generic[...] multiple times."
                    ));
                }
                generic_vars = Some(
                    instance
                        .borrow()
                        .attrs
                        .get("__parameters__")
                        .cloned()
                        .unwrap_or_else(|| Value::tuple(vec![])),
                );
            }
            match generic_vars {
                Some(generic_vars) => {
                    let declared = match generic_vars.kind() {
                        ValueKind::Tuple(items) => items.to_vec(),
                        _ => Vec::new(),
                    };
                    let used = match collected.kind() {
                        ValueKind::Tuple(items) => items.to_vec(),
                        _ => Vec::new(),
                    };
                    let missing: Vec<&Value> = used
                        .iter()
                        .filter(|var| !declared.iter().any(|declared| declared == *var))
                        .collect();
                    if !missing.is_empty() {
                        let mut missing_names = Vec::with_capacity(missing.len());
                        for var in missing {
                            missing_names.push(interp.render_value_as_str(var)?);
                        }
                        let mut declared_names = Vec::with_capacity(declared.len());
                        for var in &declared {
                            declared_names.push(interp.render_value_as_str(var)?);
                        }
                        return Err(pyrust_core::type_err!(
                            "Some type variables ({}) are not listed in Generic[{}]",
                            missing_names.join(", "),
                            declared_names.join(", ")
                        ));
                    }
                    generic_vars
                }
                None => collected,
            }
        }
        None => {
            let (direct_generic, name) = {
                let borrow = class.borrow();
                let direct = borrow
                    .base
                    .as_ref()
                    .is_some_and(|base| Rc::ptr_eq(base, &generic))
                    || borrow
                        .extra_bases
                        .iter()
                        .any(|base| Rc::ptr_eq(base, &generic));
                (direct, borrow.name.clone())
            };
            if direct_generic && name != "Protocol" {
                return Err(pyrust_core::type_err!("Cannot inherit from plain Generic"));
            }
            Value::tuple(vec![])
        }
    };
    {
        let mut borrow = class.borrow_mut();
        borrow
            .attrs
            .insert("__parameters__".to_string(), parameters);
        borrow.bump_mutation_version();
    }
    // `_ProtocolMeta` is an `ABCMeta` in CPython: protocol classes (and their
    // concrete subclasses) track `__abstractmethods__`.
    if generation::is_protocol_subclass(class) {
        crate::builtin_modules::abc::compute_protocol_abstract_methods(interp, class)?;
    }
    Ok(())
}

/// CPython `_generic_class_getitem`'s `_check_generic` for a `Generic`
/// subclass: the subscript must supply exactly one argument per type
/// parameter. Classes with `ParamSpec` / `TypeVarTuple` parameters keep the
/// dedicated normalization in [`normalize_generic_subscript`].
fn check_generic_class_arity(interp: &mut Interpreter, origin: &Value, args: &Value) -> Result<()> {
    let ValueKind::PyClass(class) = origin.kind() else {
        return Ok(());
    };
    if Rc::ptr_eq(class, &generation::stable_generic_class())
        || generation::is_protocol_marker_class(class)
    {
        return Ok(());
    }
    let parameters = class.borrow().attrs.get("__parameters__").cloned();
    let Some(parameters) = parameters else {
        return Ok(());
    };
    let ValueKind::Tuple(params) = parameters.kind() else {
        return Ok(());
    };
    let variadic = params.iter().any(|param| {
        is_paramspec_value(param)
            || matches!(param.kind(), ValueKind::PyInstance(instance)
                if crate::interpreter::is_typevartuple_class(&instance.borrow().class))
    });
    if variadic {
        return Ok(());
    }
    let expected = params.len();
    let actual = match args.kind() {
        ValueKind::Tuple(items) => items.len(),
        _ => 1,
    };
    if expected != 0 && actual == expected {
        return Ok(());
    }
    let rendered = interp.render_value_as_str(origin)?;
    if expected == 0 {
        return Err(pyrust_core::type_err!("{rendered} is not a generic class"));
    }
    let quantity = if actual > expected { "many" } else { "few" };
    Err(pyrust_core::type_err!(
        "Too {quantity} arguments for {rendered}; actual {actual}, expected {expected}"
    ))
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
    check_generic_class_arity(interp, &origin, &args_tuple)?;
    let parameters = collect_type_parameters(interp, &args_tuple)?;
    let mut attrs = InstanceAttrs::with_capacity(5);
    attrs.insert("_origin", origin.clone());
    attrs.insert("_args", args_tuple.clone());
    attrs.insert("__origin__", origin);
    attrs.insert("__args__", args_tuple);
    attrs.insert("__parameters__", parameters);
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs,
    }))))
}

/// Construct CPython's named `_GenericAlias(origin, args, name=..., inst=...)`.
///
/// This is the subscripted form of a `_SpecialGenericAlias` such as
/// `typing.Iterable[int]`: `__origin__` is the runtime class
/// (`collections.abc.Iterable`) while `_name` keeps the `typing.` spelling.
pub(super) fn make_named_generic_alias(
    interp: &mut Interpreter,
    origin: Value,
    args: Vec<Value>,
    name: Value,
    inst: bool,
) -> Result<Value> {
    let class = active_generic_alias_class(interp)?;
    let args_tuple = Value::tuple(args);
    let parameters = collect_type_parameters(interp, &args_tuple)?;
    let mut attrs = InstanceAttrs::with_capacity(7);
    attrs.insert("_origin", origin.clone());
    attrs.insert("_args", args_tuple.clone());
    attrs.insert("_inst", Value::bool_(inst));
    attrs.insert("_name", name);
    attrs.insert("__origin__", origin);
    attrs.insert("__args__", args_tuple);
    attrs.insert("__parameters__", parameters);
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs,
    }))))
}

/// True for a `TypeVar`, `ParamSpec`, or `TypeVarTuple` instance — CPython's
/// `hasattr(t, '__typing_subst__')` test in `_collect_parameters`.
pub(super) fn is_type_parameter(value: &Value) -> bool {
    let ValueKind::PyInstance(instance) = value.kind() else {
        return false;
    };
    let class = Rc::clone(&instance.borrow().class);
    crate::interpreter::is_typevar_class(&class)
        || crate::interpreter::is_paramspec_class(&class)
        || crate::interpreter::is_typevartuple_class(&class)
}

/// CPython 3.12 `typing._collect_parameters`: the type variables of `args`
/// in first-appearance order, descending into tuples and into any argument's
/// own `__parameters__` (classes are skipped).
pub(super) fn collect_type_parameters(interp: &mut Interpreter, args: &Value) -> Result<Value> {
    fn push_unique(out: &mut Vec<Value>, value: &Value) {
        if !out.iter().any(|existing| existing == value) {
            out.push(value.clone());
        }
    }
    fn collect(interp: &mut Interpreter, value: &Value, out: &mut Vec<Value>) -> Result<()> {
        // A `Callable[[T], int]`-style parameter list. Snapshot it so no list
        // borrow is held across the recursive (attribute-reading) calls.
        let list_items = match value.kind() {
            ValueKind::List(items) => Some(items.to_vec()),
            _ => None,
        };
        if let Some(items) = list_items {
            for item in &items {
                collect(interp, item, out)?;
            }
            return Ok(());
        }
        match value.kind() {
            ValueKind::PyClass(_) => {}
            ValueKind::Tuple(items) => {
                for item in items {
                    collect(interp, item, out)?;
                }
            }
            _ if is_type_parameter(value) => push_unique(out, value),
            ValueKind::PyInstance(instance) => {
                let parameters = instance.borrow().attrs.get("__parameters__").cloned();
                if let Some(parameters) = parameters
                    && let ValueKind::Tuple(items) = parameters.kind()
                {
                    for item in items.iter() {
                        push_unique(out, item);
                    }
                }
            }
            _ if pyrust_builtins::generic_alias::is_generic_alias(value) => {
                let parameters = interp.get_attr(value, "__parameters__")?;
                if let ValueKind::Tuple(items) = parameters.kind() {
                    for item in items.iter() {
                        push_unique(out, item);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut out = Vec::new();
    match args.kind() {
        ValueKind::Tuple(items) => {
            for item in items {
                collect(interp, item, &mut out)?;
            }
        }
        _ => collect(interp, args, &mut out)?,
    }
    Ok(Value::tuple(out))
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

/// `_GenericAlias.__mro_entries__(self, bases)`: validate the Python-level
/// signature, then resolve the entries for a named alias
/// (`typing.Iterable[int]`) or a `Generic[...]` alias.
pub(super) fn generic_alias_mro_entries(
    interp: &mut Interpreter,
    instance: &Rc<RefCell<PyInstance>>,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let user_args = &args[1..];
    let positional_count = user_args
        .iter()
        .filter(|argument| argument.name.is_none())
        .count();
    let has_bases_keyword = user_args
        .iter()
        .any(|argument| argument.name.as_deref() == Some("bases"));

    // `_GenericAlias.__mro_entries__` is an ordinary Python method with
    // signature `(self, bases)`. The runtime supplies `self`, so validate the
    // remaining expanded arguments here before reading the origin. `bases` is
    // unused by CPython's `Generic[...]` branch, but it is still required and
    // may be passed by keyword.
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
    let bases_arg = user_args.first().map(|arg| &arg.value);
    if let Some(bases) = bases_arg
        && let Some(entries) =
            super::special_aliases::named_mro_entries(interp, instance, &args[0].value, bases)
    {
        return entries;
    }
    resolve_generic_alias_mro_entries(instance, bases_arg)
}

/// Resolve `__mro_entries__(self, bases)` for `typing._GenericAlias`, enforcing
/// CPython 3.12's duplicate-`Generic` and implicit PEP 695 `Protocol` /
/// `NamedTuple` / `TypedDict` rules.
fn resolve_generic_alias_mro_entries(
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
    // Equal `_GenericAlias` members (`Iterable[int]` twice) collapse like
    // CPython's hash/`==` based `_deduplicate`.
    if !values
        .iter()
        .any(|existing| super::special_aliases::alias_values_equal(existing, &value))
    {
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
