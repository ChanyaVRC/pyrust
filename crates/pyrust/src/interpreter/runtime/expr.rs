impl Interpreter {
    fn unsupported_binary_operand(op: &str) -> PyError {
        PyError::Named("TypeError".to_string(), format!("unsupported operand type(s) for {op}"))
    }

    fn eval_expr(&mut self, expr: &Expr) -> Result<Value> {
        match expr {
            Expr::Int(v) => Ok(Value::int(*v)),
            Expr::Float(v) => Ok(Value::float(*v)),
            Expr::Str(v) => Ok(Value::string(v.clone())),
            Expr::Bytes(v) => Ok(Value::bytes(v.clone())),
            Expr::Complex(re, im) => Ok(Value::complex(*re, *im)),
            Expr::Bool(v) => Ok(Value::bool_(*v)),
            Expr::None => Ok(Value::none()),
            Expr::Var(name) => {
                if let Some(v) = self.lookup_name(name)? {
                    return Ok(v.clone());
                }
                resolve_builtin(name)
                    .map(Ok)
                    .unwrap_or_else(|| Err(PyError::Runtime(format!("name '{name}' is not defined"))))
            }
            Expr::List(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(self.eval_expr(item)?);
                }
                Ok(Value::list(out))
            }
            Expr::Dict(items) => {
                let mut out = indexmap::IndexMap::new();
                for (kexpr, vexpr) in items {
                    let key_val = self.eval_expr(kexpr)?;
                    let key = key_val.to_key().ok_or_else(|| {
                        PyError::Runtime("unhashable type in dict key".to_string())
                    })?;
                    let value = self.eval_expr(vexpr)?;
                    out.insert(key, value);
                }
                Ok(Value::dict(out))
            }
            Expr::Unary { op, expr } => {
                let value = self.eval_expr(expr)?;
                match op {
                    UnaryOp::Neg => {
                        if let Some(r) = self.try_dunder_unary(&value, "__neg__") {
                            return r;
                        }
                        match value.kind() {
                            ValueKind::Int(v) => Ok(Value::int(-v)),
                            ValueKind::Float(v) => Ok(Value::float(-v)),
                            ValueKind::Complex(re, im) => Ok(Value::complex(-re, -im)),
                            _ => Err(PyError::Named("TypeError".to_string(), "bad operand type for unary -".to_string())),
                        }
                    }
                    UnaryOp::Not => Ok(Value::bool_(!self.truthy_value(&value)?)),
                    UnaryOp::BitNot => {
                        if let Some(r) = self.try_dunder_unary(&value, "__invert__") {
                            return r;
                        }
                        match value.kind() {
                            ValueKind::Int(v) => Ok(Value::int(!v)),
                            ValueKind::Bool(b) => Ok(Value::int(if b { -2 } else { -1 })),
                            _ => Err(PyError::Named("TypeError".to_string(), "bad operand type for unary ~: use integer".to_string())),
                        }
                    }
                    UnaryOp::Pos => {
                        if let Some(r) = self.try_dunder_unary(&value, "__pos__") {
                            return r;
                        }
                        match value.kind() {
                            ValueKind::Int(v) => Ok(Value::int(v)),
                            ValueKind::Float(v) => Ok(Value::float(v)),
                            ValueKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
                            _ => Err(PyError::Named("TypeError".to_string(), "bad operand type for unary +".to_string())),
                        }
                    }
                }
            }
            Expr::Binary { left, op, right } => match op {
                BinaryOp::And => {
                    let left_value = self.eval_expr(left)?;
                    if !left_value.truthy() {
                        Ok(left_value)
                    } else {
                        self.eval_expr(right)
                    }
                }
                BinaryOp::Or => {
                    let left_value = self.eval_expr(left)?;
                    if left_value.truthy() {
                        Ok(left_value)
                    } else {
                        self.eval_expr(right)
                    }
                }
                _ => {
                    let left_value = self.eval_expr(left)?;
                    let right_value = self.eval_expr(right)?;
                    self.eval_binary_speculative(expr, *op, left_value, right_value)
                }
            },
            Expr::Call { func, args } => {
                let function = self.eval_expr(func)?;
                self.call_function(function, args)
            }
            Expr::Attr { target, name } => {
                let target_value = self.eval_expr(target)?;
                self.get_attr(target_value, name)
            }
            Expr::Index { target, index } => {
                let index_value = self.eval_expr(index)?;
                if let Expr::Var(name) = target.as_ref()
                    && let Some(env) = self.resolve_name_env(name) {
                        let result: Option<Result<Value>> = {
                            let borrowed = env.borrow();
                            let col = if let Some(fl) = &borrowed.fastlocals {
                                if let Some(&idx) = fl.index.get(name.as_str()) {
                                    fl.slots[idx].as_ref()
                                } else {
                                    borrowed.values.get(name.as_str())
                                }
                            } else {
                                borrowed.values.get(name.as_str())
                            };
                            col.map(|col| match col.kind() {
                                ValueKind::List(items) => {
                                    let idx = normalize_index(&index_value, items.len(), "list")?;
                                    Ok(items[idx].clone())
                                }
                                ValueKind::Tuple(items) => {
                                    let idx = normalize_index(&index_value, items.len(), "tuple")?;
                                    Ok(items[idx].clone())
                                }
                                ValueKind::Dict(items) => {
                                    let key = index_value.to_key().ok_or_else(|| {
                                        PyError::Runtime("unhashable key type".to_string())
                                    })?;
                                    items
                                        .get(&key)
                                        .cloned()
                                        .ok_or_else(|| PyError::Runtime("key error".to_string()))
                                }
                                _ => Err(PyError::Runtime(
                                    "object is not subscriptable".to_string(),
                                )),
                            })
                        };
                        if let Some(r) = result {
                            return r;
                        }
                    }
                let target_value = self.eval_expr(target)?;
                self.eval_index(target_value, index_value)
            }
            Expr::Slice { target, lower, upper, step } => {
                let target_val = self.eval_expr(target)?;
                let lo = lower.as_ref().map(|e| self.eval_expr(e)).transpose()?;
                let hi = upper.as_ref().map(|e| self.eval_expr(e)).transpose()?;
                let st = step.as_ref().map(|e| self.eval_expr(e)).transpose()?;
                self.eval_slice(target_val, lo, hi, st)
            }
            Expr::Tuple(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items { out.push(self.eval_expr(item)?); }
                Ok(Value::tuple(out))
            }
            Expr::Set(items) => {
                let mut out = indexmap::IndexSet::new();
                for item in items {
                    let v = self.eval_expr(item)?;
                    let key = v.to_key().ok_or_else(|| {
                        PyError::Runtime("unhashable type in set".to_string())
                    })?;
                    out.insert(key);
                }
                Ok(Value::set(out))
            }
            Expr::Compare { left, ops } => {
                let mut prev = self.eval_expr(left)?;
                for (op, right_expr) in ops {
                    let right = self.eval_expr(right_expr)?;
                    let result = self.eval_binary(prev.clone(), cmp_op_to_binary_op(*op), right.clone())?;
                    if !result.truthy() { return Ok(Value::bool_(false)); }
                    prev = right;
                }
                Ok(Value::bool_(true))
            }
            Expr::Ternary { cond, then, else_ } => {
                if self.eval_expr(cond)?.truthy() {
                    self.eval_expr(then)
                } else {
                    self.eval_expr(else_)
                }
            }
            Expr::Lambda { params, body } => {
                let closure = Rc::clone(&self.env);
                let local_names: std::collections::HashSet<String> =
                    params.iter().cloned().collect();
                let local_index = Rc::new(
                    (0u32..).zip(local_names.iter()).map(|(i, n)| (n.clone(), i)).collect::<HashMap<String, crate::bytecode::Reg>>()
                );
                let lambda_body = vec![crate::ast::Stmt::Return(Some(*body.clone()))];
                let func = Rc::new(crate::value::UserFunction {
                    id: crate::value::next_fn_id(),
                    kind: crate::value::UserFunctionKind::Regular,
                    name: "<lambda>".to_string(),
                    params: params.iter().map(|n| crate::value::UserFunctionParam {
                        name: n.clone(), default: None, is_args: false, is_kwargs: false, is_keyword_only: false, is_positional_only: false,
                    }).collect(),
                    is_pure: is_pure_body(&lambda_body, &std::collections::HashSet::new()),
                    local_names: Rc::new(local_names),
                    local_index,
                    global_names: Rc::new(std::collections::HashSet::new()),
                    nonlocal_names: Rc::new(std::collections::HashSet::new()),
                    env: closure,
                    precompiled_code: None,
                });
                Ok(Value::user_function(func))
            }
            Expr::ListComp { .. } | Expr::DictComp { .. } | Expr::SetComp { .. } => {
                Err(PyError::Runtime(
                    "comprehensions are only supported in compiled (bytecode) mode".to_string(),
                ))
            }
            Expr::Named { target, value } => {
                let val = self.eval_expr(value)?;
                self.assign_name(target.clone(), val.clone());
                Ok(val)
            }
            Expr::FString(parts) => {
                use crate::ast::FStringPart;
                let mut result = String::new();
                for part in parts {
                    match part {
                        FStringPart::Literal(s) => result.push_str(s),
                        FStringPart::Expr { expr, conversion, format_spec } => {
                            let val = self.eval_expr(expr)?;
                            let converted = match conversion {
                                Some('r') => Value::string(val.repr()),
                                Some('a') => Value::string(val.repr()),
                                _ => val,
                            };
                            let formatted = if let Some(spec) = format_spec {
                                apply_format_spec(&converted, spec)?
                            } else {
                                Value::string(converted.to_py_str())
                            };
                            result.push_str(formatted.as_str().unwrap_or(""));
                        }
                    }
                }
                Ok(Value::string(result))
            }
            // yield / yield from are only valid inside generator functions; the
            // compiler ensures they are only reachable through the bytecode VM
            // path, never through the tree-walking eval_expr path.
            Expr::Yield(_) | Expr::YieldFrom(_) => {
                Err(PyError::Runtime(
                    "yield outside generator function".to_string(),
                ))
            }
        }
    }

    fn eval_index(&mut self, target: Value, index: Value) -> Result<Value> {
        match target.kind() {
            ValueKind::List(items) => {
                let idx = normalize_index(&index, items.len(), "list")?;
                Ok(items[idx].clone())
            }
            ValueKind::Tuple(items) => {
                let idx = normalize_index(&index, items.len(), "tuple")?;
                Ok(items[idx].clone())
            }
            ValueKind::Str(text) => {
                let chars: Vec<char> = text.chars().collect();
                let idx = normalize_index(&index, chars.len(), "string")?;
                Ok(Value::string(chars[idx].to_string()))
            }
            ValueKind::Bytes(rc) => {
                let idx = normalize_index(&index, rc.len(), "bytes")?;
                Ok(Value::int(rc[idx] as i64))
            }
            ValueKind::Dict(items) => {
                let key = index
                    .to_key()
                    .ok_or_else(|| PyError::Runtime("unhashable key type".to_string()))?;
                items.get(&key).cloned().ok_or_else(|| PyError::Runtime("key error".to_string()))
            }
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__getitem__")
                    && let ValueKind::UserFunction(f) = method_val.kind() {
                        let func = Rc::clone(f);
                        return self.call_user_function_expanded(
                            func,
                            &[ExpandedCallArg { name: None, value: index }],
                            &[Value::py_instance(inst_rc)],
                        );
                    }
                Err(PyError::Named(
                    "TypeError".to_string(),
                    format!(
                        "'{}' object is not subscriptable",
                        class.borrow().name
                    ),
                ))
            }
            _ => Err(PyError::Runtime("object is not subscriptable".to_string())),
        }
    }

    /// Try to call a binary dunder method on `left` (named `method`), then on
    /// `right` (named `rmethod`).  Returns `Some(result)` if a dunder was found
    /// and called, or `None` if neither operand has the method.
    fn try_dunder_binary(
        &mut self,
        left: &Value,
        right: &Value,
        method: &str,
        rmethod: &str,
    ) -> Option<Result<Value>> {
        if let ValueKind::PyInstance(inst) = left.kind() {
            let class = Rc::clone(&inst.borrow().class);
            if let Some(m) = lookup_class_attr(&class, method)
                && let ValueKind::UserFunction(f) = m.kind()
            {
                let func = Rc::clone(f);
                let self_val = Value::py_instance(Rc::clone(inst));
                match self.call_user_function_expanded(func, &[], &[self_val, right.clone()]) {
                    Ok(v) if is_not_implemented(&v) => {}
                    result => return Some(result),
                }
            }
        }
        if let ValueKind::PyInstance(inst) = right.kind() {
            let class = Rc::clone(&inst.borrow().class);
            if let Some(m) = lookup_class_attr(&class, rmethod)
                && let ValueKind::UserFunction(f) = m.kind()
            {
                let func = Rc::clone(f);
                let self_val = Value::py_instance(Rc::clone(inst));
                match self.call_user_function_expanded(func, &[], &[self_val, left.clone()]) {
                    Ok(v) if is_not_implemented(&v) => {}
                    result => return Some(result),
                }
            }
        }
        None
    }

    /// Try to call a unary dunder method on a PyInstance.
    fn try_dunder_unary(&mut self, val: &Value, method: &str) -> Option<Result<Value>> {
        if let ValueKind::PyInstance(inst) = val.kind() {
            let class = Rc::clone(&inst.borrow().class);
            if let Some(m) = lookup_class_attr(&class, method)
                && let ValueKind::UserFunction(f) = m.kind()
            {
                let func = Rc::clone(f);
                let self_val = Value::py_instance(Rc::clone(inst));
                return Some(self.call_user_function_expanded(func, &[], &[self_val]));
            }
        }
        None
    }

    fn eval_binary(&mut self, left: Value, op: BinaryOp, right: Value) -> Result<Value> {
        match op {
            BinaryOp::Add => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__add__", "__radd__") {
                    return r;
                }
                self.add(left, right)
            }
            BinaryOp::Sub => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__sub__", "__rsub__") {
                    return r;
                }
                if let Some(r) = set_binary_op(&left, &right, SetOp::Sub) {
                    return r;
                }
                self.sub(left, right)
            }
            BinaryOp::Mul => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__mul__", "__rmul__") {
                    return r;
                }
                self.mul(left, right)
            }
            BinaryOp::MatMul => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__matmul__", "__rmatmul__") {
                    return r;
                }
                self.matmul(left, right)
            }
            BinaryOp::Div => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__truediv__", "__rtruediv__") {
                    return r;
                }
                self.div(left, right)
            }
            BinaryOp::FloorDiv => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__floordiv__", "__rfloordiv__") {
                    return r;
                }
                self.floor_div(left, right)
            }
            BinaryOp::Mod => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__mod__", "__rmod__") {
                    return r;
                }
                self.modulo(left, right)
            }
            BinaryOp::Eq => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__eq__", "__eq__") {
                    return r;
                }
                Ok(Value::bool_(left == right))
            }
            BinaryOp::Ne => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__ne__", "__ne__") {
                    return r;
                }
                Ok(Value::bool_(left != right))
            }
            BinaryOp::Lt => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__lt__", "__gt__") {
                    return r;
                }
                self.compare(left, right, |o| o.is_lt())
            }
            BinaryOp::Le => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__le__", "__ge__") {
                    return r;
                }
                self.compare(left, right, |o| o.is_le())
            }
            BinaryOp::Gt => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__gt__", "__lt__") {
                    return r;
                }
                self.compare(left, right, |o| o.is_gt())
            }
            BinaryOp::Ge => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__ge__", "__le__") {
                    return r;
                }
                self.compare(left, right, |o| o.is_ge())
            }
            BinaryOp::Pow => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__pow__", "__rpow__") {
                    return r;
                }
                match (left.kind(), right.kind()) {
                    (ValueKind::Int(a), ValueKind::Int(b)) if b >= 0 => {
                        Ok(Value::int(a.wrapping_pow(b as u32)))
                    }
                    _ => {
                        let a = value_to_float(&left, "**")?;
                        let b = value_to_float(&right, "**")?;
                        Ok(Value::float(a.powf(b)))
                    }
                }
            }
            BinaryOp::BitAnd => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__and__", "__rand__") {
                    return r;
                }
                if let Some(r) = set_binary_op(&left, &right, SetOp::And) {
                    return r;
                }
                self.bitwise_op(&left, &right, |a, b| Ok(a & b))
            }
            BinaryOp::BitOr => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__or__", "__ror__") {
                    return r;
                }
                if let Some(r) = set_binary_op(&left, &right, SetOp::Or) {
                    return r;
                }
                self.bitwise_op(&left, &right, |a, b| Ok(a | b))
            }
            BinaryOp::BitXor => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__xor__", "__rxor__") {
                    return r;
                }
                if let Some(r) = set_binary_op(&left, &right, SetOp::Xor) {
                    return r;
                }
                self.bitwise_op(&left, &right, |a, b| Ok(a ^ b))
            }
            BinaryOp::LShift => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__lshift__", "__rlshift__") {
                    return r;
                }
                self.bitwise_op(&left, &right, |a, b| {
                    if b < 0 { return Err(PyError::Named("ValueError".to_string(), "negative shift count".to_string())); }
                    Ok(a << (b & 63))
                })
            }
            BinaryOp::RShift => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__rshift__", "__rrshift__") {
                    return r;
                }
                self.bitwise_op(&left, &right, |a, b| {
                    if b < 0 { return Err(PyError::Named("ValueError".to_string(), "negative shift count".to_string())); }
                    Ok(a >> (b & 63))
                })
            }
            BinaryOp::In => self.eval_in(right, left),
            BinaryOp::NotIn => Ok(Value::bool_(!self.eval_in(right, left)?.truthy())),
            BinaryOp::Is    => Ok(Value::bool_(values_are_identical(&left, &right))),
            BinaryOp::IsNot => Ok(Value::bool_(!values_are_identical(&left, &right))),
            BinaryOp::And | BinaryOp::Or => unreachable!("short-circuit handled earlier"),
        }
    }

    fn add(&self, left: Value, right: Value) -> Result<Value> {
        if let Some((a, b)) = both_as_complex(&left, &right) {
            return Ok(Value::complex(a.0 + b.0, a.1 + b.1));
        }
        let (l, r) = (coerce_numeric(left), coerce_numeric(right));
        match (l.kind(), r.kind()) {
                (ValueKind::Int(a), ValueKind::Int(b)) => Ok(match a.checked_add(b) {
                    Some(r) => Value::int(r),
                    None => Value::bigint(PyBigInt::from(a) + PyBigInt::from(b)),
                }),
                (ValueKind::Int(a), ValueKind::Float(b)) => Ok(Value::float((a as f64) + b)),
                (ValueKind::Float(a), ValueKind::Int(b)) => Ok(Value::float(a + (b as f64))),
                (ValueKind::Float(a), ValueKind::Float(b)) => Ok(Value::float(a + b)),
                (ValueKind::Str(a), ValueKind::Str(b)) => Ok(Value::string(format!("{a}{b}"))),
                (ValueKind::List(a), ValueKind::List(b)) => {
                    let mut out = a.clone();
                    out.extend_from_slice(b);
                    Ok(Value::list(out))
                }
                (ValueKind::Tuple(a), ValueKind::Tuple(b)) => {
                    let mut out = a.clone();
                    out.extend_from_slice(b);
                    Ok(Value::tuple(out))
                }
                _ => Err(Self::unsupported_binary_operand("+")),
        }
    }

    fn sub(&self, left: Value, right: Value) -> Result<Value> {
        if let Some((a, b)) = both_as_complex(&left, &right) {
            return Ok(Value::complex(a.0 - b.0, a.1 - b.1));
        }
        let (l, r) = (coerce_numeric(left), coerce_numeric(right));
        match (l.kind(), r.kind()) {
            (ValueKind::Int(a), ValueKind::Int(b)) => Ok(match a.checked_sub(b) {
                Some(r) => Value::int(r),
                None => Value::bigint(PyBigInt::from(a) - PyBigInt::from(b)),
            }),
            (ValueKind::Int(a), ValueKind::Float(b)) => Ok(Value::float((a as f64) - b)),
            (ValueKind::Float(a), ValueKind::Int(b)) => Ok(Value::float(a - (b as f64))),
            (ValueKind::Float(a), ValueKind::Float(b)) => Ok(Value::float(a - b)),
            _ => Err(Self::unsupported_binary_operand("-")),
        }
    }

    fn mul(&self, left: Value, right: Value) -> Result<Value> {
        if let Some((a, b)) = both_as_complex(&left, &right) {
            // (ar+ai*j) * (br+bi*j) = (ar*br - ai*bi) + (ar*bi + ai*br)j
            return Ok(Value::complex(a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0));
        }
        let (l, r) = (coerce_numeric(left), coerce_numeric(right));
        match (l.kind(), r.kind()) {
            (ValueKind::Int(a), ValueKind::Int(b)) => Ok(match a.checked_mul(b) {
                Some(r) => Value::int(r),
                None => Value::bigint(PyBigInt::from(a) * PyBigInt::from(b)),
            }),
            (ValueKind::Int(a), ValueKind::Float(b)) => Ok(Value::float((a as f64) * b)),
            (ValueKind::Float(a), ValueKind::Int(b)) => Ok(Value::float(a * (b as f64))),
            (ValueKind::Float(a), ValueKind::Float(b)) => Ok(Value::float(a * b)),
            (ValueKind::Str(text), ValueKind::Int(n)) => {
                if n <= 0 { Ok(Value::string(String::new())) }
                else { Ok(Value::string(text.repeat(n as usize))) }
            }
            (ValueKind::Int(n), ValueKind::Str(text)) => {
                if n <= 0 { Ok(Value::string(String::new())) }
                else { Ok(Value::string(text.repeat(n as usize))) }
            }
            (ValueKind::List(items), ValueKind::Int(n)) => {
                if n <= 0 { return Ok(Value::list(Vec::new())); }
                let n = n as usize;
                let mut out = Vec::with_capacity(items.len() * n);
                for _ in 0..n { out.extend_from_slice(items); }
                Ok(Value::list(out))
            }
            (ValueKind::Int(n), ValueKind::List(items)) => {
                if n <= 0 { return Ok(Value::list(Vec::new())); }
                let n = n as usize;
                let mut out = Vec::with_capacity(items.len() * n);
                for _ in 0..n { out.extend_from_slice(items); }
                Ok(Value::list(out))
            }
            _ => Err(Self::unsupported_binary_operand("*")),
        }
    }

    fn try_call_binary_method(
        &mut self,
        receiver: &Value,
        method: &str,
        other: Value,
    ) -> Result<Option<Value>> {
        let inst = match receiver.kind() {
            ValueKind::PyInstance(i) => Rc::clone(i),
            _ => return Ok(None),
        };
        let class = Rc::clone(&inst.borrow().class);
        let Some(method_value) = lookup_class_attr(&class, method) else {
            return Ok(None);
        };

        let function = match method_value.kind() {
            ValueKind::UserFunction(f) => Rc::clone(f),
            _ => return Ok(None),
        };
        let result = self.call_user_function(
            function,
            &[],
            &[Value::py_instance(Rc::clone(&inst)), other],
        )?;
        Ok(Some(result))
    }

    pub(crate) fn try_inplace_op(
        &mut self,
        left: Value,
        op: BinaryOp,
        right: Value,
    ) -> Result<Option<Value>> {
        let dunder = match op {
            BinaryOp::Add => "__iadd__",
            BinaryOp::Sub => "__isub__",
            BinaryOp::Mul => "__imul__",
            BinaryOp::MatMul => "__imatmul__",
            BinaryOp::Div => "__itruediv__",
            BinaryOp::FloorDiv => "__ifloordiv__",
            BinaryOp::Mod => "__imod__",
            BinaryOp::Pow => "__ipow__",
            BinaryOp::BitAnd => "__iand__",
            BinaryOp::BitOr => "__ior__",
            BinaryOp::BitXor => "__ixor__",
            BinaryOp::LShift => "__ilshift__",
            BinaryOp::RShift => "__irshift__",
            _ => return Ok(None),
        };
        let result = self.try_call_binary_method(&left, dunder, right)?;
        if let Some(ref v) = result
            && is_not_implemented(v) {
                return Ok(None);
            }
        Ok(result)
    }

    fn matmul(&mut self, left: Value, right: Value) -> Result<Value> {
        if let Some(value) = self.try_call_binary_method(&left, "__matmul__", right.clone())? {
            return Ok(value);
        }
        if let Some(value) = self.try_call_binary_method(&right, "__rmatmul__", left.clone())? {
            return Ok(value);
        }
        Err(Self::unsupported_binary_operand("@"))
    }



    fn div(&self, left: Value, right: Value) -> Result<Value> {
        if let Some((a, b)) = both_as_complex(&left, &right) {
            // (ar+ai*j) / (br+bi*j) = ((ar*br + ai*bi) + (ai*br - ar*bi)j) / (br^2 + bi^2)
            let denom = b.0 * b.0 + b.1 * b.1;
            if denom == 0.0 {
                return Err(PyError::Named(
                    "ZeroDivisionError".to_string(),
                    "complex division by zero".to_string(),
                ));
            }
            return Ok(Value::complex(
                (a.0 * b.0 + a.1 * b.1) / denom,
                (a.1 * b.0 - a.0 * b.1) / denom,
            ));
        }
        let (a, b) = self.to_pair_number(left, right)?;
        if b == 0.0 {
            return Err(PyError::Runtime("division by zero".to_string()));
        }
        Ok(Value::float(a / b))
    }

    fn floor_div(&self, left: Value, right: Value) -> Result<Value> {
        match (left.kind(), right.kind()) {
            (ValueKind::Int(a), ValueKind::Int(b)) => {
                if b == 0 {
                    return Err(PyError::Runtime(
                        "integer division or modulo by zero".to_string(),
                    ));
                }
                let modulo = py_mod_i64(a, b);
                Ok(Value::int((a - modulo) / b))
            }
            _ => {
                let (a, b) = self.to_pair_number(left, right)?;
                if b == 0.0 {
                    return Err(PyError::Runtime("float floor division by zero".to_string()));
                }
                Ok(Value::float((a / b).floor()))
            }
        }
    }

    fn modulo(&self, left: Value, right: Value) -> Result<Value> {
        match (left.kind(), right.kind()) {
            (ValueKind::Int(a), ValueKind::Int(b)) => {
                if b == 0 {
                    return Err(PyError::Runtime(
                        "integer division or modulo by zero".to_string(),
                    ));
                }
                Ok(Value::int(py_mod_i64(a, b)))
            }
            _ => {
                let (a, b) = self.to_pair_number(left, right)?;
                if b == 0.0 {
                    return Err(PyError::Runtime("float modulo".to_string()));
                }
                Ok(Value::float(a - b * (a / b).floor()))
            }
        }
    }

    fn compare(&self, left: Value, right: Value, cmp: impl Fn(std::cmp::Ordering) -> bool) -> Result<Value> {
        if matches!(left.kind(), ValueKind::Float(f) if f.is_nan())
            || matches!(right.kind(), ValueKind::Float(f) if f.is_nan())
        {
            return Ok(Value::bool_(false));
        }
        Ok(Value::bool_(cmp(compare_values(&left, &right)?)))
    }

    fn to_pair_number(&self, left: Value, right: Value) -> Result<(f64, f64)> {
        Ok((self.to_number(&left)?, self.to_number(&right)?))
    }

    fn to_number(&self, value: &Value) -> Result<f64> {
        match value.kind() {
            ValueKind::Int(v) => Ok(v as f64),
            ValueKind::Float(v) => Ok(v),
            ValueKind::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
            _ => Err(PyError::Named("TypeError".to_string(), "expected number".to_string())),
        }
    }

    fn eval_slice(&mut self, target: Value, lo: Option<Value>, hi: Option<Value>, st: Option<Value>) -> Result<Value> {
        let len = match target.kind() {
            ValueKind::List(items) => items.len() as i64,
            ValueKind::Str(s) => s.chars().count() as i64,
            _ => return Err(PyError::Runtime("object is not sliceable".to_string())),
        };
        let (start, end, step) = Self::resolve_slice_bounds(len, lo.as_ref(), hi.as_ref(), st.as_ref())?;
        let indices = Self::slice_target_indices(len, start, end, step);

        match target.kind() {
            ValueKind::List(items) => Ok(Value::list(indices.into_iter().map(|ix| items[ix].clone()).collect::<Vec<Value>>())),
            ValueKind::Str(s) => {
                let chars: Vec<char> = s.chars().collect();
                let mut out = String::new();
                for ix in indices {
                    out.push(chars[ix]);
                }
                Ok(Value::string(out))
            }
            _ => unreachable!(),
        }
    }

    fn bitwise_op(&self, left: &Value, right: &Value, op: impl Fn(i64, i64) -> Result<i64>) -> Result<Value> {
        let a = match left.kind() {
            ValueKind::Int(v) => v,
            ValueKind::Bool(b) => if b { 1 } else { 0 },
            _ => return Err(PyError::Named("TypeError".to_string(), "bitwise op requires integer".to_string())),
        };
        let b = match right.kind() {
            ValueKind::Int(v) => v,
            ValueKind::Bool(b) => if b { 1 } else { 0 },
            _ => return Err(PyError::Named("TypeError".to_string(), "bitwise op requires integer".to_string())),
        };
        Ok(Value::int(op(a, b)?))
    }

    fn eval_binary_speculative(
        &mut self,
        site: &Expr,
        op: BinaryOp,
        left: Value,
        right: Value,
    ) -> Result<Value> {
        let site_id = site as *const Expr as usize;

        let tag_of = |v: &Value| match v.kind() {
            ValueKind::Int(_) => BinopTypeTag::Int,
            ValueKind::Float(_) => BinopTypeTag::Float,
            ValueKind::Str(_) => BinopTypeTag::Str,
            _ => BinopTypeTag::Other,
        };

        // Attempt the specialized fast path if this site is promoted.
        if let Some(SpecState::Specialized(spec_tag)) = self.spec_cache.get(&site_id) {
            let spec_tag = *spec_tag;
            if spec_tag == BinopTypeTag::Int {
                if let (ValueKind::Int(a), ValueKind::Int(b)) = (left.kind(), right.kind())
                    && let Some(result) = eval_binary_int(op, a, b) {
                        return result;
                    }
                // Type mismatch — deoptimize.
                self.spec_cache.insert(site_id, SpecState::Megamorphic);
            } else if spec_tag == BinopTypeTag::Float {
                if let (ValueKind::Float(a), ValueKind::Float(b)) = (left.kind(), right.kind())
                    && let Some(result) = eval_binary_float(op, a, b) {
                        return result;
                    }
                self.spec_cache.insert(site_id, SpecState::Megamorphic);
            }
        }

        // Generic path — also update speculation state.
        let observed_tag = {
            let lt = tag_of(&left);
            let rt = tag_of(&right);
            if lt == rt {
                lt
            } else {
                BinopTypeTag::Other
            }
        };

        // Only update if not already megamorphic and not yet specialized.
        if !matches!(
            self.spec_cache.get(&site_id),
            Some(SpecState::Megamorphic) | Some(SpecState::Specialized(_))
        ) {
            let next = match self.spec_cache.get(&site_id) {
                None => SpecState::Counting {
                    tag: observed_tag,
                    count: 1,
                },
                Some(SpecState::Counting { tag, count }) => {
                    if *tag == observed_tag {
                        let new_count = count + 1;
                        if new_count >= SPEC_THRESHOLD {
                            SpecState::Specialized(observed_tag)
                        } else {
                            SpecState::Counting {
                                tag: observed_tag,
                                count: new_count,
                            }
                        }
                    } else {
                        SpecState::Megamorphic
                    }
                }
                _ => unreachable!(),
            };
            self.spec_cache.insert(site_id, next);
        }

        self.eval_binary(left, op, right)
    }

    fn eval_in(&mut self, container: Value, item: Value) -> Result<Value> {
        match container.kind() {
            ValueKind::List(items) => Ok(Value::bool_(items.iter().any(|b| b == &item))),
            ValueKind::Tuple(items) => Ok(Value::bool_(items.contains(&item))),
            ValueKind::Set(items) => {
                let key = item.to_key().ok_or_else(|| PyError::Runtime("unhashable type".to_string()))?;
                Ok(Value::bool_(items.contains(&key)))
            }
            ValueKind::BuiltinObject { ops, state } => {
                ops.contains(state, &item).map(Value::bool_)
            }
            ValueKind::Bytes(rc) => {
                match item.kind() {
                    ValueKind::Int(n) if (0..=255).contains(&n) => Ok(Value::bool_(rc.contains(&(n as u8)))),
                    ValueKind::Bytes(sub) => Ok(Value::bool_(
                        sub.is_empty() || rc.windows(sub.len()).any(|w| w == sub.as_ref().as_slice())
                    )),
                    _ => Err(PyError::Named(
                        "TypeError".to_string(),
                        "a bytes-like object is required as left operand of 'in <bytes>'".to_string(),
                    )),
                }
            }
            ValueKind::Str(s) => {
                match item.kind() {
                    ValueKind::Str(sub) => Ok(Value::bool_(s.contains(sub))),
                    _ => Err(PyError::Runtime("'in <string>' requires string as left operand".to_string())),
                }
            }
            ValueKind::Dict(items) => {
                let key = item.to_key().ok_or_else(|| PyError::Runtime("unhashable type".to_string()))?;
                Ok(Value::bool_(items.contains_key(&key)))
            }
            ValueKind::Range { start, stop, step } => {
                match item.kind() {
                    ValueKind::Int(v) => {
                        let in_range = if step > 0 {
                            v >= start && v < stop && (v - start) % step == 0
                        } else if step < 0 {
                            v <= start && v > stop && (v - start) % step == 0
                        } else { false };
                        Ok(Value::bool_(in_range))
                    }
                    _ => Ok(Value::bool_(false)),
                }
            }
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__contains__") {
                    if let ValueKind::UserFunction(f) = method_val.kind() {
                        let func = Rc::clone(f);
                        let self_val = Value::py_instance(inst_rc);
                        let result = self.call_user_function_expanded(func, &[], &[self_val, item])?;
                        return Ok(Value::bool_(result.truthy()));
                    } else {
                        return Err(PyError::Named(
                            "TypeError".to_string(),
                            format!("argument of type '{}' is not iterable", class.borrow().name),
                        ));
                    }
                }
                // No __contains__: fall back to __iter__ if available.
                if let Some(iter_method) = lookup_class_attr(&class, "__iter__")
                    && let ValueKind::UserFunction(f) = iter_method.kind()
                {
                    let func = Rc::clone(f);
                    let iter_obj = self
                        .call_user_function_expanded(func, &[], &[Value::py_instance(Rc::clone(&inst_rc))])?;
                    loop {
                        match self.call_next(iter_obj.clone(), None) {
                            Ok(elem) => {
                                if elem == item {
                                    return Ok(Value::bool_(true));
                                }
                            }
                            Err(PyError::Named(ref cls, _)) if cls == "StopIteration" => {
                                return Ok(Value::bool_(false));
                            }
                            Err(e) => return Err(e),
                        }
                    }
                }
                Err(PyError::Named(
                    "TypeError".to_string(),
                    format!("argument of type '{}' is not iterable", class.borrow().name),
                ))
            }
            _ => Err(PyError::Runtime("argument of type is not iterable".to_string())),
        }
    }

}

fn is_not_implemented(v: &Value) -> bool {
    matches!(v.kind(), ValueKind::NotImplemented)
}

fn coerce_numeric(v: Value) -> Value {
    match v.kind() {
        ValueKind::Bool(b) => Value::int(b as i64),
        _ => v,
    }
}

fn iter_values(value: Value) -> Result<Vec<Value>> {
    match value.kind() {
        ValueKind::List(items) => Ok(items.clone()),
        ValueKind::Tuple(items) => Ok(items.clone()),
        ValueKind::Set(items) => Ok(items.iter().map(|k| key_to_value(k.clone())).collect()),
        ValueKind::BuiltinObject { .. } => {
            // Frozensets materialise through their inner key set; dict views
            // materialise through their backing IndexMap; everything else
            // iterates via `iter_next`.
            if let Some(rc) = pyrust_builtins::frozenset::as_items(&value) {
                return Ok(rc.iter().map(|k| key_to_value(k.clone())).collect());
            }
            if let Some(kind) = pyrust_builtins::dict_views::view_kind(&value) {
                let rc = pyrust_builtins::dict_views::as_dict_rc(&value).unwrap();
                let map = rc.borrow();
                return Ok(match kind {
                    0 => map.keys().map(|k| key_to_value(k.clone())).collect(),
                    1 => map.values().cloned().collect(),
                    _ => map
                        .iter()
                        .map(|(k, v)| Value::tuple(vec![key_to_value(k.clone()), v.clone()]))
                        .collect(),
                });
            }
            let mut out = Vec::new();
            let ValueKind::BuiltinObject { ops, state } = value.kind() else {
                unreachable!();
            };
            if !ops.is_iterable() {
                return Err(PyError::Named(
                    "TypeError".to_string(),
                    format!("'{}' object is not iterable", ops.type_name()),
                ));
            }
            while let Some(v) = ops.iter_next(state)? {
                out.push(v);
            }
            Ok(out)
        }
        ValueKind::Bytes(rc) => Ok(rc.iter().map(|b| Value::int(*b as i64)).collect()),
        ValueKind::Str(text) => Ok(text.chars().map(|c| Value::string(c.to_string())).collect()),
        ValueKind::Dict(items) => Ok(items.keys().map(|k| key_to_value(k.clone())).collect()),
        ValueKind::Range { start, stop, step } => {
            let mut out = Vec::new();
            if step > 0 {
                let mut cur = start;
                while cur < stop {
                    out.push(Value::int(cur));
                    cur += step;
                }
            } else {
                let mut cur = start;
                while cur > stop {
                    out.push(Value::int(cur));
                    cur += step;
                }
            }
            Ok(out)
        }
        ValueKind::Generator(state_rc) => {
            // Drain a NativeIterFrame (created by iter() on builtins) into a Vec.
            let mut borrow = state_rc.borrow_mut();
            if let Some(native) = borrow.downcast_mut::<NativeIterFrame>() {
                let remaining = native.items[native.pos..].to_vec();
                native.pos = native.items.len();
                Ok(remaining)
            } else {
                Err(PyError::Named(
                    "TypeError".to_string(),
                    "object is not iterable".to_string(),
                ))
            }
        }
        _ => Err(PyError::Named(
            "TypeError".to_string(),
            format!("'{}' object is not iterable", value_type_name_str(&value)),
        )),
    }
}

/// Resolve a built-in name to its `Value::builtin_function` variant.
/// Single source of truth for both the tree-walker (`eval_expr`) and the register VM
/// (`LoadGlobal` fallback).  Any new built-in must be added here only.
pub(crate) fn resolve_builtin(name: &str) -> Option<Value> {
    match name {
        "print" => Some(Value::builtin_function("print")),
        "len" => Some(Value::builtin_function("len")),
        "range" => Some(Value::builtin_function("range")),
        "enumerate" => Some(Value::builtin_function("enumerate")),
        "zip" => Some(Value::builtin_function("zip")),
        "reversed" => Some(Value::builtin_function("reversed")),
        "sorted" => Some(Value::builtin_function("sorted")),
        "abs" => Some(Value::builtin_function("abs")),
        "min" => Some(Value::builtin_function("min")),
        "max" => Some(Value::builtin_function("max")),
        "sum" => Some(Value::builtin_function("sum")),
        "list" => Some(Value::builtin_function("list")),
        "tuple" => Some(Value::builtin_function("tuple")),
        "set" => Some(Value::builtin_function("set")),
        "frozenset" => Some(Value::builtin_function("frozenset")),
        "bytes" => Some(Value::builtin_function("bytes")),
        "dict" => Some(Value::builtin_function("dict")),
        "str" => Some(Value::builtin_function("str")),
        "int" => Some(Value::builtin_function("int")),
        "float" => Some(Value::builtin_function("float")),
        "bool" => Some(Value::builtin_function("bool")),
        "isinstance" => Some(Value::builtin_function("isinstance")),
        "type" => Some(Value::builtin_function("type")),
        "id" => Some(Value::builtin_function("id")),
        "hasattr" => Some(Value::builtin_function("hasattr")),
        "getattr" => Some(Value::builtin_function("getattr")),
        "setattr" => Some(Value::builtin_function("setattr")),
        "__vcall__" => Some(Value::builtin_function("__vcall__")),
        "repr" => Some(Value::builtin_function("repr")),
        "ascii" => Some(Value::builtin_function("ascii")),
        "format" => Some(Value::builtin_function("format")),
        "any" => Some(Value::builtin_function("any")),
        "all" => Some(Value::builtin_function("all")),
        "map" => Some(Value::builtin_function("map")),
        "filter" => Some(Value::builtin_function("filter")),
        "callable" => Some(Value::builtin_function("callable")),
        "round" => Some(Value::builtin_function("round")),
        "divmod" => Some(Value::builtin_function("divmod")),
        "pow" => Some(Value::builtin_function("pow")),
        "hash" => Some(Value::builtin_function("hash")),
        "chr" => Some(Value::builtin_function("chr")),
        "ord" => Some(Value::builtin_function("ord")),
        "bin" => Some(Value::builtin_function("bin")),
        "oct" => Some(Value::builtin_function("oct")),
        "hex" => Some(Value::builtin_function("hex")),
        "issubclass" => Some(Value::builtin_function("issubclass")),
        "delattr" => Some(Value::builtin_function("delattr")),
        "classmethod" => Some(Value::builtin_function("classmethod")),
        "staticmethod" => Some(Value::builtin_function("staticmethod")),
        "property" => Some(Value::builtin_function("property")),
        "super" => Some(Value::builtin_function("super")),
        "next" => Some(Value::builtin_function("next")),
        "iter" => Some(Value::builtin_function("iter")),
        "vars" => Some(Value::builtin_function("vars")),
        "dir" => Some(Value::builtin_function("dir")),
        "open" => Some(Value::builtin_function("open")),
        "NotImplemented" => Some(Value::not_implemented()),
        "complex" => Some(Value::builtin_function("complex")),
        _ => None,
    }
}

/// Operation tag for set/frozenset binary operators.
#[derive(Clone, Copy)]
enum SetOp {
    Or,  // union
    And, // intersection
    Sub, // difference
    Xor, // symmetric difference
}

/// Compute a binary set operation when both operands are set/frozenset.
/// Returns the result wrapped in `Set` if both operands are sets, otherwise
/// `FrozenSet` (matching CPython: any frozenset operand promotes the result).
fn set_binary_op(left: &Value, right: &Value, op: SetOp) -> Option<Result<Value>> {
    let lhs_items = match left.kind() {
        ValueKind::Set(s) => Some((s.clone(), false)),
        _ => pyrust_builtins::frozenset::as_items(left).map(|rc| ((*rc).clone(), true)),
    }?;
    let rhs_items = match right.kind() {
        ValueKind::Set(s) => Some((s.clone(), false)),
        _ => pyrust_builtins::frozenset::as_items(right).map(|rc| ((*rc).clone(), true)),
    }?;
    let (a, l_frozen) = lhs_items;
    let (b, r_frozen) = rhs_items;
    let mut out = indexmap::IndexSet::new();
    match op {
        SetOp::Or => {
            for k in a.iter().chain(b.iter()) {
                out.insert(k.clone());
            }
        }
        SetOp::And => {
            for k in a.iter() {
                if b.contains(k) {
                    out.insert(k.clone());
                }
            }
        }
        SetOp::Sub => {
            for k in a.iter() {
                if !b.contains(k) {
                    out.insert(k.clone());
                }
            }
        }
        SetOp::Xor => {
            for k in a.iter() {
                if !b.contains(k) {
                    out.insert(k.clone());
                }
            }
            for k in b.iter() {
                if !a.contains(k) {
                    out.insert(k.clone());
                }
            }
        }
    }
    Some(Ok(if l_frozen || r_frozen {
        pyrust_builtins::frozenset::frozenset(out)
    } else {
        Value::set(out)
    }))
}

/// Coerce a numeric value to a `(real, imag)` pair if possible.
fn as_complex_pair(v: &Value) -> Option<(f64, f64)> {
    match v.kind() {
        ValueKind::Complex(re, im) => Some((re, im)),
        ValueKind::Int(n) => Some((n as f64, 0.0)),
        ValueKind::Float(f) => Some((f, 0.0)),
        ValueKind::Bool(b) => Some((if b { 1.0 } else { 0.0 }, 0.0)),
        _ => None,
    }
}

/// Returns the two operands as complex `(re, im)` pairs only when AT LEAST
/// one of them is already a complex number — that way pure int/float
/// arithmetic continues to use the dedicated fast paths.
fn both_as_complex(left: &Value, right: &Value) -> Option<((f64, f64), (f64, f64))> {
    let l_is_c = matches!(left.kind(), ValueKind::Complex(_, _));
    let r_is_c = matches!(right.kind(), ValueKind::Complex(_, _));
    if !l_is_c && !r_is_c {
        return None;
    }
    let a = as_complex_pair(left)?;
    let b = as_complex_pair(right)?;
    Some((a, b))
}
