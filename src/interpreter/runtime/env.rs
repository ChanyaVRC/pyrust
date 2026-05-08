impl Interpreter {
    fn get_attr(&self, target: Value, name: &str) -> Result<Value> {
        match target {
            Value::Instance(instance) => {
                if let Some(value) = instance.borrow().attrs.get(name).cloned() {
                    return Ok(value);
                }

                let class = { Rc::clone(&instance.borrow().class) };
                if let Some(value) = lookup_class_attr(&class, name) {
                    return Ok(match value {
                        Value::Function(function) => Value::BoundMethod {
                            function,
                            receiver: instance,
                        },
                        other => other,
                    });
                }

                let class_name = class.borrow().name.clone();
                Err(PyError::Runtime(format!(
                    "'{}' object has no attribute '{}'",
                    class_name, name
                )))
            }
            Value::Class(class) => {
                if let Some(value) = lookup_class_attr(&class, name) {
                    return Ok(value);
                }

                let class_name = class.borrow().name.clone();
                Err(PyError::Runtime(format!(
                    "type object '{}' has no attribute '{}'",
                    class_name, name
                )))
            }
            Value::Module(module) => {
                if let Some(value) = module.borrow().attrs.get(name).cloned() {
                    return Ok(value);
                }
                let mod_name = module.borrow().name.clone();
                Err(PyError::Runtime(format!(
                    "module '{mod_name}' has no attribute '{name}'"
                )))
            }
            _ => Err(PyError::Runtime(format!(
                "object has no attribute '{}'",
                name
            ))),
        }
    }

    fn assign_attr(&self, target: Value, name: &str, value: Value) -> Result<()> {
        match target {
            Value::Instance(instance) => {
                instance.borrow_mut().attrs.insert(name.to_string(), value);
                Ok(())
            }
            Value::Class(class) => {
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
                let module = Value::Module(Rc::new(RefCell::new(PyModule {
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

    // Returns the EnvRef that owns `name`, respecting global/nonlocal declarations.
    // Determines the env where assignments to `name` should land, resolving
    // global/nonlocal declarations. Unlike resolve_name_env this always returns
    // an env — variables not yet defined are written into the current scope.
    fn resolve_assign_env_for(&self, name: &str) -> EnvRef {
        let (is_global, is_nonlocal) = {
            let env = self.env.borrow();
            (env.global_names.contains(name), env.nonlocal_names.contains(name))
        };
        if is_global {
            return module_env(&self.env);
        }
        if is_nonlocal {
            if let Some(env) = find_enclosing_local_env_for_name(&self.env, name) {
                return env;
            }
        }
        Rc::clone(&self.env)
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

    fn declare_global_names(&self, names: &[String]) {
        let mut env = self.env.borrow_mut();
        Rc::make_mut(&mut env.global_names).extend(names.iter().cloned());
    }

    fn declare_nonlocal_names(&self, names: &[String]) -> Result<()> {
        if self.env.borrow().parent.is_none() {
            return Err(PyError::Runtime(
                "nonlocal declaration not allowed at module level".to_string(),
            ));
        }

        for name in names {
            if self.env.borrow().global_names.contains(name) {
                return Err(PyError::Runtime(format!(
                    "name '{}' is nonlocal and global",
                    name
                )));
            }
            if !has_enclosing_local_binding(&self.env, name) {
                return Err(PyError::Runtime(format!(
                    "no binding for nonlocal '{}' found",
                    name
                )));
            }
        }

        let mut env = self.env.borrow_mut();
        Rc::make_mut(&mut env.nonlocal_names).extend(names.iter().cloned());
        Ok(())
    }

    fn alloc_env(&mut self, parent: Option<EnvRef>) -> EnvRef {
        if let Some(env) = self.env_pool.pop() {
            {
                let mut e = env.borrow_mut();
                e.values.clear();
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

    fn build_class(
        &mut self,
        name: &str,
        bases: &[Expr],
        body: &[Stmt],
    ) -> Result<Rc<RefCell<PyClass>>> {
        let outer_env = Rc::clone(&self.env);
        let class_env = Environment::new(Some(Rc::clone(&outer_env)));

        let previous_env = std::mem::replace(&mut self.env, Rc::clone(&class_env));
        let previous_class_closure = self.class_closure_env.replace(outer_env);
        let signal = self.exec_block(body);
        self.class_closure_env = previous_class_closure;
        self.env = previous_env;

        match signal? {
            ExecSignal::None => {}
            ExecSignal::Break | ExecSignal::Continue => {
                return Err(PyError::Runtime(
                    "break/continue is only valid inside loops".to_string(),
                ));
            }
            ExecSignal::Return(_) => {
                return Err(PyError::Runtime(
                    "return is only valid inside functions".to_string(),
                ));
            }
        }

        let base = if let Some(base_expr) = bases.first() {
            match self.eval_expr(base_expr)? {
                Value::Class(class) => Some(class),
                _ => return Err(PyError::Runtime("class base must be a class".to_string())),
            }
        } else { None };

        let attrs = class_env.borrow().values.clone();
        Ok(Rc::new(RefCell::new(PyClass {
            name: name.to_string(),
            base,
            attrs,
        })))
    }

}
