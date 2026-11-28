//! Interpreter plumbing for the native `statistics` module: calls, operators,
//! exception helpers and the module-level state (`StatisticsError`,
//! `NormalDist`, `LinearRegression`) that native code needs without a module
//! reference.

use std::cell::RefCell;
use std::rc::Rc;

use num_bigint::BigInt;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyClass, Value, ValueKind};

pub(super) type ClassRc = Rc<RefCell<PyClass>>;

// Per-interpreter state (`gil_local!`):
// (a) Why: native functions have no module reference, yet they raise
//     `StatisticsError`, build `NormalDist` results and `LinearRegression`
//     tuples.
// (b) Every generation is kept (a re-import after `del sys.modules[...]`
//     creates new classes); the newest one is used, like CPython's module
//     global lookup after a reload.
pyrust_core::gil_local! {
    static STATE: RefCell<Vec<ModuleClasses>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone)]
pub(super) struct ModuleClasses {
    pub(super) error: ClassRc,
    pub(super) normal_dist: ClassRc,
    pub(super) linear_regression: Value,
}

pub(super) fn register_classes(classes: ModuleClasses) {
    STATE.with(|state| state.borrow_mut().push(classes));
}

fn classes() -> Option<ModuleClasses> {
    STATE.with(|state| state.borrow().last().cloned())
}

/// `StatisticsError(msg)`.
pub(super) fn stats_error(msg: impl Into<String>) -> PyError {
    match classes() {
        Some(classes) => PyError::class(classes.error, msg),
        None => PyError::runtime("statistics is not initialised".to_string()),
    }
}

pub(super) fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

pub(super) fn value_error(msg: impl Into<String>) -> PyError {
    PyError::named("ValueError", msg.into())
}

/// The module's `NormalDist` class (the newest generation).
pub(super) fn normal_dist_class() -> Result<Value> {
    classes()
        .map(|classes| Value::py_class(classes.normal_dist))
        .ok_or_else(|| PyError::runtime("statistics is not initialised".to_string()))
}

/// The module's `LinearRegression` namedtuple class.
pub(super) fn linear_regression_class() -> Result<Value> {
    classes()
        .map(|classes| classes.linear_regression)
        .ok_or_else(|| PyError::runtime("statistics is not initialised".to_string()))
}

/// `isinstance(value, NormalDist)` for any generation of the class.
pub(super) fn is_normal_dist(value: &Value) -> bool {
    let ValueKind::PyInstance(instance) = value.kind() else {
        return false;
    };
    let class = Rc::clone(&instance.borrow().class);
    STATE.with(|state| {
        state
            .borrow()
            .iter()
            .any(|c| crate::interpreter::class_is_subclass_of(&class, &c.normal_dist))
    })
}

// ── Calls ─────────────────────────────────────────────────────────────────────

pub(super) fn call(interp: &mut Interpreter, callee: Value, pos: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

pub(super) fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    expanded.extend(kw.iter().map(|(name, value)| ExpandedCallArg {
        name: Some((*name).to_string()),
        value: value.clone(),
    }));
    interp.call_function_expanded(callee, &expanded)
}

pub(super) fn call_method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    pos: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call(interp, callee, pos)
}

/// `builtins.<name>(*pos)`.
pub(super) fn builtin(
    interp: &mut Interpreter,
    name: &'static str,
    pos: &[Value],
) -> Result<Value> {
    call(interp, Value::builtin_function(name), pos)
}

/// `<module>.<name>`.
pub(super) fn module_attr(interp: &mut Interpreter, module: &str, name: &str) -> Result<Value> {
    let module = interp.load_module(module)?;
    interp.get_attr(&module, name)
}

/// `<module>.<name>(*pos)`.
pub(super) fn module_call(
    interp: &mut Interpreter,
    module: &str,
    name: &str,
    pos: &[Value],
) -> Result<Value> {
    let callee = module_attr(interp, module, name)?;
    call(interp, callee, pos)
}

/// `math.<name>(*pos)`.
pub(super) fn math_call(interp: &mut Interpreter, name: &str, pos: &[Value]) -> Result<Value> {
    module_call(interp, "math", name, pos)
}

/// `math.<name>(*pos)` as a Rust float.
pub(super) fn math_f64(interp: &mut Interpreter, name: &str, pos: &[Value]) -> Result<f64> {
    let result = math_call(interp, name, pos)?;
    to_f64(interp, &result)
}

// ── Operators and conversions ────────────────────────────────────────────────

pub(super) fn binop(
    interp: &mut Interpreter,
    left: &Value,
    op: BinaryOp,
    right: &Value,
) -> Result<Value> {
    interp.eval_binary(left.clone(), op, right.clone())
}

pub(super) fn truthy(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    interp.truthy_value(value)
}

/// `left <op> right` as a bool (for the comparison operators).
pub(super) fn compare(
    interp: &mut Interpreter,
    left: &Value,
    op: BinaryOp,
    right: &Value,
) -> Result<bool> {
    let result = binop(interp, left, op, right)?;
    truthy(interp, &result)
}

/// `a is b`.
pub(super) fn is(a: &Value, b: &Value) -> bool {
    a.is_identical_to(b)
}

pub(super) fn int_value(n: BigInt) -> Value {
    pyrust_core::value_from_bigint(n)
}

/// The value of an exact `int` / `bool` as a `BigInt`.
pub(super) fn to_bigint(value: &Value) -> Option<BigInt> {
    match value.kind() {
        ValueKind::Int(n) => Some(BigInt::from(n)),
        ValueKind::BigInt(b) => Some(b.clone()),
        ValueKind::Bool(b) => Some(BigInt::from(u8::from(b))),
        _ => None,
    }
}

/// The value as a Rust float: an exact `float` directly, anything else
/// through `float(value)` (so `__float__` / `__index__` and errors match
/// CPython).
pub(super) fn to_f64(interp: &mut Interpreter, value: &Value) -> Result<f64> {
    if let ValueKind::Float(f) = value.kind() {
        return Ok(f);
    }
    let result = builtin(interp, "float", std::slice::from_ref(value))?;
    match result.kind() {
        ValueKind::Float(f) => Ok(f),
        _ => Err(type_error("float() did not return a float")),
    }
}

/// `list(iterable)`.
pub(super) fn items(interp: &mut Interpreter, iterable: &Value) -> Result<Vec<Value>> {
    interp.collect_iterable(iterable)
}

/// `len(value)` as a `usize`.
pub(super) fn length(interp: &mut Interpreter, value: &Value) -> Result<usize> {
    let n = builtin(interp, "len", std::slice::from_ref(value))?;
    n.as_int()
        .map(|n| n.max(0) as usize)
        .ok_or_else(|| type_error("len() did not return an integer"))
}

/// `repr(value)`.
pub(super) fn repr(interp: &mut Interpreter, value: &Value) -> Result<String> {
    let text = builtin(interp, "repr", std::slice::from_ref(value))?;
    Ok(text.as_str().unwrap_or_default().to_string())
}

/// `type(value).__name__`.
pub(super) fn type_name(value: &Value) -> String {
    crate::interpreter::value_type_name_str(value).into_owned()
}

/// Run `f`.  An error accepted by `swallow` is handed back as `Ok(Err(err))`
/// after the traceback frames recorded while it unwound are dropped, so the
/// swallowed exception (CPython's `except` clause) does not leak frames into a
/// later, unrelated exception.  Any other error propagates unchanged.
pub(super) fn try_swallow<T>(
    f: impl FnOnce() -> Result<T>,
    swallow: impl FnOnce(&PyError) -> bool,
) -> Result<std::result::Result<T, PyError>> {
    let saved = pyrust_core::clone_captured_error_frames();
    match f() {
        Ok(value) => Ok(Ok(value)),
        Err(err) if swallow(&err) => {
            pyrust_core::reset_captured_error_frames();
            for frame in saved.into_iter().rev() {
                pyrust_core::record_traceback_frame(frame);
            }
            Ok(Err(err))
        }
        Err(err) => Err(err),
    }
}
