// Class-object attribute semantics.
impl Interpreter {
    /// `Cls.name` attribute access (the `ValueKind::PyClass` arm of
    /// `get_attr`).  Handles `__name__`/`__qualname__`/`__mro__`/`__bases__`,
    /// MRO attribute lookup, classmethod/staticmethod binding, metaclass
    /// descriptors, and the AttributeError fallback.
    fn get_attr_class(&mut self, class: Rc<RefCell<PyClass>>, name: &str) -> Result<Value> {
        if name == "__name__" {
            return Ok(Value::string(class.borrow().name.clone()));
        }
        if name == "__qualname__" {
            // __qualname__ is a type-level descriptor on `type` in CPython,
            // not stored in the class attrs dict.  Intercept here so that
            // C.__qualname__ works without polluting vars(C) (issue #553).
            return Ok(Value::string(class.borrow().qualname.clone()));
        }
        if name == "__dict__" {
            // Return a live mappingproxy wrapping the class's attrs dict —
            // matching CPython 3.12's `type.__dict__` descriptor, which
            // returns `types.MappingProxyType`.  Reads see the current
            // attrs (live reference); mutation raises TypeError (issue #726).
            return Ok(pyrust_builtins::mapping_proxy::mapping_proxy(Rc::clone(
                &class,
            )));
        }
        if name == "__bases__" {
            // `__bases__` reports all immediate parents in declaration
            // order.  If no explicit base was given, CPython reports
            // `(object,)` — except for `object` itself, which has no bases
            // and reports `()` (issue #1969).
            if Rc::ptr_eq(&class, &object_class_singleton()) {
                return Ok(Value::tuple(Vec::new()));
            }
            let (base, extra_bases) = {
                let borrowed = class.borrow();
                (borrowed.base.clone(), borrowed.extra_bases.clone())
            };
            let mut items: Vec<Value> = Vec::new();
            match base {
                None => items.push(Value::py_class(object_class_singleton())),
                Some(b) => {
                    items.push(Value::py_class(b));
                    for eb in extra_bases {
                        items.push(Value::py_class(eb));
                    }
                }
            }
            return Ok(Value::tuple(items));
        }
        if name == "__base__" {
            // `__base__` is the single primary base used for instance layout.
            // For typical single/multiple inheritance this is `bases[0]`.
            // `object` itself has no base and reports `None` (issue #1969).
            if Rc::ptr_eq(&class, &object_class_singleton()) {
                return Ok(Value::none());
            }
            let base = class.borrow().base.clone();
            return Ok(match base {
                Some(b) => Value::py_class(b),
                None => Value::py_class(object_class_singleton()),
            });
        }
        if name == "__mro__" {
            return Ok(Value::tuple(class_mro_items(&class)?));
        }
        if name == "mro" {
            return Ok(pyrust_builtins::bound_method::bound_method(
                "mro",
                Value::py_class(Rc::clone(&class)),
            ));
        }
        if name == "__subclasses__" {
            return Ok(pyrust_builtins::bound_method::bound_method(
                "__subclasses__",
                Value::py_class(Rc::clone(&class)),
            ));
        }
        if name == "__annotations__" {
            // `type.__annotations__` in CPython is a data descriptor on
            // `type` itself.  On first access it synthesises an empty dict,
            // writes it back into the class's own `__dict__`, and returns
            // that same dict.  Subsequent accesses hit the own-attrs check
            // and return the stored (potentially mutated) dict — so
            // `Foo.__annotations__ is Foo.__annotations__` is `True` and
            // mutations via subscript-assignment persist (issue #737).
            //
            // CPython does NOT inherit __annotations__ from base classes:
            // `B.__annotations__` is always B's own dict, never A's.
            // Use a direct own-attrs lookup (not lookup_class_attr) here.
            if let Some(stored) = class.borrow().attrs.get("__annotations__").cloned() {
                return Ok(stored);
            }
            let empty = Value::dict(PyDict::default());
            class
                .borrow_mut()
                .attrs
                .insert("__annotations__".to_string(), empty.clone());
            return Ok(empty);
        }
        if let Some(bound) = bind_builtin_class_special(&class, name) {
            return Ok(bound);
        }
        // `__module__` and `__doc__` on built-in type objects are virtual
        // attributes supplied by `type`, not entries in the class dictionary.
        // Resolve them before walking the class MRO: collections.abc registers
        // ABCs as `extra_bases` of primitive classes, and an inherited
        // `Hashable.__module__ == "collections.abc"` must never replace
        // `int.__module__ == "builtins"`.
        //
        // Keeping this virtual also preserves CPython's `int.__dict__`
        // surface (`"__module__" not in int.__dict__`).
        if matches!(name, "__module__" | "__doc__") {
            // A native type outside `builtins` may carry explicit metadata in
            // its own class dictionary. `types.GenericAlias` is the current
            // example. Prefer only the concrete class's own value here (not an
            // MRO lookup), so an ABC registered as an extra base still cannot
            // leak its `__module__` into a primitive class.
            if let Some(value) = class.borrow().attrs.get(name).cloned() {
                return Ok(value);
            }
            // These tags live only on the concrete interpreter-owned type.
            // `is_exception_class` deliberately walks the base chain and would
            // therefore misclassify every Python-defined Exception subclass
            // (including copy.Error and dataclasses.FrozenInstanceError) as a
            // built-in type, hiding its own `__module__` / `__doc__`.
            let has_native_identity = {
                let borrowed = class.borrow();
                borrowed.canonical_tag.is_some() || borrowed.builtin_exception_name.is_some()
            };
            let has_builtin_type_metadata = has_native_identity
                || Rc::ptr_eq(&class, &crate::interpreter::method_type_singleton())
                || Rc::ptr_eq(&class, &crate::interpreter::function_type_singleton())
                || Rc::ptr_eq(&class, &crate::interpreter::type_class_singleton())
                || Rc::ptr_eq(&class, &crate::interpreter::range_class_singleton());
            if has_builtin_type_metadata {
                if name == "__module__" {
                    return Ok(Value::string("builtins"));
                }
                let class_name = class.borrow().name.clone();
                return Ok(match builtin_class_doc(&class_name) {
                    Some(doc) => Value::string(doc),
                    None => Value::none(),
                });
            }
        }
        // Issue #2081: a *data* descriptor on the metaclass shadows a same-named
        // attribute in the class's own dict.  CPython's `type.__getattribute__`
        // resolves `meta_data_descriptor` BEFORE the class MRO; only non-data
        // metaclass attributes (handled further below, #1956/#2078) are shadowed
        // by class-own entries.  Check the metaclass MRO first and, when the
        // attribute found there is a data descriptor (e.g. a `property`), invoke
        // its `__get__(cls, type(cls))`.
        if let Some(meta_val) = metaclass_dunder(&class, name)
            && is_data_descriptor(&meta_val)
        {
            return call_descriptor_get(
                self,
                &meta_val,
                Value::py_class(Rc::clone(&class)),
                Value::py_class(metaclass_of(&class)),
                name,
            );
        }
        // Issue #2291: the unhashable built-in types set `__hash__ = None` on
        // the *type* (CPython: `list.__hash__ is None`).  Accessing
        // `list.__hash__` / `dict.__hash__` / `set.__hash__` /
        // `bytearray.__hash__` therefore yields `None`, and calling it raises
        // `'NoneType' object is not callable`.  Mirror the instance path
        // (`[1].__hash__` → `None`) here for the class attribute, but only when
        // no closer user override exists (an MRO lookup that lands on the
        // inherited `object.__hash__` sentinel) so a subclass that defines its
        // own `__hash__` still resolves to that function.  Issue #2299:
        // `class_hash_inherits_builtin_none` walks the MRO so that a subclass
        // which re-enables hashing (`__hash__ = object.__hash__`) shadows the
        // builtin's implicit `None` and keeps a callable `__hash__`.
        if name == "__hash__"
            && lookup_class_attr(&class, name)
                .as_ref()
                .is_some_and(|value| {
                    crate::interpreter::value_is_canonical_slot(
                        value,
                        crate::interpreter::CanonicalSlot::ObjectHash,
                    )
                })
            && class_hash_inherits_builtin_none(&class)
        {
            return Ok(Value::none());
        }
        // Issue #2276: object-level dunders that a primitive type *overrides* in
        // CPython are attributed to the called type, not `object`
        // (`str.__hash__()` → "... of 'str' object ...", not 'object').  When the
        // MRO lookup below would resolve such a dunder to the inherited
        // `object.__X__` sentinel — i.e. there is no closer user override — we
        // instead return a type-qualified `BuiltinFunction("<type>.__X__")`
        // sentinel.  It is synthesised only for direct `T.__X__` attribute
        // access and is deliberately NOT stored in the class `attrs` (which
        // would shadow the `object.__X__` sentinel that the repr/hash rendering
        // paths key on via `lookup_class_attr`).  The matching dispatch arm in
        // `call_function_expanded` validates the receiver (naming the called
        // type) and delegates to the shared `object.__X__` / formatting
        // machinery.  Ownership verified against python3.12
        // (`[c for c in T.__mro__ if '__X__' in c.__dict__]`).
        // Issue #2433: the six rich-comparison slots join `__hash__`/`__repr__`/
        // `__str__`/`__format__` here — every primitive owns its comparisons in
        // CPython, so `list.__eq__` must read `'list'`, not the inherited
        // `object.__eq__`.
        if matches!(
            name,
            "__hash__"
                | "__repr__"
                | "__str__"
                | "__format__"
                | "__eq__"
                | "__ne__"
                | "__lt__"
                | "__le__"
                | "__gt__"
                | "__ge__"
        ) && let Some(qualified) = primitive_owned_object_dunder(&class, name)
            && let Some(slot) = crate::interpreter::CanonicalSlot::object_named(name)
            && lookup_class_attr(&class, name)
                .as_ref()
                .is_some_and(|value| crate::interpreter::value_is_canonical_slot(value, slot))
        {
            return Ok(Value::builtin_function(qualified));
        }
        if let Some(value) = lookup_class_attr(&class, name) {
            if let Some(exported) =
                pyrust_builtins::member_descriptor::export_member_descriptor(&value)
            {
                return Ok(exported);
            }
            // Descriptor protocol for class-level access: if the class
            // attribute is a user-defined descriptor (PyInstance with
            // __get__), call __get__(None, cls) — CPython Data Model
            // §3.3.2.  property is handled by its own match arm (above
            // this ValueKind::PyClass arm) and returns itself on class
            // access, so we only check PyInstance here.
            if let ValueKind::PyInstance(desc_inst) = value.kind() {
                let desc_class = Rc::clone(&desc_inst.borrow().class);
                if lookup_class_attr(&desc_class, "__get__").is_some() {
                    return call_descriptor_get(
                        self,
                        &value,
                        Value::none(),
                        Value::py_class(Rc::clone(&class)),
                        name,
                    );
                }
            }
            // Drop the kind() Ref before the `_ => value` arm
            // may move `value` (#450).
            enum ClassDescTag {
                UserFunction(Rc<UserFunction>),
                ClassMethodAny(pyrust_builtins::classmethod::ClassMethodBindingSpec),
                StaticMethodAny(Value),
                Other,
            }
            let tag = match value.kind() {
                ValueKind::UserFunction(f) => ClassDescTag::UserFunction(Rc::clone(f)),
                _ => {
                    if let Some(w) = pyrust_builtins::classmethod::as_class_method_any(&value) {
                        ClassDescTag::ClassMethodAny(w)
                    } else if let Some(w) =
                        pyrust_builtins::classmethod::as_static_method_any(&value)
                    {
                        ClassDescTag::StaticMethodAny(w)
                    } else {
                        ClassDescTag::Other
                    }
                }
            };
            return Ok(match tag {
                ClassDescTag::UserFunction(f) => match f.kind {
                    UserFunctionKind::ClassMethod => {
                        Value::class_bound_method(Rc::clone(&f), Rc::clone(&class))
                    }
                    UserFunctionKind::StaticMethod => {
                        // CPython __get__ returns the underlying function directly.
                        // Prefer `wrapped_func` to preserve object identity
                        // (`sm.__get__(None, C) is fn` when `sm = staticmethod(fn)`).
                        if let Some(inner) = f.wrapped_func.as_ref() {
                            Value::user_function(Rc::clone(inner))
                        } else {
                            Value::with_function_kind(Rc::clone(&f), UserFunctionKind::Regular)
                        }
                    }
                    // Issue #2479: a non-dunder method *defined on* a builtin
                    // primitive subclass (e.g. `OrderedDict.move_to_end`, a
                    // Python `def` on `OrderedDict`) is a C `method_descriptor`
                    // in CPython, so calling it with an unrelated receiver
                    // (`OrderedDict.move_to_end({1: 2}, 1)`) raises a receiver-
                    // class TypeError.  Wrap it so the call site can enforce.
                    UserFunctionKind::Regular => adapt_builtin_subclass_method(&class, name, value),
                    UserFunctionKind::Builtin(_) => value,
                },
                // The descriptor owns the binding rule. Builtin registry
                // functions become an implicit-class adapter without teaching
                // generic attribute lookup their concrete Python names.
                ClassDescTag::ClassMethodAny(w) => {
                    pyrust_builtins::classmethod::bind_wrapped_class_method(w, Rc::clone(&class))?
                }
                // staticmethod(non_fn): returns the wrapped value directly,
                // matching CPython (`C.s` where `s = staticmethod(42)` → 42).
                ClassDescTag::StaticMethodAny(w) => w,
                // Issue #2479: an inherited primitive method sentinel
                // (e.g. `OrderedDict.clear` → `BuiltinFunction("dict.clear")`)
                // accessed on a proper subclass must reject an unrelated
                // receiver. Classmethods never reach this arm: their owner
                // installs an explicit descriptor wrapper.
                ClassDescTag::Other => adapt_builtin_subclass_method(&class, name, value),
            });
        }
        // Issue #1956/#1960: on a miss in `cls`'s own MRO, consult the
        // metaclass's MRO for the attribute, mirroring CPython's
        // `type.__getattribute__` (which looks up `name` on `type(cls)` after
        // the class's own dict).  This lets a metaclass method or attribute
        // (e.g. a `_instances` cache used by a singleton `__call__`) be reached
        // via `cls.attr`.  `metaclass_dunder` resolves user-defined attributes
        // on the metaclass MRO, returning `None` for ordinary classes.
        if name != "__getattr__"
            && let Some(meta_val) = metaclass_dunder(&class, name)
        {
            if let ValueKind::UserFunction(f) = meta_val.kind() {
                // A metaclass method accessed via `cls.method` binds `cls`
                // as the receiver (cls is an "instance" of the metaclass),
                // so the dispatch prepends `cls` as the method's first arg.
                return Ok(match f.kind {
                    UserFunctionKind::ClassMethod => {
                        // A classmethod on the metaclass receives the
                        // metaclass itself as `cls`.
                        Value::class_bound_method(Rc::clone(f), metaclass_of(&class))
                    }
                    UserFunctionKind::StaticMethod => {
                        if let Some(inner) = f.wrapped_func.as_ref() {
                            Value::user_function(Rc::clone(inner))
                        } else {
                            Value::with_function_kind(Rc::clone(f), UserFunctionKind::Regular)
                        }
                    }
                    _ => Value::class_bound_method(Rc::clone(f), Rc::clone(&class)),
                });
            }
            return Ok(meta_val);
        }
        // Issue #1960: on an MRO miss, fall back to the metaclass's
        // `__getattr__` (CPython's `type.__getattribute__` ends by invoking
        // `type(cls).__getattr__(cls, name)` if the metaclass defines one).
        // `metaclass_dunder` returns `Some` only for a user override, so
        // ordinary classes keep raising `AttributeError` directly.
        if let Some(getattr_val) = metaclass_dunder(&class, "__getattr__")
            && let ValueKind::UserFunction(f) = getattr_val.kind()
        {
            let func = Rc::clone(f);
            return self.call_user_function_expanded(
                func,
                &[ExpandedCallArg {
                    name: None,
                    value: Value::string(name),
                }],
                &[Value::py_class(Rc::clone(&class))],
            );
        }
        // Issue #2096: every class is callable (to construct instances) via the
        // built-in `type` metaclass slot `type.__call__`.  When a class defines
        // no `__call__` of its own (so the MRO lookup above missed) and its
        // metaclass is the built-in `type` (so the user-metaclass fallback also
        // missed), surface `type.__call__` as a `method-wrapper` bound to the
        // class — exactly as CPython does (`C.__call__ ==
        // <method-wrapper '__call__' of type object at 0x...>`).  This keeps
        // `hasattr(C, '__call__')` consistent with `callable(C)`, and
        // `C.__call__(...)` constructs an instance just like `C(...)`.
        // Restricted to `__call__` so that other `type`-only dunders that pyrust
        // does not model are unaffected, and only when no user metaclass
        // overrides `__call__` (handled by the `metaclass_dunder` path above).
        if name == "__call__" {
            return Ok(pyrust_builtins::type_call_wrapper::type_call_wrapper(
                Value::py_class(Rc::clone(&class)),
            ));
        }
        let class_name = class.borrow().name.clone();
        Err(PyError::attribute_error(
            format!("type object '{}' has no attribute '{}'", class_name, name),
            Some(name.to_string()),
            Some(Value::py_class(Rc::clone(&class))),
        ))
    }

    /// The CPython `slot_tp_getattr_hook` fallback: if `class` defines
    /// `__getattr__`, invoke it as `__getattr__(instance, name)` and return the
    /// result; otherwise return `None` so the caller proceeds (re-raising the
    /// original `AttributeError`, or continuing normal lookup).
    ///
    /// Shared by the three sites in `get_attr_instance_raw` that previously
    /// inlined this identical lookup-and-invoke (two on a descriptor `__get__`
    /// raising `AttributeError`, one as the final no-attribute fallback).
    fn try_invoke_getattr_hook(
        &mut self,
        class: &Rc<RefCell<PyClass>>,
        instance: &Rc<RefCell<PyInstance>>,
        name: &str,
    ) -> Option<Result<Value>> {
        let getattr_val = lookup_class_attr(class, "__getattr__")?;
        Some(invoke_class_method(
            self,
            getattr_val,
            Value::py_instance(Rc::clone(instance)),
            &[ExpandedCallArg {
                name: None,
                value: Value::string(name),
            }],
        ))
    }
}
