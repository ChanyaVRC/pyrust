// `contextvars` module — 100% native Rust implementation of CPython 3.12.3's
// `_contextvars` C extension (PEP 567; issues #3184, #3461).  `Lib/contextvars.py`
// is only `from _contextvars import Context, ContextVar, Token, copy_context`,
// so the types report `__module__ == "_contextvars"` exactly like CPython.
//
//   * `ContextVar` / `Token` / `Context` keep their state in hidden native
//     slots (no `__dict__`, not subclassable, not picklable).  A `Context`
//     maps `ContextVar` *identity* to `(var, value)` in insertion order
//     (CPython's HAMT iterates in hash order, which no program can rely on).
//   * `Context.keys()` / `values()` / `items()` / `iter(ctx)` return the
//     one-shot snapshot iterators CPython's HAMT iterators are (`type(...)`
//     is `builtins.keys` etc., `len()` is the snapshot size).
//   * The *current* context is per OS thread.  It lives in a `gil_local!`
//     map keyed by `ThreadId`, so threads sharing one interpreter never
//     clobber each other and separate interpreters never share a context; a
//     finished worker thread's entry is pruned when the next thread registers.
//   * `Context` is not registered as a `collections.abc.Mapping` — that was
//     only added in CPython 3.13 (`isinstance(ctx, Mapping)` is `False` here).
//
// Reference: <https://docs.python.org/3.12/library/contextvars.html>

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Weak};
use std::thread::ThreadId;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::macros::native_state::{
    has_native_state, set_native_state, with_native_state,
};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, lookup_exc_class, render_value_repr};
use crate::value::{InstanceAttrs, PyClass, PyInstance, PyModule, Value, ValueKind};

const VAR_TP: &str = "_contextvars.ContextVar";
const TOKEN_TP: &str = "_contextvars.Token";
const CONTEXT_TP: &str = "_contextvars.Context";

/// `contextvars.__all__`.
const ALL_NAMES: [&str; 4] = ["Context", "ContextVar", "Token", "copy_context"];

// ── State ────────────────────────────────────────────────────────────────────

/// `ContextVar`: the name object and the optional constructor default.
struct VarState {
    name: Value,
    default: Option<Value>,
}

/// `Token`: the context and variable it was created for, the previous value
/// (`None` = `Token.MISSING`) and the "already used" flag.
struct TokenState {
    context: Value,
    var: Value,
    old_value: Option<Value>,
    used: Cell<bool>,
}

/// `Context`: `ContextVar` identity -> `(var, value)`, and the "entered" flag.
struct ContextState {
    data: RefCell<IndexMap<usize, (Value, Value)>>,
    entered: Cell<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewKind {
    Keys,
    Values,
    Items,
}

/// The snapshot iterator behind `keys()` / `values()` / `items()` / `iter()`.
struct ViewState {
    kind: ViewKind,
    entries: Vec<(Value, Value)>,
    position: Cell<usize>,
}

/// The classes of the newest imported `contextvars` module.
struct Registry {
    token: Rc<RefCell<PyClass>>,
    context: Rc<RefCell<PyClass>>,
    keys: Rc<RefCell<PyClass>>,
    values: Rc<RefCell<PyClass>>,
    items: Rc<RefCell<PyClass>>,
    missing: Value,
}

pyrust_core::gil_local! {
    /// Classes and the `Token.MISSING` singleton of the newest prepared module.
    static REGISTRY: RefCell<Option<Rc<Registry>>> = RefCell::new(None);
    /// The current `Context` of every OS thread that has touched one.
    static CURRENT: RefCell<HashMap<ThreadId, (Weak<()>, Value)>> = RefCell::new(HashMap::new());
}

fn registry() -> Result<Rc<Registry>> {
    REGISTRY
        .with(|slot| slot.borrow().clone())
        .ok_or_else(|| PyError::runtime("contextvars: module classes not prepared".into()))
}

thread_local! {
    /// Liveness token of this OS thread: its TLS destructor drops the `Arc`,
    /// which is how `CURRENT` notices the thread has finished.
    static THREAD_ALIVE: Arc<()> = Arc::new(());
}

/// Entries of finished OS threads, dropped (GIL held) whenever a new thread
/// registers its first current context.
fn prune_finished_threads(map: &mut HashMap<ThreadId, (Weak<()>, Value)>) -> Vec<Value> {
    let dead: Vec<ThreadId> = map
        .iter()
        .filter(|(_, (alive, _))| alive.strong_count() == 0)
        .map(|(id, _)| *id)
        .collect();
    dead.into_iter()
        .filter_map(|id| map.remove(&id).map(|(_, value)| value))
        .collect()
}

// ── Error helpers ────────────────────────────────────────────────────────────

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn runtime_error(msg: impl Into<String>) -> PyError {
    PyError::named("RuntimeError", msg.into())
}

fn value_error(msg: impl Into<String>) -> PyError {
    PyError::named("ValueError", msg.into())
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

/// `LookupError(var)`.
fn lookup_error(interp: &mut Interpreter, var: &Value) -> PyError {
    let Some(class) = lookup_exc_class("LookupError") else {
        return PyError::named("LookupError", String::new());
    };
    match call_pos(interp, Value::py_class(class), std::slice::from_ref(var)) {
        Ok(instance) => PyError::raised(instance),
        Err(err) => err,
    }
}

fn repr_of(interp: &mut Interpreter, value: &Value) -> Result<String> {
    render_value_repr(interp, value)
}

fn address_of(value: &Value) -> usize {
    match value.object_id().kind() {
        ValueKind::Int(id) => id as usize,
        _ => 0,
    }
}

fn has_keywords(args: &[ExpandedCallArg]) -> bool {
    args.iter().any(|arg| arg.name.is_some())
}

/// `METH_NOARGS`: `what` is the qualified method name (`Context.copy`).
fn noargs(what: &str, rest: &[ExpandedCallArg]) -> Result<()> {
    if has_keywords(rest) {
        return Err(type_error(format!("{what}() takes no keyword arguments")));
    }
    if !rest.is_empty() {
        return Err(type_error(format!(
            "{what}() takes no arguments ({} given)",
            rest.len()
        )));
    }
    Ok(())
}

/// `METH_O`: exactly one positional argument.
fn one_arg<'a>(what: &str, rest: &'a [ExpandedCallArg]) -> Result<&'a Value> {
    if has_keywords(rest) {
        return Err(type_error(format!("{what}() takes no keyword arguments")));
    }
    match rest {
        [only] => Ok(&only.value),
        _ => Err(type_error(format!(
            "{what}() takes exactly one argument ({} given)",
            rest.len()
        ))),
    }
}

/// Slot-wrapper (`__len__`, `__getitem__`, ...) arity: no arguments.
fn slot_noargs(rest: &[ExpandedCallArg]) -> Result<()> {
    if rest.is_empty() {
        return Ok(());
    }
    Err(type_error(format!(
        "expected 0 arguments, got {}",
        rest.len()
    )))
}

/// Slot-wrapper arity: exactly one argument.
fn slot_one(rest: &[ExpandedCallArg]) -> Result<&Value> {
    match rest {
        [only] => Ok(&only.value),
        _ => Err(type_error(format!(
            "expected 1 argument, got {}",
            rest.len()
        ))),
    }
}

/// The receiver of a method of the class whose native state is `T`.
fn receiver<'a, T: 'static>(
    method: &str,
    tp_name: &str,
    args: &'a [ExpandedCallArg],
) -> Result<(&'a Value, &'a [ExpandedCallArg])> {
    let Some((first, rest)) = args.split_first() else {
        let short = tp_name.rsplit('.').next().unwrap_or(tp_name);
        return Err(type_error(format!(
            "unbound method {short}.{method}() needs an argument"
        )));
    };
    if matches!(first.value.kind(), ValueKind::PyInstance(_)) && has_native_state::<T>(&first.value)
    {
        return Ok((&first.value, rest));
    }
    Err(type_error(format!(
        "descriptor '{method}' for '{tp_name}' objects doesn't apply to a '{}' object",
        pyrust_core::error_type_name(&first.value)
    )))
}

fn var_receiver<'a>(
    method: &str,
    args: &'a [ExpandedCallArg],
) -> Result<(&'a Value, &'a [ExpandedCallArg])> {
    receiver::<VarState>(method, VAR_TP, args)
}

fn token_receiver<'a>(
    method: &str,
    args: &'a [ExpandedCallArg],
) -> Result<(&'a Value, &'a [ExpandedCallArg])> {
    receiver::<TokenState>(method, TOKEN_TP, args)
}

fn context_receiver<'a>(
    method: &str,
    args: &'a [ExpandedCallArg],
) -> Result<(&'a Value, &'a [ExpandedCallArg])> {
    receiver::<ContextState>(method, CONTEXT_TP, args)
}

// ── Instances ────────────────────────────────────────────────────────────────

fn new_instance<T: 'static>(class: &Rc<RefCell<PyClass>>, state: T) -> Result<Value> {
    let instance = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(class),
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(&instance, state)?;
    Ok(instance)
}

fn new_context_in(
    class: &Rc<RefCell<PyClass>>,
    data: IndexMap<usize, (Value, Value)>,
) -> Result<Value> {
    new_instance(
        class,
        ContextState {
            data: RefCell::new(data),
            entered: Cell::new(false),
        },
    )
}

fn new_context(data: IndexMap<usize, (Value, Value)>) -> Result<Value> {
    new_context_in(&registry()?.context, data)
}

/// The identity key of a `ContextVar` instance (its address; the `Context`
/// holds the variable, so the address cannot be reused while it is a key).
fn identity(var: &Value) -> usize {
    match var.kind() {
        ValueKind::PyInstance(rc) => Rc::as_ptr(rc) as *const () as usize,
        _ => 0,
    }
}

fn is_var(value: &Value) -> bool {
    matches!(value.kind(), ValueKind::PyInstance(_)) && has_native_state::<VarState>(value)
}

fn is_context(value: &Value) -> bool {
    matches!(value.kind(), ValueKind::PyInstance(_)) && has_native_state::<ContextState>(value)
}

/// `ContextVar` key check of the `Context` mapping methods.
fn check_var_key(interp: &mut Interpreter, key: &Value) -> Result<()> {
    if is_var(key) {
        return Ok(());
    }
    let shown = repr_of(interp, key)?;
    Err(type_error(format!(
        "a ContextVar key was expected, got {shown}"
    )))
}

// ── The current context ──────────────────────────────────────────────────────

fn current_context() -> Result<Value> {
    let id = std::thread::current().id();
    if let Some(existing) = CURRENT.with(|map| map.borrow().get(&id).map(|(_, v)| v.clone())) {
        return Ok(existing);
    }
    let created = new_context(IndexMap::new())?;
    set_current_context(created.clone());
    Ok(created)
}

fn set_current_context(context: Value) {
    let id = std::thread::current().id();
    let (previous, pruned) = CURRENT.with(|map| {
        let mut map = map.borrow_mut();
        if let Some(entry) = map.get_mut(&id) {
            return (Some(std::mem::replace(&mut entry.1, context)), Vec::new());
        }
        let pruned = prune_finished_threads(&mut map);
        let alive = THREAD_ALIVE.with(Arc::downgrade);
        map.insert(id, (alive, context));
        (None, pruned)
    });
    drop(previous);
    drop(pruned);
}

fn context_lookup(context: &Value, key: usize) -> Result<Option<Value>> {
    with_native_state::<ContextState, _>(context, CONTEXT_TP, |state| {
        Ok(state
            .data
            .borrow()
            .get(&key)
            .map(|(_, value)| value.clone()))
    })
}

fn context_snapshot(context: &Value) -> Result<Vec<(usize, Value, Value)>> {
    with_native_state::<ContextState, _>(context, CONTEXT_TP, |state| {
        Ok(state
            .data
            .borrow()
            .iter()
            .map(|(key, (var, value))| (*key, var.clone(), value.clone()))
            .collect())
    })
}

fn new_view(kind: ViewKind, context: &Value) -> Result<Value> {
    let entries: Vec<(Value, Value)> = context_snapshot(context)?
        .into_iter()
        .map(|(_, var, value)| (var, value))
        .collect();
    let registry = registry()?;
    let class = match kind {
        ViewKind::Keys => &registry.keys,
        ViewKind::Values => &registry.values,
        ViewKind::Items => &registry.items,
    };
    new_instance(
        class,
        ViewState {
            kind,
            entries,
            position: Cell::new(0),
        },
    )
}

// ── ContextVar ───────────────────────────────────────────────────────────────

fn var_new(args: &[ExpandedCallArg]) -> Result<Value> {
    let Some((cls, rest)) = args.split_first() else {
        return Err(type_error("ContextVar.__new__(): not enough arguments"));
    };
    let ValueKind::PyClass(class) = cls.value.kind() else {
        return Err(type_error(format!(
            "ContextVar.__new__(X): X is not a type object ({})",
            pyrust_core::error_type_name(&cls.value)
        )));
    };
    let positional: Vec<&Value> = rest
        .iter()
        .filter(|arg| arg.name.is_none())
        .map(|arg| &arg.value)
        .collect();
    let keyword_count = rest.len() - positional.len();
    // `_PyArg_UnpackKeywords` order: too few positionals, too many arguments,
    // too many positionals, then unknown keywords.
    if positional.is_empty() {
        return Err(type_error(
            "ContextVar() takes exactly 1 positional argument (0 given)",
        ));
    }
    if keyword_count > 0 && rest.len() > 2 {
        return Err(type_error(format!(
            "ContextVar() takes at most 2 arguments ({} given)",
            rest.len()
        )));
    }
    if positional.len() > 1 {
        return Err(type_error(format!(
            "ContextVar() takes at most 1 positional argument ({} given)",
            positional.len()
        )));
    }
    let mut default = None;
    for arg in rest {
        match arg.name.as_deref() {
            Some("default") => default = Some(arg.value.clone()),
            Some(other) => {
                return Err(type_error(format!(
                    "'{other}' is an invalid keyword argument for ContextVar()"
                )));
            }
            None => {}
        }
    }
    let name = positional[0];
    if !is_str_like(name) {
        return Err(type_error("context variable name must be a str"));
    }
    new_instance(
        class,
        VarState {
            name: name.clone(),
            default,
        },
    )
}

fn var_get(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, rest) = var_receiver("get", args)?;
    if has_keywords(rest) {
        return Err(type_error("ContextVar.get() takes no keyword arguments"));
    }
    if rest.len() > 1 {
        return Err(type_error(format!(
            "get expected at most 1 argument, got {}",
            rest.len()
        )));
    }
    let context = current_context()?;
    if let Some(value) = context_lookup(&context, identity(this))? {
        return Ok(value);
    }
    if let Some(explicit) = rest.first() {
        return Ok(explicit.value.clone());
    }
    let default =
        with_native_state::<VarState, _>(this, VAR_TP, |state| Ok(state.default.clone()))?;
    match default {
        Some(value) => Ok(value),
        None => Err(lookup_error(interp, this)),
    }
}

fn var_set(args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, rest) = var_receiver("set", args)?;
    let value = one_arg("ContextVar.set", rest)?;
    let context = current_context()?;
    let key = identity(this);
    let old_value = with_native_state::<ContextState, _>(&context, CONTEXT_TP, |state| {
        Ok(state
            .data
            .borrow_mut()
            .insert(key, (this.clone(), value.clone()))
            .map(|(_, old)| old))
    })?;
    new_instance(
        &registry()?.token,
        TokenState {
            context,
            var: this.clone(),
            old_value,
            used: Cell::new(false),
        },
    )
}

fn var_reset(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, rest) = var_receiver("reset", args)?;
    let token = one_arg("ContextVar.reset", rest)?;
    if !(matches!(token.kind(), ValueKind::PyInstance(_)) && has_native_state::<TokenState>(token))
    {
        let shown = repr_of(interp, token)?;
        return Err(type_error(format!(
            "expected an instance of Token, got {shown}"
        )));
    }
    let context = current_context()?;
    let (used, token_var, token_context, old_value) =
        with_native_state::<TokenState, _>(token, TOKEN_TP, |state| {
            Ok((
                state.used.get(),
                state.var.clone(),
                state.context.clone(),
                state.old_value.clone(),
            ))
        })?;
    if used {
        let shown = repr_of(interp, token)?;
        return Err(runtime_error(format!("{shown} has already been used once")));
    }
    if identity(&token_var) != identity(this) {
        let shown = repr_of(interp, token)?;
        return Err(value_error(format!(
            "{shown} was created by a different ContextVar"
        )));
    }
    if !token_context.is_identical_to(&context) {
        let shown = repr_of(interp, token)?;
        return Err(value_error(format!(
            "{shown} was created in a different Context"
        )));
    }
    with_native_state::<TokenState, _>(token, TOKEN_TP, |state| {
        state.used.set(true);
        Ok(())
    })?;
    let key = identity(this);
    let removed = with_native_state::<ContextState, _>(&context, CONTEXT_TP, |state| {
        let mut data = state.data.borrow_mut();
        Ok(match old_value {
            Some(old) => data.insert(key, (this.clone(), old)),
            None => data.shift_remove(&key),
        })
    })?;
    drop(removed);
    Ok(Value::none())
}

fn var_repr(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, rest) = var_receiver("__repr__", args)?;
    slot_noargs(rest)?;
    let (name, default) = with_native_state::<VarState, _>(this, VAR_TP, |state| {
        Ok((state.name.clone(), state.default.clone()))
    })?;
    let mut out = format!("<ContextVar name={}", repr_of(interp, &name)?);
    if let Some(default) = default {
        out.push_str(&format!(" default={}", repr_of(interp, &default)?));
    }
    out.push_str(&format!(" at 0x{:x}>", address_of(this)));
    Ok(Value::string(out))
}

// ── Token ────────────────────────────────────────────────────────────────────

fn token_repr(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, rest) = token_receiver("__repr__", args)?;
    slot_noargs(rest)?;
    let (used, var) = with_native_state::<TokenState, _>(this, TOKEN_TP, |state| {
        Ok((state.used.get(), state.var.clone()))
    })?;
    let mut out = String::from("<Token");
    if used {
        out.push_str(" used");
    }
    out.push_str(&format!(
        " var={} at 0x{:x}>",
        repr_of(interp, &var)?,
        address_of(this)
    ));
    Ok(Value::string(out))
}

// ── Context ──────────────────────────────────────────────────────────────────

fn context_new(args: &[ExpandedCallArg]) -> Result<Value> {
    let Some((cls, rest)) = args.split_first() else {
        return Err(type_error("Context.__new__(): not enough arguments"));
    };
    let ValueKind::PyClass(class) = cls.value.kind() else {
        return Err(type_error(format!(
            "Context.__new__(X): X is not a type object ({})",
            pyrust_core::error_type_name(&cls.value)
        )));
    };
    if !rest.is_empty() {
        return Err(type_error("Context() does not accept any arguments"));
    }
    new_context_in(class, IndexMap::new())
}

fn context_getitem(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, rest) = context_receiver("__getitem__", args)?;
    let key = slot_one(rest)?;
    check_var_key(interp, key)?;
    match context_lookup(this, identity(key))? {
        Some(value) => Ok(value),
        None => Err(PyError::key_error(key.clone())),
    }
}

fn context_contains(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, rest) = context_receiver("__contains__", args)?;
    let key = slot_one(rest)?;
    check_var_key(interp, key)?;
    Ok(Value::bool_(context_lookup(this, identity(key))?.is_some()))
}

fn context_get(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, rest) = context_receiver("get", args)?;
    if has_keywords(rest) {
        return Err(type_error("Context.get() takes no keyword arguments"));
    }
    if rest.is_empty() {
        return Err(type_error("get expected at least 1 argument, got 0"));
    }
    if rest.len() > 2 {
        return Err(type_error(format!(
            "get expected at most 2 arguments, got {}",
            rest.len()
        )));
    }
    let key = &rest[0].value;
    check_var_key(interp, key)?;
    match context_lookup(this, identity(key))? {
        Some(value) => Ok(value),
        None => Ok(rest
            .get(1)
            .map_or_else(Value::none, |arg| arg.value.clone())),
    }
}

fn context_len(args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, rest) = context_receiver("__len__", args)?;
    slot_noargs(rest)?;
    let len = with_native_state::<ContextState, _>(this, CONTEXT_TP, |state| {
        Ok(state.data.borrow().len())
    })?;
    Ok(Value::int(len as i64))
}

fn context_view(method: &str, kind: ViewKind, args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, rest) = context_receiver(method, args)?;
    if method.starts_with("__") {
        slot_noargs(rest)?;
    } else {
        noargs(&format!("Context.{method}"), rest)?;
    }
    new_view(kind, this)
}

fn context_copy(args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, rest) = context_receiver("copy", args)?;
    noargs("Context.copy", rest)?;
    let data = with_native_state::<ContextState, _>(this, CONTEXT_TP, |state| {
        Ok(state.data.borrow().clone())
    })?;
    new_context(data)
}

/// `Context.__eq__` / `__ne__` (`hamt_eq`: same size and, for every variable,
/// an identical-or-equal value).
fn context_compare(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    negate: bool,
) -> Result<Value> {
    let what = if negate { "__ne__" } else { "__eq__" };
    let (this, rest) = context_receiver(what, args)?;
    let other = slot_one(rest)?;
    if !is_context(other) {
        return Ok(Value::not_implemented());
    }
    let left = context_snapshot(this)?;
    let right: HashMap<usize, Value> = context_snapshot(other)?
        .into_iter()
        .map(|(key, _, value)| (key, value))
        .collect();
    let mut equal = left.len() == right.len();
    if equal {
        for (key, _, value) in &left {
            let Some(other_value) = right.get(key) else {
                equal = false;
                break;
            };
            if value.is_identical_to(other_value) {
                continue;
            }
            let result = interp.eval_binary(value.clone(), BinaryOp::Eq, other_value.clone())?;
            if !interp.truthy_value(&result)? {
                equal = false;
                break;
            }
        }
    }
    Ok(Value::bool_(equal != negate))
}

/// `Context.run(callable, /, *args, **kwargs)`.
fn context_run(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, rest) = context_receiver("run", args)?;
    let Some((callable, call_args)) = rest.split_first().filter(|(first, _)| first.name.is_none())
    else {
        return Err(type_error("run() missing 1 required positional argument"));
    };
    // `PyContext_Enter`.
    let was_entered = with_native_state::<ContextState, _>(this, CONTEXT_TP, |state| {
        Ok(state.entered.replace(true))
    })?;
    if was_entered {
        let shown = repr_of(interp, this)?;
        return Err(runtime_error(format!(
            "cannot enter context: {shown} is already entered"
        )));
    }
    let previous = match current_context() {
        Ok(previous) => previous,
        Err(err) => {
            leave_entered_flag(this);
            return Err(err);
        }
    };
    set_current_context(this.clone());
    let result = interp.call_function_expanded(callable.value.clone(), call_args);
    // `PyContext_Exit`, also on the error path.
    set_current_context(previous);
    leave_entered_flag(this);
    result
}

fn leave_entered_flag(context: &Value) {
    let _ = with_native_state::<ContextState, _>(context, CONTEXT_TP, |state| {
        state.entered.set(false);
        Ok(())
    });
}

fn not_writable(attr: &str) -> PyError {
    PyError::named(
        "AttributeError",
        format!("attribute '{attr}' of '{TOKEN_TP}' objects is not writable"),
    )
}

fn cannot_pickle(tp_name: &str) -> PyError {
    type_error(format!("cannot pickle '{tp_name}' object"))
}

// ── Views ────────────────────────────────────────────────────────────────────

fn view_receiver<'a>(
    method: &str,
    args: &'a [ExpandedCallArg],
) -> Result<(&'a Value, &'a [ExpandedCallArg])> {
    let Some((first, rest)) = args.split_first() else {
        return Err(type_error(format!(
            "descriptor '{method}' needs an argument"
        )));
    };
    if matches!(first.value.kind(), ValueKind::PyInstance(_))
        && has_native_state::<ViewState>(&first.value)
    {
        return Ok((&first.value, rest));
    }
    Err(type_error(format!(
        "descriptor '{method}' doesn't apply to a '{}' object",
        pyrust_core::error_type_name(&first.value)
    )))
}

fn view_next(args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, rest) = view_receiver("__next__", args)?;
    slot_noargs(rest)?;
    let item = with_native_state::<ViewState, _>(this, "keys", |state| {
        let index = state.position.get();
        let Some((var, value)) = state.entries.get(index) else {
            return Ok(None);
        };
        state.position.set(index + 1);
        Ok(Some(match state.kind {
            ViewKind::Keys => var.clone(),
            ViewKind::Values => value.clone(),
            ViewKind::Items => Value::tuple(vec![var.clone(), value.clone()]),
        }))
    })?;
    item.ok_or_else(|| PyError::named("StopIteration", String::new()))
}

fn view_len(args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, rest) = view_receiver("__len__", args)?;
    slot_noargs(rest)?;
    let len = with_native_state::<ViewState, _>(this, "keys", |state| Ok(state.entries.len()))?;
    Ok(Value::int(len as i64))
}

pyrust_module! {
    class ContextVar {
        fn __new__(args) -> Result<Value> {
            var_new(args)
        }

        #[getter("name")]
        fn get_name(args) -> Result<Value> {
            let (this, _rest) = var_receiver("name", args)?;
            with_native_state::<VarState, _>(this, VAR_TP, |state| Ok(state.name.clone()))
        }

        #[setter("name")]
        fn set_name(_args) -> Result<Value> {
            Err(PyError::named("AttributeError", "readonly attribute".to_string()))
        }

        fn get(args) -> Result<Value> {
            var_get(_interp, args)
        }

        fn set(args) -> Result<Value> {
            var_set(args)
        }

        fn reset(args) -> Result<Value> {
            var_reset(_interp, args)
        }

        fn __repr__(args) -> Result<Value> {
            var_repr(_interp, args)
        }

        fn __reduce__(_args) -> Result<Value> {
            Err(cannot_pickle(VAR_TP))
        }

        fn __reduce_ex__(_args) -> Result<Value> {
            Err(cannot_pickle(VAR_TP))
        }

        #[classmethod]
        fn __class_getitem__(args) -> Result<Value> {
            class_getitem(args)
        }
    }

    class Token {
        constants {
            "__hash__" => Value::none(),
        }

        fn __new__(_args) -> Result<Value> {
            Err(runtime_error("Tokens can only be created by ContextVars"))
        }

        #[getter("var")]
        fn get_var(args) -> Result<Value> {
            let (this, _rest) = token_receiver("var", args)?;
            with_native_state::<TokenState, _>(this, TOKEN_TP, |state| Ok(state.var.clone()))
        }

        #[setter("var")]
        fn set_var(_args) -> Result<Value> {
            Err(not_writable("var"))
        }

        #[setter("old_value")]
        fn set_old_value(_args) -> Result<Value> {
            Err(not_writable("old_value"))
        }

        #[getter("old_value")]
        fn get_old_value(args) -> Result<Value> {
            let (this, _rest) = token_receiver("old_value", args)?;
            let old = with_native_state::<TokenState, _>(this, TOKEN_TP, |state| {
                Ok(state.old_value.clone())
            })?;
            match old {
                Some(value) => Ok(value),
                None => Ok(registry()?.missing.clone()),
            }
        }

        fn __repr__(args) -> Result<Value> {
            token_repr(_interp, args)
        }

        fn __reduce__(_args) -> Result<Value> {
            Err(cannot_pickle(TOKEN_TP))
        }

        fn __reduce_ex__(_args) -> Result<Value> {
            Err(cannot_pickle(TOKEN_TP))
        }

        #[classmethod]
        fn __class_getitem__(args) -> Result<Value> {
            class_getitem(args)
        }
    }

    class Context {
        fn __new__(args) -> Result<Value> {
            context_new(args)
        }

        fn __getitem__(args) -> Result<Value> {
            context_getitem(_interp, args)
        }

        fn __contains__(args) -> Result<Value> {
            context_contains(_interp, args)
        }

        fn __len__(args) -> Result<Value> {
            context_len(args)
        }

        fn __iter__(args) -> Result<Value> {
            context_view("__iter__", ViewKind::Keys, args)
        }

        fn get(args) -> Result<Value> {
            context_get(_interp, args)
        }

        fn keys(args) -> Result<Value> {
            context_view("keys", ViewKind::Keys, args)
        }

        fn values(args) -> Result<Value> {
            context_view("values", ViewKind::Values, args)
        }

        fn items(args) -> Result<Value> {
            context_view("items", ViewKind::Items, args)
        }

        fn copy(args) -> Result<Value> {
            context_copy(args)
        }

        fn run(args) -> Result<Value> {
            context_run(_interp, args)
        }

        fn __eq__(args) -> Result<Value> {
            context_compare(_interp, args, false)
        }

        fn __ne__(args) -> Result<Value> {
            context_compare(_interp, args, true)
        }

        fn __reduce__(_args) -> Result<Value> {
            Err(cannot_pickle(CONTEXT_TP))
        }

        fn __reduce_ex__(_args) -> Result<Value> {
            Err(cannot_pickle(CONTEXT_TP))
        }

        constants {
            "__hash__" => Value::none(),
        }
    }

    /// The iterator type behind `Context.keys()` / `iter(Context)`.
    class keys {
        fn __iter__(args) -> Result<Value> {
            let (this, _rest) = view_receiver("__iter__", args)?;
            Ok(this.clone())
        }

        fn __next__(args) -> Result<Value> {
            view_next(args)
        }

        fn __len__(args) -> Result<Value> {
            view_len(args)
        }
    }

    /// The iterator type behind `Context.values()`.
    class values {
        fn __iter__(args) -> Result<Value> {
            let (this, _rest) = view_receiver("__iter__", args)?;
            Ok(this.clone())
        }

        fn __next__(args) -> Result<Value> {
            view_next(args)
        }

        fn __len__(args) -> Result<Value> {
            view_len(args)
        }
    }

    /// The iterator type behind `Context.items()`.
    class items {
        fn __iter__(args) -> Result<Value> {
            let (this, _rest) = view_receiver("__iter__", args)?;
            Ok(this.clone())
        }

        fn __next__(args) -> Result<Value> {
            view_next(args)
        }

        fn __len__(args) -> Result<Value> {
            view_len(args)
        }
    }

    /// The type of the `Token.MISSING` sentinel.
    class MISSING {
        fn __repr__(_args) -> Result<Value> {
            Ok(Value::string("<Token.MISSING>"))
        }
    }

    /// `copy_context()`: a shallow copy of the current context.
    fn copy_context(args) -> Result<Value> {
        if has_keywords(args) {
            return Err(type_error(
                "_contextvars.copy_context() takes no keyword arguments",
            ));
        }
        if !args.is_empty() {
            return Err(type_error(format!(
                "_contextvars.copy_context() takes no arguments ({} given)",
                args.len()
            )));
        }
        let current = current_context()?;
        let data = with_native_state::<ContextState, _>(&current, CONTEXT_TP, |state| {
            Ok(state.data.borrow().clone())
        })?;
        new_context(data)
    }
}

/// `ContextVar.__class_getitem__` / `Token.__class_getitem__`: `Py_GenericAlias`.
fn class_getitem(args: &[ExpandedCallArg]) -> Result<Value> {
    let (cls, rest) = args
        .split_first()
        .ok_or_else(|| type_error("__class_getitem__() takes exactly one argument (0 given)"))?;
    let class_name = match cls.value.kind() {
        ValueKind::PyClass(class) => class.borrow().name.clone(),
        _ => String::from("ContextVar"),
    };
    let item = one_arg(&format!("{class_name}.__class_getitem__"), rest)?;
    let type_arguments = if matches!(item.kind(), ValueKind::Tuple(_)) {
        item.clone()
    } else {
        Value::tuple(vec![item.clone()])
    };
    Ok(pyrust_builtins::generic_alias::generic_alias(
        cls.value.clone(),
        type_arguments,
    ))
}

/// Post-import wiring: give the classes their `_contextvars` identity (module,
/// error names, no `__dict__`, not subclassable), publish `Token.MISSING`,
/// hide the helper classes and put the namespace in CPython's order.
pub(crate) fn post_load_init(
    _interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let take = |name: &str| -> Result<Rc<RefCell<PyClass>>> {
        let value = module
            .borrow_mut()
            .attrs
            .shift_remove(name)
            .ok_or_else(|| PyError::runtime(format!("contextvars: missing {name}")))?;
        match value.kind() {
            ValueKind::PyClass(class) => Ok(Rc::clone(class)),
            _ => Err(PyError::runtime(format!(
                "contextvars: {name} is not a class"
            ))),
        }
    };
    let var = take("ContextVar")?;
    let token = take("Token")?;
    let context = take("Context")?;
    let keys = take("keys")?;
    let values = take("values")?;
    let items = take("items")?;
    let missing_class = take("MISSING")?;
    let copy_context = module
        .borrow_mut()
        .attrs
        .shift_remove("copy_context")
        .ok_or_else(|| PyError::runtime("contextvars: missing copy_context".into()))?;

    let finish = |class: &Rc<RefCell<PyClass>>,
                  module_name: &'static str,
                  error_name: &'static str,
                  weakref: bool| {
        let mut class = class.borrow_mut();
        class
            .attrs
            .insert("__module__".to_string(), Value::string(module_name));
        class.attrs.insert("__doc__".to_string(), Value::none());
        class.error_name = Some(error_name);
        class.non_subclassable_name = Some(error_name);
        class.slots = Some(if weakref {
            std::iter::once("__weakref__".to_string()).collect()
        } else {
            indexmap::IndexSet::new()
        });
    };
    finish(&var, "_contextvars", VAR_TP, false);
    finish(&token, "_contextvars", TOKEN_TP, false);
    finish(&context, "_contextvars", CONTEXT_TP, true);
    finish(&keys, "builtins", "keys", false);
    finish(&values, "builtins", "values", false);
    finish(&items, "builtins", "items", false);
    finish(&missing_class, "Token", "Token.MISSING", false);

    // Getset descriptors refuse deletion exactly like assignment.
    for (class, properties) in [(&var, &["name"][..]), (&token, &["var", "old_value"][..])] {
        let mut class = class.borrow_mut();
        for property in properties {
            let Some(fields) = class
                .attrs
                .get(*property)
                .and_then(pyrust_builtins::property::as_property)
            else {
                continue;
            };
            let readonly = pyrust_builtins::property::property_with_doc(
                (*fields.0).clone(),
                (*fields.1).clone(),
                (*fields.1).clone(),
                None,
            );
            class.attrs.insert((*property).to_string(), readonly);
        }
    }

    let missing = new_instance_unchecked(&missing_class);
    token
        .borrow_mut()
        .attrs
        .insert("MISSING".to_string(), missing.clone());

    REGISTRY.with(|slot| {
        *slot.borrow_mut() = Some(Rc::new(Registry {
            token: Rc::clone(&token),
            context: Rc::clone(&context),
            keys,
            values,
            items,
            missing,
        }));
    });

    let mut module = module.borrow_mut();
    module
        .attrs
        .insert("Context".to_string(), Value::py_class(context));
    module
        .attrs
        .insert("ContextVar".to_string(), Value::py_class(var));
    module
        .attrs
        .insert("Token".to_string(), Value::py_class(token));
    module
        .attrs
        .insert("copy_context".to_string(), copy_context);
    module.attrs.insert(
        "__all__".to_string(),
        Value::tuple(ALL_NAMES.iter().map(|name| Value::string(*name)).collect()),
    );
    Ok(())
}

/// `isinstance(value, str)`: a `str` or an instance of a `str` subclass.
fn is_str_like(value: &Value) -> bool {
    value.is_str()
        || (matches!(value.kind(), ValueKind::PyInstance(_))
            && crate::interpreter::builtin_data_backing(value)
                .is_some_and(|backing| backing.is_str()))
}

fn new_instance_unchecked(class: &Rc<RefCell<PyClass>>) -> Value {
    Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(class),
        attrs: InstanceAttrs::new(),
    })))
}
