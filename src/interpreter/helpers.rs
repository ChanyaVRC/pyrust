#[derive(Clone, Copy, PartialEq)]
pub(crate) enum BinopTypeTag {
    Int,
    Float,
    Str,
    Other,
}

#[derive(Clone)]
pub(crate) enum SpecState {
    /// Seen `count` examples of the same type tag.
    Counting { tag: BinopTypeTag, count: u8 },
    /// Promoted to a specialized path after SPEC_THRESHOLD observations.
    Specialized(BinopTypeTag),
    /// Seen mixed types — no further specialization.
    Megamorphic,
}

/// Attempt a pure-integer binary operation. Returns Some(Result<Value>) on success,
/// None if the operation is not applicable to integers (e.g. Str concat).
fn eval_binary_int(op: BinaryOp, a: i64, b: i64) -> Option<Result<Value>> {
    match op {
        BinaryOp::Add => Some(Ok(Value::int(a.wrapping_add(b)))),
        BinaryOp::Sub => Some(Ok(Value::int(a.wrapping_sub(b)))),
        BinaryOp::Mul => Some(Ok(Value::int(a.wrapping_mul(b)))),
        BinaryOp::Div => {
            if b == 0 {
                Some(Err(PyError::Runtime("division by zero".to_string())))
            } else {
                Some(Ok(Value::float(a as f64 / b as f64)))
            }
        }
        BinaryOp::FloorDiv => {
            if b == 0 {
                Some(Err(PyError::Runtime(
                    "integer division or modulo by zero".to_string(),
                )))
            } else {
                let modulo = py_mod_i64(a, b);
                Some(Ok(Value::int((a - modulo) / b)))
            }
        }
        BinaryOp::Mod => {
            if b == 0 {
                Some(Err(PyError::Runtime(
                    "integer division or modulo by zero".to_string(),
                )))
            } else {
                Some(Ok(Value::int(py_mod_i64(a, b))))
            }
        }
        BinaryOp::Pow => Some(Ok(if b >= 0 {
            Value::int(a.wrapping_pow(b as u32))
        } else {
            Value::float((a as f64).powi(b as i32))
        })),
        BinaryOp::Eq => Some(Ok(Value::bool_(a == b))),
        BinaryOp::Ne => Some(Ok(Value::bool_(a != b))),
        BinaryOp::Lt => Some(Ok(Value::bool_(a < b))),
        BinaryOp::Le => Some(Ok(Value::bool_(a <= b))),
        BinaryOp::Gt => Some(Ok(Value::bool_(a > b))),
        BinaryOp::Ge => Some(Ok(Value::bool_(a >= b))),
        BinaryOp::BitAnd => Some(Ok(Value::int(a & b))),
        BinaryOp::BitOr => Some(Ok(Value::int(a | b))),
        BinaryOp::BitXor => Some(Ok(Value::int(a ^ b))),
        BinaryOp::LShift => Some(Ok(Value::int(a << (b & 63)))),
        BinaryOp::RShift => Some(Ok(Value::int(a >> (b & 63)))),
        _ => None, // And/Or handled separately; In/NotIn/Is/IsNot/MatMul not applicable
    }
}

/// Attempt a pure-float binary operation.
fn eval_binary_float(op: BinaryOp, a: f64, b: f64) -> Option<Result<Value>> {
    match op {
        BinaryOp::Add => Some(Ok(Value::float(a + b))),
        BinaryOp::Sub => Some(Ok(Value::float(a - b))),
        BinaryOp::Mul => Some(Ok(Value::float(a * b))),
        BinaryOp::Div => {
            if b == 0.0 {
                Some(Err(PyError::Runtime(
                    "float division by zero".to_string(),
                )))
            } else {
                Some(Ok(Value::float(a / b)))
            }
        }
        BinaryOp::Eq => Some(Ok(Value::bool_(a == b))),
        BinaryOp::Ne => Some(Ok(Value::bool_(a != b))),
        BinaryOp::Lt => Some(Ok(Value::bool_(a < b))),
        BinaryOp::Le => Some(Ok(Value::bool_(a <= b))),
        BinaryOp::Gt => Some(Ok(Value::bool_(a > b))),
        BinaryOp::Ge => Some(Ok(Value::bool_(a >= b))),
        _ => None,
    }
}

/// Returns the Python type name string for a `Value`, used in error messages.
fn value_type_name_str(v: &Value) -> &'static str {
    match v.kind() {
        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => "int",
        ValueKind::Float(_) => "float",
        ValueKind::Str(_) => "str",
        ValueKind::None => "NoneType",
        ValueKind::List(_) => "list",
        ValueKind::Tuple(_) => "tuple",
        ValueKind::Dict(_) | ValueKind::DictKeysView(_) | ValueKind::DictValuesView(_) | ValueKind::DictItemsView(_) => "dict",
        ValueKind::Set(_) => "set",
        ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_) | ValueKind::BoundMethod { .. } => "function",
        _ => "object",
    }
}

/// Total order for Python values used by `sorted()` / `min()` / `max()` and
/// comparison operators.  Mirrors CPython's `<` semantics: numbers by
/// magnitude, strings lexicographically, bools as 0/1, lists and tuples
/// lexicographically element-by-element.  Incomparable pairs return a
/// `TypeError`.
fn compare_values(a: &Value, b: &Value) -> Result<std::cmp::Ordering> {
    use crate::value::{PyBigInt, PyToPrimitive};
    match (a.kind(), b.kind()) {
        (ValueKind::Int(x), ValueKind::Int(y)) => Ok(x.cmp(&y)),
        (ValueKind::Float(x), ValueKind::Float(y)) => Ok(x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)),
        (ValueKind::Int(x), ValueKind::Float(y)) => Ok((x as f64).partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)),
        (ValueKind::Float(x), ValueKind::Int(y)) => Ok(x.partial_cmp(&(y as f64)).unwrap_or(std::cmp::Ordering::Equal)),
        (ValueKind::Bool(x), ValueKind::Bool(y)) => Ok(x.cmp(&y)),
        (ValueKind::Bool(x), ValueKind::Int(y)) => Ok((x as i64).cmp(&y)),
        (ValueKind::Int(x), ValueKind::Bool(y)) => Ok(x.cmp(&(y as i64))),
        (ValueKind::BigInt(x), ValueKind::BigInt(y)) => Ok(x.cmp(y)),
        (ValueKind::BigInt(x), ValueKind::Int(y)) => Ok((*x).cmp(&PyBigInt::from(y))),
        (ValueKind::Int(x), ValueKind::BigInt(y)) => Ok(PyBigInt::from(x).cmp(y)),
        (ValueKind::BigInt(x), ValueKind::Float(y)) => Ok(x
            .to_f64()
            .and_then(|xf| xf.partial_cmp(&y))
            .unwrap_or(std::cmp::Ordering::Equal)),
        (ValueKind::Float(x), ValueKind::BigInt(y)) => Ok(y
            .to_f64()
            .and_then(|yf| x.partial_cmp(&yf))
            .map(|o| o.reverse())
            .unwrap_or(std::cmp::Ordering::Equal)),
        (ValueKind::Str(x), ValueKind::Str(y)) => Ok(x.cmp(y)),
        (ValueKind::List(x), ValueKind::List(y)) => {
            for (a, b) in x.iter().zip(y.iter()) {
                let ord = compare_values(a, b)?;
                if ord != std::cmp::Ordering::Equal {
                    return Ok(ord);
                }
            }
            Ok(x.len().cmp(&y.len()))
        }
        (ValueKind::Tuple(x), ValueKind::Tuple(y)) => {
            for (a, b) in x.iter().zip(y.iter()) {
                let ord = compare_values(a, b)?;
                if ord != std::cmp::Ordering::Equal {
                    return Ok(ord);
                }
            }
            Ok(x.len().cmp(&y.len()))
        }
        _ => Err(PyError::Named(
            "TypeError".to_string(),
            format!(
                "'<' not supported between instances of '{}' and '{}'",
                value_type_name_str(a),
                value_type_name_str(b),
            ),
        )),
    }
}

fn lookup_class_attr(class: &Rc<RefCell<PyClass>>, name: &str) -> Option<Value> {
    let (value, base) = {
        let borrowed = class.borrow();
        (borrowed.attrs.get(name).cloned(), borrowed.base.clone())
    };
    if value.is_some() {
        return value;
    }
    base.and_then(|base| lookup_class_attr(&base, name))
}

struct PrintOptions {
    values: Vec<Value>,
    sep: String,
    end: String,
}

#[derive(Debug, Clone)]
struct ExpandedCallArg {
    name: Option<String>,
    value: Value,
}

fn extract_optional_string(value: Value, name: &str) -> Result<Option<String>> {
    match value.kind() {
        ValueKind::Str(text) => Ok(Some(text.to_string())),
        ValueKind::None => Ok(None),
        _ => Err(PyError::Runtime(format!(
            "print() {} must be None or a string",
            name
        ))),
    }
}

fn reject_keyword_args_expanded(function_name: &str, args: &[ExpandedCallArg]) -> Result<()> {
    if args.iter().any(|arg| arg.name.is_some()) {
        return Err(PyError::Runtime(format!(
            "{}() does not accept keyword arguments",
            function_name
        )));
    }
    Ok(())
}

fn py_mod_i64(a: i64, b: i64) -> i64 {
    let mut remainder = a % b;
    if (remainder > 0 && b < 0) || (remainder < 0 && b > 0) {
        remainder += b;
    }
    remainder
}


fn normalize_index(index: &Value, len: usize) -> Result<usize> {
    let mut value = match index.kind() {
        ValueKind::Int(v) => v,
        ValueKind::Bool(b) => b as i64,
        _ => return Err(PyError::Runtime("indices must be integers".to_string())),
    };
    if value < 0 {
        value += len as i64;
    }
    if value < 0 || value >= len as i64 {
        return Err(PyError::Runtime("index out of range".to_string()));
    }
    Ok(value as usize)
}

fn class_is_subclass_of(class: &Rc<RefCell<PyClass>>, expected: &Rc<RefCell<PyClass>>) -> bool {
    if Rc::ptr_eq(class, expected) {
        return true;
    }
    let base = class.borrow().base.clone();
    base.is_some_and(|base| class_is_subclass_of(&base, expected))
}

fn is_exception_class(class: &Rc<RefCell<PyClass>>) -> bool {
    let (name, base) = {
        let borrowed = class.borrow();
        (borrowed.name.clone(), borrowed.base.clone())
    };
    if name == "Exception" {
        return true;
    }
    base.is_some_and(|base| is_exception_class(&base))
}

fn instantiate_exception(class: Rc<RefCell<PyClass>>, args: Vec<Value>) -> Value {
    let mut attrs = HashMap::new();
    attrs.insert("args".to_string(), Value::list(args));
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

fn install_exception_builtins(env: &EnvRef) {
    let exception = Rc::new(RefCell::new(PyClass {
        name: "Exception".to_string(),
        base: None,
        attrs: HashMap::new(),
    }));

    let make_child = |name: &str| {
        Rc::new(RefCell::new(PyClass {
            name: name.to_string(),
            base: Some(Rc::clone(&exception)),
            attrs: HashMap::new(),
        }))
    };

    let runtime_error = make_child("RuntimeError");
    let type_error = make_child("TypeError");
    let value_error = make_child("ValueError");
    let name_error = make_child("NameError");
    let assertion_error = make_child("AssertionError");
    let recursion_error = make_child("RecursionError");
    let not_implemented_error = make_child("NotImplementedError");
    let stop_iteration = make_child("StopIteration");
    let index_error = make_child("IndexError");
    let key_error = make_child("KeyError");
    let attribute_error = make_child("AttributeError");
    let overflow_error = make_child("OverflowError");
    let system_exit = make_child("SystemExit");

    let mut module = env.borrow_mut();
    module
        .values
        .insert("Exception".to_string(), Value::py_class(exception));
    module
        .values
        .insert("RuntimeError".to_string(), Value::py_class(runtime_error));
    module
        .values
        .insert("TypeError".to_string(), Value::py_class(type_error));
    module
        .values
        .insert("ValueError".to_string(), Value::py_class(value_error));
    module
        .values
        .insert("NameError".to_string(), Value::py_class(name_error));
    module
        .values
        .insert("AssertionError".to_string(), Value::py_class(assertion_error));
    module
        .values
        .insert("RecursionError".to_string(), Value::py_class(recursion_error));
    module
        .values
        .insert("NotImplementedError".to_string(), Value::py_class(not_implemented_error));
    module
        .values
        .insert("StopIteration".to_string(), Value::py_class(stop_iteration));
    module
        .values
        .insert("IndexError".to_string(), Value::py_class(index_error));
    module
        .values
        .insert("KeyError".to_string(), Value::py_class(key_error));
    module
        .values
        .insert("AttributeError".to_string(), Value::py_class(attribute_error));
    module
        .values
        .insert("OverflowError".to_string(), Value::py_class(overflow_error));
    module
        .values
        .insert("SystemExit".to_string(), Value::py_class(system_exit));
}

fn key_to_value(key: PyKey) -> Value {
    match key {
        PyKey::Int(v) => Value::int(v),
        PyKey::Float(v) => Value::float(f64::from_bits(v)),
        PyKey::Str(v) => Value::string(v),
        PyKey::Bool(v) => Value::bool_(v),
        PyKey::None => Value::none(),
    }
}

fn module_env(env: &EnvRef) -> EnvRef {
    let mut current = Rc::clone(env);
    loop {
        let parent = current.borrow().parent.clone();
        match parent {
            Some(parent) => current = parent,
            None => return current,
        }
    }
}

fn lookup_name_in_module(env: &EnvRef, name: &str) -> Option<Value> {
    module_env(env).borrow().values.get(name).cloned()
}

fn has_local_binding_in_current_or_ancestor(env: &EnvRef, name: &str) -> bool {
    let mut current = Some(Rc::clone(env));
    while let Some(candidate) = current {
        let (is_function_scope, has_name, next) = {
            let borrowed = candidate.borrow();
            (
                borrowed.parent.is_some(),
                borrowed.local_names.contains(name),
                borrowed.parent.clone(),
            )
        };
        if is_function_scope && has_name {
            return true;
        }
        current = next;
    }
    false
}

fn find_enclosing_local_env_for_name(env: &EnvRef, name: &str) -> Option<EnvRef> {
    let mut current = env.borrow().parent.clone();
    while let Some(candidate) = current {
        let (is_function_scope, has_name, next) = {
            let borrowed = candidate.borrow();
            (
                borrowed.parent.is_some(),
                borrowed.local_names.contains(name),
                borrowed.parent.clone(),
            )
        };
        if is_function_scope && has_name {
            return Some(candidate);
        }
        current = next;
    }
    None
}

fn lookup_name_in_enclosing_local_env(env: &EnvRef, name: &str) -> Result<Option<Value>> {
    let Some(target_env) = find_enclosing_local_env_for_name(env, name) else {
        return Err(PyError::Runtime(format!(
            "no binding for nonlocal '{}' found",
            name
        )));
    };
    lookup_name_in_env(&target_env, name)
}


// Write `value` into `env` for `name`, using fastlocals slot when available.
#[inline]
fn env_assign_local(env: &EnvRef, name: &str, value: Value) {
    let mut borrowed = env.borrow_mut();
    if let Some(fl) = &mut borrowed.fastlocals {
        if let Some(&idx) = fl.index.get(name) {
            fl.slots[idx] = Some(value);
            return;
        }
    }
    borrowed.values.insert(name.to_string(), value);
}

// Walk the env chain and return the `EnvRef` that owns `name`, without cloning
// the value. Returns `None` if the name is unresolvable (not found or unbound local).
fn find_env_for_name(env: &EnvRef, name: &str) -> Option<EnvRef> {
    let mut current = Rc::clone(env);
    loop {
        let (found, is_local_name, parent) = {
            let borrowed = current.borrow();
            let found = if let Some(fl) = &borrowed.fastlocals {
                if let Some(&idx) = fl.index.get(name) {
                    fl.slots[idx].is_some()
                } else {
                    borrowed.values.contains_key(name)
                }
            } else {
                borrowed.values.contains_key(name)
            };
            (found, borrowed.local_names.contains(name), borrowed.parent.clone())
        };
        if found {
            return Some(current);
        }
        if is_local_name {
            return None;
        }
        match parent {
            Some(p) => current = p,
            None => return None,
        }
    }
}

fn lookup_name_in_env(env: &EnvRef, name: &str) -> Result<Option<Value>> {
    let borrowed = env.borrow();
    if let Some(fl) = &borrowed.fastlocals {
        if let Some(&idx) = fl.index.get(name) {
            return if idx < 64 && fl.def_bound_mask & (1u64 << idx) != 0 {
                // Definitely-bound slot — skip the None check (analogous to CPython LOAD_FAST).
                Ok(Some(fl.slots[idx].as_ref().unwrap().clone()))
            } else {
                match &fl.slots[idx] {
                    Some(v) => Ok(Some(v.clone())),
                    None => Err(PyError::Runtime(format!(
                        "cannot access local variable '{}' where it is not associated with a value",
                        name
                    ))),
                }
            };
        }
    }
    let value = borrowed.values.get(name).cloned();
    let is_local_name = borrowed.local_names.contains(name);
    let parent = borrowed.parent.clone();
    drop(borrowed);
    if value.is_some() {
        return Ok(value);
    }
    if is_local_name {
        return Err(PyError::Runtime(format!(
            "cannot access local variable '{}' where it is not associated with a value",
            name
        )));
    }
    match parent {
        Some(parent) => lookup_name_in_env(&parent, name),
        None => Ok(None),
    }
}

pub(crate) fn collect_local_names(
    params: &[crate::ast::FunctionParam],
    body: &[Stmt],
    global_names: &HashSet<String>,
    nonlocal_names: &HashSet<String>,
) -> HashSet<String> {
    let mut names = params.iter().map(|param| param.name.clone()).collect();
    collect_local_names_from_block(body, &mut names, global_names, nonlocal_names);
    names
}

fn collect_local_names_from_block(
    body: &[Stmt],
    names: &mut HashSet<String>,
    global_names: &HashSet<String>,
    nonlocal_names: &HashSet<String>,
) {
    for stmt in body {
        match stmt {
            Stmt::Assign(target, _) => {
                collect_assign_target_names(target, names, global_names, nonlocal_names);
            }
            Stmt::AttrAssign { .. } => {}
            Stmt::Def { name, .. } => {
                if !global_names.contains(name) && !nonlocal_names.contains(name) {
                    names.insert(name.clone());
                }
            }
            Stmt::Class { name, .. } => {
                if !global_names.contains(name) && !nonlocal_names.contains(name) {
                    names.insert(name.clone());
                }
            }
            Stmt::Global(_) | Stmt::Nonlocal(_) => {}
            Stmt::Import {
                names: import_names,
            } => {
                for (module, alias) in import_names {
                    let bound = alias
                        .clone()
                        .unwrap_or_else(|| module.split('.').next().unwrap_or(module).to_string());
                    if !global_names.contains(&bound) && !nonlocal_names.contains(&bound) {
                        names.insert(bound);
                    }
                }
            }
            Stmt::ImportFrom {
                names: import_names,
                ..
            } => {
                for (attr_name, alias) in import_names {
                    if attr_name == "*" {
                        continue;
                    }
                    let bound = alias.clone().unwrap_or_else(|| attr_name.clone());
                    if !global_names.contains(&bound) && !nonlocal_names.contains(&bound) {
                        names.insert(bound);
                    }
                }
            }
            Stmt::AugAssign { .. }
            | Stmt::IndexAssign { .. }
            | Stmt::SliceAssign { .. }
            | Stmt::Delete(_)
            | Stmt::Assert { .. }
            | Stmt::Expr(_)
            | Stmt::Raise { .. }
            | Stmt::Return(_)
            | Stmt::Break
            | Stmt::Continue
            | Stmt::Pass => {}
            Stmt::With { items, body } => {
                for (_, alias) in items {
                    if let Some(target) = alias {
                        collect_assign_target_names(target, names, global_names, nonlocal_names);
                    }
                }
                collect_local_names_from_block(body, names, global_names, nonlocal_names);
            }
            Stmt::If {
                branches,
                else_branch,
            } => {
                for (_, branch) in branches {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
                if let Some(branch) = else_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
            }
            Stmt::While {
                body, else_branch, ..
            } => {
                collect_local_names_from_block(body, names, global_names, nonlocal_names);
                if let Some(branch) = else_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
            } => {
                collect_local_names_from_block(body, names, global_names, nonlocal_names);
                for handler in handlers {
                    if let Some(name) = &handler.name {
                        if !global_names.contains(name) && !nonlocal_names.contains(name) {
                            names.insert(name.clone());
                        }
                    }
                    collect_local_names_from_block(
                        &handler.body,
                        names,
                        global_names,
                        nonlocal_names,
                    );
                }
                if let Some(branch) = else_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
                if let Some(branch) = finally_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
            }
            Stmt::For {
                target,
                body,
                else_branch,
                ..
            } => {
                collect_assign_target_names(target, names, global_names, nonlocal_names);
                collect_local_names_from_block(body, names, global_names, nonlocal_names);
                if let Some(branch) = else_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
            }
        }
    }
}

pub(crate) fn collect_global_names(body: &[Stmt]) -> HashSet<String> {
    collect_declared_names(body, |s| {
        if let Stmt::Global(names) = s { Some(names) } else { None }
    })
}

pub(crate) fn collect_nonlocal_names(body: &[Stmt]) -> HashSet<String> {
    collect_declared_names(body, |s| {
        if let Stmt::Nonlocal(names) = s { Some(names) } else { None }
    })
}

fn collect_declared_names(body: &[Stmt], pick: fn(&Stmt) -> Option<&Vec<String>>) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_declared_names_from_block(body, &mut names, pick);
    names
}

fn collect_declared_names_from_block(
    body: &[Stmt],
    names: &mut HashSet<String>,
    pick: fn(&Stmt) -> Option<&Vec<String>>,
) {
    for stmt in body {
        if let Some(declared) = pick(stmt) {
            names.extend(declared.iter().cloned());
            continue;
        }
        match stmt {
            Stmt::If { branches, else_branch } => {
                for (_, branch) in branches {
                    collect_declared_names_from_block(branch, names, pick);
                }
                if let Some(branch) = else_branch {
                    collect_declared_names_from_block(branch, names, pick);
                }
            }
            Stmt::While { body, else_branch, .. } => {
                collect_declared_names_from_block(body, names, pick);
                if let Some(branch) = else_branch {
                    collect_declared_names_from_block(branch, names, pick);
                }
            }
            Stmt::For { body, else_branch, .. } => {
                collect_declared_names_from_block(body, names, pick);
                if let Some(branch) = else_branch {
                    collect_declared_names_from_block(branch, names, pick);
                }
            }
            Stmt::Try { body, handlers, else_branch, finally_branch } => {
                collect_declared_names_from_block(body, names, pick);
                for handler in handlers {
                    collect_declared_names_from_block(&handler.body, names, pick);
                }
                if let Some(branch) = else_branch {
                    collect_declared_names_from_block(branch, names, pick);
                }
                if let Some(branch) = finally_branch {
                    collect_declared_names_from_block(branch, names, pick);
                }
            }
            Stmt::With { body, .. } => {
                collect_declared_names_from_block(body, names, pick);
            }
            _ => {}
        }
    }
}

fn cmp_op_to_binary_op(op: CmpOp) -> BinaryOp {
    match op {
        CmpOp::Eq => BinaryOp::Eq,
        CmpOp::Ne => BinaryOp::Ne,
        CmpOp::Lt => BinaryOp::Lt,
        CmpOp::Le => BinaryOp::Le,
        CmpOp::Gt => BinaryOp::Gt,
        CmpOp::Ge => BinaryOp::Ge,
        CmpOp::In => BinaryOp::In,
        CmpOp::NotIn => BinaryOp::NotIn,
        CmpOp::Is => BinaryOp::Is,
        CmpOp::IsNot => BinaryOp::IsNot,
    }
}

fn values_are_identical(a: &Value, b: &Value) -> bool {
    match (a.kind(), b.kind()) {
        (ValueKind::None, ValueKind::None) => true,
        (ValueKind::Bool(x), ValueKind::Bool(y)) => x == y,
        (ValueKind::Int(x), ValueKind::Int(y)) => x == y,
        (ValueKind::PyInstance(x), ValueKind::PyInstance(y)) => Rc::ptr_eq(x, y),
        (ValueKind::PyClass(x), ValueKind::PyClass(y)) => Rc::ptr_eq(x, y),
        (ValueKind::UserFunction(x), ValueKind::UserFunction(y)) => Rc::ptr_eq(x, y),
        // BuiltinFunction values are singletons by name (static str identity).
        // This makes `type(5) is type(5)` True since both return the same name tag.
        (ValueKind::BuiltinFunction(x), ValueKind::BuiltinFunction(y)) => x == y,
        _ => false,
    }
}

fn collect_assign_target_names(
    target: &AssignTarget,
    names: &mut std::collections::HashSet<String>,
    global_names: &std::collections::HashSet<String>,
    nonlocal_names: &std::collections::HashSet<String>,
) {
    match target {
        AssignTarget::Name(n) => {
            if !global_names.contains(n) && !nonlocal_names.contains(n) {
                names.insert(n.clone());
            }
        }
        AssignTarget::Tuple(targets) => {
            for t in targets {
                collect_assign_target_names(t, names, global_names, nonlocal_names);
            }
        }
        AssignTarget::Attr(..) | AssignTarget::Index(..) => {}
    }
}

pub(crate) fn compute_def_bound_mask(
    params: &[crate::ast::FunctionParam],
    local_index: &HashMap<String, crate::bytecode::Reg>,
) -> u64 {
    let mut mask: u64 = 0;
    // Only parameters are guaranteed bound at function entry — they are set
    // by the call setup code before the body runs.  Body-level assignments
    // are NOT included here because a name can be read (as a local) before
    // it is assigned (e.g. `y = x; x = 9`), which would cause an unsound
    // unwrap.  The parameter-only subset is sufficient to eliminate the
    // None check for the most frequently read locals in hot inner loops.
    for param in params {
        if let Some(&idx) = local_index.get(&param.name) {
            if idx < 64 {
                mask |= 1u64 << idx;
            }
        }
    }
    mask
}

fn float_to_bigint(f: f64) -> Value {
    use crate::value::PyBigInt;
    // Convert via the decimal string representation of the f64's integer value.
    let s = format!("{:.0}", f);
    let n: PyBigInt = s.parse().unwrap_or_else(|_| PyBigInt::from(0i64));
    Value::bigint(n)
}

fn value_to_float(v: &Value, ctx: &str) -> Result<f64> {
    match v.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(i) => Ok(i as f64),
        ValueKind::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
        _ => Err(PyError::Runtime(format!(
            "{ctx}: a float is required, not {}",
            v.repr()
        ))),
    }
}

fn make_math_module() -> Value {
    let mut attrs: HashMap<String, Value> = HashMap::new();
    attrs.insert("pi".to_string(), Value::float(std::f64::consts::PI));
    attrs.insert("e".to_string(), Value::float(std::f64::consts::E));
    attrs.insert("tau".to_string(), Value::float(std::f64::consts::TAU));
    attrs.insert("inf".to_string(), Value::float(f64::INFINITY));
    attrs.insert("nan".to_string(), Value::float(f64::NAN));
    const MATH_FUNS: &[(&str, &str)] = &[
        ("floor", "math.floor"),
        ("ceil", "math.ceil"),
        ("sqrt", "math.sqrt"),
        ("fabs", "math.fabs"),
        ("sin", "math.sin"),
        ("cos", "math.cos"),
        ("tan", "math.tan"),
        ("asin", "math.asin"),
        ("acos", "math.acos"),
        ("atan", "math.atan"),
        ("exp", "math.exp"),
        ("log2", "math.log2"),
        ("log10", "math.log10"),
        ("isnan", "math.isnan"),
        ("isinf", "math.isinf"),
        ("pow", "math.pow"),
        ("atan2", "math.atan2"),
        ("log", "math.log"),
    ];
    for (name, builtin) in MATH_FUNS {
        attrs.insert(name.to_string(), Value::builtin_function(builtin));
    }
    Value::py_module(Rc::new(RefCell::new(PyModule {
        name: "math".to_string(),
        attrs,
    })))
}

fn make_sys_module() -> Value {
    let mut attrs: HashMap<String, Value> = HashMap::new();
    attrs.insert("version".to_string(), Value::string("PyRust 0.2".to_string()));
    attrs.insert("argv".to_string(), Value::list(Vec::new()));
    attrs.insert("exit".to_string(), Value::builtin_function("sys.exit"));
    Value::py_module(Rc::new(RefCell::new(PyModule {
        name: "sys".to_string(),
        attrs,
    })))
}

// Names of built-in callables with side effects (I/O, process control).
const IMPURE_BUILTINS: &[&str] = &["print", "input", "open", "exit", "quit", "exec", "eval"];

/// Returns true if `expr` contains no call to a known side-effecting builtin.
/// Does NOT attempt to verify purity of user-defined functions called within —
/// that would require whole-program analysis.  The conservative call-site rule
/// (only cache if `is_pure`) relies on the *body* of those callees being clean.
fn is_pure_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::None => true,
        Expr::Var(_) => true,
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            items.iter().all(is_pure_expr)
        }
        Expr::Dict(pairs) => pairs.iter().all(|(k, v)| is_pure_expr(k) && is_pure_expr(v)),
        Expr::Unary { expr, .. } => is_pure_expr(expr),
        Expr::Binary { left, right, .. } => is_pure_expr(left) && is_pure_expr(right),
        Expr::Compare { left, ops } => {
            is_pure_expr(left) && ops.iter().all(|(_, e)| is_pure_expr(e))
        }
        Expr::Ternary { cond, then, else_ } => {
            is_pure_expr(cond) && is_pure_expr(then) && is_pure_expr(else_)
        }
        Expr::Lambda { .. } => true,
        Expr::Call { func, args } => {
            // Direct call to a known side-effecting name → impure.
            if let Expr::Var(name) = func.as_ref() {
                if IMPURE_BUILTINS.contains(&name.as_str()) {
                    return false;
                }
            }
            is_pure_expr(func) && args.iter().all(|a| is_pure_expr(&a.value))
        }
        Expr::Attr { target, .. } => is_pure_expr(target),
        Expr::Index { target, index } => is_pure_expr(target) && is_pure_expr(index),
        Expr::Slice { target, lower, upper, step } => {
            is_pure_expr(target)
                && lower.as_deref().map_or(true, is_pure_expr)
                && upper.as_deref().map_or(true, is_pure_expr)
                && step.as_deref().map_or(true, is_pure_expr)
        }
    }
}

/// Returns true if every statement in `body` is free of observable side effects.
///
/// Conservative: attribute/index mutation, global/nonlocal declarations,
/// import statements, and `with` blocks are treated as impure.  Calls to
/// user-defined functions are assumed pure (the body of each callee is
/// analysed when *that* function is defined).
pub(crate) fn is_pure_body(body: &[Stmt]) -> bool {
    body.iter().all(is_pure_stmt)
}

fn is_pure_stmt(stmt: &Stmt) -> bool {
    match stmt {
        // Explicit side effects on outer state.
        Stmt::Global(_) | Stmt::Nonlocal(_) => false,
        // Object / container mutation.
        Stmt::AttrAssign { .. } | Stmt::IndexAssign { .. } | Stmt::SliceAssign { .. } => false,
        // Deletion and imports can affect shared state.
        Stmt::Delete(_) | Stmt::Import { .. } | Stmt::ImportFrom { .. } => false,
        // `with` typically wraps I/O or resource-management side effects.
        Stmt::With { .. } => false,

        // Assignments and augmented assignments are local writes → pure if RHS is.
        Stmt::Assign(_, expr) | Stmt::Expr(expr) => is_pure_expr(expr),
        Stmt::AugAssign { expr, .. } => is_pure_expr(expr),
        Stmt::Return(Some(expr)) => is_pure_expr(expr),
        Stmt::Return(None) => true,
        Stmt::Assert { test, msg } => {
            is_pure_expr(test) && msg.as_ref().map_or(true, is_pure_expr)
        }
        Stmt::Raise { expr, cause } => {
            expr.as_ref().map_or(true, is_pure_expr)
                && cause.as_ref().map_or(true, is_pure_expr)
        }

        // Control flow: recurse into sub-blocks.
        Stmt::If { branches, else_branch } => {
            branches.iter().all(|(cond, blk)| is_pure_expr(cond) && is_pure_body(blk))
                && else_branch.as_deref().map_or(true, is_pure_body)
        }
        Stmt::While { cond, body, else_branch } => {
            is_pure_expr(cond)
                && is_pure_body(body)
                && else_branch.as_deref().map_or(true, is_pure_body)
        }
        Stmt::For { iter, body, else_branch, .. } => {
            is_pure_expr(iter)
                && is_pure_body(body)
                && else_branch.as_deref().map_or(true, is_pure_body)
        }
        Stmt::Try { body, handlers, else_branch, finally_branch } => {
            is_pure_body(body)
                && handlers.iter().all(|h| is_pure_body(&h.body))
                && else_branch.as_deref().map_or(true, is_pure_body)
                && finally_branch.as_deref().map_or(true, is_pure_body)
        }

        // Nested definitions don't execute side effects at definition time.
        Stmt::Def { .. } | Stmt::Class { .. } => true,
        Stmt::Pass | Stmt::Break | Stmt::Continue => true,
    }
}


