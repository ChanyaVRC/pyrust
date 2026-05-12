impl Interpreter {
    fn get_attr(&mut self, target: Value, name: &str) -> Result<Value> {
        match target.kind() {
            ValueKind::PyInstance(instance) => {
                let instance = Rc::clone(instance);
                if name == "__class__" {
                    return Ok(Value::py_class(Rc::clone(&instance.borrow().class)));
                }

                // Check the class first for data descriptors (Property).  A data
                // descriptor takes priority over instance __dict__ — matching CPython.
                let class = { Rc::clone(&instance.borrow().class) };
                if let Some(class_val) = lookup_class_attr(&class, name)
                    && let ValueKind::Property { fget, .. } = class_val.kind() {
                        let fget = Rc::clone(fget);
                        return if fget.is_none() {
                            Err(PyError::Named(
                                "AttributeError".to_string(),
                                format!("property '{}' has no getter", name),
                            ))
                        } else {
                            let getter = (*fget).clone();
                            self.call_function_expanded(
                                getter,
                                &[ExpandedCallArg {
                                    name: None,
                                    value: Value::py_instance(Rc::clone(&instance)),
                                }],
                            )
                        };
                    }

                if let Some(value) = instance.borrow().attrs.get(name).cloned() {
                    return Ok(value);
                }

                if let Some(value) = lookup_class_attr(&class, name) {
                    return Ok(match value.kind() {
                        ValueKind::UserFunction(f) => {
                            Value::bound_method(Rc::clone(f), instance)
                        }
                        ValueKind::ClassMethod(f) => {
                            // classmethod: bind the class (not the instance) as first argument
                            Value::class_bound_method(Rc::clone(f), Rc::clone(&class))
                        }
                        ValueKind::StaticMethod(f) => {
                            // staticmethod: return the raw function, no binding
                            Value::user_function(Rc::clone(f))
                        }
                        _ => value,
                    });
                }

                let class_name = class.borrow().name.clone();
                Err(PyError::Named(
                    "AttributeError".to_string(),
                    format!("'{}' object has no attribute '{}'", class_name, name),
                ))
            }
            ValueKind::PyClass(class) => {
                let class = Rc::clone(class);
                if name == "__name__" {
                    return Ok(Value::string(class.borrow().name.clone()));
                }
                if let Some(value) = lookup_class_attr(&class, name) {
                    return Ok(match value.kind() {
                        ValueKind::ClassMethod(f) => {
                            // classmethod accessed on a class: bind the class as first argument
                            Value::class_bound_method(Rc::clone(f), Rc::clone(&class))
                        }
                        ValueKind::StaticMethod(f) => Value::user_function(Rc::clone(f)),
                        _ => value,
                    });
                }

                let class_name = class.borrow().name.clone();
                Err(PyError::Named(
                    "AttributeError".to_string(),
                    format!("type object '{}' has no attribute '{}'", class_name, name),
                ))
            }
            ValueKind::SuperProxy { class, instance } => {
                let class = Rc::clone(class);
                let instance = Rc::clone(instance);
                // Look up the method starting from class's parent (skip class itself)
                let parent = class.borrow().base.clone();
                let Some(parent_class) = parent else {
                    return Err(PyError::Named(
                        "AttributeError".to_string(),
                        format!("super(): '{}' has no base class", class.borrow().name),
                    ));
                };
                if let Some(value) = lookup_class_attr(&parent_class, name) {
                    return Ok(match value.kind() {
                        ValueKind::UserFunction(f) => {
                            Value::bound_method(Rc::clone(f), instance)
                        }
                        ValueKind::ClassMethod(f) => {
                            Value::class_bound_method(Rc::clone(f), parent_class)
                        }
                        ValueKind::StaticMethod(f) => Value::user_function(Rc::clone(f)),
                        _ => value,
                    });
                }
                Err(PyError::Named(
                    "AttributeError".to_string(),
                    format!("super(): parent class has no attribute '{name}'"),
                ))
            }
            ValueKind::SuperProxyClass { class, obj_class } => {
                let class = Rc::clone(class);
                let obj_class = Rc::clone(obj_class);
                // classmethod super(): look up from class's parent and bind to obj_class
                let parent = class.borrow().base.clone();
                let Some(parent_class) = parent else {
                    return Err(PyError::Named(
                        "AttributeError".to_string(),
                        format!("super(): '{}' has no base class", class.borrow().name),
                    ));
                };
                if let Some(value) = lookup_class_attr(&parent_class, name) {
                    return Ok(match value.kind() {
                        ValueKind::UserFunction(f) => {
                            // Regular method accessed via super in classmethod context —
                            // return the raw function (rare, but valid).
                            Value::user_function(Rc::clone(f))
                        }
                        ValueKind::ClassMethod(f) => {
                            // classmethod: bind to the concrete subclass
                            Value::class_bound_method(Rc::clone(f), obj_class)
                        }
                        ValueKind::StaticMethod(f) => Value::user_function(Rc::clone(f)),
                        _ => value,
                    });
                }
                Err(PyError::Named(
                    "AttributeError".to_string(),
                    format!("super(): parent class has no attribute '{name}'"),
                ))
            }
            // Access .setter / .deleter / .getter on a property descriptor itself.
            // These return a new property with the respective accessor replaced.
            ValueKind::Property { fget, fset, fdel } => {
                let fget_val = (**fget).clone();
                let fset_val = (**fset).clone();
                let fdel_val = (**fdel).clone();
                match name {
                    "setter" => {
                        // Return a builtin-callable that takes (new_fset) and returns
                        // a new Property with fget preserved.
                        Ok(Value::property_setter_partial(fget_val, fdel_val))
                    }
                    "deleter" => {
                        Ok(Value::property_deleter_partial(fget_val, fset_val))
                    }
                    "getter" => {
                        Ok(Value::property_getter_partial(fset_val, fdel_val))
                    }
                    "fget" => Ok(fget_val),
                    "fset" => Ok(fset_val),
                    "fdel" => Ok(fdel_val),
                    _ => Err(PyError::Named(
                        "AttributeError".to_string(),
                        format!("property object has no attribute '{name}'"),
                    )),
                }
            }
            ValueKind::PyModule(module) => {
                let module = Rc::clone(module);
                if let Some(value) = module.borrow().attrs.get(name).cloned() {
                    return Ok(value);
                }
                let mod_name = module.borrow().name.clone();
                Err(PyError::Named(
                    "AttributeError".to_string(),
                    format!("module '{mod_name}' has no attribute '{name}'"),
                ))
            }
            ValueKind::BuiltinFunction(func_name) => {
                // __name__ is supported on all builtin type/function values so that
                // `type(x).__name__` works for both builtin and user-defined types.
                if name == "__name__" {
                    return Ok(Value::string(func_name));
                }
                if func_name == "str" {
                    match name {
                        "lower"      => Ok(Value::builtin_function("str.lower")),
                        "upper"      => Ok(Value::builtin_function("str.upper")),
                        "strip"      => Ok(Value::builtin_function("str.strip")),
                        "lstrip"     => Ok(Value::builtin_function("str.lstrip")),
                        "rstrip"     => Ok(Value::builtin_function("str.rstrip")),
                        "capitalize" => Ok(Value::builtin_function("str.capitalize")),
                        "split"      => Ok(Value::builtin_function("str.split")),
                        "join"       => Ok(Value::builtin_function("str.join")),
                        "replace"    => Ok(Value::builtin_function("str.replace")),
                        "find"       => Ok(Value::builtin_function("str.find")),
                        "rfind"      => Ok(Value::builtin_function("str.rfind")),
                        "index"      => Ok(Value::builtin_function("str.index")),
                        "rindex"     => Ok(Value::builtin_function("str.rindex")),
                        "count"      => Ok(Value::builtin_function("str.count")),
                        "startswith" => Ok(Value::builtin_function("str.startswith")),
                        "endswith"   => Ok(Value::builtin_function("str.endswith")),
                        "format"     => Ok(Value::builtin_function("str.format")),
                        "isdigit"    => Ok(Value::builtin_function("str.isdigit")),
                        "isalpha"    => Ok(Value::builtin_function("str.isalpha")),
                        "isalnum"    => Ok(Value::builtin_function("str.isalnum")),
                        "isspace"    => Ok(Value::builtin_function("str.isspace")),
                        _ => Err(PyError::Named(
                            "AttributeError".to_string(),
                            format!("type object 'str' has no attribute '{name}'"),
                        )),
                    }
                } else {
                    Err(PyError::Named(
                        "AttributeError".to_string(),
                        format!("type object '{}' has no attribute '{name}'", func_name),
                    ))
                }
            }
            _ => {
                // Built-in type instance method lookup: list.append, str.upper, etc.
                if builtin_has_method(&target, name) {
                    return Ok(Value::builtin_bound_method(name, target.clone()));
                }
                let type_name = pyrust_core::builtin_type_name(&target);
                Err(PyError::Named(
                    "AttributeError".to_string(),
                    format!("'{type_name}' object has no attribute '{name}'"),
                ))
            }
        }
    }

    fn assign_attr(&mut self, target: Value, name: &str, value: Value) -> Result<()> {
        match target.kind() {
            ValueKind::PyInstance(instance) => {
                // Check for a property descriptor in the class chain.
                let class = { Rc::clone(&instance.borrow().class) };
                if let Some(class_val) = lookup_class_attr(&class, name)
                    && let ValueKind::Property { fset, .. } = class_val.kind() {
                        let fset = Rc::clone(fset);
                        return if fset.is_none() {
                            Err(PyError::Named(
                                "AttributeError".to_string(),
                                format!("property '{}' has no setter", name),
                            ))
                        } else {
                            let setter = (*fset).clone();
                            self.call_function_expanded(
                                setter,
                                &[
                                    ExpandedCallArg {
                                        name: None,
                                        value: Value::py_instance(Rc::clone(instance)),
                                    },
                                    ExpandedCallArg { name: None, value },
                                ],
                            )?;
                            Ok(())
                        };
                    }
                instance.borrow_mut().attrs.insert(name.to_string(), value);
                Ok(())
            }
            ValueKind::PyClass(class) => {
                class.borrow_mut().attrs.insert(name.to_string(), value);
                Ok(())
            }
            _ => Err(PyError::Runtime(format!(
                "object has no writable attribute '{}'",
                name
            ))),
        }
    }

    fn delete_attr(&mut self, target: Value, name: &str) -> Result<()> {
        match target.kind() {
            ValueKind::PyInstance(instance) => {
                let class = { Rc::clone(&instance.borrow().class) };
                if let Some(class_val) = lookup_class_attr(&class, name)
                    && let ValueKind::Property { fdel, .. } = class_val.kind() {
                        let fdel = Rc::clone(fdel);
                        return if fdel.is_none() {
                            Err(PyError::Named(
                                "AttributeError".to_string(),
                                format!("property '{}' has no deleter", name),
                            ))
                        } else {
                            let deleter = (*fdel).clone();
                            self.call_function_expanded(
                                deleter,
                                &[ExpandedCallArg {
                                    name: None,
                                    value: Value::py_instance(Rc::clone(instance)),
                                }],
                            )?;
                            Ok(())
                        };
                    }
                instance.borrow_mut().attrs.remove(name);
                Ok(())
            }
            _ => Err(PyError::Runtime(
                "can only delete attributes of class instances".to_string(),
            )),
        }
    }

    fn load_module(&mut self, name: &str) -> Result<Value> {
        if let Some(cached) = self.module_cache.borrow().get(name).cloned() {
            return Ok(cached);
        }
        // Built-in modules
        let builtin = match name {
            "math" => Some(make_math_module()),
            "sys" => Some(make_sys_module()),
            _ => None,
        };
        if let Some(val) = builtin {
            self.module_cache.borrow_mut().insert(name.to_string(), val.clone());
            return Ok(val);
        }
        // User .py file: look for <name>.py relative to script_dir
        if let Some(ref dir) = self.script_dir.clone() {
            // Convert dotted name to path: "foo.bar" -> "foo/bar.py"
            let rel_path = name.replace('.', "/") + ".py";
            let full_path = dir.join(&rel_path);
            if full_path.exists() {
                let src = std::fs::read_to_string(&full_path).map_err(|e| {
                    PyError::Runtime(format!("failed to read '{}': {e}", full_path.display()))
                })?;
                let tokens = Lexer::new(&src)?;
                let program = Parser::new(tokens.into_tokens()).parse_program()?;
                // Subinterpreter shares the same module_cache so results are visible to parent
                let mut sub = Interpreter {
                    script_dir: self.script_dir.clone(),
                    module_cache: Rc::clone(&self.module_cache),
                    ..Default::default()
                };
                // call_depth is thread_local — sub-interpreter automatically shares the same counter
                sub.exec_program(&program, false)?;
                // Harvest all top-level bindings as module attrs
                let attrs: HashMap<String, Value> = sub
                    .env
                    .borrow()
                    .values
                    .iter()
                    .filter(|(k, _)| {
                        !matches!(
                            k.as_str(),
                            "Exception"
                                | "RuntimeError"
                                | "TypeError"
                                | "ValueError"
                                | "AssertionError"
                        )
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let module = Value::py_module(Rc::new(RefCell::new(PyModule {
                    name: name.to_string(),
                    attrs,
                })));
                self.module_cache.borrow_mut().insert(name.to_string(), module.clone());
                return Ok(module);
            }
        }
        Err(PyError::Runtime(format!("No module named '{name}'")))
    }

    fn assign_name(&self, name: String, value: Value) {
        let (is_global, is_nonlocal) = {
            let env = self.env.borrow();
            (env.global_names.contains(&name), env.nonlocal_names.contains(&name))
        };
        if is_global {
            module_env(&self.env).borrow_mut().values.insert(name, value);
            return;
        }
        if is_nonlocal
            && let Some(env) = find_enclosing_local_env_for_name(&self.env, &name) {
                env_assign_local(&env, &name, value);
                return;
            }
        env_assign_local(&self.env, &name, value);
    }

    fn lookup_name(&self, name: &str) -> Result<Option<Value>> {
        let (is_global, is_nonlocal) = {
            let env = self.env.borrow();
            (env.global_names.contains(name), env.nonlocal_names.contains(name))
        };
        if is_global {
            return Ok(lookup_name_in_module(&self.env, name));
        }
        if is_nonlocal {
            return lookup_name_in_enclosing_local_env(&self.env, name);
        }
        lookup_name_in_env(&self.env, name)
    }

    fn resolve_name_env(&self, name: &str) -> Option<EnvRef> {
        let (is_global, is_nonlocal) = {
            let env = self.env.borrow();
            (env.global_names.contains(name), env.nonlocal_names.contains(name))
        };
        if is_global {
            let me = module_env(&self.env);
            return if me.borrow().values.contains_key(name) { Some(me) } else { None };
        }
        if is_nonlocal {
            return find_enclosing_local_env_for_name(&self.env, name);
        }
        find_env_for_name(&self.env, name)
    }

    fn alloc_env(&mut self, parent: Option<EnvRef>) -> EnvRef {
        if let Some(env) = self.env_pool.pop() {
            {
                let mut e = env.borrow_mut();
                e.values.clear();
                e.fastlocals = None;
                e.parent = parent;
            }
            env
        } else {
            Environment::new(parent)
        }
    }

    fn free_env(&mut self, env: EnvRef) {
        if self.env_pool.len() < ENV_POOL_MAX && Rc::strong_count(&env) == 1 {
            self.env_pool.push(env);
        }
    }

    /// Like `Value::truthy()` but dispatches `__bool__` / `__len__` for instances.
    fn truthy_value(&mut self, value: &Value) -> Result<bool> {
        if let ValueKind::PyInstance(inst) = value.kind() {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            // Try __bool__ first.
            if let Some(method_val) = lookup_class_attr(&class, "__bool__")
                && let ValueKind::UserFunction(f) = method_val.kind() {
                    let func = Rc::clone(f);
                    let result = self.call_user_function_expanded(
                        func,
                        &[],
                        &[Value::py_instance(inst_rc)],
                    )?;
                    return match result.kind() {
                        ValueKind::Bool(b) => Ok(b),
                        ValueKind::Int(_) => Err(PyError::Named(
                            "TypeError".to_string(),
                            "__bool__ should return bool, not int".to_string(),
                        )),
                        _ => Err(PyError::Named(
                            "TypeError".to_string(),
                            "__bool__ should return bool".to_string(),
                        )),
                    };
                }
            // Fall back to __len__.
            if let Some(method_val) = lookup_class_attr(&class, "__len__")
                && let ValueKind::UserFunction(f) = method_val.kind() {
                    let func = Rc::clone(f);
                    let result = self.call_user_function_expanded(
                        func,
                        &[],
                        &[Value::py_instance(inst_rc)],
                    )?;
                    return match result.kind() {
                        ValueKind::Int(n) => Ok(n != 0),
                        ValueKind::Bool(b) => Ok(b),
                        _ => Err(PyError::Named(
                            "TypeError".to_string(),
                            "__len__ returned non-int".to_string(),
                        )),
                    };
                }
            // No __bool__ or __len__: always truthy.
            return Ok(true);
        }
        Ok(value.truthy())
    }

}

/// Returns `true` if `name` is a built-in method on `target`'s type.
/// Used by `get_attr` to produce `BuiltinBoundMethod` values.
fn builtin_has_method(target: &Value, name: &str) -> bool {
    match target.kind() {
        ValueKind::Str(_) => pyrust_builtins::string::has_method(name),
        ValueKind::List(_) => pyrust_builtins::list::has_method(name),
        ValueKind::Tuple(_) => pyrust_builtins::tuple::has_method(name),
        ValueKind::Dict(_) => pyrust_builtins::dict::has_method(name),
        ValueKind::Set(_) => pyrust_builtins::set::has_method(name),
        ValueKind::FrozenSet(_) => matches!(
            name,
            "copy"
                | "union"
                | "intersection"
                | "difference"
                | "symmetric_difference"
                | "issubset"
                | "issuperset"
                | "isdisjoint"
        ),
        _ => false,
    }
}
