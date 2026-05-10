impl Interpreter {
    fn unsupported_binary_operand(op: &str) -> PyError {
        PyError::Runtime(format!("unsupported operand types for {op}"))
    }

    fn eval_expr(&mut self, expr: &Expr) -> Result<Value> {
        match expr {
            Expr::Int(v) => Ok(Value::int(*v)),
            Expr::Float(v) => Ok(Value::float(*v)),
            Expr::Str(v) => Ok(Value::string(v.clone())),
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
                    UnaryOp::Neg => match value.kind() {
                        ValueKind::Int(v) => Ok(Value::int(-v)),
                        ValueKind::Float(v) => Ok(Value::float(-v)),
                        _ => Err(PyError::Runtime("bad operand type for unary -".to_string())),
                    },
                    UnaryOp::Not => Ok(Value::bool_(!value.truthy())),
                    UnaryOp::BitNot => match value.kind() {
                        ValueKind::Int(v) => Ok(Value::int(!v)),
                        ValueKind::Bool(b) => Ok(Value::int(if b { -2 } else { -1 })),
                        _ => Err(PyError::Runtime("bad operand type for unary ~: use integer".to_string())),
                    },
                    UnaryOp::Pos => match value.kind() {
                        ValueKind::Int(v) => Ok(Value::int(v)),
                        ValueKind::Float(v) => Ok(Value::float(v)),
                        ValueKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
                        _ => Err(PyError::Runtime("bad operand type for unary +".to_string())),
                    },
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
                if let Expr::Var(name) = target.as_ref() {
                    if let Some(env) = self.resolve_name_env(name) {
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
                                    let idx = normalize_index(&index_value, items.len())?;
                                    Ok(items[idx].clone())
                                }
                                ValueKind::Tuple(items) => {
                                    let idx = normalize_index(&index_value, items.len())?;
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
                    name: "<lambda>".to_string(),
                    params: params.iter().map(|n| crate::value::UserFunctionParam {
                        name: n.clone(), default: None, is_args: false, is_kwargs: false,
                    }).collect(),
                    is_pure: is_pure_body(&lambda_body),
                    local_names: Rc::new(local_names),
                    local_index,
                    global_names: Rc::new(std::collections::HashSet::new()),
                    nonlocal_names: Rc::new(std::collections::HashSet::new()),
                    env: closure,
                    precompiled_code: None,
                });
                Ok(Value::user_function(func))
            }
        }
    }

    fn eval_index(&self, target: Value, index: Value) -> Result<Value> {
        match target.kind() {
            ValueKind::List(items) => {
                let idx = normalize_index(&index, items.len())?;
                Ok(items[idx].clone())
            }
            ValueKind::Tuple(items) => {
                let idx = normalize_index(&index, items.len())?;
                Ok(items[idx].clone())
            }
            ValueKind::Str(text) => {
                let chars: Vec<char> = text.chars().collect();
                let idx = normalize_index(&index, chars.len())?;
                Ok(Value::string(chars[idx].to_string()))
            }
            ValueKind::Dict(items) => {
                let key = index
                    .to_key()
                    .ok_or_else(|| PyError::Runtime("unhashable key type".to_string()))?;
                items.get(&key).cloned().ok_or_else(|| PyError::Runtime("key error".to_string()))
            }
            _ => Err(PyError::Runtime("object is not subscriptable".to_string())),
        }
    }

    fn eval_binary(&mut self, left: Value, op: BinaryOp, right: Value) -> Result<Value> {
        match op {
            BinaryOp::Add => self.add(left, right),
            BinaryOp::Sub => self.num_op(left, right, |a, b| a - b, |a, b| a - b),
            BinaryOp::Mul => self.mul(left, right),
            BinaryOp::MatMul => self.matmul(left, right),
            BinaryOp::Div => self.div(left, right),
            BinaryOp::FloorDiv => self.floor_div(left, right),
            BinaryOp::Mod => self.modulo(left, right),
            BinaryOp::Eq => Ok(Value::bool_(left == right)),
            BinaryOp::Ne => Ok(Value::bool_(left != right)),
            BinaryOp::Lt => self.compare(left, right, |o| o.is_lt()),
            BinaryOp::Le => self.compare(left, right, |o| o.is_le()),
            BinaryOp::Gt => self.compare(left, right, |o| o.is_gt()),
            BinaryOp::Ge => self.compare(left, right, |o| o.is_ge()),
            BinaryOp::Pow => {
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
            BinaryOp::BitAnd => self.bitwise_op(&left, &right, |a, b| Ok(a & b)),
            BinaryOp::BitOr  => self.bitwise_op(&left, &right, |a, b| Ok(a | b)),
            BinaryOp::BitXor => self.bitwise_op(&left, &right, |a, b| Ok(a ^ b)),
            BinaryOp::LShift => self.bitwise_op(&left, &right, |a, b| {
                if b < 0 { return Err(PyError::Named("ValueError".to_string(), "negative shift count".to_string())); }
                Ok(a << (b & 63))
            }),
            BinaryOp::RShift => self.bitwise_op(&left, &right, |a, b| {
                if b < 0 { return Err(PyError::Named("ValueError".to_string(), "negative shift count".to_string())); }
                Ok(a >> (b & 63))
            }),
            BinaryOp::In => self.eval_in(right, left),
            BinaryOp::NotIn => Ok(Value::bool_(!self.eval_in(right, left)?.truthy())),
            BinaryOp::Is    => Ok(Value::bool_(values_are_identical(&left, &right))),
            BinaryOp::IsNot => Ok(Value::bool_(!values_are_identical(&left, &right))),
            BinaryOp::And | BinaryOp::Or => unreachable!("short-circuit handled earlier"),
        }
    }

    fn add(&self, left: Value, right: Value) -> Result<Value> {
        match (coerce_numeric(left), coerce_numeric(right)) {
            (l, r) => match (l.kind(), r.kind()) {
                (ValueKind::Int(a), ValueKind::Int(b)) => Ok(Value::int(a + b)),
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
    }

    fn mul(&self, left: Value, right: Value) -> Result<Value> {
        match (coerce_numeric(left), coerce_numeric(right)) {
            (l, r) => match (l.kind(), r.kind()) {
                (ValueKind::Int(a), ValueKind::Int(b)) => Ok(Value::int(a * b)),
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

    fn try_inplace_matmul(&mut self, left: Value, right: Value) -> Result<Option<Value>> {
        self.try_call_binary_method(&left, "__imatmul__", right)
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

    fn num_op(
        &self,
        left: Value,
        right: Value,
        int_op: impl Fn(i64, i64) -> i64,
        float_op: impl Fn(f64, f64) -> f64,
    ) -> Result<Value> {
        match (coerce_numeric(left), coerce_numeric(right)) {
            (l, r) => match (l.kind(), r.kind()) {
                (ValueKind::Int(a), ValueKind::Int(b)) => Ok(Value::int(int_op(a, b))),
                (ValueKind::Int(a), ValueKind::Float(b)) => Ok(Value::float(float_op(a as f64, b))),
                (ValueKind::Float(a), ValueKind::Int(b)) => Ok(Value::float(float_op(a, b as f64))),
                (ValueKind::Float(a), ValueKind::Float(b)) => Ok(Value::float(float_op(a, b))),
                _ => Err(PyError::Runtime(
                    "unsupported operand types for numeric operation".to_string(),
                )),
            }
        }
    }

    fn div(&self, left: Value, right: Value) -> Result<Value> {
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
        Ok(Value::bool_(cmp(py_ordering(&left, &right)?)))
    }

    fn to_pair_number(&self, left: Value, right: Value) -> Result<(f64, f64)> {
        Ok((self.to_number(&left)?, self.to_number(&right)?))
    }

    fn to_number(&self, value: &Value) -> Result<f64> {
        match value.kind() {
            ValueKind::Int(v) => Ok(v as f64),
            ValueKind::Float(v) => Ok(v),
            ValueKind::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
            _ => Err(PyError::Runtime("expected number".to_string())),
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
            _ => return Err(PyError::Runtime("bitwise op requires integer".to_string())),
        };
        let b = match right.kind() {
            ValueKind::Int(v) => v,
            ValueKind::Bool(b) => if b { 1 } else { 0 },
            _ => return Err(PyError::Runtime("bitwise op requires integer".to_string())),
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
                if let (ValueKind::Int(a), ValueKind::Int(b)) = (left.kind(), right.kind()) {
                    if let Some(result) = eval_binary_int(op, a, b) {
                        return result;
                    }
                }
                // Type mismatch — deoptimize.
                self.spec_cache.insert(site_id, SpecState::Megamorphic);
            } else if spec_tag == BinopTypeTag::Float {
                if let (ValueKind::Float(a), ValueKind::Float(b)) = (left.kind(), right.kind()) {
                    if let Some(result) = eval_binary_float(op, a, b) {
                        return result;
                    }
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
            ValueKind::DictKeysView(rc) => {
                let key = item.to_key().ok_or_else(|| PyError::Runtime("unhashable type".to_string()))?;
                Ok(Value::bool_(rc.borrow().contains_key(&key)))
            }
            ValueKind::DictValuesView(rc) => {
                Ok(Value::bool_(rc.borrow().values().any(|v| v == &item)))
            }
            ValueKind::DictItemsView(rc) => {
                match item.kind() {
                    ValueKind::Tuple(kv) if kv.len() == 2 => {
                        let key = kv[0].to_key().ok_or_else(|| PyError::Runtime("unhashable type".to_string()))?;
                        let map = rc.borrow();
                        Ok(Value::bool_(map.get(&key).map_or(false, |v| v == &kv[1])))
                    }
                    _ => Ok(Value::bool_(false)),
                }
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
            _ => Err(PyError::Runtime("argument of type is not iterable".to_string())),
        }
    }

}

fn coerce_numeric(v: Value) -> Value {
    match v.kind() {
        ValueKind::Bool(b) => Value::int(b as i64),
        _ => v,
    }
}

fn py_ordering(left: &Value, right: &Value) -> Result<std::cmp::Ordering> {
    use crate::value::{PyBigInt, PyToPrimitive};
    match (left.kind(), right.kind()) {
        (ValueKind::Int(a), ValueKind::Int(b)) => Ok(a.cmp(&b)),
        (ValueKind::Bool(a), ValueKind::Bool(b)) => Ok(a.cmp(&b)),
        (ValueKind::Bool(a), ValueKind::Int(b)) => Ok((a as i64).cmp(&b)),
        (ValueKind::Int(a), ValueKind::Bool(b)) => Ok(a.cmp(&(b as i64))),
        (ValueKind::Float(a), ValueKind::Float(b)) => {
            Ok(a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal))
        }
        (ValueKind::Int(a), ValueKind::Float(b)) => {
            Ok((a as f64).partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal))
        }
        (ValueKind::Float(a), ValueKind::Int(b)) => {
            Ok(a.partial_cmp(&(b as f64)).unwrap_or(std::cmp::Ordering::Equal))
        }
        (ValueKind::BigInt(a), ValueKind::BigInt(b)) => Ok(a.cmp(b)),
        (ValueKind::BigInt(a), ValueKind::Int(b)) => Ok((*a).cmp(&PyBigInt::from(b))),
        (ValueKind::Int(a), ValueKind::BigInt(b)) => Ok(PyBigInt::from(a).cmp(b)),
        (ValueKind::BigInt(a), ValueKind::Float(b)) => Ok(a
            .to_f64()
            .and_then(|af| af.partial_cmp(&b))
            .unwrap_or(std::cmp::Ordering::Equal)),
        (ValueKind::Float(a), ValueKind::BigInt(b)) => Ok(b
            .to_f64()
            .and_then(|bf| a.partial_cmp(&bf))
            .map(|o| o.reverse())
            .unwrap_or(std::cmp::Ordering::Equal)),
        (ValueKind::Str(a), ValueKind::Str(b)) => Ok(a.cmp(b)),
        (ValueKind::List(a), ValueKind::List(b)) => {
            for (x, y) in a.iter().zip(b.iter()) {
                let ord = py_ordering(x, y)?;
                if ord != std::cmp::Ordering::Equal {
                    return Ok(ord);
                }
            }
            Ok(a.len().cmp(&b.len()))
        }
        (ValueKind::Tuple(a), ValueKind::Tuple(b)) => {
            for (x, y) in a.iter().zip(b.iter()) {
                let ord = py_ordering(x, y)?;
                if ord != std::cmp::Ordering::Equal {
                    return Ok(ord);
                }
            }
            Ok(a.len().cmp(&b.len()))
        }
        _ => Err(PyError::Runtime(
            "'<' not supported between instances of these types".to_string(),
        )),
    }
}

fn iter_values(value: Value) -> Result<Vec<Value>> {
    match value.kind() {
        ValueKind::List(items) => Ok(items.clone()),
        ValueKind::Tuple(items) => Ok(items.clone()),
        ValueKind::Set(items) => Ok(items.iter().map(|k| key_to_value(k.clone())).collect()),
        ValueKind::Str(text) => Ok(text.chars().map(|c| Value::string(c.to_string())).collect()),
        ValueKind::Dict(items) => Ok(items.keys().map(|k| key_to_value(k.clone())).collect()),
        ValueKind::DictKeysView(rc) => {
            let map = rc.borrow();
            Ok(map.keys().map(|k| key_to_value(k.clone())).collect())
        }
        ValueKind::DictValuesView(rc) => {
            let map = rc.borrow();
            Ok(map.values().cloned().collect())
        }
        ValueKind::DictItemsView(rc) => {
            let map = rc.borrow();
            Ok(map.iter().map(|(k, v)| Value::tuple(vec![key_to_value(k.clone()), v.clone()])).collect())
        }
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
        _ => Err(PyError::Runtime("object is not iterable".to_string())),
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
        _ => None,
    }
}
