// Thread-local call depth counter. Using thread_local avoids the split-borrow
// problem: a guard that holds &mut self.call_depth cannot coexist with a &mut self
// method call. The thread_local is safe because the interpreter is single-threaded.
use std::cell::Cell;

use smallvec::SmallVec;

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

impl Interpreter {
    fn call_function(&mut self, function: Value, args: &[CallArg]) -> Result<Value> {
        let expanded = self.expand_call_args(args)?;
        self.call_function_expanded(function, &expanded)
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
                let mut receiver = receiver_owned;
                // Separate positional and keyword args.
                let mut pos: Vec<Value> = Vec::with_capacity(args.len());
                let mut kw: indexmap::IndexMap<PyKey, Value> = indexmap::IndexMap::new();
                for a in args {
                    match &a.name {
                        Some(n) => { kw.insert(PyKey::Str(n.clone()), a.value.clone()); }
                        None => pos.push(a.value.clone()),
                    }
                }
                match receiver.kind() {
                    ValueKind::Str(_) => {
                        pyrust_builtins::string::call(method, &receiver, pos)
                    }
                    ValueKind::List(_) => {
                        let items = receiver
                            .as_list_mut()
                            .ok_or_else(|| PyError::Runtime("internal: expected list".to_string()))?;
                        pyrust_builtins::list::call(method, items, pos, &kw)
                    }
                    ValueKind::Tuple(items) => {
                        pyrust_builtins::tuple::call(method, items, pos)
                    }
                    ValueKind::Dict(_) => {
                        let dict = receiver
                            .as_dict_mut()
                            .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                        pyrust_builtins::dict::call(method, dict, pos)
                    }
                    ValueKind::Set(_) => {
                        let set = receiver
                            .as_set_mut()
                            .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
                        pyrust_builtins::set::call(method, set, pos)
                    }
                    ValueKind::Complex(re, im) if method == "conjugate" => {
                        Ok(Value::complex(re, -im))
                    }
                    ValueKind::BuiltinObject { ops, state } => {
                        let empty_kw: indexmap::IndexMap<String, Value> =
                            indexmap::IndexMap::new();
                        ops.call_method(state, method, pos, &empty_kw)
                    }
                    _ => Err(PyError::named(
                        "TypeError",
                        format!("'{}' object has no method '{method}'", pyrust_core::builtin_type_name(&receiver)),
                    )),
                }
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
                format_str_template(&template, &positional, &keyword)
            }
            ValueKind::BuiltinFunction(name) if name.starts_with("str.") => {
                let method = &name[4..];
                let self_val = args
                    .first()
                    .map(|a| &a.value)
                    .ok_or_else(|| PyError::named(
                        "TypeError",
                        format!("descriptor '{method}' of 'str' object needs an argument"),
                    ))?;
                let rest: Vec<Value> = args[1..].iter().map(|a| a.value.clone()).collect();
                pyrust_builtins::string::call(method, self_val, rest)
            }
            ValueKind::UserFunction(function) => {
                let function = Rc::clone(function);
                self.call_user_function_expanded(function, args, &[])
            }
            ValueKind::PyClass(class) => {
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
                if let Some(method_val) = lookup_class_attr(&class, "__call__")
                    && let ValueKind::UserFunction(f) = method_val.kind() {
                        let func = Rc::clone(f);
                        return self.call_user_function_expanded(
                            func,
                            args,
                            &[Value::py_instance(inst_rc)],
                        );
                    }
                Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object is not callable",
                        class.borrow().name
                    ),
                ))
            }
            _ => Err(PyError::Runtime("object is not callable".to_string())),
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
                    Err(PyError::GeneratorYield(yielded)) => {
                        drop(borrow);
                        items.push(yielded);
                    }
                    Err(PyError::Named(ref cls, _)) if cls == "StopIteration" => {
                        break;
                    }
                    Err(e) => return Err(e),
                    Ok(_) => unreachable!("resume_generator always returns Err"),
                }
            }
            Ok(items)
        } else if let ValueKind::PyInstance(inst) = val.kind() {
            // Get iterator via __iter__ (or use self if it only has __next__).
            let iterator = {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__iter__")
                    && let ValueKind::UserFunction(f) = method_val.kind()
                {
                    let func = Rc::clone(f);
                    self.call_user_function_expanded(
                        func,
                        &[],
                        &[Value::py_instance(inst_rc)],
                    )?
                } else if lookup_class_attr(&class, "__next__").is_some() {
                    val.clone()
                } else {
                    return Err(PyError::named(
                        "TypeError",
                        format!("'{}' object is not iterable", class.borrow().name),
                    ));
                }
            };
            let mut items = Vec::new();
            loop {
                match self.call_next(iterator.clone(), None) {
                    Ok(item) => items.push(item),
                    Err(PyError::Named(ref cls, _)) if cls == "StopIteration" => break,
                    Err(PyError::Raised(ref exc)) => {
                        let is_stop = matches!(exc.kind(),
                            ValueKind::PyInstance(i) if i.borrow().class.borrow().name == "StopIteration"
                        );
                        if is_stop { break; }
                        return Err(PyError::Raised(exc.clone()));
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok(items)
        } else {
            iter_values(val)
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
                    Err(PyError::named("StopIteration", String::new()))
                };
            }
            match self.resume_generator(frame) {
                Err(PyError::GeneratorYield(yielded)) => Ok(yielded),
                Err(PyError::Named(ref cls, _)) if cls == "StopIteration" => {
                    drop(borrow);
                    if let Some(d) = default {
                        Ok(d)
                    } else {
                        Err(PyError::named("StopIteration", String::new()))
                    }
                }
                Err(e) => Err(e),
                Ok(_) => unreachable!("resume_generator always returns Err"),
            }
        } else if let ValueKind::PyInstance(inst) = val.kind() {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__next__")
                && let ValueKind::UserFunction(f) = method_val.kind()
            {
                let func = Rc::clone(f);
                match self.call_user_function_expanded(func, &[], &[Value::py_instance(inst_rc)]) {
                    Ok(v) => Ok(v),
                    Err(PyError::Raised(exc)) => {
                        let is_stop = match exc.kind() {
                            ValueKind::PyInstance(i) => {
                                i.borrow().class.borrow().name == "StopIteration"
                            }
                            _ => false,
                        };
                        if is_stop {
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

    fn expand_call_args(
        &mut self,
        args: &[CallArg],
    ) -> Result<SmallVec<[ExpandedCallArg; 4]>> {
        let mut out: SmallVec<[ExpandedCallArg; 4]> = SmallVec::new();
        for arg in args {
            if arg.splat {
                let value = self.eval_expr(&arg.value)?;
                let items = iter_values(value)?;
                for item in items {
                    out.push(ExpandedCallArg {
                        name: None,
                        value: item,
                    });
                }
                continue;
            }
            if arg.double_splat {
                let value = self.eval_expr(&arg.value)?;
                let items = match value.kind() {
                    ValueKind::Dict(d) => d.clone(),
                    _ => return Err(PyError::Runtime(
                        "** argument after ** must be a mapping".to_string(),
                    )),
                };
                for (k, v) in items {
                    let name = match k {
                        PyKey::Str(s) => s,
                        _ => return Err(PyError::Runtime(
                            "keywords must be strings".to_string(),
                        )),
                    };
                    out.push(ExpandedCallArg {
                        name: Some(name),
                        value: v,
                    });
                }
                continue;
            }

            out.push(ExpandedCallArg {
                name: arg.name.clone(),
                value: self.eval_expr(&arg.value)?,
            });
        }
        Ok(out)
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

    fn call_user_function(
        &mut self,
        function: Rc<UserFunction>,
        args: &[CallArg],
        bound_prefix: &[Value],
    ) -> Result<Value> {
        let expanded = self.expand_call_args(args)?;
        self.call_user_function_expanded(function, &expanded, bound_prefix)
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
                return Err(PyError::Runtime(format!(
                    "{}() takes from {} to {} arguments but {} were given",
                    function.name,
                    required_params,
                    function.params.len(),
                    positional_count + bound_prefix.len()
                )));
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
                        return Err(PyError::Runtime(format!(
                            "{}() got an unexpected keyword argument '{}'",
                            function.name, name
                        )));
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
                        return Err(PyError::Runtime(format!(
                            "{}() got multiple values for argument '{}'",
                            function.name, name
                        )));
                    }
                    bound_args[param_index] = Some(value);
                } else {
                    while positional_index < bound_args.len() && bound_args[positional_index].is_some() {
                        positional_index += 1;
                    }
                    if positional_index >= bound_args.len() {
                        return Err(PyError::Runtime(format!(
                            "{}() takes from {} to {} arguments but {} were given",
                            function.name, required_params, function.params.len(),
                            positional_count + bound_prefix.len()
                        )));
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
            let cache_key: Option<(u64, Vec<PyKey>)> = if function.is_pure {
                bound_args
                    .iter()
                    .map(|v| v.as_ref().unwrap().to_key())
                    .collect::<Option<Vec<PyKey>>>()
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
                    let exc = self.instantiate_named_exception(
                        "RecursionError",
                        "maximum recursion depth exceeded".to_string(),
                    )?;
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
                    let gen_env = std::mem::replace(&mut self.env, previous_env);
                    if !needs_local_env {
                        // No local env was allocated; use the function's own env.
                        // (gen_env == function.env here)
                    }
                    let frame = GeneratorFrame {
                        code: Rc::clone(&code),
                        regs,
                        iters: vec![None; code.num_iters as usize],
                        exc_handlers: Vec::new(),
                        pc: 0,
                        done: false,
                        saved_env: gen_env,
                    };
                    return Ok(Value::generator(Box::new(frame)));
                }

                let vm_result = self.run_bytecode_for_fn(&code, &mut regs, function.id);

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
                let exc = self.instantiate_named_exception(
                    "RecursionError",
                    "maximum recursion depth exceeded".to_string(),
                )?;
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

            let vm_result = self.run_bytecode_for_fn(&code, &mut regs, function.id);

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
            return Ok(instantiate_exception(class, values));
        }

        let instance = Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(&class),
            attrs: HashMap::new(),
        }));

        let init = lookup_class_attr(&class, "__init__");
        match init {
            Some(ref v) if matches!(v.kind(), ValueKind::UserFunction(_)) => {
                let function = if let ValueKind::UserFunction(f) = v.kind() {
                    Rc::clone(f)
                } else { unreachable!() };
                let result = self.call_user_function_expanded(
                    function,
                    args,
                    &[Value::py_instance(Rc::clone(&instance))],
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
                if !args.is_empty() {
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

}

/// Apply a Python format spec string to a `Value` and return the formatted string.
/// Supports common numeric specs: `d`, `f`, `e`, `g`, `x`, `X`, `o`, `b`, `s`,
/// with optional width, fill/align, sign, and precision.
pub(crate) fn apply_format_spec(value: &Value, spec: &str) -> Result<Value> {
    if spec.is_empty() {
        return Ok(Value::string(value.to_py_str()));
    }

    // Parse the format spec.  We support the most common subset:
    //   [[fill]align][sign][#][0][width][.precision][type]
    // where align is <, >, ^; sign is +, -, ' '; type is one of s d f e g x X o b.
    let chars: Vec<char> = spec.chars().collect();
    let len = chars.len();
    let mut pos = 0;

    // fill + align
    let (fill, align) = if len >= 2 && matches!(chars[1], '<' | '>' | '^') {
        let f = chars[0];
        let a = chars[1];
        pos += 2;
        (f, Some(a))
    } else if len >= 1 && matches!(chars[0], '<' | '>' | '^') {
        let a = chars[0];
        pos += 1;
        (' ', Some(a))
    } else {
        (' ', None)
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

    // zero-padding '0'
    let zero_pad = if pos < len && chars[pos] == '0' {
        pos += 1;
        true
    } else {
        false
    };
    let fill = if zero_pad && align.is_none() { '0' } else { fill };

    // width
    let width_start = pos;
    while pos < len && chars[pos].is_ascii_digit() {
        pos += 1;
    }
    let width: usize = if pos > width_start {
        chars[width_start..pos]
            .iter()
            .collect::<String>()
            .parse()
            .unwrap_or(0)
    } else {
        0
    };

    // precision
    let precision = if pos < len && chars[pos] == '.' {
        pos += 1;
        let prec_start = pos;
        while pos < len && chars[pos].is_ascii_digit() {
            pos += 1;
        }
        if pos > prec_start {
            Some(
                chars[prec_start..pos]
                    .iter()
                    .collect::<String>()
                    .parse::<usize>()
                    .unwrap_or(6),
            )
        } else {
            Some(6)
        }
    } else {
        None
    };

    // type char
    let type_char = if pos < len { Some(chars[pos]) } else { None };

    // Format the value into a raw string (without width/alignment).
    let raw = match type_char {
        None | Some('s') => match value.kind() {
            ValueKind::Str(s) => {
                let s = s.to_string();
                match precision {
                    Some(p) => s.chars().take(p).collect(),
                    None => s,
                }
            }
            _ => value.to_py_str(),
        },
        Some('d') => match value.kind() {
            ValueKind::Int(n) => format_int_with_sign(n, sign),
            ValueKind::Bool(b) => format_int_with_sign(if b { 1 } else { 0 }, sign),
            _ => return Err(PyError::named(
                "ValueError",
                format!("unknown format code 'd' for object of type '{}'", value_type_name_str(value)),
            )),
        },
        Some('f') | Some('F') => {
            let prec = precision.unwrap_or(6);
            let f = fmt_value_to_float(value)?;
            let s = if type_char == Some('F') {
                format!("{:.prec$}", f).to_uppercase()
            } else {
                format!("{:.prec$}", f)
            };
            apply_sign_str(s, f, sign)
        }
        Some('e') | Some('E') => {
            let prec = precision.unwrap_or(6);
            let f = fmt_value_to_float(value)?;
            let s = if type_char == Some('E') {
                format!("{:.prec$E}", f)
            } else {
                format!("{:.prec$e}", f)
            };
            // Python uses e+XX not e+0XX — Rust uses two digits for small exponents.
            // Normalise to match Python's format.
            normalise_exp_str(s, f, sign)
        }
        Some('g') | Some('G') => {
            let prec = precision.unwrap_or(6);
            let prec = if prec == 0 { 1 } else { prec };
            let f = fmt_value_to_float(value)?;
            let s = format_g(f, prec, type_char == Some('G'));
            apply_sign_str(s, f, sign)
        }
        Some('%') => {
            let prec = precision.unwrap_or(6);
            let f = fmt_value_to_float(value)?;
            format!("{:.prec$}%", f * 100.0)
        }
        Some('x') => match value.kind() {
            ValueKind::Int(n) => {
                let s = if n < 0 {
                    format!("-{:x}", (-n) as u64)
                } else {
                    format!("{:x}", n as u64)
                };
                if alt { format!("0x{s}") } else { s }
            }
            ValueKind::Bool(b) => {
                let n: i64 = if b { 1 } else { 0 };
                if alt { format!("0x{n:x}") } else { format!("{n:x}") }
            }
            _ => return Err(PyError::named(
                "ValueError",
                format!("unknown format code 'x' for object of type '{}'", value_type_name_str(value)),
            )),
        },
        Some('X') => match value.kind() {
            ValueKind::Int(n) => {
                let s = if n < 0 {
                    format!("-{:X}", (-n) as u64)
                } else {
                    format!("{:X}", n as u64)
                };
                if alt { format!("0X{s}") } else { s }
            }
            ValueKind::Bool(b) => {
                let n: i64 = if b { 1 } else { 0 };
                if alt { format!("0X{n:X}") } else { format!("{n:X}") }
            }
            _ => return Err(PyError::named(
                "ValueError",
                format!("unknown format code 'X' for object of type '{}'", value_type_name_str(value)),
            )),
        },
        Some('o') => match value.kind() {
            ValueKind::Int(n) => {
                let s = if n < 0 {
                    format!("-{:o}", (-n) as u64)
                } else {
                    format!("{:o}", n as u64)
                };
                if alt { format!("0o{s}") } else { s }
            }
            ValueKind::Bool(b) => {
                let n: i64 = if b { 1 } else { 0 };
                if alt { format!("0o{n:o}") } else { format!("{n:o}") }
            }
            _ => return Err(PyError::named(
                "ValueError",
                format!("unknown format code 'o' for object of type '{}'", value_type_name_str(value)),
            )),
        },
        Some('b') => match value.kind() {
            ValueKind::Int(n) => {
                let s = if n < 0 {
                    format!("-{:b}", (-n) as u64)
                } else {
                    format!("{:b}", n as u64)
                };
                if alt { format!("0b{s}") } else { s }
            }
            ValueKind::Bool(b) => {
                let n: i64 = if b { 1 } else { 0 };
                if alt { format!("0b{n:b}") } else { format!("{n:b}") }
            }
            _ => return Err(PyError::named(
                "ValueError",
                format!("unknown format code 'b' for object of type '{}'", value_type_name_str(value)),
            )),
        },
        Some(other) => {
            return Err(PyError::named(
                "ValueError",
                format!("unknown format code '{other}' for object of type '{}'", value_type_name_str(value)),
            ))
        }
    };

    // Apply width / alignment.
    if width == 0 || raw.chars().count() >= width {
        return Ok(Value::string(raw));
    }
    let pad = width - raw.chars().count();
    let effective_align = align.unwrap_or(if matches!(type_char, Some('d' | 'f' | 'e' | 'E' | 'g' | 'G' | 'x' | 'X' | 'o' | 'b') | None) && !matches!(value.kind(), ValueKind::Str(_)) {
        '>'
    } else {
        '<'
    });
    let padded = match effective_align {
        '>' => {
            let mut s = fill.to_string().repeat(pad);
            s.push_str(&raw);
            s
        }
        '^' => {
            let left = pad / 2;
            let right = pad - left;
            let mut s = fill.to_string().repeat(left);
            s.push_str(&raw);
            s.push_str(&fill.to_string().repeat(right));
            s
        }
        _ => {
            // '<' or default
            let mut s = raw;
            s.push_str(&fill.to_string().repeat(pad));
            s
        }
    };
    Ok(Value::string(padded))
}

fn fmt_value_to_float(value: &Value) -> Result<f64> {
    match value.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(n) => Ok(n as f64),
        ValueKind::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
        _ => Err(PyError::named(
            "TypeError",
            format!("must be real number, not {}", value_type_name_str(value)),
        )),
    }
}

fn format_int_with_sign(n: i64, sign: Option<char>) -> String {
    match sign {
        Some('+') => {
            if n >= 0 { format!("+{n}") } else { format!("{n}") }
        }
        Some(' ') => {
            if n >= 0 { format!(" {n}") } else { format!("{n}") }
        }
        _ => format!("{n}"),
    }
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
    match value.kind() {
        ValueKind::PyInstance(inst) => {
            let mut names: Vec<String> = inst.borrow().attrs.keys().cloned().collect();
            let class = Rc::clone(&inst.borrow().class);
            let mut cls = Some(class);
            while let Some(c) = cls {
                let cb = c.borrow();
                for k in cb.attrs.keys() {
                    names.push(k.clone());
                }
                cls = cb.base.clone();
            }
            names
        }
        ValueKind::PyClass(class) => {
            let mut names: Vec<String> = Vec::new();
            let mut cls = Some(Rc::clone(class));
            while let Some(c) = cls {
                let cb = c.borrow();
                for k in cb.attrs.keys() {
                    names.push(k.clone());
                }
                cls = cb.base.clone();
            }
            names
        }
        ValueKind::PyModule(module) => module.borrow().attrs.keys().cloned().collect(),
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
/// `str.format` is appended explicitly because it is dispatched at the VM
/// level rather than through `string::call` (see the comment on
/// `pyrust_builtins::string::METHODS`).  This is the one method-name source
/// of truth that lives outside `METHODS`.
///
/// TODO: also include the dunder methods CPython exposes via `dir([])` /
/// `dir("")` etc. (`__iter__`, `__len__`, `__getitem__`, `__contains__`,
/// `__add__`, …). Programs that introspect protocol support via `dir()`
/// currently get a partial answer.  Tracked separately.
fn builtin_method_names(type_name: &str) -> Vec<String> {
    let names: &[&str] = match type_name {
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
    }
    out
}

/// Implements `str.format()`.  Parses `{...}` replacement fields in `template`
/// and substitutes positional or keyword arguments, optionally formatted by
/// a `:spec` and/or converted by `!r`/`!s`/`!a`.  Supports `{{` / `}}` for
/// literal braces and `{0.attr}` / `{0[key]}` field accessors.
fn format_str_template(
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
                    .ok_or_else(|| PyError::named(
                        "KeyError",
                        format!("'{head}'"),
                    ))?
            };

            // Apply field accessors (`.attr` / `[key]`) — limited support.
            let value = apply_field_accessors(base, rest)?;

            // Apply conversion (`!r`, `!s`, `!a`).
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

            // Apply the format spec.
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
            match value.kind() {
                ValueKind::PyInstance(inst) => {
                    let v = inst
                        .borrow()
                        .attrs
                        .get(attr)
                        .cloned()
                        .ok_or_else(|| PyError::named(
                            "AttributeError",
                            format!("attribute '{attr}' not found"),
                        ))?;
                    value = v;
                }
                _ => {
                    return Err(PyError::named(
                        "AttributeError",
                        format!("attribute access '.{attr}' is only supported on instances"),
                    ));
                }
            }
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
                match value.kind() {
                    ValueKind::List(items) | ValueKind::Tuple(items) => {
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
                    ValueKind::Dict(map) => map
                        .get(&PyKey::Int(idx))
                        .cloned()
                        .ok_or_else(|| PyError::named(
                            "KeyError",
                            format!("{idx}"),
                        ))?,
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "object is not subscriptable".to_string(),
                        ));
                    }
                }
            } else {
                match value.kind() {
                    ValueKind::Dict(map) => map
                        .get(&PyKey::Str(key_str.to_string()))
                        .cloned()
                        .ok_or_else(|| PyError::named(
                            "KeyError",
                            format!("'{key_str}'"),
                        ))?,
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
