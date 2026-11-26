// PEP 695 TypeAliasType / TypeVar runtime objects. These are language object
// definitions, not VM execution machinery.

include!("type_objects/value_class.rs");
include!("type_objects/type_names.rs");
include!("type_objects/member_descriptors.rs");

// Thread-local justification:
// (a) Why `thread_local!`: Stores `!Send` `Rc<RefCell<PyClass>>` singletons for
//     runtime type objects (`TypeAliasType`, `TypeVar`, `ParamSpec`,
//     `TypeVarTuple`, `UnionType`, `GenericAlias`, etc.). Each singleton sits
//     behind a lazy `OnceCell` so programs that never touch `typing` / PEP 695
//     allocate none of them; predicates inspect `PyClass::canonical_tag` with
//     zero TLS access and zero allocation.
// (b) Multi-interpreter / multi-threaded impact: Multiple interpreters on one
//     thread share these built-in runtime type singletons; separate threads
//     initialize independent class singletons.
thread_local! {
    /// Class singleton for `TypeAliasType` objects created by `type X = ...`.
    static TYPE_ALIAS_CLASS: std::cell::OnceCell<Rc<RefCell<PyClass>>> =
        const { std::cell::OnceCell::new() };

    /// Class singleton for `TypeVar` objects created by generic type params.
    static TYPEVAR_CLASS: std::cell::OnceCell<Rc<RefCell<PyClass>>> =
        const { std::cell::OnceCell::new() };

    /// Class singleton for `typing.ParamSpec` (shared by PEP 695 `[**P]` and
    /// manual `typing.ParamSpec("P")`, canonical across `typing` reloads).
    static PARAMSPEC_CLASS: std::cell::OnceCell<Rc<RefCell<PyClass>>> =
        const { std::cell::OnceCell::new() };

    /// Class singleton for `typing.ParamSpecArgs`.
    static PARAMSPEC_ARGS_CLASS: std::cell::OnceCell<Rc<RefCell<PyClass>>> =
        const { std::cell::OnceCell::new() };

    /// Class singleton for `typing.ParamSpecKwargs`.
    static PARAMSPEC_KWARGS_CLASS: std::cell::OnceCell<Rc<RefCell<PyClass>>> =
        const { std::cell::OnceCell::new() };

    /// Class singleton for `typing.TypeVarTuple` (shared by PEP 695 `[*Ts]` and
    /// manual `typing.TypeVarTuple("Ts")`, canonical across `typing` reloads).
    static TYPEVARTUPLE_CLASS: std::cell::OnceCell<Rc<RefCell<PyClass>>> =
        const { std::cell::OnceCell::new() };
}

#[cold]
#[inline(never)]
fn build_type_alias_class() -> Rc<RefCell<PyClass>> {
    let mut attrs: IndexMap<String, Value> = IndexMap::new();
    // __repr__ is handled by the instance's __name__ attribute via a
    // builtin function registered as "builtins.TypeAliasType.__repr__".
    attrs.insert(
        "__repr__".to_string(),
        Value::builtin_function("builtins.TypeAliasType.__repr__"),
    );
    // PEP 695: generic aliases are subscriptable.  The operator form
    // `Pair[int]` is served by the inline fast path in `eval_index`; this
    // slot makes `hasattr(alias, "__getitem__")` True and lets an explicit
    // `alias.__getitem__(x)` call work, matching CPython 3.12 (issue #2779).
    attrs.insert(
        "__getitem__".to_string(),
        Value::builtin_function("builtins.TypeAliasType.__getitem__"),
    );
    attrs.insert(
        "__init__".to_string(),
        Value::builtin_function("builtins.TypeAliasType.__init__"),
    );
    // CPython exposes `TypeAliasType` from `typing`, so
    // `type(my_alias).__module__ == "typing"` and the bare class reprs as
    // `<class 'typing.TypeAliasType'>` (issue #2779).
    attrs.insert("__module__".to_string(), Value::string("typing"));
    let mut class = PyClass::new("TypeAliasType", "TypeAliasType", None, attrs);
    class.canonical_tag = Some(pyrust_core::CanonicalClassTag::TypeAliasType);
    class.non_subclassable_name = Some("typing.TypeAliasType");
    Rc::new(RefCell::new(class))
}

#[cold]
#[inline(never)]
fn build_typevar_class() -> Rc<RefCell<PyClass>> {
    let mut attrs: IndexMap<String, Value> = IndexMap::new();
    // `typing.TypeVar(...)` and PEP 695 syntax create the same concrete
    // runtime type in CPython.  Keep the canonical class here and attach
    // the public constructor protocol instead of letting each `typing`
    // module generation synthesize a replacement class.
    attrs.insert(
        "__init__".to_string(),
        Value::builtin_function("typing.TypeVar.__init__"),
    );
    attrs.insert(
        "__repr__".to_string(),
        Value::builtin_function("builtins.TypeVar.__repr__"),
    );
    attrs.insert("__module__".to_string(), Value::string("typing"));
    let mut class = PyClass::new("TypeVar", "TypeVar", None, attrs);
    class.canonical_tag = Some(pyrust_core::CanonicalClassTag::TypeVar);
    class.non_subclassable_name = Some("typing.TypeVar");
    Rc::new(RefCell::new(class))
}

#[cold]
#[inline(never)]
fn build_paramspec_class() -> Rc<RefCell<PyClass>> {
    let mut attrs: IndexMap<String, Value> = IndexMap::new();
    attrs.insert(
        "__init__".to_string(),
        Value::builtin_function("builtins.ParamSpec.__init__"),
    );
    attrs.insert(
        "__repr__".to_string(),
        Value::builtin_function("builtins.ParamSpec.__repr__"),
    );
    attrs.insert(
        "__or__".to_string(),
        Value::builtin_function("builtins.ParamSpec.__or__"),
    );
    attrs.insert(
        "__ror__".to_string(),
        Value::builtin_function("builtins.ParamSpec.__ror__"),
    );
    attrs.insert(
        "args".to_string(),
        pyrust_builtins::property::property(
            Value::builtin_function("builtins.ParamSpec._get_args"),
            Value::none(),
            Value::none(),
        ),
    );
    attrs.insert(
        "kwargs".to_string(),
        pyrust_builtins::property::property(
            Value::builtin_function("builtins.ParamSpec._get_kwargs"),
            Value::none(),
            Value::none(),
        ),
    );
    attrs.insert("__module__".to_string(), Value::string("typing"));
    let mut class = PyClass::new("ParamSpec", "ParamSpec", None, attrs);
    class.canonical_tag = Some(pyrust_core::CanonicalClassTag::ParamSpec);
    class.non_subclassable_name = Some("typing.ParamSpec");
    Rc::new(RefCell::new(class))
}

#[cold]
#[inline(never)]
fn build_paramspec_args_class() -> Rc<RefCell<PyClass>> {
    let mut attrs: IndexMap<String, Value> = IndexMap::new();
    attrs.insert(
        "__init__".to_string(),
        Value::builtin_function("builtins.ParamSpecArgs.__init__"),
    );
    attrs.insert(
        "__repr__".to_string(),
        Value::builtin_function("builtins.ParamSpecArgs.__repr__"),
    );
    attrs.insert(
        "__eq__".to_string(),
        Value::builtin_function("builtins.ParamSpecArgs.__eq__"),
    );
    attrs.insert("__hash__".to_string(), Value::none());
    attrs.insert("__module__".to_string(), Value::string("typing"));
    let mut class = PyClass::new("ParamSpecArgs", "ParamSpecArgs", None, attrs);
    class.canonical_tag = Some(pyrust_core::CanonicalClassTag::ParamSpecArgs);
    class.non_subclassable_name = Some("typing.ParamSpecArgs");
    Rc::new(RefCell::new(class))
}

#[cold]
#[inline(never)]
fn build_paramspec_kwargs_class() -> Rc<RefCell<PyClass>> {
    let mut attrs: IndexMap<String, Value> = IndexMap::new();
    attrs.insert(
        "__init__".to_string(),
        Value::builtin_function("builtins.ParamSpecKwargs.__init__"),
    );
    attrs.insert(
        "__repr__".to_string(),
        Value::builtin_function("builtins.ParamSpecKwargs.__repr__"),
    );
    attrs.insert(
        "__eq__".to_string(),
        Value::builtin_function("builtins.ParamSpecKwargs.__eq__"),
    );
    attrs.insert("__hash__".to_string(), Value::none());
    attrs.insert("__module__".to_string(), Value::string("typing"));
    let mut class = PyClass::new("ParamSpecKwargs", "ParamSpecKwargs", None, attrs);
    class.canonical_tag = Some(pyrust_core::CanonicalClassTag::ParamSpecKwargs);
    class.non_subclassable_name = Some("typing.ParamSpecKwargs");
    Rc::new(RefCell::new(class))
}

#[cold]
#[inline(never)]
fn build_typevartuple_class() -> Rc<RefCell<PyClass>> {
    let mut attrs: IndexMap<String, Value> = IndexMap::new();
    attrs.insert(
        "__init__".to_string(),
        Value::builtin_function("builtins.TypeVarTuple.__init__"),
    );
    attrs.insert(
        "__repr__".to_string(),
        Value::builtin_function("builtins.TypeVarTuple.__repr__"),
    );
    attrs.insert(
        "__iter__".to_string(),
        Value::builtin_function("builtins.TypeVarTuple.__iter__"),
    );
    attrs.insert("__module__".to_string(), Value::string("typing"));
    let mut class = PyClass::new("TypeVarTuple", "TypeVarTuple", None, attrs);
    class.canonical_tag = Some(pyrust_core::CanonicalClassTag::TypeVarTuple);
    class.non_subclassable_name = Some("typing.TypeVarTuple");
    Rc::new(RefCell::new(class))
}

/// Construct an (initially unbounded) `TypeVar` `PyInstance` with `__name__`,
/// `__constraints__`, and `__bound__` attributes, matching the observable
/// surface of CPython's `typing.TypeVar` as created by PEP 695 type parameter
/// syntax.  `__bound__` starts as `None` and `__constraints__` as `()`; a
/// bounded/constrained parameter's clause is evaluated lazily (after every type
/// parameter is in scope) and written back via `SetTypeVarAttr` — see
/// `Compiler::emit_typevar_bound`.
pub(crate) fn make_typevar_instance(name: String) -> Value {
    let cls = typevar_class_singleton();
    let mut attrs = InstanceAttrs::new();
    attrs.insert("__name__", Value::string(name));
    attrs.insert("__constraints__", Value::tuple(vec![]));
    attrs.insert("__bound__", Value::none());
    attrs.insert("__covariant__", Value::bool_(false));
    attrs.insert("__contravariant__", Value::bool_(false));
    // PEP 695 parameters infer variance and therefore repr without the
    // legacy `~` prefix used by manually-created invariant TypeVars.
    attrs.insert("__infer_variance__", Value::bool_(true));
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class: cls, attrs })))
}

/// True if `class` is the PEP 695 `TypeVar` singleton.  Zero-allocation and
/// zero-TLS: reads the immutable `PyClass::canonical_tag` directly.
#[inline]
pub(crate) fn is_typevar_class(class: &Rc<RefCell<PyClass>>) -> bool {
    class.borrow().canonical_tag == Some(pyrust_core::CanonicalClassTag::TypeVar)
}

/// Classify a would-be write/delete of `name` on a `TypeVar` instance against
/// CPython 3.12's read-only getset descriptors.  Returns the exact
/// `AttributeError` message CPython raises, or `None` if the name is not a
/// protected descriptor (arbitrary attributes are writable, matching CPython).
///
///   * `__bound__` / `__constraints__` raise
///     `attribute '<name>' of 'typing.TypeVar' objects is not writable`
///   * `__name__` / `__covariant__` / `__contravariant__` /
///     `__infer_variance__` raise the generic `readonly attribute`.
pub(crate) fn typevar_readonly_attr_error(name: &str) -> Option<String> {
    match name {
        "__bound__" | "__constraints__" => Some(format!(
            "attribute '{name}' of 'typing.TypeVar' objects is not writable"
        )),
        "__name__" | "__covariant__" | "__contravariant__" | "__infer_variance__" => {
            Some("readonly attribute".to_string())
        }
        _ => None,
    }
}

fn make_lazy_type_alias_instance(
    name: String,
    value_thunk: Value,
    type_params: Value,
    module: String,
) -> Value {
    let cls = type_alias_class_singleton();
    let mut attrs = InstanceAttrs::new();
    attrs.insert("__name__", Value::string(name));
    attrs.insert("__evaluate_value__", value_thunk);
    attrs.insert("__type_params__", type_params);
    attrs.insert("__module__", Value::string(module));
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class: cls, attrs })))
}

/// The PEP 695 `TypeAliasType` class singleton, so the `typing` module can
/// export it (`typing.TypeAliasType`) and `type(my_alias) is
/// typing.TypeAliasType` holds (issue #2779).
pub(crate) fn type_alias_class_singleton() -> Rc<RefCell<PyClass>> {
    TYPE_ALIAS_CLASS.with(|cell| Rc::clone(cell.get_or_init(build_type_alias_class)))
}

/// The canonical `typing.TypeVar` class shared by manual construction and
/// PEP 695 syntax, and retained across public `typing` module generations.
pub(crate) fn typevar_class_singleton() -> Rc<RefCell<PyClass>> {
    TYPEVAR_CLASS.with(|cell| Rc::clone(cell.get_or_init(build_typevar_class)))
}

/// The canonical `typing.ParamSpec` class singleton.
pub(crate) fn paramspec_class_singleton() -> Rc<RefCell<PyClass>> {
    PARAMSPEC_CLASS.with(|cell| Rc::clone(cell.get_or_init(build_paramspec_class)))
}

/// The canonical `typing.ParamSpecArgs` class singleton.
pub(crate) fn paramspec_args_class_singleton() -> Rc<RefCell<PyClass>> {
    PARAMSPEC_ARGS_CLASS.with(|cell| Rc::clone(cell.get_or_init(build_paramspec_args_class)))
}

/// The canonical `typing.ParamSpecKwargs` class singleton.
pub(crate) fn paramspec_kwargs_class_singleton() -> Rc<RefCell<PyClass>> {
    PARAMSPEC_KWARGS_CLASS.with(|cell| Rc::clone(cell.get_or_init(build_paramspec_kwargs_class)))
}

/// The canonical `typing.TypeVarTuple` class singleton.
pub(crate) fn typevartuple_class_singleton() -> Rc<RefCell<PyClass>> {
    TYPEVARTUPLE_CLASS.with(|cell| Rc::clone(cell.get_or_init(build_typevartuple_class)))
}

#[inline]
pub(crate) fn is_paramspec_class(class: &Rc<RefCell<PyClass>>) -> bool {
    class.borrow().canonical_tag == Some(pyrust_core::CanonicalClassTag::ParamSpec)
}

#[inline]
pub(crate) fn is_paramspec_args_class(class: &Rc<RefCell<PyClass>>) -> bool {
    class.borrow().canonical_tag == Some(pyrust_core::CanonicalClassTag::ParamSpecArgs)
}

#[inline]
pub(crate) fn is_paramspec_kwargs_class(class: &Rc<RefCell<PyClass>>) -> bool {
    class.borrow().canonical_tag == Some(pyrust_core::CanonicalClassTag::ParamSpecKwargs)
}

#[inline]
pub(crate) fn is_typevartuple_class(class: &Rc<RefCell<PyClass>>) -> bool {
    class.borrow().canonical_tag == Some(pyrust_core::CanonicalClassTag::TypeVarTuple)
}

/// Construct a PEP 695 `ParamSpec` instance (inferred variance, bare repr).
pub(crate) fn make_paramspec_instance(name: String) -> Value {
    let cls = paramspec_class_singleton();
    let mut attrs = InstanceAttrs::new();
    attrs.insert("__name__", Value::string(name));
    attrs.insert("__bound__", Value::none());
    attrs.insert("__covariant__", Value::bool_(false));
    attrs.insert("__contravariant__", Value::bool_(false));
    attrs.insert("__infer_variance__", Value::bool_(true));
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class: cls, attrs })))
}

/// Construct a `ParamSpecArgs` proxy for `origin`.
pub(crate) fn make_paramspec_args_instance(origin: Value) -> Value {
    let cls = paramspec_args_class_singleton();
    let mut attrs = InstanceAttrs::new();
    attrs.insert("__origin__", origin);
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class: cls, attrs })))
}

/// Construct a `ParamSpecKwargs` proxy for `origin`.
pub(crate) fn make_paramspec_kwargs_instance(origin: Value) -> Value {
    let cls = paramspec_kwargs_class_singleton();
    let mut attrs = InstanceAttrs::new();
    attrs.insert("__origin__", origin);
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class: cls, attrs })))
}

/// Construct a PEP 695 `TypeVarTuple` instance.
pub(crate) fn make_typevartuple_instance(name: String) -> Value {
    let cls = typevartuple_class_singleton();
    let mut attrs = InstanceAttrs::new();
    attrs.insert("__name__", Value::string(name));
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class: cls, attrs })))
}

pub(crate) fn paramspec_readonly_attr_error(name: &str) -> Option<String> {
    match name {
        "args" | "kwargs" => Some(format!(
            "attribute '{name}' of 'typing.ParamSpec' objects is not writable"
        )),
        "__name__" | "__bound__" | "__covariant__" | "__contravariant__" | "__infer_variance__" => {
            Some("readonly attribute".to_string())
        }
        _ => None,
    }
}

pub(crate) fn typevartuple_readonly_attr_error(name: &str) -> Option<String> {
    match name {
        "__name__" => Some("readonly attribute".to_string()),
        _ => None,
    }
}

pub(crate) fn paramspec_proxy_readonly_attr_error(name: &str) -> Option<String> {
    match name {
        "__origin__" => Some("readonly attribute".to_string()),
        _ => None,
    }
}

/// True if `class` is the PEP 695 `TypeAliasType` singleton.  Zero-allocation
/// and zero-TLS: reads the immutable `PyClass::canonical_tag` directly.
#[inline]
pub(crate) fn is_type_alias_class(class: &Rc<RefCell<PyClass>>) -> bool {
    class.borrow().canonical_tag == Some(pyrust_core::CanonicalClassTag::TypeAliasType)
}

#[cfg(test)]
pub(crate) fn typing_class_singletons_initialized() -> [(&'static str, bool); 6] {
    [
        (
            "TypeAliasType",
            TYPE_ALIAS_CLASS.with(|cell| cell.get().is_some()),
        ),
        ("TypeVar", TYPEVAR_CLASS.with(|cell| cell.get().is_some())),
        (
            "ParamSpec",
            PARAMSPEC_CLASS.with(|cell| cell.get().is_some()),
        ),
        (
            "ParamSpecArgs",
            PARAMSPEC_ARGS_CLASS.with(|cell| cell.get().is_some()),
        ),
        (
            "ParamSpecKwargs",
            PARAMSPEC_KWARGS_CLASS.with(|cell| cell.get().is_some()),
        ),
        (
            "TypeVarTuple",
            TYPEVARTUPLE_CLASS.with(|cell| cell.get().is_some()),
        ),
    ]
}

/// Classify writes/deletes against `TypeAliasType`'s C-level read-only
/// members.  The constructor and compiler initialize these fields directly;
/// Python-visible mutation must not change alias identity or meaning.
pub(crate) fn type_alias_readonly_attr_error(name: &str) -> Option<String> {
    match name {
        "__name__" => Some("readonly attribute".to_string()),
        "__value__" | "__type_params__" | "__module__" => Some(format!(
            "attribute '{name}' of 'typing.TypeAliasType' objects is not writable"
        )),
        _ => None,
    }
}

fn syntax_object_name(value: &Value, opcode: &str) -> Result<String> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| PyError::runtime(format!("{opcode}: name must be a string constant")))
}

/// Construct a TypeVar, ParamSpec, or TypeVarTuple from `MakeTypeVar`.
pub(crate) fn make_type_param_from_syntax(
    name: &Value,
    kind: crate::ast::TypeParamKind,
) -> Result<Value> {
    let name = syntax_object_name(name, "MakeTypeVar")?;
    Ok(match kind {
        crate::ast::TypeParamKind::TypeVar => make_typevar_instance(name),
        crate::ast::TypeParamKind::ParamSpec => make_paramspec_instance(name),
        crate::ast::TypeParamKind::TypeVarTuple => make_typevartuple_instance(name),
    })
}

/// Construct a TypeAliasType from the operands of `MakeTypeAlias`.
pub(crate) fn make_type_alias_from_syntax(
    name: &Value,
    value_thunk: Value,
    type_params: Value,
    module: String,
) -> Result<Value> {
    Ok(make_lazy_type_alias_instance(
        syntax_object_name(name, "MakeTypeAlias")?,
        value_thunk,
        type_params,
        module,
    ))
}

/// Populate a freshly-created TypeVar's lazy bound/constraint storage.
///
/// This deliberately bypasses the Python-visible read-only descriptor guard;
/// only the dedicated compiler opcode calls it.
pub(crate) fn initialize_typevar_attr(target: &Value, name: &str, value: Value) {
    if let ValueKind::PyInstance(instance) = target.kind() {
        instance.borrow_mut().attrs.insert(name, value);
    }
}
