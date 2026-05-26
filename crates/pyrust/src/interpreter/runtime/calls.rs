// Thread-local call depth counter. Using thread_local avoids the split-borrow
// problem: a guard that holds &mut self.call_depth cannot coexist with a &mut self
// method call. The thread_local is safe because the interpreter is single-threaded.


thread_local! {
    static CALL_DEPTH: Cell<usize> = const { Cell::new(0) };
}

fn call_depth() -> usize {
    CALL_DEPTH.with(|d| d.get())
}

// RAII guard: increments the thread-local call depth on creation and decrements
// it on drop, including on panic, without any unsafe code.
struct CallDepthGuard;

impl CallDepthGuard {
    fn enter() -> Self {
        CALL_DEPTH.with(|d| d.set(d.get() + 1));
        Self
    }
}

impl Drop for CallDepthGuard {
    fn drop(&mut self) {
        CALL_DEPTH.with(|d| d.set(d.get() - 1));
    }
}

/// Build the per-type `TypeError` message for sequence item access with a
/// non-integer non-`__index__` index, matching CPython 3.12.
///
/// CPython uses different message formats per type:
/// - list / tuple / bytes: `"X indices must be integers or slices, not Y"`
/// - string: `"string indices must be integers, not 'Y'"` (different wording
///   and the type name is quoted).
fn seq_index_type_error(label: &str, type_name: &str) -> String {
    if label == "string" {
        format!("string indices must be integers, not '{type_name}'")
    } else {
        format!("{label} indices must be integers or slices, not {type_name}")
    }
}

impl Interpreter {
    /// Shared constructor for the `GeneratorFrame` wrapped in a
    /// `Value::generator`. Both call-site branches in
    /// `call_user_function_expanded` (simple-arity and variadic) bind
    /// params into `regs`, capture the active env, then need to short-
    /// circuit before `run_bytecode_for_fn`. The two branches must keep
    /// the frame initialisation identical — factored here so issue #488
    /// (variadic) can't drift from the simple-path baseline again.
    fn build_generator_value(
        code: &Rc<crate::bytecode::FnCode>,
        regs: RegsBuf,
        saved_env: EnvRef,
        local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
        fn_name: std::sync::Arc<str>,
    ) -> Value {
        let frame = GeneratorFrame {
            code: Rc::clone(code),
            regs,
            iters: vec![None; code.num_iters as usize],
            exc_handlers: Vec::new(),
            pc: 0,
            done: false,
            saved_env,
            // PEP 3134 per-generator exception state; both empty until
            // the body actually pushes handlers and yields inside one.
            handled_exc_slice: Vec::new(),
            active_exception: None,
            local_index,
            // Meaningless until the first yield; initialised to 0 as a safe
            // default (the Yield opcode always overwrites this before resumption).
            yield_dst: 0,
            // Set by resume_generator_with_exc when FrameOutcome::Returned(val)
            // is received; read by Insn::YieldFrom to retrieve the sub-iterator's
            // StopIteration.value (PEP 380).
            last_return_value: None,
            // Stored so resume_generator_with_exc can name the generator's
            // traceback frame when an exception propagates out (issue #908).
            fn_name,
        };
        Value::generator(Box::new(frame))
    }

    pub(crate) fn call_function_expanded(
        &mut self,
        function: Value,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        // Registry-driven dispatch: built-in callables declared via
        // `pyrust_module! { … fn name(args) … }` (or the per-fn fallback
        // `#[pyfunction(name = …)]`) are collected into
        // `crate::builtin_registry`.  We probe it first; the only arms left
        // in the match cascade below are pattern-guarded (bound-method
        // dispatch, `str.*` method-name prefix matching, the `property`
        // accessor partial-slot guard) — these key on `Value` state rather
        // than on a registered name, so they can't live in the registry.
        // See `crates/pyrust/src/builtin_modules/` for the per-module bodies.

        // Issue #988: `super().__init__(args)` on a dict/list/set subclass.
        // The SuperProxy wraps the resolved BuiltinFunction sentinel together
        // with the instance in a `super_bound_builtin` object so that `self`
        // is available here.  Prepend the instance and call the registry
        // dispatch — mirroring what `invoke_class_method` does for the normal
        // `__init__` path (called from `call_class_expanded`).
        if let Some((fn_name, instance)) =
            pyrust_builtins::super_bound_builtin::as_super_bound_builtin(&function)
        {
            if let Some(dispatch) = crate::builtin_registry::lookup(&fn_name) {
                let mut combined: Vec<ExpandedCallArg> = Vec::with_capacity(args.len() + 1);
                combined.push(ExpandedCallArg { name: None, value: instance });
                combined.extend(args.iter().cloned());
                return dispatch(self, &combined);
            }
        }

        if let ValueKind::BuiltinFunction(name) = function.kind()
            && let Some(dispatch) = crate::builtin_registry::lookup(name)
        {
            return dispatch(self, args);
        }
        match function.kind() {
            // Migrated to `crate::builtin_modules::builtins`:
            //   print, range, open, __vcall__, plus all the simple top-level
            //   builtins (abs, len, …) — dispatched via the registry probe
            //   at the top of this fn.  `math.*` and `sys.exit` similarly
            //   live in their per-module bodies.
            _ if pyrust_builtins::bound_method::as_bound_method(&function).is_some() => {
                let (name_rc, receiver_owned) =
                    pyrust_builtins::bound_method::as_bound_method(&function).unwrap();
                // Borrow the method name as `&str` from the `Rc<String>` rather
                // than cloning into a fresh `String` — the dispatch helpers
                // below all accept `&str`. See issue #276 item #1.
                let method: &str = name_rc.as_str();
                let receiver = receiver_owned;
                // Reuse the interpreter-level positional-args buffer so that
                // tight loops calling a bound method pay zero allocation once
                // the buffer has grown to the high-watermark arg count (issue
                // #276 item #3).  The buffer is taken with `std::mem::take` so
                // no borrow-checker split is needed; it is restored at the end
                // of this arm (via `bound_method_pos_buf = pos` after the match).
                let mut pos = std::mem::take(&mut self.bound_method_pos_buf);
                pos.clear();
                // Keyword-args fast path (issue #276 item #4): skip IndexMap
                // construction entirely when all arguments are positional —
                // the common case for builtin bound methods.
                let has_kw = args.iter().any(|a| a.name.is_some());
                let mut kw: indexmap::IndexMap<PyKey, Value> = indexmap::IndexMap::new();
                if has_kw {
                    for a in args {
                        match &a.name {
                            Some(n) => { kw.insert(PyKey::str_from(n.as_str()), a.value.clone()); }
                            None => pos.push(a.value.clone()),
                        }
                    }
                } else {
                    for a in args {
                        pos.push(a.value.clone());
                    }
                }
                // Bound-method dispatch: each builtin takes `&Value`
                // and scopes its own `RefCell::borrow_mut()` for the
                // duration of the operation (#448).  No `&mut Vec /
                // Map / Set` crosses the crate boundary, so the
                // previous `unalias_args_for_mutation` dance is
                // unnecessary — aliasing-safety is structural now,
                // not a discipline the caller has to remember.
                // Probe the receiver's kind via a scoped block so the
                // `kind()` Ref guards drop before we may move
                // `receiver` into call_dict_method / call_set_method
                // (#450).  Variants that read the kind's payload
                // (Tuple, Complex, BuiltinObject, PyInstance) keep
                // working off the live Ref because they don't move
                // the receiver.
                enum Kind {
                    Int,
                    Float,
                    Bytes,
                    Str,
                    List,
                    Dict,
                    Set,
                    Other,
                }
                let kind_tag = match receiver.kind() {
                    // bool is a subclass of int in CPython; route to the int
                    // dispatch so True.bit_length() / True.is_integer() work.
                    ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_) => Kind::Int,
                    ValueKind::Float(_) => Kind::Float,
                    ValueKind::Bytes(_) => Kind::Bytes,
                    ValueKind::Str(_) => Kind::Str,
                    ValueKind::List(_) => Kind::List,
                    ValueKind::Dict(_) => Kind::Dict,
                    ValueKind::Set(_) => Kind::Set,
                    _ => Kind::Other,
                };
                // Arms that accept `&[Value]` (Int, Float, Bytes) borrow `pos`
                // directly — the buf's capacity is fully preserved on return.
                // Arms that need `Vec<Value>` ownership drain `pos` into a
                // fresh allocation so the (now-empty) buf can be returned to
                // `bound_method_pos_buf` below, retaining its capacity for the
                // next call.  Early-return error paths also restore the buf.
                let result = match kind_tag {
                    Kind::Int => {
                        if !kw.is_empty() {
                            self.bound_method_pos_buf = pos;
                            return Err(PyError::named(
                                "TypeError",
                                format!("int.{method}() takes no keyword arguments"),
                            ));
                        }
                        pyrust_builtins::int::call(method, &receiver, &pos)
                    }
                    Kind::Float => {
                        let f = match receiver.kind() {
                            ValueKind::Float(f) => f,
                            _ => unreachable!("kind_tag guard above"),
                        };
                        pyrust_builtins::float::call(method, f, &pos)
                    }
                    Kind::Bytes => {
                        if method == "join" {
                            let args_vec: Vec<Value> = pos.drain(..).collect();
                            self.call_bytes_join(receiver, args_vec)
                        } else {
                            pyrust_builtins::bytes::call(method, &receiver, &pos, &kw)
                        }
                    }
                    Kind::Str => {
                        // `format` needs kwargs threaded into the template.
                        // Intercept before `call_str_method`, which only receives
                        // positional args and would silently drop keyword arguments.
                        if method == "format" {
                            let template = match receiver.kind() {
                                ValueKind::Str(s) => s.to_string(),
                                _ => {
                                    self.bound_method_pos_buf = pos;
                                    return Err(PyError::named(
                                        "TypeError",
                                        "descriptor 'format' requires a 'str' object".to_string(),
                                    ));
                                }
                            };
                            let keyword: Vec<(String, Value)> = kw
                                .into_iter()
                                .filter_map(|(k, v)| {
                                    if let PyKey::Str(name) = k {
                                        Some((name.as_str().unwrap_or("").to_owned(), v))
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            // Borrow pos; capacity retained in the buf below.
                            self.format_str_template(&template, &pos, &keyword)
                        } else if !kw.is_empty() {
                            // Resolve kwargs for str methods before passing to
                            // call_str_method, which only accepts positional args.
                            match str_merge_kwargs(method, &mut pos, kw) {
                                Ok(()) => {
                                    let args_vec: Vec<Value> = pos.drain(..).collect();
                                    self.call_str_method(method, receiver, args_vec)
                                }
                                Err(e) => {
                                    self.bound_method_pos_buf = pos;
                                    return Err(e);
                                }
                            }
                        } else {
                            // call_str_method takes Vec<Value> by value; drain
                            // so pos retains its capacity for the next call.
                            let args_vec: Vec<Value> = pos.drain(..).collect();
                            self.call_str_method(method, receiver, args_vec)
                        }
                    }
                    Kind::List => {
                        let args_vec: Vec<Value> = pos.drain(..).collect();
                        let args_vec = if method == "index" {
                            match self.resolve_seq_index_pos(args_vec) {
                                Ok(v) => v,
                                Err(e) => {
                                    self.bound_method_pos_buf = pos;
                                    return Err(e);
                                }
                            }
                        } else {
                            args_vec
                        };
                        pyrust_builtins::list::call(method, &receiver, args_vec, &kw)
                    }
                    Kind::Dict => {
                        let args_vec: Vec<Value> = pos.drain(..).collect();
                        self.call_dict_method(method, receiver, args_vec)
                    }
                    Kind::Set => {
                        let args_vec: Vec<Value> = pos.drain(..).collect();
                        self.call_set_method(method, receiver, args_vec)
                    }
                    Kind::Other => match receiver.kind() {
                    ValueKind::Tuple(items) => {
                        let args_vec: Vec<Value> = pos.drain(..).collect();
                        let args_vec = if method == "index" {
                            match self.resolve_seq_index_pos(args_vec) {
                                Ok(v) => v,
                                Err(e) => {
                                    self.bound_method_pos_buf = pos;
                                    return Err(e);
                                }
                            }
                        } else {
                            args_vec
                        };
                        pyrust_builtins::tuple::call(method, items, args_vec)
                    }
                    ValueKind::Complex(_, _) => {
                        let args_vec: Vec<Value> = pos.drain(..).collect();
                        pyrust_builtins::complex::call(method, &receiver, args_vec)
                    }
                    ValueKind::BuiltinObject { ops, state } => {
                        let args_vec: Vec<Value> = pos.drain(..).collect();
                        let empty_kw: indexmap::IndexMap<String, Value> =
                            indexmap::IndexMap::new();
                        ops.call_method(state, method, args_vec, &empty_kw)
                    }
                    ValueKind::PyInstance(inst) => {
                        // Class method backed by a `BuiltinFunction` — emitted
                        // by `pyrust_module!`'s `class { … }` block.  `get_attr`
                        // wrapped the method-name-on-instance pair as a
                        // bound_method; here we re-resolve the method on the
                        // class and dispatch with `self` prepended through the
                        // unified helper.
                        let class = Rc::clone(&inst.borrow().class);
                        let method_val = match lookup_class_attr(&class, method) {
                            Some(v) => v,
                            None => {
                                self.bound_method_pos_buf = pos;
                                return Err(PyError::named(
                                    "AttributeError",
                                    format!(
                                        "'{}' object has no attribute '{method}'",
                                        class.borrow().name,
                                    ),
                                ));
                            }
                        };
                        // Issue #976/#994: if the resolved method is a primitive
                        // builtin (e.g. `dict.keys`, `tuple.count`), and the
                        // instance has a `__builtin_data__` backing value (set at
                        // construction time for subclasses of dict/list/set/
                        // frozenset/tuple), dispatch on the backing value directly.
                        // This avoids the `kind_ok` type guard in
                        // `call_function_expanded` rejecting the PyInstance receiver.
                        if let ValueKind::BuiltinFunction(fn_name) = method_val.kind() {
                            if fn_name.split_once('.').is_some_and(|(t, _)| {
                                matches!(t, "dict" | "list" | "set" | "frozenset" | "tuple")
                            }) {
                                if let Some(backing) = instance_builtin_data(&inst) {
                                    enum BkKind { Dict, List, Set, Frozenset, Tuple, Other }
                                    let bk_kind = match backing.kind() {
                                        ValueKind::Dict(_) => BkKind::Dict,
                                        ValueKind::List(_) => BkKind::List,
                                        ValueKind::Set(_) => BkKind::Set,
                                        ValueKind::BuiltinObject { ops, .. }
                                            if ops.type_name() == "frozenset" =>
                                        {
                                            BkKind::Frozenset
                                        }
                                        ValueKind::Tuple(_) => BkKind::Tuple,
                                        _ => BkKind::Other,
                                    };
                                    let args_vec: Vec<Value> = pos.drain(..).collect();
                                    self.bound_method_pos_buf = pos;
                                    return match bk_kind {
                                        BkKind::Dict => {
                                            self.call_dict_method(method, backing, args_vec)
                                        }
                                        BkKind::List => {
                                            pyrust_builtins::list::call(
                                                method,
                                                &backing,
                                                args_vec,
                                                &kw,
                                            )
                                        }
                                        BkKind::Set => {
                                            self.call_set_method(method, backing, args_vec)
                                        }
                                        BkKind::Frozenset => {
                                            pyrust_builtins::frozenset::call(
                                                method,
                                                &backing,
                                                args_vec,
                                            )
                                        }
                                        BkKind::Tuple => match backing.kind() {
                                            ValueKind::Tuple(items) => {
                                                pyrust_builtins::tuple::call(
                                                    method,
                                                    items,
                                                    args_vec,
                                                )
                                            }
                                            _ => unreachable!("BkKind::Tuple guard above"),
                                        },
                                        BkKind::Other => Err(PyError::Runtime(format!(
                                            "internal: unexpected builtin_data kind for method '{method}'"
                                        ))),
                                    };
                                }
                            }
                        }
                        // Reconstitute kwargs as ExpandedCallArgs (the
                        // bound_method dispatch split them into pos+kw maps).
                        // Drain pos so its capacity is preserved in the buf.
                        let mut combined: Vec<ExpandedCallArg> =
                            Vec::with_capacity(pos.len() + kw.len());
                        for v in pos.drain(..) {
                            combined.push(ExpandedCallArg { name: None, value: v });
                        }
                        for (k, v) in kw {
                            if let PyKey::Str(name) = k {
                                combined.push(ExpandedCallArg {
                                    name: Some(name.as_str().unwrap_or("").to_owned()),
                                    value: v,
                                });
                            }
                        }
                        invoke_class_method(
                            self,
                            method_val,
                            Value::py_instance(inst.clone()),
                            &combined,
                        )
                    }
                    _ => Err(PyError::named(
                        "TypeError",
                        format!("'{}' object has no method '{method}'", pyrust_core::builtin_type_name(&receiver)),
                    )),
                    },
                };
                // Restore the positional-args buffer.  For borrow arms (Int,
                // Float, Bytes, Str::format) pos still holds all elements with
                // full capacity.  For drain arms pos is empty but retains the
                // grown capacity, avoiding a re-allocation on the next call.
                self.bound_method_pos_buf = pos;
                result
            }
            ValueKind::BuiltinFunction("str.format") => {
                let self_val = args
                    .first()
                    .map(|a| &a.value)
                    .ok_or_else(|| PyError::named(
                        "TypeError",
                        "descriptor 'format' of 'str' object needs an argument".to_string(),
                    ))?;
                let template = match self_val.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Err(PyError::named(
                        "TypeError",
                        "descriptor 'format' requires a 'str' object".to_string(),
                    )),
                };
                let mut positional: Vec<Value> = Vec::new();
                let mut keyword: Vec<(String, Value)> = Vec::new();
                for a in &args[1..] {
                    match &a.name {
                        Some(n) => keyword.push((n.clone(), a.value.clone())),
                        None => positional.push(a.value.clone()),
                    }
                }
                self.format_str_template(&template, &positional, &keyword)
            }
            // `float.fromhex` is a classmethod: the first positional arg is the
            // string to parse.  It must be dispatched before the generic
            // `"float.*"` arm below so that the arg-0-is-receiver assumption
            // in that arm is not applied here.
            ValueKind::BuiltinFunction("float.fromhex") => {
                // Accept both `float.fromhex(s)` and `(1.0).fromhex(s)`.
                // Filter out a leading float or class receiver if present, then
                // enforce exactly one remaining positional argument.
                let positional_args: Vec<_> = args
                    .iter()
                    .filter(|a| {
                        a.name.is_none()
                            && !matches!(
                                a.value.kind(),
                                ValueKind::Float(_) | ValueKind::PyClass(_)
                            )
                    })
                    .collect();
                // If every arg was a float/class receiver (e.g. pure class call
                // with no string) fall back to requiring exactly one total arg.
                let n_payload = if positional_args.is_empty() {
                    args.len()
                } else {
                    positional_args.len()
                };
                if n_payload == 0 {
                    return Err(PyError::named(
                        "TypeError",
                        "float.fromhex() takes exactly one argument (0 given)",
                    ));
                }
                if n_payload > 1 {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "float.fromhex() takes exactly one argument ({} given)",
                            n_payload
                        ),
                    ));
                }
                let s_val = if positional_args.is_empty() {
                    args.first().map(|a| a.value.clone())
                } else {
                    positional_args.first().map(|a| a.value.clone())
                }
                .ok_or_else(|| {
                    PyError::named(
                        "TypeError",
                        "float.fromhex() takes exactly one argument (0 given)",
                    )
                })?;
                let s = match s_val.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "bad argument type for built-in operation",
                        ))
                    }
                };
                pyrust_builtins::float::fromhex(&s).map(Value::float)
            }
            // float instance methods via descriptor call: `float.is_integer(x)`.
            // The float call fn takes an `f64` receiver directly, so this arm
            // is separate from the generic str/list/… arm below.
            ValueKind::BuiltinFunction(name)
                if name.split_once('.').is_some_and(|(t, _)| t == "float") =>
            {
                let (_, method) = name.split_once('.').unwrap();
                let self_val = args
                    .first()
                    .map(|a| a.value.clone())
                    .ok_or_else(|| {
                        PyError::named(
                            "TypeError",
                            format!("descriptor '{method}' of 'float' object needs an argument"),
                        )
                    })?;
                let f = match self_val.kind() {
                    ValueKind::Float(f) => f,
                    _ => {
                        let actual = pyrust_core::builtin_type_name(&self_val);
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "descriptor '{method}' for 'float' objects doesn't apply to a '{actual}' object",
                            ),
                        ));
                    }
                };
                let pos: Vec<Value> = args[1..]
                    .iter()
                    .filter(|a| a.name.is_none())
                    .map(|a| a.value.clone())
                    .collect();
                pyrust_builtins::float::call(method, f, &pos)
            }
            // PEP 585: `__class_getitem__` classmethods on built-in collection
            // types.  Sentinel names follow the pattern
            // `"<type>.__class_getitem__"` (registered by
            // `build_primitive_classes` in `helpers.rs`).
            // This arm handles the direct-call path:
            //   `list.__class_getitem__(int)` → GenericAlias(list, (int,))
            // The `list[int]` subscript path is handled directly in
            // `eval_index` without passing through here.
            // This arm must come before the instance-method arm below because
            // `"list.__class_getitem__"` would otherwise match that arm's
            // `"list"` prefix guard and raise a spurious descriptor TypeError.
            ValueKind::BuiltinFunction(name)
                if name
                    .split_once('.')
                    .is_some_and(|(_, m)| m == "__class_getitem__") =>
            {
                let type_name = name.split_once('.').unwrap().0;
                // Recover the origin class value from the per-thread singleton.
                let origin_class = primitive_class_by_name(type_name).ok_or_else(|| {
                    PyError::Runtime(format!(
                        "internal: unknown primitive class for __class_getitem__: {type_name}"
                    ))
                })?;
                // Accept one positional argument: the type parameter(s).
                //   `list.__class_getitem__(int)`       → args[0] = int
                //   `list.__class_getitem__((str, int))` → args[0] = (str, int)
                let index = args
                    .iter()
                    .find(|a| a.name.is_none())
                    .map(|a| a.value.clone())
                    .ok_or_else(|| {
                        PyError::named(
                            "TypeError",
                            format!(
                                "descriptor '__class_getitem__' of '{type_name}' object \
                                 needs an argument"
                            ),
                        )
                    })?;
                let is_tuple = matches!(index.kind(), ValueKind::Tuple(_));
                let type_args = if is_tuple { index } else { Value::tuple(vec![index]) };
                Ok(pyrust_builtins::generic_alias::generic_alias(
                    Value::py_class(origin_class),
                    type_args,
                ))
            }
            ValueKind::BuiltinFunction("str.format_map") => {
                let self_val = args
                    .first()
                    .map(|a| &a.value)
                    .ok_or_else(|| PyError::named(
                        "TypeError",
                        "descriptor 'format_map' of 'str' object needs an argument".to_string(),
                    ))?;
                let template = match self_val.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Err(PyError::named(
                        "TypeError",
                        "descriptor 'format_map' requires a 'str' object".to_string(),
                    )),
                };
                // format_map takes exactly one positional argument (the mapping).
                let rest = &args[1..];
                let kw_count = rest.iter().filter(|a| a.name.is_some()).count();
                let pos_count = rest.iter().filter(|a| a.name.is_none()).count();
                if pos_count != 1 || kw_count != 0 {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "str.format_map() takes exactly one argument ({} given)",
                            pos_count + kw_count
                        ),
                    ));
                }
                let mapping = rest[0].value.clone();
                self.format_str_template_map(&template, mapping)
            }
            // #462: class-method-of-primitive dispatch.  When a primitive
            // class's attr is `BuiltinFunction("<type>.<method>")` — populated
            // by `populate_*_methods` in `helpers.rs` — calling it dispatches
            // like a bound method with `args[0]` as the receiver.  Mirrors the
            // bound-method arm above so `str.upper(s)` and `s.upper()` go
            // through the same per-type `call` fn.  `str.format` is handled
            // by the preceding arm because it threads kwargs into the template.
            ValueKind::BuiltinFunction(name)
                if name
                    .split_once('.')
                    .is_some_and(|(t, _)| matches!(t, "int" | "bytes" | "str" | "list" | "tuple" | "dict" | "set" | "complex" | "frozenset")) =>
            {
                let (type_name, method) = name.split_once('.').unwrap();
                let self_val = args
                    .first()
                    .map(|a| a.value.clone())
                    .ok_or_else(|| PyError::named(
                        "TypeError",
                        format!("descriptor '{method}' of '{type_name}' object needs an argument"),
                    ))?;
                let mut pos: Vec<Value> = Vec::with_capacity(args.len().saturating_sub(1));
                let mut kw: indexmap::IndexMap<PyKey, Value> = indexmap::IndexMap::new();
                for a in &args[1..] {
                    match &a.name {
                        Some(n) => { kw.insert(PyKey::str_from(n.as_str()), a.value.clone()); }
                        None => pos.push(a.value.clone()),
                    }
                }
                // Issue #976/#994: if `self_val` is a PyInstance with a
                // `__builtin_data__` backing value (set at construction for
                // subclasses of dict/list/set/frozenset/tuple), use that
                // backing value as the effective receiver so the kind_ok
                // check and dispatch below see the expected primitive type.
                let self_val =
                    if matches!(type_name, "dict" | "list" | "set" | "frozenset" | "tuple") {
                        // Extract the Rc before kind() drops its borrow.
                        let maybe_inst = if let ValueKind::PyInstance(inst) = self_val.kind() {
                            Some(Rc::clone(inst))
                        } else {
                            None
                        };
                        if let Some(inst) = maybe_inst {
                            instance_builtin_data(&inst).unwrap_or(self_val)
                        } else {
                            self_val
                        }
                    } else {
                        self_val
                    };
                // Receiver-type guard: validate `self_val`'s kind matches
                // `type_name` before dispatch — otherwise the per-type call fn
                // surfaces an internal `"expected dict"` / `"receiver is not a
                // list"` runtime error instead of the descriptor TypeError that
                // CPython raises.  See Copilot review on #463.
                let kind_ok = match (type_name, self_val.kind()) {
                    ("int", ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_)) => true,
                    ("bytes", ValueKind::Bytes(_)) => true,
                    ("str", ValueKind::Str(_)) => true,
                    ("list", ValueKind::List(_)) => true,
                    ("tuple", ValueKind::Tuple(_)) => true,
                    ("dict", ValueKind::Dict(_)) => true,
                    ("set", ValueKind::Set(_)) => true,
                    ("complex", ValueKind::Complex(_, _)) => true,
                    ("frozenset", ValueKind::BuiltinObject { ops, .. })
                        if ops.type_name() == "frozenset" => true,
                    _ => false,
                };
                if !kind_ok {
                    let actual = pyrust_core::builtin_type_name(&self_val);
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "descriptor '{method}' for '{type_name}' objects doesn't apply to a '{actual}' object",
                        ),
                    ));
                }
                match type_name {
                    "int" => {
                        if !kw.is_empty() {
                            return Err(PyError::named(
                                "TypeError",
                                format!("int.{method}() takes no keyword arguments"),
                            ));
                        }
                        pyrust_builtins::int::call(method, &self_val, &pos)
                    }
                    "bytes" => pyrust_builtins::bytes::call(method, &self_val, &pos, &kw),
                    "str" => {
                        if kw.is_empty() || method == "format" {
                            self.call_str_method(method, self_val, pos)
                        } else {
                            str_merge_kwargs(method, &mut pos, kw)?;
                            self.call_str_method(method, self_val, pos)
                        }
                    }
                    "list" => {
                        let pos = if method == "index" {
                            self.resolve_seq_index_pos(pos)?
                        } else {
                            pos
                        };
                        pyrust_builtins::list::call(method, &self_val, pos, &kw)
                    }
                    "tuple" => {
                        let pos = if method == "index" {
                            self.resolve_seq_index_pos(pos)?
                        } else {
                            pos
                        };
                        match self_val.kind() {
                            ValueKind::Tuple(items) => {
                                pyrust_builtins::tuple::call(method, items, pos)
                            }
                            _ => unreachable!("kind_ok guard above"),
                        }
                    }
                    "dict" => self.call_dict_method(method, self_val, pos),
                    "set" => self.call_set_method(method, self_val, pos),
                    "complex" => pyrust_builtins::complex::call(method, &self_val, pos),
                    "frozenset" => pyrust_builtins::frozenset::call(method, &self_val, pos),
                    _ => unreachable!("guard matched type_name above"),
                }
            }
            ValueKind::UserFunction(function) => {
                let function = Rc::clone(function);
                self.call_user_function_expanded(function, args, &[])
            }
            ValueKind::PyClass(class) => {
                // #462 perf: primitive classes (`int`, `str`, …) bypass
                // `call_class_expanded` entirely and dispatch straight
                // to the same registry fn that `BuiltinFunction("int")`
                // would.  One `HashMap` lookup + fn pointer call vs.
                // the three-layer Pyinstance-alloc / lookup / re-call
                // chain.
                if let Some(dispatch) = primitive_class_dispatch(class) {
                    return dispatch(self, args);
                }
                let class = Rc::clone(class);
                self.call_class_expanded(class, args)
            }
            ValueKind::BoundMethod { function, receiver } => {
                let function = Rc::clone(function);
                let receiver = Rc::clone(receiver);
                self.call_user_function_expanded(function, args, &[Value::py_instance(receiver)])
            }
            ValueKind::ClassBoundMethod { function, class } => {
                let function = Rc::clone(function);
                let class = Rc::clone(class);
                // First argument is the class itself (not an instance)
                self.call_user_function_expanded(function, args, &[Value::py_class(class)])
            }
            // `format`, `classmethod`, `staticmethod` migrated to
            // `crate::builtin_modules::builtins`.

            // Calling prop.setter(fn), prop.deleter(fn), or prop.getter(fn).
            // Returns a new Property with the respective slot replaced.
            _ if pyrust_builtins::property::property_partial_slot(&function)
                .is_some_and(|s| s.is_some()) =>
            {
                let (fget, fset, fdel, slot) =
                    pyrust_builtins::property::with_property(&function, |s| {
                        (
                            Rc::clone(&s.fget),
                            Rc::clone(&s.fset),
                            Rc::clone(&s.fdel),
                            s.partial_slot.expect("guard ensured Some"),
                        )
                    })
                    .expect("guard ensured property");
                reject_keyword_args_expanded("property accessor", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "property accessor takes exactly one argument".to_string(),
                    ));
                }
                let new_fn = args[0].value.clone();
                let (fget_val, fset_val, fdel_val) = match slot {
                    0 => (new_fn, (*fset).clone(), (*fdel).clone()),
                    1 => ((*fget).clone(), new_fn, (*fdel).clone()),
                    2 => ((*fget).clone(), (*fset).clone(), new_fn),
                    _ => unreachable!(),
                };
                Ok(pyrust_builtins::property::property(
                    fget_val, fset_val, fdel_val,
                ))
            }

            // `property` (named-arm form) and `super` migrated to
            // `crate::builtin_modules::builtins`.  `super` reaches the
            // registry through `#[py_name = "super"]` on its Rust fn
            // (named `super_fn`) — `super` is a strict Rust keyword that
            // rejects even the raw-ident form, so the explicit override is
            // the only way to give it its Python-level name.

            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__call__") {
                    return invoke_class_method(
                        self,
                        method_val,
                        Value::py_instance(inst_rc),
                        args,
                    );
                }
                Err(PyError::named(
                    "TypeError",
                    format!("'{}' object is not callable", class.borrow().name),
                ))
            }
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "'{}' object is not callable",
                    pyrust_core::builtin_type_name(&function)
                ),
            )),
        }
    }

    /// Collect all values from an iterable (including generators) into a Vec.
    pub(crate) fn collect_iterable(&mut self, val: Value) -> Result<Vec<Value>> {
        if let ValueKind::Generator(state_rc) = val.kind() {
            let state_rc = Rc::clone(state_rc);

            // Fast path: NativeIterFrame — drain remaining items in one shot.
            {
                let mut borrow = state_rc.borrow_mut();
                if let Some(native) = borrow.downcast_mut::<NativeIterFrame>() {
                    let remaining: Vec<Value> = native.items[native.pos..].to_vec();
                    native.pos = native.items.len();
                    return Ok(remaining);
                }
            }

            // CallableIter path: drive iter(callable, sentinel) to exhaustion.
            {
                let is_callable_iter = state_rc
                    .borrow()
                    .downcast_ref::<CallableIter>()
                    .is_some();
                if is_callable_iter {
                    let mut items = Vec::new();
                    loop {
                        match self.step_callable_iter(&state_rc) {
                            Ok(Some(v)) => items.push(v),
                            Ok(None) => break,
                            // StopIteration raised by the callable stops the
                            // iteration gracefully — CPython treats it the same
                            // as hitting the sentinel (list/tuple exhaustion).
                            Err(ref e) if e.class_name_is("StopIteration") => break,
                            Err(e) => return Err(e),
                        }
                    }
                    return Ok(items);
                }
            }

            // GetItemIter path: drive __getitem__(0), __getitem__(1), … to exhaustion.
            {
                let is_getitem_iter = state_rc
                    .borrow()
                    .downcast_ref::<GetItemIter>()
                    .is_some();
                if is_getitem_iter {
                    let mut items = Vec::new();
                    loop {
                        match self.step_getitem_iter(&state_rc)? {
                            Some(v) => items.push(v),
                            None => break,
                        }
                    }
                    return Ok(items);
                }
            }

            // GeneratorFrame path: drive the generator to exhaustion.
            let mut items = Vec::new();
            loop {
                let mut borrow = state_rc.borrow_mut();
                let frame = borrow
                    .downcast_mut::<GeneratorFrame>()
                    .ok_or_else(|| PyError::Runtime("invalid generator state".to_string()))?;
                if frame.done {
                    break;
                }
                match self.resume_generator(frame) {
                    Ok(yielded) => {
                        drop(borrow);
                        items.push(yielded);
                    }
                    Err(ref e) if e.class_name_is("StopIteration") => {
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok(items)
        } else if let ValueKind::PyInstance(inst) = val.kind() {
            // CPython fallback order is `__iter__` first, then the legacy
            // sequence-iter protocol via `__getitem__`.  Having only
            // `__next__` does *not* make a class iterable — that property
            // belongs to iterator objects, not iterables (#416 Copilot
            // review).
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            // Check __builtin_data__ before __getitem__: list/dict/set subclasses
            // with no user-defined __iter__ should iterate the backing primitive.
            if lookup_class_attr(&class, "__iter__").is_none() {
                if let Some(backing) = instance_builtin_data(&inst_rc) {
                    return self.collect_iterable(backing);
                }
            }
            let iterator = if let Some(method_val) = lookup_class_attr(&class, "__iter__") {
                invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(Rc::clone(&inst_rc)),
                    &[],
                )?
            } else if lookup_class_attr(&class, "__getitem__").is_some() {
                // Legacy sequence-iter protocol — lazy iterator that
                // drives `__getitem__(0)`, `__getitem__(1)`, … on
                // demand and terminates on IndexError/StopIteration.
                self.make_getitem_iter(Rc::clone(&inst_rc))?
            } else {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{}' object is not iterable", class.borrow().name),
                ));
            };
            let mut items = Vec::new();
            // Cache the __next__ method once for PyInstance iterators to avoid a
            // per-iteration class-walk (lookup_class_attr traverses the full MRO).
            if let ValueKind::PyInstance(iter_inst) = iterator.kind() {
                let iter_class = Rc::clone(&iter_inst.borrow().class);
                let Some(next_method) = lookup_class_attr(&iter_class, "__next__") else {
                    return Err(PyError::named(
                        "TypeError",
                        format!("'{}' object is not an iterator", iter_class.borrow().name),
                    ));
                };
                loop {
                    match invoke_class_method(self, next_method.clone(), iterator.clone(), &[]) {
                        Ok(v) => items.push(v),
                        Err(ref e) if e.class_name_is("StopIteration") => break,
                        Err(e) => return Err(e),
                    }
                }
            } else {
                loop {
                    match self.call_next(iterator.clone(), None) {
                        Ok(item) => items.push(item),
                        // class_name_is now walks the hierarchy for Raised variants,
                        // so StopIteration subclasses raised by __next__ are caught here.
                        Err(ref e) if e.class_name_is("StopIteration") => break,
                        Err(e) => return Err(e),
                    }
                }
            }
            Ok(items)
        } else {
            iter_values(val)
        }
    }

    /// Build a lazy iterator wrapping the legacy `__getitem__`
    /// sequence-iter protocol for `inst_rc`.  Returns a
    /// `Value::generator(...)` that downcasts to [`GetItemIter`].
    /// Each `next()` call invokes `inst.__getitem__(i)` once; the
    /// caller's `break`/early-return correctly stops at the first
    /// unused index (CPython semantics, #394).  Issue #416 Copilot
    /// review: switched from eager materialise to lazy after the
    /// reviewer flagged the observable break/short-circuit gap.
    pub(crate) fn make_getitem_iter(&self, inst_rc: Rc<RefCell<PyInstance>>) -> Result<Value> {
        let class = Rc::clone(&inst_rc.borrow().class);
        let method_val = lookup_class_attr(&class, "__getitem__").ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!("'{}' object is not iterable", class.borrow().name),
            )
        })?;
        let obj = Value::py_instance(inst_rc);
        Ok(Value::generator(Box::new(GetItemIter {
            obj,
            method: method_val,
            index: 0,
            exhausted: false,
        })))
    }

    /// One step of the lazy `__getitem__` iterator.
    /// `Ok(Some(v))` → next element; `Ok(None)` → exhausted (caller
    /// should yield `StopIteration` or return default); `Err(e)` →
    /// any non-terminator exception from `__getitem__` propagates.
    ///
    /// Called from `call_next`'s GetItemIter branch and from
    /// `ForIter`'s `UserDefined` arm via the same downcast.
    pub(crate) fn step_getitem_iter(
        &mut self,
        state_rc: &Rc<RefCell<Box<dyn std::any::Any>>>,
    ) -> Result<Option<Value>> {
        let snapshot: Option<(Value, Value, i64)> = {
            let borrow = state_rc.borrow();
            if let Some(it) = borrow.downcast_ref::<GetItemIter>() {
                if it.exhausted {
                    return Ok(None);
                }
                Some((it.obj.clone(), it.method.clone(), it.index))
            } else {
                return Err(PyError::Runtime("step_getitem_iter on non-GetItemIter state".to_string()));
            }
        };
        let (obj, method, index) = snapshot.unwrap();
        let arg = ExpandedCallArg {
            name: None,
            value: Value::int(index),
        };
        let result = invoke_class_method(self, method, obj, &[arg]);
        match result {
            Ok(v) => {
                if let Some(it) = state_rc.borrow_mut().downcast_mut::<GetItemIter>() {
                    it.index = it.index.saturating_add(1);
                }
                Ok(Some(v))
            }
            Err(e) if is_sequence_iter_terminator(self, &e) => {
                if let Some(it) = state_rc.borrow_mut().downcast_mut::<GetItemIter>() {
                    it.exhausted = true;
                }
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// One step of the callable iterator created by `iter(callable, sentinel)`.
    /// Invokes `callable()` with no arguments, then checks whether the returned
    /// value equals `sentinel`.  Returns `Ok(Some(v))` when a value was
    /// produced, `Ok(None)` when the sentinel was matched (exhausted), or
    /// `Err(e)` when the callable raised.
    ///
    /// The borrow on `state_rc` is fully released before `call_function_expanded`
    /// is invoked, mirroring the `step_getitem_iter` approach.
    pub(crate) fn step_callable_iter(
        &mut self,
        state_rc: &Rc<RefCell<Box<dyn std::any::Any>>>,
    ) -> Result<Option<Value>> {
        // Extract callable and sentinel while releasing the borrow, so that
        // call_function_expanded can re-enter the interpreter without aliasing.
        let snapshot: Option<(Value, Value)> = {
            let borrow = state_rc.borrow();
            if let Some(it) = borrow.downcast_ref::<CallableIter>() {
                if it.done {
                    return Ok(None);
                }
                Some((it.callable.clone(), it.sentinel.clone()))
            } else {
                return Err(PyError::Runtime(
                    "step_callable_iter on non-CallableIter state".to_string(),
                ));
            }
        };
        let (callable, sentinel) = snapshot.unwrap();
        let result = self.call_function_expanded(callable, &[]);
        match result {
            Ok(v) => {
                let equal = self.values_user_eq(&v, &sentinel)?;
                if equal {
                    if let Some(it) = state_rc.borrow_mut().downcast_mut::<CallableIter>() {
                        it.done = true;
                    }
                    Ok(None)
                } else {
                    Ok(Some(v))
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Call next() on a generator or any object with __next__.
    pub(crate) fn call_next(&mut self, val: Value, default: Option<Value>) -> Result<Value> {
        if let ValueKind::Generator(state_rc) = val.kind() {
            let state_rc = Rc::clone(state_rc);

            // Fast path: NativeIterFrame (no VM required).
            {
                let mut borrow = state_rc.borrow_mut();
                if let Some(native) = borrow.downcast_mut::<NativeIterFrame>() {
                    if native.pos >= native.items.len() {
                        return if let Some(d) = default {
                            Ok(d)
                        } else {
                            Err(PyError::named("StopIteration", String::new()))
                        };
                    }
                    let item = native.items[native.pos].clone();
                    native.pos += 1;
                    return Ok(item);
                }
            }

            // GetItemIter path: drive one `__getitem__(i)` call lazily.
            // Borrow released by step_getitem_iter before invoking the
            // method (it would otherwise re-entrantly re-borrow).
            {
                let is_getitem = state_rc
                    .borrow()
                    .downcast_ref::<GetItemIter>()
                    .is_some();
                if is_getitem {
                    return match self.step_getitem_iter(&state_rc)? {
                        Some(v) => Ok(v),
                        None => {
                            if let Some(d) = default {
                                Ok(d)
                            } else {
                                Err(PyError::named("StopIteration", String::new()))
                            }
                        }
                    };
                }
            }

            // CallableIter path: invoke callable(), stop when result == sentinel.
            {
                let is_callable_iter = state_rc
                    .borrow()
                    .downcast_ref::<CallableIter>()
                    .is_some();
                if is_callable_iter {
                    return match self.step_callable_iter(&state_rc)? {
                        Some(v) => Ok(v),
                        None => {
                            if let Some(d) = default {
                                Ok(d)
                            } else {
                                Err(PyError::named("StopIteration", String::new()))
                            }
                        }
                    };
                }
            }

            // GeneratorFrame path.
            let mut borrow = state_rc.borrow_mut();
            let frame = borrow
                .downcast_mut::<GeneratorFrame>()
                .ok_or_else(|| PyError::Runtime("invalid generator state".to_string()))?;
            if frame.done {
                drop(borrow);
                return if let Some(d) = default {
                    Ok(d)
                } else {
                    // Exhausted generator: StopIteration() with no args → .value is None.
                    let exc = if let Some(cls) = self.exc_classes.get("StopIteration") {
                        PyError::Raised(instantiate_exception(cls, vec![]))
                    } else {
                        PyError::named("StopIteration", String::new())
                    };
                    Err(exc)
                };
            }
            match self.resume_generator(frame) {
                Ok(yielded) => Ok(yielded),
                Err(e) if e.class_name_is("StopIteration") => {
                    drop(borrow);
                    if let Some(d) = default {
                        Ok(d)
                    } else {
                        // Propagate the original error so StopIteration.value
                        // is preserved (PEP 380 / issue #600).
                        Err(e)
                    }
                }
                Err(e) => Err(e),
            }
        } else if let ValueKind::PyInstance(inst) = val.kind() {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__next__") {
                match invoke_class_method(self, method_val, Value::py_instance(inst_rc), &[]) {
                    Ok(v) => Ok(v),
                    Err(PyError::Raised(exc)) => {
                        // Use class_name_is so StopIteration subclasses are
                        // correctly detected (hierarchy walk, not exact name).
                        if PyError::Raised(exc.clone()).class_name_is("StopIteration") {
                            if let Some(d) = default {
                                Ok(d)
                            } else {
                                Err(PyError::Raised(exc))
                            }
                        } else {
                            Err(PyError::Raised(exc))
                        }
                    }
                    Err(e) => Err(e),
                }
            } else {
                Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object is not an iterator",
                        class.borrow().name
                    ),
                ))
            }
        } else if let ValueKind::BuiltinObject { ops, state } = val.kind()
            && ops.is_iterable()
        {
            match ops.iter_next(state)? {
                Some(v) => Ok(v),
                None => {
                    if let Some(d) = default {
                        Ok(d)
                    } else {
                        Err(PyError::named("StopIteration", String::new()))
                    }
                }
            }
        } else {
            Err(PyError::named(
                "TypeError",
                format!("'{}' object is not an iterator", value_type_name_str(&val)),
            ))
        }
    }

    pub(crate) fn parse_print_options_expanded(&mut self, args: &[ExpandedCallArg]) -> Result<PrintOptions> {
        let mut values = Vec::new();
        let mut sep = String::from(" ");
        let mut end = String::from("\n");

        for arg in args {
            let value = arg.value.clone();
            match arg.name.as_deref() {
                None => values.push(value),
                Some("sep") => {
                    sep = extract_optional_string(value, "sep")?.unwrap_or_else(|| " ".to_string());
                }
                Some("end") => {
                    end =
                        extract_optional_string(value, "end")?.unwrap_or_else(|| "\n".to_string());
                }
                Some("file") => {
                    if !value.is_none() {
                        return Err(PyError::Runtime(
                            "print() file argument is not supported yet".to_string(),
                        ));
                    }
                }
                Some("flush") => match value.kind() {
                    ValueKind::Bool(_) => {}
                    _ => {
                        return Err(PyError::Runtime(
                            "print() flush must be a boolean".to_string(),
                        ));
                    }
                },
                Some(other) => {
                    return Err(PyError::Runtime(format!(
                        "print() got an unexpected keyword argument '{}'",
                        other
                    )));
                }
            }
        }

        Ok(PrintOptions { values, sep, end })
    }

    pub(crate) fn call_user_function_expanded(
        &mut self,
        function: Rc<UserFunction>,
        args: &[ExpandedCallArg],
        bound_prefix: &[Value],
    ) -> Result<Value> {
        // Check if function has *args or **kwargs
        let has_args_param = function.params.iter().any(|p| p.is_args);
        let has_kwargs_param = function.params.iter().any(|p| p.is_kwargs);

        let positional_count = args.iter().filter(|arg| arg.name.is_none()).count();

        if !has_args_param && !has_kwargs_param {
            // Fast path: no variadic params - original logic
            let required_params = function
                .params
                .iter()
                .filter(|param| param.default.is_none())
                .count();
            if positional_count + bound_prefix.len() > function.params.len() {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "{}() takes from {} to {} arguments but {} were given",
                        function.name,
                        required_params,
                        function.params.len(),
                        positional_count + bound_prefix.len()
                    ),
                ));
            }
            let mut bound_args: Vec<Option<Value>> = vec![None; function.params.len()];
            for (index, value) in bound_prefix.iter().enumerate() {
                bound_args[index] = Some(value.clone());
            }
            let mut positional_index = bound_prefix.len();
            for arg in args {
                let value = arg.value.clone();
                if let Some(name) = &arg.name {
                    let Some(param_index) =
                        function.params.iter().position(|param| param.name == *name)
                    else {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "{}() got an unexpected keyword argument '{}'",
                                function.name, name
                            ),
                        ));
                    };
                    if function.params[param_index].is_positional_only {
                        // The fast path only runs when the function has neither
                        // *args nor **kwargs (see the `if !has_args_param &&
                        // !has_kwargs_param` guard above), so there is no
                        // **kwargs to absorb this name — TypeError is correct.
                        // The variadic path (`compute_kw_pos` below) handles
                        // the "absorb into **kwargs" case separately.
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "{}() got some positional-only arguments passed as keyword arguments: '{}'",
                                function.name, name
                            ),
                        ));
                    }
                    if bound_args[param_index].is_some() {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "{}() got multiple values for argument '{}'",
                                function.name, name
                            ),
                        ));
                    }
                    bound_args[param_index] = Some(value);
                } else {
                    while positional_index < bound_args.len() && bound_args[positional_index].is_some() {
                        positional_index += 1;
                    }
                    if positional_index >= bound_args.len() {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "{}() takes from {} to {} arguments but {} were given",
                                function.name,
                                required_params,
                                function.params.len(),
                                positional_count + bound_prefix.len()
                            ),
                        ));
                    }
                    bound_args[positional_index] = Some(value);
                    positional_index += 1;
                }
            }
            // Resolve defaults: fill any still-empty bound_args slots in-place.
            for (index, param) in function.params.iter().enumerate() {
                if bound_args[index].is_none() {
                    bound_args[index] = Some(param.default.clone().ok_or_else(|| {
                        if param.is_keyword_only {
                            PyError::named(
                                "TypeError",
                                format!(
                                    "{}() missing 1 required keyword-only argument: '{}'",
                                    function.name, param.name
                                ),
                            )
                        } else {
                            PyError::named(
                                "TypeError",
                                format!(
                                    "{}() missing required positional argument: '{}'",
                                    function.name, param.name
                                ),
                            )
                        }
                    })?);
                }
            }

            // Memoization: build cache key by borrowing from bound_args — no extra clone.
            // MemoKey wraps PyKey and includes the ValueKind discriminant so that
            // Float(1.0) and Int(1) — equal as PyKey but distinct types — are never
            // treated as the same cache entry (fixes #562).
            let cache_key: Option<(u64, Vec<MemoKey>)> = if function.is_pure {
                bound_args
                    .iter()
                    .map(|v| v.as_ref().unwrap().to_key().map(MemoKey))
                    .collect::<Option<Vec<MemoKey>>>()
                    .map(|keys| (function.id, keys))
            } else {
                None
            };
            if let Some(ref ck) = cache_key
                && let Some(cached) = self.fn_cache.get(ck).cloned() {
                    return Ok(cached);
                }

            // Tier-0: register-VM path — try compiled bytecode before any env allocation.
            if let Some(code) = self.get_or_compile_bytecode(&function) {
                let num_regs = code.num_regs as usize;
                let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];

                let _depth_guard = CallDepthGuard::enter();
                if call_depth() > MAX_CALL_DEPTH {
                    let exc = if let Some(cls) = self.exc_classes.get("RecursionError") {
                        instantiate_exception(
                            cls,
                            vec![Value::string("maximum recursion depth exceeded")],
                        )
                    } else {
                        self.instantiate_named_exception(
                            "RecursionError",
                            "maximum recursion depth exceeded".to_string(),
                        )?
                    };
                    return Err(PyError::Raised(exc));
                }

                // Create a local env when the function uses globals, nonlocals, or cell vars.
                let needs_local_env = !function.global_names.is_empty()
                    || !function.nonlocal_names.is_empty()
                    || !code.cell_vars.is_empty();

                // Bind params: move each value from bound_args into a register or env
                // cell — one pass, zero extra clones.
                let previous_env = if needs_local_env {
                    let local_env = self.alloc_env(Some(Rc::clone(&function.env)));
                    {
                        let mut e = local_env.borrow_mut();
                        e.local_names = Rc::clone(&function.local_names);
                        e.global_names = Rc::clone(&function.global_names);
                        e.nonlocal_names = Rc::clone(&function.nonlocal_names);
                        for (param, slot) in function.params.iter().zip(bound_args.iter_mut()) {
                            let val = slot.take().unwrap();
                            if code.cell_vars.contains(&param.name) {
                                e.values.insert(param.name.clone(), val);
                            } else if let Some(&reg) = function.local_index.get(&param.name) {
                                if reg as usize >= num_regs {
                                    return Err(PyError::named(
                                        "SystemError",
                                        format!(
                                            "parameter '{}' register index {} out of range (num_regs={})",
                                            param.name, reg, num_regs
                                        ),
                                    ));
                                }
                                regs[reg as usize] = val;
                            }
                        }
                    }
                    std::mem::replace(&mut self.env, local_env)
                } else {
                    for (param, slot) in function.params.iter().zip(bound_args.iter_mut()) {
                        let val = slot.take().unwrap();
                        if let Some(&reg) = function.local_index.get(&param.name) {
                            if reg as usize >= num_regs {
                                return Err(PyError::named(
                                    "SystemError",
                                    format!(
                                        "parameter '{}' register index {} out of range (num_regs={})",
                                        param.name, reg, num_regs
                                    ),
                                ));
                            }
                            regs[reg as usize] = val;
                        }
                    }
                    std::mem::replace(&mut self.env, Rc::clone(&function.env))
                };

                // Self-reference for recursive calls (only if not a cell var).
                if !code.cell_vars.contains(&function.name)
                    && let Some(&slot) = function.local_index.get(&function.name) {
                        if slot as usize >= num_regs {
                            return Err(PyError::named(
                                "SystemError",
                                format!(
                                    "self-reference register index {} out of range (num_regs={})",
                                    slot, num_regs
                                ),
                            ));
                        }
                        regs[slot as usize] = Value::user_function(Rc::clone(&function));
                    }

                // Generator function: create a frame rather than executing.
                if code.is_generator {
                    // Restore env before capturing it into the frame.
                    // (When `needs_local_env` is false, `gen_env` ==
                    // `function.env` — the GeneratorFrame keeps it alive.)
                    let gen_env = std::mem::replace(&mut self.env, previous_env);
                    return Ok(Self::build_generator_value(
                        &code,
                        regs,
                        gen_env,
                        Rc::clone(&function.local_index),
                        std::sync::Arc::from(function.name.as_str()),
                    ));
                }

                // Issue #389: publish a view of this function frame so
                // `locals()` can surface its fastlocal registers
                // mid-call.  Popped immediately after `run_bytecode`
                // returns so the raw pointer never outlives `regs`.
                // Issue #486: also capture nonlocal_names and the
                // current env so `snapshot_current_locals` can resolve
                // nonlocal bindings that live in enclosing envs.
                let nonlocal_names_opt = if function.nonlocal_names.is_empty() {
                    None
                } else {
                    Some(Rc::clone(&function.nonlocal_names))
                };
                let env_opt = if function.nonlocal_names.is_empty() {
                    None
                } else {
                    Some(Rc::clone(&self.env))
                };
                // Capture the raw pointer and length BEFORE constructing RegSlice
                // so both the VmFrameView and the dispatch loop share the same raw
                // pointer with no &mut [Value] in scope (issue #547 / PR #646).
                let regs_ptr = unsafe {
                    std::ptr::NonNull::new_unchecked(regs.as_mut_ptr())
                };
                let regs_len = regs.len();
                self.vm_frame_views.push(VmFrameView {
                    kind: FrameKind::Function,
                    // SAFETY: SmallVec / Vec allocation is always non-null.
                    // Popped before `regs` is dropped (see above).
                    regs_ptr,
                    regs_len,
                    local_index: Rc::clone(&function.local_index),
                    nonlocal_names: nonlocal_names_opt,
                    env: env_opt,
                    is_class_method: code.is_class_method,
                });
                // SAFETY: regs_ptr is valid for regs_len Values for the lifetime
                // of `regs` (a local RegsBuf that outlives this call).  No
                // &mut [Value] referencing `regs` is held while the dispatch loop
                // runs; RegSlice (raw pointer + len) is used instead, removing
                // the LLVM noalias constraint (issue #547).
                let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
                // Push a traceback frame so errors propagating out of this
                // function body carry the correct `File / in <name>` entry.
                // Cloning an `Arc<str>` is a cheap reference-count bump; no
                // heap allocation per call.
                let tb_filename = self
                    .script_filename
                    .clone()
                    .unwrap_or_else(|| std::sync::Arc::from("<unknown>"));
                pyrust_core::push_traceback_frame(pyrust_core::FrameInfo {
                    filename: tb_filename,
                    lineno: None,
                    funcname: std::sync::Arc::from(function.name.as_str()),
                });
                let vm_result = self.run_bytecode_for_fn(&code, regs_slice, function.id);
                // Pop the frame, capturing the chain if an error occurred.
                // `pop_traceback_frame(true)` snapshots the stack (including this
                // frame) into CAPTURED_ERROR_FRAMES before removing this entry.
                pyrust_core::pop_traceback_frame(vm_result.is_err());
                self.vm_frame_views.pop();

                let used_env = std::mem::replace(&mut self.env, previous_env);
                if needs_local_env {
                    self.free_env(used_env);
                }
                let value = vm_result?;
                if let Some(ck) = cache_key {
                    self.fn_cache.insert(ck, value.clone());
                }
                return Ok(value);
            }

            // All user functions must have precompiled bytecode
            return Err(PyError::Runtime(format!("no bytecode for '{}'", function.name)));
        }

        // Variadic path: handle *args and **kwargs
        // Gather positional and keyword args
        let mut positional_vals: Vec<Value> = bound_prefix.to_vec();
        let mut keyword_vals: Vec<(String, Value)> = Vec::new();
        for arg in args {
            if let Some(name) = &arg.name {
                keyword_vals.push((name.clone(), arg.value.clone()));
            } else {
                positional_vals.push(arg.value.clone());
            }
        }

        let has_kwargs = function.params.iter().any(|p| p.is_kwargs);
        let mut consumed_keywords = std::collections::HashSet::new();
        let mut pos_idx = 0;
        let mut param_vals: Vec<Value> = Vec::with_capacity(function.params.len());

        for param in function.params.iter() {
            let value = if param.is_args {
                let rest = positional_vals[pos_idx..].to_vec();
                pos_idx = positional_vals.len();
                Value::tuple(rest)
            } else if param.is_kwargs {
                let mut dict: indexmap::IndexMap<crate::value::PyKey, Value> = indexmap::IndexMap::new();
                for (k, v) in &keyword_vals {
                    if !consumed_keywords.contains(k)
                        && let Some(key) = Value::string(k.clone()).to_key() {
                            dict.insert(key, v.clone());
                        }
                }
                Value::dict(dict)
            } else {
                let kw_pos = if param.is_positional_only {
                    None
                } else {
                    keyword_vals.iter().position(|(k, _)| k == &param.name)
                };
                if let Some(ki) = kw_pos {
                    consumed_keywords.insert(keyword_vals[ki].0.clone());
                    keyword_vals[ki].1.clone()
                } else if pos_idx < positional_vals.len() {
                    let v = positional_vals[pos_idx].clone();
                    pos_idx += 1;
                    v
                } else if let Some(d) = &param.default {
                    d.clone()
                } else if param.is_keyword_only {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "{}() missing 1 required keyword-only argument: '{}'",
                            function.name, param.name
                        ),
                    ));
                } else {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "{}() missing required argument: '{}'",
                            function.name, param.name
                        ),
                    ));
                }
            };
            param_vals.push(value);
        }

        if !has_kwargs {
            for (name, _) in &keyword_vals {
                if !consumed_keywords.contains(name) {
                    // Distinguish "this name matches a positional-only param"
                    // from "this name is completely unknown" — CPython raises
                    // a more specific TypeError in the former case.
                    if function
                        .params
                        .iter()
                        .any(|p| p.is_positional_only && &p.name == name)
                    {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "{}() got some positional-only arguments passed as keyword arguments: '{}'",
                                function.name, name
                            ),
                        ));
                    }
                    return Err(PyError::Runtime(format!(
                        "{}() got unexpected keyword argument '{}'",
                        function.name, name
                    )));
                }
            }
        }

        // Now run via VM (same as non-variadic Tier-0 path)
        if let Some(code) = self.get_or_compile_bytecode(&function) {
            let num_regs = code.num_regs as usize;
            let mut regs: RegsBuf = smallvec![Value::unset(); num_regs];

            // Bind non-cell params into register file using fastlocals slot indices.
            for (param, val) in function.params.iter().zip(param_vals.iter()) {
                if !code.cell_vars.contains(&param.name)
                    && let Some(&slot) = function.local_index.get(&param.name) {
                        if (slot as usize) >= num_regs {
                            return Err(PyError::named(
                                "SystemError",
                                format!(
                                    "parameter '{}' register index {} out of range (num_regs={})",
                                    param.name, slot, num_regs
                                ),
                            ));
                        }
                        regs[slot as usize] = val.clone();
                    }
            }
            // Self-reference for recursive calls (only if not a cell var).
            if !code.cell_vars.contains(&function.name)
                && let Some(&slot) = function.local_index.get(&function.name) {
                    if (slot as usize) >= num_regs {
                        return Err(PyError::named(
                            "SystemError",
                            format!(
                                "self-reference register index {} out of range (num_regs={})",
                                slot, num_regs
                            ),
                        ));
                    }
                    regs[slot as usize] = Value::user_function(Rc::clone(&function));
                }

            let _depth_guard = CallDepthGuard::enter();
            if call_depth() > MAX_CALL_DEPTH {
                let exc = if let Some(cls) = self.exc_classes.get("RecursionError") {
                    instantiate_exception(
                        cls,
                        vec![Value::string("maximum recursion depth exceeded")],
                    )
                } else {
                    self.instantiate_named_exception(
                        "RecursionError",
                        "maximum recursion depth exceeded".to_string(),
                    )?
                };
                return Err(PyError::Raised(exc));
            }

            // Create a local env when the function uses globals, nonlocals, or cell vars.
            let needs_local_env = !function.global_names.is_empty()
                || !function.nonlocal_names.is_empty()
                || !code.cell_vars.is_empty();

            let previous_env = if needs_local_env {
                let local_env = self.alloc_env(Some(Rc::clone(&function.env)));
                {
                    let mut e = local_env.borrow_mut();
                    e.local_names = Rc::clone(&function.local_names);
                    e.global_names = Rc::clone(&function.global_names);
                    e.nonlocal_names = Rc::clone(&function.nonlocal_names);
                    // Store cell var params in the env so inner closures can capture them.
                    for (param, val) in function.params.iter().zip(param_vals.iter()) {
                        if code.cell_vars.contains(&param.name) {
                            e.values.insert(param.name.clone(), val.clone());
                        }
                    }
                }
                std::mem::replace(&mut self.env, local_env)
            } else {
                std::mem::replace(&mut self.env, Rc::clone(&function.env))
            };

            // Issue #488: variadic generator functions (`def g(*args):
            // yield ...` and friends) must also be wrapped in a
            // GeneratorFrame instead of executed synchronously — the
            // simple-path branch already does this above; mirror it here
            // so the body's `yield` isn't observed as a runtime error.
            if code.is_generator {
                let gen_env = std::mem::replace(&mut self.env, previous_env);
                return Ok(Self::build_generator_value(
                    &code,
                    regs,
                    gen_env,
                    Rc::clone(&function.local_index),
                    std::sync::Arc::from(function.name.as_str()),
                ));
            }

            // Issue #389: publish a function frame view (see the
            // matching push in the simple-path branch above).
            // Issue #486: nonlocal_names + env for nonlocal resolution.
            let nonlocal_names_opt = if function.nonlocal_names.is_empty() {
                None
            } else {
                Some(Rc::clone(&function.nonlocal_names))
            };
            let env_opt = if function.nonlocal_names.is_empty() {
                None
            } else {
                Some(Rc::clone(&self.env))
            };
            // Capture the raw pointer and length BEFORE constructing RegSlice
            // so both the VmFrameView and the dispatch loop share the same raw
            // pointer with no &mut [Value] in scope (issue #547 / PR #646).
            let regs_ptr = unsafe {
                std::ptr::NonNull::new_unchecked(regs.as_mut_ptr())
            };
            let regs_len = regs.len();
            self.vm_frame_views.push(VmFrameView {
                kind: FrameKind::Function,
                // SAFETY: SmallVec / Vec allocation is always non-null.
                // Popped before `regs` is dropped (see above).
                regs_ptr,
                regs_len,
                local_index: Rc::clone(&function.local_index),
                nonlocal_names: nonlocal_names_opt,
                env: env_opt,
                is_class_method: code.is_class_method,
            });
            // SAFETY: regs_ptr is valid for regs_len Values for the lifetime
            // of `regs` (a local RegsBuf that outlives this call).  No
            // &mut [Value] referencing `regs` is held while the dispatch loop
            // runs; RegSlice (raw pointer + len) is used instead, removing
            // the LLVM noalias constraint (issue #547).
            let regs_slice = unsafe { RegSlice::from_raw(regs_ptr.as_ptr(), regs_len) };
            // Push a traceback frame so errors propagating out of this
            // function body carry the correct `File / in <name>` entry.
            // Cloning an `Arc<str>` is a cheap reference-count bump; no
            // heap allocation per call.
            let tb_filename = self
                .script_filename
                .clone()
                .unwrap_or_else(|| std::sync::Arc::from("<unknown>"));
            pyrust_core::push_traceback_frame(pyrust_core::FrameInfo {
                filename: tb_filename,
                lineno: None,
                funcname: std::sync::Arc::from(function.name.as_str()),
            });
            let vm_result = self.run_bytecode_for_fn(&code, regs_slice, function.id);
            // Pop the frame, capturing the chain if an error occurred.
            pyrust_core::pop_traceback_frame(vm_result.is_err());
            self.vm_frame_views.pop();

            let used_env = std::mem::replace(&mut self.env, previous_env);
            if needs_local_env {
                self.free_env(used_env);
            }
            return vm_result;
        }

        // All user functions must have precompiled bytecode
        Err(PyError::Runtime(format!("no bytecode for '{}'", function.name)))
    }

    fn call_class_expanded(
        &mut self,
        class: Rc<RefCell<PyClass>>,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        if is_exception_class(&class) {
            reject_keyword_args_expanded(&class.borrow().name, args)?;
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                values.push(arg.value.clone());
            }
            // CPython 3.12 SyntaxError.__init__ validates args[1] if present:
            // it must be an iterable that yields exactly 4 or 6 elements.
            // Non-iterables raise TypeError; the wrong number raises TypeError.
            if class_chain_contains_name(&class, "SyntaxError") && values.len() >= 2 {
                let second = &values[1];
                let items_opt: Option<Vec<Value>> = second
                    .as_tuple()
                    .map(|s| s.to_vec())
                    .or_else(|| second.as_list().map(|s| s.to_vec()));
                match items_opt {
                    None => {
                        // args[1] is not a sequence — CPython raises TypeError
                        return Err(PyError::named(
                            "TypeError",
                            &format!(
                                "'{}' object is not iterable",
                                pyrust_core::builtin_type_name(second)
                            ),
                        ));
                    }
                    Some(ref items) if items.len() < 4 => {
                        return Err(PyError::named(
                            "TypeError",
                            &format!(
                                "function takes at least 4 arguments ({} given)",
                                items.len()
                            ),
                        ));
                    }
                    Some(ref items) if items.len() == 5 => {
                        return Err(PyError::named(
                            "TypeError",
                            "end_offset must be provided when end_lineno is provided",
                        ));
                    }
                    Some(ref items) if items.len() > 6 => {
                        return Err(PyError::named(
                            "TypeError",
                            &format!(
                                "function takes at most 6 arguments ({} given)",
                                items.len()
                            ),
                        ));
                    }
                    _ => {}
                }
            }
            return Ok(instantiate_exception(class, values));
        }

        // Primitive classes never reach this fn — the `PyClass` arm in
        // `call_function_expanded` short-circuits them via
        // `PRIMITIVE_CLASS_DISPATCH` (issue #462).  Subclasses of
        // primitives (`class S(int): pass`) DO reach here but without an
        // inherited `__init__` (helpers.rs deliberately leaves primitive
        // class attrs empty so the BuiltinFunction constructor isn't
        // exposed to PyInstance-based subclass dispatch — see #463
        // Copilot review).  They land in the `None` arm of the
        // init match below.

        // `__new__` protocol: if the class declares a `__new__` builtin in
        // its OWN attrs (not inherited — CPython's `object.__new__` is the
        // root and we don't emulate the full MRO here), call it as a class
        // method with `cls` as the first argument.  The return value is the
        // new instance; if it is a `PyInstance` whose class is a subclass of
        // `cls`, `__init__` is called on it (CPython parity).  This is the
        // mechanism that lets `pathlib.Path.__new__` return a `PosixPath`
        // instance on POSIX platforms (issue #922).
        let own_new = class.borrow().attrs.get("__new__").cloned();
        if let Some(new_val) = own_new {
            if let ValueKind::BuiltinFunction(name) = new_val.kind() {
                let dispatch = crate::builtin_registry::lookup(name).ok_or_else(|| {
                    PyError::Runtime(format!(
                        "internal: __new__ builtin '{name}' not in registry"
                    ))
                })?;
                // Pass cls as args[0], user args follow.
                let mut combined: Vec<ExpandedCallArg> = Vec::with_capacity(args.len() + 1);
                combined.push(ExpandedCallArg {
                    name: None,
                    value: Value::py_class(Rc::clone(&class)),
                });
                combined.extend(args.iter().cloned());
                let new_result = dispatch(self, &combined)?;

                // After `__new__` succeeds, call `__init__` on the result if it
                // is a PyInstance whose class is equal to or a subclass of `cls`.
                if let ValueKind::PyInstance(inst_rc) = new_result.kind() {
                    let inst_class = inst_rc.borrow().class.clone();
                    if class_is_subclass_of(&inst_class, &class) {
                        let init = lookup_class_attr(&inst_class, "__init__");
                        if let Some(init_val) = init {
                            if matches!(
                                init_val.kind(),
                                ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_)
                            ) {
                                let result = invoke_class_method(
                                    self,
                                    init_val,
                                    Value::py_instance(Rc::clone(&inst_rc)),
                                    args,
                                )?;
                                if !result.is_none() {
                                    return Err(PyError::Runtime(
                                        "__init__() should return None".to_string(),
                                    ));
                                }
                            }
                        }
                    }
                }
                return Ok(new_result);
            }
        }

        let instance = Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(&class),
            attrs: IndexMap::new(),
        }));

        // Issue #976: if this class inherits from dict/list/set, pre-initialise
        // an empty backing store on the instance *before* calling __init__.
        // This ensures that `self[k] = v` (or any other subscript / method
        // call on `self`) inside a user-defined __init__ sees a valid
        // __builtin_data__ entry to delegate to.  When there is no __init__,
        // the None arm below calls the primitive constructor with the user's
        // args to populate the backing value.
        let prim_base = find_mutable_primitive_base(&class);
        if let Some(prim_name) = prim_base {
            if let Some(dispatch) = crate::builtin_registry::lookup(prim_name) {
                // Empty args → empty primitive (dict/list/set with no content).
                let backing = dispatch(self, &[])?;
                instance
                    .borrow_mut()
                    .attrs
                    .insert(BUILTIN_DATA_ATTR.to_string(), backing);
            }
        }

        // Issue #994: if this class inherits from frozenset/tuple (immutable
        // primitives), build the backing from the constructor args immediately.
        // Unlike mutable types, there is no empty pre-initialisation step —
        // the content is fixed at construction and __init__ cannot change it.
        let immutable_prim_base = find_immutable_primitive_base(&class);
        if let Some(prim_name) = immutable_prim_base {
            if let Some(dispatch) = crate::builtin_registry::lookup(prim_name) {
                let backing = dispatch(self, args)?;
                instance
                    .borrow_mut()
                    .attrs
                    .insert(BUILTIN_DATA_ATTR.to_string(), backing);
            }
        }

        let init = lookup_class_attr(&class, "__init__");
        match init {
            Some(method_val)
                if matches!(
                    method_val.kind(),
                    ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_)
                ) =>
            {
                let result = invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(Rc::clone(&instance)),
                    args,
                )?;
                if !result.is_none() {
                    return Err(PyError::Runtime(
                        "__init__() should return None".to_string(),
                    ));
                }
            }
            Some(_) => {
                return Err(PyError::Runtime(
                    "__init__ attribute is not callable".to_string(),
                ));
            }
            None => {
                // No `__init__` in the MRO.  If the class inherits from a
                // mutable primitive (dict / list / set), call that base's
                // constructor with the provided args and store the result as
                // `__builtin_data__` so subscript / method dispatch can
                // delegate to the backing value (issue #976).
                // NOTE: the empty backing was already inserted above; replace it
                // with the args-populated value now.
                if let Some(prim_name) = prim_base {
                    if let Some(dispatch) = crate::builtin_registry::lookup(prim_name) {
                        let backing = dispatch(self, args)?;
                        instance
                            .borrow_mut()
                            .attrs
                            .insert(BUILTIN_DATA_ATTR.to_string(), backing);
                    }
                } else if immutable_prim_base.is_none() && !args.is_empty() {
                    let class_name = class.borrow().name.clone();
                    return Err(PyError::Runtime(format!(
                        "{}() takes no arguments",
                        class_name
                    )));
                }
            }
        }

        Ok(Value::py_instance(instance))
    }

    pub(crate) fn call_range_expanded(&mut self, args: &[ExpandedCallArg]) -> Result<Value> {
        reject_keyword_args_expanded("range", args)?;
        if args.is_empty() || args.len() > 3 {
            return Err(PyError::Runtime(
                "range expected 1 to 3 arguments".to_string(),
            ));
        }

        let mut ints = Vec::with_capacity(args.len());
        for arg in args {
            match arg.value.kind() {
                ValueKind::Int(v) => ints.push(v),
                ValueKind::Bool(b) => ints.push(b as i64),
                _ => {
                    return Err(PyError::Runtime(
                        "range arguments must be integers".to_string(),
                    ));
                }
            }
        }

        let (start, stop, step) = match ints.as_slice() {
            [stop] => (0, *stop, 1),
            [start, stop] => (*start, *stop, 1),
            [start, stop, step] => (*start, *stop, *step),
            _ => unreachable!("validated by length"),
        };

        if step == 0 {
            return Err(PyError::named(
                "ValueError",
                "range() arg 3 must not be zero".to_string(),
            ));
        }

        Ok(Value::range(start, stop, step))
    }

    /// Return the compiled `FnCode` for `function`.
    /// Returns `None` only if `precompiled_code` is absent.
    fn get_or_compile_bytecode(&mut self, function: &Rc<UserFunction>) -> Option<Rc<FnCode>> {
        function
            .precompiled_code
            .as_ref()
            .and_then(|rc| Rc::clone(rc).downcast::<FnCode>().ok())
    }

    /// Implements `str.format_map(mapping)`.  Parses `{name}` replacement
    /// fields and looks up each name via `mapping[name]` (`__getitem__`).
    ///
    /// Unlike `format_str_template`, positional fields (`{}` or `{0}`) raise
    /// `ValueError: Format string contains positional fields`, matching CPython
    /// 3.12.  Conversion (`!r`/`!s`/`!a`), format specs (`:…`), and field
    /// accessors (`.attr` / `[key]`) are supported identically to `format`.
    pub(crate) fn format_str_template_map(
        &mut self,
        template: &str,
        mapping: Value,
    ) -> Result<Value> {
        let bytes = template.as_bytes();
        let mut out = String::with_capacity(template.len());
        let mut i = 0;

        while i < bytes.len() {
            let c = bytes[i];
            if c == b'{' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                    out.push('{');
                    i += 2;
                    continue;
                }
                // Find matching '}'.
                let mut depth = 1;
                let mut j = i + 1;
                while j < bytes.len() && depth > 0 {
                    match bytes[j] {
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        _ => {}
                    }
                    if depth == 0 {
                        break;
                    }
                    j += 1;
                }
                if depth != 0 {
                    return Err(PyError::named(
                        "ValueError",
                        "Single '{' encountered in format string".to_string(),
                    ));
                }
                let field = &template[i + 1..j];
                i = j + 1;

                let (field_name_full, spec) = split_field_and_spec(field);
                let (field_name, conversion) = match field_name_full.rsplit_once('!') {
                    Some((name, conv)) if conv.len() == 1 => {
                        (name, Some(conv.chars().next().unwrap()))
                    }
                    _ => (field_name_full, None),
                };

                let (head, rest) = split_head_and_accessors(field_name);
                // format_map does not support positional fields.
                if head.is_empty() || head.parse::<usize>().is_ok() {
                    return Err(PyError::named(
                        "ValueError",
                        "Format string contains positional fields".to_string(),
                    ));
                }
                // Look up the named key in the mapping via __getitem__.
                let base =
                    self.eval_index(mapping.clone(), Value::string(head.to_string()))?;

                let value = apply_field_accessors(base, rest)?;
                let value = match conversion {
                    Some('r') => Value::string(value.repr()),
                    Some('s') => Value::string(value.to_py_str()),
                    Some('a') => Value::string(ascii_repr(&value)),
                    Some(c) => {
                        return Err(PyError::named(
                            "ValueError",
                            format!("Unknown conversion specifier {c}"),
                        ));
                    }
                    None => value,
                };
                let formatted = apply_format_spec(&value, spec)?;
                if let ValueKind::Str(s) = formatted.kind() {
                    out.push_str(s);
                } else {
                    out.push_str(&formatted.to_py_str());
                }
            } else if c == b'}' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'}' {
                    out.push('}');
                    i += 2;
                } else {
                    return Err(PyError::named(
                        "ValueError",
                        "Single '}' encountered in format string".to_string(),
                    ));
                }
            } else {
                let ch_start = i;
                i += 1;
                while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 {
                    i += 1;
                }
                out.push_str(&template[ch_start..i]);
            }
        }
        Ok(Value::string(out))
    }

    /// Resolve a single start/stop argument for `list.index` / `tuple.index`
    /// through the `__index__` protocol, matching CPython 3.12 semantics.
    ///
    /// - `Int` / `Bool` / `BigInt`: returned unchanged (`BigInt` is still `int`).
    /// - `PyInstance` with `__index__`: the method is called; its return value
    ///   must be `Int`, `Bool`, or `BigInt`, and is returned.
    /// - Anything else: `TypeError: slice indices must be integers or have an
    ///   __index__ method`.
    fn resolve_index_arg(&mut self, val: Value) -> Result<Value> {
        // Probe the kind in a scoped block so the Ref guard drops before we
        // may need to move `val`.
        enum Tag {
            Int,
            Instance(Rc<RefCell<PyInstance>>),
            Other,
        }
        let tag = match val.kind() {
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => Tag::Int,
            ValueKind::PyInstance(inst) => Tag::Instance(Rc::clone(inst)),
            _ => Tag::Other,
        };
        match tag {
            Tag::Int => Ok(val),
            Tag::Instance(inst_rc) => {
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__index__") {
                    let result = invoke_class_method(
                        self,
                        method_val,
                        Value::py_instance(Rc::clone(&inst_rc)),
                        &[],
                    )?;
                    // Check result kind in a scoped block to release the Ref.
                    // BigInt is a valid int result (e.g. __index__ returning 2**100).
                    let result_ok = matches!(
                        result.kind(),
                        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                    );
                    if result_ok {
                        Ok(result)
                    } else {
                        Err(PyError::named(
                            "TypeError",
                            format!(
                                "__index__ returned non-int (type {})",
                                value_type_name_str(&result),
                            ),
                        ))
                    }
                } else {
                    Err(PyError::named(
                        "TypeError",
                        "slice indices must be integers or have an __index__ method".to_string(),
                    ))
                }
            }
            Tag::Other => Err(PyError::named(
                "TypeError",
                "slice indices must be integers or have an __index__ method".to_string(),
            )),
        }
    }

    /// For `list.index` / `tuple.index`, resolve start (pos[1]) and stop
    /// (pos[2]) through `resolve_index_arg`.  pos[0] is the search target
    /// and is left unchanged.  Returns a new `Vec<Value>` with the resolved
    /// slice-boundary arguments in place.
    fn resolve_seq_index_pos(&mut self, mut pos: Vec<Value>) -> Result<Vec<Value>> {
        if pos.len() >= 2 {
            let start = pos.remove(1);
            let resolved = self.resolve_index_arg(start)?;
            pos.insert(1, resolved);
        }
        if pos.len() >= 3 {
            let stop = pos.remove(2);
            let resolved = self.resolve_index_arg(stop)?;
            pos.insert(2, resolved);
        }
        Ok(pos)
    }

    /// Resolve an index argument for **sequence item access** (`a[i]`, `a[i] = v`,
    /// `del a[i]`) through the `__index__` protocol, matching CPython 3.12.
    ///
    /// Differs from `resolve_index_arg` (used for slice bounds) only in the
    /// error message when the object provides neither an integer type nor
    /// `__index__`.  CPython's per-type messages:
    /// - list/tuple/bytes: `"X indices must be integers or slices, not Y"`
    /// - string: `"string indices must be integers, not 'Y'"` (different!)
    ///
    /// - `Int` / `Bool` / `BigInt`: returned unchanged.
    /// - `PyInstance` with `__index__`: called; result must be `Int`/`Bool`/`BigInt`.
    /// - `PyInstance` without `__index__` / any other type: `TypeError` with the
    ///   label-specific message.
    pub(crate) fn call_index_protocol(&mut self, val: Value, label: &str) -> Result<Value> {
        enum Tag {
            Int,
            Instance(Rc<RefCell<PyInstance>>),
            Other,
        }
        let tag = match val.kind() {
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => Tag::Int,
            ValueKind::PyInstance(inst) => Tag::Instance(Rc::clone(inst)),
            _ => Tag::Other,
        };
        match tag {
            Tag::Int => Ok(val),
            Tag::Instance(inst_rc) => {
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__index__") {
                    let result = invoke_class_method(
                        self,
                        method_val,
                        Value::py_instance(Rc::clone(&inst_rc)),
                        &[],
                    )?;
                    let result_ok = matches!(
                        result.kind(),
                        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                    );
                    if result_ok {
                        Ok(result)
                    } else {
                        Err(PyError::named(
                            "TypeError",
                            format!(
                                "__index__ returned non-int (type {})",
                                value_type_name_str(&result),
                            ),
                        ))
                    }
                } else {
                    let type_name = value_type_name_str(&Value::py_instance(inst_rc));
                    Err(PyError::named(
                        "TypeError",
                        seq_index_type_error(label, &type_name),
                    ))
                }
            }
            Tag::Other => {
                let type_name = value_type_name_str(&val);
                Err(PyError::named(
                    "TypeError",
                    seq_index_type_error(label, &type_name),
                ))
            }
        }
    }
}

/// Apply a Python format spec string to a `Value` and return the formatted string.
///
/// Implements the [Python format-spec mini-language][docs]:
///
/// ```text
/// [[fill]align][sign][#][0][width][grouping][.precision][type]
/// ```
///
/// Supported components for the built-in numeric / string types:
/// - **fill / align** (`<`, `>`, `^`, `=`) with any single fill character
/// - **sign** (`+`, `-`, ` `)
/// - **alternate form** `#` for `b`, `o`, `x`, `X` (and float types)
/// - **zero-pad** `0` (implies sign-aware `=` alignment)
/// - **width** (decimal integer)
/// - **grouping** `,` (comma) or `_` (underscore)
/// - **precision** `.N` for floats and strings
/// - **type** `b`, `c`, `d`, `e`, `E`, `f`, `F`, `g`, `G`, `n`, `o`, `s`, `x`, `X`, `%`
///
/// Complex values support the bare width / fill / align spec (matching
/// CPython's `format(1+2j, ">10")` -> `"    (1+2j)"`) but do not yet accept
/// numeric type codes (`e`/`f`/`g`) or sign / precision / grouping / `#` /
/// `0` — those will raise ValueError.
///
/// Not yet implemented: locale-aware grouping (`n` and float `n` types),
/// Complex with explicit numeric type codes, and non-ASCII fill characters
/// in nested f-string specs round-trip through `str.format` as bytes rather
/// than chars.  These gaps mirror documented pyrust limitations.
///
/// [docs]: https://docs.python.org/3/library/string.html#format-specification-mini-language
pub(crate) fn apply_format_spec(value: &Value, spec: &str) -> Result<Value> {
    if spec.is_empty() {
        return Ok(Value::string(value.to_py_str()));
    }

    let parsed = parse_format_spec(spec)?;
    let formatted = render_format_spec(value, &parsed)?;
    Ok(Value::string(formatted))
}

#[derive(Debug, Clone)]
struct FormatSpec {
    fill: char,
    align: Option<char>,
    /// True when the user explicitly supplied a fill character (the
    /// two-character `[fill]align` form).  When false, a subsequent `0`
    /// flag promotes the fill to `'0'`.
    fill_explicit: bool,
    sign: Option<char>,
    alt: bool,
    zero_pad: bool,
    width: usize,
    grouping: Option<char>,
    precision: Option<usize>,
    type_char: Option<char>,
}

fn parse_format_spec(spec: &str) -> Result<FormatSpec> {
    let chars: Vec<char> = spec.chars().collect();
    let len = chars.len();
    let mut pos = 0;

    // fill + align: the align character (one of <>=^) must be at index 1
    // when a fill is present.  A bare align character at index 0 means
    // fill defaults to space.  '{' and '}' are not legal fill characters
    // (they would terminate the replacement field) — guard explicitly.
    let (fill, align, fill_explicit) =
        if len >= 2 && matches!(chars[1], '<' | '>' | '=' | '^') && !matches!(chars[0], '{' | '}') {
            let f = chars[0];
            let a = chars[1];
            pos += 2;
            (f, Some(a), true)
        } else if len >= 1 && matches!(chars[0], '<' | '>' | '=' | '^') {
            let a = chars[0];
            pos += 1;
            (' ', Some(a), false)
        } else {
            (' ', None, false)
        };

    // sign
    let sign = if pos < len && matches!(chars[pos], '+' | '-' | ' ') {
        let s = chars[pos];
        pos += 1;
        Some(s)
    } else {
        None
    };

    // alternate form '#'
    let alt = if pos < len && chars[pos] == '#' {
        pos += 1;
        true
    } else {
        false
    };

    // zero-padding '0' — always consumed when present at this position.
    // Semantics depend on whether align/fill were explicit (see render).
    let zero_pad = if pos < len && chars[pos] == '0' {
        pos += 1;
        true
    } else {
        false
    };

    // width
    let width_start = pos;
    while pos < len && chars[pos].is_ascii_digit() {
        pos += 1;
    }
    let width: usize = if pos > width_start {
        let raw: String = chars[width_start..pos].iter().collect();
        raw.parse::<usize>().map_err(|_| {
            PyError::named(
                "ValueError",
                "Too many decimal digits in format string".to_string(),
            )
        })?
    } else {
        0
    };

    // grouping option (',' or '_') — sits between width and precision.
    let grouping = if pos < len && (chars[pos] == ',' || chars[pos] == '_') {
        let g = chars[pos];
        pos += 1;
        Some(g)
    } else {
        None
    };

    // precision
    let precision = if pos < len && chars[pos] == '.' {
        pos += 1;
        let prec_start = pos;
        while pos < len && chars[pos].is_ascii_digit() {
            pos += 1;
        }
        if pos > prec_start {
            let raw: String = chars[prec_start..pos].iter().collect();
            Some(raw.parse::<usize>().map_err(|_| {
                PyError::named(
                    "ValueError",
                    "Too many decimal digits in format string".to_string(),
                )
            })?)
        } else {
            // '.' with no digits is a syntax error in CPython.
            return Err(PyError::named(
                "ValueError",
                "Format specifier missing precision".to_string(),
            ));
        }
    } else {
        None
    };

    // type char (must be the last character if present)
    let type_char = if pos < len {
        let t = chars[pos];
        pos += 1;
        Some(t)
    } else {
        None
    };

    if pos != len {
        return Err(PyError::named(
            "ValueError",
            "Invalid format specifier".to_string(),
        ));
    }

    Ok(FormatSpec {
        fill,
        align,
        fill_explicit,
        sign,
        alt,
        zero_pad,
        width,
        grouping,
        precision,
        type_char,
    })
}

/// Apply the parsed spec to the value.  Splits into a string-typed branch
/// and a numeric branch so type-specific validation stays close to formatting.
fn render_format_spec(value: &Value, fs: &FormatSpec) -> Result<String> {
    // Treat the value as a string when the type code is 's' (or absent and
    // the value is a string).  For non-string values with no type code, fall
    // back to numeric handling so width / zero-pad / sign still apply.
    let is_string_target = matches!(fs.type_char, Some('s'))
        || (fs.type_char.is_none() && matches!(value.kind(), ValueKind::Str(_)));

    if is_string_target {
        return format_as_string(value, fs);
    }

    // No type code and a non-string value: route by value kind.
    if fs.type_char.is_none() {
        match value.kind() {
            ValueKind::Int(_) | ValueKind::Bool(_) => return format_int_value(value, fs, None),
            ValueKind::Float(_) => return format_float_value(value, fs, None),
            // Complex with no explicit type code: render via complex_repr
            // (matching CPython's `format(1+2j)` -> "(1+2j)") and then apply
            // width / align / fill to the resulting string.  The float
            // format codes are rejected for Complex in format_float_value,
            // so we must short-circuit here for the bare-spec case.
            ValueKind::Complex(_, _) => return format_complex_value(value, fs),
            _ => {
                // Anything else: fall back to str() then pad like a string.
                return format_as_string(value, fs);
            }
        }
    }

    let t = fs.type_char.unwrap();
    match t {
        'd' | 'b' | 'o' | 'x' | 'X' | 'c' | 'n' => format_int_value(value, fs, Some(t)),
        'e' | 'E' | 'f' | 'F' | 'g' | 'G' | '%' => format_float_value(value, fs, Some(t)),
        's' => format_as_string(value, fs),
        _ => Err(PyError::named(
            "ValueError",
            format!(
                "Unknown format code '{t}' for object of type '{}'",
                value_type_name_str(value)
            ),
        )),
    }
}

fn format_as_string(value: &Value, fs: &FormatSpec) -> Result<String> {
    // Reject numeric-only options on strings, matching CPython.
    if matches!(fs.type_char, Some('s')) && !matches!(value.kind(), ValueKind::Str(_)) {
        return Err(PyError::named(
            "ValueError",
            format!(
                "Unknown format code 's' for object of type '{}'",
                value_type_name_str(value)
            ),
        ));
    }
    if fs.sign.is_some() {
        return Err(PyError::named(
            "ValueError",
            "Sign not allowed in string format specifier".to_string(),
        ));
    }
    if fs.alt {
        return Err(PyError::named(
            "ValueError",
            "Alternate form (#) not allowed in string format specifier".to_string(),
        ));
    }
    if fs.grouping.is_some() {
        return Err(PyError::named(
            "ValueError",
            "Cannot specify ',' or '_' with 's'.".to_string(),
        ));
    }
    if matches!(fs.align, Some('=')) {
        return Err(PyError::named(
            "ValueError",
            "'=' alignment not allowed in string format specifier".to_string(),
        ));
    }

    let raw = match value.kind() {
        ValueKind::Str(s) => s.to_string(),
        _ => value.to_py_str(),
    };
    let raw = match fs.precision {
        Some(p) => raw.chars().take(p).collect::<String>(),
        None => raw,
    };
    // CPython accepts the `0` zero-pad flag on strings (with a
    // DeprecationWarning) — it promotes the fill character to '0' but keeps
    // the default left-alignment.  We just match the run-time behavior.
    let effective_fill = if fs.zero_pad && !fs.fill_explicit {
        '0'
    } else {
        fs.fill
    };
    Ok(pad_value(&raw, fs, '<', effective_fill))
}

/// Produce the digit-only "body" of an integer formatting (no sign / prefix).
fn int_body(magnitude: u64, type_char: char) -> String {
    match type_char {
        'd' => format!("{magnitude}"),
        'b' => format!("{magnitude:b}"),
        'o' => format!("{magnitude:o}"),
        'x' => format!("{magnitude:x}"),
        'X' => format!("{magnitude:X}"),
        _ => format!("{magnitude}"),
    }
}

fn prefix_for(type_char: char, alt: bool) -> &'static str {
    if !alt {
        return "";
    }
    match type_char {
        'b' => "0b",
        'o' => "0o",
        'x' => "0x",
        'X' => "0X",
        _ => "",
    }
}

fn format_int_value(value: &Value, fs: &FormatSpec, type_char: Option<char>) -> Result<String> {
    let n: i128 = match value.kind() {
        ValueKind::Int(n) => n as i128,
        ValueKind::Bool(b) => {
            if b {
                1
            } else {
                0
            }
        }
        _ => {
            let code = type_char.unwrap_or('d');
            return Err(PyError::named(
                "ValueError",
                format!(
                    "Unknown format code '{code}' for object of type '{}'",
                    value_type_name_str(value)
                ),
            ));
        }
    };

    if fs.precision.is_some() {
        return Err(PyError::named(
            "ValueError",
            "Precision not allowed in integer format specifier".to_string(),
        ));
    }

    let t = type_char.unwrap_or('d');

    // 'c': render as the unicode character.
    if t == 'c' {
        if fs.sign.is_some() || fs.alt || fs.grouping.is_some() {
            return Err(PyError::named(
                "ValueError",
                "Cannot specify ',' or '_', sign, or '#' with 'c'.".to_string(),
            ));
        }
        if n < 0 || n > 0x10FFFF {
            return Err(PyError::named(
                "OverflowError",
                "%c arg not in range(0x110000)".to_string(),
            ));
        }
        let ch = char::from_u32(n as u32).ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "%c arg not in range(0x110000)".to_string(),
            )
        })?;
        let raw = ch.to_string();
        return Ok(pad_value(&raw, fs, '<', fs.fill));
    }

    // 'n' = same as 'd' for now (no locale-aware grouping).
    let effective_t = if t == 'n' { 'd' } else { t };

    // Validate grouping vs type.
    if let Some(g) = fs.grouping {
        let ok = match (g, effective_t) {
            (',', 'd') => true,
            (',', _) => false,
            ('_', 'd' | 'b' | 'o' | 'x' | 'X') => true,
            _ => false,
        };
        if !ok {
            return Err(PyError::named(
                "ValueError",
                format!("Cannot specify '{g}' with '{effective_t}'."),
            ));
        }
    }

    let negative = n < 0;
    let magnitude: u64 = if negative {
        // i64::MIN edge case: -(i128) fits in u64 via wrap.
        (-n) as u64
    } else {
        n as u64
    };

    let sign_prefix = sign_prefix_for(negative, fs.sign);
    let alt_prefix = prefix_for(effective_t, fs.alt);
    let mut body = int_body(magnitude, effective_t);

    // Apply grouping to the digit body.  For non-decimal bases (b/o/x/X),
    // CPython groups every 4 digits with '_'.  For decimal, every 3 digits
    // with either ',' or '_'.
    let group_size = if effective_t == 'd' { 3 } else { 4 };
    if let Some(g) = fs.grouping {
        body = group_digits(&body, g, group_size);
    }

    // Apply zero-pad / width / alignment.  Pass `group_size` so that
    // zero-pad + grouping with non-decimal bases (e.g. `{:0_12x}`) re-groups
    // the zero-padded body every 4 digits rather than every 3.
    Ok(assemble_numeric(
        sign_prefix,
        alt_prefix,
        body,
        fs,
        // Numeric default alignment is right.
        '>',
        group_size,
    ))
}

fn format_float_value(value: &Value, fs: &FormatSpec, type_char: Option<char>) -> Result<String> {
    // Complex numbers don't yet support the explicit float / int type codes
    // here.  The bare-spec (no type char) path routes Complex through
    // `format_complex_value` before reaching this function, so a Complex
    // value here means the user supplied an unsupported type code.
    if matches!(value.kind(), ValueKind::Complex(_, _)) {
        let code = type_char.unwrap_or('\0');
        return Err(PyError::named(
            "ValueError",
            format!("Unknown format code '{code}' for object of type 'complex'"),
        ));
    }

    let f = fmt_value_to_float(value)?;
    let t = type_char.unwrap_or('\0'); // '\0' = no type, use shortest repr-ish

    let negative = f.is_sign_negative() && !f.is_nan();
    let sign_prefix = sign_prefix_for(negative, fs.sign);
    let abs_f = f.abs();

    // Special values: inf / nan ignore precision / alt / grouping.
    if f.is_nan() {
        let body = if matches!(t, 'F' | 'G' | 'E') {
            "NAN".to_string()
        } else {
            "nan".to_string()
        };
        return Ok(assemble_numeric("", "", body, fs, '>', 3));
    }
    if f.is_infinite() {
        let body = if matches!(t, 'F' | 'G' | 'E') {
            "INF".to_string()
        } else {
            "inf".to_string()
        };
        return Ok(assemble_numeric(sign_prefix, "", body, fs, '>', 3));
    }

    // Validate grouping vs type.  Comma allowed on all float types; '_'
    // similarly per CPython.
    let (mut body, alt_prefix) = match t {
        'f' | 'F' => {
            let prec = fs.precision.unwrap_or(6);
            let s = format!("{:.prec$}", abs_f);
            let s = if t == 'F' { s.to_uppercase() } else { s };
            (ensure_alt_float(s, fs.alt, fs.precision), "")
        }
        'e' | 'E' => {
            let prec = fs.precision.unwrap_or(6);
            let s = if t == 'E' {
                format!("{:.prec$E}", abs_f)
            } else {
                format!("{:.prec$e}", abs_f)
            };
            let s = normalise_exp_digits(s);
            (ensure_alt_float(s, fs.alt, fs.precision), "")
        }
        'g' | 'G' => {
            let prec = fs.precision.unwrap_or(6);
            let prec = if prec == 0 { 1 } else { prec };
            let s = format_g(abs_f, prec, t == 'G');
            // Alternate '#': keep trailing zeros / decimal point.
            let s = if fs.alt {
                ensure_g_trailing_zeros(s, prec, t == 'G', abs_f)
            } else {
                s
            };
            (s, "")
        }
        '%' => {
            let prec = fs.precision.unwrap_or(6);
            let s = format!("{:.prec$}", abs_f * 100.0);
            let s = ensure_alt_float(s, fs.alt, fs.precision);
            (format!("{s}%"), "")
        }
        _ => {
            // No type: like 'g' but with at least one digit after the decimal
            // point and a shortest-roundtrip-ish repr.  We approximate by
            // calling Python's default via `to_py_str` for the magnitude.
            let s = match value.kind() {
                ValueKind::Float(_) => {
                    let raw = Value::float(abs_f).to_py_str();
                    if !raw.contains('.') && !raw.contains('e') && !raw.contains('n') {
                        format!("{raw}.0")
                    } else {
                        raw
                    }
                }
                ValueKind::Int(n) => {
                    let n = if n < 0 { -n } else { n };
                    format!("{n}.0")
                }
                ValueKind::Bool(b) => if b { "1.0" } else { "0.0" }.to_string(),
                _ => format!("{abs_f}"),
            };
            (s, "")
        }
    };

    // Apply grouping on the integer part of the float body.
    if let Some(g) = fs.grouping {
        if g == ',' || g == '_' {
            body = group_float_int_part(&body, g);
        }
    }

    Ok(assemble_numeric(sign_prefix, alt_prefix, body, fs, '>', 3))
}

/// Format a Complex value when no explicit numeric type code was given.
///
/// CPython's `format(1+2j)` returns `"(1+2j)"` and applying width / align /
/// fill (e.g. `format(1+2j, ">10")` -> `"    (1+2j)"`) pads that string.
///
/// This handles only the bare-spec case (fill / align / width); full Complex
/// formatting with type codes (`e`, `f`, `g`) is not yet implemented and
/// would be a larger feature.  Zero-padding and `=` alignment are rejected
/// because the leading `(` makes them ill-defined for Complex.
fn format_complex_value(value: &Value, fs: &FormatSpec) -> Result<String> {
    if fs.zero_pad && !fs.fill_explicit {
        return Err(PyError::named(
            "ValueError",
            "Zero padding is not allowed in complex format specifier".to_string(),
        ));
    }
    if matches!(fs.align, Some('=')) {
        return Err(PyError::named(
            "ValueError",
            "'=' alignment flag is not allowed in complex format specifier".to_string(),
        ));
    }
    // Sign / precision / grouping / alt with no type code would require
    // re-rendering the components; not supported here.  Reject explicitly so
    // the user gets a clear error instead of silently dropping the flag.
    if fs.sign.is_some()
        || fs.alt
        || fs.precision.is_some()
        || fs.grouping.is_some()
    {
        return Err(PyError::named(
            "ValueError",
            "Format specifier missing precision".to_string(),
        ));
    }

    // Use the canonical Complex repr (mirrors CPython's `format(c)`).
    let raw = value.to_py_str();
    // CPython right-aligns Complex on width (numeric default), matching the
    // behavior of the bare format spec.
    Ok(pad_value(&raw, fs, '>', fs.fill))
}

fn sign_prefix_for(negative: bool, sign: Option<char>) -> &'static str {
    if negative {
        return "-";
    }
    match sign {
        Some('+') => "+",
        Some(' ') => " ",
        _ => "",
    }
}

/// Insert grouping characters into a digit string (right-to-left).
fn group_digits(digits: &str, sep: char, group_size: usize) -> String {
    let bytes: Vec<char> = digits.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(bytes.len() + bytes.len() / group_size);
    for (i, c) in bytes.iter().rev().enumerate() {
        if i > 0 && i % group_size == 0 {
            out.push(sep);
        }
        out.push(*c);
    }
    out.iter().rev().collect()
}

/// Apply decimal grouping to the integer part of a float body (e.g. "1234.50"
/// → "1,234.50").  Leaves any exponent / suffix portion intact.
fn group_float_int_part(body: &str, sep: char) -> String {
    // Find the integer portion: up to the first '.' or 'e' / 'E' or '%'.
    let mut end = body.len();
    for (i, c) in body.char_indices() {
        if matches!(c, '.' | 'e' | 'E' | '%') {
            end = i;
            break;
        }
    }
    let (int_part, rest) = body.split_at(end);
    let grouped = group_digits(int_part, sep, 3);
    format!("{grouped}{rest}")
}

/// Assemble the final string with sign / alt-prefix / body and apply width
/// + alignment + zero-pad rules.
///
/// `group_size` controls how zero-pad interleaves with the grouping
/// separator: `3` for decimal / float grouping, `4` for `_` grouping with
/// non-decimal integer bases (`b`/`o`/`x`/`X`), matching CPython.
fn assemble_numeric(
    sign_prefix: &str,
    alt_prefix: &str,
    body: String,
    fs: &FormatSpec,
    default_align: char,
    group_size: usize,
) -> String {
    let raw_len = sign_prefix.chars().count() + alt_prefix.chars().count() + body.chars().count();

    // Determine effective alignment.  If zero-pad is set and no explicit
    // align was given, alignment becomes '=' (pad between sign/prefix and
    // digits).
    let effective_align = if let Some(a) = fs.align {
        a
    } else if fs.zero_pad {
        '='
    } else {
        default_align
    };
    // Zero-pad promotes fill to '0' unless the user explicitly supplied a
    // fill character via the two-character `[fill]align` form.
    let effective_fill = if fs.zero_pad && !fs.fill_explicit {
        '0'
    } else {
        fs.fill
    };

    if fs.width == 0 || raw_len >= fs.width {
        return format!("{sign_prefix}{alt_prefix}{body}");
    }
    let pad = fs.width - raw_len;
    let fill_str: String = std::iter::repeat(effective_fill).take(pad).collect();

    match effective_align {
        '=' => {
            // sign + prefix + fill + body
            let body_grouped = if effective_fill == '0' && fs.grouping.is_some() {
                // CPython interleaves the grouping separator with the zero
                // pad characters so the resulting body still groups in
                // threes-or-fours.  Apply by left-padding the body with
                // zeros first, then re-grouping the integer portion.
                regroup_with_zero_pad(&body, pad, fs.grouping.unwrap(), group_size, alt_prefix)
            } else {
                let mut s = String::with_capacity(pad + body.len());
                s.push_str(&fill_str);
                s.push_str(&body);
                s
            };
            format!("{sign_prefix}{alt_prefix}{body_grouped}")
        }
        '>' => format!("{fill_str}{sign_prefix}{alt_prefix}{body}"),
        '<' => format!("{sign_prefix}{alt_prefix}{body}{fill_str}"),
        '^' => {
            let left = pad / 2;
            let right = pad - left;
            let left_fill: String = std::iter::repeat(effective_fill).take(left).collect();
            let right_fill: String = std::iter::repeat(effective_fill).take(right).collect();
            format!("{left_fill}{sign_prefix}{alt_prefix}{body}{right_fill}")
        }
        _ => format!("{sign_prefix}{alt_prefix}{body}"),
    }
}

/// When zero-padding combines with thousands grouping (e.g. `{-12345:08,}` ->
/// `-012,345`), CPython expands the integer portion with zeros then regroups
/// so the leading zeros are themselves separated by the group char.  The
/// final grouped string must be at least `current_int_len + pad` characters
/// long.
///
/// `group_size` is `3` for decimal grouping (`,` or `_` with `d`/`n`/no type
/// or with floats), and `4` for `_` grouping with non-decimal integer bases
/// (`b`/`o`/`x`/`X`), matching CPython's rules.
fn regroup_with_zero_pad(
    body: &str,
    pad: usize,
    sep: char,
    group_size: usize,
    _alt_prefix: &str,
) -> String {
    // Split body into integer / fractional / suffix parts.  For non-decimal
    // integer bases (group_size == 4) the body is hex/oct/bin digits only —
    // including `e`, which is a legitimate hex digit — so skip the split.
    // For decimal / float (group_size == 3) the body may contain `.`, `e`,
    // `E`, or `%` which mark the end of the integer portion.
    let (int_part, rest) = if group_size == 3 {
        match body.find(|c: char| matches!(c, '.' | 'e' | 'E' | '%')) {
            Some(i) => (&body[..i], &body[i..]),
            None => (body, ""),
        }
    } else {
        (body, "")
    };

    // Strip existing separators from the integer part.
    let bare_int: String = int_part.chars().filter(|c| *c != sep).collect();
    let target_int_len = int_part.chars().count() + pad;

    // Iteratively prepend zeros and regroup until length matches target.
    // Bounded by `target_int_len` iterations.
    let mut digits = bare_int;
    for _ in 0..=target_int_len {
        let grouped = group_digits(&digits, sep, group_size);
        if grouped.chars().count() >= target_int_len {
            return format!("{grouped}{rest}");
        }
        digits.insert(0, '0');
    }
    // Safety fallback (unreachable in practice).
    let grouped = group_digits(&digits, sep, group_size);
    format!("{grouped}{rest}")
}

/// When the alternate form '#' is given to f/e/E/%, force a decimal point in
/// the body even if precision was 0.
fn ensure_alt_float(s: String, alt: bool, precision: Option<usize>) -> String {
    if !alt {
        return s;
    }
    if precision == Some(0) && !s.contains('.') {
        // Insert '.' before exponent if present, else append.
        if let Some(e_pos) = s.find(|c: char| matches!(c, 'e' | 'E')) {
            let (a, b) = s.split_at(e_pos);
            format!("{a}.{b}")
        } else {
            format!("{s}.")
        }
    } else {
        s
    }
}

/// For 'g'/'G' with '#': keep the trailing zeros that Python's '#g' preserves.
fn ensure_g_trailing_zeros(s: String, prec: usize, _upper: bool, _abs_f: f64) -> String {
    // For exponential form we already keep zeros via the format string; trim
    // happens only in the trailing-zero pass which `#g` opts out of.
    if s.contains('e') || s.contains('E') {
        return s;
    }
    if !s.contains('.') {
        // Append decimal point and pad zeros to `prec` significant figures.
        let total_digits: usize = s.chars().filter(|c| c.is_ascii_digit()).count();
        let zeros_needed = prec.saturating_sub(total_digits);
        let mut out = s;
        if zeros_needed == 0 && prec > total_digits {
            out.push('.');
        } else {
            out.push('.');
            for _ in 0..zeros_needed {
                out.push('0');
            }
        }
        return out;
    }
    s
}

/// Pad a string-typed value per the format spec.
fn pad_value(raw: &str, fs: &FormatSpec, default_align: char, fill: char) -> String {
    let raw_len = raw.chars().count();
    if fs.width == 0 || raw_len >= fs.width {
        return raw.to_string();
    }
    let pad = fs.width - raw_len;
    let align = fs.align.unwrap_or(default_align);
    let fill_str: String = std::iter::repeat(fill).take(pad).collect();
    match align {
        '>' => format!("{fill_str}{raw}"),
        '<' => format!("{raw}{fill_str}"),
        '^' => {
            let left = pad / 2;
            let right = pad - left;
            let left_fill: String = std::iter::repeat(fill).take(left).collect();
            let right_fill: String = std::iter::repeat(fill).take(right).collect();
            format!("{left_fill}{raw}{right_fill}")
        }
        _ => format!("{raw}{fill_str}"),
    }
}

/// Normalise Rust's e-notation digits to Python's: always at least two
/// exponent digits and an explicit sign.
fn normalise_exp_digits(s: String) -> String {
    let e_pos = match s.find(|c: char| matches!(c, 'e' | 'E')) {
        Some(p) => p,
        None => return s,
    };
    let (mantissa, exp_part) = s.split_at(e_pos);
    let e_char = &exp_part[..1];
    let exp_digits = &exp_part[1..];
    let (exp_sign, exp_num) = if exp_digits.starts_with('+') || exp_digits.starts_with('-') {
        (&exp_digits[..1], &exp_digits[1..])
    } else {
        ("+", exp_digits)
    };
    let exp_num_padded = if exp_num.len() < 2 {
        format!("0{exp_num}")
    } else {
        exp_num.to_string()
    };
    format!("{mantissa}{e_char}{exp_sign}{exp_num_padded}")
}

/// Coerce a `Value` to `f64` for format-spec (`format(x, ".2f")` / f-string)
/// numeric formatting.  Thin wrapper around [`try_value_to_float`] that
/// reports the format-path CPython-parity error message.
///
/// Raises `OverflowError` (not `TypeError`) when a `BigInt` argument overflows
/// f64 range, matching CPython's behaviour for `format(2**10000, ".2f")`.
fn fmt_value_to_float(value: &Value) -> Result<f64> {
    if let ValueKind::BigInt(b) = value.kind() {
        use crate::value::PyToPrimitive;
        let f = b.to_f64().unwrap_or(f64::INFINITY);
        return if f.is_finite() {
            Ok(f)
        } else {
            Err(PyError::named(
                "OverflowError",
                "int too large to convert to float".to_string(),
            ))
        };
    }
    try_value_to_float(value).ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!("must be real number, not {}", value_type_name_str(value)),
        )
    })
}

fn apply_sign_str(s: String, f: f64, sign: Option<char>) -> String {
    match sign {
        Some('+') if f >= 0.0 && !s.starts_with('-') => format!("+{s}"),
        Some(' ') if f >= 0.0 && !s.starts_with('-') => format!(" {s}"),
        _ => s,
    }
}

fn normalise_exp_str(s: String, f: f64, sign: Option<char>) -> String {
    // Rust: 1.23e5; Python: 1.23e+05 — adjust exponent format.
    let s = if let Some(e_pos) = s.find('e').or_else(|| s.find('E')) {
        let (mantissa, exp_part) = s.split_at(e_pos);
        let e_char = &exp_part[..1];
        let exp_digits = &exp_part[1..];
        // exp_digits starts with optional sign then digits
        let (exp_sign, exp_num) = if exp_digits.starts_with('+') || exp_digits.starts_with('-') {
            (&exp_digits[..1], &exp_digits[1..])
        } else {
            ("+", exp_digits)
        };
        // Python always uses at least 2 digits for the exponent (e.g. e+05 not e+5).
        let exp_num_padded = if exp_num.len() < 2 {
            format!("0{exp_num}")
        } else {
            exp_num.to_string()
        };
        format!("{mantissa}{e_char}{exp_sign}{exp_num_padded}")
    } else {
        s
    };
    apply_sign_str(s, f, sign)
}

fn format_g(f: f64, prec: usize, upper: bool) -> String {
    // Python's %g: use exponential notation if exponent < -4 or >= precision.
    if f == 0.0 {
        return "0".to_string();
    }
    let exp = f.abs().log10().floor() as i32;
    if exp < -(4_i32) || exp >= prec as i32 {
        // Use exponential notation.
        let sig_digits = prec.saturating_sub(1);
        let s = if upper {
            format!("{:.sig_digits$E}", f)
        } else {
            format!("{:.sig_digits$e}", f)
        };
        // Trim trailing zeros from mantissa, then normalise exponent.
        trim_g_trailing_zeros(normalise_exp_str(s, f, None))
    } else {
        // Fixed notation.  sig_digits significant figures.
        let decimal_digits = if exp >= 0 {
            prec.saturating_sub(exp as usize + 1)
        } else {
            prec + (-exp - 1) as usize
        };
        let s = format!("{:.decimal_digits$}", f);
        trim_g_trailing_zeros(s)
    }
}

fn trim_g_trailing_zeros(s: String) -> String {
    // Trim trailing zeros after decimal point (but keep 'e' part intact).
    let (mantissa, exp_part) = if let Some(e_pos) = s.find('e').or_else(|| s.find('E')) {
        (&s[..e_pos], &s[e_pos..])
    } else {
        (s.as_str(), "")
    };
    if mantissa.contains('.') {
        let trimmed = mantissa.trim_end_matches('0').trim_end_matches('.');
        format!("{trimmed}{exp_part}")
    } else {
        s
    }
}

/// Returns the list of attribute/method names that `dir(obj)` should report.
pub(crate) fn dir_names(value: &Value) -> Vec<String> {
    /// Recursively collect all attribute names from a class and its entire
    /// MRO (primary base then extra_bases, depth-first).
    fn collect_class_names(class: &Rc<RefCell<PyClass>>, names: &mut Vec<String>) {
        let (own_keys, base, extra_bases): (Vec<String>, _, _) = {
            let borrowed = class.borrow();
            (
                borrowed.attrs.keys().cloned().collect(),
                borrowed.base.clone(),
                borrowed.extra_bases.clone(),
            )
        };
        names.extend(own_keys);
        if let Some(b) = base {
            collect_class_names(&b, names);
        }
        for eb in &extra_bases {
            collect_class_names(eb, names);
        }
    }
    match value.kind() {
        ValueKind::PyInstance(inst) => {
            let mut names: Vec<String> = inst.borrow().attrs.keys().cloned().collect();
            let class = Rc::clone(&inst.borrow().class);
            collect_class_names(&class, &mut names);
            names
        }
        ValueKind::PyClass(class) => {
            let mut names: Vec<String> = Vec::new();
            collect_class_names(class, &mut names);
            names
        }
        ValueKind::PyModule(module) => module.borrow().attrs.keys().cloned().collect(),
        ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_) => {
            builtin_method_names("int")
        }
        ValueKind::Bytes(_) => builtin_method_names("bytes"),
        ValueKind::Str(_) => builtin_method_names("str"),
        ValueKind::List(_) => builtin_method_names("list"),
        ValueKind::Tuple(_) => builtin_method_names("tuple"),
        ValueKind::Dict(_) => builtin_method_names("dict"),
        ValueKind::Set(_) => builtin_method_names("set"),
        ValueKind::BuiltinObject { ops, .. } => builtin_method_names(ops.type_name()),
        _ => Vec::new(),
    }
}

/// Public method names per built-in type for `dir()`.
///
/// Derives the list from each type's canonical `METHODS` slice in
/// `pyrust_builtins`, so adding a new method there automatically surfaces
/// it via `dir()` without a parallel table to maintain.
///
/// TODO: also include the dunder methods CPython exposes via `dir([])` /
/// `dir("")` etc. (`__iter__`, `__len__`, `__getitem__`, `__contains__`,
/// `__add__`, …). Programs that introspect protocol support via `dir()`
/// currently get a partial answer.  Tracked separately.
fn builtin_method_names(type_name: &str) -> Vec<String> {
    let names: &[&str] = match type_name {
        "int" => pyrust_builtins::int::METHODS,
        "bytes" => pyrust_builtins::bytes::METHODS,
        "str" => pyrust_builtins::string::METHODS,
        "list" => pyrust_builtins::list::METHODS,
        "tuple" => pyrust_builtins::tuple::METHODS,
        "dict" => pyrust_builtins::dict::METHODS,
        "set" => pyrust_builtins::set::METHODS,
        "frozenset" => pyrust_builtins::frozenset::METHODS,
        _ => &[],
    };
    let mut out: Vec<String> = names.iter().map(|s| (*s).to_string()).collect();
    if type_name == "str" {
        out.push("format".to_string());
        out.push("format_map".to_string());
    }
    out
}

impl Interpreter {
    /// Renders a value to a string using the same priority as `str(x)`:
    /// `__str__` first, then `__repr__`, then the default object repr.
    /// For non-PyInstance values falls back to `Value::to_py_str()`.
    /// Exception instances bypass dunder dispatch (matching CPython's
    /// `BaseException.__str__` special-casing) and use `to_py_str()`.
    /// Used by `str.format` for the `!s` conversion and the empty-spec path.
    fn render_value_as_str(&mut self, value: &Value) -> Result<String> {
        let ValueKind::PyInstance(inst) = value.kind() else {
            return Ok(value.to_py_str());
        };
        let inst_rc = Rc::clone(inst);
        let class = Rc::clone(&inst_rc.borrow().class);
        // Exception instances use the built-in formatting (CPython special-case).
        if is_exception_class(&class) {
            return Ok(value.to_py_str());
        }
        for dunder in &["__str__", "__repr__"] {
            if let Some(method_val) = lookup_class_attr(&class, dunder) {
                let result = invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(Rc::clone(&inst_rc)),
                    &[],
                )?;
                return match result.kind() {
                    ValueKind::Str(s) => Ok(s.to_string()),
                    _ => Err(PyError::named(
                        "TypeError",
                        format!(
                            "{dunder} returned non-string (type {})",
                            pyrust_core::builtin_type_name(&result)
                        ),
                    )),
                };
            }
        }
        // No dunders found: fall back to Value::repr(), which produces
        // `<module.qualname object at 0xADDR>` matching CPython's object.__repr__.
        Ok(value.repr())
    }
}

/// Implements `str.format()`.  Parses `{...}` replacement fields in `template`
/// and substitutes positional or keyword arguments, optionally formatted by
/// a `:spec` and/or converted by `!r`/`!s`/`!a`.  Supports `{{` / `}}` for
/// literal braces and `{0.attr}` / `{0[key]}` field accessors.
impl Interpreter {
fn format_str_template(
    &mut self,
    template: &str,
    positional: &[Value],
    keyword: &[(String, Value)],
) -> Result<Value> {
    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    // Tracks auto-numbering of `{}` placeholders.  `Some(n)` = next index;
    // `None` = mixed manual/auto detected, raise on next auto.
    let mut auto_idx: Option<usize> = Some(0);
    let mut saw_manual = false;

    while i < bytes.len() {
        let c = bytes[i];
        if c == b'{' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                out.push('{');
                i += 2;
                continue;
            }
            // Find matching '}'. Track nested braces inside the format spec
            // (e.g. "{:{width}}") at depth-1 only.
            let mut depth = 1;
            let mut j = i + 1;
            while j < bytes.len() && depth > 0 {
                match bytes[j] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    break;
                }
                j += 1;
            }
            if depth != 0 {
                return Err(PyError::named(
                    "ValueError",
                    "Single '{' encountered in format string".to_string(),
                ));
            }
            let field = &template[i + 1..j];
            i = j + 1;

            // Split off the format spec at the first ':' that isn't inside `[]`.
            let (field_name_full, spec) = split_field_and_spec(field);
            // Split off the conversion (`!r`, `!s`, `!a`) from the field name.
            let (field_name, conversion) = match field_name_full.rsplit_once('!') {
                Some((name, conv)) if conv.len() == 1 => (name, Some(conv.chars().next().unwrap())),
                _ => (field_name_full, None),
            };

            // Resolve the base value from the field name's head segment.
            let (head, rest) = split_head_and_accessors(field_name);
            let base = if head.is_empty() {
                // Auto-numbered field
                if saw_manual {
                    return Err(PyError::named(
                        "ValueError",
                        "cannot switch from manual field specification to automatic field numbering".to_string(),
                    ));
                }
                let Some(idx) = auto_idx else { unreachable!() };
                auto_idx = Some(idx + 1);
                positional.get(idx).cloned().ok_or_else(|| PyError::named(
                    "IndexError",
                    format!("Replacement index {idx} out of range for positional args tuple"),
                ))?
            } else if let Ok(n) = head.parse::<usize>() {
                if auto_idx.is_some() && auto_idx != Some(0) {
                    return Err(PyError::named(
                        "ValueError",
                        "cannot switch from automatic field numbering to manual field specification".to_string(),
                    ));
                }
                saw_manual = true;
                auto_idx = None;
                positional.get(n).cloned().ok_or_else(|| PyError::named(
                    "IndexError",
                    format!("Replacement index {n} out of range for positional args tuple"),
                ))?
            } else {
                keyword
                    .iter()
                    .find(|(k, _)| k == head)
                    .map(|(_, v)| v.clone())
                    .ok_or_else(|| PyError::key_error(Value::string(head.to_string())))?
            };

            // Apply field accessors (`.attr` / `[key]`) — limited support.
            let value = apply_field_accessors(base, rest)?;

            // Apply conversion (`!r`, `!s`, `!a`).
            // `!s` dispatches `__str__` on user instances (mirrors `str(x)`).
            let value = match conversion {
                Some('r') => Value::string(render_instance_repr(self, &value)?),
                Some('s') => Value::string(self.render_value_as_str(&value)?),
                Some('a') => Value::string(ascii_repr(&value)),
                Some(c) => {
                    return Err(PyError::named(
                        "ValueError",
                        format!("Unknown conversion specifier {c}"),
                    ));
                }
                None => value,
            };

            // Apply the format spec.  When the spec is empty and the value is a
            // PyInstance, dispatch `__str__` the same way `str(x)` would — the
            // default `to_py_str()` falls through to repr instead of __str__.
            let formatted = if spec.is_empty() && matches!(value.kind(), ValueKind::PyInstance(_)) {
                Value::string(self.render_value_as_str(&value)?)
            } else {
                apply_format_spec(&value, spec)?
            };
            if let ValueKind::Str(s) = formatted.kind() {
                out.push_str(s);
            } else {
                out.push_str(&formatted.to_py_str());
            }
        } else if c == b'}' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'}' {
                out.push('}');
                i += 2;
            } else {
                return Err(PyError::named(
                    "ValueError",
                    "Single '}' encountered in format string".to_string(),
                ));
            }
        } else {
            // Walk one UTF-8 char: advance past the start byte, then skip any
            // continuation bytes (0b10xxxxxx). The outer loop guarantees the
            // current byte is a start byte (the `{`/`}` branches handle the
            // ASCII delimiters; other ASCII bytes are 1-byte chars).
            let ch_start = i;
            i += 1;
            while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 {
                i += 1;
            }
            out.push_str(&template[ch_start..i]);
        }
    }
    Ok(Value::string(out))
}
} // impl Interpreter (format_str_template + render_value_as_str)

/// Splits a replacement field on the first `:` that is not inside `[]`,
/// returning `(field_name_with_conversion, format_spec)`.
fn split_field_and_spec(field: &str) -> (&str, &str) {
    let bytes = field.as_bytes();
    let mut bracket_depth = 0;
    for (idx, b) in bytes.iter().enumerate() {
        match b {
            b'[' => bracket_depth += 1,
            b']' if bracket_depth > 0 => bracket_depth -= 1,
            b':' if bracket_depth == 0 => return (&field[..idx], &field[idx + 1..]),
            _ => {}
        }
    }
    (field, "")
}

/// Splits a field name like `0.x[1].y` into `("0", ".x[1].y")`.
fn split_head_and_accessors(name: &str) -> (&str, &str) {
    let bytes = name.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'.' || *b == b'[' {
            return (&name[..i], &name[i..]);
        }
    }
    (name, "")
}

/// Applies a chain of `.attr` / `[key]` accessors to a value.
fn apply_field_accessors(mut value: Value, mut rest: &str) -> Result<Value> {
    while !rest.is_empty() {
        let bytes = rest.as_bytes();
        if bytes[0] == b'.' {
            // Find next '.' or '['
            let end = bytes[1..]
                .iter()
                .position(|&b| b == b'.' || b == b'[')
                .map(|i| i + 1)
                .unwrap_or(rest.len());
            let attr = &rest[1..end];
            rest = &rest[end..];
            // Snapshot the PyInstance Rc in a scoped block so the
            // kind() Ref drops before we may reassign `value` (#450).
            let inst = match value.kind() {
                ValueKind::PyInstance(inst) => Some(Rc::clone(inst)),
                _ => None,
            };
            let inst = inst.ok_or_else(|| {
                PyError::named(
                    "AttributeError",
                    format!("attribute access '.{attr}' is only supported on instances"),
                )
            })?;
            // Look up the attribute: instance dict first, then class MRO.
            let class = Rc::clone(&inst.borrow().class);
            let v = inst.borrow().attrs.get(attr).cloned().or_else(|| {
                lookup_class_attr(&class, attr)
            }).ok_or_else(|| PyError::named(
                "AttributeError",
                format!("'{}' object has no attribute '{attr}'", class.borrow().name),
            ))?;
            value = v;
        } else if bytes[0] == b'[' {
            let end = bytes
                .iter()
                .position(|&b| b == b']')
                .ok_or_else(|| PyError::named(
                    "ValueError",
                    "Missing ']' in format field accessor".to_string(),
                ))?;
            let key_str = &rest[1..end];
            rest = &rest[end + 1..];
            // Try integer index first; fall back to string key.
            let next = if let Ok(idx) = key_str.parse::<i64>() {
                // Extract the indexed item in a scoped block so any
                // `kind()` Ref drops before we may construct a new
                // Value (#450).  List/Tuple snapshot to an owned Vec;
                // Dict pulls the keyed value.
                enum SeqGet {
                    Sequence(Vec<Value>),
                    DictMatch(Value),
                    NotSubscriptable,
                }
                let snap = match value.kind() {
                    ValueKind::List(items) => SeqGet::Sequence(items.clone()),
                    ValueKind::Tuple(items) => SeqGet::Sequence(items.to_vec()),
                    ValueKind::Dict(map) => match map.get(&PyKey::Int(idx)).cloned() {
                        Some(v) => SeqGet::DictMatch(v),
                        None => {
                            return Err(PyError::key_error(Value::int(idx)));
                        }
                    },
                    _ => SeqGet::NotSubscriptable,
                };
                match snap {
                    SeqGet::Sequence(items) => {
                        let len = items.len() as i64;
                        let i = if idx < 0 { idx + len } else { idx };
                        if i < 0 || i >= len {
                            return Err(PyError::named(
                                "IndexError",
                                "list index out of range".to_string(),
                            ));
                        }
                        items[i as usize].clone()
                    }
                    SeqGet::DictMatch(v) => v,
                    SeqGet::NotSubscriptable => {
                        return Err(PyError::named(
                            "TypeError",
                            "object is not subscriptable".to_string(),
                        ));
                    }
                }
            } else {
                match value.kind() {
                    ValueKind::Dict(map) => map
                        .get(&StrKey(key_str))
                        .cloned()
                        .ok_or_else(|| PyError::key_error(Value::string(key_str.to_string())))?,
                    ValueKind::List(_) | ValueKind::Tuple(_) => {
                        return Err(PyError::named(
                            "TypeError",
                            "list indices must be integers or slices, not str".to_string(),
                        ));
                    }
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "object is not subscriptable".to_string(),
                        ));
                    }
                }
            };
            value = next;
        } else {
            return Err(PyError::named(
                "ValueError",
                format!("unexpected character in format field: '{}'", &rest[..1]),
            ));
        }
    }
    Ok(value)
}

/// Does `err` signal end-of-sequence for the legacy `__getitem__`
/// iter protocol?  CPython terminates iteration on `IndexError`,
/// `StopIteration`, **and any subclass of those** raised from
/// `__getitem__`; anything else propagates to the caller.  Issue #394.
///
/// Subclass-aware: we look up the canonical `IndexError` /
/// `StopIteration` classes via [`lookup_name_in_module`] and walk the
/// raised instance's `class.base` chain with [`class_is_subclass_of`].
/// A user-defined class *named* `IndexError` that doesn't actually
/// derive from the built-in `IndexError` no longer falsely terminates
/// iteration.
pub(crate) fn is_sequence_iter_terminator(interp: &Interpreter, err: &PyError) -> bool {
    // `class_is_subclass_of` and `lookup_name_in_module` are pulled into
    // the `interpreter` module directly via `include!` in interpreter.rs
    // — no use-import needed.

    // Look up the canonical built-in exception classes in the module env.
    // If they aren't present (shouldn't happen — installed by
    // install_exception_builtins at startup), fall back to a name match
    // so the iterator still terminates rather than spinning.
    let (built_in_index, built_in_stop) = {
        let env = &interp.env;
        let idx = lookup_name_in_module(env, "IndexError");
        let stp = lookup_name_in_module(env, "StopIteration");
        let to_class = |v: Option<Value>| match v.as_ref().map(|v| v.kind()) {
            Some(ValueKind::PyClass(c)) => Some(Rc::clone(c)),
            _ => None,
        };
        (to_class(idx), to_class(stp))
    };

    let cls_is_terminator = |cls: &Rc<RefCell<PyClass>>| -> bool {
        if let Some(ref base) = built_in_index
            && class_is_subclass_of(cls, base)
        {
            return true;
        }
        if let Some(ref base) = built_in_stop
            && class_is_subclass_of(cls, base)
        {
            return true;
        }
        false
    };

    match err {
        // PyError::Named is the VM-internal raise shape that pre-dates
        // the PyInstance-backed exception path.  Match the canonical
        // built-in names directly — VM-internal raises never come from
        // user subclasses of IndexError/StopIteration.
        PyError::Named(cls, _) => cls == "IndexError" || cls == "StopIteration",
        // PyError::Class carries class identity directly; check the name the
        // same way as Named — no env lookup needed.
        PyError::Class(cls, _) => {
            let borrow = cls.borrow();
            borrow.name == "IndexError" || borrow.name == "StopIteration"
        }
        PyError::Raised(exc) => match exc.kind() {
            ValueKind::PyInstance(inst) => {
                let class = Rc::clone(&inst.borrow().class);
                cls_is_terminator(&class)
            }
            _ => false,
        },
        _ => false,
    }
}

/// Renders a value using its `__repr__` dunder for the `!r` conversion flag in
/// `str.format`.  Mirrors the `repr()` builtin's dispatch: for `PyInstance`
/// values, looks up `__repr__` via MRO, calls it, and validates the return is a
/// `str`.  Non-instances (built-in types) fall back to `value.repr()` unchanged.
///
/// Note: exception instances do not bypass `__repr__` here — CPython dispatches
/// `__repr__` on exceptions normally (only `__str__` has the special-case).
fn render_instance_repr(interp: &mut Interpreter, value: &Value) -> Result<String> {
    let ValueKind::PyInstance(inst) = value.kind() else {
        return Ok(value.repr());
    };
    let inst_rc = Rc::clone(&inst);
    let class = Rc::clone(&inst_rc.borrow().class);
    if let Some(method_val) = lookup_class_attr(&class, "__repr__") {
        let result = invoke_class_method(
            interp,
            method_val,
            Value::py_instance(Rc::clone(&inst_rc)),
            &[],
        )?;
        return match result.kind() {
            ValueKind::Str(s) => Ok(s.to_string()),
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "__repr__ returned non-string (type {})",
                    pyrust_core::builtin_type_name(&result)
                ),
            )),
        };
    }
    Ok(value.repr())
}

/// Returns the ASCII-escaped repr of a value (like the built-in `ascii()`).
pub(crate) fn ascii_repr(value: &Value) -> String {
    value
        .repr()
        .chars()
        .flat_map(|c| {
            if c.is_ascii() {
                vec![c]
            } else {
                let cp = c as u32;
                if cp <= 0xFF {
                    format!("\\x{cp:02x}").chars().collect()
                } else if cp <= 0xFFFF {
                    format!("\\u{cp:04x}").chars().collect()
                } else {
                    format!("\\U{cp:08x}").chars().collect()
                }
            }
        })
        .collect()
}

/// Merge keyword arguments into the positional `pos` buffer for str methods,
/// or raise `TypeError` for unknown kwargs or methods that accept none.
///
/// Methods that accept kwargs map them into the appropriate positional slot.
/// All other str methods reject any keyword arguments with a CPython-matching
/// `TypeError` message.  This is called only when `!kw.is_empty()`.
fn str_merge_kwargs(
    method: &str,
    pos: &mut Vec<Value>,
    kw: indexmap::IndexMap<PyKey, Value>,
) -> Result<()> {
    match method {
        // split(sep=None, maxsplit=-1)
        "split" | "rsplit" => {
            let mut sep: Option<Value> = None;
            let mut maxsplit: Option<Value> = None;
            for (k, v) in kw {
                let key_str = match &k {
                    PyKey::Str(s) => s.as_str().unwrap_or("").to_owned(),
                    _ => String::new(),
                };
                match key_str.as_str() {
                    "sep" => {
                        if pos.first().is_some() {
                            return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "argument for {method}() given by name ('sep') and position (1)"
                                ),
                            ));
                        }
                        sep = Some(v);
                    }
                    "maxsplit" => {
                        if pos.get(1).is_some() {
                            return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "argument for {method}() given by name ('maxsplit') and position (2)"
                                ),
                            ));
                        }
                        maxsplit = Some(v);
                    }
                    other => {
                        return Err(PyError::named(
                            "TypeError",
                            format!("'{other}' is an invalid keyword argument for {method}()"),
                        ));
                    }
                }
            }
            // Merge into positional slots, extending as needed.
            // pos[0] = sep, pos[1] = maxsplit
            if let Some(ms) = maxsplit {
                // Ensure pos[0] exists (sep defaults to None)
                if pos.is_empty() {
                    pos.push(sep.unwrap_or_else(Value::none));
                } else if let Some(sep_val) = sep {
                    pos[0] = sep_val;
                }
                if pos.len() < 2 {
                    pos.push(ms);
                }
            } else if let Some(sep_val) = sep {
                if pos.is_empty() {
                    pos.push(sep_val);
                } else {
                    pos[0] = sep_val;
                }
            }
            Ok(())
        }
        // splitlines(keepends=False)
        "splitlines" => {
            let mut keepends: Option<Value> = None;
            for (k, v) in kw {
                let key_str = match &k {
                    PyKey::Str(s) => s.as_str().unwrap_or("").to_owned(),
                    _ => String::new(),
                };
                match key_str.as_str() {
                    "keepends" => {
                        if pos.first().is_some() {
                            return Err(PyError::named(
                                "TypeError",
                                "argument for splitlines() given by name ('keepends') and position (1)".to_string(),
                            ));
                        }
                        keepends = Some(v);
                    }
                    other => {
                        return Err(PyError::named(
                            "TypeError",
                            format!("'{other}' is an invalid keyword argument for splitlines()"),
                        ));
                    }
                }
            }
            if let Some(ke) = keepends {
                pos.push(ke);
            }
            Ok(())
        }
        // expandtabs(tabsize=8)
        "expandtabs" => {
            let mut tabsize: Option<Value> = None;
            for (k, v) in kw {
                let key_str = match &k {
                    PyKey::Str(s) => s.as_str().unwrap_or("").to_owned(),
                    _ => String::new(),
                };
                match key_str.as_str() {
                    "tabsize" => {
                        if pos.first().is_some() {
                            return Err(PyError::named(
                                "TypeError",
                                "argument for expandtabs() given by name ('tabsize') and position (1)".to_string(),
                            ));
                        }
                        tabsize = Some(v);
                    }
                    other => {
                        // CPython: "expandtabs() takes at most 1 keyword argument (N given)"
                        // but for unknown kwarg it raises "'foo' is an invalid keyword argument"
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "'{other}' is an invalid keyword argument for expandtabs()"
                            ),
                        ));
                    }
                }
            }
            if let Some(ts) = tabsize {
                pos.push(ts);
            }
            Ok(())
        }
        // All str methods that take no keyword arguments use the `str.` prefix
        _ => Err(PyError::named(
            "TypeError",
            format!("str.{method}() takes no keyword arguments"),
        )),
    }
}
