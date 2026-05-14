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
        BinaryOp::Add => Some(Ok(match a.checked_add(b) {
            Some(r) => Value::int(r),
            None => Value::bigint(PyBigInt::from(a) + PyBigInt::from(b)),
        })),
        BinaryOp::Sub => Some(Ok(match a.checked_sub(b) {
            Some(r) => Value::int(r),
            None => Value::bigint(PyBigInt::from(a) - PyBigInt::from(b)),
        })),
        BinaryOp::Mul => Some(Ok(match a.checked_mul(b) {
            Some(r) => Value::int(r),
            None => Value::bigint(PyBigInt::from(a) * PyBigInt::from(b)),
        })),
        BinaryOp::Div => {
            if b == 0 {
                Some(Err(PyError::named(
                    "ZeroDivisionError",
                    "division by zero".to_string(),
                )))
            } else {
                Some(Ok(Value::float(a as f64 / b as f64)))
            }
        }
        BinaryOp::FloorDiv => {
            if b == 0 {
                Some(Err(PyError::named(
                    "ZeroDivisionError",
                    "integer division or modulo by zero".to_string(),
                )))
            } else {
                let modulo = py_mod_i64(a, b);
                Some(Ok(Value::int((a - modulo) / b)))
            }
        }
        BinaryOp::Mod => {
            if b == 0 {
                Some(Err(PyError::named(
                    "ZeroDivisionError",
                    "integer modulo by zero".to_string(),
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
                Some(Err(PyError::named(
                    "ZeroDivisionError",
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
///
/// Thin alias for [`pyrust_core::builtin_type_name`] — kept locally so the
/// many interpreter call sites stay short.
pub(crate) fn value_type_name_str(v: &Value) -> &'static str {
    pyrust_core::builtin_type_name(v)
}

/// Total order for Python values used by `sorted()` / `min()` / `max()` and
/// comparison operators.  Mirrors CPython's `<` semantics: numbers by
/// magnitude, strings lexicographically, bools as 0/1, lists and tuples
/// lexicographically element-by-element.  Incomparable pairs return a
/// `TypeError`.
pub(crate) fn compare_values(a: &Value, b: &Value) -> Result<std::cmp::Ordering> {
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
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'<' not supported between instances of '{}' and '{}'",
                value_type_name_str(a),
                value_type_name_str(b),
            ),
        )),
    }
}

pub(crate) fn lookup_class_attr(class: &Rc<RefCell<PyClass>>, name: &str) -> Option<Value> {
    let (value, base) = {
        let borrowed = class.borrow();
        (borrowed.attrs.get(name).cloned(), borrowed.base.clone())
    };
    if value.is_some() {
        return value;
    }
    base.and_then(|base| lookup_class_attr(&base, name))
}

thread_local! {
    static OBJECT_CLASS: Rc<RefCell<PyClass>> = Rc::new(RefCell::new(PyClass {
        name: "object".to_string(),
        base: None,
        attrs: IndexMap::new(),
    }));
}

/// Returns the singleton synthetic `object` class used as the terminal
/// entry of every class's `__mro__`. pyrust does not (yet) model `object`
/// as a real first-class type — every user class chains to `None` — so
/// this provides a stable, identity-comparable terminator so that
/// `A.__mro__[-1] is B.__mro__[-1]` holds, matching CPython.
pub(crate) fn object_class_singleton() -> Rc<RefCell<PyClass>> {
    OBJECT_CLASS.with(|c| Rc::clone(c))
}

pub(crate) struct PrintOptions {
    pub(crate) values: Vec<Value>,
    pub(crate) sep: String,
    pub(crate) end: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ExpandedCallArg {
    pub(crate) name: Option<String>,
    pub(crate) value: Value,
}

/// Invoke a method that was looked up on a class — handling both
/// `UserFunction` methods (compiled Python bytecode, bound via the
/// interpreter's user-function path) and `BuiltinFunction` methods
/// (registered Rust dispatch fns from `pyrust_module!`'s `class` block).
///
/// In both cases `instance` is prepended as the implicit `self` —
/// matching how `inst.method(...)` semantics work in CPython.  This
/// helper centralises the binding rule so dunder dispatch sites
/// (`__getitem__`, `__iter__`, `__call__`, `__len__`, `__init__`,
/// …) don't have to repeat the UserFunction-vs-BuiltinFunction
/// branching at every call site.
pub(crate) fn invoke_class_method(
    interp: &mut Interpreter,
    method_val: Value,
    instance: Value,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    match method_val.kind() {
        ValueKind::UserFunction(f) => {
            let func = Rc::clone(f);
            interp.call_user_function_expanded(func, args, &[instance])
        }
        ValueKind::BuiltinFunction(name) => {
            let dispatch = crate::builtin_registry::lookup(name).ok_or_else(|| {
                PyError::Runtime(format!(
                    "internal: builtin method '{name}' not in registry"
                ))
            })?;
            let mut combined: Vec<ExpandedCallArg> = Vec::with_capacity(args.len() + 1);
            combined.push(ExpandedCallArg {
                name: None,
                value: instance,
            });
            combined.extend(args.iter().cloned());
            dispatch(interp, &combined)
        }
        _ => {
            // Resolved class attr is something other than a function —
            // usually because the user did `Foo.method = 42` or similar.
            // Surface the class name + the offending value's type so the
            // diagnostic is actionable.
            let class_name = match instance.kind() {
                ValueKind::PyInstance(i) => i.borrow().class.borrow().name.clone(),
                _ => "<unknown>".to_string(),
            };
            Err(PyError::named(
                "TypeError",
                format!(
                    "'{class_name}' class attribute is not callable (got {})",
                    value_type_name_str(&method_val),
                ),
            ))
        }
    }
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

pub(crate) fn reject_keyword_args_expanded(function_name: &str, args: &[ExpandedCallArg]) -> Result<()> {
    if let Some(arg) = args.iter().find(|arg| arg.name.is_some()) {
        // CPython raises TypeError, not RuntimeError, for unexpected
        // kwargs.  Match that so user code using `except TypeError:` on
        // builtin call failures keeps working.
        let kw = arg.name.as_deref().unwrap_or("");
        return Err(PyError::named(
            "TypeError",
            format!("{function_name}() got an unexpected keyword argument '{kw}'"),
        ));
    }
    Ok(())
}

pub(crate) fn py_mod_i64(a: i64, b: i64) -> i64 {
    let mut remainder = a % b;
    if (remainder > 0 && b < 0) || (remainder < 0 && b > 0) {
        remainder += b;
    }
    remainder
}


fn normalize_index(index: &Value, len: usize, label: &str) -> Result<usize> {
    let mut value = match index.kind() {
        ValueKind::Int(v) => v,
        ValueKind::Bool(b) => b as i64,
        _ => return Err(PyError::Runtime("indices must be integers".to_string())),
    };
    if value < 0 {
        value += len as i64;
    }
    if value < 0 || value >= len as i64 {
        return Err(PyError::named("IndexError", format!("{label} index out of range")));
    }
    Ok(value as usize)
}

pub(crate) fn class_is_subclass_of(class: &Rc<RefCell<PyClass>>, expected: &Rc<RefCell<PyClass>>) -> bool {
    if Rc::ptr_eq(class, expected) {
        return true;
    }
    let base = class.borrow().base.clone();
    base.is_some_and(|base| class_is_subclass_of(&base, expected))
}

pub(crate) fn is_exception_class(class: &Rc<RefCell<PyClass>>) -> bool {
    let (name, base) = {
        let borrowed = class.borrow();
        (borrowed.name.clone(), borrowed.base.clone())
    };
    // `Exception` is the canonical root for catchable exceptions.
    // `GeneratorExit` is a sibling root in CPython (derives from `BaseException`,
    // not `Exception`); we treat its name as a root for the same reason.
    if name == "Exception" || name == "GeneratorExit" {
        return true;
    }
    base.is_some_and(|base| is_exception_class(&base))
}

pub(crate) fn instantiate_exception(class: Rc<RefCell<PyClass>>, args: Vec<Value>) -> Value {
    let mut attrs = IndexMap::new();
    attrs.insert("args".to_string(), Value::list(args));
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

fn install_exception_builtins(env: &EnvRef) {
    let exception = Rc::new(RefCell::new(PyClass {
        name: "Exception".to_string(),
        base: None,
        attrs: IndexMap::new(),
    }));

    let make_child = |name: &str| {
        Rc::new(RefCell::new(PyClass {
            name: name.to_string(),
            base: Some(Rc::clone(&exception)),
            attrs: IndexMap::new(),
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
    let zero_division_error = make_child("ZeroDivisionError");
    let system_exit = make_child("SystemExit");
    let os_error = make_child("OSError");
    // FileNotFoundError inherits from OSError in CPython; we just register it
    // as a sibling for now.
    let file_not_found_error = make_child("FileNotFoundError");
    // `GeneratorExit` is a sibling root in CPython (derives from
    // `BaseException`, not `Exception`).  Modelled here as a class with no
    // base so that `except Exception:` does NOT catch it, while
    // `except GeneratorExit:` and bare `except:` / `finally` still do.
    let generator_exit = Rc::new(RefCell::new(PyClass {
        name: "GeneratorExit".to_string(),
        base: None,
        attrs: IndexMap::new(),
    }));

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
        .insert("ZeroDivisionError".to_string(), Value::py_class(zero_division_error));
    module
        .values
        .insert("SystemExit".to_string(), Value::py_class(system_exit));
    module
        .values
        .insert("OSError".to_string(), Value::py_class(os_error));
    module
        .values
        .insert("FileNotFoundError".to_string(), Value::py_class(file_not_found_error));
    module
        .values
        .insert("GeneratorExit".to_string(), Value::py_class(generator_exit));
}

/// Register built-in singleton values (currently just `NotImplemented`).
/// Kept separate from `install_exception_builtins` because singletons are
/// neither exceptions nor classes; future additions like `Ellipsis` will
/// live here too.
fn install_singleton_builtins(env: &EnvRef) {
    let mut module = env.borrow_mut();
    module
        .values
        .insert("NotImplemented".to_string(), Value::not_implemented());
}

fn key_to_value(key: PyKey) -> Value {
    match key {
        PyKey::Int(v) => Value::int(v),
        PyKey::Float(v) => Value::float(f64::from_bits(v)),
        PyKey::Str(v) => Value::string(v),
        PyKey::Bool(v) => Value::bool_(v),
        PyKey::None => Value::none(),
        PyKey::FrozenSet(items) => {
            let mut set = indexmap::IndexSet::new();
            for k in items {
                set.insert(k);
            }
            pyrust_builtins::frozenset::frozenset(set)
        }
        PyKey::Tuple(items) => Value::tuple(items.into_iter().map(key_to_value).collect()),
        PyKey::Object { value, .. } => value,
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

pub(crate) fn lookup_name_in_module(env: &EnvRef, name: &str) -> Option<Value> {
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
    if let Some(fl) = &mut borrowed.fastlocals
        && let Some(&idx) = fl.index.get(name) {
            fl.slots[idx] = Some(value);
            return;
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
    if let Some(fl) = &borrowed.fastlocals
        && let Some(&idx) = fl.index.get(name) {
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
            | Stmt::Raise { .. }
            | Stmt::Break
            | Stmt::Continue
            | Stmt::Pass => {}
            // Walk expressions for walrus operator targets.
            Stmt::Expr(e) => {
                collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
            }
            Stmt::Return(Some(e)) => {
                collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
            }
            Stmt::Return(None) => {}
            Stmt::Assert { test, msg } => {
                collect_walrus_targets_in_expr(test, names, global_names, nonlocal_names);
                if let Some(m) = msg {
                    collect_walrus_targets_in_expr(m, names, global_names, nonlocal_names);
                }
            }
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
                for (cond, branch) in branches {
                    collect_walrus_targets_in_expr(cond, names, global_names, nonlocal_names);
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
                if let Some(branch) = else_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
            }
            Stmt::While {
                cond,
                body,
                else_branch,
            } => {
                collect_walrus_targets_in_expr(cond, names, global_names, nonlocal_names);
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
                    if let Some(name) = &handler.name
                        && !global_names.contains(name) && !nonlocal_names.contains(name) {
                            names.insert(name.clone());
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
                iter,
                body,
                else_branch,
            } => {
                collect_walrus_targets_in_expr(iter, names, global_names, nonlocal_names);
                collect_assign_target_names(target, names, global_names, nonlocal_names);
                collect_local_names_from_block(body, names, global_names, nonlocal_names);
                if let Some(branch) = else_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
            }
            Stmt::Match { subject, arms } => {
                collect_walrus_targets_in_expr(subject, names, global_names, nonlocal_names);
                for arm in arms {
                    // Collect capture names introduced by patterns.
                    collect_pattern_names(&arm.pattern, names, global_names, nonlocal_names);
                    if let Some(guard) = &arm.guard {
                        collect_walrus_targets_in_expr(guard, names, global_names, nonlocal_names);
                    }
                    collect_local_names_from_block(&arm.body, names, global_names, nonlocal_names);
                }
            }
        }
    }
}

/// Collect names that a pattern binds (capture patterns, star captures in sequences,
/// and `**rest` in mappings).
fn collect_pattern_names(
    pattern: &crate::ast::Pattern,
    names: &mut std::collections::HashSet<String>,
    global_names: &std::collections::HashSet<String>,
    nonlocal_names: &std::collections::HashSet<String>,
) {
    use crate::ast::Pattern;
    match pattern {
        Pattern::Wildcard | Pattern::Literal(_) => {}
        Pattern::Capture(name) => {
            if !global_names.contains(name) && !nonlocal_names.contains(name) {
                names.insert(name.clone());
            }
        }
        Pattern::Or(alts) => {
            for alt in alts {
                collect_pattern_names(alt, names, global_names, nonlocal_names);
            }
        }
        Pattern::Sequence(elems) => {
            for (elem_pat, _) in elems {
                collect_pattern_names(elem_pat, names, global_names, nonlocal_names);
            }
        }
        Pattern::Mapping(pairs, rest) => {
            for (_, val_pat) in pairs {
                collect_pattern_names(val_pat, names, global_names, nonlocal_names);
            }
            if let Some(rest_name) = rest
                && !global_names.contains(rest_name) && !nonlocal_names.contains(rest_name) {
                    names.insert(rest_name.clone());
                }
        }
        Pattern::Class { kwargs, .. } => {
            for (_, attr_pat) in kwargs {
                collect_pattern_names(attr_pat, names, global_names, nonlocal_names);
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
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    collect_declared_names_from_block(&arm.body, names, pick);
                }
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
        // For mutable containers (list/dict/set) and tuples, identity is the
        // shared backing-storage id surfaced by `value_id()` — `b = a; a is b`
        // is True after Rc-sharing storage on clone (#305).  Two distinct
        // literals of the same shape produce different ids, matching CPython.
        (ValueKind::List(_), ValueKind::List(_))
        | (ValueKind::Set(_), ValueKind::Set(_))
        | (ValueKind::Dict(_), ValueKind::Dict(_))
        | (ValueKind::Tuple(_), ValueKind::Tuple(_)) => match (a.value_id(), b.value_id()) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
        _ => false,
    }
}

/// Walk an expression tree and collect names bound by walrus operators (`:=`).
fn collect_walrus_targets_in_expr(
    expr: &Expr,
    names: &mut std::collections::HashSet<String>,
    global_names: &std::collections::HashSet<String>,
    nonlocal_names: &std::collections::HashSet<String>,
) {
    match expr {
        Expr::Named { target, value } => {
            if !global_names.contains(target) && !nonlocal_names.contains(target) {
                names.insert(target.clone());
            }
            collect_walrus_targets_in_expr(value, names, global_names, nonlocal_names);
        }
        Expr::Binary { left, right, .. } => {
            collect_walrus_targets_in_expr(left, names, global_names, nonlocal_names);
            collect_walrus_targets_in_expr(right, names, global_names, nonlocal_names);
        }
        Expr::Unary { expr: e, .. } => {
            collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
        }
        Expr::Compare { left, ops } => {
            collect_walrus_targets_in_expr(left, names, global_names, nonlocal_names);
            for (_, e) in ops {
                collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
            }
        }
        Expr::Call { func, args } => {
            collect_walrus_targets_in_expr(func, names, global_names, nonlocal_names);
            for a in args {
                collect_walrus_targets_in_expr(&a.value, names, global_names, nonlocal_names);
            }
        }
        Expr::Ternary { cond, then, else_ } => {
            collect_walrus_targets_in_expr(cond, names, global_names, nonlocal_names);
            collect_walrus_targets_in_expr(then, names, global_names, nonlocal_names);
            collect_walrus_targets_in_expr(else_, names, global_names, nonlocal_names);
        }
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            for e in items {
                collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
            }
        }
        Expr::Starred(inner) => {
            collect_walrus_targets_in_expr(inner, names, global_names, nonlocal_names);
        }
        Expr::Dict(items) => {
            for item in items {
                match item {
                    crate::ast::DictItem::Pair(k, v) => {
                        collect_walrus_targets_in_expr(k, names, global_names, nonlocal_names);
                        collect_walrus_targets_in_expr(v, names, global_names, nonlocal_names);
                    }
                    crate::ast::DictItem::DoubleSplat(e) => {
                        collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
                    }
                }
            }
        }
        Expr::Index { target, index } => {
            collect_walrus_targets_in_expr(target, names, global_names, nonlocal_names);
            collect_walrus_targets_in_expr(index, names, global_names, nonlocal_names);
        }
        Expr::Attr { target, .. } => {
            collect_walrus_targets_in_expr(target, names, global_names, nonlocal_names);
        }
        Expr::Slice { target, lower, upper, step } => {
            collect_walrus_targets_in_expr(target, names, global_names, nonlocal_names);
            for e in [lower, upper, step].iter().flat_map(|o| o.as_deref()) {
                collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
            }
        }
        _ => {}
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
        AssignTarget::Starred(inner) => {
            collect_assign_target_names(inner, names, global_names, nonlocal_names);
        }
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
        if let Some(&idx) = local_index.get(&param.name)
            && idx < 64 {
                mask |= 1u64 << idx;
            }
    }
    mask
}

pub(crate) fn float_to_bigint(f: f64) -> Value {
    use crate::value::PyBigInt;
    // Convert via the decimal string representation of the f64's integer value.
    let s = format!("{:.0}", f);
    let n: PyBigInt = s.parse().unwrap_or_else(|_| PyBigInt::from(0i64));
    Value::bigint(n)
}

pub(crate) fn value_to_float(v: &Value, ctx: &str) -> Result<f64> {
    match v.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(i) => Ok(i as f64),
        ValueKind::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
        _ => Err(PyError::named("TypeError", format!(
            "{ctx}: a float is required, not {}",
            v.repr()
        ))),
    }
}

// `make_math_module()` / `make_sys_module()` removed — both are now
// generated by the `pyrust_module!` macro inside
// `crates/pyrust/src/builtin_modules/{math,sys}.rs`.  See
// `docs/builtin-migration.md` for the recipe.

// Names of built-in callables with observable side effects.
const IMPURE_BUILTINS: &[&str] = &["print", "input", "open", "exit", "quit", "exec", "eval"];

// Built-in callables that are definitionally pure (no side effects, deterministic).
const PURE_BUILTINS: &[&str] = &[
    "abs", "all", "any", "bin", "bool", "chr", "dict", "divmod", "enumerate", "float",
    "hash", "hex", "id", "int", "isinstance", "issubclass", "iter", "len", "list", "max",
    "min", "oct", "ord", "pow", "range", "repr", "reversed", "round", "set", "sorted",
    "str", "sum", "tuple", "type", "zip",
];

/// Returns true if `expr` produces no observable side effects given the set of
/// locally-defined functions already confirmed pure (`pure_fns`).
///
/// A `Call` is pure only when the callee is a known-pure builtin or a
/// locally-defined function in `pure_fns`.  Indirect calls (methods, closures
/// through computed expressions) and calls to names not in either set are
/// conservatively treated as impure.
fn is_pure_expr(expr: &Expr, pure_fns: &std::collections::HashSet<String>) -> bool {
    match expr {
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bytes(_) | Expr::Bool(_) | Expr::None => true,
        Expr::Int(_) | Expr::Float(_) | Expr::Complex(_, _) | Expr::Str(_) | Expr::Bool(_) | Expr::None => true,
        Expr::Var(_) => true,
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            items.iter().all(|e| is_pure_expr(e, pure_fns))
        }
        Expr::Starred(inner) => is_pure_expr(inner, pure_fns),
        Expr::Dict(items) => items.iter().all(|item| match item {
            crate::ast::DictItem::Pair(k, v) => {
                is_pure_expr(k, pure_fns) && is_pure_expr(v, pure_fns)
            }
            crate::ast::DictItem::DoubleSplat(e) => is_pure_expr(e, pure_fns),
        }),
        Expr::Unary { expr, .. } => is_pure_expr(expr, pure_fns),
        Expr::Binary { left, right, .. } => {
            is_pure_expr(left, pure_fns) && is_pure_expr(right, pure_fns)
        }
        Expr::Compare { left, ops } => {
            is_pure_expr(left, pure_fns)
                && ops.iter().all(|(_, e)| is_pure_expr(e, pure_fns))
        }
        Expr::Ternary { cond, then, else_ } => {
            is_pure_expr(cond, pure_fns)
                && is_pure_expr(then, pure_fns)
                && is_pure_expr(else_, pure_fns)
        }
        Expr::Lambda { .. } => true,
        Expr::Call { func, args } => {
            // Only direct calls to named callees can be pure.
            if let Expr::Var(name) = func.as_ref() {
                // Known-impure builtins (I/O, process control) → always impure.
                if IMPURE_BUILTINS.contains(&name.as_str()) {
                    return false;
                }
                // Accept known-pure builtins or locally-defined pure functions.
                let callee_is_pure = PURE_BUILTINS.contains(&name.as_str())
                    || pure_fns.contains(name.as_str());
                if !callee_is_pure {
                    return false;
                }
            } else {
                // Indirect call (method, computed callee) — conservatively impure.
                return false;
            }
            args.iter().all(|a| is_pure_expr(&a.value, pure_fns))
        }
        Expr::Attr { target, .. } => is_pure_expr(target, pure_fns),
        Expr::Index { target, index } => {
            is_pure_expr(target, pure_fns) && is_pure_expr(index, pure_fns)
        }
        Expr::Slice { target, lower, upper, step } => {
            is_pure_expr(target, pure_fns)
                && lower.as_deref().is_none_or(|e| is_pure_expr(e, pure_fns))
                && upper.as_deref().is_none_or(|e| is_pure_expr(e, pure_fns))
                && step.as_deref().is_none_or(|e| is_pure_expr(e, pure_fns))
        }
        // Comprehensions involve iteration (GetIter, ForIter) which may call
        // __iter__/__next__ — conservatively treat as impure.
        Expr::ListComp { .. } | Expr::DictComp { .. } | Expr::SetComp { .. } => false,
        // Walrus has a side effect (assignment).
        Expr::Named { .. } => false,
        Expr::FString(parts) => {
            use crate::ast::FStringPart;
            parts.iter().all(|p| match p {
                FStringPart::Literal(_) => true,
                FStringPart::Expr { expr, .. } => is_pure_expr(expr, pure_fns),
            })
        }
        // yield/yield from always have side effects (generator suspension).
        Expr::Yield(_) | Expr::YieldFrom(_) => false,
    }
}

/// Returns true if every statement in `body` is free of observable side effects.
///
/// `pure_fns` is the set of locally-defined functions already confirmed pure;
/// calls to names outside this set and outside `PURE_BUILTINS` are treated as
/// impure.  Attribute/index mutation, global/nonlocal declarations, imports,
/// and `with` blocks are always impure.
pub(crate) fn is_pure_body(
    body: &[Stmt],
    pure_fns: &std::collections::HashSet<String>,
) -> bool {
    body.iter().all(|s| is_pure_stmt(s, pure_fns))
}

fn is_pure_stmt(stmt: &Stmt, pure_fns: &std::collections::HashSet<String>) -> bool {
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
        Stmt::Assign(_, expr) | Stmt::Expr(expr) => is_pure_expr(expr, pure_fns),
        Stmt::AugAssign { expr, .. } => is_pure_expr(expr, pure_fns),
        Stmt::Return(Some(expr)) => is_pure_expr(expr, pure_fns),
        Stmt::Return(None) => true,
        Stmt::Assert { test, msg } => {
            is_pure_expr(test, pure_fns)
                && msg.as_ref().is_none_or(|e| is_pure_expr(e, pure_fns))
        }
        Stmt::Raise { expr, cause } => {
            expr.as_ref().is_none_or(|e| is_pure_expr(e, pure_fns))
                && cause.as_ref().is_none_or(|e| is_pure_expr(e, pure_fns))
        }

        // Control flow: recurse into sub-blocks.
        Stmt::If { branches, else_branch } => {
            branches
                .iter()
                .all(|(cond, blk)| is_pure_expr(cond, pure_fns) && is_pure_body(blk, pure_fns))
                && else_branch
                    .as_deref()
                    .is_none_or(|b| is_pure_body(b, pure_fns))
        }
        Stmt::While { cond, body, else_branch } => {
            is_pure_expr(cond, pure_fns)
                && is_pure_body(body, pure_fns)
                && else_branch
                    .as_deref()
                    .is_none_or(|b| is_pure_body(b, pure_fns))
        }
        Stmt::For { iter, body, else_branch, .. } => {
            is_pure_expr(iter, pure_fns)
                && is_pure_body(body, pure_fns)
                && else_branch
                    .as_deref()
                    .is_none_or(|b| is_pure_body(b, pure_fns))
        }
        Stmt::Try { body, handlers, else_branch, finally_branch } => {
            is_pure_body(body, pure_fns)
                && handlers.iter().all(|h| is_pure_body(&h.body, pure_fns))
                && else_branch
                    .as_deref()
                    .is_none_or(|b| is_pure_body(b, pure_fns))
                && finally_branch
                    .as_deref()
                    .is_none_or(|b| is_pure_body(b, pure_fns))
        }

        // Nested definitions don't execute side effects at definition time.
        Stmt::Def { .. } | Stmt::Class { .. } => true,
        Stmt::Pass | Stmt::Break | Stmt::Continue => true,
        Stmt::Match { subject, arms } => {
            is_pure_expr(subject, pure_fns)
                && arms
                    .iter()
                    .all(|arm| is_pure_body(&arm.body, pure_fns))
        }
    }
}

/// Round a float to the nearest integer using banker's rounding (round half to even),
/// matching CPython's `round(x)` with no ndigits argument.
pub(crate) fn py_round_half_even(v: f64) -> i64 {
    let floor = v.floor();
    let diff = v - floor;
    if diff < 0.5 {
        floor as i64
    } else if diff > 0.5 {
        (floor + 1.0) as i64
    } else {
        // Exactly 0.5: round to even
        let floor_i = floor as i64;
        if floor_i % 2 == 0 {
            floor_i
        } else {
            floor_i + 1
        }
    }
}

/// Round a float to nearest using banker's rounding, returning f64.
/// Used by round(x, n) for float inputs.
pub(crate) fn py_round_half_even_f64(v: f64) -> f64 {
    let floor = v.floor();
    let diff = v - floor;
    if diff < 0.5 {
        floor
    } else if diff > 0.5 {
        floor + 1.0
    } else {
        // Exactly 0.5: round to even
        let floor_i = floor as i64;
        if floor_i % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    }
}

/// Modular exponentiation: (base^exp) % modulus for i64.
pub(crate) fn modpow_i64(base: i64, exp: u64, modulus: i64) -> i64 {
    if modulus == 1 {
        return 0;
    }
    let mut result: i64 = 1;
    let mut base = ((base % modulus) + modulus) % modulus;
    let mut exp = exp;
    while exp > 0 {
        if exp % 2 == 1 {
            result = (result * base) % modulus;
        }
        exp >>= 1;
        base = (base * base) % modulus;
    }
    result
}


