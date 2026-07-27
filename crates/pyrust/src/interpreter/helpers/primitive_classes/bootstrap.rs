/// Build the per-primitive `PyClass` singletons.  Called once per thread
/// (via `thread_local!` init).  Each class's `__init__` slot is the
/// existing builtin constructor (`BuiltinFunction("int")` etc.) so that
/// `T(args)` keeps its existing behaviour through `call_class_expanded`'s
/// primitive short-circuit.
///
/// `#[cold]` + `#[inline(never)]` keeps this one-time init code out of the
/// hot-path icache footprint of `call_function_expanded`, `get_attr`, etc.
/// — observable as a small but uniform speedup on benches that never touch
/// primitive classes (`literal_int`, `literal_dict`).
#[cold]
#[inline(never)]
fn build_primitive_classes() -> PrimitiveClasses {
    #[cold]
    #[inline(never)]
    fn make(name: &'static str, base: Option<Rc<RefCell<PyClass>>>) -> Rc<RefCell<PyClass>> {
        let attrs: IndexMap<String, Value> = IndexMap::new();
        // Note: no `__init__` is installed.  Direct `int(5)` / `str(x)` calls
        // dispatch via `PRIMITIVE_CLASS_DISPATCH` (HashMap → registry fn),
        // bypassing `__init__` entirely.  Installing the BuiltinFunction
        // constructor as `__init__` would leak it to subclasses via
        // `lookup_class_attr`, where `invoke_class_method` prepends the
        // fresh `PyInstance` receiver and breaks the constructor signature
        // (`class S(int): pass; S(5)` → `int(PyInstance, 5)` argument
        // mismatch).  See Copilot review on #463.
        let mut pyclass = PyClass::new(name, name, base.clone(), attrs);
        pyclass.canonical_tag = Some(
            pyrust_core::CanonicalClassTag::from_primitive_name(name)
                .expect("primitive singleton must have a canonical class tag"),
        );
        let class = Rc::new(RefCell::new(pyclass));
        if let Some(b) = base {
            b.borrow()
                .subclasses
                .borrow_mut()
                .push(Rc::downgrade(&class));
        }
        class
    }
    // Issue #1537: every primitive type inherits from `object` in CPython
    // (`int.__bases__ == (object,)`, etc.).  Setting an explicit `base` here
    // lets `lookup_class_attr` walk to `object` and find dunders like
    // `__init_subclass__`, so `hasattr(int, '__init_subclass__')` returns True.
    // The PRIMITIVE_CLASS_DISPATCH table is keyed on the class pointer (not the
    // base), so the fast-path constructor dispatch is unaffected.
    let obj = object_class_singleton();
    let int_class = make("int", Some(Rc::clone(&obj)));
    // `bool` inherits from `int` (CPython: `bool.__bases__ == (int,)`).  Created
    // here (rather than inline in the returned struct) so its bool-owned slot
    // wrappers can be registered below (issue #2424).
    let bool_class = make("bool", Some(Rc::clone(&int_class)));
    let str_class = make("str", Some(Rc::clone(&obj)));
    let list_class = make("list", Some(Rc::clone(&obj)));
    let tuple_class = make("tuple", Some(Rc::clone(&obj)));
    let dict_class = make("dict", Some(Rc::clone(&obj)));
    let set_class = make("set", Some(Rc::clone(&obj)));
    let bytes_class = make("bytes", Some(Rc::clone(&obj)));
    let bytearray_class = make("bytearray", Some(Rc::clone(&obj)));
    let complex_class = make("complex", Some(Rc::clone(&obj)));
    let frozenset_class = make("frozenset", Some(Rc::clone(&obj)));
    let float_class = make("float", Some(Rc::clone(&obj)));
    // Attribute spelling and descriptor category are owned by each primitive's
    // provider metadata in pyrust-builtins.  This bootstrap only associates
    // those immutable specifications with the per-thread class identities.
    for (class, attrs) in [
        (&bool_class, &pyrust_builtins::primitive_class_attrs::BOOL),
        (&bytearray_class, &pyrust_builtins::bytearray::CLASS_ATTRS),
        (&bytes_class, &pyrust_builtins::bytes::CLASS_ATTRS),
        (&complex_class, &pyrust_builtins::complex::CLASS_ATTRS),
        (&dict_class, &pyrust_builtins::dict::CLASS_ATTRS),
        (&float_class, &pyrust_builtins::float::CLASS_ATTRS),
        (&frozenset_class, &pyrust_builtins::frozenset::CLASS_ATTRS),
        (&int_class, &pyrust_builtins::int::CLASS_ATTRS),
        (&list_class, &pyrust_builtins::list::CLASS_ATTRS),
        (&set_class, &pyrust_builtins::set::CLASS_ATTRS),
        (&str_class, &pyrust_builtins::string::CLASS_ATTRS),
        (&tuple_class, &pyrust_builtins::tuple::CLASS_ATTRS),
    ] {
        install_primitive_class_attrs(class, attrs);
    }
    PrimitiveClasses {
        bytearray_class,
        bytes_class,
        complex_class,
        dict_class,
        // Issue #2151: NoneType/NotImplementedType/ellipsis/mappingproxy must
        // inherit from `object` like every other primitive, so that the object
        // dunders (`__eq__`, `__str__`, `__repr__`, `__hash__`, `__doc__`,
        // `__sizeof__`, …) resolve for `None`/`NotImplemented`/`...`.  Without
        // an explicit base these classes ended their MRO at themselves and
        // exposed *no* dunders at all (`hasattr(None, '__eq__')` was False).
        ellipsis_class: make("ellipsis", Some(Rc::clone(&obj))),
        float_class,
        frozenset_class,
        list_class,
        mappingproxy_class: make("mappingproxy", Some(Rc::clone(&obj))),
        none_class: {
            let c = make("NoneType", Some(Rc::clone(&obj)));
            // `None.__bool__()` returns False; `__bool__` is NoneType-specific
            // (not inherited from `object`), so register it on the class.
            c.borrow_mut().attrs.insert(
                "__bool__".to_string(),
                Value::builtin_function("NoneType.__bool__"),
            );
            c
        },
        notimplemented_class: make("NotImplementedType", Some(Rc::clone(&obj))),
        set_class,
        str_class,
        tuple_class,
        bool_class,
        int_class,
    }
}

/// Materialize provider-owned primitive attributes for this thread's class.
///
/// `PrimitiveClassAttrs::iter` filters explicit class/static overrides before
/// yielding ordinary methods, so descriptor categories are correct from the
/// first insertion rather than being repaired by a later overwrite.
#[cold]
#[inline(never)]
fn install_primitive_class_attrs(
    class: &Rc<RefCell<PyClass>>,
    attrs: &'static pyrust_builtins::primitive_class_attrs::PrimitiveClassAttrs,
) {
    use pyrust_builtins::primitive_class_attrs::PrimitiveClassAttrKind;

    for attr in attrs.iter() {
        let dispatch_key = registered_builtin_method_name(attrs.type_name, attr.name);
        match attr.kind {
            PrimitiveClassAttrKind::NativeClassMethod => {
                install_native_class_method(class, attr.name, dispatch_key);
            }
            PrimitiveClassAttrKind::NativeStaticMethod => {
                install_native_static_method(class, attr.name, dispatch_key);
            }
            PrimitiveClassAttrKind::InstanceMethod
            | PrimitiveClassAttrKind::Init
            | PrimitiveClassAttrKind::New
            | PrimitiveClassAttrKind::ClassGetItem
            | PrimitiveClassAttrKind::OwnedSlot => {
                class
                    .borrow_mut()
                    .attrs
                    .insert(attr.name.to_string(), Value::builtin_function(dispatch_key));
            }
        }
    }

    // Slot spelling remains owned by the interpreter's typed slot policy. It
    // is consumed here, beside the provider metadata, so bootstrap has only
    // one primitive-type inventory. SLOT_ATTR rows provide the unbound
    // type-level form and subclass `super()` resolution.
    if attrs.slot_attr_policy
        == pyrust_builtins::primitive_class_attrs::PrimitiveSlotAttrPolicy::MaterializeDeclared
    {
        for (dunder, flags) in builtin_methods::slot_dunder_table(attrs.type_name) {
            if flags & builtin_methods::SLOT_ATTR == 0
                || attrs.explicit_kind(dunder) == Some(PrimitiveClassAttrKind::OwnedSlot)
            {
                continue;
            }
            let dispatch_key = registered_builtin_method_name(attrs.type_name, dunder);
            class
                .borrow_mut()
                .attrs
                .insert(dunder.to_string(), Value::builtin_function(dispatch_key));
        }
    }
}

/// Install a C-style classmethod descriptor with immutable ownership supplied
/// by the primitive class being built.
fn install_native_class_method(
    class: &Rc<RefCell<PyClass>>,
    name: &'static str,
    dispatch_key: &'static str,
) {
    let descriptor = pyrust_builtins::classmethod::native_class_method_descriptor(
        Value::builtin_function(dispatch_key),
        class,
        name,
    );
    class
        .borrow_mut()
        .attrs
        .insert(name.to_string(), descriptor);
}

/// Install a real `staticmethod` whose wrapped native callable is stable
/// across class and instance access, matching CPython's descriptor identity.
fn install_native_static_method(
    class: &Rc<RefCell<PyClass>>,
    name: &'static str,
    dispatch_key: &'static str,
) {
    let callable = pyrust_builtins::native_builtin_callable::native_static_builtin(
        Value::builtin_function(dispatch_key),
        class,
        name,
    );
    class.borrow_mut().attrs.insert(
        name.to_string(),
        pyrust_builtins::classmethod::static_method_any(callable),
    );
}
