impl Interpreter {
    fn call_function(&mut self, function: Value, args: &[CallArg]) -> Result<Value> {
        let expanded = self.expand_call_args(args)?;
        self.call_function_expanded(function, &expanded)
    }

    fn call_function_expanded(
        &mut self,
        function: Value,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        match function {
            Value::Builtin("print") => {
                let print_options = self.parse_print_options_expanded(args)?;
                let mut rendered = Vec::with_capacity(print_options.values.len());
                for value in print_options.values {
                    rendered.push(value.to_py_str());
                }
                print!("{}{}", rendered.join(&print_options.sep), print_options.end);
                Ok(Value::None)
            }
            Value::Builtin("len") => {
                reject_keyword_args_expanded("len", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "len() takes exactly one argument".to_string(),
                    ));
                }
                let value = args[0].value.clone();
                let size = match value {
                    Value::Str(text) => text.chars().count() as i64,
                    Value::List(items) => items.len() as i64,
                    Value::Tuple(items) => items.len() as i64,
                    Value::Set(items) => items.len() as i64,
                    Value::Dict(items) => items.len() as i64,
                    Value::Range { start, stop, step } => range_len(start, stop, step),
                    _ => {
                        return Err(PyError::Runtime("object has no len()".to_string()));
                    }
                };
                Ok(Value::Int(size))
            }
            Value::Builtin("range") => self.call_range_expanded(args),

            Value::Builtin("enumerate") => {
                reject_keyword_args_expanded("enumerate", args)?;
                if args.is_empty() || args.len() > 2 {
                    return Err(PyError::Runtime(
                        "enumerate() takes 1 or 2 arguments".to_string(),
                    ));
                }
                let start = if args.len() == 2 {
                    match &args[1].value {
                        Value::Int(n) => *n,
                        _ => return Err(PyError::Runtime(
                            "enumerate() start argument must be an integer".to_string(),
                        )),
                    }
                } else {
                    0i64
                };
                let items = iter_values(args[0].value.clone())?;
                Ok(Value::List(
                    items
                        .into_iter()
                        .enumerate()
                        .map(|(i, v)| Value::Tuple(vec![Value::Int(i as i64 + start), v]))
                        .collect(),
                ))
            }

            Value::Builtin("zip") => {
                reject_keyword_args_expanded("zip", args)?;
                if args.is_empty() {
                    return Ok(Value::List(vec![]));
                }
                let mut iters: Vec<Vec<Value>> = Vec::with_capacity(args.len());
                for arg in args {
                    iters.push(iter_values(arg.value.clone())?);
                }
                let len = iters.iter().map(|v| v.len()).min().unwrap_or(0);
                Ok(Value::List(
                    (0..len)
                        .map(|i| Value::Tuple(iters.iter().map(|it| it[i].clone()).collect()))
                        .collect(),
                ))
            }

            Value::Builtin("reversed") => {
                reject_keyword_args_expanded("reversed", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "reversed() takes exactly one argument".to_string(),
                    ));
                }
                let mut items = iter_values(args[0].value.clone())?;
                items.reverse();
                Ok(Value::List(items))
            }

            Value::Builtin("sorted") => {
                if args.is_empty() {
                    return Err(PyError::Runtime(
                        "sorted() requires at least one argument".to_string(),
                    ));
                }
                let reverse = args.iter().find(|a| a.name.as_deref() == Some("reverse"))
                    .map(|a| a.value.truthy())
                    .unwrap_or(false);
                let key_fn = args.iter().find(|a| a.name.as_deref() == Some("key"))
                    .map(|a| a.value.clone());
                let positional: Vec<&ExpandedCallArg> = args.iter()
                    .filter(|a| a.name.is_none())
                    .collect();
                if positional.len() != 1 {
                    return Err(PyError::Runtime(
                        "sorted() takes exactly one positional argument".to_string(),
                    ));
                }
                let mut items = iter_values(positional[0].value.clone())?;
                if let Some(kfn) = key_fn {
                    let mut keyed: Vec<(Value, Value)> = items
                        .into_iter()
                        .map(|v| {
                            let k = self.call_function_expanded(
                                kfn.clone(),
                                &[ExpandedCallArg { name: None, value: v.clone() }],
                            )?;
                            Ok((k, v))
                        })
                        .collect::<Result<_>>()?;
                    keyed.sort_by(|(a, _), (b, _)| compare_values(a, b));
                    items = keyed.into_iter().map(|(_, v)| v).collect();
                } else {
                    items.sort_by(compare_values);
                }
                if reverse {
                    items.reverse();
                }
                Ok(Value::List(items))
            }

            Value::Builtin("abs") => {
                reject_keyword_args_expanded("abs", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "abs() takes exactly one argument".to_string(),
                    ));
                }
                match &args[0].value {
                    Value::Int(v) => Ok(Value::Int(v.abs())),
                    Value::Float(v) => Ok(Value::Float(v.abs())),
                    Value::Bool(b) => Ok(Value::Int(if *b { 1 } else { 0 })),
                    _ => Err(PyError::Runtime(
                        "abs() argument must be a number".to_string(),
                    )),
                }
            }

            Value::Builtin("min") | Value::Builtin("max") => {
                let is_max = matches!(function, Value::Builtin("max"));
                let fname = if is_max { "max" } else { "min" };
                reject_keyword_args_expanded(fname, args)?;
                let items: Vec<Value> = if args.len() == 1 {
                    iter_values(args[0].value.clone())?
                } else if args.len() >= 2 {
                    args.iter().map(|a| a.value.clone()).collect()
                } else {
                    return Err(PyError::Runtime(format!(
                        "{fname}() expected at least one argument"
                    )));
                };
                if items.is_empty() {
                    return Err(PyError::Runtime(format!(
                        "{fname}() arg is an empty sequence"
                    )));
                }
                let result = items.into_iter().reduce(|acc, v| {
                    let cmp = compare_values(&v, &acc);
                    if is_max && cmp == std::cmp::Ordering::Greater { v } else if !is_max && cmp == std::cmp::Ordering::Less { v } else { acc }
                }).unwrap();
                Ok(result)
            }

            Value::Builtin("sum") => {
                reject_keyword_args_expanded("sum", args)?;
                if args.is_empty() || args.len() > 2 {
                    return Err(PyError::Runtime(
                        "sum() takes 1 or 2 arguments".to_string(),
                    ));
                }
                let items = iter_values(args[0].value.clone())?;
                let start = if args.len() == 2 { args[1].value.clone() } else { Value::Int(0) };
                let mut acc = start;
                for item in items {
                    acc = self.eval_binary(acc, BinaryOp::Add, item)?;
                }
                Ok(acc)
            }

            Value::Builtin("list") => {
                reject_keyword_args_expanded("list", args)?;
                match args.len() {
                    0 => Ok(Value::List(vec![])),
                    1 => Ok(Value::List(iter_values(args[0].value.clone())?)),
                    _ => Err(PyError::Runtime("list() takes at most one argument".to_string())),
                }
            }

            Value::Builtin("tuple") => {
                reject_keyword_args_expanded("tuple", args)?;
                match args.len() {
                    0 => Ok(Value::Tuple(vec![])),
                    1 => Ok(Value::Tuple(iter_values(args[0].value.clone())?)),
                    _ => Err(PyError::Runtime("tuple() takes at most one argument".to_string())),
                }
            }

            Value::Builtin("str") => {
                reject_keyword_args_expanded("str", args)?;
                match args.len() {
                    0 => Ok(Value::Str(String::new())),
                    1 => Ok(Value::Str(args[0].value.to_py_str())),
                    _ => Err(PyError::Runtime("str() takes at most one argument".to_string())),
                }
            }

            Value::Builtin("int") => {
                reject_keyword_args_expanded("int", args)?;
                match args.len() {
                    0 => Ok(Value::Int(0)),
                    1 => match &args[0].value {
                        Value::Int(v) => Ok(Value::Int(*v)),
                        Value::Float(v) => Ok(Value::Int(*v as i64)),
                        Value::Bool(b) => Ok(Value::Int(if *b { 1 } else { 0 })),
                        Value::Str(s) => s.trim().parse::<i64>().map(Value::Int).map_err(|_| {
                            PyError::Runtime(format!(
                                "invalid literal for int() with base 10: '{s}'"
                            ))
                        }),
                        _ => Err(PyError::Runtime(
                            "int() argument must be a number or string".to_string(),
                        )),
                    },
                    _ => Err(PyError::Runtime("int() takes at most one argument".to_string())),
                }
            }

            Value::Builtin("float") => {
                reject_keyword_args_expanded("float", args)?;
                match args.len() {
                    0 => Ok(Value::Float(0.0)),
                    1 => match &args[0].value {
                        Value::Float(v) => Ok(Value::Float(*v)),
                        Value::Int(v) => Ok(Value::Float(*v as f64)),
                        Value::Bool(b) => Ok(Value::Float(if *b { 1.0 } else { 0.0 })),
                        Value::Str(s) => s.trim().parse::<f64>().map(Value::Float).map_err(|_| {
                            PyError::Runtime(format!(
                                "could not convert string to float: '{s}'"
                            ))
                        }),
                        _ => Err(PyError::Runtime(
                            "float() argument must be a number or string".to_string(),
                        )),
                    },
                    _ => Err(PyError::Runtime("float() takes at most one argument".to_string())),
                }
            }

            Value::Builtin("bool") => {
                reject_keyword_args_expanded("bool", args)?;
                match args.len() {
                    0 => Ok(Value::Bool(false)),
                    1 => Ok(Value::Bool(args[0].value.truthy())),
                    _ => Err(PyError::Runtime("bool() takes at most one argument".to_string())),
                }
            }

            Value::Builtin("sys.exit") => {
                let code = if args.is_empty() {
                    0i32
                } else {
                    reject_keyword_args_expanded("sys.exit", args)?;
                    match &args[0].value {
                        Value::Int(n) => *n as i32,
                        _ => 1,
                    }
                };
                std::process::exit(code);
            }
            Value::Builtin(
                name @ ("math.floor" | "math.ceil" | "math.sqrt" | "math.fabs" | "math.sin"
                | "math.cos" | "math.tan" | "math.asin" | "math.acos" | "math.atan"
                | "math.exp" | "math.log2" | "math.log10" | "math.isnan" | "math.isinf"),
            ) => {
                reject_keyword_args_expanded(name, args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(format!(
                        "{name}() takes exactly one argument"
                    )));
                }
                let x = value_to_float(&args[0].value, name)?;
                match name {
                    "math.floor" => Ok(Value::Int(x.floor() as i64)),
                    "math.ceil" => Ok(Value::Int(x.ceil() as i64)),
                    "math.sqrt" => Ok(Value::Float(x.sqrt())),
                    "math.fabs" => Ok(Value::Float(x.abs())),
                    "math.sin" => Ok(Value::Float(x.sin())),
                    "math.cos" => Ok(Value::Float(x.cos())),
                    "math.tan" => Ok(Value::Float(x.tan())),
                    "math.asin" => Ok(Value::Float(x.asin())),
                    "math.acos" => Ok(Value::Float(x.acos())),
                    "math.atan" => Ok(Value::Float(x.atan())),
                    "math.exp" => Ok(Value::Float(x.exp())),
                    "math.log2" => Ok(Value::Float(x.log2())),
                    "math.log10" => Ok(Value::Float(x.log10())),
                    "math.isnan" => Ok(Value::Bool(x.is_nan())),
                    "math.isinf" => Ok(Value::Bool(x.is_infinite())),
                    _ => unreachable!(),
                }
            }
            Value::Builtin("math.pow") => {
                reject_keyword_args_expanded("math.pow", args)?;
                if args.len() != 2 {
                    return Err(PyError::Runtime(
                        "math.pow() takes exactly two arguments".to_string(),
                    ));
                }
                let x = value_to_float(&args[0].value, "math.pow")?;
                let y = value_to_float(&args[1].value, "math.pow")?;
                Ok(Value::Float(x.powf(y)))
            }
            Value::Builtin("math.atan2") => {
                reject_keyword_args_expanded("math.atan2", args)?;
                if args.len() != 2 {
                    return Err(PyError::Runtime(
                        "math.atan2() takes exactly two arguments".to_string(),
                    ));
                }
                let y = value_to_float(&args[0].value, "math.atan2")?;
                let x = value_to_float(&args[1].value, "math.atan2")?;
                Ok(Value::Float(y.atan2(x)))
            }
            Value::Builtin("math.log") => {
                reject_keyword_args_expanded("math.log", args)?;
                if args.is_empty() || args.len() > 2 {
                    return Err(PyError::Runtime(
                        "math.log() takes one or two arguments".to_string(),
                    ));
                }
                let x = value_to_float(&args[0].value, "math.log")?;
                if args.len() == 2 {
                    let base = value_to_float(&args[1].value, "math.log")?;
                    Ok(Value::Float(x.log(base)))
                } else {
                    Ok(Value::Float(x.ln()))
                }
            }
            Value::Function(function) => self.call_user_function_expanded(function, args, &[]),
            Value::Class(class) => self.call_class_expanded(class, args),
            Value::BoundMethod { function, receiver } => {
                self.call_user_function_expanded(function, args, &[Value::Instance(receiver)])
            }
            _ => Err(PyError::Runtime("object is not callable".to_string())),
        }
    }

    fn expand_call_args(&mut self, args: &[CallArg]) -> Result<Vec<ExpandedCallArg>> {
        let mut out = Vec::new();
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
                let Value::Dict(items) = value else {
                    return Err(PyError::Runtime(
                        "** argument after ** must be a mapping".to_string(),
                    ));
                };
                for (k, v) in items {
                    let PyKey::Str(name) = k else {
                        return Err(PyError::Runtime(
                            "keywords must be strings".to_string(),
                        ));
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

    fn parse_print_options_expanded(&mut self, args: &[ExpandedCallArg]) -> Result<PrintOptions> {
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
                    if value != Value::None {
                        return Err(PyError::Runtime(
                            "print() file argument is not supported yet".to_string(),
                        ));
                    }
                }
                Some("flush") => match value {
                    Value::Bool(_) => {}
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

    fn create_user_function(
        &mut self,
        name: &str,
        params: &[FunctionParam],
        body: &[Stmt],
        closure_env: EnvRef,
    ) -> Result<Rc<UserFunction>> {
        let mut evaluated_params = Vec::with_capacity(params.len());
        for param in params {
            let default = match &param.default {
                Some(expr) => Some(self.eval_expr(expr)?),
                None => None,
            };
            evaluated_params.push(UserFunctionParam {
                name: param.name.clone(),
                default,
                is_args: param.is_args,
                is_kwargs: param.is_kwargs,
            });
        }
        let global_names = collect_global_names(body);
        let nonlocal_names = collect_nonlocal_names(body);
        if let Some(param_name) = params
            .iter()
            .map(|param| &param.name)
            .find(|param_name| global_names.contains(*param_name))
        {
            return Err(PyError::Runtime(format!(
                "name '{}' is parameter and global",
                param_name
            )));
        }
        if let Some(param_name) = params
            .iter()
            .map(|param| &param.name)
            .find(|param_name| nonlocal_names.contains(*param_name))
        {
            return Err(PyError::Runtime(format!(
                "name '{}' is parameter and nonlocal",
                param_name
            )));
        }
        if let Some(name) = nonlocal_names
            .iter()
            .find(|name| global_names.contains(*name))
        {
            return Err(PyError::Runtime(format!(
                "name '{}' is nonlocal and global",
                name
            )));
        }
        if let Some(name) = nonlocal_names
            .iter()
            .find(|name| !has_local_binding_in_current_or_ancestor(&closure_env, name))
        {
            return Err(PyError::Runtime(format!(
                "no binding for nonlocal '{}' found",
                name
            )));
        }

        let local_names = collect_local_names(params, body, &global_names, &nonlocal_names);
        let local_index = Rc::new(
            local_names.iter().enumerate().map(|(i, n)| (n.clone(), i)).collect::<HashMap<String, usize>>()
        );
        let def_bound_mask = compute_def_bound_mask(params, body, &local_index);
        Ok(Rc::new(UserFunction {
            name: name.to_string(),
            params: evaluated_params,
            is_pure: is_pure_body(body),
            body: body.to_vec(),
            local_names: Rc::new(local_names),
            local_index,
            global_names: Rc::new(global_names),
            nonlocal_names: Rc::new(nonlocal_names),
            env: closure_env,
            def_bound_mask,
        }))
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

    fn call_user_function_expanded(
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
            // Resolve all parameter values (apply defaults where needed).
            let mut param_vals: Vec<Value> = Vec::with_capacity(function.params.len());
            for (index, param) in function.params.iter().enumerate() {
                let value = if let Some(v) = &bound_args[index] {
                    v.clone()
                } else {
                    param.default.clone().ok_or_else(|| {
                        PyError::Runtime(format!(
                            "{}() missing required positional argument: '{}'",
                            function.name, param.name
                        ))
                    })?
                };
                param_vals.push(value);
            }

            // Memoization: if the function is pure and all args are hashable,
            // check the cache before building a call frame.
            let cache_key: Option<(usize, Vec<PyKey>)> = if function.is_pure {
                param_vals
                    .iter()
                    .map(|v| v.to_key())
                    .collect::<Option<Vec<PyKey>>>()
                    .map(|keys| (Rc::as_ptr(&function) as usize, keys))
            } else {
                None
            };
            if let Some(ref ck) = cache_key {
                if let Some(cached) = self.fn_cache.get(ck).cloned() {
                    return Ok(cached);
                }
            }

            let fn_ptr = Rc::as_ptr(&function) as usize;

            // Tier-0: register-VM path — try compiled bytecode before any env allocation.
            if let Some(code) = self.get_or_compile_bytecode(fn_ptr, &function) {
                let num_regs = code.num_regs as usize;
                let mut regs: Vec<Option<Value>> = vec![None; num_regs];
                // Bind params into register file using fastlocals slot indices.
                for (param, val) in function.params.iter().zip(param_vals.iter()) {
                    if let Some(&slot) = function.local_index.get(&param.name) {
                        if slot < num_regs {
                            regs[slot] = Some(val.clone());
                        }
                    }
                }
                // Self-reference for recursive calls.
                if let Some(&slot) = function.local_index.get(&function.name) {
                    if slot < num_regs {
                        regs[slot] = Some(Value::Function(Rc::clone(&function)));
                    }
                }
                self.call_depth += 1;
                if self.call_depth > MAX_CALL_DEPTH {
                    self.call_depth -= 1;
                    let exc = self.instantiate_named_exception(
                        "RecursionError",
                        "maximum recursion depth exceeded".to_string(),
                    )?;
                    return Err(PyError::Raised(exc));
                }
                let previous_env = std::mem::replace(&mut self.env, Rc::clone(&function.env));
                let vm_result = self.run_bytecode(&code, &mut regs);
                self.env = previous_env;
                self.call_depth -= 1;
                let value = vm_result?;
                if let Some(ck) = cache_key {
                    self.fn_cache.insert(ck, value.clone());
                }
                return Ok(value);
            }

            // Tier-1: count calls.
            let count = self.call_counts.entry(fn_ptr).or_insert(0);
            *count += 1;

            // Tier-2: promote to hot frame once threshold is reached.
            if *count == HOT_THRESHOLD && is_hot_frame_eligible(&function) {
                // Create the dedicated hot frame once.
                let hot_env = self.alloc_env(Some(Rc::clone(&function.env)));
                {
                    let mut e = hot_env.borrow_mut();
                    e.local_names = Rc::clone(&function.local_names);
                    e.global_names = Rc::clone(&function.global_names);
                    e.nonlocal_names = Rc::clone(&function.nonlocal_names);
                    e.fastlocals = Some(FunctionLocals {
                        slots: vec![None; function.local_index.len()],
                        index: Rc::clone(&function.local_index),
                        def_bound_mask: function.def_bound_mask,
                    });
                    // Insert function name binding.
                    let fn_val = Value::Function(Rc::clone(&function));
                    let fn_name = &function.name;
                    match e.fastlocals.as_mut().and_then(|fl| fl.index.get(fn_name).copied()) {
                        Some(idx) => e.fastlocals.as_mut().unwrap().slots[idx] = Some(fn_val),
                        None => { e.values.insert(fn_name.clone(), fn_val); }
                    }
                }
                self.hot_frames.insert(fn_ptr, hot_env);
            }

            // Use the hot frame if available and not currently active (recursion guard).
            if let Some(hot_env) = self.hot_frames.get(&fn_ptr).cloned() {
                if !self.hot_frames_active.contains(&fn_ptr) {
                    self.hot_frames_active.insert(fn_ptr);

                    // Clear non-function-name slots from the previous call.
                    {
                        let mut e = hot_env.borrow_mut();
                        if let Some(fl) = e.fastlocals.as_mut() {
                            // Clear all slots that aren't the function-name self-reference.
                            let fn_slot = fl.index.get(&function.name).copied();
                            for (i, slot) in fl.slots.iter_mut().enumerate() {
                                if Some(i) != fn_slot {
                                    *slot = None;
                                }
                            }
                        }
                        e.values.retain(|k, _| k == &function.name);
                    }

                    // Bind params into hot frame.
                    {
                        let mut e = hot_env.borrow_mut();
                        for (param, value) in function.params.iter().zip(param_vals) {
                            let fl_idx = e.fastlocals.as_ref().and_then(|fl| fl.index.get(&param.name).copied());
                            match fl_idx {
                                Some(slot) => e.fastlocals.as_mut().unwrap().slots[slot] = Some(value),
                                None => { e.values.insert(param.name.clone(), value); }
                            }
                        }
                    }

                    self.call_depth += 1;
                    if self.call_depth > MAX_CALL_DEPTH {
                        self.call_depth -= 1;
                        self.hot_frames_active.remove(&fn_ptr);
                        let exc = self.instantiate_named_exception(
                            "RecursionError",
                            "maximum recursion depth exceeded".to_string(),
                        )?;
                        return Err(PyError::Raised(exc));
                    }
                    let previous_env = std::mem::replace(&mut self.env, hot_env);
                    let signal = self.exec_block(&function.body);
                    let _ = std::mem::replace(&mut self.env, previous_env);
                    self.call_depth -= 1;
                    self.hot_frames_active.remove(&fn_ptr);

                    let result = match signal? {
                        ExecSignal::None => Value::None,
                        ExecSignal::Return(value) => *value,
                        ExecSignal::Break | ExecSignal::Continue => {
                            return Err(PyError::Runtime(
                                "break/continue is only valid inside loops".to_string(),
                            ))
                        }
                    };
                    if let Some(ck) = cache_key {
                        self.fn_cache.insert(ck, result.clone());
                    }
                    return Ok(result);
                }
            }

            let local_env = self.alloc_env(Some(Rc::clone(&function.env)));
            {
                let mut local_env_ref = local_env.borrow_mut();
                local_env_ref.local_names = Rc::clone(&function.local_names);
                local_env_ref.global_names = Rc::clone(&function.global_names);
                local_env_ref.nonlocal_names = Rc::clone(&function.nonlocal_names);
                if !function.local_index.is_empty() {
                    local_env_ref.fastlocals = Some(FunctionLocals {
                        slots: vec![None; function.local_index.len()],
                        index: Rc::clone(&function.local_index),
                        def_bound_mask: function.def_bound_mask,
                    });
                }
                let fn_val = Value::Function(Rc::clone(&function));
                let fn_name = &function.name;
                match local_env_ref.fastlocals.as_mut().and_then(|fl| fl.index.get(fn_name).copied()) {
                    Some(idx) => local_env_ref.fastlocals.as_mut().unwrap().slots[idx] = Some(fn_val),
                    None => { local_env_ref.values.insert(fn_name.clone(), fn_val); }
                }
                for (param, value) in function.params.iter().zip(param_vals) {
                    let fl_idx = local_env_ref.fastlocals.as_ref().and_then(|fl| fl.index.get(&param.name).copied());
                    match fl_idx {
                        Some(slot) => local_env_ref.fastlocals.as_mut().unwrap().slots[slot] = Some(value),
                        None => { local_env_ref.values.insert(param.name.clone(), value); }
                    }
                }
            }
            self.call_depth += 1;
            if self.call_depth > MAX_CALL_DEPTH {
                self.call_depth -= 1;
                let exc = self.instantiate_named_exception(
                    "RecursionError",
                    "maximum recursion depth exceeded".to_string(),
                )?;
                return Err(PyError::Raised(exc));
            }
            let previous_env = std::mem::replace(&mut self.env, local_env);
            let signal = self.exec_block(&function.body);
            let local_env = std::mem::replace(&mut self.env, previous_env);
            self.call_depth -= 1;
            self.free_env(local_env);
            let result = match signal? {
                ExecSignal::None => Value::None,
                ExecSignal::Return(value) => *value,
                ExecSignal::Break | ExecSignal::Continue => {
                    return Err(PyError::Runtime(
                        "break/continue is only valid inside loops".to_string(),
                    ))
                }
            };
            if let Some(ck) = cache_key {
                self.fn_cache.insert(ck, result.clone());
            }
            return Ok(result);
        }

        // Variadic path: handle *args and **kwargs
        // Evaluate all args
        let mut positional_vals: Vec<Value> = bound_prefix.to_vec();
        let mut keyword_vals: Vec<(String, Value)> = Vec::new();
        for arg in args {
            let value = arg.value.clone();
            if let Some(name) = &arg.name {
                keyword_vals.push((name.clone(), value));
            } else {
                positional_vals.push(value);
            }
        }

        let local_env = self.alloc_env(Some(Rc::clone(&function.env)));
        {
            let mut local_env_ref = local_env.borrow_mut();
            local_env_ref.local_names = Rc::clone(&function.local_names);
            local_env_ref.global_names = Rc::clone(&function.global_names);
            local_env_ref.nonlocal_names = Rc::clone(&function.nonlocal_names);
            if !function.local_index.is_empty() {
                local_env_ref.fastlocals = Some(FunctionLocals {
                    slots: vec![None; function.local_index.len()],
                    index: Rc::clone(&function.local_index),
                    def_bound_mask: function.def_bound_mask,
                });
            }
            let fn_val = Value::Function(Rc::clone(&function));
            let fn_name = &function.name;
            match local_env_ref.fastlocals.as_mut().and_then(|fl| fl.index.get(fn_name).copied()) {
                Some(idx) => local_env_ref.fastlocals.as_mut().unwrap().slots[idx] = Some(fn_val),
                None => { local_env_ref.values.insert(fn_name.clone(), fn_val); }
            }
        }

        let mut pos_idx = 0;
        let mut kwargs_dict: indexmap::IndexMap<crate::value::PyKey, Value> = indexmap::IndexMap::new();

        let has_kwargs = function.params.iter().any(|p| p.is_kwargs);
        let mut consumed_keywords = std::collections::HashSet::new();

        for param in function.params.iter() {
            let value = if param.is_args {
                let rest: Vec<Value> = positional_vals[pos_idx..].to_vec();
                pos_idx = positional_vals.len();
                Value::Tuple(rest)
            } else if param.is_kwargs {
                for (k, v) in &keyword_vals {
                    if let Some(key) = Value::Str(k.clone()).to_key() {
                        kwargs_dict.insert(key, v.clone());
                    }
                }
                Value::Dict(kwargs_dict.clone())
            } else {
                let kw_pos = keyword_vals.iter().position(|(k, _)| k == &param.name);
                if let Some(ki) = kw_pos {
                    consumed_keywords.insert(keyword_vals[ki].0.clone());
                    keyword_vals[ki].1.clone()
                } else if pos_idx < positional_vals.len() {
                    let v = positional_vals[pos_idx].clone();
                    pos_idx += 1;
                    v
                } else if let Some(default) = &param.default {
                    default.clone()
                } else {
                    return Err(PyError::Runtime(format!(
                        "{}() missing required argument: '{}'",
                        function.name, param.name
                    )));
                }
            };
            let mut env = local_env.borrow_mut();
            let fl_idx = env.fastlocals.as_ref().and_then(|fl| fl.index.get(&param.name).copied());
            match fl_idx {
                Some(slot) => env.fastlocals.as_mut().unwrap().slots[slot] = Some(value),
                None => { env.values.insert(param.name.clone(), value); }
            }
        }

        // Check for unexpected keyword arguments if no **kwargs
        if !has_kwargs {
            for (name, _) in &keyword_vals {
                if !consumed_keywords.contains(name) {
                    return Err(PyError::Runtime(format!(
                        "{}() got unexpected keyword argument '{}'",
                        function.name, name
                    )));
                }
            }
        }

        self.call_depth += 1;
        if self.call_depth > MAX_CALL_DEPTH {
            self.call_depth -= 1;
            let exc = self.instantiate_named_exception(
                "RecursionError",
                "maximum recursion depth exceeded".to_string(),
            )?;
            return Err(PyError::Raised(exc));
        }
        let previous_env = std::mem::replace(&mut self.env, local_env);
        let signal = self.exec_block(&function.body);
        let local_env = std::mem::replace(&mut self.env, previous_env);
        self.call_depth -= 1;
        self.free_env(local_env);

        match signal? {
            ExecSignal::None => Ok(Value::None),
            ExecSignal::Return(value) => Ok(*value),
            ExecSignal::Break | ExecSignal::Continue => Err(PyError::Runtime(
                "break/continue is only valid inside loops".to_string(),
            )),
        }
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
            Some(Value::Function(function)) => {
                let result = self.call_user_function_expanded(
                    function,
                    args,
                    &[Value::Instance(Rc::clone(&instance))],
                )?;
                if result != Value::None {
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

        Ok(Value::Instance(instance))
    }

    fn call_range_expanded(&mut self, args: &[ExpandedCallArg]) -> Result<Value> {
        reject_keyword_args_expanded("range", args)?;
        if args.is_empty() || args.len() > 3 {
            return Err(PyError::Runtime(
                "range expected 1 to 3 arguments".to_string(),
            ));
        }

        let mut ints = Vec::with_capacity(args.len());
        for arg in args {
            match arg.value {
                Value::Int(v) => ints.push(v),
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
            return Err(PyError::Runtime(
                "range() arg 3 must not be zero".to_string(),
            ));
        }

        Ok(Value::Range { start, stop, step })
    }

    fn call_instance_method_values(
        &mut self,
        obj: &Value,
        method: &str,
        extra_args: &[Value],
    ) -> Result<Option<Value>> {
        let (func, self_val) = match obj {
            Value::Instance(inst) => {
                let class = Rc::clone(&inst.borrow().class);
                let Some(f) = lookup_class_attr(&class, method) else {
                    return Ok(None);
                };
                (f, Value::Instance(Rc::clone(inst)))
            }
            _ => return Ok(None),
        };
        if let Value::Function(func) = func {
            let mut all_args = vec![self_val];
            all_args.extend_from_slice(extra_args);
            Ok(Some(self.call_user_function(Rc::clone(&func), &[], &all_args)?))
        } else {
            Ok(None)
        }
    }

    /// Return the compiled `FnCode` for `function`, compiling and caching it on first call.
    /// Returns `None` if the function is ineligible for bytecode compilation.
    fn get_or_compile_bytecode(
        &mut self,
        fn_ptr: usize,
        function: &Rc<UserFunction>,
    ) -> Option<Rc<FnCode>> {
        if let Some((weak_fn, entry)) = self.bytecode_cache.get(&fn_ptr) {
            // Guard against stale entries from pointer reuse after drop.
            if weak_fn.upgrade().is_some() {
                return entry.clone();
            }
        }
        let compiled = crate::compiler::compile_fn(function).map(Rc::new);
        self.bytecode_cache.insert(fn_ptr, (Rc::downgrade(function), compiled.clone()));
        compiled
    }

}

fn is_hot_frame_eligible(f: &UserFunction) -> bool {
    f.global_names.is_empty()
        && f.nonlocal_names.is_empty()
        && !f.params.iter().any(|p| p.is_args || p.is_kwargs)
        && !f.local_index.is_empty()
        && f.env.borrow().parent.is_none()
}
