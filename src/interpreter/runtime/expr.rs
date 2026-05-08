impl Interpreter {
    fn unsupported_binary_operand(op: &str) -> PyError {
        PyError::Runtime(format!("unsupported operand types for {op}"))
    }

    fn eval_expr(&mut self, expr: &Expr) -> Result<Value> {
        match expr {
            Expr::Int(v) => Ok(Value::Int(*v)),
            Expr::Float(v) => Ok(Value::Float(*v)),
            Expr::Str(v) => Ok(Value::Str(v.clone())),
            Expr::Bool(v) => Ok(Value::Bool(*v)),
            Expr::None => Ok(Value::None),
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
                Ok(Value::List(out))
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
                Ok(Value::Dict(out))
            }
            Expr::Unary { op, expr } => {
                let value = self.eval_expr(expr)?;
                match op {
                    UnaryOp::Neg => match value {
                        Value::Int(v) => Ok(Value::Int(-v)),
                        Value::Float(v) => Ok(Value::Float(-v)),
                        _ => Err(PyError::Runtime("bad operand type for unary -".to_string())),
                    },
                    UnaryOp::Not => Ok(Value::Bool(!value.truthy())),
                    UnaryOp::BitNot => match value {
                        Value::Int(v) => Ok(Value::Int(!v)),
                        Value::Bool(b) => Ok(Value::Int(if b { -2 } else { -1 })),
                        _ => Err(PyError::Runtime("bad operand type for unary ~: use integer".to_string())),
                    },
                    UnaryOp::Pos => match value {
                        Value::Int(v) => Ok(Value::Int(v)),
                        Value::Float(v) => Ok(Value::Float(v)),
                        Value::Bool(b) => Ok(Value::Int(if b { 1 } else { 0 })),
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
                            col.map(|col| match col {
                                Value::List(items) | Value::Tuple(items) => {
                                    let idx = normalize_index(&index_value, items.len())?;
                                    Ok(items[idx].clone())
                                }
                                Value::Dict(items) => {
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
                Ok(Value::Tuple(out))
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
                Ok(Value::Set(out))
            }
            Expr::Compare { left, ops } => {
                let mut prev = self.eval_expr(left)?;
                for (op, right_expr) in ops {
                    let right = self.eval_expr(right_expr)?;
                    let result = self.eval_binary(prev.clone(), cmp_op_to_binary_op(*op), right.clone())?;
                    if !result.truthy() { return Ok(Value::Bool(false)); }
                    prev = right;
                }
                Ok(Value::Bool(true))
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
                    local_names.iter().enumerate().map(|(i, n)| (n.clone(), i)).collect::<HashMap<String, usize>>()
                );
                let lambda_body = vec![crate::ast::Stmt::Return(Some(*body.clone()))];
                let params_ufp: Vec<crate::ast::FunctionParam> = params
                    .iter()
                    .map(|n| crate::ast::FunctionParam {
                        name: n.clone(),
                        default: None,
                        is_args: false,
                        is_kwargs: false,
                    })
                    .collect();
                let def_bound_mask =
                    compute_def_bound_mask(&params_ufp, &local_index);
                let func = Rc::new(crate::value::UserFunction {
                    name: "<lambda>".to_string(),
                    params: params.iter().map(|n| crate::value::UserFunctionParam {
                        name: n.clone(), default: None, is_args: false, is_kwargs: false,
                    }).collect(),
                    is_pure: is_pure_body(&lambda_body),
                    body: lambda_body,
                    local_names: Rc::new(local_names),
                    local_index,
                    global_names: Rc::new(std::collections::HashSet::new()),
                    nonlocal_names: Rc::new(std::collections::HashSet::new()),
                    env: closure,
                    def_bound_mask,
                    precompiled_code: None,
                });
                Ok(Value::Function(func))
            }
        }
    }

    fn eval_index(&self, target: Value, index: Value) -> Result<Value> {
        match target {
            Value::List(items) | Value::Tuple(items) => {
                let idx = normalize_index(&index, items.len())?;
                Ok(items[idx].clone())
            }
            Value::Str(text) => {
                let chars: Vec<char> = text.chars().collect();
                let idx = normalize_index(&index, chars.len())?;
                Ok(Value::Str(chars[idx].to_string()))
            }
            Value::Dict(items) => {
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
            BinaryOp::Eq => Ok(Value::Bool(left == right)),
            BinaryOp::Ne => Ok(Value::Bool(left != right)),
            BinaryOp::Lt => self.compare(left, right, |o| o.is_lt()),
            BinaryOp::Le => self.compare(left, right, |o| o.is_le()),
            BinaryOp::Gt => self.compare(left, right, |o| o.is_gt()),
            BinaryOp::Ge => self.compare(left, right, |o| o.is_ge()),
            BinaryOp::Pow => {
                match (&left, &right) {
                    (Value::Int(a), Value::Int(b)) if *b >= 0 => {
                        Ok(Value::Int(a.wrapping_pow(*b as u32)))
                    }
                    _ => {
                        let a = value_to_float(&left, "**")?;
                        let b = value_to_float(&right, "**")?;
                        Ok(Value::Float(a.powf(b)))
                    }
                }
            }
            BinaryOp::BitAnd => self.bitwise_op(&left, &right, |a, b| a & b),
            BinaryOp::BitOr  => self.bitwise_op(&left, &right, |a, b| a | b),
            BinaryOp::BitXor => self.bitwise_op(&left, &right, |a, b| a ^ b),
            BinaryOp::LShift => self.bitwise_op(&left, &right, |a, b| a << (b & 63)),
            BinaryOp::RShift => self.bitwise_op(&left, &right, |a, b| a >> (b & 63)),
            BinaryOp::In => self.eval_in(right, left),
            BinaryOp::NotIn => Ok(Value::Bool(!self.eval_in(right, left)?.truthy())),
            BinaryOp::Is    => Ok(Value::Bool(values_are_identical(&left, &right))),
            BinaryOp::IsNot => Ok(Value::Bool(!values_are_identical(&left, &right))),
            BinaryOp::And | BinaryOp::Or => unreachable!("short-circuit handled earlier"),
        }
    }

    fn add(&self, left: Value, right: Value) -> Result<Value> {
        match (coerce_numeric(left), coerce_numeric(right)) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float((a as f64) + b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + (b as f64))),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
            (Value::Str(a), Value::Str(b)) => Ok(Value::Str(format!("{a}{b}"))),
            (Value::List(mut a), Value::List(b)) => {
                a.extend(b);
                Ok(Value::List(a))
            }
            _ => Err(Self::unsupported_binary_operand("+")),
        }
    }

    fn mul(&self, left: Value, right: Value) -> Result<Value> {
        match (coerce_numeric(left), coerce_numeric(right)) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a * b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float((a as f64) * b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a * (b as f64))),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
            (Value::Str(text), Value::Int(n)) | (Value::Int(n), Value::Str(text)) => {
                if n <= 0 {
                    Ok(Value::Str(String::new()))
                } else {
                    Ok(Value::Str(text.repeat(n as usize)))
                }
            }
            (Value::List(items), Value::Int(n)) | (Value::Int(n), Value::List(items)) => {
                if n <= 0 {
                    return Ok(Value::List(Vec::new()));
                }
                let n = n as usize;
                let mut out = Vec::with_capacity(items.len() * n);
                for _ in 0..n {
                    out.extend_from_slice(&items);
                }
                Ok(Value::List(out))
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
        let Value::Instance(inst) = receiver else {
            return Ok(None);
        };
        let class = Rc::clone(&inst.borrow().class);
        let Some(method_value) = lookup_class_attr(&class, method) else {
            return Ok(None);
        };

        let Value::Function(function) = method_value else {
            return Ok(None);
        };
        let result = self.call_user_function(
            function,
            &[],
            &[Value::Instance(Rc::clone(inst)), other],
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
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(int_op(a, b))),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(float_op(a as f64, b))),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(float_op(a, b as f64))),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(float_op(a, b))),
            _ => Err(PyError::Runtime(
                "unsupported operand types for numeric operation".to_string(),
            )),
        }
    }

    fn div(&self, left: Value, right: Value) -> Result<Value> {
        let (a, b) = self.to_pair_number(left, right)?;
        if b == 0.0 {
            return Err(PyError::Runtime("division by zero".to_string()));
        }
        Ok(Value::Float(a / b))
    }

    fn floor_div(&self, left: Value, right: Value) -> Result<Value> {
        match (&left, &right) {
            (Value::Int(a), Value::Int(b)) => {
                if *b == 0 {
                    return Err(PyError::Runtime(
                        "integer division or modulo by zero".to_string(),
                    ));
                }
                let modulo = py_mod_i64(*a, *b);
                Ok(Value::Int((*a - modulo) / *b))
            }
            _ => {
                let (a, b) = self.to_pair_number(left, right)?;
                if b == 0.0 {
                    return Err(PyError::Runtime("float floor division by zero".to_string()));
                }
                Ok(Value::Float((a / b).floor()))
            }
        }
    }

    fn modulo(&self, left: Value, right: Value) -> Result<Value> {
        match (&left, &right) {
            (Value::Int(a), Value::Int(b)) => {
                if *b == 0 {
                    return Err(PyError::Runtime(
                        "integer division or modulo by zero".to_string(),
                    ));
                }
                Ok(Value::Int(py_mod_i64(*a, *b)))
            }
            _ => {
                let (a, b) = self.to_pair_number(left, right)?;
                if b == 0.0 {
                    return Err(PyError::Runtime("float modulo".to_string()));
                }
                Ok(Value::Float(a - b * (a / b).floor()))
            }
        }
    }

    fn compare(&self, left: Value, right: Value, cmp: impl Fn(std::cmp::Ordering) -> bool) -> Result<Value> {
        Ok(Value::Bool(cmp(py_ordering(&left, &right)?)))
    }

    fn to_pair_number(&self, left: Value, right: Value) -> Result<(f64, f64)> {
        Ok((self.to_number(&left)?, self.to_number(&right)?))
    }

    fn to_number(&self, value: &Value) -> Result<f64> {
        match value {
            Value::Int(v) => Ok(*v as f64),
            Value::Float(v) => Ok(*v),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            _ => Err(PyError::Runtime("expected number".to_string())),
        }
    }

    fn eval_slice(&mut self, target: Value, lo: Option<Value>, hi: Option<Value>, st: Option<Value>) -> Result<Value> {
        let len = match &target {
            Value::List(items) => items.len() as i64,
            Value::Str(s) => s.chars().count() as i64,
            _ => return Err(PyError::Runtime("object is not sliceable".to_string())),
        };
        let (start, end, step) = Self::resolve_slice_bounds(len, lo.as_ref(), hi.as_ref(), st.as_ref())?;
        let indices = Self::slice_target_indices(len, start, end, step);

        match target {
            Value::List(items) => Ok(Value::List(indices.into_iter().map(|ix| items[ix].clone()).collect())),
            Value::Str(s) => {
                let chars: Vec<char> = s.chars().collect();
                let mut out = String::new();
                for ix in indices {
                    out.push(chars[ix]);
                }
                Ok(Value::Str(out))
            }
            _ => unreachable!(),
        }
    }

    fn bitwise_op(&self, left: &Value, right: &Value, op: impl Fn(i64, i64) -> i64) -> Result<Value> {
        let a = match left {
            Value::Int(v) => *v,
            Value::Bool(b) => if *b { 1 } else { 0 },
            _ => return Err(PyError::Runtime("bitwise op requires integer".to_string())),
        };
        let b = match right {
            Value::Int(v) => *v,
            Value::Bool(b) => if *b { 1 } else { 0 },
            _ => return Err(PyError::Runtime("bitwise op requires integer".to_string())),
        };
        Ok(Value::Int(op(a, b)))
    }

    fn eval_binary_speculative(
        &mut self,
        site: &Expr,
        op: BinaryOp,
        left: Value,
        right: Value,
    ) -> Result<Value> {
        let site_id = site as *const Expr as usize;

        let tag_of = |v: &Value| match v {
            Value::Int(_) => BinopTypeTag::Int,
            Value::Float(_) => BinopTypeTag::Float,
            Value::Str(_) => BinopTypeTag::Str,
            _ => BinopTypeTag::Other,
        };

        // Attempt the specialized fast path if this site is promoted.
        if let Some(SpecState::Specialized(spec_tag)) = self.spec_cache.get(&site_id) {
            let spec_tag = *spec_tag;
            if spec_tag == BinopTypeTag::Int {
                if let (Value::Int(a), Value::Int(b)) = (&left, &right) {
                    if let Some(result) = eval_binary_int(op, *a, *b) {
                        return result;
                    }
                }
                // Type mismatch — deoptimize.
                self.spec_cache.insert(site_id, SpecState::Megamorphic);
            } else if spec_tag == BinopTypeTag::Float {
                if let (Value::Float(a), Value::Float(b)) = (&left, &right) {
                    if let Some(result) = eval_binary_float(op, *a, *b) {
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
        match container {
            Value::List(items) => Ok(Value::Bool(items.contains(&item))),
            Value::Tuple(items) => Ok(Value::Bool(items.contains(&item))),
            Value::Set(items) => {
                let key = item.to_key().ok_or_else(|| PyError::Runtime("unhashable type".to_string()))?;
                Ok(Value::Bool(items.contains(&key)))
            }
            Value::Str(s) => {
                if let Value::Str(sub) = &item {
                    Ok(Value::Bool(s.contains(sub.as_str())))
                } else {
                    Err(PyError::Runtime("'in <string>' requires string as left operand".to_string()))
                }
            }
            Value::Dict(items) => {
                let key = item.to_key().ok_or_else(|| PyError::Runtime("unhashable type".to_string()))?;
                Ok(Value::Bool(items.contains_key(&key)))
            }
            Value::Range { start, stop, step } => {
                if let Value::Int(v) = item {
                    let in_range = if step > 0 {
                        v >= start && v < stop && (v - start) % step == 0
                    } else if step < 0 {
                        v <= start && v > stop && (v - start) % step == 0
                    } else { false };
                    Ok(Value::Bool(in_range))
                } else {
                    Ok(Value::Bool(false))
                }
            }
            _ => Err(PyError::Runtime("argument of type is not iterable".to_string())),
        }
    }

}

fn coerce_numeric(v: Value) -> Value {
    match v {
        Value::Bool(b) => Value::Int(b as i64),
        other => other,
    }
}

fn py_ordering(left: &Value, right: &Value) -> Result<std::cmp::Ordering> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => Ok(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => Ok(a.cmp(b)),
        (Value::Bool(a), Value::Int(b)) => Ok((*a as i64).cmp(b)),
        (Value::Int(a), Value::Bool(b)) => Ok(a.cmp(&(*b as i64))),
        (Value::Float(a), Value::Float(b)) => {
            Ok(a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::Int(a), Value::Float(b)) => {
            Ok((*a as f64).partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::Float(a), Value::Int(b)) => {
            Ok(a.partial_cmp(&(*b as f64)).unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::Str(a), Value::Str(b)) => Ok(a.cmp(b)),
        (Value::List(a), Value::List(b)) | (Value::Tuple(a), Value::Tuple(b)) => {
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
    match value {
        Value::List(items) | Value::Tuple(items) => Ok(items),
        Value::Set(items) => Ok(items.into_iter().map(key_to_value).collect()),
        Value::Str(text) => Ok(text.chars().map(|c| Value::Str(c.to_string())).collect()),
        Value::Dict(items) => Ok(items.into_iter().map(|(key, _)| key_to_value(key)).collect()),
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
        _ => Err(PyError::Runtime("object is not iterable".to_string())),
    }
}

/// Resolve a built-in name to its `Value::Builtin` variant.
/// Single source of truth for both the tree-walker (`eval_expr`) and the register VM
/// (`LoadGlobal` fallback).  Any new built-in must be added here only.
pub(crate) fn resolve_builtin(name: &str) -> Option<Value> {
    match name {
        "print" => Some(Value::Builtin("print")),
        "len" => Some(Value::Builtin("len")),
        "range" => Some(Value::Builtin("range")),
        "enumerate" => Some(Value::Builtin("enumerate")),
        "zip" => Some(Value::Builtin("zip")),
        "reversed" => Some(Value::Builtin("reversed")),
        "sorted" => Some(Value::Builtin("sorted")),
        "abs" => Some(Value::Builtin("abs")),
        "min" => Some(Value::Builtin("min")),
        "max" => Some(Value::Builtin("max")),
        "sum" => Some(Value::Builtin("sum")),
        "list" => Some(Value::Builtin("list")),
        "tuple" => Some(Value::Builtin("tuple")),
        "str" => Some(Value::Builtin("str")),
        "int" => Some(Value::Builtin("int")),
        "float" => Some(Value::Builtin("float")),
        "bool" => Some(Value::Builtin("bool")),
        "__vcall__" => Some(Value::Builtin("__vcall__")),
        _ => None,
    }
}
