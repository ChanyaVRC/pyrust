enum BlockOutcome {
    Signal(ExecSignal),
    Error(PyError),
}

impl BlockOutcome {
    fn from_result(result: Result<ExecSignal>) -> Self {
        match result {
            Ok(signal) => Self::Signal(signal),
            Err(error) => Self::Error(error),
        }
    }

    fn into_result(self) -> Result<ExecSignal> {
        match self {
            Self::Signal(signal) => Ok(signal),
            Self::Error(error) => Err(error),
        }
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
    match value {
        Value::Str(text) => Ok(Some(text)),
        Value::None => Ok(None),
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

fn set_dict_key(items: &mut Vec<(PyKey, Value)>, key: PyKey, value: Value) {
    for (existing_key, existing_value) in items.iter_mut() {
        if *existing_key == key {
            *existing_value = value;
            return;
        }
    }
    items.push((key, value));
}

fn normalize_index(index: &Value, len: usize) -> Result<usize> {
    let mut value = match index {
        Value::Int(v) => *v,
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
    attrs.insert("args".to_string(), Value::List(args));
    Value::Instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
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
    let assertion_error = make_child("AssertionError");

    let mut module = env.borrow_mut();
    module
        .values
        .insert("Exception".to_string(), Value::Class(exception));
    module
        .values
        .insert("RuntimeError".to_string(), Value::Class(runtime_error));
    module
        .values
        .insert("TypeError".to_string(), Value::Class(type_error));
    module
        .values
        .insert("ValueError".to_string(), Value::Class(value_error));
    module
        .values
        .insert("AssertionError".to_string(), Value::Class(assertion_error));
}

fn key_to_value(key: PyKey) -> Value {
    match key {
        PyKey::Int(v) => Value::Int(v),
        PyKey::Float(v) => Value::Float(f64::from_bits(v)),
        PyKey::Str(v) => Value::Str(v),
        PyKey::Bool(v) => Value::Bool(v),
        PyKey::None => Value::None,
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

fn has_enclosing_local_binding(env: &EnvRef, name: &str) -> bool {
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

fn lookup_name_in_env(env: &EnvRef, name: &str) -> Result<Option<Value>> {
    let (value, is_local_name, parent) = {
        let borrowed = env.borrow();
        (
            borrowed.values.get(name).cloned(),
            borrowed.local_names.contains(name),
            borrowed.parent.clone(),
        )
    };
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

fn collect_local_names(
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

fn collect_global_names(body: &[Stmt]) -> HashSet<String> {
    collect_declared_names(body, |s| {
        if let Stmt::Global(names) = s { Some(names) } else { None }
    })
}

fn collect_nonlocal_names(body: &[Stmt]) -> HashSet<String> {
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
    match (a, b) {
        (Value::None, Value::None) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Instance(x), Value::Instance(y)) => Rc::ptr_eq(x, y),
        (Value::Class(x), Value::Class(y)) => Rc::ptr_eq(x, y),
        (Value::Function(x), Value::Function(y)) => Rc::ptr_eq(x, y),
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

fn value_to_float(v: &Value, ctx: &str) -> Result<f64> {
    match v {
        Value::Float(f) => Ok(*f),
        Value::Int(i) => Ok(*i as f64),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        _ => Err(PyError::Runtime(format!(
            "{ctx}: a float is required, not {}",
            v.repr()
        ))),
    }
}

fn make_math_module() -> Value {
    let mut attrs: HashMap<String, Value> = HashMap::new();
    attrs.insert("pi".to_string(), Value::Float(std::f64::consts::PI));
    attrs.insert("e".to_string(), Value::Float(std::f64::consts::E));
    attrs.insert("tau".to_string(), Value::Float(std::f64::consts::TAU));
    attrs.insert("inf".to_string(), Value::Float(f64::INFINITY));
    attrs.insert("nan".to_string(), Value::Float(f64::NAN));
    for fname in &[
        "floor", "ceil", "sqrt", "fabs", "sin", "cos", "tan", "asin", "acos", "atan", "exp",
        "log2", "log10", "isnan", "isinf", "pow", "atan2", "log",
    ] {
        // Static string slices for the builtin names
        let builtin: &'static str = match *fname {
            "floor" => "math.floor",
            "ceil" => "math.ceil",
            "sqrt" => "math.sqrt",
            "fabs" => "math.fabs",
            "sin" => "math.sin",
            "cos" => "math.cos",
            "tan" => "math.tan",
            "asin" => "math.asin",
            "acos" => "math.acos",
            "atan" => "math.atan",
            "exp" => "math.exp",
            "log2" => "math.log2",
            "log10" => "math.log10",
            "isnan" => "math.isnan",
            "isinf" => "math.isinf",
            "pow" => "math.pow",
            "atan2" => "math.atan2",
            "log" => "math.log",
            _ => unreachable!(),
        };
        attrs.insert(fname.to_string(), Value::Builtin(builtin));
    }
    Value::Module(Rc::new(RefCell::new(PyModule {
        name: "math".to_string(),
        attrs,
    })))
}

fn make_sys_module() -> Value {
    let mut attrs: HashMap<String, Value> = HashMap::new();
    attrs.insert("version".to_string(), Value::Str("PyRust 0.2".to_string()));
    attrs.insert("argv".to_string(), Value::List(vec![]));
    attrs.insert("exit".to_string(), Value::Builtin("sys.exit"));
    Value::Module(Rc::new(RefCell::new(PyModule {
        name: "sys".to_string(),
        attrs,
    })))
}

fn range_len(start: i64, stop: i64, step: i64) -> i64 {
    if step == 0 {
        return 0;
    }
    if step > 0 {
        if start >= stop {
            0
        } else {
            ((stop - start - 1) / step) + 1
        }
    } else if start <= stop {
        0
    } else {
        ((start - stop - 1) / (-step)) + 1
    }
}

