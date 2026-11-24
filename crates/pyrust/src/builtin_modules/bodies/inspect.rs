// `inspect` module — body for the `inspect @inject` entry in
// `pyrust_builtin_modules!` (issue #3152).
//
// CPython 3.12's `inspect` is a pure-Python module built on runtime `code`,
// `frame`, `traceback`, `function`, `method`, generator/coroutine/asyncgen,
// and descriptor introspection.  A single private native helper
// `_code_names_by_kind` distinguishes `LoadGlobal` vs `GetAttr`/`CallMethod`
// instructions in a function's `FnCode` so `getclosurevars()` classifies
// globals, builtins, and unbound attribute names identically to CPython 3.12.
//
// Reference: <https://docs.python.org/3/library/inspect.html>

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use crate::bytecode::{FnCode, Insn};
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::Interpreter;
use crate::interpreter::builtin_args::PyValue;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `inspect` module.
const INSPECT_PY_SOURCE: &str = include_str!("inspect_py.py");

/// Public names exported from `INSPECT_PY_SOURCE` onto the `inspect` module.
const INSPECT_PY_EXPORTS: &[&str] = &[
    "__all__",
    "AGEN_CLOSED",
    "AGEN_CREATED",
    "AGEN_RUNNING",
    "AGEN_SUSPENDED",
    "ArgInfo",
    "Arguments",
    "Attribute",
    "BoundArguments",
    "BufferFlags",
    "CORO_CLOSED",
    "CORO_CREATED",
    "CORO_RUNNING",
    "CORO_SUSPENDED",
    "CO_ASYNC_GENERATOR",
    "CO_COROUTINE",
    "CO_GENERATOR",
    "CO_ITERABLE_COROUTINE",
    "CO_NESTED",
    "CO_NEWLOCALS",
    "CO_NOFREE",
    "CO_OPTIMIZED",
    "CO_VARARGS",
    "CO_VARKEYWORDS",
    "ClassFoundException",
    "ClosureVars",
    "EndOfBlock",
    "FrameInfo",
    "FullArgSpec",
    "GEN_CLOSED",
    "GEN_CREATED",
    "GEN_RUNNING",
    "GEN_SUSPENDED",
    "Parameter",
    "Signature",
    "TPFLAGS_IS_ABSTRACT",
    "Traceback",
    "classify_class_attrs",
    "cleandoc",
    "currentframe",
    "findsource",
    "formatannotation",
    "formatannotationrelativeto",
    "formatargvalues",
    "get_annotations",
    "getabsfile",
    "getargs",
    "getargvalues",
    "getasyncgenlocals",
    "getasyncgenstate",
    "getattr_static",
    "getblock",
    "getcallargs",
    "getclasstree",
    "getclosurevars",
    "getcomments",
    "getcoroutinelocals",
    "getcoroutinestate",
    "getdoc",
    "getfile",
    "getframeinfo",
    "getfullargspec",
    "getgeneratorlocals",
    "getgeneratorstate",
    "getinnerframes",
    "getlineno",
    "getmembers",
    "getmembers_static",
    "getmodule",
    "getmodulename",
    "getmro",
    "getouterframes",
    "getsource",
    "getsourcefile",
    "getsourcelines",
    "indentsize",
    "isabstract",
    "isasyncgen",
    "isasyncgenfunction",
    "isawaitable",
    "isbuiltin",
    "isclass",
    "iscode",
    "iscoroutine",
    "iscoroutinefunction",
    "isdatadescriptor",
    "isframe",
    "isfunction",
    "isgenerator",
    "isgeneratorfunction",
    "isgetsetdescriptor",
    "ismemberdescriptor",
    "ismethod",
    "ismethoddescriptor",
    "ismethodwrapper",
    "ismodule",
    "isroutine",
    "istraceback",
    "markcoroutinefunction",
    "modulesbyfile",
    "signature",
    "stack",
    "trace",
    "unwrap",
    "walktree",
];

pyrust_module! {
    /// Inspect a `UserFunction`'s compiled `FnCode` instructions and return
    /// `(global_names, attr_names)` as tuples of strings for `getclosurevars`.
    #[arity_style(takes_exactly_one)]
    fn _code_names_by_kind(#[positional_only] func: PyValue) -> Result<Value> {
        let ValueKind::UserFunction(function) = func.0.kind() else {
            return Ok(Value::tuple(vec![
                Value::tuple(Vec::new()),
                Value::tuple(Vec::new()),
            ]));
        };
        let Some(rc) = function.precompiled_code.as_ref() else {
            return Ok(Value::tuple(vec![
                Value::tuple(Vec::new()),
                Value::tuple(Vec::new()),
            ]));
        };
        let Ok(fncode) = Rc::clone(rc).downcast::<FnCode>() else {
            return Ok(Value::tuple(vec![
                Value::tuple(Vec::new()),
                Value::tuple(Vec::new()),
            ]));
        };
        let mut global_seen: HashSet<&str> = HashSet::new();
        let mut global_names: Vec<Value> = Vec::new();
        let mut attr_seen: HashSet<&str> = HashSet::new();
        let mut attr_names: Vec<Value> = Vec::new();

        let mut push_attr = |idx: u16| {
            if let Some(sym) = fncode.names.get(idx as usize) {
                let s = sym.as_str();
                if attr_seen.insert(s) {
                    attr_names.push(Value::string(s));
                }
            }
        };

        for insn in &fncode.insns {
            match insn {
                Insn::LoadGlobal(_, idx) => {
                    if let Some(sym) = fncode.names.get(*idx as usize) {
                        let s = sym.as_str();
                        if global_seen.insert(s) {
                            global_names.push(Value::string(s));
                        }
                    }
                }
                Insn::GetAttr(_, _, idx)
                | Insn::GetAttrForWith(_, _, idx, _)
                | Insn::CallMethod { name_idx: idx, .. } => push_attr(*idx),
                Insn::CallMethodKw(desc_idx) => {
                    push_attr(fncode.descs[*desc_idx as usize].as_call_method_kw().name_idx);
                }
                Insn::CallMethodExpanded(desc_idx) => {
                    push_attr(
                        fncode.descs[*desc_idx as usize]
                            .as_call_method_expanded()
                            .name_idx,
                    );
                }
                _ => {}
            }
        }
        Ok(Value::tuple(vec![
            Value::tuple(global_names),
            Value::tuple(attr_names),
        ]))
    }
}

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    if let Some(helper) = module.borrow().attrs.get("_code_names_by_kind").cloned() {
        ns.dict_insert(PyKey::str_from("_code_names_by_kind"), helper)?;
    }
    interp.exec_source(INSPECT_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("inspect: exec namespace not a dict".into()))?;
    for name in INSPECT_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    module
        .borrow_mut()
        .attrs
        .shift_remove("_code_names_by_kind");
    Ok(Some(ns.clone()))
}
