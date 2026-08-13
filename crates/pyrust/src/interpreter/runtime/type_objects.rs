// PEP 695 TypeAliasType / TypeVar runtime objects. These are language object
// definitions, not VM execution machinery.

include!("type_objects/value_class.rs");
include!("type_objects/type_names.rs");
include!("type_objects/member_descriptors.rs");

thread_local! {
    /// Class singleton for `TypeAliasType` objects created by `type X = ...`.
    static TYPE_ALIAS_CLASS: Rc<RefCell<PyClass>> = {
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
        let mut class = PyClass::new(
            "TypeAliasType",
            "TypeAliasType",
            None,
            attrs,
        );
        class.non_subclassable_name = Some("typing.TypeAliasType");
        Rc::new(RefCell::new(class))
    };

    /// Class singleton for `TypeVar` objects created by generic type params.
    static TYPEVAR_CLASS: Rc<RefCell<PyClass>> = {
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
    };
}

/// Construct an (initially unbounded) `TypeVar` `PyInstance` with `__name__`,
/// `__constraints__`, and `__bound__` attributes, matching the observable
/// surface of CPython's `typing.TypeVar` as created by PEP 695 type parameter
/// syntax.  `__bound__` starts as `None` and `__constraints__` as `()`; a
/// bounded/constrained parameter's clause is evaluated lazily (after every type
/// parameter is in scope) and written back via `SetTypeVarAttr` — see
/// `Compiler::emit_typevar_bound`.
pub(crate) fn make_typevar_instance(name: String) -> Value {
    TYPEVAR_CLASS.with(|cls| {
        let mut attrs = InstanceAttrs::new();
        attrs.insert("__name__", Value::string(name));
        attrs.insert("__constraints__", Value::tuple(vec![]));
        attrs.insert("__bound__", Value::none());
        attrs.insert("__covariant__", Value::bool_(false));
        attrs.insert("__contravariant__", Value::bool_(false));
        // PEP 695 parameters infer variance and therefore repr without the
        // legacy `~` prefix used by manually-created invariant TypeVars.
        attrs.insert("__infer_variance__", Value::bool_(true));
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(cls),
            attrs,
        })))
    })
}

/// True if `class` is the PEP 695 `TypeVar` singleton.  Used by the attribute
/// assignment / deletion slow paths to enforce CPython's read-only getset
/// descriptors on TypeVar objects.
pub(crate) fn is_typevar_class(class: &Rc<RefCell<PyClass>>) -> bool {
    TYPEVAR_CLASS.with(|cls| Rc::ptr_eq(class, cls))
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
    TYPE_ALIAS_CLASS.with(|cls| {
        let mut attrs = InstanceAttrs::new();
        attrs.insert("__name__", Value::string(name));
        attrs.insert("__evaluate_value__", value_thunk);
        attrs.insert("__type_params__", type_params);
        attrs.insert("__module__", Value::string(module));
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(cls),
            attrs,
        })))
    })
}

/// The PEP 695 `TypeAliasType` class singleton, so the `typing` module can
/// export it (`typing.TypeAliasType`) and `type(my_alias) is
/// typing.TypeAliasType` holds (issue #2779).
pub(crate) fn type_alias_class_singleton() -> Rc<RefCell<PyClass>> {
    TYPE_ALIAS_CLASS.with(Rc::clone)
}

/// The canonical `typing.TypeVar` class shared by manual construction and
/// PEP 695 syntax, and retained across public `typing` module generations.
pub(crate) fn typevar_class_singleton() -> Rc<RefCell<PyClass>> {
    TYPEVAR_CLASS.with(Rc::clone)
}

/// True if `class` is the PEP 695 `TypeAliasType` singleton.  Used by the
/// subscript path to give `Pair[int]` a `types.GenericAlias` while a
/// non-generic alias raises CPython's "Only generic type aliases are
/// subscriptable" (issue #2779).
pub(crate) fn is_type_alias_class(class: &Rc<RefCell<PyClass>>) -> bool {
    TYPE_ALIAS_CLASS.with(|cls| Rc::ptr_eq(class, cls))
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
        .ok_or_else(|| PyError::Runtime(format!("{opcode}: name must be a string constant")))
}

/// Construct a TypeVar from the constant operand of `MakeTypeVar`.
pub(crate) fn make_typevar_from_syntax(name: &Value) -> Result<Value> {
    Ok(make_typevar_instance(syntax_object_name(
        name,
        "MakeTypeVar",
    )?))
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
