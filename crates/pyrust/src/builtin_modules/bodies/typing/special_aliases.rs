//! CPython 3.12 `_SpecialGenericAlias` and `_GenericAlias` dunder behaviour.
//!
//! `typing.Iterable`, `typing.Mapping`, `typing.ContextManager`,
//! `typing.Pattern`, … are `_SpecialGenericAlias` instances wrapping a real
//! runtime class (their `__origin__`). Subscripting one builds a *named*
//! `_GenericAlias`, so `typing.Iterable[int]` keeps the `typing.` spelling
//! while `get_origin()` reports `collections.abc.Iterable`. The shared
//! `_BaseGenericAlias` behaviour (calling, `|`, attribute delegation, MRO
//! entries) also applies to the `Generic[...]` aliases built by
//! [`super::aliases::make_generic_alias`].
//!
//! The generation registry owns class identity; this module owns the values'
//! Python-visible semantics.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyInstance, Value, ValueKind};

use super::{aliases, generation};

const SUBSCRIPTED_CHECK_ERROR: &str =
    "Subscripted generics cannot be used with class and instance checks";

const PARAMETERS_MUST_BE_TYPES: &str = "Parameters to generic types must be types.";

fn positional_arg(args: &[ExpandedCallArg], index: usize) -> Value {
    args.iter()
        .filter(|argument| argument.name.is_none())
        .nth(index)
        .map(|argument| argument.value.clone())
        .unwrap_or_else(Value::none)
}

fn receiver(args: &[ExpandedCallArg], method: &str) -> Result<Rc<RefCell<PyInstance>>> {
    match args.first().map(|argument| argument.value.kind()) {
        Some(ValueKind::PyInstance(instance)) => Ok(Rc::clone(instance)),
        _ => Err(pyrust_core::type_err!(
            "descriptor '{method}' requires a typing alias object"
        )),
    }
}

fn attr(instance: &Rc<RefCell<PyInstance>>, name: &str) -> Option<Value> {
    instance.borrow().attrs.get(name).cloned()
}

fn origin_of(instance: &Rc<RefCell<PyInstance>>) -> Result<Value> {
    attr(instance, "__origin__")
        .ok_or_else(|| PyError::attribute_error("__origin__", Some("__origin__".to_string()), None))
}

/// The `_name` of a named alias (`"Iterable"`), if it has one.
fn alias_name(instance: &Rc<RefCell<PyInstance>>) -> Option<String> {
    attr(instance, "_name")
        .and_then(|name| name.as_str().map(str::to_owned))
        .filter(|name| !name.is_empty())
}

fn is_dunder(name: &str) -> bool {
    name.len() > 4 && name.starts_with("__") && name.ends_with("__")
}

fn instance_class_matches(
    value: &Value,
    matches: fn(&Rc<RefCell<crate::value::PyClass>>) -> bool,
) -> bool {
    match value.kind() {
        ValueKind::PyInstance(instance) => {
            let class = Rc::clone(&instance.borrow().class);
            matches(&class)
        }
        _ => false,
    }
}

/// `isinstance(value, typing._SpecialGenericAlias)` for any live generation.
pub(super) fn is_special_alias_value(value: &Value) -> bool {
    instance_class_matches(value, generation::is_special_generic_alias_class)
}

/// `isinstance(value, typing._GenericAlias)` for any live generation.
pub(super) fn is_generic_alias_value(value: &Value) -> bool {
    instance_class_matches(value, generation::is_generic_alias_class)
}

/// `isinstance(value, typing._BaseGenericAlias)`: either alias family, a
/// legacy `typing.List`-style alias class, or one of its PEP 585 subscripts.
fn is_base_generic_alias_value(value: &Value) -> bool {
    if is_special_alias_value(value) || is_generic_alias_value(value) {
        return true;
    }
    if let ValueKind::PyClass(class) = value.kind() {
        return generation::is_legacy_alias_class(class);
    }
    if let Some(origin) = pyrust_builtins::generic_alias::as_generic_alias_origin(value)
        && let ValueKind::PyClass(class) = origin.kind()
    {
        return generation::is_legacy_alias_class(class);
    }
    false
}

fn call_builtin(interp: &mut Interpreter, name: &str, values: Vec<Value>) -> Result<Value> {
    let dispatch = crate::builtin_registry::lookup(name)
        .ok_or_else(|| PyError::runtime(format!("internal: builtin {name} is not registered")))?;
    let args: Vec<ExpandedCallArg> = values
        .into_iter()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    dispatch(interp, &args)
}

fn issubclass(interp: &mut Interpreter, cls: Value, classinfo: Value) -> Result<Value> {
    let result = call_builtin(interp, "issubclass", vec![cls, classinfo])?;
    Ok(Value::bool_(interp.truthy_value(&result)?))
}

fn render_str(interp: &mut Interpreter, value: &Value) -> Result<String> {
    interp.render_value_as_str(value)
}

// ── _SpecialGenericAlias ─────────────────────────────────────────────────────

/// `_SpecialGenericAlias.__init__(origin, nparams, *, inst=True, name=None)`;
/// `_DeprecatedGenericAlias` additionally requires `removal_version=`.
pub(super) fn special_init(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let instance = receiver(args, "__init__")?;
    let class = Rc::clone(&instance.borrow().class);
    let class_name = class.borrow().name.clone();
    let deprecated = generation::is_deprecated_generic_alias_class(&class);
    let positionals: Vec<Value> = args[1..]
        .iter()
        .filter(|argument| argument.name.is_none())
        .map(|argument| argument.value.clone())
        .collect();
    let [origin, nparams] = positionals.as_slice() else {
        return Err(pyrust_core::type_err!(
            "{class_name}.__init__() takes 3 positional arguments but {} were given",
            positionals.len() + 1
        ));
    };
    let mut inst = true;
    let mut name = Value::none();
    let mut removal_version = None;
    for argument in args.iter().filter(|argument| argument.name.is_some()) {
        match argument.name.as_deref() {
            Some("inst") => inst = interp.truthy_value(&argument.value)?,
            Some("name") => name = argument.value.clone(),
            Some("removal_version") if deprecated => {
                removal_version = Some(argument.value.clone());
            }
            Some(other) => {
                return Err(pyrust_core::type_err!(
                    "{class_name}.__init__() got an unexpected keyword argument '{other}'"
                ));
            }
            None => {}
        }
    }
    if deprecated && removal_version.is_none() {
        return Err(pyrust_core::type_err!(
            "{class_name}.__init__() missing 1 required keyword-only argument: 'removal_version'"
        ));
    }
    if name.is_none() {
        name = interp.get_attr(origin, "__name__")?;
    }
    let module = interp.get_attr(origin, "__module__")?;
    let module = render_str(interp, &module)?;
    let qualname = interp.get_attr(origin, "__qualname__")?;
    let qualname = render_str(interp, &qualname)?;
    let doc = if module == "builtins" {
        format!("A generic version of {qualname}.")
    } else {
        format!("A generic version of {module}.{qualname}.")
    };
    let mut borrow = instance.borrow_mut();
    borrow.attrs.insert("__origin__", origin.clone());
    borrow.attrs.insert("_inst", Value::bool_(inst));
    borrow.attrs.insert("_name", name);
    borrow.attrs.insert("_nparams", nparams.clone());
    borrow.attrs.insert("__doc__", Value::string(doc));
    if let Some(removal_version) = removal_version {
        borrow.attrs.insert("_removal_version", removal_version);
    }
    Ok(Value::none())
}

pub(super) fn special_repr(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let instance = receiver(args, "__repr__")?;
    let name = alias_name(&instance).unwrap_or_default();
    Ok(Value::string(format!("typing.{name}")))
}

pub(super) fn special_reduce(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let instance = receiver(args, "__reduce__")?;
    Ok(attr(&instance, "_name").unwrap_or_else(Value::none))
}

/// `_SpecialGenericAlias.__getitem__`: type-check every parameter, enforce
/// the alias arity, and build the named `_GenericAlias`.
pub(super) fn special_getitem(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let instance = receiver(args, "__getitem__")?;
    let params = positional_arg(&args[1..], 0);
    let raw = match params.kind() {
        ValueKind::Tuple(items) => items.to_vec(),
        _ => vec![params.clone()],
    };
    let mut checked = Vec::with_capacity(raw.len());
    for param in raw {
        checked.push(type_check(interp, param, PARAMETERS_MUST_BE_TYPES)?);
    }
    let alias_repr = format!("typing.{}", alias_name(&instance).unwrap_or_default());
    let expected = attr(&instance, "_nparams")
        .and_then(|value| value.as_int())
        .unwrap_or(0);
    check_generic(&alias_repr, checked.len(), expected)?;
    let inst = attr(&instance, "_inst").is_none_or(|value| value.truthy_raw());
    let name = attr(&instance, "_name").unwrap_or_else(Value::none);
    aliases::make_named_generic_alias(interp, origin_of(&instance)?, checked, name, inst)
}

/// CPython `_check_generic`: the arity error raised by alias subscription.
fn check_generic(alias_repr: &str, actual: usize, expected: i64) -> Result<()> {
    if expected == 0 {
        return Err(pyrust_core::type_err!(
            "{alias_repr} is not a generic class"
        ));
    }
    let expected_count = usize::try_from(expected).unwrap_or(usize::MAX);
    if actual != expected_count {
        let quantity = if actual > expected_count {
            "many"
        } else {
            "few"
        };
        return Err(pyrust_core::type_err!(
            "Too {quantity} arguments for {alias_repr}; actual {actual}, expected {expected}"
        ));
    }
    Ok(())
}

/// CPython `_type_convert`: `None` → `NoneType`, `str` → `ForwardRef(str)`.
fn type_convert(interp: &mut Interpreter, arg: Value) -> Result<Value> {
    if arg.is_none() {
        return Ok(crate::interpreter::primitive_class_by_name("NoneType")
            .map(Value::py_class)
            .unwrap_or(arg));
    }
    if matches!(arg.kind(), ValueKind::Str(_)) {
        let typing = interp.load_module("typing")?;
        let forward_ref = interp.get_attr(&typing, "ForwardRef")?;
        return interp.call_function_expanded(
            forward_ref,
            &[ExpandedCallArg {
                name: None,
                value: arg,
            }],
        );
    }
    Ok(arg)
}

fn is_generic_or_protocol_class(value: &Value) -> bool {
    matches!(
        value.kind(),
        ValueKind::PyClass(class)
            if Rc::ptr_eq(class, &generation::stable_generic_class())
                || generation::is_protocol_marker_class(class)
    )
}

/// The special-form class name (`"Union"`, `"ClassVar"`, …) of `value`, if it
/// is one of `typing`'s subscriptable special forms.
fn special_form_name(value: &Value) -> Option<String> {
    let ValueKind::PyClass(class) = value.kind() else {
        return None;
    };
    generation::is_special_form_class(class).then(|| class.borrow().name.clone())
}

/// The origin of a subscripted typing alias (`Generic[T]`, `ClassVar[int]`).
fn subscripted_origin(value: &Value) -> Option<Value> {
    if is_generic_alias_value(value) {
        return value
            .as_py_instance_rc()
            .and_then(|instance| attr(instance, "__origin__"));
    }
    pyrust_builtins::generic_alias::as_generic_alias_origin(value)
}

/// CPython 3.12 `typing._type_check` for a generic-alias parameter.
fn type_check(interp: &mut Interpreter, arg: Value, msg: &str) -> Result<Value> {
    let arg = type_convert(interp, arg)?;
    if let Some(origin) = subscripted_origin(&arg)
        && (is_generic_or_protocol_class(&origin)
            || matches!(
                special_form_name(&origin).as_deref(),
                Some("ClassVar" | "Final")
            ))
    {
        let rendered = render_str(interp, &arg)?;
        return Err(pyrust_core::type_err!(
            "{rendered} is not valid as type argument"
        ));
    }
    // `Callable` is a generic alias in CPython, not a `_SpecialForm`.
    let plain_special_form = special_form_name(&arg).is_some_and(|name| name != "Callable");
    if plain_special_form || is_generic_or_protocol_class(&arg) {
        let rendered = render_str(interp, &arg)?;
        return Err(pyrust_core::type_err!(
            "Plain {rendered} is not valid as type argument"
        ));
    }
    if matches!(arg.kind(), ValueKind::Tuple(_)) {
        let rendered: String = crate::interpreter::render_value_repr(interp, &arg)?
            .chars()
            .take(100)
            .collect();
        return Err(pyrust_core::type_err!("{msg} Got {rendered}."));
    }
    Ok(arg)
}

/// `_BaseGenericAlias.__mro_entries__`: substitute the origin, plus `Generic`
/// unless a later base already supplies it.
fn base_mro_entries(
    interp: &mut Interpreter,
    instance: &Rc<RefCell<PyInstance>>,
    self_value: &Value,
    bases: &Value,
) -> Result<Value> {
    let origin = origin_of(instance)?;
    let items = match bases.kind() {
        ValueKind::Tuple(items) => items.to_vec(),
        _ => vec![bases.clone()],
    };
    let mut entries = Vec::with_capacity(2);
    if !items.iter().any(|base| base == &origin) {
        entries.push(origin);
    }
    // `bases.index(self)`: identity first, then `==` (an equal alias).
    let mut position = items.iter().position(|base| base == self_value);
    if position.is_none() {
        for (index, base) in items.iter().enumerate() {
            if interp.values_user_eq(base, self_value)? {
                position = Some(index);
                break;
            }
        }
    }
    let position = position.ok_or_else(|| {
        PyError::named("ValueError", "tuple.index(x): x not in tuple".to_string())
    })?;
    let generic = generation::stable_generic_class();
    let supplies_generic = items[position + 1..].iter().any(|base| {
        is_base_generic_alias_value(base)
            || matches!(base.kind(), ValueKind::PyClass(class)
                if crate::interpreter::class_is_subclass_of(class, &generic))
    });
    if !supplies_generic {
        entries.push(Value::py_class(generic));
    }
    Ok(Value::tuple(entries))
}

pub(super) fn special_mro_entries(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let instance = receiver(args, "__mro_entries__")?;
    let bases = positional_arg(&args[1..], 0);
    base_mro_entries(interp, &instance, &args[0].value, &bases)
}

/// `_SpecialGenericAlias.__subclasscheck__`: delegate to the origin unless the
/// candidate is a subscripted generic.
fn special_subclasscheck_value(
    interp: &mut Interpreter,
    instance: &Rc<RefCell<PyInstance>>,
    cls: Value,
) -> Result<Value> {
    let origin = origin_of(instance)?;
    if is_special_alias_value(&cls) {
        let cls_origin = cls
            .as_py_instance_rc()
            .and_then(|other| attr(other, "__origin__"))
            .unwrap_or(cls);
        return issubclass(interp, cls_origin, origin);
    }
    if !is_generic_alias_value(&cls) {
        return issubclass(interp, cls, origin);
    }
    Err(pyrust_core::type_err!("{SUBSCRIPTED_CHECK_ERROR}"))
}

pub(super) fn special_subclasscheck(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let instance = receiver(args, "__subclasscheck__")?;
    special_subclasscheck_value(interp, &instance, positional_arg(&args[1..], 0))
}

/// `_BaseGenericAlias.__instancecheck__`, i.e. `self.__subclasscheck__(type(obj))`.
///
/// `type(obj)` is never a subscripted alias, so this is the origin check.
/// It is phrased as `isinstance(obj, origin)` because some runtime values
/// (builtin functions, …) have no class object that `issubclass` accepts.
pub(super) fn special_instancecheck(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let instance = receiver(args, "__instancecheck__")?;
    let obj = positional_arg(&args[1..], 0);
    let origin = origin_of(&instance)?;
    let result = call_builtin(interp, "isinstance", vec![obj, origin])?;
    Ok(Value::bool_(interp.truthy_value(&result)?))
}

/// `_DeprecatedGenericAlias.__instancecheck__` (`typing.ByteString`): warn,
/// then defer to the ordinary special-alias check.
pub(super) fn deprecated_instancecheck(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let instance = receiver(args, "__instancecheck__")?;
    let name = alias_name(&instance).unwrap_or_default();
    let removal = match attr(&instance, "_removal_version") {
        Some(version) => match version.kind() {
            ValueKind::Tuple(parts) => parts
                .iter()
                .map(|part| render_str(interp, part))
                .collect::<Result<Vec<_>>>()?
                .join("."),
            _ => render_str(interp, &version)?,
        },
        None => String::new(),
    };
    let message =
        format!("'typing.{name}' is deprecated and slated for removal in Python {removal}");
    let warnings = interp.load_module("warnings")?;
    let warn = interp.get_attr(&warnings, "warn")?;
    let category = crate::interpreter::lookup_exc_class("DeprecationWarning")
        .map(Value::py_class)
        .unwrap_or_else(Value::none);
    interp.call_function_expanded(
        warn,
        &[
            ExpandedCallArg {
                name: None,
                value: Value::string(message),
            },
            ExpandedCallArg {
                name: None,
                value: category,
            },
            ExpandedCallArg {
                name: Some("stacklevel".to_string()),
                value: Value::int(2),
            },
        ],
    )?;
    special_instancecheck(interp, args)
}

// ── _BaseGenericAlias (shared by both alias families) ───────────────────────

/// `_BaseGenericAlias.__call__`: instantiate the origin and record the alias
/// on the result as `__orig_class__` when the object allows it.
pub(super) fn base_call(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let instance = receiver(args, "__call__")?;
    let origin = attr(&instance, "__origin__")
        .unwrap_or_else(|| Value::py_class(generation::current_generic_class()));
    if attr(&instance, "_inst").is_some_and(|value| !value.truthy_raw()) {
        let name = alias_name(&instance).unwrap_or_default();
        let origin_name = interp.get_attr(&origin, "__name__")?;
        let origin_name = render_str(interp, &origin_name)?;
        return Err(pyrust_core::type_err!(
            "Type {name} cannot be instantiated; use {origin_name}() instead"
        ));
    }
    let result = interp.call_function_expanded(origin, &args[1..])?;
    // CPython swallows every exception raised while recording the alias.
    let _ = interp.assign_attr(result.clone(), "__orig_class__", args[0].value.clone());
    Ok(result)
}

/// `Union[members]` for the active `typing` generation.
fn union_of(members: Value) -> Result<Value> {
    aliases::special_form_class_getitem(
        "Union",
        &[ExpandedCallArg {
            name: None,
            value: members,
        }],
    )
}

/// `Union[self, other]` (or `Union[other, self]` when `reflected`).
fn union_with(self_value: Value, other: Value, reflected: bool) -> Result<Value> {
    let members = if reflected {
        vec![other, self_value]
    } else {
        vec![self_value, other]
    };
    union_of(Value::tuple(members))
}

/// Data-only `_GenericAlias.__eq__` used where no interpreter is available
/// (Union de-duplication): equal origins and pairwise-equal arguments.
pub(super) fn alias_values_equal(left: &Value, right: &Value) -> bool {
    if left == right {
        return true;
    }
    if !(is_generic_alias_value(left) && is_generic_alias_value(right)) {
        return false;
    }
    let (Some(left), Some(right)) = (left.as_py_instance_rc(), right.as_py_instance_rc()) else {
        return false;
    };
    let (left_origin, left_args) = origin_and_args(left);
    let (right_origin, right_args) = origin_and_args(right);
    if !alias_values_equal(&left_origin, &right_origin) {
        return false;
    }
    match (left_args.kind(), right_args.kind()) {
        (ValueKind::Tuple(left), ValueKind::Tuple(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(left, right)| alias_values_equal(left, right))
        }
        _ => left_args == right_args,
    }
}

pub(super) fn base_or(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    receiver(args, "__or__")?;
    union_with(args[0].value.clone(), positional_arg(&args[1..], 0), false)
}

pub(super) fn base_ror(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    receiver(args, "__ror__")?;
    union_with(args[0].value.clone(), positional_arg(&args[1..], 0), true)
}

/// `_BaseGenericAlias.__getattr__`: serve `__name__` / `__qualname__` and
/// relay every non-dunder attribute to the origin.
pub(super) fn base_getattr(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let instance = receiver(args, "__getattr__")?;
    let name_value = positional_arg(&args[1..], 0);
    let name = name_value.as_str().map(str::to_owned).unwrap_or_default();
    if name == "__name__" || name == "__qualname__" {
        if let Some(alias) = attr(&instance, "_name").filter(|value| value.truthy_raw()) {
            return Ok(alias);
        }
        let origin = origin_of(&instance)?;
        return interp.get_attr(&origin, "__name__");
    }
    if !is_dunder(&name)
        && let Some(origin) = attr(&instance, "__origin__")
    {
        return interp.get_attr(&origin, &name);
    }
    Err(PyError::attribute_error(
        name.clone(),
        Some(name),
        Some(args[0].value.clone()),
    ))
}

// ── _GenericAlias ────────────────────────────────────────────────────────────

/// Repr prefix of a `_GenericAlias`: `typing.<name>` for a named alias.
pub(super) fn named_origin_repr(instance: &Rc<RefCell<PyInstance>>) -> Option<String> {
    alias_name(instance).map(|name| format!("typing.{name}"))
}

/// `_GenericAlias.__mro_entries__` for a named alias; `None` for the
/// `Generic[...]` family, which keeps its dedicated resolution.
pub(super) fn named_mro_entries(
    interp: &mut Interpreter,
    instance: &Rc<RefCell<PyInstance>>,
    self_value: &Value,
    bases: &Value,
) -> Option<Result<Value>> {
    alias_name(instance)?;
    Some(base_mro_entries(interp, instance, self_value, bases))
}

pub(super) fn generic_subclasscheck(
    _interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    receiver(args, "__subclasscheck__")?;
    Err(pyrust_core::type_err!("{SUBSCRIPTED_CHECK_ERROR}"))
}

fn origin_and_args(instance: &Rc<RefCell<PyInstance>>) -> (Value, Value) {
    let borrow = instance.borrow();
    (
        borrow
            .attrs
            .get("__origin__")
            .cloned()
            .unwrap_or_else(Value::none),
        borrow
            .attrs
            .get("__args__")
            .cloned()
            .unwrap_or_else(|| Value::tuple(vec![])),
    )
}

pub(super) fn generic_eq(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let instance = receiver(args, "__eq__")?;
    let other = positional_arg(&args[1..], 0);
    if !is_generic_alias_value(&other) {
        return Ok(Value::not_implemented());
    }
    let other_instance = other
        .as_py_instance_rc()
        .map(Rc::clone)
        .ok_or_else(|| PyError::runtime("internal: _GenericAlias is not an instance".into()))?;
    let (origin, alias_args) = origin_and_args(&instance);
    let (other_origin, other_args) = origin_and_args(&other_instance);
    let equal = interp.values_user_eq(&origin, &other_origin)?
        && interp.values_user_eq(&alias_args, &other_args)?;
    Ok(Value::bool_(equal))
}

pub(super) fn generic_hash(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let instance = receiver(args, "__hash__")?;
    let (origin, alias_args) = origin_and_args(&instance);
    let hash = crate::interpreter::hash_value_with_interp(
        interp,
        &Value::tuple(vec![origin, alias_args]),
    )?;
    Ok(Value::int(hash))
}

/// `_GenericAlias.__getitem__`: substitute the alias's type parameters
/// (`Mapping[str, T][int]` → `typing.Mapping[str, int]`).
pub(super) fn generic_getitem(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let instance = receiver(args, "__getitem__")?;
    let self_value = args[0].value.clone();
    let (origin, alias_args) = origin_and_args(&instance);
    let self_repr = crate::interpreter::render_value_repr(interp, &self_value)?;
    if is_generic_or_protocol_class(&origin) {
        return Err(pyrust_core::type_err!(
            "Cannot subscript already-subscripted {self_repr}"
        ));
    }
    let parameters = match attr(&instance, "__parameters__") {
        Some(value) => match value.kind() {
            ValueKind::Tuple(items) => items.to_vec(),
            _ => Vec::new(),
        },
        None => Vec::new(),
    };
    if parameters.is_empty() {
        return Err(pyrust_core::type_err!("{self_repr} is not a generic class"));
    }
    let subscript = positional_arg(&args[1..], 0);
    let raw = match subscript.kind() {
        ValueKind::Tuple(items) => items.to_vec(),
        _ => vec![subscript.clone()],
    };
    let mut replacements = Vec::with_capacity(raw.len());
    for value in raw {
        replacements.push(type_convert(interp, value)?);
    }
    if replacements.len() != parameters.len() {
        let quantity = if replacements.len() > parameters.len() {
            "many"
        } else {
            "few"
        };
        return Err(pyrust_core::type_err!(
            "Too {quantity} arguments for {self_repr}; actual {}, expected {}",
            replacements.len(),
            parameters.len()
        ));
    }
    let old_args = match alias_args.kind() {
        ValueKind::Tuple(items) => items.to_vec(),
        _ => vec![alias_args.clone()],
    };
    let mut new_args = Vec::with_capacity(old_args.len());
    for old in old_args {
        new_args.push(substitute(interp, &old, &parameters, &replacements)?);
    }
    match alias_name(&instance) {
        Some(_) => {
            let name = attr(&instance, "_name").unwrap_or_else(Value::none);
            let inst = attr(&instance, "_inst").is_none_or(|value| value.truthy_raw());
            aliases::make_named_generic_alias(interp, origin, new_args, name, inst)
        }
        None => aliases::make_generic_alias(interp, origin, Value::tuple(new_args)),
    }
}

/// Replace each type parameter in `value` with its new argument, re-subscripting
/// nested generic aliases by their own `__parameters__`.
fn substitute(
    interp: &mut Interpreter,
    value: &Value,
    parameters: &[Value],
    replacements: &[Value],
) -> Result<Value> {
    if aliases::is_type_parameter(value) {
        if let Some(index) = parameters.iter().position(|parameter| parameter == value) {
            return Ok(replacements[index].clone());
        }
        return Ok(value.clone());
    }
    if let ValueKind::Tuple(items) = value.kind() {
        let mut substituted = Vec::with_capacity(items.len());
        for item in items {
            substituted.push(substitute(interp, item, parameters, replacements)?);
        }
        return Ok(Value::tuple(substituted));
    }
    // `Callable[[T], int]` stores its parameter list as a `list`. Snapshot it
    // so no list borrow is held while nested aliases are re-subscripted.
    let list_items = match value.kind() {
        ValueKind::List(items) => Some(items.to_vec()),
        _ => None,
    };
    if let Some(items) = list_items {
        let mut substituted = Vec::with_capacity(items.len());
        for item in &items {
            substituted.push(substitute(interp, item, parameters, replacements)?);
        }
        return Ok(Value::list(substituted));
    }
    if matches!(value.kind(), ValueKind::PyClass(_)) {
        return Ok(value.clone());
    }
    // PEP 585 / legacy `typing.List[T]` / `Union[T, …]` aliases are rebuilt
    // directly: `types.GenericAlias` itself is not re-subscriptable here.
    if let Some((origin, args)) =
        pyrust_builtins::generic_alias::as_generic_alias_origin_args(value)
    {
        let old = match args.kind() {
            ValueKind::Tuple(items) => items.to_vec(),
            _ => vec![args.clone()],
        };
        let mut new_args = Vec::with_capacity(old.len());
        for item in old {
            new_args.push(substitute(interp, &item, parameters, replacements)?);
        }
        if pyrust_builtins::generic_alias::as_typing_union_args(value).is_some() {
            return union_of(Value::tuple(new_args));
        }
        return Ok(pyrust_builtins::generic_alias::generic_alias(
            origin,
            Value::tuple(new_args),
        ));
    }
    let nested = if is_generic_alias_value(value) {
        interp.get_attr(value, "__parameters__")?
    } else {
        return Ok(value.clone());
    };
    let ValueKind::Tuple(nested) = nested.kind() else {
        return Ok(value.clone());
    };
    if nested.is_empty() {
        return Ok(value.clone());
    }
    let mut nested_args = Vec::with_capacity(nested.len());
    for parameter in nested {
        nested_args.push(substitute(interp, parameter, parameters, replacements)?);
    }
    let index = if nested_args.len() == 1 {
        nested_args.pop().expect("one nested argument")
    } else {
        Value::tuple(nested_args)
    };
    interp.eval_index(value, index)
}
