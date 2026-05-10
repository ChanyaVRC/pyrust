impl Interpreter {
    fn get_attr(&self, target: Value, name: &str) -> Result<Value> {
        match target.kind() {
            ValueKind::PyInstance(instance) => {
                let instance = Rc::clone(instance);
                if name == "__class__" {
                    return Ok(Value::py_class(Rc::clone(&instance.borrow().class)));
                }
                if let Some(value) = instance.borrow().attrs.get(name).cloned() {
                    return Ok(value);
                }

                let class = { Rc::clone(&instance.borrow().class) };
                if let Some(value) = lookup_class_attr(&class, name) {
                    return Ok(match value.kind() {
                        ValueKind::UserFunction(_) => {
                            if let ValueKind::UserFunction(f) = value.kind() {
                                Value::bound_method(Rc::clone(f), instance)
                            } else { unreachable!() }
                        }
                        _ => value,
                    });
                }

                let class_name = class.borrow().name.clone();
                Err(PyError::Runtime(format!(
                    "'{}' object has no attribute '{}'",
                    class_name, name
                )))
            }
            ValueKind::PyClass(class) => {
                let class = Rc::clone(class);
                if name == "__name__" {
                    return Ok(Value::string(class.borrow().name.clone()));
                }
                if let Some(value) = lookup_class_attr(&class, name) {
                    return Ok(value);
                }

                let class_name = class.borrow().name.clone();
                Err(PyError::Runtime(format!(
                    "type object '{}' has no attribute '{}'",
                    class_name, name
                )))
            }
            ValueKind::PyModule(module) => {
                let module = Rc::clone(module);
                if let Some(value) = module.borrow().attrs.get(name).cloned() {
                    return Ok(value);
                }
                let mod_name = module.borrow().name.clone();
                Err(PyError::Runtime(format!(
                    "module '{mod_name}' has no attribute '{name}'"
                )))
            }
            ValueKind::BuiltinFunction("str") => {
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
                    "isdigit"    => Ok(Value::builtin_function("str.isdigit")),
                    "isalpha"    => Ok(Value::builtin_function("str.isalpha")),
                    "isalnum"    => Ok(Value::builtin_function("str.isalnum")),
                    "isspace"    => Ok(Value::builtin_function("str.isspace")),
                    _ => Err(PyError::Runtime(format!(
                        "type object 'str' has no attribute '{name}'"
                    ))),
                }
            }
            _ => Err(PyError::Runtime(format!(
                "object has no attribute '{}'",
                name
            ))),
        }
    }

    fn assign_attr(&self, target: Value, name: &str, value: Value) -> Result<()> {
        match target.kind() {
            ValueKind::PyInstance(instance) => {
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
                let mut sub = Interpreter::default();
                sub.script_dir = self.script_dir.clone();
                sub.module_cache = Rc::clone(&self.module_cache);
                sub.call_depth = self.call_depth;
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
        if is_nonlocal {
            if let Some(env) = find_enclosing_local_env_for_name(&self.env, &name) {
                env_assign_local(&env, &name, value);
                return;
            }
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

}
