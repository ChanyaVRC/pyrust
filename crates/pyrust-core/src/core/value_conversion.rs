impl Value {
    /// Unified int accessor (handles inline i48 and PyBigInt that fits in i64)
    pub fn as_int(&self) -> Option<i64> {
        debug_assert!(
            !self.is_unset(),
            "Value::as_int() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        match top16(self.0) {
            TAG_INT => Some(self.as_int_raw()),
            TAG_OPAQUE => {
                if let Opaque::PyBigInt(rc) = unsafe { &*self.opaque_ptr() } {
                    rc.to_i64()
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    // ── kind() — borrow-based view for pattern matching ──────────────────────

    pub fn kind(&self) -> ValueKind<'_> {
        // Catch reads of uninitialised register slots early.  In debug builds
        // this panics with a diagnostic message so the bug surfaces immediately
        // rather than silently propagating a NaN through the program.  Release
        // builds elide the assert (zero cost on the hot path).
        debug_assert!(
            !self.is_unset(),
            "Value::kind() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        // Reserved NaN-box sentinels: check before the float arm so they
        // don't get classified as float NaNs.
        if self.0 == NOT_IMPLEMENTED_BITS {
            return ValueKind::NotImplemented;
        }
        if self.0 == ELLIPSIS_BITS {
            return ValueKind::Ellipsis;
        }
        match top16(self.0) {
            t if t <= TAG_FLOAT_MAX => ValueKind::Float(self.as_float_raw()),
            TAG_NONE => ValueKind::None,
            TAG_BOOL => ValueKind::Bool(self.as_bool()),
            TAG_INT => ValueKind::Int(self.as_int_raw()),
            TAG_STR => ValueKind::Str(unsafe { self.str_as_str() }),
            TAG_TUPLE => ValueKind::Tuple(&unsafe { self.tuple_inner() }.items),
            // List/Dict/Set views: take a scoped `RefCell::borrow()` so
            // the cell's runtime borrow check is *honoured*.  A
            // concurrent `borrow_mut()` while the resulting ValueKind
            // is alive will panic with the standard already-borrowed
            // message — strictly safer than the previous
            // `unsafe { &*cell.as_ptr() }` bypass which produced silent
            // UB (#450).
            TAG_LIST => {
                let inner = unsafe { self.list_inner() };
                ValueKind::List(inner.items.borrow())
            }
            TAG_OPAQUE => match unsafe { &*self.opaque_ptr() } {
                Opaque::PyBigInt(rc) => {
                    if let Some(n) = rc.to_i64() {
                        ValueKind::Int(n)
                    } else {
                        ValueKind::BigInt(rc.as_ref())
                    }
                }
                Opaque::Dict(rc) => ValueKind::Dict(rc.as_ref().borrow()),
                Opaque::Set(rc) => ValueKind::Set(rc.items.borrow()),
                Opaque::Range { start, stop, step } => ValueKind::Range {
                    start: *start,
                    stop: *stop,
                    step: *step,
                },
                Opaque::BigRange(rc) => ValueKind::BigRange {
                    start: &rc.start,
                    stop: &rc.stop,
                    step: &rc.step,
                },
                Opaque::UserFunction(f) => match f.kind {
                    UserFunctionKind::Builtin(name) => ValueKind::BuiltinFunction(name),
                    _ => ValueKind::UserFunction(f),
                },
                Opaque::PyClass(c) => ValueKind::PyClass(c),
                Opaque::PyInstance(i) => ValueKind::PyInstance(i),
                Opaque::PyModule(m) => ValueKind::PyModule(m),
                Opaque::BoundMethod {
                    function, receiver, ..
                } => ValueKind::BoundMethod { function, receiver },
                Opaque::ClassBoundMethod {
                    function, class, ..
                } => ValueKind::ClassBoundMethod { function, class },
                Opaque::SuperProxy {
                    class, instance, ..
                } => ValueKind::SuperProxy { class, instance },
                Opaque::SuperProxyClass {
                    class, obj_class, ..
                } => ValueKind::SuperProxyClass { class, obj_class },
                Opaque::SuperProxyUnbound { class, .. } => ValueKind::SuperProxyUnbound { class },
                Opaque::Generator(state) => ValueKind::Generator(state),
                Opaque::Bytes(rc) => ValueKind::Bytes(rc),
                Opaque::Complex(re, im) => ValueKind::Complex(*re, *im),
                // Inline small tuples surface as `ValueKind::Tuple(&[Value])`
                // so all existing match arms keep working without learning
                // about the new variant.  See #281.
                Opaque::SmallTuple2 { items, .. } => ValueKind::Tuple(&items[..]),
                Opaque::SmallTuple3 { items, .. } => ValueKind::Tuple(&items[..]),
                Opaque::BuiltinObject { ops, state } => {
                    ValueKind::BuiltinObject { ops: *ops, state }
                }
            },
            _ => unreachable!(),
        }
    }

    // ── Existing Value methods rewritten with kind() ─────────────────────────

    /// Structural truthiness that bypasses `__bool__` / `__len__` dispatch.
    ///
    /// This is a *bypass* method: it never invokes user-defined dunder
    /// methods.  It exists for `pyrust-core` / `pyrust-builtins` code that has
    /// no `Interpreter` access.  From interpreter-layer code (including any
    /// `pyrust_module!` body) use `Interpreter::truthy_value` instead, which
    /// dispatches `__bool__` / `__len__` for user instances.
    pub fn truthy_raw(&self) -> bool {
        match self.kind() {
            ValueKind::Bool(v) => v,
            ValueKind::Int(v) => v != 0,
            ValueKind::BigInt(v) => !v.is_zero(),
            ValueKind::Float(v) => v != 0.0,
            ValueKind::Str(v) => !v.is_empty(),
            ValueKind::None => false,
            ValueKind::List(v) => !v.is_empty(),
            ValueKind::Dict(v) => !v.is_empty(),
            ValueKind::Set(v) => !v.is_empty(),
            ValueKind::Range { start, stop, step } => range_len(start, stop, step) > 0,
            ValueKind::BigRange { start, stop, step } => !bigrange_len(start, stop, step).is_zero(),
            ValueKind::UserFunction(_) => true,
            ValueKind::BuiltinFunction(_) => true,
            ValueKind::PyClass(_) => true,
            ValueKind::PyInstance(_) => true,
            ValueKind::BoundMethod { .. } => true,
            ValueKind::PyModule(_) => true,
            ValueKind::Tuple(v) => !v.is_empty(),
            ValueKind::ClassBoundMethod { .. } => true,
            ValueKind::SuperProxy { .. } => true,
            ValueKind::SuperProxyClass { .. } => true,
            ValueKind::SuperProxyUnbound { .. } => true,
            ValueKind::Generator(_) => true,
            ValueKind::NotImplemented => true,
            ValueKind::Ellipsis => true,
            // (NaN-box pattern handled by kind() dispatch above; included
            // in this match for completeness.)
            ValueKind::Bytes(b) => !b.is_empty(),
            ValueKind::Complex(re, im) => re != 0.0 || im != 0.0,
            ValueKind::BuiltinObject { ops, state } => ops.truthy(state),
        }
    }

    pub fn to_py_str(&self) -> String {
        match self.kind() {
            ValueKind::PyInstance(instance) if is_exception_instance(instance) => {
                exception_to_string(instance)
            }
            ValueKind::Str(s) => s.to_string(),
            _ => self.repr_raw(),
        }
    }

    /// Structural `repr()` that bypasses `__repr__` dispatch.
    ///
    /// This is a *bypass* method: it never invokes a user-defined `__repr__`.
    /// It exists for `pyrust-core` / `pyrust-builtins` code that has no
    /// `Interpreter` access (e.g. error-message formatting, panic debug).
    /// From interpreter-layer code (including any `pyrust_module!` body) use
    /// the `repr()` builtin, which dispatches `__repr__` for user instances.
    pub fn repr_raw(&self) -> String {
        match self.kind() {
            ValueKind::Int(v) => v.to_string(),
            ValueKind::BigInt(v) => v.to_string(),
            ValueKind::Float(v) => format_float(v),
            ValueKind::Str(v) => {
                let q = repr_quote(v);
                format!("{}{}{}", q, escape_str(v, q), q)
            }
            ValueKind::Bool(v) => {
                if v {
                    "True".to_string()
                } else {
                    "False".to_string()
                }
            }
            ValueKind::None => "None".to_string(),
            ValueKind::Ellipsis => "Ellipsis".to_string(),
            ValueKind::List(items) => {
                // Cycle detection (#364): if the same list is already being
                // formatted further up the call stack, emit CPython's
                // placeholder `[...]` instead of recursing into ourselves.
                let id = self.value_id();
                let _guard = match id {
                    Some(id) => match ReprGuard::enter(id) {
                        Some(g) => Some(g),
                        None => return "[...]".to_string(),
                    },
                    None => None,
                };
                let inner = items
                    .iter()
                    .map(|v| v.repr_raw())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{inner}]")
            }
            ValueKind::Dict(items) => {
                // Cycle detection (#364): self-referential dicts (via a value)
                // are reported as `{...}` by CPython.
                let id = self.value_id();
                let _guard = match id {
                    Some(id) => match ReprGuard::enter(id) {
                        Some(g) => Some(g),
                        None => return "{...}".to_string(),
                    },
                    None => None,
                };
                let mut out = String::new();
                out.push('{');
                for (i, (k, v)) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&key_repr(k));
                    out.push_str(": ");
                    out.push_str(&v.repr_raw());
                }
                out.push('}');
                out
            }
            ValueKind::Set(items) => {
                if items.is_empty() {
                    return "set()".to_string();
                }
                // Cycle detection (#364): a set can only hold hashable values,
                // and the cycle-producing collections (list/dict/set) aren't
                // hashable, so a true set self-cycle is impossible.  Keep the
                // guard anyway for defence-in-depth — the cost is one
                // thread-local lookup per set repr.
                let id = self.value_id();
                let _guard = match id {
                    Some(id) => match ReprGuard::enter(id) {
                        Some(g) => Some(g),
                        None => return "{...}".to_string(),
                    },
                    None => None,
                };
                let inner = items.iter().map(key_repr).collect::<Vec<_>>().join(", ");
                format!("{{{inner}}}")
            }
            ValueKind::Range { start, stop, step } => {
                if step == 1 {
                    format!("range({start}, {stop})")
                } else {
                    format!("range({start}, {stop}, {step})")
                }
            }
            ValueKind::BigRange { start, stop, step } => {
                if *step == BigInt::from(1) {
                    format!("range({start}, {stop})")
                } else {
                    format!("range({start}, {stop}, {step})")
                }
            }
            ValueKind::BuiltinFunction(name) => match builtin_callable_presentation(name) {
                BuiltinCallablePresentation::Function { name } => {
                    format!("<built-in function {name}>")
                }
                BuiltinCallablePresentation::MethodDescriptor { owner, name } => {
                    format!("<method '{name}' of '{owner}' objects>")
                }
                BuiltinCallablePresentation::WrapperDescriptor { owner, name } => {
                    format!("<slot wrapper '{name}' of '{owner}' objects>")
                }
            },
            ValueKind::UserFunction(func) => match func.kind {
                UserFunctionKind::ClassMethod => format!("<classmethod '{}'>", func.name),
                UserFunctionKind::StaticMethod => format!("<staticmethod '{}'>", func.name),
                UserFunctionKind::Regular => format!("<function {}>", func.name),
                // Builtins are surfaced via `ValueKind::BuiltinFunction` by
                // `kind()`, so we never reach this arm — but the match is
                // total either way.
                UserFunctionKind::Builtin(name) => format!("<built-in function {name}>"),
            },
            ValueKind::PyClass(class) => {
                let c = class.borrow();
                // Some pseudo-classes (e.g. the deprecated `typing.List`
                // aliases, CPython's `_SpecialGenericAlias`) render without
                // the `<class '...'>` wrapper.  This is keyed off a dedicated
                // `override_repr` field — never a `__dict__` attribute — so a
                // user class cannot hijack its own repr (issue #2608).
                if let Some(custom_repr) = c.override_repr.as_ref() {
                    return custom_repr.to_string();
                }
                let mut out = String::from("<class '");
                c.push_repr_display_name(&mut out);
                out.push_str("'>");
                out
            }
            ValueKind::PyInstance(instance) => {
                if is_exception_instance(instance) {
                    return exception_repr(instance);
                }
                // Builtin-subclass carrier with no repr-like override: render
                // as the backing value, matching CPython's inherited tp_repr
                // (issue #2389 — core-side renderers such as exception-arg
                // formatting reach this without interpreter access).
                if let Some(backing) = instance_backing_for_repr(instance) {
                    return backing.repr_raw();
                }
                let mut out = String::from("<");
                {
                    let inst = instance.borrow();
                    let class = inst.class.borrow();
                    class.push_repr_display_name(&mut out);
                }
                use std::fmt::Write as _;
                let addr = Rc::as_ptr(instance) as usize;
                let _ = write!(out, " object at 0x{addr:x}>");
                out
            }
            ValueKind::BoundMethod { function, receiver } => {
                let class_name = receiver.borrow().class.borrow().name.clone();
                format!("<bound method {class_name}.{}>", function.name)
            }
            ValueKind::PyModule(m) => format!("<module '{}'>", m.borrow().name),
            ValueKind::Tuple(items) => {
                // Cycle detection (#364): tuples are immutable so a *direct*
                // self-cycle isn't constructible from Python, but a tuple can
                // hold a list that holds the tuple — and the recursion still
                // passes through here.  CPython emits `(...)` for a tuple
                // self-cycle; we match that.
                let id = self.value_id();
                let _guard = match id {
                    Some(id) => match ReprGuard::enter(id) {
                        Some(g) => Some(g),
                        None => return "(...)".to_string(),
                    },
                    None => None,
                };
                let inner = items
                    .iter()
                    .map(|v| v.repr_raw())
                    .collect::<Vec<_>>()
                    .join(", ");
                if items.len() == 1 {
                    format!("({inner},)")
                } else {
                    format!("({inner})")
                }
            }
            ValueKind::ClassBoundMethod { function, class } => {
                format!("<bound method {}.{}>", class.borrow().name, function.name)
            }
            ValueKind::SuperProxy { class, .. } => {
                format!("<super: <class '{}'>>", class.borrow().name)
            }
            ValueKind::SuperProxyClass { class, .. } => {
                format!("<super: <class '{}'>>", class.borrow().name)
            }
            ValueKind::SuperProxyUnbound { class, .. } => {
                format!("<super: <class '{}'>, NULL>", class.borrow().name)
            }
            ValueKind::Generator(_) => "<generator object>".to_string(),
            ValueKind::NotImplemented => "NotImplemented".to_string(),
            ValueKind::Bytes(rc) => bytes_repr(rc),
            ValueKind::Complex(re, im) => complex_repr(re, im),
            ValueKind::BuiltinObject { ops, state } => ops.repr(state),
        }
    }

    pub fn to_key(&self) -> Option<PyKey> {
        match self.kind() {
            ValueKind::Int(v) => Some(PyKey::Int(v)),
            ValueKind::BigInt(v) => v
                .to_i64()
                .map(PyKey::Int)
                .or_else(|| Some(PyKey::BigInt(Box::new(v.clone())))),
            ValueKind::Float(v) => Some(PyKey::Float(v.to_bits())),
            ValueKind::Str(_) => Some(PyKey::Str(self.clone())),
            ValueKind::Bool(v) => Some(PyKey::Bool(v)),
            ValueKind::None => Some(PyKey::None),
            ValueKind::Ellipsis => Some(PyKey::Ellipsis),
            ValueKind::Bytes(rc) => Some(PyKey::Bytes(Rc::clone(rc))),
            // Every complex maps to `PyKey::Complex`, including a zero imaginary
            // part.  Collapsing `1+0j` to `PyKey::Float(1.0)` used to lose the
            // inserted key object, so `{1+0j: 'a'}` listed `1.0` instead of
            // `(1+0j)` (#2900).  Cross-type unification (`1+0j == 1 == 1.0`) is
            // instead provided by the real-valued `Complex <-> Int/Bool/Float/
            // BigInt` arms in `PyKey::PartialEq` plus the matching `Hash` arm,
            // so the four numeric types still share a single dict/set slot with
            // CPython's first-inserted-key-wins behaviour.
            ValueKind::Complex(re, im) => Some(PyKey::Complex(re, im)),
            ValueKind::BuiltinObject { ops, state } => ops.to_key(state),
            ValueKind::Tuple(items) => {
                // Recursively hash each element.  If any element is itself
                // unhashable (e.g. a list inside the tuple), the whole tuple
                // is unhashable — matches CPython's `hash((1, [2]))` raising
                // TypeError.
                let mut keys = Vec::with_capacity(items.len());
                for item in items {
                    keys.push(item.to_key()?);
                }
                Some(PyKey::Tuple(keys))
            }
            _ => None,
        }
    }
}
