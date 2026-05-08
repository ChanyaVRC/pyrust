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

    #[inline(always)]
    fn exec_block(&mut self, block: &[Stmt]) -> Result<ExecSignal> {
        for stmt in block {
            let signal = self.exec_stmt(stmt)?;
            if signal != ExecSignal::None {
                return Ok(signal);
            }
        }
        Ok(ExecSignal::None)
    }

    // Drives a range(start, stop, step) loop.
    //
    // Pre-computes the total number of iterations once (range_len), then
    // counts down with a single comparison per iteration instead of two
    // (step-sign check + value vs stop).
    //
    // For a simple Name target, the target env is also resolved once and the
    // slot is updated in-place — no String key clone after the first iteration,
    // no per-iteration global/nonlocal HashSet checks.
    fn exec_range_loop(
        &mut self,
        start: i64,
        stop: i64,
        step: i64,
        target: &AssignTarget,
        body: &[Stmt],
        else_branch: Option<&[Stmt]>,
    ) -> Result<ExecSignal> {
        if step == 0 {
            return Err(PyError::Runtime(
                "range() step argument must not be zero".to_string(),
            ));
        }

        let mut broke = false;

        // step=1 fast path: avoids division in range_len and reduces per-iteration ops
        if step == 1 {
            if let AssignTarget::Name(loop_var) = target {
                let target_env = self.resolve_assign_env_for(loop_var);
                let mut cur = start;
                'step1_name: loop {
                    if cur >= stop {
                        break;
                    }
                    env_assign_local(&target_env, loop_var, Value::Int(cur));
                    match self.exec_block(body)? {
                        ExecSignal::None => {}
                        ExecSignal::Break => {
                            broke = true;
                            break 'step1_name;
                        }
                        ExecSignal::Continue => {}
                        ExecSignal::Return(v) => return Ok(ExecSignal::Return(v)),
                    }
                    cur += 1;
                }
            } else {
                let mut cur = start;
                loop {
                    if cur >= stop {
                        break;
                    }
                    self.assign_target(target, Value::Int(cur))?;
                    match self.exec_block(body)? {
                        ExecSignal::None => {}
                        ExecSignal::Break => {
                            broke = true;
                            break;
                        }
                        ExecSignal::Continue => {}
                        ExecSignal::Return(v) => return Ok(ExecSignal::Return(v)),
                    }
                    cur += 1;
                }
            }
            if !broke {
                if let Some(branch) = else_branch {
                    return self.exec_block(branch);
                }
            }
            return Ok(ExecSignal::None);
        }

        // Pre-compute iteration count (one division, done once).
        let mut remaining = range_len(start, stop, step);
        if let AssignTarget::Name(loop_var) = target {
            let target_env = self.resolve_assign_env_for(loop_var);
            let mut cur = start;
            'range_loop: loop {
                if remaining == 0 { break; }
                env_assign_local(&target_env, loop_var, Value::Int(cur));
                match self.exec_block(body)? {
                    ExecSignal::None => {}
                    ExecSignal::Break => { broke = true; break 'range_loop; }
                    ExecSignal::Continue => {}
                    ExecSignal::Return(v) => return Ok(ExecSignal::Return(v)),
                }
                cur += step;
                remaining -= 1;
            }
        } else {
            let mut cur = start;
            'range_loop: loop {
                if remaining == 0 { break; }
                self.assign_target(target, Value::Int(cur))?;
                match self.exec_block(body)? {
                    ExecSignal::None => {}
                    ExecSignal::Break => { broke = true; break 'range_loop; }
                    ExecSignal::Continue => {}
                    ExecSignal::Return(v) => return Ok(ExecSignal::Return(v)),
                }
                cur += step;
                remaining -= 1;
            }
        }
        if !broke {
            if let Some(branch) = else_branch {
                return self.exec_block(branch);
            }
        }
        Ok(ExecSignal::None)
    }

    // Drives a loop over a pre-collected Vec<Value>.  Same Name-target
    // optimization as exec_range_loop: env resolved once, slot updated in-place.
    fn exec_items_loop(
        &mut self,
        items: Vec<Value>,
        target: &AssignTarget,
        body: &[Stmt],
        else_branch: Option<&[Stmt]>,
    ) -> Result<ExecSignal> {
        let mut broke = false;
        if let AssignTarget::Name(loop_var) = target {
            let target_env = self.resolve_assign_env_for(loop_var);
            for value in items {
                env_assign_local(&target_env, loop_var, value);
                match self.exec_block(body)? {
                    ExecSignal::None => {}
                    ExecSignal::Break => { broke = true; break; }
                    ExecSignal::Continue => continue,
                    ExecSignal::Return(v) => return Ok(ExecSignal::Return(v)),
                }
            }
        } else {
            for value in items {
                self.assign_target(target, value)?;
                match self.exec_block(body)? {
                    ExecSignal::None => {}
                    ExecSignal::Break => { broke = true; break; }
                    ExecSignal::Continue => continue,
                    ExecSignal::Return(v) => return Ok(ExecSignal::Return(v)),
                }
            }
        }
        if !broke {
            if let Some(branch) = else_branch {
                return self.exec_block(branch);
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
                // Constant-false: skip body and always run else (if present).
                // Mirrors CPython's peephole folding of dead while-bodies.
                if is_const_false(cond) {
                    return if let Some(branch) = else_branch {
                        self.exec_block(branch)
                    } else {
                        Ok(ExecSignal::None)
                    };
                }

                let mut broke = false;
                // Constant-true (`while True` / `while 1`): skip the condition
                // re-evaluation on every iteration — the condition is never
                // going to flip.  Analogous to CPython's `JUMP_BACKWARD`
                // replacing `LOAD_CONST True; POP_JUMP_IF_FALSE` after
                // peephole optimisation.
                if is_const_true(cond) {
                    loop {
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
                } else {
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
                // Named variable fast-path: one env borrow avoids cloning the
                // iterable Value and, for Range, prevents a second borrow.
                if let Expr::Var(name) = iter {
                    if let Some(env) = self.resolve_name_env(name) {
                        let range_spec: Option<(i64, i64, i64)> = {
                            let borrowed = env.borrow();
                            borrowed.values.get(name).and_then(|v| {
                                if let Value::Range { start, stop, step } = v {
                                    Some((*start, *stop, *step))
                                } else {
                                    None
                                }
                            })
                        };
                        if let Some((start, stop, step)) = range_spec {
                            return self.exec_range_loop(start, stop, step, target, body, else_branch.as_deref());
                        }

                        let fast_items: Option<Vec<Value>> = {
                            let borrowed = env.borrow();
                            borrowed.values.get(name).and_then(|v| match v {
                                Value::List(items) | Value::Tuple(items) => Some(items.clone()),
                                Value::Dict(map) => {
                                    Some(map.keys().map(|k| key_to_value(k.clone())).collect())
                                }
                                Value::Set(set) => {
                                    Some(set.iter().map(|k| key_to_value(k.clone())).collect())
                                }
                                Value::Str(s) => {
                                    Some(s.chars().map(|c| Value::Str(c.to_string())).collect())
                                }
                                _ => None,
                            })
                        };
                        if let Some(items) = fast_items {
                            return self.exec_items_loop(items, target, body, else_branch.as_deref());
                        }
                    }
                }

                let iter_value = self.eval_expr(iter)?;
                if let Value::Range { start, stop, step } = iter_value {
                    return self.exec_range_loop(start, stop, step, target, body, else_branch.as_deref());
                }
                let values = iter_values(iter_value)?;
                self.exec_items_loop(values, target, body, else_branch.as_deref())
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
                Ok(ExecSignal::Return(Box::new(value)))
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
