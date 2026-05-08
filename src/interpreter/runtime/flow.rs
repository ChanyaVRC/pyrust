impl Interpreter {
    fn context_manager_protocol_error() -> PyError {
        PyError::Runtime("object does not support the context manager protocol".to_string())
    }

    fn call_context_enter(&mut self, ctx: &Value) -> Result<Value> {
        self.call_instance_method_values(ctx, "__enter__", &[])?
            .ok_or_else(Self::context_manager_protocol_error)
    }

    fn call_context_exit(
        &mut self,
        ctx: &Value,
        exc_type: Value,
        exc_value: Value,
        traceback: Value,
    ) -> Result<Value> {
        self.call_instance_method_values(ctx, "__exit__", &[exc_type, exc_value, traceback])?
            .ok_or_else(Self::context_manager_protocol_error)
    }

    fn assign_target(&mut self, target: &AssignTarget, value: Value) -> Result<()> {
        match target {
            AssignTarget::Name(name) => {
                self.assign_name(name.clone(), value);
                Ok(())
            }
            AssignTarget::Attr(obj_expr, attr) => {
                let obj = self.eval_expr(obj_expr)?;
                match obj {
                    Value::Instance(inst) => {
                        inst.borrow_mut().attrs.insert(attr.clone(), value);
                        Ok(())
                    }
                    _ => Err(PyError::Runtime("can only assign attr on instance".to_string())),
                }
            }
            AssignTarget::Index(target_expr, index_expr) => {
                let index_val = self.eval_expr(index_expr)?;
                self.exec_index_assign(target_expr, index_val, value)
            }
            AssignTarget::Tuple(targets) => {
                let items = self.iter_values(value)?;
                if items.len() != targets.len() {
                    return Err(PyError::Runtime(format!(
                        "not enough values to unpack (expected {}, got {})",
                        targets.len(), items.len()
                    )));
                }
                for (t, v) in targets.iter().zip(items) {
                    self.assign_target(t, v)?;
                }
                Ok(())
            }
        }
    }

    fn load_aug_target_value(&mut self, target: &AssignTarget) -> Result<Value> {
        match target {
            AssignTarget::Name(name) => self
                .lookup_name(name)?
                .ok_or_else(|| PyError::Runtime(format!("name '{}' is not defined", name))),
            AssignTarget::Attr(obj_expr, attr) => {
                let obj = self.eval_expr(obj_expr)?;
                match obj {
                    Value::Instance(inst) => {
                        inst.borrow().attrs.get(attr).cloned().ok_or_else(|| {
                            PyError::Runtime(format!("attribute '{}' not found", attr))
                        })
                    }
                    _ => Err(PyError::Runtime("attr access on non-instance".to_string())),
                }
            }
            AssignTarget::Index(target_expr, index_expr) => {
                let tval = self.eval_expr(target_expr)?;
                let ival = self.eval_expr(index_expr)?;
                self.eval_index(tval, ival)
            }
            AssignTarget::Tuple(_) => Err(PyError::Runtime(
                "illegal target for augmented assignment".to_string(),
            )),
        }
    }

    fn store_aug_target_value(&mut self, target: &AssignTarget, value: Value) -> Result<()> {
        match target {
            AssignTarget::Name(name) => {
                self.assign_name(name.clone(), value);
                Ok(())
            }
            AssignTarget::Attr(obj_expr, attr) => {
                let obj = self.eval_expr(obj_expr)?;
                match obj {
                    Value::Instance(inst) => {
                        inst.borrow_mut().attrs.insert(attr.clone(), value);
                        Ok(())
                    }
                    _ => Err(PyError::Runtime("attr assign on non-instance".to_string())),
                }
            }
            AssignTarget::Index(target_expr, index_expr) => {
                let ival = self.eval_expr(index_expr)?;
                self.exec_index_assign(target_expr, ival, value)
            }
            AssignTarget::Tuple(_) => Err(PyError::Runtime(
                "illegal target for augmented assignment".to_string(),
            )),
        }
    }

    fn exec_aug_assign(&mut self, target: &AssignTarget, op: BinaryOp, rhs: Value) -> Result<()> {
        let current = self.load_aug_target_value(target)?;
        let new_val = if matches!(op, BinaryOp::MatMul) {
            if let Some(value) = self.try_inplace_matmul(current.clone(), rhs.clone())? {
                value
            } else {
                self.eval_binary(current, op, rhs)?
            }
        } else {
            self.eval_binary(current, op, rhs)?
        };
        self.store_aug_target_value(target, new_val)
    }

    fn exec_index_assign(&mut self, target: &Expr, index: Value, value: Value) -> Result<()> {
        match target {
            Expr::Var(name) => self.update_named_collection(name, move |collection| {
                Self::do_index_set(collection, index, value)
            }),
            Expr::Attr {
                target: obj_expr,
                name: attr,
            } => self.update_attr_collection(
                obj_expr,
                attr,
                "cannot index-assign non-instance attribute",
                move |current| Self::do_index_set(current, index, value),
            ),
            _ => Err(PyError::Runtime("illegal target for index assignment".to_string())),
        }
    }

    fn exec_slice_assign(
        &mut self,
        target: &Expr,
        lo: Option<Value>,
        hi: Option<Value>,
        st: Option<Value>,
        rhs: Value,
    ) -> Result<()> {
        match target {
            Expr::Var(name) => self.update_named_collection(name, move |collection| {
                Self::do_slice_set(collection, lo, hi, st, rhs)
            }),
            Expr::Attr {
                target: obj_expr,
                name: attr,
            } => self.update_attr_collection(
                obj_expr,
                attr,
                "cannot slice-assign non-instance attribute",
                move |current| Self::do_slice_set(current, lo, hi, st, rhs),
            ),
            _ => Err(PyError::Runtime(
                "illegal target for slice assignment".to_string(),
            )),
        }
    }

    fn do_index_set(collection: Value, index: Value, value: Value) -> Result<Value> {
        match collection {
            Value::List(mut items) => {
                let idx = normalize_index(&index, items.len())?;
                items[idx] = value;
                Ok(Value::List(items))
            }
            Value::Dict(mut items) => {
                let key = index.to_key().ok_or_else(|| PyError::Runtime("unhashable type".to_string()))?;
                items.insert(key, value);
                Ok(Value::Dict(items))
            }
            _ => Err(PyError::Runtime("object does not support item assignment".to_string())),
        }
    }

    fn update_named_collection<F>(&mut self, name: &str, transform: F) -> Result<()>
    where
        F: FnOnce(Value) -> Result<Value>,
    {
        let name = name.to_string();
        let collection = self
            .lookup_name(&name)?
            .ok_or_else(|| PyError::Runtime(format!("name '{}' is not defined", name)))?;
        let updated = transform(collection)?;
        self.assign_name(name, updated);
        Ok(())
    }

    fn update_attr_collection<F>(
        &mut self,
        obj_expr: &Expr,
        attr: &str,
        non_instance_msg: &str,
        transform: F,
    ) -> Result<()>
    where
        F: FnOnce(Value) -> Result<Value>,
    {
        let obj = self.eval_expr(obj_expr)?;
        match obj {
            Value::Instance(inst) => {
                let current = inst
                    .borrow()
                    .attrs
                    .get(attr)
                    .cloned()
                    .ok_or_else(|| PyError::Runtime(format!("no attribute '{}'", attr)))?;
                let updated = transform(current)?;
                inst.borrow_mut().attrs.insert(attr.to_string(), updated);
                Ok(())
            }
            _ => Err(PyError::Runtime(non_instance_msg.to_string())),
        }
    }

    fn slice_index_from_value(value: &Value) -> Result<i64> {
        match value {
            Value::Int(i) => Ok(*i),
            Value::Bool(b) => Ok(if *b { 1 } else { 0 }),
            _ => Err(PyError::Runtime("slice indices must be integers".to_string())),
        }
    }

    fn resolve_slice_bounds(
        len: i64,
        lo: Option<&Value>,
        hi: Option<&Value>,
        st: Option<&Value>,
    ) -> Result<(i64, i64, i64)> {
        let step = match st {
            None | Some(Value::None) => 1,
            Some(v) => {
                let s = Self::slice_index_from_value(v)?;
                if s == 0 {
                    return Err(PyError::Runtime("slice step cannot be zero".to_string()));
                }
                s
            }
        };

        let normalize = |idx: i64| -> i64 {
            if idx < 0 {
                (idx + len).clamp(0, len)
            } else {
                idx.clamp(0, len)
            }
        };

        let start_default = if step > 0 { 0 } else { len - 1 };
        let end_default = if step > 0 { len } else { -1 };

        let start = match lo {
            None | Some(Value::None) => start_default,
            Some(v) => {
                let i = Self::slice_index_from_value(v)?;
                if step > 0 {
                    normalize(i)
                } else if i < 0 {
                    (i + len).clamp(-1, len - 1)
                } else {
                    i.clamp(-1, len - 1)
                }
            }
        };

        let end = match hi {
            None | Some(Value::None) => end_default,
            Some(v) => {
                let i = Self::slice_index_from_value(v)?;
                if step > 0 {
                    normalize(i)
                } else if i < 0 {
                    (i + len).clamp(-1, len - 1)
                } else {
                    i.clamp(-1, len - 1)
                }
            }
        };

        Ok((start, end, step))
    }

    fn slice_target_indices(len: i64, start: i64, end: i64, step: i64) -> Vec<usize> {
        let mut targets = Vec::new();
        let mut i = start;

        if step > 0 {
            while i < end {
                if i >= 0 && i < len {
                    targets.push(i as usize);
                }
                i += step;
            }
        } else {
            while i > end {
                if i >= 0 && i < len {
                    targets.push(i as usize);
                }
                i += step;
            }
        }
        targets
    }

    fn to_assignment_iterable(value: Value) -> Result<Vec<Value>> {
        match value {
            Value::List(v) => Ok(v),
            Value::Tuple(v) => Ok(v),
            Value::Str(s) => Ok(s.chars().map(|c| Value::Str(c.to_string())).collect()),
            Value::Set(v) => Ok(v),
            Value::Range { start, stop, step } => {
                let mut out = Vec::new();
                if step > 0 {
                    let mut cur = start;
                    while cur < stop {
                        out.push(Value::Int(cur));
                        cur += step;
                    }
                } else {
                    let mut cur = start;
                    while cur > stop {
                        out.push(Value::Int(cur));
                        cur += step;
                    }
                }
                Ok(out)
            }
            other => Err(PyError::Runtime(format!(
                "can only assign an iterable, got {}",
                other.repr()
            ))),
        }
    }

    fn do_slice_set(
        collection: Value,
        lo: Option<Value>,
        hi: Option<Value>,
        st: Option<Value>,
        rhs: Value,
    ) -> Result<Value> {
        let Value::List(mut items) = collection else {
            return Err(PyError::Runtime(
                "object does not support slice assignment".to_string(),
            ));
        };

        let len = items.len() as i64;
        let (start, end, step) =
            Self::resolve_slice_bounds(len, lo.as_ref(), hi.as_ref(), st.as_ref())?;
        let replacement = Self::to_assignment_iterable(rhs)?;

        if step == 1 {
            let start_u = start as usize;
            let end_u = (end.max(start)).min(len) as usize;
            items.splice(start_u..end_u, replacement);
            return Ok(Value::List(items));
        }

        let targets = Self::slice_target_indices(len, start, end, step);

        if targets.len() != replacement.len() {
            return Err(PyError::Runtime(format!(
                "attempt to assign sequence of size {} to extended slice of size {}",
                replacement.len(),
                targets.len()
            )));
        }

        for (idx, value) in targets.into_iter().zip(replacement.into_iter()) {
            items[idx] = value;
        }
        Ok(Value::List(items))
    }

    fn do_slice_delete(
        collection: Value,
        lo: Option<Value>,
        hi: Option<Value>,
        st: Option<Value>,
    ) -> Result<Value> {
        let Value::List(mut items) = collection else {
            return Err(PyError::Runtime(
                "object does not support item deletion".to_string(),
            ));
        };

        let len = items.len() as i64;
        let (start, end, step) =
            Self::resolve_slice_bounds(len, lo.as_ref(), hi.as_ref(), st.as_ref())?;

        if step == 1 {
            let start_u = start as usize;
            let end_u = (end.max(start)).min(len) as usize;
            items.splice(start_u..end_u, std::iter::empty());
            return Ok(Value::List(items));
        }

        let mut targets = Self::slice_target_indices(len, start, end, step);

        // Delete from back to front to keep earlier indices stable.
        targets.sort_unstable();
        for idx in targets.into_iter().rev() {
            items.remove(idx);
        }
        Ok(Value::List(items))
    }

    fn exec_delete(&mut self, expr: &Expr) -> Result<()> {
        match expr {
            Expr::Var(name) => {
                let removed = self.env.borrow_mut().values.remove(name);
                if removed.is_none() {
                    return Err(PyError::Runtime(format!("name '{}' is not defined", name)));
                }
                Ok(())
            }
            Expr::Attr { target, name } => {
                let val = self.eval_expr(target)?;
                match val {
                    Value::Instance(inst) => {
                        inst.borrow_mut().attrs.remove(name);
                        Ok(())
                    }
                    _ => Err(PyError::Runtime("can only delete attr on instance".to_string())),
                }
            }
            Expr::Index { target, index } => {
                let index_val = self.eval_expr(index)?;
                match target.as_ref() {
                    Expr::Var(name) => self.update_named_collection(name, move |collection| {
                        let updated = match collection {
                            Value::Dict(mut items) => {
                                let key = index_val
                                    .to_key()
                                    .ok_or_else(|| PyError::Runtime("unhashable type".to_string()))?;
                                items.shift_remove(&key);
                                Value::Dict(items)
                            }
                            Value::List(mut items) => {
                                let idx = normalize_index(&index_val, items.len())?;
                                items.remove(idx);
                                Value::List(items)
                            }
                            _ => {
                                return Err(PyError::Runtime(
                                    "object does not support item deletion".to_string(),
                                ))
                            }
                        };
                        Ok(updated)
                    }),
                    _ => Err(PyError::Runtime("cannot delete indexed item on complex expression".to_string())),
                }
            }
            Expr::Slice {
                target,
                lower,
                upper,
                step,
            } => {
                let lo = lower.as_ref().map(|e| self.eval_expr(e)).transpose()?;
                let hi = upper.as_ref().map(|e| self.eval_expr(e)).transpose()?;
                let st = step.as_ref().map(|e| self.eval_expr(e)).transpose()?;
                match target.as_ref() {
                    Expr::Var(name) => self.update_named_collection(name, move |collection| {
                        Self::do_slice_delete(collection, lo, hi, st)
                    }),
                    Expr::Attr {
                        target: obj_expr,
                        name: attr,
                    } => self.update_attr_collection(
                        obj_expr,
                        attr,
                        "cannot delete slice on non-instance attribute",
                        move |current| Self::do_slice_delete(current, lo, hi, st),
                    ),
                    _ => Err(PyError::Runtime(
                        "cannot delete sliced item on complex expression".to_string(),
                    )),
                }
            }
            _ => Err(PyError::Runtime("cannot delete this expression".to_string())),
        }
    }

    fn exec_with(&mut self, items: &[(Expr, Option<AssignTarget>)], body: &[Stmt]) -> Result<ExecSignal> {
        let mut contexts: Vec<Value> = Vec::with_capacity(items.len());

        for (ctx_expr, alias) in items {
            let ctx = match self.eval_expr(ctx_expr) {
                Ok(v) => v,
                Err(err) => return self.unwind_with_contexts(contexts, Some(err), ExecSignal::None),
            };

            let entered = match self.call_context_enter(&ctx) {
                Ok(v) => v,
                Err(err) => return self.unwind_with_contexts(contexts, Some(err), ExecSignal::None),
            };

            if let Some(target) = alias {
                if let Err(err) = self.assign_target(target, entered) {
                    return self.unwind_with_contexts(contexts, Some(err), ExecSignal::None);
                }
            }

            contexts.push(ctx);
        }

        let (signal, pending_error) = match self.exec_block(body) {
            Ok(signal) => (signal, None),
            Err(err) => (ExecSignal::None, Some(err)),
        };
        self.unwind_with_contexts(contexts, pending_error, signal)
    }

    fn unwind_with_contexts(
        &mut self,
        contexts: Vec<Value>,
        mut pending_error: Option<PyError>,
        signal: ExecSignal,
    ) -> Result<ExecSignal> {
        for ctx in contexts.into_iter().rev() {
            let mut had_error = false;
            let mut exception_for_exit: Option<Value> = None;

            let (exc_type, exc_value, traceback) = if let Some(err) = pending_error.take() {
                had_error = true;
                match self.error_to_exception(err) {
                    Ok(exception) => {
                        let exc_type = match &exception {
                            Value::Instance(inst) => Value::Class(Rc::clone(&inst.borrow().class)),
                            _ => Value::None,
                        };
                        exception_for_exit = Some(exception.clone());
                        (exc_type, exception, Value::Str("<traceback>".to_string()))
                    }
                    Err(err) => {
                        pending_error = Some(err);
                        (Value::None, Value::None, Value::None)
                    }
                }
            } else {
                (Value::None, Value::None, Value::None)
            };

            let exit_result = match self.call_context_exit(&ctx, exc_type, exc_value, traceback) {
                Ok(v) => v,
                Err(err) => {
                    pending_error = Some(err);
                    continue;
                }
            };

            if had_error {
                if exit_result.truthy() {
                    pending_error = None;
                } else if let Some(exception) = exception_for_exit {
                    pending_error = Some(PyError::Raised(exception));
                }
            }
        }

        if let Some(err) = pending_error {
            return Err(err);
        }
        Ok(signal)
    }

    fn exec_try(
        &mut self,
        body: &[Stmt],
        handlers: &[ExceptHandler],
        else_branch: Option<&[Stmt]>,
        finally_branch: Option<&[Stmt]>,
    ) -> Result<ExecSignal> {
        let mut outcome = match self.exec_block(body) {
            Ok(ExecSignal::None) => {
                if let Some(branch) = else_branch {
                    BlockOutcome::from_result(self.exec_block(branch))
                } else {
                    BlockOutcome::Signal(ExecSignal::None)
                }
            }
            Ok(signal) => BlockOutcome::Signal(signal),
            Err(error) => BlockOutcome::Error(error),
        };

        if let BlockOutcome::Error(error) = outcome {
            let exception = self.error_to_exception(error)?;
            outcome = self.handle_exception(handlers, exception)?;
        }

        if let Some(branch) = finally_branch {
            outcome = match self.exec_block(branch) {
                Ok(ExecSignal::None) => outcome,
                Ok(signal) => BlockOutcome::Signal(signal),
                Err(error) => BlockOutcome::Error(error),
            };
        }

        outcome.into_result()
    }

    fn handle_exception(
        &mut self,
        handlers: &[ExceptHandler],
        exception: Value,
    ) -> Result<BlockOutcome> {
        for handler in handlers {
            let matches = match &handler.kind {
                Some(expr) => {
                    let kind = self.eval_expr(expr)?;
                    self.exception_matches(&exception, &kind)?
                }
                None => true,
            };

            if !matches {
                continue;
            }

            let previous_active = self.active_exception.replace(exception.clone());
            let previous_binding = if let Some(name) = &handler.name {
                self.env
                    .borrow_mut()
                    .values
                    .insert(name.clone(), exception.clone())
            } else {
                None
            };

            let result = BlockOutcome::from_result(self.exec_block(&handler.body));

            self.active_exception = previous_active;
            if let Some(name) = &handler.name {
                let mut env = self.env.borrow_mut();
                if let Some(value) = previous_binding {
                    env.values.insert(name.clone(), value);
                } else {
                    env.values.remove(name);
                }
            }

            return Ok(result);
        }

        Ok(BlockOutcome::Error(PyError::Raised(exception)))
    }

    fn error_to_exception(&self, error: PyError) -> Result<Value> {
        match error {
            PyError::Raised(value) => Ok(value),
            PyError::Runtime(message) => self.instantiate_named_exception("RuntimeError", message),
            other => Err(other),
        }
    }

    fn raise_value_from_expr(&mut self, expr: &Expr) -> Result<Value> {
        let value = self.eval_expr(expr)?;
        self.coerce_to_exception(value)
    }

    fn coerce_to_exception(&self, value: Value) -> Result<Value> {
        match value {
            Value::Instance(instance) => {
                if is_exception_class(&instance.borrow().class) {
                    Ok(Value::Instance(instance))
                } else {
                    Err(PyError::Runtime(
                        "exceptions must derive from Exception".to_string(),
                    ))
                }
            }
            Value::Class(class) => {
                if is_exception_class(&class) {
                    Ok(instantiate_exception(class, Vec::new()))
                } else {
                    Err(PyError::Runtime(
                        "exceptions must derive from Exception".to_string(),
                    ))
                }
            }
            _ => Err(PyError::Runtime(
                "exceptions must derive from Exception".to_string(),
            )),
        }
    }

    fn instantiate_named_exception(&self, name: &str, message: String) -> Result<Value> {
        let Some(Value::Class(class)) = lookup_name_in_module(&self.env, name) else {
            return Err(PyError::Runtime(format!(
                "built-in exception '{}' is not defined",
                name
            )));
        };
        Ok(instantiate_exception(class, vec![Value::Str(message)]))
    }

    fn exception_matches(&self, exception: &Value, kind: &Value) -> Result<bool> {
        let Value::Instance(instance) = exception else {
            return Ok(false);
        };

        let raised_class = Rc::clone(&instance.borrow().class);
        match kind {
            Value::Class(expected) => {
                if !is_exception_class(expected) {
                    return Err(PyError::Runtime(
                        "except clause must reference an exception class".to_string(),
                    ));
                }
                Ok(class_is_subclass_of(&raised_class, expected))
            }
            Value::Tuple(items) => {
                for item in items {
                    let Value::Class(expected) = item else {
                        return Err(PyError::Runtime(
                            "except clause must reference an exception class".to_string(),
                        ));
                    };
                    if !is_exception_class(expected) {
                        return Err(PyError::Runtime(
                            "except clause must reference an exception class".to_string(),
                        ));
                    }
                    if class_is_subclass_of(&raised_class, expected) {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            _ => Err(PyError::Runtime(
                "except clause must reference an exception class".to_string(),
            )),
        }
    }

}
