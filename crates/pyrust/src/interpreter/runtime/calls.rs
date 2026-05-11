// Thread-local call depth counter. Using thread_local avoids the split-borrow
// problem: a guard that holds &mut self.call_depth cannot coexist with a &mut self
// method call. The thread_local is safe because the interpreter is single-threaded.
use std::cell::Cell;

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

    fn call_function_expanded(
        &mut self,
        function: Value,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        match function.kind() {
            ValueKind::BuiltinFunction("print") => {
                let print_options = self.parse_print_options_expanded(args)?;
                let mut rendered = Vec::with_capacity(print_options.values.len());
                for value in print_options.values {
                    let s = if let ValueKind::PyInstance(inst) = value.kind() {
                        let inst_rc = Rc::clone(inst);
                        let class = Rc::clone(&inst_rc.borrow().class);
                        // Exception instances use the built-in formatting.
                        if is_exception_class(&class) {
                            value.to_py_str()
                        } else {
                            let mut found = None;
                            for dunder in &["__str__", "__repr__"] {
                                if let Some(method_val) = lookup_class_attr(&class, dunder)
                                    && let ValueKind::UserFunction(f) = method_val.kind()
                                {
                                    let func = Rc::clone(f);
                                    let result = self.call_user_function_expanded(
                                        func,
                                        &[],
                                        &[Value::py_instance(Rc::clone(&inst_rc))],
                                    )?;
                                    found = Some(match result.kind() {
                                        ValueKind::Str(s) => s.to_string(),
                                        _ => return Err(PyError::Named(
                                            "TypeError".to_string(),
                                            format!("{dunder} returned non-string"),
                                        )),
                                    });
                                    break;
                                }
                            }
                            found.unwrap_or_else(|| {
                                let class_name = class.borrow().name.clone();
                                format!("<{class_name} object>")
                            })
                        }
                    } else {
                        value.to_py_str()
                    };
                    rendered.push(s);
                }
                print!("{}{}", rendered.join(&print_options.sep), print_options.end);
                Ok(Value::none())
            }
            ValueKind::BuiltinFunction("len") => {
                reject_keyword_args_expanded("len", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "len() takes exactly one argument".to_string(),
                    ));
                }
                let value = args[0].value.clone();
                let size = match value.kind() {
                    ValueKind::Str(text) => text.chars().count() as i64,
                    ValueKind::List(items) => items.len() as i64,
                    ValueKind::Tuple(items) => items.len() as i64,
                    ValueKind::Set(items) => items.len() as i64,
                    ValueKind::Dict(items) => items.len() as i64,
                    ValueKind::Range { start, stop, step } => range_len(start, stop, step),
                    ValueKind::PyInstance(inst) => {
                        let inst_rc = Rc::clone(inst);
                        let class = Rc::clone(&inst_rc.borrow().class);
                        if let Some(method_val) = lookup_class_attr(&class, "__len__") {
                            if let ValueKind::UserFunction(f) = method_val.kind() {
                                let func = Rc::clone(f);
                                let result = self.call_user_function_expanded(
                                    func,
                                    &[],
                                    &[Value::py_instance(inst_rc)],
                                )?;
                                match result.kind() {
                                    ValueKind::Int(n) if n >= 0 => n,
                                    ValueKind::Int(_) => return Err(PyError::Named(
                                        "ValueError".to_string(),
                                        "__len__() should return >= 0".to_string(),
                                    )),
                                    ValueKind::Bool(b) => if b { 1 } else { 0 },
                                    _ => return Err(PyError::Named(
                                        "TypeError".to_string(),
                                        "__len__ returned non-int".to_string(),
                                    )),
                                }
                            } else {
                                return Err(PyError::Runtime("object has no len()".to_string()));
                            }
                        } else {
                            return Err(PyError::Runtime("object has no len()".to_string()));
                        }
                    }
                    _ => {
                        return Err(PyError::Runtime("object has no len()".to_string()));
                    }
                };
                Ok(Value::int(size))
            }
            ValueKind::BuiltinFunction("range") => self.call_range_expanded(args),

            ValueKind::BuiltinFunction("enumerate") => {
                reject_keyword_args_expanded("enumerate", args)?;
                if args.is_empty() || args.len() > 2 {
                    return Err(PyError::Runtime(
                        "enumerate() takes 1 or 2 arguments".to_string(),
                    ));
                }
                let start = if args.len() == 2 {
                    match args[1].value.kind() {
                        ValueKind::Int(n) => n,
                        _ => return Err(PyError::Runtime(
                            "enumerate() start argument must be an integer".to_string(),
                        )),
                    }
                } else {
                    0i64
                };
                Ok(Value::lazy_enumerate(args[0].value.clone(), start))
            }

            ValueKind::BuiltinFunction("zip") => {
                reject_keyword_args_expanded("zip", args)?;
                let sources = args.iter().map(|a| a.value.clone()).collect();
                Ok(Value::lazy_zip(sources))
            }

            ValueKind::BuiltinFunction("reversed") => {
                reject_keyword_args_expanded("reversed", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "reversed() takes exactly one argument".to_string(),
                    ));
                }
                Ok(Value::lazy_reversed(args[0].value.clone()))
            }

            ValueKind::BuiltinFunction("sorted") => {
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
                    let mut sort_err: Option<PyError> = None;
                    keyed.sort_by(|(a, _), (b, _)| {
                        if sort_err.is_some() { return std::cmp::Ordering::Equal; }
                        match compare_values(a, b) {
                            Ok(ord) => ord,
                            Err(e) => { sort_err = Some(e); std::cmp::Ordering::Equal }
                        }
                    });
                    if let Some(e) = sort_err { return Err(e); }
                    items = keyed.into_iter().map(|(_, v)| v).collect();
                } else {
                    let mut sort_err: Option<PyError> = None;
                    items.sort_by(|a, b| {
                        if sort_err.is_some() { return std::cmp::Ordering::Equal; }
                        match compare_values(a, b) {
                            Ok(ord) => ord,
                            Err(e) => { sort_err = Some(e); std::cmp::Ordering::Equal }
                        }
                    });
                    if let Some(e) = sort_err { return Err(e); }
                }
                if reverse {
                    items.reverse();
                }
                Ok(Value::list(items))
            }

            ValueKind::BuiltinFunction("abs") => {
                reject_keyword_args_expanded("abs", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "abs() takes exactly one argument".to_string(),
                    ));
                }
                let val = args[0].value.clone();
                if let ValueKind::PyInstance(inst) = val.kind() {
                    let inst_rc = Rc::clone(inst);
                    let class = Rc::clone(&inst_rc.borrow().class);
                    if let Some(method_val) = lookup_class_attr(&class, "__abs__")
                        && let ValueKind::UserFunction(f) = method_val.kind()
                    {
                        let func = Rc::clone(f);
                        return self.call_user_function_expanded(
                            func,
                            &[],
                            &[Value::py_instance(inst_rc)],
                        );
                    }
                    return Err(PyError::Named(
                        "TypeError".to_string(),
                        format!(
                            "bad operand type for abs(): '{}'",
                            class.borrow().name
                        ),
                    ));
                }
                match val.kind() {
                    ValueKind::Int(v) => Ok(Value::int(v.abs())),
                    ValueKind::Float(v) => Ok(Value::float(v.abs())),
                    ValueKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
                    _ => Err(PyError::Runtime(
                        "abs() argument must be a number".to_string(),
                    )),
                }
            }

            ValueKind::BuiltinFunction("min") | ValueKind::BuiltinFunction("max") => {
                let is_max = matches!(function.kind(), ValueKind::BuiltinFunction("max"));
                let fname = if is_max { "max" } else { "min" };
                let key_fn = args.iter().find(|a| a.name.as_deref() == Some("key"))
                    .map(|a| a.value.clone());
                for a in args.iter().filter(|a| a.name.is_some()) {
                    if a.name.as_deref() != Some("key") {
                        return Err(PyError::Runtime(format!(
                            "{fname}() got an unexpected keyword argument '{}'",
                            a.name.as_ref().unwrap()
                        )));
                    }
                }
                let positional: Vec<&ExpandedCallArg> =
                    args.iter().filter(|a| a.name.is_none()).collect();
                let items: Vec<Value> = if positional.len() == 1 {
                    iter_values(positional[0].value.clone())?
                } else if positional.len() >= 2 {
                    positional.iter().map(|a| a.value.clone()).collect()
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
                if let Some(kfn) = key_fn {
                    let keyed: Vec<(Value, Value)> = items
                        .into_iter()
                        .map(|v| {
                            let k = self.call_function_expanded(
                                kfn.clone(),
                                &[ExpandedCallArg { name: None, value: v.clone() }],
                            )?;
                            Ok((k, v))
                        })
                        .collect::<Result<_>>()?;
                    let mut result_err: Option<PyError> = None;
                    let result = keyed.into_iter().reduce(|acc, item| {
                        if result_err.is_some() { return acc; }
                        match compare_values(&item.0, &acc.0) {
                            Ok(cmp) => {
                                if (is_max && cmp == std::cmp::Ordering::Greater)
                                    || (!is_max && cmp == std::cmp::Ordering::Less) { item }
                                else { acc }
                            }
                            Err(e) => { result_err = Some(e); acc }
                        }
                    }).unwrap();
                    if let Some(e) = result_err { return Err(e); }
                    Ok(result.1)
                } else {
                    let mut result_err: Option<PyError> = None;
                    let result = items.into_iter().reduce(|acc, v| {
                        if result_err.is_some() { return acc; }
                        match compare_values(&v, &acc) {
                            Ok(cmp) => {
                                if (is_max && cmp == std::cmp::Ordering::Greater)
                                    || (!is_max && cmp == std::cmp::Ordering::Less) { v }
                                else { acc }
                            }
                            Err(e) => { result_err = Some(e); acc }
                        }
                    }).unwrap();
                    if let Some(e) = result_err { return Err(e); }
                    Ok(result)
                }
            }

            ValueKind::BuiltinFunction("sum") => {
                reject_keyword_args_expanded("sum", args)?;
                if args.is_empty() || args.len() > 2 {
                    return Err(PyError::Runtime(
                        "sum() takes 1 or 2 arguments".to_string(),
                    ));
                }
                let items = iter_values(args[0].value.clone())?;
                let start = if args.len() == 2 { args[1].value.clone() } else { Value::int(0) };
                let mut acc = start;
                for item in items {
                    acc = self.eval_binary(acc, BinaryOp::Add, item)?;
                }
                Ok(acc)
            }

            ValueKind::BuiltinFunction("list") => {
                reject_keyword_args_expanded("list", args)?;
                match args.len() {
                    0 => Ok(Value::list(vec![])),
                    1 => Ok(Value::list(self.collect_iterable(args[0].value.clone())?)),
                    _ => Err(PyError::Runtime("list() takes at most one argument".to_string())),
                }
            }

            ValueKind::BuiltinFunction("tuple") => {
                reject_keyword_args_expanded("tuple", args)?;
                match args.len() {
                    0 => Ok(Value::tuple(vec![])),
                    1 => Ok(Value::tuple(self.collect_iterable(args[0].value.clone())?)),
                    _ => Err(PyError::Runtime("tuple() takes at most one argument".to_string())),
                }
            }

            ValueKind::BuiltinFunction("set") => {
                reject_keyword_args_expanded("set", args)?;
                match args.len() {
                    0 => Ok(Value::set(indexmap::IndexSet::new())),
                    1 => {
                        let items = self.collect_iterable(args[0].value.clone())?;
                        let mut set = indexmap::IndexSet::new();
                        for item in items {
                            let key = item.to_key().ok_or_else(|| {
                                PyError::Runtime("unhashable type in set".to_string())
                            })?;
                            set.insert(key);
                        }
                        Ok(Value::set(set))
                    }
                    _ => Err(PyError::Runtime("set() takes at most one argument".to_string())),
                }
            }

            ValueKind::BuiltinFunction("str") => {
                reject_keyword_args_expanded("str", args)?;
                match args.len() {
                    0 => Ok(Value::string(String::new())),
                    1 => {
                        let val = args[0].value.clone();
                        // Try __str__ on PyInstance, fall back to __repr__, then default.
                        // Exception instances use the built-in to_py_str() formatting.
                        if let ValueKind::PyInstance(inst) = val.kind() {
                            let inst_rc = Rc::clone(inst);
                            let class = Rc::clone(&inst_rc.borrow().class);
                            if is_exception_class(&class) {
                                return Ok(Value::string(val.to_py_str()));
                            }
                            for dunder in &["__str__", "__repr__"] {
                                if let Some(method_val) = lookup_class_attr(&class, dunder)
                                    && let ValueKind::UserFunction(f) = method_val.kind()
                                {
                                    let func = Rc::clone(f);
                                    let result = self.call_user_function_expanded(
                                        func,
                                        &[],
                                        &[Value::py_instance(Rc::clone(&inst_rc))],
                                    )?;
                                    return match result.kind() {
                                        ValueKind::Str(_) => Ok(result),
                                        _ => Err(PyError::Named(
                                            "TypeError".to_string(),
                                            format!("{dunder} returned non-string"),
                                        )),
                                    };
                                }
                            }
                            let class_name = class.borrow().name.clone();
                            return Ok(Value::string(format!("<{class_name} object>")));
                        }
                        Ok(Value::string(val.to_py_str()))
                    }
                    _ => Err(PyError::Runtime("str() takes at most one argument".to_string())),
                }
            }

            ValueKind::BuiltinFunction("int") => {
                reject_keyword_args_expanded("int", args)?;
                match args.len() {
                    0 => Ok(Value::int(0)),
                    1 => match args[0].value.kind() {
                        ValueKind::Int(v) => Ok(Value::int(v)),
                        ValueKind::Float(v) => Ok(Value::int(v as i64)),
                        ValueKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
                        ValueKind::Str(s) => s.trim().parse::<i64>().map(Value::int).map_err(|_| {
                            PyError::Named(
                                "ValueError".to_string(),
                                format!("invalid literal for int() with base 10: '{s}'"),
                            )
                        }),
                        _ => Err(PyError::Runtime(
                            "int() argument must be a number or string".to_string(),
                        )),
                    },
                    2 => {
                        let base = match args[1].value.kind() {
                            ValueKind::Int(b) if (2..=36).contains(&b) => b as u32,
                            ValueKind::Int(b) => return Err(PyError::Named(
                                "ValueError".to_string(),
                                format!("int() base must be >= 2 and <= 36, or 0, not {b}"))),
                            _ => return Err(PyError::Runtime("int() base must be an integer".to_string())),
                        };
                        match args[0].value.kind() {
                            ValueKind::Str(s) => {
                                let stripped = s.trim();
                                let stripped = if (base == 16 && (stripped.starts_with("0x") || stripped.starts_with("0X")))
                                    || (base == 2 && (stripped.starts_with("0b") || stripped.starts_with("0B")))
                                    || (base == 8 && (stripped.starts_with("0o") || stripped.starts_with("0O"))) {
                                    &stripped[2..]
                                } else {
                                    stripped
                                };
                                i64::from_str_radix(stripped, base)
                                    .map(Value::int)
                                    .map_err(|_| PyError::Named(
                                        "ValueError".to_string(),
                                        format!("invalid literal for int() with base {base}: '{}'", s.trim()),
                                    ))
                            }
                            _ => Err(PyError::Runtime("int() can't convert non-string with explicit base".to_string())),
                        }
                    }
                    _ => Err(PyError::Runtime("int() takes at most two arguments".to_string())),
                }
            }

            ValueKind::BuiltinFunction("float") => {
                reject_keyword_args_expanded("float", args)?;
                match args.len() {
                    0 => Ok(Value::float(0.0)),
                    1 => match args[0].value.kind() {
                        ValueKind::Float(v) => Ok(Value::float(v)),
                        ValueKind::Int(v) => Ok(Value::float(v as f64)),
                        ValueKind::Bool(b) => Ok(Value::float(if b { 1.0 } else { 0.0 })),
                        ValueKind::Str(s) => s.trim().parse::<f64>().map(Value::float).map_err(|_| {
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

            ValueKind::BuiltinFunction("bool") => {
                reject_keyword_args_expanded("bool", args)?;
                match args.len() {
                    0 => Ok(Value::bool_(false)),
                    1 => {
                        let val = args[0].value.clone();
                        let result = self.truthy_value(&val)?;
                        Ok(Value::bool_(result))
                    }
                    _ => Err(PyError::Runtime("bool() takes at most one argument".to_string())),
                }
            }

            ValueKind::BuiltinFunction("sys.exit") => {
                reject_keyword_args_expanded("sys.exit", args)?;
                let arg = if args.is_empty() {
                    Value::int(0)
                } else {
                    args[0].value.clone()
                };
                // Raise SystemExit like CPython — lets finally/with handlers run.
                // Look up the SystemExit class and instantiate it with the original arg
                // so program.rs can extract the integer exit code without reparsing a string.
                let class = match lookup_name_in_module(&self.env, "SystemExit") {
                    Some(v) => match v.kind() {
                        ValueKind::PyClass(c) => Rc::clone(c),
                        _ => return Err(PyError::Runtime(
                            "built-in exception 'SystemExit' is not defined".to_string(),
                        )),
                    },
                    None => return Err(PyError::Runtime(
                        "built-in exception 'SystemExit' is not defined".to_string(),
                    )),
                };
                let exc = instantiate_exception(class, vec![arg]);
                Err(PyError::Raised(exc))
            }
            ValueKind::BuiltinFunction(
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
                    "math.floor" => {
                        let f = x.floor();
                        if f > i64::MAX as f64 || f < i64::MIN as f64 {
                            Ok(float_to_bigint(f))
                        } else {
                            Ok(Value::int(f as i64))
                        }
                    }
                    "math.ceil" => {
                        let f = x.ceil();
                        if f > i64::MAX as f64 || f < i64::MIN as f64 {
                            Ok(float_to_bigint(f))
                        } else {
                            Ok(Value::int(f as i64))
                        }
                    }
                    "math.sqrt" => Ok(Value::float(x.sqrt())),
                    "math.fabs" => Ok(Value::float(x.abs())),
                    "math.sin" => Ok(Value::float(x.sin())),
                    "math.cos" => Ok(Value::float(x.cos())),
                    "math.tan" => Ok(Value::float(x.tan())),
                    "math.asin" => Ok(Value::float(x.asin())),
                    "math.acos" => Ok(Value::float(x.acos())),
                    "math.atan" => Ok(Value::float(x.atan())),
                    "math.exp" => Ok(Value::float(x.exp())),
                    "math.log2" => Ok(Value::float(x.log2())),
                    "math.log10" => Ok(Value::float(x.log10())),
                    "math.isnan" => Ok(Value::bool_(x.is_nan())),
                    "math.isinf" => Ok(Value::bool_(x.is_infinite())),
                    _ => unreachable!(),
                }
            }
            ValueKind::BuiltinFunction("math.pow") => {
                reject_keyword_args_expanded("math.pow", args)?;
                if args.len() != 2 {
                    return Err(PyError::Runtime(
                        "math.pow() takes exactly two arguments".to_string(),
                    ));
                }
                let x = value_to_float(&args[0].value, "math.pow")?;
                let y = value_to_float(&args[1].value, "math.pow")?;
                Ok(Value::float(x.powf(y)))
            }
            ValueKind::BuiltinFunction("math.atan2") => {
                reject_keyword_args_expanded("math.atan2", args)?;
                if args.len() != 2 {
                    return Err(PyError::Runtime(
                        "math.atan2() takes exactly two arguments".to_string(),
                    ));
                }
                let y = value_to_float(&args[0].value, "math.atan2")?;
                let x = value_to_float(&args[1].value, "math.atan2")?;
                Ok(Value::float(y.atan2(x)))
            }
            ValueKind::BuiltinFunction("math.log") => {
                reject_keyword_args_expanded("math.log", args)?;
                if args.is_empty() || args.len() > 2 {
                    return Err(PyError::Runtime(
                        "math.log() takes one or two arguments".to_string(),
                    ));
                }
                let x = value_to_float(&args[0].value, "math.log")?;
                if args.len() == 2 {
                    let base = value_to_float(&args[1].value, "math.log")?;
                    Ok(Value::float(x.log(base)))
                } else {
                    Ok(Value::float(x.ln()))
                }
            }
            ValueKind::BuiltinFunction("__vcall__") => {
                if args.len() != 3 {
                    return Err(PyError::Runtime("__vcall__ requires 3 arguments".to_string()));
                }
                let func = args[0].value.clone();
                let pos_items = iter_values(args[1].value.clone())?;
                let mut expanded: Vec<ExpandedCallArg> = pos_items
                    .into_iter()
                    .map(|v| ExpandedCallArg { name: None, value: v })
                    .collect();
                if let ValueKind::Dict(kw_map) = args[2].value.kind() {
                    for (k, v) in kw_map {
                        if let PyKey::Str(name) = k {
                            expanded.push(ExpandedCallArg { name: Some(name.clone()), value: v.clone() });
                        }
                    }
                }
                self.call_function_expanded(func, &expanded)
            }
            ValueKind::BuiltinFunction("isinstance") => {
                reject_keyword_args_expanded("isinstance", args)?;
                if args.len() != 2 {
                    return Err(PyError::Runtime(
                        "isinstance() takes exactly 2 arguments".to_string(),
                    ));
                }
                let obj = &args[0].value;
                let cls = &args[1].value;
                let result = match (obj.kind(), cls.kind()) {
                    (ValueKind::PyInstance(inst), ValueKind::PyClass(expected)) => {
                        class_is_subclass_of(&inst.borrow().class, expected)
                    }
                    (ValueKind::Int(_) | ValueKind::Bool(_), ValueKind::BuiltinFunction("int")) => true,
                    (ValueKind::Float(_), ValueKind::BuiltinFunction("float")) => true,
                    (ValueKind::Str(_), ValueKind::BuiltinFunction("str")) => true,
                    (ValueKind::Bool(_), ValueKind::BuiltinFunction("bool")) => true,
                    (ValueKind::None, ValueKind::BuiltinFunction("NoneType")) => true,
                    (ValueKind::List(_), ValueKind::BuiltinFunction("list")) => true,
                    (ValueKind::Tuple(_), ValueKind::BuiltinFunction("tuple")) => true,
                    (ValueKind::Set(_), ValueKind::BuiltinFunction("set")) => true,
                    (ValueKind::Dict(_), ValueKind::BuiltinFunction("dict")) => true,
                    _ => false,
                };
                Ok(Value::bool_(result))
            }
            ValueKind::BuiltinFunction("type") => {
                reject_keyword_args_expanded("type", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "type() takes exactly 1 argument (or 3 for type creation)".to_string(),
                    ));
                }
                let obj = &args[0].value;
                // For user-defined class instances return the actual Rc so that
                // `type(x) is type(x)` works via Rc::ptr_eq in values_are_identical.
                // For builtin types return a BuiltinFunction value (singleton-like) so
                // that `type(5) is type(5)` works and isinstance(x, type(x)) succeeds.
                match obj.kind() {
                    ValueKind::PyInstance(inst) => Ok(Value::py_class(Rc::clone(&inst.borrow().class))),
                    ValueKind::PyClass(_) => Ok(Value::builtin_function("type")),
                    ValueKind::Bool(_) => Ok(Value::builtin_function("bool")),
                    ValueKind::Int(_) => Ok(Value::builtin_function("int")),
                    ValueKind::Float(_) => Ok(Value::builtin_function("float")),
                    ValueKind::Str(_) => Ok(Value::builtin_function("str")),
                    ValueKind::None => Ok(Value::builtin_function("NoneType")),
                    ValueKind::List(_) => Ok(Value::builtin_function("list")),
                    ValueKind::Tuple(_) => Ok(Value::builtin_function("tuple")),
                    ValueKind::Dict(_) => Ok(Value::builtin_function("dict")),
                    ValueKind::DictKeysView(_) => Ok(Value::builtin_function("dict_keys")),
                    ValueKind::DictValuesView(_) => Ok(Value::builtin_function("dict_values")),
                    ValueKind::DictItemsView(_) => Ok(Value::builtin_function("dict_items")),
                    ValueKind::Set(_) => Ok(Value::builtin_function("set")),
                    ValueKind::Range { .. } => Ok(Value::builtin_function("range")),
                    ValueKind::UserFunction(_)
                    | ValueKind::BoundMethod { .. }
                    | ValueKind::ClassBoundMethod { .. } => Ok(Value::builtin_function("function")),
                    ValueKind::BuiltinFunction(_) => Ok(Value::builtin_function("builtin_function_or_method")),
                    ValueKind::PyModule(_) => Ok(Value::builtin_function("module")),
                    ValueKind::BigInt(_) => Ok(Value::builtin_function("int")),
                    ValueKind::Enumerate { .. } => Ok(Value::builtin_function("enumerate")),
                    ValueKind::Zip { .. } => Ok(Value::builtin_function("zip")),
                    ValueKind::Reversed { .. } => Ok(Value::builtin_function("reversed")),
                    ValueKind::ClassMethod(_) | ValueKind::StaticMethod(_) => Ok(Value::builtin_function("function")),
                    ValueKind::SuperProxy { .. } | ValueKind::SuperProxyClass { .. } => Ok(Value::builtin_function("super")),
                    ValueKind::Generator(_) => Ok(Value::builtin_function("generator")),
                }
            }
            ValueKind::BuiltinFunction("id") => {
                reject_keyword_args_expanded("id", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "id() takes exactly 1 argument".to_string(),
                    ));
                }
                let id_val: i64 = match args[0].value.kind() {
                    ValueKind::PyInstance(rc) => Rc::as_ptr(rc) as i64,
                    ValueKind::PyClass(rc) => Rc::as_ptr(rc) as i64,
                    ValueKind::PyModule(rc) => Rc::as_ptr(rc) as i64,
                    ValueKind::UserFunction(rc) => Rc::as_ptr(rc) as i64,
                    ValueKind::Int(n) => n,
                    ValueKind::Bool(b) => b as i64,
                    ValueKind::None => 0,
                    _ => args[0].value.value_id().unwrap_or(0),
                };
                Ok(Value::int(id_val))
            }
            ValueKind::BuiltinFunction("hasattr") => {
                reject_keyword_args_expanded("hasattr", args)?;
                if args.len() != 2 {
                    return Err(PyError::Runtime(
                        "hasattr() takes exactly 2 arguments".to_string(),
                    ));
                }
                let name = match args[1].value.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Err(PyError::Named(
                        "TypeError".to_string(),
                        "hasattr(): attribute name must be a string".to_string(),
                    )),
                };
                let result = match self.get_attr(args[0].value.clone(), &name) {
                    Ok(_) => true,
                    Err(PyError::Named(ref cls, _)) if cls == "AttributeError" => false,
                    Err(e) => return Err(e),
                };
                Ok(Value::bool_(result))
            }
            ValueKind::BuiltinFunction("getattr") => {
                reject_keyword_args_expanded("getattr", args)?;
                if args.len() < 2 || args.len() > 3 {
                    return Err(PyError::Runtime(
                        "getattr() takes 2 or 3 arguments".to_string(),
                    ));
                }
                let name = match args[1].value.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Err(PyError::Named(
                        "TypeError".to_string(),
                        "getattr(): attribute name must be a string".to_string(),
                    )),
                };
                match self.get_attr(args[0].value.clone(), &name) {
                    Ok(v) => Ok(v),
                    Err(PyError::Named(ref cls, _)) if cls == "AttributeError" && args.len() == 3 => {
                        Ok(args[2].value.clone())
                    }
                    Err(e) => Err(e),
                }
            }
            ValueKind::BuiltinFunction("setattr") => {
                reject_keyword_args_expanded("setattr", args)?;
                if args.len() != 3 {
                    return Err(PyError::Runtime(
                        "setattr() takes exactly 3 arguments".to_string(),
                    ));
                }
                let name = match args[1].value.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Err(PyError::Named(
                        "TypeError".to_string(),
                        "setattr(): attribute name must be a string".to_string(),
                    )),
                };
                self.assign_attr(args[0].value.clone(), &name, args[2].value.clone())?;
                Ok(Value::none())
            }
            ValueKind::BuiltinFunction("dict") => {
                reject_keyword_args_expanded("dict", args)?;
                if args.is_empty() {
                    Ok(Value::dict(indexmap::IndexMap::new()))
                } else {
                    Err(PyError::Named(
                        "TypeError".to_string(),
                        "dict() with arguments is not yet supported".to_string(),
                    ))
                }
            }
            ValueKind::BuiltinFunction(name) if name.starts_with("str.") => {
                let method = &name[4..];
                let self_val = args
                    .first()
                    .map(|a| &a.value)
                    .ok_or_else(|| PyError::Named(
                        "TypeError".to_string(),
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
            ValueKind::BuiltinFunction("format") => {
                reject_keyword_args_expanded("format", args)?;
                let (value, spec) = match args.len() {
                    1 => (args[0].value.clone(), String::new()),
                    2 => {
                        let value = args[0].value.clone();
                        let spec = match args[1].value.kind() {
                            ValueKind::Str(s) => s.to_string(),
                            _ => {
                                return Err(PyError::Runtime(
                                    "format spec must be a string".to_string(),
                                ))
                            }
                        };
                        (value, spec)
                    }
                    _ => {
                        return Err(PyError::Runtime(
                            "format() takes 1 or 2 arguments".to_string(),
                        ))
                    }
                };
                // Dispatch __format__(spec) for user instances.
                if let ValueKind::PyInstance(instance) = value.kind() {
                    let instance_rc = Rc::clone(instance);
                    let class = Rc::clone(&instance_rc.borrow().class);
                    if let Some(method_val) = lookup_class_attr(&class, "__format__")
                        && let ValueKind::UserFunction(f) = method_val.kind()
                    {
                        let func = Rc::clone(f);
                        let spec_val = Value::string(spec.clone());
                        let result = self.call_user_function_expanded(
                            func,
                            &[],
                            &[Value::py_instance(instance_rc), spec_val],
                        )?;
                        return match result.kind() {
                            ValueKind::Str(_) => Ok(result),
                            _ => Err(PyError::Named(
                                "TypeError".to_string(),
                                format!(
                                    "__format__ must return a str, not {}",
                                    value_type_name_str(&result)
                                ),
                            )),
                        };
                    }
                }
                apply_format_spec(&value, &spec)
            }

            ValueKind::BuiltinFunction("repr") => {
                reject_keyword_args_expanded("repr", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "repr() takes exactly one argument".to_string(),
                    ));
                }
                let obj = args[0].value.clone();
                // Check for user-defined __repr__ method on instances.
                if let ValueKind::PyInstance(instance) = obj.kind() {
                    let instance_rc = Rc::clone(instance);
                    let class = Rc::clone(&instance_rc.borrow().class);
                    if let Some(method_val) = lookup_class_attr(&class, "__repr__")
                        && let ValueKind::UserFunction(f) = method_val.kind() {
                            let func = Rc::clone(f);
                            let result = self.call_user_function_expanded(
                                func,
                                &[],
                                &[Value::py_instance(instance_rc)],
                            )?;
                            return match result.kind() {
                                ValueKind::Str(_) => Ok(result),
                                _ => Err(PyError::Named(
                                    "TypeError".to_string(),
                                    "__repr__ returned non-string".to_string(),
                                )),
                            };
                        }
                }
                Ok(Value::string(obj.repr()))
            }

            ValueKind::BuiltinFunction("ascii") => {
                reject_keyword_args_expanded("ascii", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "ascii() takes exactly one argument".to_string(),
                    ));
                }
                // Get repr first, then escape any non-ASCII characters.
                let repr_str = args[0].value.repr();
                let escaped: String = repr_str
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
                    .collect();
                Ok(Value::string(escaped))
            }

            ValueKind::BuiltinFunction("any") => {
                reject_keyword_args_expanded("any", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "any() takes exactly one argument".to_string(),
                    ));
                }
                let items = iter_values(args[0].value.clone())?;
                for item in items {
                    if item.truthy() {
                        return Ok(Value::bool_(true));
                    }
                }
                Ok(Value::bool_(false))
            }

            ValueKind::BuiltinFunction("all") => {
                reject_keyword_args_expanded("all", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "all() takes exactly one argument".to_string(),
                    ));
                }
                let items = iter_values(args[0].value.clone())?;
                for item in items {
                    if !item.truthy() {
                        return Ok(Value::bool_(false));
                    }
                }
                Ok(Value::bool_(true))
            }

            ValueKind::BuiltinFunction("map") => {
                reject_keyword_args_expanded("map", args)?;
                if args.len() != 2 {
                    return Err(PyError::Runtime(
                        "map() takes exactly 2 arguments".to_string(),
                    ));
                }
                let func = args[0].value.clone();
                let items = iter_values(args[1].value.clone())?;
                let mut result = Vec::with_capacity(items.len());
                for item in items {
                    let mapped = self.call_function_expanded(
                        func.clone(),
                        &[ExpandedCallArg { name: None, value: item }],
                    )?;
                    result.push(mapped);
                }
                Ok(Value::list(result))
            }

            ValueKind::BuiltinFunction("filter") => {
                reject_keyword_args_expanded("filter", args)?;
                if args.len() != 2 {
                    return Err(PyError::Runtime(
                        "filter() takes exactly 2 arguments".to_string(),
                    ));
                }
                let func = args[0].value.clone();
                let items = iter_values(args[1].value.clone())?;
                let use_identity = func.is_none();
                let mut result = Vec::new();
                for item in items {
                    let keep = if use_identity {
                        item.truthy()
                    } else {
                        let test = self.call_function_expanded(
                            func.clone(),
                            &[ExpandedCallArg { name: None, value: item.clone() }],
                        )?;
                        test.truthy()
                    };
                    if keep {
                        result.push(item);
                    }
                }
                Ok(Value::list(result))
            }

            ValueKind::BuiltinFunction("callable") => {
                reject_keyword_args_expanded("callable", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "callable() takes exactly one argument".to_string(),
                    ));
                }
                let is_callable = match args[0].value.kind() {
                    ValueKind::UserFunction(_)
                    | ValueKind::BuiltinFunction(_)
                    | ValueKind::BoundMethod { .. }
                    | ValueKind::ClassBoundMethod { .. }
                    | ValueKind::PyClass(_)
                    | ValueKind::ClassMethod(_)
                    | ValueKind::StaticMethod(_) => true,
                    ValueKind::PyInstance(inst) => {
                        let class = Rc::clone(&inst.borrow().class);
                        lookup_class_attr(&class, "__call__").is_some()
                    }
                    _ => false,
                };
                Ok(Value::bool_(is_callable))
            }

            ValueKind::BuiltinFunction("round") => {
                reject_keyword_args_expanded("round", args)?;
                if args.is_empty() || args.len() > 2 {
                    return Err(PyError::Runtime(
                        "round() takes 1 or 2 arguments".to_string(),
                    ));
                }
                let ndigits: Option<i32> = if args.len() == 2 {
                    match args[1].value.kind() {
                        ValueKind::Int(n) => Some(n as i32),
                        ValueKind::None => None,
                        _ => return Err(PyError::Named(
                            "TypeError".to_string(),
                            "round() ndigits must be an integer or None".to_string(),
                        )),
                    }
                } else {
                    None
                };
                match args[0].value.kind() {
                    ValueKind::Int(v) => {
                        // round(int) always returns int
                        Ok(Value::int(v))
                    }
                    ValueKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
                    ValueKind::Float(v) => {
                        match ndigits {
                            None => {
                                // round to nearest even int
                                let rounded = py_round_half_even(v);
                                Ok(Value::int(rounded))
                            }
                            Some(n) => {
                                if n >= 0 {
                                    let factor = 10f64.powi(n);
                                    Ok(Value::float(py_round_half_even_f64(v * factor) / factor))
                                } else {
                                    let factor = 10f64.powi(-n);
                                    Ok(Value::float(py_round_half_even_f64(v / factor) * factor))
                                }
                            }
                        }
                    }
                    _ => Err(PyError::Named(
                        "TypeError".to_string(),
                        "round() argument must be a number".to_string(),
                    )),
                }
            }

            ValueKind::BuiltinFunction("classmethod") => {
                reject_keyword_args_expanded("classmethod", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "classmethod() takes exactly one argument".to_string(),
                    ));
                }
                match args[0].value.kind() {
                    ValueKind::UserFunction(f) => Ok(Value::class_method(Rc::clone(f))),
                    _ => Err(PyError::Runtime(
                        "classmethod() argument must be a function".to_string(),
                    )),
                }
            }

            ValueKind::BuiltinFunction("divmod") => {
                reject_keyword_args_expanded("divmod", args)?;
                if args.len() != 2 {
                    return Err(PyError::Runtime(
                        "divmod() takes exactly 2 arguments".to_string(),
                    ));
                }
                match (args[0].value.kind(), args[1].value.kind()) {
                    (ValueKind::Int(a), ValueKind::Int(b)) => {
                        if b == 0 {
                            return Err(PyError::Named(
                                "ZeroDivisionError".to_string(),
                                "integer division or modulo by zero".to_string(),
                            ));
                        }
                        let modulo = py_mod_i64(a, b);
                        let quotient = (a - modulo) / b;
                        Ok(Value::tuple(vec![Value::int(quotient), Value::int(modulo)]))
                    }
                    (ValueKind::Bool(a), ValueKind::Bool(b)) => {
                        let a = a as i64;
                        let b = b as i64;
                        if b == 0 {
                            return Err(PyError::Named(
                                "ZeroDivisionError".to_string(),
                                "integer division or modulo by zero".to_string(),
                            ));
                        }
                        let modulo = py_mod_i64(a, b);
                        let quotient = (a - modulo) / b;
                        Ok(Value::tuple(vec![Value::int(quotient), Value::int(modulo)]))
                    }
                    _ => {
                        let a = value_to_float(&args[0].value, "divmod")?;
                        let b = value_to_float(&args[1].value, "divmod")?;
                        if b == 0.0 {
                            return Err(PyError::Named(
                                "ZeroDivisionError".to_string(),
                                "float divmod()".to_string(),
                            ));
                        }
                        let quotient = (a / b).floor();
                        let modulo = a - b * quotient;
                        Ok(Value::tuple(vec![Value::float(quotient), Value::float(modulo)]))
                    }
                }
            }

            ValueKind::BuiltinFunction("pow") => {
                reject_keyword_args_expanded("pow", args)?;
                if args.len() < 2 || args.len() > 3 {
                    return Err(PyError::Runtime(
                        "pow() takes 2 or 3 arguments".to_string(),
                    ));
                }
                if args.len() == 3 {
                    // 3-argument pow: integer only
                    let base = match args[0].value.kind() {
                        ValueKind::Int(v) => v,
                        ValueKind::Bool(b) => b as i64,
                        _ => return Err(PyError::Named(
                            "TypeError".to_string(),
                            "pow() 3-argument form requires integers".to_string(),
                        )),
                    };
                    let exp = match args[1].value.kind() {
                        ValueKind::Int(v) => v,
                        ValueKind::Bool(b) => b as i64,
                        _ => return Err(PyError::Named(
                            "TypeError".to_string(),
                            "pow() 3-argument form requires integers".to_string(),
                        )),
                    };
                    let modulus = match args[2].value.kind() {
                        ValueKind::Int(v) => v,
                        ValueKind::Bool(b) => b as i64,
                        _ => return Err(PyError::Named(
                            "TypeError".to_string(),
                            "pow() 3-argument form requires integers".to_string(),
                        )),
                    };
                    if modulus == 0 {
                        return Err(PyError::Named(
                            "ValueError".to_string(),
                            "pow() 3rd argument cannot be 0".to_string(),
                        ));
                    }
                    if exp < 0 {
                        return Err(PyError::Named(
                            "ValueError".to_string(),
                            "pow() 2nd argument cannot be negative when 3rd argument specified".to_string(),
                        ));
                    }
                    // Use modular exponentiation
                    let result = modpow_i64(base, exp as u64, modulus);
                    Ok(Value::int(result))
                } else {
                    // 2-argument pow
                    match (args[0].value.kind(), args[1].value.kind()) {
                        (ValueKind::Int(a), ValueKind::Int(b)) if b >= 0 => {
                            Ok(Value::int(a.wrapping_pow(b as u32)))
                        }
                        (ValueKind::Bool(a), ValueKind::Int(b)) if b >= 0 => {
                            Ok(Value::int((a as i64).wrapping_pow(b as u32)))
                        }
                        _ => {
                            let a = value_to_float(&args[0].value, "pow")?;
                            let b = value_to_float(&args[1].value, "pow")?;
                            Ok(Value::float(a.powf(b)))
                        }
                    }
                }
            }

            ValueKind::BuiltinFunction("hash") => {
                reject_keyword_args_expanded("hash", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "hash() takes exactly one argument".to_string(),
                    ));
                }
                let hash_val = match args[0].value.kind() {
                    ValueKind::Int(v) => v,
                    ValueKind::Bool(b) => b as i64,
                    ValueKind::Float(v) => {
                        // CPython: if float == int, hash equals int hash
                        if v.fract() == 0.0 && v.is_finite() {
                            v as i64
                        } else {
                            // Simple bit-cast hash for non-integer floats
                            v.to_bits() as i64
                        }
                    }
                    ValueKind::Str(s) => {
                        // FNV-1a hash
                        let mut h: u64 = 14695981039346656037u64;
                        for b in s.bytes() {
                            h ^= b as u64;
                            h = h.wrapping_mul(1099511628211u64);
                        }
                        h as i64
                    }
                    ValueKind::None => 0,
                    ValueKind::Tuple(items) => {
                        // Simple tuple hash: combine element hashes
                        let mut h: i64 = 3527539;
                        for item in items {
                            let item_hash = match item.kind() {
                                ValueKind::Int(v) => v,
                                ValueKind::Bool(b) => b as i64,
                                ValueKind::Float(fv) => {
                                    if fv.fract() == 0.0 && fv.is_finite() { fv as i64 }
                                    else { fv.to_bits() as i64 }
                                }
                                ValueKind::Str(s) => {
                                    let mut sh: u64 = 14695981039346656037u64;
                                    for byte in s.bytes() {
                                        sh ^= byte as u64;
                                        sh = sh.wrapping_mul(1099511628211u64);
                                    }
                                    sh as i64
                                }
                                ValueKind::None => 0,
                                _ => return Err(PyError::Named(
                                    "TypeError".to_string(),
                                    "unhashable type in tuple".to_string(),
                                )),
                            };
                            h = h.wrapping_mul(1000003).wrapping_add(item_hash);
                        }
                        h
                    }
                    ValueKind::List(_) => return Err(PyError::Named(
                        "TypeError".to_string(),
                        "unhashable type: 'list'".to_string(),
                    )),
                    ValueKind::Dict(_) => return Err(PyError::Named(
                        "TypeError".to_string(),
                        "unhashable type: 'dict'".to_string(),
                    )),
                    ValueKind::Set(_) => return Err(PyError::Named(
                        "TypeError".to_string(),
                        "unhashable type: 'set'".to_string(),
                    )),
                    _ => return Err(PyError::Named(
                        "TypeError".to_string(),
                        "unhashable type".to_string(),
                    )),
                };
                Ok(Value::int(hash_val))
            }

            ValueKind::BuiltinFunction("chr") => {
                reject_keyword_args_expanded("chr", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "chr() takes exactly one argument".to_string(),
                    ));
                }
                let code_point = match args[0].value.kind() {
                    ValueKind::Int(v) => v,
                    ValueKind::Bool(b) => b as i64,
                    _ => return Err(PyError::Named(
                        "TypeError".to_string(),
                        "an integer is required (got type {})".to_string(),
                    )),
                };
                if !(0..=1114111).contains(&code_point) {
                    return Err(PyError::Named(
                        "ValueError".to_string(),
                        format!("chr() arg not in range(0x110000): {code_point}"),
                    ));
                }
                let ch = char::from_u32(code_point as u32).ok_or_else(|| {
                    PyError::Named(
                        "ValueError".to_string(),
                        format!("chr() arg not in range(0x110000): {code_point}"),
                    )
                })?;
                Ok(Value::string(ch.to_string()))
            }

            ValueKind::BuiltinFunction("ord") => {
                reject_keyword_args_expanded("ord", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "ord() takes exactly one argument".to_string(),
                    ));
                }
                match args[0].value.kind() {
                    ValueKind::Str(s) => {
                        let mut chars = s.chars();
                        let first = chars.next();
                        let second = chars.next();
                        match (first, second) {
                            (Some(c), None) => Ok(Value::int(c as i64)),
                            (None, _) => Err(PyError::Named(
                                "TypeError".to_string(),
                                "ord() expected a character, but string of length 0 found".to_string(),
                            )),
                            (Some(_), Some(_)) => Err(PyError::Named(
                                "TypeError".to_string(),
                                format!("ord() expected a character, but string of length {} found", s.chars().count()),
                            )),
                        }
                    }
                    _ => Err(PyError::Named(
                        "TypeError".to_string(),
                        "ord() expected string of length 1, but got non-string".to_string(),
                    )),
                }
            }

            ValueKind::BuiltinFunction("bin") => {
                reject_keyword_args_expanded("bin", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "bin() takes exactly one argument".to_string(),
                    ));
                }
                match args[0].value.kind() {
                    ValueKind::Int(v) => {
                        if v < 0 {
                            Ok(Value::string(format!("-0b{:b}", -v)))
                        } else {
                            Ok(Value::string(format!("0b{:b}", v)))
                        }
                    }
                    ValueKind::Bool(b) => Ok(Value::string(if b { "0b1".to_string() } else { "0b0".to_string() })),
                    _ => Err(PyError::Named(
                        "TypeError".to_string(),
                        "'{}' object cannot be interpreted as an integer".to_string(),
                    )),
                }
            }

            ValueKind::BuiltinFunction("oct") => {
                reject_keyword_args_expanded("oct", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "oct() takes exactly one argument".to_string(),
                    ));
                }
                match args[0].value.kind() {
                    ValueKind::Int(v) => {
                        if v < 0 {
                            Ok(Value::string(format!("-0o{:o}", -v)))
                        } else {
                            Ok(Value::string(format!("0o{:o}", v)))
                        }
                    }
                    ValueKind::Bool(b) => Ok(Value::string(if b { "0o1".to_string() } else { "0o0".to_string() })),
                    _ => Err(PyError::Named(
                        "TypeError".to_string(),
                        "'{}' object cannot be interpreted as an integer".to_string(),
                    )),
                }
            }

            ValueKind::BuiltinFunction("hex") => {
                reject_keyword_args_expanded("hex", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "hex() takes exactly one argument".to_string(),
                    ));
                }
                match args[0].value.kind() {
                    ValueKind::Int(v) => {
                        if v < 0 {
                            Ok(Value::string(format!("-0x{:x}", -v)))
                        } else {
                            Ok(Value::string(format!("0x{:x}", v)))
                        }
                    }
                    ValueKind::Bool(b) => Ok(Value::string(if b { "0x1".to_string() } else { "0x0".to_string() })),
                    _ => Err(PyError::Named(
                        "TypeError".to_string(),
                        "'{}' object cannot be interpreted as an integer".to_string(),
                    )),
                }
            }

            ValueKind::BuiltinFunction("issubclass") => {
                reject_keyword_args_expanded("issubclass", args)?;
                if args.len() != 2 {
                    return Err(PyError::Runtime(
                        "issubclass() takes exactly 2 arguments".to_string(),
                    ));
                }
                let cls = match args[0].value.kind() {
                    ValueKind::PyClass(c) => Rc::clone(c),
                    _ => return Err(PyError::Named(
                        "TypeError".to_string(),
                        "issubclass() arg 1 must be a class".to_string(),
                    )),
                };
                let result = match args[1].value.kind() {
                    ValueKind::PyClass(expected) => {
                        class_is_subclass_of(&cls, expected)
                    }
                    ValueKind::Tuple(items) => {
                        let mut found = false;
                        for item in items {
                            if let ValueKind::PyClass(expected) = item.kind()
                                && class_is_subclass_of(&cls, expected) {
                                    found = true;
                                    break;
                                }
                        }
                        found
                    }
                    _ => return Err(PyError::Named(
                        "TypeError".to_string(),
                        "issubclass() arg 2 must be a class or tuple of classes".to_string(),
                    )),
                };
                Ok(Value::bool_(result))
            }

            ValueKind::BuiltinFunction("delattr") => {
                reject_keyword_args_expanded("delattr", args)?;
                if args.len() != 2 {
                    return Err(PyError::Runtime(
                        "delattr() takes exactly 2 arguments".to_string(),
                    ));
                }
                let name = match args[1].value.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Err(PyError::Named(
                        "TypeError".to_string(),
                        "delattr(): attribute name must be a string".to_string(),
                    )),
                };
                match args[0].value.kind() {
                    ValueKind::PyInstance(instance) => {
                        let instance = Rc::clone(instance);
                        if instance.borrow_mut().attrs.remove(&name).is_none() {
                            let class_name = instance.borrow().class.borrow().name.clone();
                            return Err(PyError::Named(
                                "AttributeError".to_string(),
                                format!("'{}' object has no attribute '{}'", class_name, name),
                            ));
                        }
                        Ok(Value::none())
                    }
                    ValueKind::PyClass(class) => {
                        let class = Rc::clone(class);
                        if class.borrow_mut().attrs.remove(&name).is_none() {
                            let class_name = class.borrow().name.clone();
                            return Err(PyError::Named(
                                "AttributeError".to_string(),
                                format!("type object '{}' has no attribute '{}'", class_name, name),
                            ));
                        }
                        Ok(Value::none())
                    }
                    _ => Err(PyError::Named(
                        "AttributeError".to_string(),
                        "delattr() object has no writable attributes".to_string(),
                    )),
                }
            }

            ValueKind::BuiltinFunction("staticmethod") => {
                reject_keyword_args_expanded("staticmethod", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "staticmethod() takes exactly one argument".to_string(),
                    ));
                }
                match args[0].value.kind() {
                    ValueKind::UserFunction(f) => Ok(Value::static_method(Rc::clone(f))),
                    _ => Err(PyError::Runtime(
                        "staticmethod() argument must be a function".to_string(),
                    )),
                }
            }

            // `super(cls, instance)` — two-argument form only.
            // Zero-argument `super()` (implicit __class__ cell) is not supported;
            // users must pass both arguments explicitly.
            ValueKind::BuiltinFunction("super") => {
                reject_keyword_args_expanded("super", args)?;
                if args.len() != 2 {
                    return Err(PyError::Runtime(
                        "super() requires exactly 2 arguments: super(CurrentClass, self)".to_string(),
                    ));
                }
                let cls_val = args[0].value.clone();
                let inst_val = args[1].value.clone();
                let class = match cls_val.kind() {
                    ValueKind::PyClass(c) => Rc::clone(c),
                    _ => return Err(PyError::Runtime(
                        "super() first argument must be a class".to_string(),
                    )),
                };
                match inst_val.kind() {
                    ValueKind::PyInstance(i) => {
                        let instance = Rc::clone(i);
                        // Bug #199: validate instance is an instance of class
                        if !class_is_subclass_of(&instance.borrow().class, &class) {
                            return Err(PyError::Named(
                                "TypeError".to_string(),
                                "super(type, obj): obj must be an instance or subtype of type".to_string(),
                            ));
                        }
                        Ok(Value::super_proxy(class, instance))
                    }
                    ValueKind::PyClass(obj_class) => {
                        // Bug #197: classmethod case — second arg is a class
                        let obj_class = Rc::clone(obj_class);
                        // Validate obj_class is a subclass of class
                        if !class_is_subclass_of(&obj_class, &class) {
                            return Err(PyError::Named(
                                "TypeError".to_string(),
                                "super(type, obj): obj must be an instance or subtype of type".to_string(),
                            ));
                        }
                        Ok(Value::super_proxy_class(class, obj_class))
                    }
                    _ => Err(PyError::Runtime(
                        "super() second argument must be a class instance".to_string(),
                    )),
                }
            }

            ValueKind::BuiltinFunction("next") => {
                reject_keyword_args_expanded("next", args)?;
                if args.is_empty() || args.len() > 2 {
                    return Err(PyError::Runtime(
                        "next() takes 1 or 2 arguments".to_string(),
                    ));
                }
                let gen_val = args[0].value.clone();
                let default_val = if args.len() == 2 {
                    Some(args[1].value.clone())
                } else {
                    None
                };
                self.call_next(gen_val, default_val)
            }

            ValueKind::BuiltinFunction("iter") => {
                reject_keyword_args_expanded("iter", args)?;
                if args.len() != 1 {
                    return Err(PyError::Runtime(
                        "iter() takes exactly one argument".to_string(),
                    ));
                }
                let val = args[0].value.clone();
                match val.kind() {
                    // Generators are their own iterators.
                    ValueKind::Generator(_) => Ok(val),
                    // User-defined objects: call __iter__().
                    ValueKind::PyInstance(inst) => {
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
                            )
                        } else if lookup_class_attr(&class, "__next__").is_some() {
                            // Already an iterator (has __next__ but no separate __iter__).
                            Ok(val)
                        } else {
                            Err(PyError::Named(
                                "TypeError".to_string(),
                                format!("'{}' object is not iterable", class.borrow().name),
                            ))
                        }
                    }
                    // Built-in iterables: materialise into a NativeIterFrame so that
                    // next() works on the returned value.
                    _ => {
                        let items = iter_values(val.clone()).map_err(|_| {
                            PyError::Named(
                                "TypeError".to_string(),
                                format!("'{}' object is not iterable", value_type_name_str(&val)),
                            )
                        })?;
                        Ok(Value::generator(Box::new(NativeIterFrame { items, pos: 0 })))
                    }
                }
            }

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
                Err(PyError::Named(
                    "TypeError".to_string(),
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
    fn collect_iterable(&mut self, val: Value) -> Result<Vec<Value>> {
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
                    return Err(PyError::Named(
                        "TypeError".to_string(),
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
    fn call_next(&mut self, val: Value, default: Option<Value>) -> Result<Value> {
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
                            Err(PyError::Named("StopIteration".to_string(), String::new()))
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
                    Err(PyError::Named("StopIteration".to_string(), String::new()))
                };
            }
            match self.resume_generator(frame) {
                Err(PyError::GeneratorYield(yielded)) => Ok(yielded),
                Err(PyError::Named(ref cls, _)) if cls == "StopIteration" => {
                    drop(borrow);
                    if let Some(d) = default {
                        Ok(d)
                    } else {
                        Err(PyError::Named("StopIteration".to_string(), String::new()))
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
                Err(PyError::Named(
                    "TypeError".to_string(),
                    format!(
                        "'{}' object is not an iterator",
                        class.borrow().name
                    ),
                ))
            }
        } else {
            Err(PyError::Named(
                "TypeError".to_string(),
                format!("'{}' object is not an iterator", value_type_name_str(&val)),
            ))
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
            // Resolve defaults: fill any still-empty bound_args slots in-place.
            for (index, param) in function.params.iter().enumerate() {
                if bound_args[index].is_none() {
                    bound_args[index] = Some(param.default.clone().ok_or_else(|| {
                        if param.is_keyword_only {
                            PyError::Named(
                                "TypeError".to_string(),
                                format!(
                                    "{}() missing 1 required keyword-only argument: '{}'",
                                    function.name, param.name
                                ),
                            )
                        } else {
                            PyError::Named(
                                "TypeError".to_string(),
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
                let mut regs: Vec<Option<Value>> = vec![None; num_regs];

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
                                    return Err(PyError::Named(
                                        "SystemError".to_string(),
                                        format!(
                                            "parameter '{}' register index {} out of range (num_regs={})",
                                            param.name, reg, num_regs
                                        ),
                                    ));
                                }
                                regs[reg as usize] = Some(val);
                            }
                        }
                    }
                    std::mem::replace(&mut self.env, local_env)
                } else {
                    for (param, slot) in function.params.iter().zip(bound_args.iter_mut()) {
                        let val = slot.take().unwrap();
                        if let Some(&reg) = function.local_index.get(&param.name) {
                            if reg as usize >= num_regs {
                                return Err(PyError::Named(
                                    "SystemError".to_string(),
                                    format!(
                                        "parameter '{}' register index {} out of range (num_regs={})",
                                        param.name, reg, num_regs
                                    ),
                                ));
                            }
                            regs[reg as usize] = Some(val);
                        }
                    }
                    std::mem::replace(&mut self.env, Rc::clone(&function.env))
                };

                // Self-reference for recursive calls (only if not a cell var).
                if !code.cell_vars.contains(&function.name)
                    && let Some(&slot) = function.local_index.get(&function.name) {
                        if slot as usize >= num_regs {
                            return Err(PyError::Named(
                                "SystemError".to_string(),
                                format!(
                                    "self-reference register index {} out of range (num_regs={})",
                                    slot, num_regs
                                ),
                            ));
                        }
                        regs[slot as usize] = Some(Value::user_function(Rc::clone(&function)));
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
                let kw_pos = keyword_vals.iter().position(|(k, _)| k == &param.name);
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
                    return Err(PyError::Named(
                        "TypeError".to_string(),
                        format!(
                            "{}() missing 1 required keyword-only argument: '{}'",
                            function.name, param.name
                        ),
                    ));
                } else {
                    return Err(PyError::Named(
                        "TypeError".to_string(),
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
            let mut regs: Vec<Option<Value>> = vec![None; num_regs];

            // Bind non-cell params into register file using fastlocals slot indices.
            for (param, val) in function.params.iter().zip(param_vals.iter()) {
                if !code.cell_vars.contains(&param.name)
                    && let Some(&slot) = function.local_index.get(&param.name) {
                        if (slot as usize) >= num_regs {
                            return Err(PyError::Named(
                                "SystemError".to_string(),
                                format!(
                                    "parameter '{}' register index {} out of range (num_regs={})",
                                    param.name, slot, num_regs
                                ),
                            ));
                        }
                        regs[slot as usize] = Some(val.clone());
                    }
            }
            // Self-reference for recursive calls (only if not a cell var).
            if !code.cell_vars.contains(&function.name)
                && let Some(&slot) = function.local_index.get(&function.name) {
                    if (slot as usize) >= num_regs {
                        return Err(PyError::Named(
                            "SystemError".to_string(),
                            format!(
                                "self-reference register index {} out of range (num_regs={})",
                                slot, num_regs
                            ),
                        ));
                    }
                    regs[slot as usize] = Some(Value::user_function(Rc::clone(&function)));
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

    fn call_range_expanded(&mut self, args: &[ExpandedCallArg]) -> Result<Value> {
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
            return Err(PyError::Named(
                "ValueError".to_string(),
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
fn apply_format_spec(value: &Value, spec: &str) -> Result<Value> {
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
            _ => return Err(PyError::Named(
                "ValueError".to_string(),
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
            _ => return Err(PyError::Named(
                "ValueError".to_string(),
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
            _ => return Err(PyError::Named(
                "ValueError".to_string(),
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
            _ => return Err(PyError::Named(
                "ValueError".to_string(),
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
            _ => return Err(PyError::Named(
                "ValueError".to_string(),
                format!("unknown format code 'b' for object of type '{}'", value_type_name_str(value)),
            )),
        },
        Some(other) => {
            return Err(PyError::Named(
                "ValueError".to_string(),
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
        _ => Err(PyError::Named(
            "TypeError".to_string(),
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
