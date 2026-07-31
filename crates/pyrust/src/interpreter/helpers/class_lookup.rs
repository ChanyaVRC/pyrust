pub(crate) fn lookup_class_attr(class: &Rc<RefCell<PyClass>>, name: &str) -> Option<Value> {
    // Borrow the class to read its own attrs and recurse into the base chain by
    // reference, cloning out only the matched `Value`.  Distinct classes are
    // distinct `RefCell`s, so recursing under the current borrow never
    // conflicts. Avoiding the previous per-node `base`/`extra_bases`
    // `Rc`+`Vec` clones removes the dominant allocation churn on the
    // exception-construction path, where this is called twice per `raise` (for
    // `__new__` and `__init__`).
    let borrowed = class.borrow();
    if let Some(v) = borrowed.attrs.get(name) {
        return Some(v.clone());
    }
    let has_explicit_base = borrowed.base.is_some();
    // Issue #2075: when a class participates in *multiple* inheritance, plain
    // depth-first recursion ("primary base's full ancestry, then the extra
    // bases") is NOT C3: in a diamond `D(B, C)` it descends `D → B → A` and
    // returns `A`'s attribute before ever considering the sibling `C` that
    // overrides it.  CPython resolves attributes by scanning the C3 `__mro__`
    // left-to-right and returning the first class whose *own* dict defines the
    // name.  Switch to that order here whenever this class has extra bases.
    //
    // The fast single-inheritance path below (no `extra_bases`) is left exactly
    // as before: for a linear chain depth-first recursion already equals C3, so
    // ordinary classes and the hot exception-construction path pay nothing.
    if !borrowed.extra_bases.is_empty() {
        // Drop the borrow before computing the MRO (which borrows each class).
        drop(borrowed);
        for cls in c3_linearize_classes(class) {
            if let Some(v) = cls.borrow().attrs.get(name) {
                return Some(v.clone());
            }
        }
        return None;
    }
    if let Some(base) = &borrowed.base
        && let Some(v) = lookup_class_attr(base, name)
    {
        return Some(v);
    }
    // Issue #1378: every class implicitly has `object` as its ultimate ancestor
    // (CPython's invariant).  When the MRO chain terminates (no explicit primary
    // base) and the class is not itself `object`, fall through to the object
    // singleton's attrs.  This mirrors class_is_subclass_of (which returns true
    // for any class when expected is object) and class_mro_items (which appends
    // object at the end of every MRO).
    //
    // Without this fallback, built-in exception classes whose base chain ends at
    // BaseException (base==None) never reached object's attrs — so
    // `hasattr(Exception, '__init_subclass__')` was False.
    //
    // Issue #1537: primitive class singletons (int, str, list, …) now set their
    // `base` to the `object` singleton explicitly, so `has_explicit_base` is
    // true for them and this fallback branch is skipped for them.  The
    // `is_primitive_class` guard is retained as a safety net for any class that
    // might lack an explicit base but still should not fall through to object.
    if !has_explicit_base && !is_primitive_class(class) {
        let obj = object_class_singleton();
        if !Rc::ptr_eq(class, &obj) {
            return lookup_class_attr(&obj, name);
        }
    }
    None
}

/// Return the class whose own attribute dictionary supplies the first MRO
/// definition of `name`.
///
/// This is the provenance counterpart of [`lookup_class_attr`].  Consumers
/// that must distinguish an inherited slot from an explicit assignment of the
/// same descriptor value must inspect the defining class, not parse a
/// `BuiltinFunction`'s presentation/dispatch name.
///
/// Linear inheritance stays allocation-free.  Multiple inheritance follows
/// the same canonical C3 order as [`lookup_class_attr`], including the
/// implicit `object` fallback for classes without an explicit base.
pub(crate) fn lookup_class_attr_owner(
    class: &Rc<RefCell<PyClass>>,
    name: &str,
) -> Option<Rc<RefCell<PyClass>>> {
    let borrowed = class.borrow();
    if borrowed.attrs.contains_key(name) {
        return Some(Rc::clone(class));
    }
    let has_explicit_base = borrowed.base.is_some();
    if !borrowed.extra_bases.is_empty() {
        drop(borrowed);
        return c3_linearize_classes(class)
            .into_iter()
            .find(|owner| owner.borrow().attrs.contains_key(name));
    }
    if let Some(base) = &borrowed.base
        && let Some(owner) = lookup_class_attr_owner(base, name)
    {
        return Some(owner);
    }
    if !has_explicit_base && !is_primitive_class(class) {
        let object = object_class_singleton();
        if !Rc::ptr_eq(class, &object) {
            drop(borrowed);
            return lookup_class_attr_owner(&object, name);
        }
    }
    None
}

/// C3 linearization of `class`, returning the MRO as a `Vec` of class pointers
/// (the same order as `__mro__` / `class_mro_items`), with the `object`
/// singleton appended last.  Unlike `class_mro_items`, this returns class
/// pointers (no `Value` wrapping) and is infallible: it is only ever called on
/// classes that were successfully created (so a consistent linearization was
/// already verified at class-creation time).  Used by `lookup_class_attr` to
/// scan multiple-inheritance bases in C3 order (issue #2075).
///
/// Runs the identical C3 algorithm as `class_mro_items` (which `__mro__` and
/// `mro()` use), so the two always agree on order.  They are kept separate
/// because `class_mro_items` is fallible — it reports a `TypeError` for an
/// inconsistent linearization at class-creation time — whereas this variant is
/// only reached after a class already exists and so never needs to fail.
pub(crate) fn c3_linearize_classes(class: &Rc<RefCell<PyClass>>) -> Vec<Rc<RefCell<PyClass>>> {
    fn linearize(c: &Rc<RefCell<PyClass>>) -> Vec<Rc<RefCell<PyClass>>> {
        let (base, extra_bases) = {
            let b = c.borrow();
            (b.base.clone(), b.extra_bases.clone())
        };
        let mut all_bases: Vec<Rc<RefCell<PyClass>>> = Vec::new();
        if let Some(b) = base {
            all_bases.push(b);
        }
        all_bases.extend(extra_bases);
        if all_bases.is_empty() {
            return vec![Rc::clone(c)];
        }
        let mut lists: Vec<Vec<Rc<RefCell<PyClass>>>> = all_bases.iter().map(linearize).collect();
        lists.push(all_bases);

        let mut result = vec![Rc::clone(c)];
        loop {
            lists.retain(|l| !l.is_empty());
            if lists.is_empty() {
                break;
            }
            // `object` is deferred so it never wins ahead of a non-object head;
            // see the matching `class_mro_items` (env.rs) comment for why (#2611).
            let obj_ptr = Rc::as_ptr(&object_class_singleton());
            let mut chosen: Option<Rc<RefCell<PyClass>>> = None;
            let mut deferred_object: Option<Rc<RefCell<PyClass>>> = None;
            'outer: for list in &lists {
                let head_ptr = Rc::as_ptr(&list[0]);
                for other in &lists {
                    for tail in other.iter().skip(1) {
                        if Rc::as_ptr(tail) == head_ptr {
                            continue 'outer;
                        }
                    }
                }
                if head_ptr == obj_ptr {
                    deferred_object = Some(Rc::clone(&list[0]));
                    continue;
                }
                chosen = Some(Rc::clone(&list[0]));
                break;
            }
            // No consistent head: fall back to a deferred `object`, else to the
            // first remaining head so the scan still terminates.  The latter
            // cannot happen for a validly-created class (the MRO was checked at
            // creation), but we never panic.
            let chosen = chosen
                .or(deferred_object)
                .unwrap_or_else(|| Rc::clone(&lists[0][0]));
            let chosen_ptr = Rc::as_ptr(&chosen);
            result.push(chosen);
            for list in &mut lists {
                if !list.is_empty() && Rc::as_ptr(&list[0]) == chosen_ptr {
                    list.remove(0);
                }
            }
        }
        result
    }

    let mut mro = linearize(class);
    let obj = object_class_singleton();
    if !mro.iter().any(|c| Rc::ptr_eq(c, &obj)) {
        mro.push(obj);
    }
    mro
}

thread_local! {
    static OBJECT_CLASS: Rc<RefCell<PyClass>> = {
        // Issue #1047: object.__init_subclass__ is a no-op classmethod in
        // CPython.  Register the builtin sentinel so that
        // `super().__init_subclass__(**kwargs)` inside user __init_subclass__
        // methods finds it when the MRO walk reaches `object`.
        //
        // Issue #1256: also register the common object dunders so that
        // `hasattr(object, '__str__')` returns True and `super().__str__()`
        // in user classes resolves via MRO to the registered handler.
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        for dunder in &[
            "__init_subclass__",
            "__subclasshook__",
            "__getattribute__",
            "__setattr__",
            "__delattr__",
            "__str__",
            "__repr__",
            "__eq__",
            "__ne__",
            "__hash__",
            "__init__",
            "__new__",
            "__lt__",
            "__le__",
            "__gt__",
            "__ge__",
            "__format__",
            // Issue #2151: object-protocol methods every object inherits.
            // `obj.__sizeof__()` / `obj.__dir__()` / `obj.__reduce__()` /
            // `obj.__reduce_ex__(p)` resolve here for all values whose class
            // chains to `object`.
            "__sizeof__",
            "__dir__",
            "__reduce__",
            "__reduce_ex__",
        ] {
            if matches!(*dunder, "__init_subclass__" | "__subclasshook__") {
                continue;
            }
            let qualified = registered_builtin_method_name("object", dunder);
            let function = Value::builtin_function(qualified);
            attrs.insert((*dunder).to_string(), function);
        }
        let mut class = PyClass::new("object", "object", None, attrs);
        class.canonical_tag = Some(pyrust_core::CanonicalClassTag::Object);
        let class = Rc::new(RefCell::new(class));
        for (name, qualified) in [
            ("__init_subclass__", "object.__init_subclass__"),
            ("__subclasshook__", "object.__subclasshook__"),
        ] {
            let descriptor = pyrust_builtins::classmethod::native_class_method_descriptor(
                Value::builtin_function(qualified),
                &class,
                name,
            );
            class
                .borrow_mut()
                .attrs
                .insert(name.to_string(), descriptor);
        }
        class
    };

    /// Per-primitive `PyClass` singletons.  Issue #462 — `int`, `str`,
    /// `list`, … are now real `PyClass` values, not `BuiltinFunction(name)`
    /// sentinels.  `type(x)` returns the matching entry here; the names
    /// resolve to these classes via `resolve_builtin`; and `isinstance`
    /// works through the standard `class_is_subclass_of` walk.
    ///
    /// Each class's `__init__` is the existing `BuiltinFunction("<name>")`
    /// constructor (so `int("42")` etc. keep their established behaviour);
    /// the call-site dispatch in `call_class_expanded` recognises primitive
    /// classes and returns the constructor's `Value` directly instead of
    /// wrapping it in a `PyInstance`.
    ///
    /// `bool` chains its `base` to `int`, matching CPython's
    /// `bool.__bases__ == (int,)`.  Storage-variant constraints prevent
    /// subclassing primitives in pyrust today; the migration is purely
    /// metadata + dispatch routing.
    static PRIMITIVE_CLASSES: PrimitiveClasses = build_primitive_classes();

    /// Per-thread metaclass singleton for `type`.  In CPython, `type` is
    /// both a callable and a class — `type(int)` returns `<class 'type'>`,
    /// and `isinstance(int, type)` is True.  Mirrors the `OBJECT_CLASS`
    /// pattern (issue #1312).
    ///
    /// Issue #1537: `type.__bases__ == (object,)` in CPython.  Setting the
    /// explicit base lets `lookup_class_attr` walk to `object` so that
    /// `hasattr(type, '__init_subclass__')` returns True.
    static TYPE_CLASS: Rc<RefCell<PyClass>> = {
        let obj = OBJECT_CLASS.with(Rc::clone);
        // Issue #1385: register type.__new__ and type.__init__ so that
        // `super().__new__(mcs, name, bases, namespace)` and
        // `super().__init__(name, bases, namespace)` inside custom metaclass
        // methods resolve to these builtins instead of falling through to
        // object.__new__ / object.__init__ which reject the extra arguments.
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        attrs.insert(
            "__new__".to_string(),
            Value::builtin_function("type.__new__"),
        );
        attrs.insert(
            "__init__".to_string(),
            Value::builtin_function("type.__init__"),
        );
        // Issue #1956: register `type.__call__` so that `super().__call__(*a)`
        // inside a metaclass `__call__` override resolves (via the metaclass
        // MRO super-walk) to the default construct.
        attrs.insert(
            "__call__".to_string(),
            Value::builtin_function("type.__call__"),
        );
        // Issue #2128: register the default `type.__prepare__` so
        // `hasattr(type, '__prepare__')` is true, `type.__prepare__(name, bases)`
        // returns a fresh dict, and `super().__prepare__(...)` resolves inside a
        // custom metaclass.  It is a classmethod (receives the metaclass).
        attrs.insert(
            "__prepare__".to_string(),
            Value::builtin_function("type.__prepare__"),
        );
        // PEP 585: `type` is subscriptable (`type[int]` → `types.GenericAlias`)
        // in CPython 3.9+.  Unlike `list`/`dict`/…, CPython does NOT expose a
        // `__class_getitem__` attribute on `type` (`hasattr(type,
        // '__class_getitem__')` is False and `type.__class_getitem__(int)`
        // raises AttributeError), so no sentinel is registered here.  The
        // `type[int]` subscript is handled directly in `eval_index` by
        // pointer-identity matching the `type` singleton.
        let cls = Rc::new(RefCell::new(PyClass::new(
            "type",
            "type",
            Some(Rc::clone(&obj)),
            attrs,
        )));
        let prepare = pyrust_builtins::classmethod::native_class_method_descriptor(
            Value::builtin_function("type.__prepare__"),
            &cls,
            "__prepare__",
        );
        cls.borrow_mut()
            .attrs
            .insert("__prepare__".to_string(), prepare);
        obj.borrow().subclasses.borrow_mut().push(Rc::downgrade(&cls));
        cls
    };

    /// Per-thread `PyClass` singleton for the `method` type.  In CPython,
    /// `type(instance.method)` returns `<class 'method'>` — a proper class
    /// whose metatype is `type`, so `type(type(c.m)) is type` holds.
    /// Issue #1528: previously `type(c.m)` returned a `BuiltinFunction("method")`
    /// sentinel, so `type(type(c.m))` resolved to `builtin_function_or_method`.
    static METHOD_TYPE: Rc<RefCell<PyClass>> = Rc::new(RefCell::new(PyClass::new(
        "method",
        "method",
        None,
        IndexMap::new(),
    )));

    /// Per-thread `PyClass` singleton for the `function` type.  In CPython,
    /// `type(lambda: None)` returns `<class 'function'>` — a proper class
    /// whose metatype is `type`, so `type(type(lambda: None)) is type` holds.
    /// Issue #1528: previously `type(f)` for a user-defined function returned
    /// a `BuiltinFunction("function")` sentinel.
    static FUNCTION_TYPE: Rc<RefCell<PyClass>> = Rc::new(RefCell::new(PyClass::new(
        "function",
        "function",
        None,
        IndexMap::new(),
    )));

    /// Per-thread `PyClass` singleton for the `range` type.  In CPython,
    /// `range` is a proper class (`type(range(5)) is range`), not a builtin
    /// function.  This singleton lets `type(range(5))` return a real `PyClass`
    /// and enables `issubclass(range, Sequence)` via `extra_bases` registration
    /// in `register_abc_extra_bases`.  Issues #1793, #1800.
    static RANGE_CLASS: Rc<RefCell<PyClass>> = {
        let obj = OBJECT_CLASS.with(Rc::clone);
        // Issue #2399: expose `range`'s slot dunders as type-level attributes,
        // mirroring the SLOT_ATTR registration `build_primitive_classes` does for
        // the other primitives.  `range` is built in its own thread-local (not the
        // primitive-class loop), so the registration is inlined here.  Each name
        // becomes a `BuiltinFunction("range.<dunder>")` sentinel that resolves via
        // `get_attr_class`'s MRO lookup and dispatches through
        // `dispatch_builtin_protocol_dunder` (the `<type>.<method>` call arm).
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        for (dunder, flags) in builtin_methods::slot_dunder_table("range") {
            if flags & builtin_methods::SLOT_ATTR == 0 {
                continue;
            }
            let qualified = registered_builtin_method_name("range", dunder);
            attrs.insert(dunder.to_string(), Value::builtin_function(qualified));
        }
        let cls = Rc::new(RefCell::new(PyClass::new(
            "range",
            "range",
            Some(Rc::clone(&obj)),
            attrs,
        )));
        obj.borrow().subclasses.borrow_mut().push(Rc::downgrade(&cls));
        cls
    };

    /// Per-thread `PyClass` singletons for the remaining `builtins` types that
    /// pyrust used to model as `BuiltinFunction(name)` class-tokens: `zip`,
    /// `map`, `filter`, `enumerate`, `slice`, `reversed` (issue #3000).  Same
    /// shape as [`RANGE_CLASS`]: a real class based on `object`, so
    /// `type(zip(...))` reprs as `<class 'zip'>`, `issubclass(type(z), object)`
    /// is True and `type(z).__mro__` resolves.  Indexed by
    /// [`BuiltinTypeClass`]; the constructor for each is wired into
    /// `PRIMITIVE_CLASS_DISPATCH` below so `zip(a, b)` still reaches the
    /// existing registry entry instead of allocating a `PyInstance`.
    static BUILTIN_TYPE_CLASSES: [Rc<RefCell<PyClass>>; BuiltinTypeClass::ALL.len()] = {
        let obj = OBJECT_CLASS.with(Rc::clone);
        BuiltinTypeClass::ALL.map(|kind| {
            let name = kind.class_name();
            let mut class = PyClass::new(name, name, Some(Rc::clone(&obj)), IndexMap::new());
            class.non_subclassable_name = kind.non_subclassable_name();
            let cls = Rc::new(RefCell::new(class));
            obj.borrow().subclasses.borrow_mut().push(Rc::downgrade(&cls));
            cls
        })
    };

    /// Per-thread `PyClass` singleton for the `types.GenericAlias` type — the
    /// type of `list[int]`, `dict[str, int]`, etc. (PEP 585).  In CPython 3.12
    /// `type(list[int])` is `<class 'types.GenericAlias'>`, with
    /// `__name__ == "GenericAlias"`, `__qualname__ == "GenericAlias"`, and
    /// `__module__ == "types"`.  Issue #2733: previously `type(list[int])`
    /// returned a `BuiltinFunction("types.GenericAlias")` sentinel, so the repr
    /// read `<built-in function types.GenericAlias>` and `__module__` raised
    /// `AttributeError`.  `__module__` is stored as a dict attribute so the
    /// PyClass repr in `pyrust-core` renders the `types.` qualifier.
    static GENERIC_ALIAS_CLASS: Rc<RefCell<PyClass>> = {
        let obj = OBJECT_CLASS.with(Rc::clone);
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        attrs.insert("__module__".to_string(), Value::string("types"));
        // Issue #2733: `type(list[int]).__doc__` is the GenericAlias docstring
        // in CPython 3.12 (not the origin's docstring and not AttributeError).
        // The singleton is not in `env.rs::is_builtin_class`, so without an
        // explicit `__doc__` attr the class attribute lookup misses and raises
        // AttributeError; store it on the class to match CPython.
        attrs.insert(
            "__doc__".to_string(),
            Value::string(
                "Represent a PEP 585 generic type\n\nE.g. for t = list[int], \
                 t.__origin__ is list and t.__args__ is (int,).",
            ),
        );
        let mut class = PyClass::new(
            "GenericAlias",
            "GenericAlias",
            Some(Rc::clone(&obj)),
            attrs,
        );
        class.canonical_tag = Some(pyrust_core::CanonicalClassTag::GenericAlias);
        let cls = Rc::new(RefCell::new(class));
        obj.borrow().subclasses.borrow_mut().push(Rc::downgrade(&cls));
        cls
    };

    /// O(1) dispatch table for primitive classes (#462 perf): maps the
    /// `Rc<RefCell<PyClass>>` identity (by raw pointer) to the registry's
    /// `BuiltinDispatchFn` for the corresponding constructor.  Populated
    /// once per thread alongside `PRIMITIVE_CLASSES`.
    ///
    /// Hot path: `call_function_expanded`'s `ValueKind::PyClass(class)`
    /// arm looks up `Rc::as_ptr(class)` here; on hit it dispatches
    /// directly to the registry fn, skipping `call_class_expanded`'s
    /// PyInstance allocation + `lookup_class_attr("__init__")` walk +
    /// recursive `call_function_expanded` step.  The lookup is one
    /// `HashMap::get` + a fn pointer call.
    ///
    /// Hashed with `pyrust_core::PyHasher` rather than the stdlib default: the key is a
    /// raw pointer to an interpreter-owned singleton, so SipHash's
    /// DoS-resistance buys nothing while costing more than the probe it
    /// guards.  Under the default hasher this lookup measured ~12 ns, which
    /// showed up as a 1.1-1.3x regression on constructor-heavy loops the
    /// moment #3000 moved `zip` / `map` / `filter` / `enumerate` / `slice` /
    /// `reversed` onto this table.
    static PRIMITIVE_CLASS_DISPATCH:
        std::cell::RefCell<
            std::collections::HashMap<
                *const std::cell::RefCell<PyClass>,
                crate::builtin_registry::BuiltinDispatchFn,
                pyrust_core::PyHasher,
            >,
        > = {
        let cell = std::cell::RefCell::new(std::collections::HashMap::with_capacity_and_hasher(
            24,
            pyrust_core::PyHasher::default(),
        ));
        PRIMITIVE_CLASSES.with(|c| {
            let mut m = cell.borrow_mut();
            for (class, name) in [
                (&c.bool_class, "bool"),
                (&c.bytearray_class, "bytearray"),
                (&c.bytes_class, "bytes"),
                (&c.complex_class, "complex"),
                (&c.dict_class, "dict"),
                (&c.float_class, "float"),
                (&c.frozenset_class, "frozenset"),
                (&c.int_class, "int"),
                (&c.list_class, "list"),
                (&c.set_class, "set"),
                (&c.str_class, "str"),
                (&c.tuple_class, "tuple"),
            ] {
                if let Some(dispatch) = crate::builtin_registry::lookup(name) {
                    m.insert(Rc::as_ptr(class), dispatch);
                }
            }
            // Issue #1451: NoneType, NotImplementedType, and ellipsis were
            // added to PrimitiveClasses by #1403 but not registered here,
            // causing calls like `type(None)()` to fall through to
            // `call_class_expanded` which allocated a bogus PyInstance.
            // CPython 3.12: zero-arg call returns the singleton; any
            // arguments raise TypeError "<TypeName> takes no arguments".
            m.insert(
                Rc::as_ptr(&c.none_class),
                none_ctor as crate::builtin_registry::BuiltinDispatchFn,
            );
            m.insert(
                Rc::as_ptr(&c.notimplemented_class),
                notimplemented_ctor as crate::builtin_registry::BuiltinDispatchFn,
            );
            m.insert(
                Rc::as_ptr(&c.ellipsis_class),
                ellipsis_ctor as crate::builtin_registry::BuiltinDispatchFn,
            );
        });
        // `type` metaclass: every `PyClass` value is an instance of `type`
        // in CPython.  Register the TYPE_CLASS singleton here so that
        // calling `type(x)` dispatches to the existing "type" registry entry
        // without going through `call_class_expanded` (issue #1312).
        TYPE_CLASS.with(|t| {
            if let Some(dispatch) = crate::builtin_registry::lookup("type") {
                cell.borrow_mut().insert(Rc::as_ptr(t), dispatch);
            }
        });
        // Issues #1793, #1800: `range` is a proper class in CPython, so
        // `range(1, 10)` is a constructor call on the range class.  Register
        // it so `call_function_expanded`'s PyClass arm dispatches to the
        // existing "range" registry fn instead of falling through to
        // `call_class_expanded` which would allocate a bogus PyInstance.
        RANGE_CLASS.with(|r| {
            if let Some(dispatch) = crate::builtin_registry::lookup("range") {
                cell.borrow_mut().insert(Rc::as_ptr(r), dispatch);
            }
        });
        // Issue #3000: `zip` / `map` / `filter` / `enumerate` / `slice` /
        // `reversed` are classes too, so `zip(a, b)` is a constructor call.
        // Register each singleton against its existing registry entry, exactly
        // as `range` above, so the call never reaches `call_class_expanded`.
        BUILTIN_TYPE_CLASSES.with(|classes| {
            for (kind, class) in BuiltinTypeClass::ALL.iter().zip(classes) {
                if let Some(dispatch) = crate::builtin_registry::lookup(kind.class_name()) {
                    cell.borrow_mut().insert(Rc::as_ptr(class), dispatch);
                }
            }
        });
        cell
    };
}

thread_local! {
    /// The newest imported `itertools.chain` class generation.
    ///
    /// Iterator states snapshot an Rc at construction. Re-importing
    /// `itertools` therefore updates the factory generation without changing
    /// the type of any already-created iterator. The registry itself is weak:
    /// it must not keep an otherwise-unreachable module generation alive.
    static ITERTOOLS_CHAIN_CLASS: RefCell<Option<std::rc::Weak<RefCell<PyClass>>>> =
        const { RefCell::new(None) };
}

/// Record the newest imported `itertools.chain` class generation.
pub(crate) fn set_itertools_chain_class(class: Rc<RefCell<PyClass>>) {
    ITERTOOLS_CHAIN_CLASS.with(|c| {
        *c.borrow_mut() = Some(Rc::downgrade(&class));
    });
}

/// The newest `itertools.chain` class, if the module has been imported.
pub(crate) fn itertools_chain_class() -> Option<Rc<RefCell<PyClass>>> {
    ITERTOOLS_CHAIN_CLASS.with(|c| c.borrow().as_ref().and_then(std::rc::Weak::upgrade))
}
