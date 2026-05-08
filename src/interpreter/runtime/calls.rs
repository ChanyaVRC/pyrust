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
                let items = self.iter_values(value)?;
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

        Ok(Rc::new(UserFunction {
            name: name.to_string(),
            params: evaluated_params,
            body: body.to_vec(),
            local_names: collect_local_names(params, body, &global_names, &nonlocal_names),
            global_names,
            nonlocal_names,
            env: closure_env,
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
            let local_env = Environment::new(Some(Rc::clone(&function.env)));
            {
                let mut local_env_ref = local_env.borrow_mut();
                local_env_ref.local_names = function.local_names.clone();
                local_env_ref.global_names = function.global_names.clone();
                local_env_ref.nonlocal_names = function.nonlocal_names.clone();
                local_env_ref.values.insert(function.name.clone(), Value::Function(Rc::clone(&function)));
            }
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
                local_env.borrow_mut().values.insert(param.name.clone(), value);
            }
            let previous_env = std::mem::replace(&mut self.env, local_env);
            let signal = self.exec_block(&function.body);
            self.env = previous_env;
            return match signal? {
                ExecSignal::None => Ok(Value::None),
                ExecSignal::Return(value) => Ok(value),
                ExecSignal::Break | ExecSignal::Continue => Err(PyError::Runtime(
                    "break/continue is only valid inside loops".to_string(),
                )),
            };
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

        let local_env = Environment::new(Some(Rc::clone(&function.env)));
        {
            let mut local_env_ref = local_env.borrow_mut();
            local_env_ref.local_names = function.local_names.clone();
            local_env_ref.global_names = function.global_names.clone();
            local_env_ref.nonlocal_names = function.nonlocal_names.clone();
            local_env_ref.values.insert(function.name.clone(), Value::Function(Rc::clone(&function)));
        }

        let mut pos_idx = 0;
        let mut kwargs_dict: indexmap::IndexMap<crate::value::PyKey, Value> = indexmap::IndexMap::new();

        let has_kwargs = function.params.iter().any(|p| p.is_kwargs);
        let mut consumed_keywords = std::collections::HashSet::new();

        for param in function.params.iter() {
            if param.is_args {
                // Collect remaining positional args
                let rest: Vec<Value> = positional_vals[pos_idx..].to_vec();
                pos_idx = positional_vals.len();
                local_env.borrow_mut().values.insert(param.name.clone(), Value::Tuple(rest));
            } else if param.is_kwargs {
                // Collect remaining keyword args
                for (k, v) in &keyword_vals {
                    if let Some(key) = Value::Str(k.clone()).to_key() {
                        kwargs_dict.insert(key, v.clone());
                    }
                }
                local_env.borrow_mut().values.insert(param.name.clone(), Value::Dict(kwargs_dict.clone()));
            } else {
                // Check if provided by keyword
                let kw_pos = keyword_vals.iter().position(|(k, _)| k == &param.name);
                let value = if let Some(ki) = kw_pos {
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
                };
                local_env.borrow_mut().values.insert(param.name.clone(), value);
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

        let previous_env = std::mem::replace(&mut self.env, local_env);
        let signal = self.exec_block(&function.body);
        self.env = previous_env;

        match signal? {
            ExecSignal::None => Ok(Value::None),
            ExecSignal::Return(value) => Ok(value),
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

}
