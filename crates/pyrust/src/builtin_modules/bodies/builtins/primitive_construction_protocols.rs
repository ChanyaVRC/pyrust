use pyrust_derive::pyrust_module;

pyrust_module! {
    /// Issue #1143: `tuple.__new__(cls, iterable=())` — allocator for tuple
    /// subclasses. Creates a `PyInstance` of `cls` with the tuple backing store
    /// (`__builtin_data__`) populated from `iterable`. Called when a tuple
    /// subclass's `__new__` calls `super().__new__(cls, it)`.
    ///
    /// CPython signature: `tuple.__new__(cls, iterable=(), /)`
    #[py_name = "tuple.__new__"]
    fn tuple_new_dunder(args) -> Result<Value> {
        let (cls_val, rest) = match args {
            [] => {
                return Err(PyError::named(
                    "TypeError",
                    "tuple.__new__(): not enough arguments".to_string(),
                ));
            }
            [first, rest @ ..] => (first.value.clone(), rest),
        };
        let class_rc = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "tuple.__new__(X): X is not a type object ({})",
                        value_type_name_str(&cls_val)
                    ),
                ));
            }
        };
        check_new_subtype(&class_rc, "tuple")?;
        let backing = match rest {
            [] => Value::tuple(vec![]),
            [single] => Value::tuple(_interp.collect_iterable(&single.value)?),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "tuple expected at most 1 argument, got {}",
                        rest.len()
                    ),
                ));
            }
        };
        let mut attrs = InstanceAttrs::new();
        attrs.insert(
            crate::interpreter::BUILTIN_DATA_ATTR,
            backing,
        );
        Ok(Value::py_instance(Rc::new(std::cell::RefCell::new(
            crate::value::PyInstance {
                class: class_rc,
                attrs,
            },
        ))))
    }

    /// Issue #1143: `frozenset.__new__(cls, iterable=())` — allocator for
    /// frozenset subclasses.  Creates a `PyInstance` of `cls` with the
    /// frozenset backing store (`__builtin_data__`) populated from `iterable`.
    ///
    /// CPython signature: `frozenset.__new__(cls, iterable=(), /)`
    #[py_name = "frozenset.__new__"]
    fn frozenset_new_dunder(args) -> Result<Value> {
        let (cls_val, rest) = match args {
            [] => {
                return Err(PyError::named(
                    "TypeError",
                    "frozenset.__new__(): not enough arguments".to_string(),
                ));
            }
            [first, rest @ ..] => (first.value.clone(), rest),
        };
        let class_rc = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "frozenset.__new__(X): X is not a type object ({})",
                        value_type_name_str(&cls_val)
                    ),
                ));
            }
        };
        check_new_subtype(&class_rc, "frozenset")?;
        let backing = match rest {
            [] => pyrust_builtins::frozenset::frozenset(PySet::default()),
            [single] => {
                if let Some(source) = pyrust_builtins::frozenset::as_items(&single.value) {
                    pyrust_builtins::frozenset::frozenset(PySet::cpython_merged_copy(&source))
                } else if let ValueKind::Set(source) = single.value.kind() {
                    pyrust_builtins::frozenset::frozenset(PySet::cpython_merged_copy(&source))
                } else if let ValueKind::Dict(source) = single.value.kind() {
                    let keys: Vec<pyrust_core::PyKey> = source.keys().cloned().collect();
                    let mut set = PySet::with_cpython_dict_capacity(keys.len());
                    for key in keys {
                        _interp.set_insert(&mut set, key)?;
                    }
                    pyrust_builtins::frozenset::frozenset(set)
                } else {
                let items = _interp.collect_iterable(&single.value)?;
                let mut set: PySet = PySet::default();
                for item in items {
                    let key = _interp.value_to_pykey(&item)?;
                    _interp.set_insert(&mut set, key)?;
                }
                pyrust_builtins::frozenset::frozenset(set)
                }
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "frozenset expected at most 1 argument, got {}",
                        rest.len()
                    ),
                ));
            }
        };
        let mut attrs = InstanceAttrs::new();
        attrs.insert(
            crate::interpreter::BUILTIN_DATA_ATTR,
            backing,
        );
        Ok(Value::py_instance(Rc::new(std::cell::RefCell::new(
            crate::value::PyInstance {
                class: class_rc,
                attrs,
            },
        ))))
    }

    /// Issue #2619: `bool.__new__(cls, x=False)` — applies truthiness
    /// conversion and returns a canonical bool.  `bool` is final in CPython,
    /// so `cls` is always `bool` and the result is `True if x else False`.
    /// Without this dedicated handler `bool.__new__` would inherit
    /// `int.__new__`, returning an int-backed value tagged as bool.
    ///
    /// CPython signature: `bool.__new__(cls, x=False, /)`
    #[py_name = "bool.__new__"]
    fn bool_new_dunder(args) -> Result<Value> {
        let (cls_val, rest) = match args {
            [] => {
                return Err(PyError::named(
                    "TypeError",
                    "bool.__new__(): not enough arguments".to_string(),
                ));
            }
            [first, rest @ ..] => (first.value.clone(), rest),
        };
        let class_rc = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "bool.__new__(X): X is not a type object ({})",
                        value_type_name_str(&cls_val)
                    ),
                ));
            }
        };
        check_new_subtype(&class_rc, "bool")?;
        match rest {
            [] => Ok(Value::bool_(false)),
            [x] => Ok(Value::bool_(_interp.truthy_value(&x.value)?)),
            _ => Err(PyError::named(
                "TypeError",
                format!("bool expected at most 1 argument, got {}", rest.len()),
            )),
        }
    }

    /// Issue #1465: `int.__new__(cls, x=0)` — allocator for int subclasses.
    /// Creates a `PyInstance` of `cls` with the int backing store
    /// (`__builtin_data__`) populated from the constructor arguments.
    /// Called when an `int` subclass's `__new__` calls `super().__new__(cls, val)`.
    ///
    /// CPython signature: `int.__new__(cls, x=0, /)`
    #[py_name = "int.__new__"]
    fn int_new_dunder(args) -> Result<Value> {
        let (cls_val, rest) = match args {
            [] => {
                return Err(PyError::named(
                    "TypeError",
                    "int.__new__(): not enough arguments".to_string(),
                ));
            }
            [first, rest @ ..] => (first.value.clone(), rest),
        };
        let class_rc = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "int.__new__(X): X is not a type object ({})",
                        value_type_name_str(&cls_val)
                    ),
                ));
            }
        };
        check_new_subtype(&class_rc, "int")?;
        let backing = if let Some(dispatch) = crate::builtin_registry::lookup("int") {
            dispatch(_interp, rest)?
        } else {
            return Err(PyError::Runtime("internal: int not in registry".to_string()));
        };
        let mut attrs = InstanceAttrs::new();
        attrs.insert(
            crate::interpreter::BUILTIN_DATA_ATTR,
            backing,
        );
        Ok(Value::py_instance(Rc::new(std::cell::RefCell::new(
            crate::value::PyInstance {
                class: class_rc,
                attrs,
            },
        ))))
    }

    /// Issue #1465: `str.__new__(cls, object='')` — allocator for str subclasses.
    /// Creates a `PyInstance` of `cls` with the str backing store
    /// (`__builtin_data__`) populated from the constructor arguments.
    /// Called when a `str` subclass's `__new__` calls `super().__new__(cls, val)`.
    ///
    /// CPython signature: `str.__new__(cls, object='', /)`
    #[py_name = "str.__new__"]
    fn str_new_dunder(args) -> Result<Value> {
        let (cls_val, rest) = match args {
            [] => {
                return Err(PyError::named(
                    "TypeError",
                    "str.__new__(): not enough arguments".to_string(),
                ));
            }
            [first, rest @ ..] => (first.value.clone(), rest),
        };
        let class_rc = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "str.__new__(X): X is not a type object ({})",
                        value_type_name_str(&cls_val)
                    ),
                ));
            }
        };
        check_new_subtype(&class_rc, "str")?;
        let backing = if let Some(dispatch) = crate::builtin_registry::lookup("str") {
            dispatch(_interp, rest)?
        } else {
            return Err(PyError::Runtime("internal: str not in registry".to_string()));
        };
        let mut attrs = InstanceAttrs::new();
        attrs.insert(
            crate::interpreter::BUILTIN_DATA_ATTR,
            backing,
        );
        Ok(Value::py_instance(Rc::new(std::cell::RefCell::new(
            crate::value::PyInstance {
                class: class_rc,
                attrs,
            },
        ))))
    }

    /// Issue #1465: `float.__new__(cls, x=0.0)` — allocator for float subclasses.
    /// Creates a `PyInstance` of `cls` with the float backing store
    /// (`__builtin_data__`) populated from the constructor arguments.
    /// Called when a `float` subclass's `__new__` calls `super().__new__(cls, val)`.
    ///
    /// CPython signature: `float.__new__(cls, x=0.0, /)`
    #[py_name = "float.__new__"]
    fn float_new_dunder(args) -> Result<Value> {
        let (cls_val, rest) = match args {
            [] => {
                return Err(PyError::named(
                    "TypeError",
                    "float.__new__(): not enough arguments".to_string(),
                ));
            }
            [first, rest @ ..] => (first.value.clone(), rest),
        };
        let class_rc = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "float.__new__(X): X is not a type object ({})",
                        value_type_name_str(&cls_val)
                    ),
                ));
            }
        };
        check_new_subtype(&class_rc, "float")?;
        let backing = if let Some(dispatch) = crate::builtin_registry::lookup("float") {
            dispatch(_interp, rest)?
        } else {
            return Err(PyError::Runtime(
                "internal: float not in registry".to_string(),
            ));
        };
        let mut attrs = InstanceAttrs::new();
        attrs.insert(
            crate::interpreter::BUILTIN_DATA_ATTR,
            backing,
        );
        Ok(Value::py_instance(Rc::new(std::cell::RefCell::new(
            crate::value::PyInstance {
                class: class_rc,
                attrs,
            },
        ))))
    }

    /// Issue #1465: `bytes.__new__(cls, source=b'')` — allocator for bytes subclasses.
    /// Creates a `PyInstance` of `cls` with the bytes backing store
    /// (`__builtin_data__`) populated from the constructor arguments.
    /// Called when a `bytes` subclass's `__new__` calls `super().__new__(cls, val)`.
    ///
    /// CPython signature: `bytes.__new__(cls, source=b'', /)`
    #[py_name = "bytes.__new__"]
    fn bytes_new_dunder(args) -> Result<Value> {
        let (cls_val, rest) = match args {
            [] => {
                return Err(PyError::named(
                    "TypeError",
                    "bytes.__new__(): not enough arguments".to_string(),
                ));
            }
            [first, rest @ ..] => (first.value.clone(), rest),
        };
        let class_rc = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "bytes.__new__(X): X is not a type object ({})",
                        value_type_name_str(&cls_val)
                    ),
                ));
            }
        };
        check_new_subtype(&class_rc, "bytes")?;
        let backing = if let Some(dispatch) = crate::builtin_registry::lookup("bytes") {
            dispatch(_interp, rest)?
        } else {
            return Err(PyError::Runtime(
                "internal: bytes not in registry".to_string(),
            ));
        };
        let mut attrs = InstanceAttrs::new();
        attrs.insert(
            crate::interpreter::BUILTIN_DATA_ATTR,
            backing,
        );
        Ok(Value::py_instance(Rc::new(std::cell::RefCell::new(
            crate::value::PyInstance {
                class: class_rc,
                attrs,
            },
        ))))
    }
}
