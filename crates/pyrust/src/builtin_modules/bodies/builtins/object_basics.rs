use pyrust_derive::pyrust_module;

pyrust_module! {
    /// Issue #1047: `object.__init_subclass__` — the default no-op hook.
    /// CPython (Objects/typeobject.c) registers this on `object` so that
    /// `super().__init_subclass__(**kwargs)` inside a user-defined
    /// `__init_subclass__` terminates the MRO walk without error.
    ///
    /// CPython raises TypeError if any keyword arguments reach this point:
    /// the expectation is that each level of the MRO consumed its own kwargs
    /// before forwarding the rest upward with `super().__init_subclass__(**kwargs)`.
    ///
    /// CPython signature: `object.__init_subclass__(cls, /)`
    #[py_name = "object.__init_subclass__"]
    fn object_init_subclass(args) -> Result<Value> {
        // Raise TypeError if any keyword arguments reach this point.  Each
        // level of the MRO should have consumed its own kwargs before calling
        // super().__init_subclass__(**remaining_kwargs).
        //
        // CPython's error message uses the new class's name (the `cls` arg),
        // not the literal string "object". E.g. for `class B(A, foo=1)` the
        // message is "B.__init_subclass__() takes no keyword arguments".
        //
        // Check keyword args first (CPython raises keyword error even when
        // positional excess is also present).
        if args.iter().any(|a| a.name.is_some()) {
            let cls_name = args
                .first()
                .and_then(|a| match a.value.kind() {
                    ValueKind::PyClass(c) => Some(c.borrow().name.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| "object".to_string());
            return Err(PyError::named(
                "TypeError",
                format!("{cls_name}.__init_subclass__() takes no keyword arguments"),
            ));
        }
        // args[0] is the implicit `cls` prepended by the classmethod dispatch.
        // Any additional positional arguments are excess.  Use the same cls_name
        // lookup as the keyword-error path: CPython uses the subclass name in
        // the positional error too (e.g. "B.__init_subclass__() takes no
        // arguments (1 given)" when called as `B.__init_subclass__(42)`).
        let n_positional = args.iter().filter(|a| a.name.is_none()).count();
        if n_positional > 1 {
            let excess = n_positional - 1;
            let cls_name = args
                .first()
                .and_then(|a| match a.value.kind() {
                    ValueKind::PyClass(c) => Some(c.borrow().name.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| "object".to_string());
            return Err(PyError::named(
                "TypeError",
                format!("{cls_name}.__init_subclass__() takes no arguments ({excess} given)"),
            ));
        }
        Ok(Value::none())
    }

    /// Issue #1738: `object.__subclasshook__` — the default classmethod hook
    /// used by `ABCMeta.__subclasscheck__` to allow custom `issubclass()`
    /// behaviour.  The default implementation on `object` always returns
    /// `NotImplemented`, signalling that the normal MRO-based subclass check
    /// should proceed.
    ///
    /// CPython signature: `object.__subclasshook__(cls, subclass, /)`
    ///
    /// CPython rejects keyword arguments with the message
    /// `__subclasshook__() takes no keyword arguments` (note: no `object.`
    /// prefix, unlike `__init_subclass__`).  Any number of positional args
    /// is accepted — the implementation ignores them all.
    #[py_name = "object.__subclasshook__"]
    fn object_subclasshook(args) -> Result<Value> {
        reject_keyword_args_expanded("__subclasshook__", args)?;
        Ok(Value::not_implemented())
    }

    /// Issue #1256: `object.__str__(self)` — the default __str__ exposed on
    /// the `object` class so that `super().__str__()` and `hasattr(object,
    /// '__str__')` work correctly.
    ///
    /// CPython's `object.__str__` is implemented by calling `tp_repr` on the
    /// object's type (typeobject.c:object_str).  We route through
    /// `render_value_repr` which dispatches `type(self).__repr__(self)` for
    /// user instances, and falls back to `value.repr_raw()` for primitives.
    ///
    /// CPython signature: `object.__str__(self, /)`
    #[py_name = "object.__str__"]
    fn object_str_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__str__", "object")
        })?;
        let s = render_value_repr(_interp, &self_val)?;
        Ok(Value::string(s))
    }

    /// Issue #1256: `object.__repr__(self)` — the default __repr__ on `object`.
    ///
    /// Returns the canonical `<ClassName object at 0xADDR>` format for plain
    /// instances.  For `PyInstance` values that carry a primitive backing store
    /// (e.g. a `MyList(list)` subclass instance), delegates to the backing data
    /// so that `list.__repr__(MyList([1,2,3]))` returns `[1, 2, 3]` rather than
    /// the generic `<__main__.MyList object at 0x...>` form.
    ///
    /// Issue #1600: regression from PR #1595 (primitive types got
    /// `base: Some(OBJECT_CLASS)`), which caused `list.__repr__` to resolve via
    /// MRO to this sentinel and fall through to `self_val.repr_raw()`.
    ///
    /// Note: we call `render_value_repr(interp, &backing)` (on the raw backing
    /// value, NOT on the instance) so that nested `PyInstance` elements inside a
    /// list/dict/tuple get their own `__repr__` dispatched correctly, but we do
    /// NOT re-run the MRO lookup on the outer instance — this matches CPython's
    /// behaviour where `list.__repr__(MyList_with_custom_repr)` still renders the
    /// list contents, not the custom `__repr__`.
    ///
    /// CPython signature: `object.__repr__(self, /)`
    #[py_name = "object.__repr__"]
    fn object_repr_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__repr__", "object")
        })?;
        if native_iterator_class(&self_val).is_some() {
            return Ok(Value::string(render_value_repr(_interp, &self_val)?));
        }
        // Issue #1600: for a PyInstance with backing primitive data, render the
        // backing value directly (bypassing MRO lookup) so that
        // `list.__repr__(MyList([1,2,3]))` returns `[1, 2, 3]`.
        if let ValueKind::PyInstance(inst_rc) = self_val.kind() {
            let inst_rc = Rc::clone(inst_rc);
            if let Some(backing) = instance_builtin_data(&inst_rc) {
                let class = Rc::clone(&inst_rc.borrow().class);
                // Do not dispatch while matching a mutable container's
                // `ValueKind`: List/Dict/Set carry a live RefCell read guard.
                // An element __repr__ is allowed to mutate the same backing
                // container, so classify those containers with short-lived
                // helpers before entering interpreter-aware rendering.
                let s = if backing.is_list() || backing.is_dict() || backing.is_tuple() {
                    render_value_repr(_interp, &backing)?
                } else if let Some(is_empty) = backing.set_len().map(|len| len == 0) {
                    let class_name = class.borrow().name.clone();
                    if is_empty {
                        format!("{class_name}()")
                    } else {
                        let inner = render_value_repr(_interp, &backing)?;
                        format!("{class_name}({inner})")
                    }
                } else {
                    match backing.kind() {
                        ValueKind::Str(_)
                        | ValueKind::Int(_)
                        | ValueKind::BigInt(_)
                        | ValueKind::Bool(_)
                        | ValueKind::Float(_)
                        | ValueKind::Complex(_, _)
                        | ValueKind::Bytes(_) => backing.repr_raw(),
                        ValueKind::BuiltinObject { ops, .. }
                            if ops.canonical_class_tag()
                                == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
                        {
                            let class_name = class.borrow().name.clone();
                            let items = pyrust_builtins::frozenset::as_items(&backing);
                            let is_empty = items.as_ref().is_none_or(|rc| rc.is_empty());
                            if is_empty {
                                format!("{class_name}()")
                            } else {
                                let snapshot: Vec<_> =
                                    items.unwrap().iter().cloned().collect();
                                let mut parts = Vec::with_capacity(snapshot.len());
                                for k in &snapshot {
                                    parts.push(render_key_repr(_interp, k)?);
                                }
                                format!("{class_name}({{{}}})", parts.join(", "))
                            }
                        }
                        _ => self_val.repr_raw(),
                    }
                };
                return Ok(Value::string(s));
            }
        }
        Ok(Value::string(self_val.repr_raw()))
    }

    /// Issue #1256: `object.__eq__(self, other)` — default identity equality.
    ///
    /// Returns `True` if `self is other`, `NotImplemented` otherwise (so the
    /// reflected `other.__eq__(self)` gets a chance).  This matches CPython's
    /// `object_richcompare` for `Py_EQ`.
    ///
    /// CPython signature: `object.__eq__(self, value, /)`
    #[py_name = "object.__eq__"]
    fn object_eq_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__eq__", "object")),
        };
        // Identity comparison: two PyInstance values are equal iff they are
        // the same object (Rc::ptr_eq), matching CPython's default __eq__.
        // For non-instance primitives we fall back to structural equality so
        // that `int.__eq__(1, 1)` still returns True.
        let same = if native_iterator_class(&a).is_some() {
            a.object_id() == b.object_id()
        } else {
            match (a.kind(), b.kind()) {
                (ValueKind::PyInstance(ra), ValueKind::PyInstance(rb)) => {
                    Rc::ptr_eq(ra, rb)
                }
                _ => a == b,
            }
        };
        Ok(if same { Value::bool_(true) } else { Value::not_implemented() })
    }

    /// Issue #1256: `object.__ne__(self, other)` — default identity inequality.
    ///
    /// Returns `False` if `self is other`, `NotImplemented` otherwise.
    ///
    /// CPython signature: `object.__ne__(self, value, /)`
    #[py_name = "object.__ne__"]
    fn object_ne_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__ne__", "object")),
        };
        let same = if native_iterator_class(&a).is_some() {
            a.object_id() == b.object_id()
        } else {
            match (a.kind(), b.kind()) {
                (ValueKind::PyInstance(ra), ValueKind::PyInstance(rb)) => {
                    Rc::ptr_eq(ra, rb)
                }
                _ => a == b,
            }
        };
        Ok(if same { Value::bool_(false) } else { Value::not_implemented() })
    }

    /// Issue #1256: `object.__hash__(self)` — default identity-based hash.
    ///
    /// For user instances, CPython hashes by `id(self) // 16`.  For primitives
    /// routed here via an explicit `object.__hash__(x)` call, delegate to the
    /// standard `hash_value_with_interp` helper which already contains the
    /// correct Mersenne-prime hash for each primitive type.
    ///
    /// CPython signature: `object.__hash__(self, /)`
    #[py_name = "object.__hash__"]
    fn object_hash_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__hash__", "object")
        })?;
        if native_iterator_class(&self_val).is_some() {
            return Ok(self_val.object_id());
        }
        // For user instances use the Rc pointer as the identity hash, matching
        // CPython's default `id(x) >> 4`.  Map -1 → -2 as CPython requires.
        if let ValueKind::PyInstance(inst) = self_val.kind() {
            let ptr = Rc::as_ptr(inst) as i64;
            let h = if ptr == -1 { -2 } else { ptr };
            return Ok(Value::int(h));
        }
        // For primitives, use the shared hash helper.
        let h = hash_value_with_interp(_interp, &self_val)?;
        Ok(Value::int(h))
    }

    /// Issue #2151: `object.__sizeof__(self)` — the size of the object in
    /// bytes.  CPython returns an implementation-specific value; pyrust's
    /// NaN-boxed representation has no comparable layout, so we report the
    /// in-memory `Value` size as a plausible, deterministic-per-build int.
    /// Tests assert only the return type (int), not the exact value.
    #[py_name = "object.__sizeof__"]
    fn object_sizeof_dunder(args) -> Result<Value> {
        if args.is_empty() {
            return Err(pyrust_core::descriptor_needs_arg!("__sizeof__", "object", method));
        }
        Ok(Value::int(std::mem::size_of::<Value>() as i64))
    }

    /// Issue #2151: `object.__dir__(self)` — the default attribute listing,
    /// equivalent to `dir(self)` before `dir()` sorts.  Returns a `list`.
    #[py_name = "object.__dir__"]
    fn object_dir_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__dir__", "object", method)
        })?;
        let mut names = dir_names(&self_val);
        names.sort();
        names.dedup();
        Ok(Value::list(names.into_iter().map(Value::string).collect()))
    }

    #[py_name = "object.__getstate__"]
    fn object_getstate_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|arg| arg.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__getstate__", "object", method)
        })?;
        if args.len() != 1 || args.iter().any(|arg| arg.name.is_some()) {
            return Err(pyrust_core::type_err!(
                "object.__getstate__() takes no arguments ({} given)",
                args.len().saturating_sub(1)
            ));
        }
        match self_val.kind() {
            ValueKind::PyInstance(rc) => Ok(
                crate::builtin_modules::copy::default_instance_state(rc),
            ),
            _ => Ok(Value::none()),
        }
    }

    /// Issue #2151: `object.__reduce_ex__(self, protocol)` — the pickle-protocol
    /// reduction.  CPython returns the `copyreg.__newobj__` tuple; pyrust does
    /// not model copyreg, so we return a tuple of the correct *shape*
    /// (`(class, ())`).  Tests assert only the return type (tuple).
    #[py_name = "object.__reduce_ex__"]
    fn object_reduce_ex_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__reduce_ex__", "object", method)
        })?;
        if is_reversed_iterator(&self_val) || is_getitem_iterator(&self_val) {
            if args.len() != 2 || args.iter().any(|arg| arg.name.is_some()) {
                return Err(pyrust_core::type_err!(
                    "object.__reduce_ex__() takes exactly one argument ({} given)",
                    args.len().saturating_sub(1)
                ));
            }
            let protocol = _interp.value_to_isize(
                &args[1].value,
                "Python int too large to convert to C int",
            )?;
            i32::try_from(protocol).map_err(|_| {
                pyrust_core::overflow_err!("Python int too large to convert to C int")
            })?;
            return if is_reversed_iterator(&self_val) {
                reversed_iterator_reduce(&self_val)
            } else {
                getitem_iterator_reduce(&self_val)
            };
        }
        if let Some(kind) = native_iterator_class(&self_val) {
            if args.len() != 2 || args.iter().any(|arg| arg.name.is_some()) {
                return Err(pyrust_core::type_err!(
                    "object.__reduce_ex__() takes exactly one argument ({} given)",
                    args.len().saturating_sub(1)
                ));
            }
            let protocol = _interp.value_to_isize(
                &args[1].value,
                "Python int too large to convert to C int",
            )?;
            i32::try_from(protocol).map_err(|_| {
                pyrust_core::overflow_err!("Python int too large to convert to C int")
            })?;
            return native_iterator_reduce(&self_val, kind);
        }
        Ok(Value::tuple(vec![
            value_class(&self_val),
            Value::tuple(Vec::new()),
        ]))
    }

    /// Issue #2151: `object.__reduce__(self)` — the default reduction, which
    /// CPython implements as `self.__reduce_ex__(2)`.  Returns a tuple.
    #[py_name = "object.__reduce__"]
    fn object_reduce_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__reduce__", "object", method)
        })?;
        if let Some(kind) = native_iterator_class(&self_val) {
            return Err(pyrust_core::type_err!(
                "cannot pickle '{}' object",
                kind.class_name()
            ));
        }
        Ok(Value::tuple(vec![
            value_class(&self_val),
            Value::tuple(Vec::new()),
        ]))
    }

    /// Issue #2151: `None.__bool__()` returns `False`.  `__bool__` is
    /// NoneType-specific (not inherited from `object`).
    #[py_name = "NoneType.__bool__"]
    fn none_bool_dunder(args) -> Result<Value> {
        if args.is_empty() {
            return Err(pyrust_core::descriptor_needs_arg!("__bool__", "NoneType"));
        }
        Ok(Value::bool_(false))
    }

    /// Issue #1256: `object.__init__(self, *args, **kwargs)` — the default no-op
    /// `__init__` exposed on `object` so that `super().__init__()` in user
    /// classes terminates the MRO walk without error.
    ///
    /// Issue #1016: CPython 3.12 arg-leniency rule — extra positional/keyword
    /// arguments are accepted (and ignored) only when BOTH:
    ///   (a) `type(self)` defines a custom `__new__` (not `object.__new__`), AND
    ///   (b) `type(self)` does NOT define a custom `__init__` (i.e. inherits
    ///       `object.__init__`).
    /// This is the symmetric counterpart of the rule in `object_new_dunder`.
    /// In all other cases extra args raise `TypeError: object.__init__() takes
    /// exactly one argument (the instance to initialize)`.
    ///
    /// CPython signature: `object.__init__(self, /)`
    #[py_name = "object.__init__"]
    fn object_init_dunder(args) -> Result<Value> {
        // CPython 3.12 descriptor protocol: object.__init__() called with no
        // arguments (no self) raises TypeError.
        // Reproduced from CPython's slot_tp_init / descriptor wrappers:
        //   TypeError: descriptor '__init__' of 'object' object needs an argument
        if args.is_empty() {
            return Err(pyrust_core::descriptor_needs_arg!("__init__", "object"));
        }
        // Only args beyond the mandatory first (self) are "extra".
        let has_extra_args = args.len() > 1 || args.iter().skip(1).any(|a| a.name.is_some());
        if has_extra_args {
            // Determine whether the leniency rule applies.  From CPython's
            // object_init in Objects/typeobject.c:
            //
            //   if (excess_args(args, kwds)) {
            //       if (type->tp_new == object_new) {
            //           /* no custom __new__ → error */
            //       } else if (type->tp_init != object_init) {
            //           /* custom __new__ AND custom __init__ → error */
            //       }
            //       /* else: custom __new__, no custom __init__ → lenient */
            //   }
            //
            // Lenient iff: has_custom_new AND NOT has_custom_init.
            let self_val = args.first().map(|a| a.value.clone());
            let class_rc_opt = self_val.as_ref().and_then(|v| match v.kind() {
                ValueKind::PyInstance(inst) => Some(Rc::clone(&inst.borrow().class)),
                _ => None,
            });
            // CPython error message prefix:
            //   - when type(self) has a custom __init__: "object.__init__()"
            //   - when type(self) has no custom __init__: "<typename>.__init__()"
            // (Objects/typeobject.c, object_init:
            //    PyErr_Format(..., "%.100s.__init__()", Py_TYPE(self)->tp_name)
            //    when tp_init == object_init; "object.__init__()" otherwise)
            let (is_lenient, err_prefix) = if let Some(ref class_rc) = class_rc_opt {
                // "Custom __new__" = any __new__ that is not object.__new__.
                // This includes:
                //   (a) user-defined __new__ (UserFunction), or
                //   (b) a registered builtin __new__ (str.__new__, int.__new__,
                //       etc.) — BuiltinFunction with name != "object.__new__",
                //   (c) a primitive subclass that uses the type's builtin
                //       constructor as its allocator (e.g. complex/list/dict/set
                //       which lack an explicit __new__ registration but are
                //       handled by find_scalar/mutable/immutable_primitive_base
                //       in call_class_expanded).  In CPython these types have
                //       tp_new != object_new at the C level.
                let new_val = lookup_class_attr(class_rc, "__new__");
                let has_custom_new = match new_val.as_ref().map(|v| v.kind()) {
                    Some(ValueKind::UserFunction(_)) => true,
                    Some(ValueKind::BuiltinFunction("object.__new__")) => {
                        // __new__ resolved to object.__new__ via MRO.  This can
                        // happen for primitive types like complex/list/dict/set
                        // that have no explicit __new__ registration in pyrust.
                        // In CPython these types have tp_new != object_new at the
                        // C level, so treat their subclasses as having a custom
                        // __new__ by checking for primitive ancestry.
                        find_scalar_primitive_base(class_rc).is_some()
                            || find_mutable_primitive_base(class_rc).is_some()
                            || find_immutable_primitive_base(class_rc).is_some()
                    }
                    Some(ValueKind::BuiltinFunction(_)) => true,
                    _ => false,
                };
                // "Custom __init__" = any __init__ other than object.__init__:
                // both user-defined (UserFunction) and builtin subtype inits
                // (list.__init__, dict.__init__, etc.) count.  object.__init__
                // is the sentinel; None means the MRO has no __init__ at all
                // (only possible for pathological class structures).
                let init_val = lookup_class_attr(class_rc, "__init__");
                let has_custom_init = !matches!(
                    init_val.as_ref().map(|v| v.kind()),
                    None | Some(ValueKind::BuiltinFunction("object.__init__"))
                );
                // CPython error prefix: type name when no custom __init__,
                // "object" when a custom __init__ is present.
                let prefix = if has_custom_init {
                    "object".to_string()
                } else {
                    class_rc.borrow().name.clone()
                };
                (has_custom_new && !has_custom_init, prefix)
            } else {
                (false, "object".to_string())
            };
            if !is_lenient {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "{err_prefix}.__init__() takes exactly one argument (the instance to initialize)"
                    ),
                ));
            }
        }
        Ok(Value::none())
    }

    /// Issue #1143: `object.__new__(cls)` — the default allocator that creates
    /// a bare `PyInstance` of `cls`.  Registered so that `super().__new__(cls)`
    /// in user-defined `__new__` methods can resolve it via the MRO walk and
    /// `call_class_expanded` can distinguish it from user-defined `__new__`.
    ///
    /// Issue #1421: CPython 3.12 arg-leniency rule — extra positional/keyword
    /// arguments are accepted (and ignored) only when BOTH:
    ///   (a) `cls` does NOT define a custom `__new__` (i.e. cls.__new__ IS
    ///       object.__new__), AND
    ///   (b) `cls` defines a custom `__init__` (something other than
    ///       object.__init__).
    /// When `cls` defines a custom `__new__`, extra args raise
    /// `TypeError: object.__new__() takes exactly one argument (the type to
    /// instantiate)`.  When neither custom override is present, extra args
    /// raise `TypeError: <cls>() takes no arguments`.
    ///
    /// CPython signature: `object.__new__(cls, /)`
    #[py_name = "object.__new__"]
    fn object_new_dunder(args) -> Result<Value> {
        let cls_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            PyError::named(
                "TypeError",
                "object.__new__(): not enough arguments".to_string(),
            )
        })?;
        let class_rc = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "object.__new__(X): X is not a type object ({})",
                        value_type_name_str(&cls_val)
                    ),
                ));
            }
        };
        // These native built-in types own a backing layout. Allocating a bare
        // PyInstance through object.__new__ would create an object that passes
        // the class test but has no state for its native slots. CPython rejects
        // the same unsafe allocator bypass, including for exact `slice`.
        if class_has_native_builtin_type_ancestor(&class_rc) {
            let class_name = class_rc.borrow().name.clone();
            return Err(PyError::named(
                "TypeError",
                format!(
                    "object.__new__({class_name}) is not safe, use {class_name}.__new__()"
                ),
            ));
        }
        // Issue #1421: reject extra args unless the full CPython 3.12 leniency
        // rule is satisfied.  From Objects/typeobject.c (object_new):
        //
        //   lenient iff:
        //     (a) cls.__new__  IS  object.__new__  (no custom __new__), AND
        //     (b) cls.__init__ is NOT object.__init__ (has a custom __init__)
        //
        // When both (a) and (b) hold, the extra args are intended for the
        // custom __init__ and object.__new__ should silently ignore them.
        // In all other cases extra args are a programmer error:
        //
        //   - cls has a custom __new__ → the custom __new__ is responsible for
        //     any extra args; object.__new__ rejects them with the "takes
        //     exactly one argument" wording.
        //   - cls has no custom __init__ → no-one will consume the args →
        //     "<cls>() takes no arguments".
        // Only args beyond the mandatory first (cls) are "extra".  Do not
        // include the cls arg itself — it may arrive as a keyword arg via the
        // raw expanded-arg slice, and that must not trigger the leniency check.
        let has_extra_args = args.len() > 1 || args.iter().skip(1).any(|a| a.name.is_some());
        if has_extra_args {
            let new_val = lookup_class_attr(&class_rc, "__new__");
            let has_custom_new = new_val.as_ref().is_some_and(|value| {
                !crate::interpreter::value_is_canonical_slot(
                    value,
                    crate::interpreter::CanonicalSlot::ObjectNew,
                )
            });
            // Exception subclasses are special: CPython's BaseException.__new__
            // (BaseException_new in Objects/exceptions.c) accepts extra args and
            // stores them as .args.  In pyrust there is no separate
            // BaseException.__new__ registration — the MRO walk falls through to
            // object_new_dunder.  When cls is a BaseException subclass, mirror
            // CPython by accepting the extra args silently (they will be processed
            // by BaseException.__init__).
            let is_exception_subclass =
                pyrust_core::class_chain_contains_builtin_exception(&class_rc, "BaseException");
            if is_exception_subclass {
                // Accept extra args for exception subclasses unconditionally.
                // BaseException.__new__ is responsible for them in CPython.
            } else {
                if has_custom_new {
                    return Err(PyError::named(
                        "TypeError",
                        "object.__new__() takes exactly one argument (the type to instantiate)"
                            .to_string(),
                    ));
                }
                let init_val = lookup_class_attr(&class_rc, "__init__");
                let has_custom_init = matches!(
                    init_val.as_ref().map(|v| v.kind()),
                    Some(ValueKind::UserFunction(_))
                );
                if !has_custom_init {
                    let cls_name = class_rc.borrow().name.clone();
                    return Err(PyError::named(
                        "TypeError",
                        format!("{cls_name}() takes no arguments"),
                    ));
                }
            }
        }
        Ok(Value::py_instance(Rc::new(std::cell::RefCell::new(
            crate::value::PyInstance {
                class: class_rc,
                attrs: InstanceAttrs::new(),
            },
        ))))
    }
}
