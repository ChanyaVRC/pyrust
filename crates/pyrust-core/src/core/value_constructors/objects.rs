impl Value {
    // ── Container, function, class, and opaque object constructors ───────────

    /// Construct a heap-tuple Value from an existing `Rc<TupleInner>` — used when
    /// multiple Values must share the same immutable backing (cloning).  Consumes
    /// one strong-count reference from `rc`; the matching drop happens in
    /// `Drop for Value` when `TAG_TUPLE` is observed (#2268).
    unsafe fn tuple_from_rc(rc: Rc<TupleInner>) -> Self {
        let raw = Rc::into_raw(rc);
        Value::from_bits(encode_nanbox_heap_pointer(TAG_TUPLE_BITS, raw))
    }

    /// Construct a new `list` Value.  Storage is an `Rc<ListInner>` so that
    /// `Value::clone` shares the backing — matching Python's reference
    /// semantics for mutable containers (#305).
    pub fn list(v: Vec<Value>) -> Self {
        let inner = Rc::new(ListInner {
            items: RefCell::new(v),
            obj_id: next_obj_id(),
        });
        unsafe { Self::list_from_rc(inner) }
    }

    /// Construct a list Value from an existing `Rc<ListInner>` — used when
    /// multiple Values must share the same backing list (e.g. cloning).
    /// Caller is responsible for incrementing the strong count *before*
    /// calling this if they want a logical alias rather than a move.
    ///
    /// SAFETY: consumes one strong-count reference from `rc`.  The matching
    /// drop happens in `Drop for Value` when `TAG_LIST` is observed.
    unsafe fn list_from_rc(rc: Rc<ListInner>) -> Self {
        let raw = Rc::into_raw(rc);
        Value::from_bits(encode_nanbox_heap_pointer(TAG_LIST_BITS, raw))
    }

    pub fn tuple(mut v: Vec<Value>) -> Self {
        // Small-tuple fast path (#281): route 1- to 3-element tuples through
        // `Opaque::SmallTuple1/2/3` so the backing `Vec<Value>` heap allocation
        // is avoided.  These shapes dominate hot sites (`dict.items()`,
        // `enumerate()`, `divmod()`, `str.partition()`, exception `args`, …).
        match v.len() {
            0 => {
                // Empty tuple: share one immutable singleton, so `() is ()` is
                // True (CPython parity, where `()` is a singleton) and an empty
                // `*args` collection costs no allocation.
                EMPTY_TUPLE.with(|t| t.clone())
            }
            1 => {
                let a = v.pop().unwrap();
                Value::opaque(Opaque::SmallTuple1 {
                    items: [a],
                    obj_id: next_obj_id(),
                })
            }
            2 => {
                let b = v.pop().unwrap();
                let a = v.pop().unwrap();
                Value::opaque(Opaque::SmallTuple2 {
                    items: [a, b],
                    obj_id: next_obj_id(),
                })
            }
            3 => {
                let c = v.pop().unwrap();
                let b = v.pop().unwrap();
                let a = v.pop().unwrap();
                Value::opaque(Opaque::SmallTuple3 {
                    items: [a, b, c],
                    obj_id: next_obj_id(),
                })
            }
            _ => {
                let inner = Rc::new(TupleInner {
                    items: v,
                    obj_id: next_obj_id(),
                });
                unsafe { Self::tuple_from_rc(inner) }
            }
        }
    }

    /// Build a tuple by cloning `items`, without the intermediate `Vec` that
    /// [`Value::tuple`] needs for the empty and 1- to 3-element shapes
    /// (#3422): those go straight to the shared empty singleton or the inline
    /// `SmallTuple1/2/3` representation.  Used to pack a callee's `*args`.
    pub fn tuple_from_slice(items: &[Value]) -> Self {
        match items {
            [] => EMPTY_TUPLE.with(|t| t.clone()),
            [a] => Value::opaque(Opaque::SmallTuple1 {
                items: [a.clone()],
                obj_id: next_obj_id(),
            }),
            [a, b] => Value::opaque(Opaque::SmallTuple2 {
                items: [a.clone(), b.clone()],
                obj_id: next_obj_id(),
            }),
            [a, b, c] => Value::opaque(Opaque::SmallTuple3 {
                items: [a.clone(), b.clone(), c.clone()],
                obj_id: next_obj_id(),
            }),
            _ => Value::tuple(items.to_vec()),
        }
    }

    pub fn dict(d: PyDict) -> Self {
        Value::opaque(Opaque::Dict(Rc::new(RefCell::new(d))))
    }

    /// Construct a dictionary value that aliases an existing backing map.
    ///
    /// Dictionary views keep only this shared backing cell. Runtime protocols
    /// that must perform callback-safe live lookup can temporarily recover a
    /// normal `Value` without cloning and detaching the mapping.
    pub fn dict_shared(d: Rc<RefCell<PyDict>>) -> Self {
        Value::opaque(Opaque::Dict(d))
    }

    pub fn set(s: PySet) -> Self {
        Value::opaque(Opaque::Set(Rc::new(SetInner {
            items: RefCell::new(s),
            obj_id: next_obj_id(),
        })))
    }

    pub fn set_shared(s: Rc<SetInner>) -> Self {
        Value::opaque(Opaque::Set(s))
    }

    pub fn bytes(b: Vec<u8>) -> Self {
        Value::opaque(Opaque::Bytes(Rc::new(b)))
    }

    pub fn complex(re: f64, im: f64) -> Self {
        Value::opaque(Opaque::Complex(re, im))
    }

    /// Construct a generic built-in object dispatched through the installed
    /// [`BuiltinTypeOps`] table.  `ops` must outlive the program (typically
    /// `&'static`); `state` is owned heap state of any concrete type.
    pub fn builtin_object(ops: &'static dyn BuiltinTypeOps, state: Box<dyn Any>) -> Self {
        Value::opaque(Opaque::BuiltinObject {
            ops,
            state: Rc::new(RefCell::new(state)),
        })
    }

    /// Construct a generic built-in object that shares state with an existing
    /// `BuiltinState` cell.  Used when multiple Values must reference the
    /// same underlying mutable state.
    pub fn builtin_object_shared(ops: &'static dyn BuiltinTypeOps, state: BuiltinState) -> Self {
        Value::opaque(Opaque::BuiltinObject { ops, state })
    }

    pub fn range(start: i64, stop: i64, step: i64) -> Self {
        Value::opaque(Opaque::Range { start, stop, step })
    }

    /// Construct a `range` from arbitrary-precision bounds (#2118).  When all
    /// three bounds fit in `i64` this collapses to the cheap i64-backed
    /// [`Self::range`]; otherwise it produces a [`Opaque::BigRange`].  This keeps
    /// the common small-range path unchanged (no `BigRange` allocation, no extra
    /// match arms exercised) and only pays for `BigInt` storage when needed.
    pub fn range_big(start: BigInt, stop: BigInt, step: BigInt) -> Self {
        match (start.to_i64(), stop.to_i64(), step.to_i64()) {
            (Some(s), Some(e), Some(st)) => Value::range(s, e, st),
            _ => Value::opaque(Opaque::BigRange(Rc::new(BigRangeData {
                start,
                stop,
                step,
            }))),
        }
    }

    pub fn user_function(f: Rc<UserFunction>) -> Self {
        Value::opaque(Opaque::UserFunction(f))
    }

    /// Construct a built-in function value.  Stored as a `UserFunction` with
    /// `kind = Builtin(name)` so the function machinery is unified (one Opaque
    /// variant for both user and built-in functions).  The per-name
    /// `UserFunction` stub is interned in a thread-local cache so repeated
    /// calls don't reallocate it — equivalent in cost to the previous
    /// single-pointer payload.
    fn interned_builtin_function_rc(name: &'static str) -> Rc<UserFunction> {
        BUILTIN_FN_CACHE.with(|c| {
            if let Some(f) = c.borrow().get(name) {
                return Rc::clone(f);
            }
            let f = Rc::new(UserFunction {
                id: next_fn_id(),
                kind: UserFunctionKind::Builtin(name),
                name: Rc::from(name),
                qualname: Rc::from(name),
                name_overrides: RefCell::new(None),
                // Unset means the interpreter-owned registry supplies the
                // declaring module. An explicit deletion stores None.
                module: RefCell::new(Value::unset()),
                doc: RefCell::new(Value::none()),
                attrs: RefCell::new(None),
                annotations: RefCell::new(Value::unset()),
                defaults_override: RefCell::new(None),
                params: Vec::new(),
                param_binds: Rc::new(Vec::new()),
                memo_positional_parameter_count: 0,
                trampoline_arity: UserFunction::TRAMPOLINE_INELIGIBLE,
                self_bind: None,
                local_names: Rc::new(HashSet::new()),
                local_index: Rc::new(HashMap::new()),
                global_names: Rc::new(HashSet::new()),
                nonlocal_names: Rc::new(HashSet::new()),
                env: Environment::new(None),
                closure_cells: smallvec::SmallVec::new(),
                is_memo_pure: false,
                iterable_coroutine: std::cell::Cell::new(false),
                precompiled_code: None,
                wrapped_func: None,
            });
            c.borrow_mut().insert(name, Rc::clone(&f));
            f
        })
    }

    pub fn builtin_function(name: &'static str) -> Self {
        Value::opaque(Opaque::UserFunction(Self::interned_builtin_function_rc(
            name,
        )))
    }

    /// Construct a distinct builtin callable object with the same immutable
    /// name-based dispatcher as the interned template. Immutable empty
    /// tables/environment are Rc-shared with the template; mutable state
    /// starts fresh.
    pub fn fresh_builtin_function(name: &'static str) -> Self {
        let template = Self::interned_builtin_function_rc(name);
        let function = Rc::new(UserFunction {
            id: next_fn_id(),
            kind: template.kind,
            name: Rc::clone(&template.name),
            qualname: Rc::clone(&template.qualname),
            name_overrides: RefCell::new(None),
            module: RefCell::new(Value::unset()),
            doc: RefCell::new(Value::none()),
            attrs: RefCell::new(None),
            annotations: RefCell::new(Value::unset()),
            defaults_override: RefCell::new(None),
            params: template.params.clone(),
            param_binds: Rc::clone(&template.param_binds),
            memo_positional_parameter_count: template.memo_positional_parameter_count,
            trampoline_arity: UserFunction::TRAMPOLINE_INELIGIBLE,
            self_bind: template.self_bind,
            local_names: Rc::clone(&template.local_names),
            local_index: Rc::clone(&template.local_index),
            global_names: Rc::clone(&template.global_names),
            nonlocal_names: Rc::clone(&template.nonlocal_names),
            env: Rc::clone(&template.env),
            closure_cells: smallvec::SmallVec::new(),
            is_memo_pure: false,
            iterable_coroutine: std::cell::Cell::new(false),
            precompiled_code: None,
            wrapped_func: None,
        });
        Value::opaque(Opaque::UserFunction(function))
    }

    pub fn py_class(c: Rc<RefCell<PyClass>>) -> Self {
        Value::opaque(Opaque::PyClass(c))
    }

    pub fn py_instance(i: Rc<RefCell<PyInstance>>) -> Self {
        Value::opaque(Opaque::PyInstance(i))
    }

    pub fn py_module(m: Rc<RefCell<PyModule>>) -> Self {
        Value::opaque(Opaque::PyModule(m))
    }

    pub fn bound_method(function: Rc<UserFunction>, receiver: Rc<RefCell<PyInstance>>) -> Self {
        Value::opaque(Opaque::BoundMethod {
            function,
            receiver,
            obj_id: next_obj_id(),
        })
    }

    /// Wrap a function with a different `UserFunctionKind` tag.  Used by
    /// `@classmethod` / `@staticmethod`: produces a new UserFunction that
    /// shares everything but the kind tag.
    ///
    /// The wrapped function reuses the **original** `id` so the fn_cache and
    /// any other id-keyed caches share a single entry between the decorated
    /// and undecorated forms.  The function body and `is_memo_pure` flag are
    /// identical (the kind tag only affects attribute-lookup-time binding,
    /// not execution), so cache hits across forms are correct.  See #303.
    pub fn with_function_kind(f: Rc<UserFunction>, kind: UserFunctionKind) -> Self {
        // Fast path: kind already matches and is not a wrapper kind — reuse the Rc
        // directly.  Wrapper kinds (StaticMethod/ClassMethod) must always produce a
        // new Rc so that `staticmethod(sm)` gives a fresh object distinct from `sm`
        // (matching CPython identity semantics where each `staticmethod(x)` call
        // returns a new object regardless of whether `x` is already a staticmethod).
        let is_wrapper_kind = matches!(
            kind,
            UserFunctionKind::StaticMethod | UserFunctionKind::ClassMethod
        );
        if f.kind == kind && !is_wrapper_kind {
            return Value::opaque(Opaque::UserFunction(f));
        }
        // When wrapping as staticmethod/classmethod, record `f` directly so
        // `sm.__func__` returns the exact same Rc that was passed in, preserving
        // object identity (`sm.__func__ is f`).
        let wrapped_func = if is_wrapper_kind {
            Some(Rc::clone(&f))
        } else {
            None
        };
        let new_fn = UserFunction {
            id: f.id,
            kind,
            name: f.name.clone(),
            qualname: f.qualname.clone(),
            name_overrides: RefCell::new(f.name_overrides.borrow().clone()),
            module: RefCell::new(f.module.borrow().clone()),
            doc: RefCell::new(f.doc.borrow().clone()),
            attrs: RefCell::new(f.attrs.borrow().as_ref().map(Rc::clone)),
            annotations: RefCell::new(f.annotations.borrow().clone()),
            // Carry over any per-object defaults override (#2395) so a
            // staticmethod/classmethod wrapper observes the same `__defaults__`
            // / `__kwdefaults__` state as the wrapped function.
            defaults_override: RefCell::new(f.defaults_override.borrow().clone()),
            params: f.params.clone(),
            param_binds: Rc::clone(&f.param_binds),
            memo_positional_parameter_count: f.memo_positional_parameter_count,
            trampoline_arity: if matches!(kind, UserFunctionKind::Regular) {
                f.trampoline_arity
            } else {
                UserFunction::TRAMPOLINE_INELIGIBLE
            },
            self_bind: f.self_bind,
            local_names: Rc::clone(&f.local_names),
            local_index: Rc::clone(&f.local_index),
            global_names: Rc::clone(&f.global_names),
            nonlocal_names: Rc::clone(&f.nonlocal_names),
            env: Rc::clone(&f.env),
            closure_cells: f.closure_cells.clone(),
            is_memo_pure: f.is_memo_pure,
            iterable_coroutine: std::cell::Cell::new(f.iterable_coroutine.get()),
            precompiled_code: f.precompiled_code.clone(),
            wrapped_func,
        };
        Value::opaque(Opaque::UserFunction(Rc::new(new_fn)))
    }

    pub fn class_method(f: Rc<UserFunction>) -> Self {
        Value::with_function_kind(f, UserFunctionKind::ClassMethod)
    }

    pub fn static_method(f: Rc<UserFunction>) -> Self {
        Value::with_function_kind(f, UserFunctionKind::StaticMethod)
    }

    pub fn class_bound_method(function: Rc<UserFunction>, class: Rc<RefCell<PyClass>>) -> Self {
        Value::opaque(Opaque::ClassBoundMethod {
            function,
            class,
            obj_id: next_obj_id(),
        })
    }

    pub fn super_proxy(class: Rc<RefCell<PyClass>>, instance: Rc<RefCell<PyInstance>>) -> Self {
        Value::opaque(Opaque::SuperProxy {
            class,
            instance,
            obj_id: next_obj_id(),
        })
    }

    pub fn super_proxy_class(class: Rc<RefCell<PyClass>>, obj_class: Rc<RefCell<PyClass>>) -> Self {
        Value::opaque(Opaque::SuperProxyClass {
            class,
            obj_class,
            obj_id: next_obj_id(),
        })
    }

    /// Construct the unbound super object produced by `super(cls)` (#2704).
    pub fn super_proxy_unbound(class: Rc<RefCell<PyClass>>) -> Self {
        Value::opaque(Opaque::SuperProxyUnbound {
            class,
            obj_id: next_obj_id(),
        })
    }

    /// Create a built-in iterator value (`map`, `zip`, `range_iterator`, a
    /// provider iterator, …).  `state` is the type-erased cursor managed by the
    /// interpreter; these iterators release the cell before running user code,
    /// so their exact type name is read back out of `state`.
    pub fn generator(state: Box<dyn std::any::Any>) -> Self {
        Value::opaque(Opaque::Generator(Rc::new(GeneratorCell {
            kind: GeneratorKind::Iterator,
            iterable_coroutine: false,
            names: None,
            function: None,
            frame_ptr: None,
            state: RefCell::new(state),
        })))
    }

    /// Create a Python frame object — a generator, coroutine or async
    /// generator.  `state` is the type-erased `GeneratorFrame` managed by the
    /// VM; `kind`, the display names, the originating `UserFunction`, and the
    /// frame's stable heap address are stored beside it rather than inside it,
    /// because the cell is mutably checked out for as long as the body runs
    /// and the object must still be able to answer introspection queries
    /// (#2978, #2980).
    pub fn generator_frame(
        state: Box<dyn std::any::Any>,
        kind: GeneratorKind,
        names: Rc<RefCell<GeneratorNames>>,
        iterable_coroutine: bool,
        function: Rc<UserFunction>,
    ) -> Self {
        let frame_ptr =
            std::ptr::NonNull::new((&*state as *const dyn std::any::Any as *const ()).cast_mut());
        Value::opaque(Opaque::Generator(Rc::new(GeneratorCell {
            kind,
            iterable_coroutine,
            names: Some(names),
            function: Some(function),
            frame_ptr,
            state: RefCell::new(state),
        })))
    }

    pub fn generator_shared(cell: Rc<GeneratorCell>) -> Self {
        Value::opaque(Opaque::Generator(cell))
    }

    fn opaque(o: Opaque) -> Self {
        // SAFETY: `pool_opaque_alloc` returns a block sized/aligned for
        // `OpaqueSlot`
        // (either a recycled one from this thread's free list or a fresh
        // allocation).  Writing through the cast pointer initialises both the
        // refcount and payload; final `Drop` destroys the slot before returning
        // it to the pool.
        let ptr = unsafe { pool_opaque_alloc() as *mut OpaqueSlot };
        unsafe {
            std::ptr::write(
                ptr,
                OpaqueSlot {
                    strong: Cell::new(1),
                    weak: Cell::new(1),
                    value: std::mem::ManuallyDrop::new(o),
                },
            )
        };
        Value::from_bits(encode_nanbox_heap_pointer(TAG_OPAQUE_BITS, ptr))
    }
}
