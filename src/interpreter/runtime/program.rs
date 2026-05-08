impl Interpreter {
    pub fn with_script_dir(dir: PathBuf) -> Self {
        let mut interp = Self::default();
        interp.script_dir = Some(dir);
        interp
    }

    pub fn exec_program(&mut self, program: &[Stmt], repl_mode: bool) -> Result<()> {
        for stmt in program {
            match stmt {
                Stmt::Expr(expr) if repl_mode => {
                    let value = self.eval_expr(expr)?;
                    if value != Value::None {
                        println!("{}", value.to_py_str());
                    }
                }
                _ => {
                    let signal = self.exec_stmt(stmt)?;
                    match signal {
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
                }
            }
        }
        Ok(())
    }

    fn exec_block(&mut self, block: &[Stmt]) -> Result<ExecSignal> {
        for stmt in block {
            let signal = self.exec_stmt(stmt)?;
            if signal != ExecSignal::None {
                return Ok(signal);
            }
        }
        Ok(ExecSignal::None)
    }

    fn exec_stmt(&mut self, stmt: &Stmt) -> Result<ExecSignal> {
        match stmt {
            Stmt::Assign(target, expr) => {
                let val = self.eval_expr(expr)?;
                self.assign_target(target, val)?;
                Ok(ExecSignal::None)
            }
            Stmt::AttrAssign { target, name, expr } => {
                let target_value = self.eval_expr(target)?;
                let value = self.eval_expr(expr)?;
                self.assign_attr(target_value, name, value)?;
                Ok(ExecSignal::None)
            }
            Stmt::Def {
                name,
                params,
                body,
                decorators,
            } => {
                let closure_env = self
                    .class_closure_env
                    .clone()
                    .unwrap_or_else(|| Rc::clone(&self.env));
                let function = self.create_user_function(name, params, body, closure_env)?;
                let mut bound = Value::Function(Rc::clone(&function));
                bound = self.apply_decorators(bound, decorators)?;
                self.assign_name(name.clone(), bound);
                Ok(ExecSignal::None)
            }
            Stmt::Class {
                name,
                bases,
                body,
                decorators,
            } => {
                let class = self.build_class(name, bases.as_slice(), body)?;
                let mut bound = Value::Class(class);
                bound = self.apply_decorators(bound, decorators)?;
                self.assign_name(name.clone(), bound);
                Ok(ExecSignal::None)
            }
            Stmt::Global(names) => {
                self.declare_global_names(names);
                Ok(ExecSignal::None)
            }
            Stmt::Nonlocal(names) => {
                self.declare_nonlocal_names(names)?;
                Ok(ExecSignal::None)
            }
            Stmt::IndexAssign { target, index, expr } => {
                let index_val = self.eval_expr(index)?;
                let value = self.eval_expr(expr)?;
                self.exec_index_assign(target, index_val, value)?;
                Ok(ExecSignal::None)
            }
            Stmt::SliceAssign {
                target,
                lower,
                upper,
                step,
                expr,
            } => {
                let lo = lower.as_ref().map(|e| self.eval_expr(e)).transpose()?;
                let hi = upper.as_ref().map(|e| self.eval_expr(e)).transpose()?;
                let st = step.as_ref().map(|e| self.eval_expr(e)).transpose()?;
                let rhs = self.eval_expr(expr)?;
                self.exec_slice_assign(target, lo, hi, st, rhs)?;
                Ok(ExecSignal::None)
            }
            Stmt::AugAssign { target, op, expr } => {
                let rhs = self.eval_expr(expr)?;
                self.exec_aug_assign(target, *op, rhs)?;
                Ok(ExecSignal::None)
            }
            Stmt::Delete(exprs) => {
                for expr in exprs {
                    self.exec_delete(expr)?;
                }
                Ok(ExecSignal::None)
            }
            Stmt::Assert { test, msg } => {
                if !self.eval_expr(test)?.truthy() {
                    let msg_str = match msg {
                        Some(expr) => self.eval_expr(expr)?.to_py_str(),
                        None => String::new(),
                    };
                    let exc = self.instantiate_named_exception("AssertionError", msg_str)?;
                    return Err(PyError::Raised(exc));
                }
                Ok(ExecSignal::None)
            }
            Stmt::With { items, body } => {
                self.exec_with(items, body)
            }
            Stmt::Expr(expr) => {
                self.eval_expr(expr)?;
                Ok(ExecSignal::None)
            }
            Stmt::If {
                branches,
                else_branch,
            } => {
                for (cond, body) in branches {
                    if self.eval_expr(cond)?.truthy() {
                        return self.exec_block(body);
                    }
                }
                if let Some(body) = else_branch {
                    return self.exec_block(body);
                }
                Ok(ExecSignal::None)
            }
            Stmt::While {
                cond,
                body,
                else_branch,
            } => {
                let mut broke = false;
                while self.eval_expr(cond)?.truthy() {
                    match self.exec_block(body)? {
                        ExecSignal::None => {}
                        ExecSignal::Break => {
                            broke = true;
                            break;
                        }
                        ExecSignal::Continue => continue,
                        ExecSignal::Return(value) => return Ok(ExecSignal::Return(value)),
                    }
                }
                if !broke {
                    if let Some(branch) = else_branch {
                        return self.exec_block(branch);
                    }
                }
                Ok(ExecSignal::None)
            }
            Stmt::For {
                target,
                iter,
                body,
                else_branch,
            } => {
                let iter_value = self.eval_expr(iter)?;
                let values = iter_values(iter_value)?;
                let mut broke = false;
                for value in values {
                    self.assign_target(target, value)?;
                    match self.exec_block(body)? {
                        ExecSignal::None => {}
                        ExecSignal::Break => {
                            broke = true;
                            break;
                        }
                        ExecSignal::Continue => continue,
                        ExecSignal::Return(value) => return Ok(ExecSignal::Return(value)),
                    }
                }
                if !broke {
                    if let Some(branch) = else_branch {
                        return self.exec_block(branch);
                    }
                }
                Ok(ExecSignal::None)
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
            } => self.exec_try(
                body,
                handlers,
                else_branch.as_deref(),
                finally_branch.as_deref(),
            ),
            Stmt::Raise { expr, cause } => {
                let exception = match expr {
                    Some(expr) => self.raise_value_from_expr(expr)?,
                    None => self.active_exception.clone().ok_or_else(|| {
                        PyError::Runtime("No active exception to reraise".to_string())
                    })?,
                };

                if let Some(cause_expr) = cause {
                    let cause_val = self.eval_expr(cause_expr)?;
                    let cause_exc = if cause_val == Value::None {
                        Value::None
                    } else {
                        self.coerce_to_exception(cause_val)?
                    };
                    if let Value::Instance(inst) = &exception {
                        inst.borrow_mut()
                            .attrs
                            .insert("__cause__".to_string(), cause_exc);
                    }
                }
                Err(PyError::Raised(exception))
            }
            Stmt::Import { names } => {
                for (module_name, alias) in names {
                    let module_val = self.load_module(module_name)?;
                    // Bind top-level name: alias || first segment of dotted name
                    let bound_name = alias.clone().unwrap_or_else(|| {
                        module_name
                            .split('.')
                            .next()
                            .unwrap_or(module_name)
                            .to_string()
                    });
                    // For dotted names without alias we must set nested attrs.
                    // e.g. `import os.path` binds `os` with attr `path = <module os.path>`.
                    // Simple case: single segment or alias -> just assign.
                    if alias.is_some() || !module_name.contains('.') {
                        self.assign_name(bound_name, module_val);
                    } else {
                        // Dotted without alias: bind first segment as a module with nested attrs.
                        let segments: Vec<&str> = module_name.split('.').collect();
                        // For simplicity, just assign the leaf module value under the first segment.
                        // Real Python would create intermediate package objects.
                        if let Some(existing) = self.lookup_name(segments[0])? {
                            // Add the leaf as attribute of existing top-level
                            if let Value::Module(m) = existing {
                                let leaf_name = segments[segments.len() - 1].to_string();
                                m.borrow_mut().attrs.insert(leaf_name, module_val);
                            }
                        } else {
                            // Create a minimal stub module for the top-level name
                            let leaf_val = module_val;
                            let stub = Value::Module(Rc::new(RefCell::new(PyModule {
                                name: segments[0].to_string(),
                                attrs: {
                                    let mut m = HashMap::new();
                                    m.insert(segments[1].to_string(), leaf_val);
                                    m
                                },
                            })));
                            self.assign_name(bound_name, stub);
                        }
                    }
                }
                Ok(ExecSignal::None)
            }
            Stmt::ImportFrom { module, names } => {
                let module_val = self.load_module(module)?;
                if names.len() == 1 && names[0].0 == "*" {
                    // Star import: copy all public attrs into current scope
                    if let Value::Module(m) = module_val {
                        let attrs: Vec<(String, Value)> = m
                            .borrow()
                            .attrs
                            .iter()
                            .filter(|(k, _)| !k.starts_with('_'))
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        for (name, val) in attrs {
                            self.assign_name(name, val);
                        }
                    } else {
                        return Err(PyError::Runtime(format!(
                            "cannot star-import from non-module '{module}'"
                        )));
                    }
                } else {
                    for (attr_name, alias) in names {
                        let val = self.get_attr(module_val.clone(), attr_name).map_err(|_| {
                            PyError::Runtime(format!(
                                "cannot import name '{attr_name}' from '{module}'"
                            ))
                        })?;
                        let bound = alias.clone().unwrap_or_else(|| attr_name.clone());
                        self.assign_name(bound, val);
                    }
                }
                Ok(ExecSignal::None)
            }
            Stmt::Return(expr) => {
                let value = match expr {
                    Some(expr) => self.eval_expr(expr)?,
                    None => Value::None,
                };
                Ok(ExecSignal::Return(value))
            }
            Stmt::Break => Ok(ExecSignal::Break),
            Stmt::Continue => Ok(ExecSignal::Continue),
            Stmt::Pass => Ok(ExecSignal::None),
        }
    }

    fn apply_decorators(&mut self, mut value: Value, decorators: &[Expr]) -> Result<Value> {
        for deco_expr in decorators.iter().rev() {
            let deco = self.eval_expr(deco_expr)?;
            value = self.call_function_expanded(
                deco,
                &[ExpandedCallArg {
                    name: None,
                    value,
                }],
            )?;
        }
        Ok(value)
    }

}
