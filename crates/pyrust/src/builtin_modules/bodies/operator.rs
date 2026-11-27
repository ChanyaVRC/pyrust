// `operator` module — 100% native Rust port of CPython 3.12's `operator`
// (`Lib/operator.py` + the `_operator` accelerator; issues #2514, #3364).
//
// The module exports the functions corresponding to the intrinsic operators
// (`add`, `sub`, `lt`, `getitem`, …), the in-place variants (`iadd`, …), the
// sequence helpers (`concat`, `countOf`, `indexOf`, …) and the generalized
// lookup objects (`attrgetter`, `itemgetter`, `methodcaller`).  There is no
// Python-source injection: every function routes through the interpreter's own
// operator machinery (`eval_binary`, `try_inplace_op`, the subscript opcodes,
// the iterator protocol), so BigInt promotion, user dunder dispatch and error
// wording stay identical to the corresponding expression.
//
// Behaviour pinned against CPython 3.12.3's `_operator` accelerator, which
// shadows `Lib/operator.py` at runtime:
//  * argument errors use the C wording (`add expected 2 arguments, got 1`,
//    `_operator.not_() takes exactly one argument (0 given)`,
//    `_operator.add() takes no keyword arguments`);
//  * `concat` / `iconcat` follow `PySequence_Concat` / `PySequence_InPlaceConcat`
//    (a type's own `sq_concat` wins; otherwise two sequences fall back to
//    `nb_add`; anything else is `'T' object can't be concatenated`);
//  * `countOf` / `indexOf` iterate lazily and report a non-iterable container
//    as `argument of type 'T' is not iterable`;
//  * `attrgetter` / `itemgetter` / `methodcaller` are final, slot-only types
//    with C-style constructors, `__repr__` and `__reduce__`.
//
// `index` (the `PyNumber_Index` intrinsic), the dunder aliases (`__add__` …,
// the very same function objects) and each function's `__module__ ==
// "_operator"` are wired in [`prepare_module`].
//
// Reference: <https://docs.python.org/3/library/operator.html>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;

use crate::ast::BinaryOp;
use crate::builtin_modules::macros::native_state::{set_native_state, with_native_state};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyValueRef;
use crate::interpreter::{
    ExpandedCallArg, Interpreter, RegSlice, error_type_name_str, extract_str_value, is_deque_value,
    is_str_or_str_subclass, lookup_value_special_method, render_value_repr, value_class,
};
use crate::value::{InstanceAttrs, PyInstance, Value, ValueKind};
use pyrust_derive::pyrust_module;

const OPERATOR_DOC: &str = "Operator interface.\n\nThis module exports a set of functions implemented in C corresponding\nto the intrinsic operators of Python.  For example, operator.add(x, y)\nis equivalent to the expression x+y.  The function names are those\nused for special methods; variants without leading and trailing\n'__' are also provided for convenience.";

const ATTRGETTER_DOC: &str = "Return a callable object that fetches the given attribute(s) from its operand.\nAfter f = attrgetter('name'), the call f(r) returns r.name.\nAfter g = attrgetter('name', 'date'), the call g(r) returns (r.name, r.date).\nAfter h = attrgetter('name.first', 'name.last'), the call h(r) returns\n(r.name.first, r.name.last).";

const ITEMGETTER_DOC: &str = "Return a callable object that fetches the given item(s) from its operand.\nAfter f = itemgetter(2), the call f(r) returns r[2].\nAfter g = itemgetter(2, 5, 3), the call g(r) returns (r[2], r[5], r[3])";

const METHODCALLER_DOC: &str = "Return a callable object that calls the given method on its operand.\nAfter f = methodcaller('name'), the call f(r) returns r.name().\nAfter g = methodcaller('name', 'date', foo=1), the call g(r) returns\nr.name('date', foo=1).";

/// `operator.__all__` exactly as CPython 3.12 spells it.
const OPERATOR_ALL: [&str; 55] = [
    "abs",
    "add",
    "and_",
    "attrgetter",
    "call",
    "concat",
    "contains",
    "countOf",
    "delitem",
    "eq",
    "floordiv",
    "ge",
    "getitem",
    "gt",
    "iadd",
    "iand",
    "iconcat",
    "ifloordiv",
    "ilshift",
    "imatmul",
    "imod",
    "imul",
    "index",
    "indexOf",
    "inv",
    "invert",
    "ior",
    "ipow",
    "irshift",
    "is_",
    "is_not",
    "isub",
    "itemgetter",
    "itruediv",
    "ixor",
    "le",
    "length_hint",
    "lshift",
    "lt",
    "matmul",
    "methodcaller",
    "mod",
    "mul",
    "ne",
    "neg",
    "not_",
    "or_",
    "pos",
    "pow",
    "rshift",
    "setitem",
    "sub",
    "truediv",
    "truth",
    "xor",
];

/// `(dunder, function)` aliases installed by `Lib/operator.py`; each dunder is
/// the very same function object as its plain name.
const DUNDER_ALIASES: [(&str, &str); 46] = [
    ("__lt__", "lt"),
    ("__le__", "le"),
    ("__eq__", "eq"),
    ("__ne__", "ne"),
    ("__ge__", "ge"),
    ("__gt__", "gt"),
    ("__not__", "not_"),
    ("__abs__", "abs"),
    ("__add__", "add"),
    ("__and__", "and_"),
    ("__call__", "call"),
    ("__floordiv__", "floordiv"),
    ("__index__", "index"),
    ("__inv__", "inv"),
    ("__invert__", "invert"),
    ("__lshift__", "lshift"),
    ("__mod__", "mod"),
    ("__mul__", "mul"),
    ("__matmul__", "matmul"),
    ("__neg__", "neg"),
    ("__or__", "or_"),
    ("__pos__", "pos"),
    ("__pow__", "pow"),
    ("__rshift__", "rshift"),
    ("__sub__", "sub"),
    ("__truediv__", "truediv"),
    ("__xor__", "xor"),
    ("__concat__", "concat"),
    ("__contains__", "contains"),
    ("__delitem__", "delitem"),
    ("__getitem__", "getitem"),
    ("__setitem__", "setitem"),
    ("__iadd__", "iadd"),
    ("__iand__", "iand"),
    ("__iconcat__", "iconcat"),
    ("__ifloordiv__", "ifloordiv"),
    ("__ilshift__", "ilshift"),
    ("__imod__", "imod"),
    ("__imul__", "imul"),
    ("__imatmul__", "imatmul"),
    ("__ior__", "ior"),
    ("__ipow__", "ipow"),
    ("__irshift__", "irshift"),
    ("__isub__", "isub"),
    ("__itruediv__", "itruediv"),
    ("__ixor__", "ixor"),
];

/// Module-load wiring (replaces the former `@inject` hook):
///  * `index` is the interpreter's `PyNumber_Index` intrinsic;
///  * the dunder aliases are the same function objects as their plain names;
///  * CPython's functions live in the `_operator` accelerator that
///    `operator.py` star-imports, so their `__module__` is `"_operator"`;
///  * the lookup types are final and report `operator.<name>` in diagnostics.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };
    mod_rc.borrow_mut().attrs.insert(
        "index".to_string(),
        pyrust_builtins::native_builtin_callable::native_intrinsic_builtin(
            pyrust_builtins::native_builtin_callable::NativeBuiltinIntrinsic::IndexProtocol,
            "index",
            "_operator",
            Some("Same as a.__index__()"),
        ),
    );
    for name in OPERATOR_ALL {
        if matches!(name, "attrgetter" | "itemgetter" | "methodcaller" | "index") {
            continue;
        }
        let func = mod_rc.borrow().attrs.get(name).cloned();
        if let Some(func) = func {
            // Best effort: a failure only leaves `__module__` as `"operator"`.
            let _ = interp.assign_attr(func, "__module__", Value::string("_operator"));
        }
    }
    for (dunder, name) in DUNDER_ALIASES {
        let func = mod_rc.borrow().attrs.get(name).cloned();
        if let Some(func) = func {
            mod_rc.borrow_mut().attrs.insert(dunder.to_string(), func);
        }
    }
    for (name, tp_name) in [
        ("attrgetter", "operator.attrgetter"),
        ("itemgetter", "operator.itemgetter"),
        ("methodcaller", "operator.methodcaller"),
    ] {
        let class = mod_rc.borrow().attrs.get(name).cloned();
        if let Some(class) = class
            && let ValueKind::PyClass(class_rc) = class.kind()
        {
            let mut class_mut = class_rc.borrow_mut();
            class_mut.non_subclassable_name = Some(tp_name);
            class_mut.error_name = Some(tp_name);
            // `__slots__ = ()`: no `__dict__`, so attribute writes are refused.
            class_mut.slots = Some(indexmap::IndexSet::new());
        }
    }
}

// ─── Argument parsing (C-level wording) ─────────────────────────────────────

fn type_error(msg: String) -> PyError {
    PyError::named("TypeError", msg)
}

/// `_PyArg_NoKeywords`-style rejection for the `_operator.NAME()` functions.
fn reject_keywords(name: &str, args: &[ExpandedCallArg]) -> Result<()> {
    if args.iter().any(|arg| arg.name.is_some()) {
        return Err(type_error(format!(
            "_operator.{name}() takes no keyword arguments"
        )));
    }
    Ok(())
}

/// `NAME expected N arguments, got M` for a fixed-arity (two/three argument)
/// accelerated function.
fn fixed_args<'a>(
    name: &str,
    args: &'a [ExpandedCallArg],
    count: usize,
) -> Result<&'a [ExpandedCallArg]> {
    reject_keywords(name, args)?;
    if args.len() != count {
        return Err(type_error(format!(
            "{name} expected {count} arguments, got {}",
            args.len()
        )));
    }
    Ok(args)
}

/// The METH_O wording: `_operator.NAME() takes exactly one argument (N given)`.
fn one_arg(name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    reject_keywords(name, args)?;
    if args.len() != 1 {
        return Err(type_error(format!(
            "_operator.{name}() takes exactly one argument ({} given)",
            args.len()
        )));
    }
    Ok(args[0].value.clone())
}

fn two_args(name: &str, args: &[ExpandedCallArg]) -> Result<(Value, Value)> {
    let args = fixed_args(name, args, 2)?;
    Ok((args[0].value.clone(), args[1].value.clone()))
}

fn binary(
    interp: &mut Interpreter,
    name: &str,
    args: &[ExpandedCallArg],
    op: BinaryOp,
) -> Result<Value> {
    let (a, b) = two_args(name, args)?;
    interp.eval_binary(a, op, b)
}

/// `a OP= b`: the in-place dunder first, then the plain binary operation with
/// the augmented-assignment diagnostics.
fn inplace_op(interp: &mut Interpreter, a: Value, op: BinaryOp, b: Value) -> Result<Value> {
    if let Some(result) = interp.try_inplace_op(&a, op, &b, true)? {
        return Ok(result);
    }
    interp.eval_binary_aug(a, op, b)
}

fn inplace(
    interp: &mut Interpreter,
    name: &str,
    args: &[ExpandedCallArg],
    op: BinaryOp,
) -> Result<Value> {
    let (a, b) = two_args(name, args)?;
    inplace_op(interp, a, op, b)
}

// ─── Subscript helpers (run through the VM item opcodes) ────────────────────

/// `obj[key]` with the VM's exact subscription semantics.
pub(crate) fn getitem_value(interp: &mut Interpreter, obj: Value, key: Value) -> Result<Value> {
    let mut scratch = vec![obj, key];
    // SAFETY: `scratch` outlives `regs`, is not otherwise borrowed, and the
    // pointer/length describe exactly its allocation.
    let regs = unsafe { RegSlice::from_raw(scratch.as_mut_ptr(), scratch.len()) };
    interp.exec_get_item(&regs, 0, 0, 1)
}

pub(crate) fn setitem_value(
    interp: &mut Interpreter,
    obj: Value,
    key: Value,
    value: Value,
) -> Result<()> {
    let mut scratch = vec![obj, key, value];
    // SAFETY: as in `getitem_value`.
    let mut regs = unsafe { RegSlice::from_raw(scratch.as_mut_ptr(), scratch.len()) };
    interp.exec_set_item(&mut regs, 0, 0, 1, 2)
}

pub(crate) fn delitem_value(interp: &mut Interpreter, obj: Value, key: Value) -> Result<()> {
    let mut scratch = vec![obj, key];
    // SAFETY: as in `getitem_value`.
    let mut regs = unsafe { RegSlice::from_raw(scratch.as_mut_ptr(), scratch.len()) };
    interp.exec_delete_item(&mut regs, 0, 0, 1)
}

// ─── Sequence concatenation (`PySequence_Concat` family) ────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum NativeSeq {
    List,
    Tuple,
    Str,
    Bytes,
    Bytearray,
    Deque,
}

impl NativeSeq {
    /// Whether the type also owns an `sq_inplace_concat` slot.
    fn has_inplace_concat(self) -> bool {
        matches!(self, Self::List | Self::Bytearray | Self::Deque)
    }

    fn name(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Tuple => "tuple",
            Self::Str => "str",
            Self::Bytes => "bytes",
            Self::Bytearray => "bytearray",
            Self::Deque => "deque",
        }
    }
}

/// The built-in sequence type whose C slots `value` inherits, if any.
///
/// A subclass that redefines `__add__` / `__iadd__` in Python replaces the
/// corresponding slot with a generic one that `PySequence_Concat` does not
/// consult, so the caller passes the dunder to treat as an override.
fn native_sequence(value: &Value) -> Option<NativeSeq> {
    match value.kind() {
        ValueKind::List(_) => return Some(NativeSeq::List),
        ValueKind::Tuple(_) => return Some(NativeSeq::Tuple),
        ValueKind::Str(_) => return Some(NativeSeq::Str),
        ValueKind::Bytes(_) => return Some(NativeSeq::Bytes),
        _ => {}
    }
    if is_deque_value(value) {
        return Some(NativeSeq::Deque);
    }
    if pyrust_builtins::bytearray::as_bytearray_snapshot(value).is_some() {
        return Some(NativeSeq::Bytearray);
    }
    if let ValueKind::PyInstance(_) = value.kind()
        && let Some(backing) = crate::interpreter::builtin_data_backing(value)
    {
        return match backing.kind() {
            ValueKind::List(_) => Some(NativeSeq::List),
            ValueKind::Tuple(_) => Some(NativeSeq::Tuple),
            ValueKind::Str(_) => Some(NativeSeq::Str),
            ValueKind::Bytes(_) => Some(NativeSeq::Bytes),
            _ if pyrust_builtins::bytearray::as_bytearray_snapshot(&backing).is_some() => {
                Some(NativeSeq::Bytearray)
            }
            _ => None,
        };
    }
    None
}

/// Whether the type of `value` defines `name` in Python source (as opposed to
/// inheriting the built-in slot wrapper): only a Python-level definition
/// replaces the C slot with a generic one.
fn python_defined(value: &Value, name: &str) -> bool {
    match lookup_value_special_method(value, name) {
        Some(Ok(method)) => !matches!(
            method.kind(),
            ValueKind::BuiltinFunction(_) | ValueKind::BuiltinObject { .. }
        ),
        Some(Err(_)) => true,
        None => false,
    }
}

/// `PySequence_Check`: a `__getitem__` that is not a `dict`.
fn is_sequence(value: &Value) -> bool {
    match value.kind() {
        ValueKind::List(_)
        | ValueKind::Tuple(_)
        | ValueKind::Str(_)
        | ValueKind::Bytes(_)
        | ValueKind::Range { .. } => true,
        ValueKind::Dict(_) => false,
        ValueKind::PyInstance(_) => {
            if let Some(backing) = crate::interpreter::builtin_data_backing(value)
                && matches!(backing.kind(), ValueKind::Dict(_))
            {
                return false;
            }
            native_sequence(value).is_some()
                || is_deque_value(value)
                || lookup_value_special_method(value, "__getitem__").is_some()
        }
        _ => native_sequence(value).is_some() || pyrust_builtins::memoryview::is_memoryview(value),
    }
}

/// CPython's `tp_name` for diagnostics (`collections.deque` is qualified).
fn tp_name(value: &Value) -> String {
    if is_deque_value(value) {
        "collections.deque".to_string()
    } else {
        error_type_name_str(value).into_owned()
    }
}

fn cannot_concatenate(value: &Value) -> PyError {
    type_error(format!("'{}' object can't be concatenated", tp_name(value)))
}

/// Call the built-in `sq_concat` / `sq_inplace_concat` slot of `seq` through
/// its bound dunder (no reflected-operand dispatch, as in C).
fn call_sequence_slot(
    interp: &mut Interpreter,
    seq: NativeSeq,
    dunder: &str,
    a: &Value,
    b: &Value,
) -> Result<Value> {
    let method = interp.get_attr(a, dunder)?;
    let result = interp.call_function_expanded(
        method,
        &[ExpandedCallArg {
            name: None,
            value: b.clone(),
        }],
    )?;
    if matches!(result.kind(), ValueKind::NotImplemented) {
        return Err(type_error(match seq {
            NativeSeq::Bytes | NativeSeq::Bytearray => {
                format!("can't concat {} to {}", tp_name(b), seq.name())
            }
            _ => format!(
                "can only concatenate {} (not \"{}\") to {}",
                seq.name(),
                tp_name(b),
                seq.name()
            ),
        }));
    }
    Ok(result)
}

/// The `nb_add` fallback of `PySequence_Concat`: a `NotImplemented` outcome of
/// the binary operation is reported as "can't be concatenated".
fn concat_via_add(interp: &mut Interpreter, a: &Value, b: &Value, inplace: bool) -> Result<Value> {
    let outcome = if inplace {
        inplace_op(interp, a.clone(), BinaryOp::Add, b.clone())
    } else {
        interp.eval_binary(a.clone(), BinaryOp::Add, b.clone())
    };
    match outcome {
        Err(err) if is_unsupported_add(&err) => Err(cannot_concatenate(a)),
        other => other,
    }
}

fn is_unsupported_add(err: &PyError) -> bool {
    matches!(
        err.kind(),
        pyrust_core::PyErrorKind::Named(cls, msg)
            if cls.as_ref() == "TypeError"
                && (msg.starts_with("unsupported operand type(s) for +:")
                    || msg.starts_with("unsupported operand type(s) for +=:"))
    )
}

fn sequence_concat(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<Value> {
    if let Some(seq) = native_sequence(a)
        && !python_defined(a, "__add__")
    {
        return call_sequence_slot(interp, seq, "__add__", a, b);
    }
    if is_sequence(a) && is_sequence(b) {
        return concat_via_add(interp, a, b, false);
    }
    Err(cannot_concatenate(a))
}

fn sequence_inplace_concat(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<Value> {
    if let Some(seq) = native_sequence(a) {
        if seq.has_inplace_concat() && !python_defined(a, "__iadd__") {
            return call_sequence_slot(interp, seq, "__iadd__", a, b);
        }
        if !python_defined(a, "__add__") {
            return call_sequence_slot(interp, seq, "__add__", a, b);
        }
    }
    if is_sequence(a) && is_sequence(b) {
        return concat_via_add(interp, a, b, true);
    }
    Err(cannot_concatenate(a))
}

// ─── Iteration helpers (`_PySequence_IterSearch`) ───────────────────────────

/// Scan `container` for items equal to `target` (`item == target`, identity
/// first), calling `visit(index)` for each match until it returns `false`.
fn iter_search(
    interp: &mut Interpreter,
    container: &Value,
    target: &Value,
    mut visit: impl FnMut(i64) -> bool,
) -> Result<()> {
    let iterator = match interp.call_function_expanded(
        Value::builtin_function("iter"),
        &[ExpandedCallArg {
            name: None,
            value: container.clone(),
        }],
    ) {
        Ok(iterator) => iterator,
        Err(err) if is_type_error(&err) => {
            return Err(type_error(format!(
                "argument of type '{}' is not iterable",
                tp_name(container)
            )));
        }
        Err(err) => return Err(err),
    };
    let mut index: i64 = 0;
    loop {
        let item = match interp.call_next(&iterator, None) {
            Ok(item) => item,
            Err(err) if crate::interpreter::is_stop_iteration_error(&err) => return Ok(()),
            Err(err) => return Err(err),
        };
        if interp.values_richcompare_eq(&item, target)? && !visit(index) {
            return Ok(());
        }
        index += 1;
    }
}

/// Whether `err` is a `TypeError` (or a subclass), however it was raised.
fn is_type_error(err: &PyError) -> bool {
    match err.kind() {
        pyrust_core::PyErrorKind::Named(cls, _) => cls.as_ref() == "TypeError",
        pyrust_core::PyErrorKind::Class(cls, _) => {
            crate::interpreter::class_is_builtin_exception_subclass(cls, "TypeError")
        }
        pyrust_core::PyErrorKind::Raised(exc) => match exc.kind() {
            ValueKind::PyInstance(inst) => crate::interpreter::class_is_builtin_exception_subclass(
                &inst.borrow().class,
                "TypeError",
            ),
            ValueKind::PyClass(cls) => {
                crate::interpreter::class_is_builtin_exception_subclass(cls, "TypeError")
            }
            _ => false,
        },
        _ => false,
    }
}

// ─── Lookup objects ─────────────────────────────────────────────────────────

struct AttrGetterState {
    /// The attribute arguments exactly as passed (for `repr` / `__reduce__`).
    attrs: Vec<Value>,
    /// Each attribute split at its dots.
    paths: Vec<Vec<String>>,
}

struct ItemGetterState {
    items: Vec<Value>,
}

struct MethodCallerState {
    name: Value,
    name_str: String,
    args: Vec<Value>,
    kwargs: Vec<(String, Value)>,
}

/// A bare slot-only instance of `cls` (no `__dict__`, native state attached by
/// the caller).
fn new_instance(cls: &Value) -> Result<Value> {
    let ValueKind::PyClass(class_rc) = cls.kind() else {
        return Err(type_error(
            "object.__new__(X): X is not a type object".to_string(),
        ));
    };
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(class_rc),
        attrs: InstanceAttrs::new(),
    }))))
}

fn positional_tail(args: &[ExpandedCallArg]) -> Vec<Value> {
    args.iter()
        .skip(1)
        .filter(|arg| arg.name.is_none())
        .map(|arg| arg.value.clone())
        .collect()
}

/// `(cls, args)` — the shared `__reduce__` shape of `attrgetter` / `itemgetter`.
fn reduce_with_args(self_val: &Value, args: Vec<Value>) -> Value {
    Value::tuple(vec![value_class(self_val), Value::tuple(args)])
}

/// `NAME(part, part, …)` with `NAME` the type's `tp_name`.  Re-entering the
/// repr of an object already being formatted renders `NAME(...)`
/// (`Py_ReprEnter` / `Py_ReprLeave`), sharing the interpreter's repr stack.
fn repr_call(
    interp: &mut Interpreter,
    self_val: &Value,
    type_name: &str,
    parts: Vec<Value>,
    formatted: impl FnOnce(&mut Interpreter, Vec<Value>) -> Result<Vec<String>>,
) -> Result<Value> {
    let identity = match self_val.kind() {
        ValueKind::PyInstance(inst) => Rc::as_ptr(inst) as usize as i64,
        _ => 0,
    };
    let Some(_guard) = pyrust_core::enter_repr_cycle_guard(identity) else {
        return Ok(Value::string(format!("{type_name}(...)")));
    };
    let rendered = formatted(interp, parts)?;
    Ok(Value::string(format!(
        "{type_name}({})",
        rendered.join(", ")
    )))
}

fn repr_values(interp: &mut Interpreter, values: &[Value]) -> Result<Vec<String>> {
    values
        .iter()
        .map(|value| render_value_repr(interp, value))
        .collect()
}

/// `getattr(obj, name)` for a name that is already a Rust string.
fn get_named_attr(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<Value> {
    interp.get_attr(obj, name)
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(OPERATOR_DOC),
        "__all__" => Value::list(OPERATOR_ALL.iter().map(|name| Value::string(*name)).collect()),
        "_abs" => Value::builtin_function("abs"),
    }

    // Comparison operations.

    fn lt(args) -> Result<Value> {
        binary(_interp, "lt", args, BinaryOp::Lt)
    }

    fn le(args) -> Result<Value> {
        binary(_interp, "le", args, BinaryOp::Le)
    }

    fn eq(args) -> Result<Value> {
        binary(_interp, "eq", args, BinaryOp::Eq)
    }

    fn ne(args) -> Result<Value> {
        binary(_interp, "ne", args, BinaryOp::Ne)
    }

    fn ge(args) -> Result<Value> {
        binary(_interp, "ge", args, BinaryOp::Ge)
    }

    fn gt(args) -> Result<Value> {
        binary(_interp, "gt", args, BinaryOp::Gt)
    }

    // Logical operations.

    fn not_(args) -> Result<Value> {
        let a = one_arg("not_", args)?;
        Ok(Value::bool_(!_interp.truthy_value(&a)?))
    }

    fn truth(args) -> Result<Value> {
        let a = one_arg("truth", args)?;
        Ok(Value::bool_(_interp.truthy_value(&a)?))
    }

    fn is_(args) -> Result<Value> {
        let (a, b) = two_args("is_", args)?;
        Ok(Value::bool_(a.is_identical_to(&b)))
    }

    fn is_not(args) -> Result<Value> {
        let (a, b) = two_args("is_not", args)?;
        Ok(Value::bool_(!a.is_identical_to(&b)))
    }

    // Mathematical / bitwise operations.

    fn abs(args) -> Result<Value> {
        let a = one_arg("abs", args)?;
        _interp.call_function_expanded(
            Value::builtin_function("abs"),
            &[ExpandedCallArg { name: None, value: a }],
        )
    }

    fn add(args) -> Result<Value> {
        binary(_interp, "add", args, BinaryOp::Add)
    }

    fn and_(args) -> Result<Value> {
        binary(_interp, "and_", args, BinaryOp::BitAnd)
    }

    fn floordiv(args) -> Result<Value> {
        binary(_interp, "floordiv", args, BinaryOp::FloorDiv)
    }

    fn inv(args) -> Result<Value> {
        let a = one_arg("inv", args)?;
        _interp.eval_unary(a, crate::ast::UnaryOp::BitNot)
    }

    fn invert(args) -> Result<Value> {
        let a = one_arg("invert", args)?;
        _interp.eval_unary(a, crate::ast::UnaryOp::BitNot)
    }

    fn lshift(args) -> Result<Value> {
        binary(_interp, "lshift", args, BinaryOp::LShift)
    }

    #[py_name = "mod"]
    fn mod_(args) -> Result<Value> {
        binary(_interp, "mod", args, BinaryOp::Mod)
    }

    fn mul(args) -> Result<Value> {
        binary(_interp, "mul", args, BinaryOp::Mul)
    }

    fn matmul(args) -> Result<Value> {
        binary(_interp, "matmul", args, BinaryOp::MatMul)
    }

    fn neg(args) -> Result<Value> {
        let a = one_arg("neg", args)?;
        _interp.eval_unary(a, crate::ast::UnaryOp::Neg)
    }

    fn or_(args) -> Result<Value> {
        binary(_interp, "or_", args, BinaryOp::BitOr)
    }

    fn pos(args) -> Result<Value> {
        let a = one_arg("pos", args)?;
        _interp.eval_unary(a, crate::ast::UnaryOp::Pos)
    }

    fn pow(args) -> Result<Value> {
        binary(_interp, "pow", args, BinaryOp::Pow)
    }

    fn rshift(args) -> Result<Value> {
        binary(_interp, "rshift", args, BinaryOp::RShift)
    }

    fn sub(args) -> Result<Value> {
        binary(_interp, "sub", args, BinaryOp::Sub)
    }

    fn truediv(args) -> Result<Value> {
        binary(_interp, "truediv", args, BinaryOp::Div)
    }

    fn xor(args) -> Result<Value> {
        binary(_interp, "xor", args, BinaryOp::BitXor)
    }

    // Sequence operations.

    fn concat(args) -> Result<Value> {
        let (a, b) = two_args("concat", args)?;
        sequence_concat(_interp, &a, &b)
    }

    fn contains(args) -> Result<Value> {
        let (a, b) = two_args("contains", args)?;
        let found = _interp.eval_in(a, b)?;
        Ok(Value::bool_(_interp.truthy_value(&found)?))
    }

    fn countOf(args) -> Result<Value> {
        let (a, b) = two_args("countOf", args)?;
        let mut count: i64 = 0;
        iter_search(_interp, &a, &b, |_| {
            count += 1;
            true
        })?;
        Ok(Value::int(count))
    }

    fn delitem(args) -> Result<Value> {
        let (a, b) = two_args("delitem", args)?;
        delitem_value(_interp, a, b)?;
        Ok(Value::none())
    }

    fn getitem(args) -> Result<Value> {
        let (a, b) = two_args("getitem", args)?;
        getitem_value(_interp, a, b)
    }

    fn indexOf(args) -> Result<Value> {
        let (a, b) = two_args("indexOf", args)?;
        let mut found: Option<i64> = None;
        iter_search(_interp, &a, &b, |index| {
            found = Some(index);
            false
        })?;
        match found {
            Some(index) => Ok(Value::int(index)),
            None => Err(PyError::named(
                "ValueError",
                "sequence.index(x): x not in sequence".to_string(),
            )),
        }
    }

    fn setitem(args) -> Result<Value> {
        let args = fixed_args("setitem", args, 3)?;
        setitem_value(
            _interp,
            args[0].value.clone(),
            args[1].value.clone(),
            args[2].value.clone(),
        )?;
        Ok(Value::none())
    }

    /// CPython: operator.length_hint(obj, default=0) → int.
    /// <https://docs.python.org/3/library/operator.html#operator.length_hint>
    ///
    /// The accelerated `_operator.length_hint` (which shadows the Python
    /// definition in `Lib/operator.py`) is a thin wrapper over
    /// `PyObject_LengthHint`, so this body only parses the argument list and
    /// hands off to the interpreter's shared hint protocol
    /// (`Interpreter::object_length_hint`).  Keeping the protocol there is
    /// what lets a built-in iterator answer from its own cursor rather than
    /// from a special case here (issue #2920).
    ///
    /// The legacy argument dialect is deliberate: CPython's C signature is
    /// positional-only with `METH_VARARGS` arity wording that the typed
    /// prelude's `_operator.`-qualified keyword rejection cannot reproduce.
    fn length_hint(args) -> Result<Value> {
        if args.iter().any(|arg| arg.name.is_some()) {
            return Err(PyError::named(
                "TypeError",
                "_operator.length_hint() takes no keyword arguments".to_string(),
            ));
        }
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                "length_hint expected at least 1 argument, got 0".to_string(),
            ));
        }
        if args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "length_hint expected at most 2 arguments, got {}",
                    args.len()
                ),
            ));
        }
        // `default` is a plain `Py_ssize_t` in the C signature: it goes through
        // `__index__` and may legitimately be negative, so it does not share
        // the `>= 0` validation the hint result gets.
        let default = match args.get(1) {
            Some(arg) => _interp.value_to_isize(
                &arg.value,
                "Python int too large to convert to C ssize_t",
            )?,
            None => 0,
        };
        _interp
            .object_length_hint(&args[0].value, default)
            .map(Value::int)
    }

    // Other operations.

    /// CPython: operator.call(obj, /, *args, **kwargs) — `obj(*args, **kwargs)`.
    fn call(args) -> Result<Value> {
        let Some(first) = args.first().filter(|arg| arg.name.is_none()) else {
            return Err(type_error(format!(
                "call expected at least 1 argument, got {}",
                args.iter().filter(|arg| arg.name.is_none()).count()
            )));
        };
        _interp.call_function_expanded(first.value.clone(), &args[1..])
    }

    // In-place operations.

    fn iadd(args) -> Result<Value> {
        inplace(_interp, "iadd", args, BinaryOp::Add)
    }

    fn iand(args) -> Result<Value> {
        inplace(_interp, "iand", args, BinaryOp::BitAnd)
    }

    fn iconcat(args) -> Result<Value> {
        let (a, b) = two_args("iconcat", args)?;
        sequence_inplace_concat(_interp, &a, &b)
    }

    fn ifloordiv(args) -> Result<Value> {
        inplace(_interp, "ifloordiv", args, BinaryOp::FloorDiv)
    }

    fn ilshift(args) -> Result<Value> {
        inplace(_interp, "ilshift", args, BinaryOp::LShift)
    }

    fn imod(args) -> Result<Value> {
        inplace(_interp, "imod", args, BinaryOp::Mod)
    }

    fn imul(args) -> Result<Value> {
        inplace(_interp, "imul", args, BinaryOp::Mul)
    }

    fn imatmul(args) -> Result<Value> {
        inplace(_interp, "imatmul", args, BinaryOp::MatMul)
    }

    fn ior(args) -> Result<Value> {
        inplace(_interp, "ior", args, BinaryOp::BitOr)
    }

    fn ipow(args) -> Result<Value> {
        inplace(_interp, "ipow", args, BinaryOp::Pow)
    }

    fn irshift(args) -> Result<Value> {
        inplace(_interp, "irshift", args, BinaryOp::RShift)
    }

    fn isub(args) -> Result<Value> {
        inplace(_interp, "isub", args, BinaryOp::Sub)
    }

    fn itruediv(args) -> Result<Value> {
        inplace(_interp, "itruediv", args, BinaryOp::Div)
    }

    fn ixor(args) -> Result<Value> {
        inplace(_interp, "ixor", args, BinaryOp::BitXor)
    }

    // Generalized lookup objects.

    #[native_state(AttrGetterState)]
    class attrgetter {
        constants {
            "__doc__" => Value::string(ATTRGETTER_DOC),
        }

        fn __new__(args) -> Result<Value> {
            let Some(cls) = args.first() else {
                return Err(type_error("attrgetter.__new__(): not enough arguments".to_string()));
            };
            if args[1..].iter().any(|arg| arg.name.is_some()) {
                return Err(type_error("attrgetter() takes no keyword arguments".to_string()));
            }
            let attrs = positional_tail(args);
            if attrs.is_empty() {
                return Err(type_error("attrgetter expected 1 argument, got 0".to_string()));
            }
            let mut paths = Vec::with_capacity(attrs.len());
            for attr in &attrs {
                if !is_str_or_str_subclass(attr) {
                    return Err(type_error("attribute name must be a string".to_string()));
                }
                paths.push(
                    extract_str_value(attr)
                        .split('.')
                        .map(str::to_string)
                        .collect::<Vec<_>>(),
                );
            }
            let inst = new_instance(&cls.value)?;
            set_native_state(&inst, AttrGetterState { attrs, paths })?;
            Ok(inst)
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] call_args: &[Value],
            #[varkwargs] call_kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            if !call_kwargs.is_empty() {
                return Err(type_error("attrgetter() takes no keyword arguments".to_string()));
            }
            if call_args.len() != 1 {
                return Err(type_error(format!(
                    "attrgetter expected 1 argument, got {}",
                    call_args.len()
                )));
            }
            let paths = with_native_state::<AttrGetterState, _>(self_val.0, "attrgetter", |st| {
                Ok(st.paths.clone())
            })?;
            let obj = &call_args[0];
            let mut results = Vec::with_capacity(paths.len());
            for path in &paths {
                let mut current = obj.clone();
                for name in path {
                    current = get_named_attr(_interp, &current, name)?;
                }
                results.push(current);
            }
            if paths.len() == 1 {
                Ok(results.pop().unwrap_or_else(Value::none))
            } else {
                Ok(Value::tuple(results))
            }
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let attrs = with_native_state::<AttrGetterState, _>(self_val.0, "attrgetter", |st| {
                Ok(st.attrs.clone())
            })?;
            repr_call(_interp, self_val.0, "operator.attrgetter", attrs, |interp, attrs| repr_values(interp, &attrs))
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let attrs = with_native_state::<AttrGetterState, _>(self_val.0, "attrgetter", |st| {
                Ok(st.attrs.clone())
            })?;
            Ok(reduce_with_args(self_val.0, attrs))
        }
    }

    #[native_state(ItemGetterState)]
    class itemgetter {
        constants {
            "__doc__" => Value::string(ITEMGETTER_DOC),
        }

        fn __new__(args) -> Result<Value> {
            let Some(cls) = args.first() else {
                return Err(type_error("itemgetter.__new__(): not enough arguments".to_string()));
            };
            if args[1..].iter().any(|arg| arg.name.is_some()) {
                return Err(type_error("itemgetter() takes no keyword arguments".to_string()));
            }
            let items = positional_tail(args);
            if items.is_empty() {
                return Err(type_error("itemgetter expected 1 argument, got 0".to_string()));
            }
            let inst = new_instance(&cls.value)?;
            set_native_state(&inst, ItemGetterState { items })?;
            Ok(inst)
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] call_args: &[Value],
            #[varkwargs] call_kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            if !call_kwargs.is_empty() {
                return Err(type_error("itemgetter() takes no keyword arguments".to_string()));
            }
            if call_args.len() != 1 {
                return Err(type_error(format!(
                    "itemgetter expected 1 argument, got {}",
                    call_args.len()
                )));
            }
            let items = with_native_state::<ItemGetterState, _>(self_val.0, "itemgetter", |st| {
                Ok(st.items.clone())
            })?;
            let obj = &call_args[0];
            if items.len() == 1 {
                return getitem_value(_interp, obj.clone(), items[0].clone());
            }
            let mut results = Vec::with_capacity(items.len());
            for item in items {
                results.push(getitem_value(_interp, obj.clone(), item)?);
            }
            Ok(Value::tuple(results))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let items = with_native_state::<ItemGetterState, _>(self_val.0, "itemgetter", |st| {
                Ok(st.items.clone())
            })?;
            repr_call(_interp, self_val.0, "operator.itemgetter", items, |interp, items| repr_values(interp, &items))
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let items = with_native_state::<ItemGetterState, _>(self_val.0, "itemgetter", |st| {
                Ok(st.items.clone())
            })?;
            Ok(reduce_with_args(self_val.0, items))
        }
    }

    #[native_state(MethodCallerState)]
    class methodcaller {
        constants {
            "__doc__" => Value::string(METHODCALLER_DOC),
        }

        fn __new__(args) -> Result<Value> {
            let Some(cls) = args.first() else {
                return Err(type_error("methodcaller.__new__(): not enough arguments".to_string()));
            };
            let positional = positional_tail(args);
            let Some(name) = positional.first().cloned() else {
                return Err(type_error(
                    "methodcaller needs at least one argument, the method name".to_string(),
                ));
            };
            if !is_str_or_str_subclass(&name) {
                return Err(type_error("method name must be a string".to_string()));
            }
            let name_str = extract_str_value(&name);
            let kwargs = args[1..]
                .iter()
                .filter_map(|arg| arg.name.clone().map(|key| (key, arg.value.clone())))
                .collect();
            let inst = new_instance(&cls.value)?;
            set_native_state(
                &inst,
                MethodCallerState {
                    name,
                    name_str,
                    args: positional[1..].to_vec(),
                    kwargs,
                },
            )?;
            Ok(inst)
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] call_args: &[Value],
            #[varkwargs] call_kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            if !call_kwargs.is_empty() {
                return Err(type_error("methodcaller() takes no keyword arguments".to_string()));
            }
            if call_args.len() != 1 {
                return Err(type_error(format!(
                    "methodcaller expected 1 argument, got {}",
                    call_args.len()
                )));
            }
            let (name, positional, kwargs) =
                with_native_state::<MethodCallerState, _>(self_val.0, "methodcaller", |st| {
                    Ok((st.name_str.clone(), st.args.clone(), st.kwargs.clone()))
                })?;
            let method = get_named_attr(_interp, &call_args[0], &name)?;
            let mut call: Vec<ExpandedCallArg> = positional
                .into_iter()
                .map(|value| ExpandedCallArg { name: None, value })
                .collect();
            call.extend(kwargs.into_iter().map(|(key, value)| ExpandedCallArg {
                name: Some(key),
                value,
            }));
            _interp.call_function_expanded(method, &call)
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (name, positional, kwargs) =
                with_native_state::<MethodCallerState, _>(self_val.0, "methodcaller", |st| {
                    Ok((st.name.clone(), st.args.clone(), st.kwargs.clone()))
                })?;
            let mut parts = vec![name];
            parts.extend(positional);
            let keys: Vec<String> = kwargs.iter().map(|(key, _)| key.clone()).collect();
            parts.extend(kwargs.into_iter().map(|(_, value)| value));
            let positional_count = parts.len() - keys.len();
            repr_call(_interp, self_val.0, "operator.methodcaller", parts, move |interp, parts| {
                let rendered = repr_values(interp, &parts)?;
                Ok(rendered
                    .into_iter()
                    .enumerate()
                    .map(|(index, text)| match index.checked_sub(positional_count) {
                        Some(key_index) => format!("{}={text}", keys[key_index]),
                        None => text,
                    })
                    .collect())
            })
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (name, positional, kwargs) =
                with_native_state::<MethodCallerState, _>(self_val.0, "methodcaller", |st| {
                    Ok((st.name.clone(), st.args.clone(), st.kwargs.clone()))
                })?;
            let cls = value_class(self_val.0);
            if kwargs.is_empty() {
                let mut all = vec![name];
                all.extend(positional);
                return Ok(Value::tuple(vec![cls, Value::tuple(all)]));
            }
            let functools = _interp.load_module("functools")?;
            let partial = _interp.get_attr(&functools, "partial")?;
            let mut call = vec![
                ExpandedCallArg { name: None, value: cls },
                ExpandedCallArg { name: None, value: name },
            ];
            call.extend(kwargs.into_iter().map(|(key, value)| ExpandedCallArg {
                name: Some(key),
                value,
            }));
            let constructor = _interp.call_function_expanded(partial, &call)?;
            Ok(Value::tuple(vec![constructor, Value::tuple(positional)]))
        }
    }
}
