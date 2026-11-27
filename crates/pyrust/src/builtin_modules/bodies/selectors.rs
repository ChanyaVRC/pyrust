// `selectors` module — 100% native Rust port of CPython 3.12.3's
// `Lib/selectors.py` (issues #3205, #3453); there is no Python-source
// injection.
//
// High-level I/O multiplexing built on the `select` module primitives:
// `EVENT_READ` / `EVENT_WRITE`, the `SelectorKey` named tuple, the read-only
// `_SelectorMapping` returned by `get_map()`, the `BaseSelector` ABC, the
// `_BaseSelectorImpl` bookkeeping base, and the `SelectSelector`,
// `PollSelector`, `EpollSelector` and `KqueueSelector` implementations plus
// the `DefaultSelector` alias.
//
// Like the CPython source, the implementation keeps its state in ordinary
// instance attributes (`_fd_to_key`, `_map`, `_readers`, `_writers`,
// `_selector`, `_max_events`), dispatches every internal step through the
// instance (`self._fileobj_lookup`, `self._select`, `super().register`, ...)
// and looks module globals up on the live module, so subclasses, mixins and
// monkeypatching behave as they do on CPython.  Waiting is delegated to
// `select.select` / `poll` / `epoll` / `kqueue`, which release the GIL while
// they block.
//
// `post_load_init` runs the module-level statements of `selectors.py` that
// need the interpreter: the `namedtuple` call, the `hasattr(select, ...)`
// platform gating, the ABC wiring (`ABCMeta`, abstract methods,
// `_SelectorMapping(Mapping)`) and the `DefaultSelector` choice.
//
// Reference: <https://docs.python.org/3/library/selectors.html>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::ast::{BinaryOp, UnaryOp};
use crate::builtin_modules::json::args::{self, Sig};
use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValue, PyValueRef};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, class_is_subclass_of, lookup_exc_class, render_value_repr,
};
use crate::value::{PyDict, PyModule, PySet, Value, ValueKind};

const EVENT_READ: i64 = 1;
const EVENT_WRITE: i64 = 2;

const MODULE_DOC: &str = "Selectors module.\n\n\
This module allows high-level and efficient I/O multiplexing, built upon the\n\
`select` module primitives.\n";

/// Module namespace order of CPython 3.12.3's `selectors.py` (after the
/// dunder entries); `post_load_init` restores it.
const MODULE_ORDER: &[&str] = &[
    "ABCMeta",
    "abstractmethod",
    "namedtuple",
    "Mapping",
    "math",
    "select",
    "sys",
    "EVENT_READ",
    "EVENT_WRITE",
    "_fileobj_to_fd",
    "SelectorKey",
    "_SelectorMapping",
    "BaseSelector",
    "_BaseSelectorImpl",
    "SelectSelector",
    "_PollLikeSelector",
    "PollSelector",
    "EpollSelector",
    "KqueueSelector",
    "_can_use",
    "DefaultSelector",
];

/// Classes made into ABCs, parents first.
const ABC_CLASSES: &[&str] = &[
    "BaseSelector",
    "_BaseSelectorImpl",
    "SelectSelector",
    "_PollLikeSelector",
    "PollSelector",
    "EpollSelector",
    "KqueueSelector",
    "_SelectorMapping",
];

/// `(selector class, select attribute, event-in, event-out)` for the
/// platform selectors gated by `hasattr(select, attribute)`.
const PLATFORM_SELECTORS: &[(&str, &str, &str, &str)] = &[
    ("PollSelector", "poll", "POLLIN", "POLLOUT"),
    ("EpollSelector", "epoll", "EPOLLIN", "EPOLLOUT"),
    ("KqueueSelector", "kqueue", "", ""),
];

const fn sig(qualname: &'static str, params: &'static [&'static str], defaults: usize) -> Sig {
    Sig {
        qualname,
        params,
        defaults,
        kwonly: &[],
        var_kw: false,
    }
}

const SIG_FILEOBJ_TO_FD: Sig = sig("_fileobj_to_fd", &["fileobj"], 0);
const SIG_CAN_USE: Sig = sig("_can_use", &["method"], 0);
const SIG_MAPPING_INIT: Sig = sig("_SelectorMapping.__init__", &["self", "selector"], 0);
const SIG_MAPPING_LEN: Sig = sig("_SelectorMapping.__len__", &["self"], 0);
const SIG_MAPPING_GETITEM: Sig = sig("_SelectorMapping.__getitem__", &["self", "fileobj"], 0);
const SIG_MAPPING_ITER: Sig = sig("_SelectorMapping.__iter__", &["self"], 0);
const SIG_BASE_REGISTER: Sig = sig(
    "BaseSelector.register",
    &["self", "fileobj", "events", "data"],
    1,
);
const SIG_BASE_UNREGISTER: Sig = sig("BaseSelector.unregister", &["self", "fileobj"], 0);
const SIG_BASE_MODIFY: Sig = sig(
    "BaseSelector.modify",
    &["self", "fileobj", "events", "data"],
    1,
);
const SIG_BASE_SELECT: Sig = sig("BaseSelector.select", &["self", "timeout"], 1);
const SIG_BASE_CLOSE: Sig = sig("BaseSelector.close", &["self"], 0);
const SIG_BASE_GET_KEY: Sig = sig("BaseSelector.get_key", &["self", "fileobj"], 0);
const SIG_BASE_GET_MAP: Sig = sig("BaseSelector.get_map", &["self"], 0);
const SIG_BASE_ENTER: Sig = sig("BaseSelector.__enter__", &["self"], 0);
const SIG_IMPL_INIT: Sig = sig("_BaseSelectorImpl.__init__", &["self"], 0);
const SIG_IMPL_LOOKUP: Sig = sig("_BaseSelectorImpl._fileobj_lookup", &["self", "fileobj"], 0);
const SIG_IMPL_REGISTER: Sig = sig(
    "_BaseSelectorImpl.register",
    &["self", "fileobj", "events", "data"],
    1,
);
const SIG_IMPL_UNREGISTER: Sig = sig("_BaseSelectorImpl.unregister", &["self", "fileobj"], 0);
const SIG_IMPL_MODIFY: Sig = sig(
    "_BaseSelectorImpl.modify",
    &["self", "fileobj", "events", "data"],
    1,
);
const SIG_IMPL_CLOSE: Sig = sig("_BaseSelectorImpl.close", &["self"], 0);
const SIG_IMPL_GET_MAP: Sig = sig("_BaseSelectorImpl.get_map", &["self"], 0);
const SIG_IMPL_KEY_FROM_FD: Sig = sig("_BaseSelectorImpl._key_from_fd", &["self", "fd"], 0);
const SIG_SELECT_INIT: Sig = sig("SelectSelector.__init__", &["self"], 0);
const SIG_SELECT_REGISTER: Sig = sig(
    "SelectSelector.register",
    &["self", "fileobj", "events", "data"],
    1,
);
const SIG_SELECT_UNREGISTER: Sig = sig("SelectSelector.unregister", &["self", "fileobj"], 0);
#[cfg(windows)]
const SIG_SELECT_SELECT_HOOK: Sig = sig(
    "SelectSelector._select",
    &["self", "r", "w", "_", "timeout"],
    1,
);
const SIG_SELECT_SELECT: Sig = sig("SelectSelector.select", &["self", "timeout"], 1);
const SIG_POLL_INIT: Sig = sig("_PollLikeSelector.__init__", &["self"], 0);
const SIG_POLL_REGISTER: Sig = sig(
    "_PollLikeSelector.register",
    &["self", "fileobj", "events", "data"],
    1,
);
const SIG_POLL_UNREGISTER: Sig = sig("_PollLikeSelector.unregister", &["self", "fileobj"], 0);
const SIG_POLL_MODIFY: Sig = sig(
    "_PollLikeSelector.modify",
    &["self", "fileobj", "events", "data"],
    1,
);
const SIG_POLL_SELECT: Sig = sig("_PollLikeSelector.select", &["self", "timeout"], 1);
const SIG_EPOLL_FILENO: Sig = sig("EpollSelector.fileno", &["self"], 0);
const SIG_EPOLL_SELECT: Sig = sig("EpollSelector.select", &["self", "timeout"], 1);
const SIG_EPOLL_CLOSE: Sig = sig("EpollSelector.close", &["self"], 0);
const SIG_KQUEUE_INIT: Sig = sig("KqueueSelector.__init__", &["self"], 0);
const SIG_KQUEUE_FILENO: Sig = sig("KqueueSelector.fileno", &["self"], 0);
const SIG_KQUEUE_REGISTER: Sig = sig(
    "KqueueSelector.register",
    &["self", "fileobj", "events", "data"],
    1,
);
const SIG_KQUEUE_UNREGISTER: Sig = sig("KqueueSelector.unregister", &["self", "fileobj"], 0);
const SIG_KQUEUE_SELECT: Sig = sig("KqueueSelector.select", &["self", "timeout"], 1);
const SIG_KQUEUE_CLOSE: Sig = sig("KqueueSelector.close", &["self"], 0);

// ── Small protocol helpers ───────────────────────────────────────────────────

fn call(interp: &mut Interpreter, callee: &Value, values: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = values
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee.clone(), &expanded)
}

/// `receiver.name(*values)`.
fn method(
    interp: &mut Interpreter,
    receiver: &Value,
    name: &str,
    values: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(receiver, name)?;
    call(interp, &callee, values)
}

/// A builtin function called with positional arguments.
fn builtin(interp: &mut Interpreter, name: &'static str, values: &[Value]) -> Result<Value> {
    call(interp, &Value::builtin_function(name), values)
}

fn binary(interp: &mut Interpreter, left: &Value, op: BinaryOp, right: &Value) -> Result<Value> {
    interp.eval_binary(left.clone(), op, right.clone())
}

/// `bool(left <op> right)`.
fn test(interp: &mut Interpreter, left: &Value, op: BinaryOp, right: &Value) -> Result<bool> {
    let result = binary(interp, left, op, right)?;
    interp.truthy_value(&result)
}

fn attr(interp: &mut Interpreter, receiver: &Value, name: &str) -> Result<Value> {
    interp.get_attr(receiver, name)
}

fn set_attr(interp: &mut Interpreter, receiver: &Value, name: &str, value: Value) -> Result<()> {
    interp.assign_attr(receiver.clone(), name, value)
}

/// A global of the live `selectors` module (what the CPython source reads as a
/// bare name, so monkeypatching `selectors.X` is honoured).
fn module_global(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = interp.load_module("selectors")?;
    interp.get_attr(&module, name)
}

fn is_same(interp: &mut Interpreter, left: &Value, right: &Value) -> Result<bool> {
    test(interp, left, BinaryOp::Is, right)
}

fn is_instance_of_exceptions(value: &Value, names: &[&str]) -> bool {
    let ValueKind::PyInstance(instance) = value.kind() else {
        return false;
    };
    let class = Rc::clone(&instance.borrow().class);
    names.iter().any(|name| {
        lookup_exc_class(name).is_some_and(|expected| class_is_subclass_of(&class, &expected))
    })
}

/// Whether `error` is an instance of any of the named builtin exception
/// classes (`except (A, B):`), plus an equivalent error to re-raise.
fn error_is(interp: &mut Interpreter, error: PyError, names: &[&str]) -> Result<(bool, PyError)> {
    let exception = interp.materialize_pyerror(error)?;
    let hit = is_instance_of_exceptions(&exception, names);
    Ok((hit, PyError::raised(exception)))
}

/// `raise <error> from None`.
fn from_none(interp: &mut Interpreter, error: PyError) -> PyError {
    let Ok(exception) = interp.materialize_pyerror(error) else {
        return PyError::runtime("selectors: could not build exception".to_string());
    };
    let _ = interp.assign_attr(exception.clone(), "__cause__", Value::none());
    let _ = interp.assign_attr(
        exception.clone(),
        "__suppress_context__",
        Value::bool_(true),
    );
    PyError::raised(exception)
}

/// `KeyError(f"{fileobj!r} is not registered")`, raised `from None`.
fn not_registered(interp: &mut Interpreter, fileobj: &Value) -> PyError {
    match render_value_repr(interp, fileobj) {
        Ok(repr) => {
            let error = PyError::key_error(Value::string(format!("{repr} is not registered")));
            from_none(interp, error)
        }
        Err(error) => error,
    }
}

/// `except KeyError: raise KeyError(f"{fileobj!r} is not registered") from None`
/// applied to `result`.
fn key_error_to_not_registered(
    interp: &mut Interpreter,
    result: Result<Value>,
    fileobj: &Value,
) -> Result<Value> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let (is_key_error, error) = error_is(interp, error, &["KeyError"])?;
            if is_key_error {
                Err(not_registered(interp, fileobj))
            } else {
                Err(error)
            }
        }
    }
}

/// `a, b, ... = iterable` with CPython's unpacking errors.
fn unpack(interp: &mut Interpreter, value: &Value, expected: usize) -> Result<Vec<Value>> {
    let items = interp.collect_iterable(value)?;
    if items.len() < expected {
        return Err(PyError::named(
            "ValueError",
            format!(
                "not enough values to unpack (expected {expected}, got {})",
                items.len()
            ),
        ));
    }
    if items.len() > expected {
        return Err(PyError::named(
            "ValueError",
            format!("too many values to unpack (expected {expected})"),
        ));
    }
    Ok(items)
}

/// `super(<class named cls>, this).<name>`.
fn super_method(
    interp: &mut Interpreter,
    class_name: &str,
    this: &Value,
    name: &str,
) -> Result<Value> {
    let class = module_global(interp, class_name)?;
    let proxy = builtin(interp, "super", &[class, this.clone()])?;
    interp.get_attr(&proxy, name)
}

fn empty_list() -> Value {
    Value::list(Vec::new())
}

fn not_implemented<T>() -> Result<T> {
    Err(PyError::named("NotImplementedError", String::new()))
}

// ── `_fileobj_to_fd` ─────────────────────────────────────────────────────────

/// `isinstance(value, int)`.
fn is_int_instance(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    match value.kind() {
        ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_) => Ok(true),
        // Instances of `int` subclasses are not primitives.
        _ => {
            let builtins = interp.load_module("builtins")?;
            let int_class = interp.get_attr(&builtins, "int")?;
            let result = builtin(interp, "isinstance", &[value.clone(), int_class])?;
            interp.truthy_value(&result)
        }
    }
}

/// `int(fileobj.fileno())`.
fn fileno_of(interp: &mut Interpreter, fileobj: &Value) -> Result<Value> {
    let fileno = method(interp, fileobj, "fileno", &[])?;
    builtin(interp, "int", &[fileno])
}

/// `_fileobj_to_fd(fileobj)`: the file descriptor of a file object or int.
fn fileobj_to_fd(interp: &mut Interpreter, fileobj: &Value) -> Result<Value> {
    let fd = if is_int_instance(interp, fileobj)? {
        fileobj.clone()
    } else {
        match fileno_of(interp, fileobj) {
            Ok(fd) => fd,
            Err(error) => {
                let (invalid, error) = error_is(
                    interp,
                    error,
                    &["AttributeError", "TypeError", "ValueError"],
                )?;
                if !invalid {
                    return Err(error);
                }
                let repr = render_value_repr(interp, fileobj)?;
                let error = PyError::named("ValueError", format!("Invalid file object: {repr}"));
                return Err(from_none(interp, error));
            }
        }
    };
    if test(interp, &fd, BinaryOp::Lt, &Value::int(0))? {
        let text = interp.render_value_as_str(&fd)?;
        return Err(PyError::named(
            "ValueError",
            format!("Invalid file descriptor: {text}"),
        ));
    }
    Ok(fd)
}

// ── `_BaseSelectorImpl` building blocks ──────────────────────────────────────

fn fd_to_key(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    attr(interp, this, "_fd_to_key")
}

/// `self._fileobj_lookup(fileobj)`.
fn lookup(interp: &mut Interpreter, this: &Value, fileobj: &Value) -> Result<Value> {
    method(
        interp,
        this,
        "_fileobj_lookup",
        std::slice::from_ref(fileobj),
    )
}

/// `self._key_from_fd(fd)`.
fn key_from_fd(interp: &mut Interpreter, this: &Value, fd: &Value) -> Result<Value> {
    method(interp, this, "_key_from_fd", std::slice::from_ref(fd))
}

/// `events & ~(EVENT_READ | EVENT_WRITE)` or `not events`.
fn events_invalid(interp: &mut Interpreter, events: &Value) -> Result<bool> {
    if !interp.truthy_value(events)? {
        return Ok(true);
    }
    test(
        interp,
        events,
        BinaryOp::BitAnd,
        &Value::int(!(EVENT_READ | EVENT_WRITE)),
    )
}

/// `events & flag` as a truth value.
fn has_event(interp: &mut Interpreter, events: &Value, flag: &Value) -> Result<bool> {
    test(interp, events, BinaryOp::BitAnd, flag)
}

/// `self._fd_to_key[key.fd] = key`.
fn store_key(interp: &mut Interpreter, this: &Value, key: &Value) -> Result<()> {
    let map = fd_to_key(interp, this)?;
    let fd = attr(interp, key, "fd")?;
    method(interp, &map, "__setitem__", &[fd, key.clone()])?;
    Ok(())
}

fn impl_register(
    interp: &mut Interpreter,
    this: &Value,
    fileobj: &Value,
    events: &Value,
    data: &Value,
) -> Result<Value> {
    if events_invalid(interp, events)? {
        let repr = render_value_repr(interp, events)?;
        return Err(PyError::named(
            "ValueError",
            format!("Invalid events: {repr}"),
        ));
    }
    let selector_key = module_global(interp, "SelectorKey")?;
    let fd = lookup(interp, this, fileobj)?;
    let key = call(
        interp,
        &selector_key,
        &[fileobj.clone(), fd.clone(), events.clone(), data.clone()],
    )?;
    let key_fd = attr(interp, &key, "fd")?;
    let map = fd_to_key(interp, this)?;
    if test(interp, &key_fd, BinaryOp::In, &map)? {
        let repr = render_value_repr(interp, fileobj)?;
        let text = interp.render_value_as_str(&key_fd)?;
        return Err(PyError::key_error(Value::string(format!(
            "{repr} (FD {text}) is already registered"
        ))));
    }
    store_key(interp, this, &key)?;
    Ok(key)
}

fn impl_unregister(interp: &mut Interpreter, this: &Value, fileobj: &Value) -> Result<Value> {
    let popped = (|| {
        let fd = lookup(interp, this, fileobj)?;
        let map = fd_to_key(interp, this)?;
        method(interp, &map, "pop", &[fd])
    })();
    key_error_to_not_registered(interp, popped, fileobj)
}

fn impl_modify(
    interp: &mut Interpreter,
    this: &Value,
    fileobj: &Value,
    events: &Value,
    data: &Value,
) -> Result<Value> {
    let found = (|| {
        let fd = lookup(interp, this, fileobj)?;
        let map = fd_to_key(interp, this)?;
        interp.eval_index(&map, fd)
    })();
    let key = key_error_to_not_registered(interp, found, fileobj)?;
    let key_events = attr(interp, &key, "events")?;
    if test(interp, events, BinaryOp::Ne, &key_events)? {
        method(interp, this, "unregister", std::slice::from_ref(fileobj))?;
        return method(
            interp,
            this,
            "register",
            &[fileobj.clone(), events.clone(), data.clone()],
        );
    }
    let key_data = attr(interp, &key, "data")?;
    if test(interp, data, BinaryOp::Ne, &key_data)? {
        // Use a shortcut to update the data.
        let replace = attr(interp, &key, "_replace")?;
        let replaced = interp.call_function_expanded(
            replace,
            &[ExpandedCallArg {
                name: Some("data".to_string()),
                value: data.clone(),
            }],
        )?;
        store_key(interp, this, &replaced)?;
        return Ok(replaced);
    }
    Ok(key)
}

// ── `SelectSelector` ─────────────────────────────────────────────────────────

fn select_select(interp: &mut Interpreter, this: &Value, timeout: &Value) -> Result<Value> {
    let timeout = if timeout.is_none() {
        Value::none()
    } else {
        builtin(interp, "max", &[timeout.clone(), Value::int(0)])?
    };
    let readers = attr(interp, this, "_readers")?;
    let writers = attr(interp, this, "_writers")?;
    let hook = attr(interp, this, "_select")?;
    let outcome = call(interp, &hook, &[readers, writers, empty_list(), timeout]);
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            let (interrupted, error) = error_is(interp, error, &["InterruptedError"])?;
            return if interrupted {
                Ok(empty_list())
            } else {
                Err(error)
            };
        }
    };
    let lists = unpack(interp, &outcome, 3)?;
    let ready_read = builtin(interp, "set", &[lists[0].clone()])?;
    let ready_write = builtin(interp, "set", &[lists[1].clone()])?;
    let union = binary(interp, &ready_read, BinaryOp::BitOr, &ready_write)?;
    let mut ready = Vec::new();
    for fd in interp.collect_iterable(&union)? {
        let mut events = 0;
        if test(interp, &fd, BinaryOp::In, &ready_read)? {
            events |= EVENT_READ;
        }
        if test(interp, &fd, BinaryOp::In, &ready_write)? {
            events |= EVENT_WRITE;
        }
        let key = key_from_fd(interp, this, &fd)?;
        if interp.truthy_value(&key)? {
            let key_events = attr(interp, &key, "events")?;
            let masked = binary(interp, &Value::int(events), BinaryOp::BitAnd, &key_events)?;
            ready.push(Value::tuple(vec![key, masked]));
        }
    }
    Ok(Value::list(ready))
}

// ── `_PollLikeSelector` and friends ──────────────────────────────────────────

/// Translate `events` (`EVENT_READ | EVENT_WRITE`) into the poller's mask.
fn poller_events(interp: &mut Interpreter, this: &Value, events: &Value) -> Result<Value> {
    let mut mask = Value::int(0);
    for (flag, name) in [(EVENT_READ, "_EVENT_READ"), (EVENT_WRITE, "_EVENT_WRITE")] {
        if has_event(interp, events, &Value::int(flag))? {
            let bit = attr(interp, this, name)?;
            mask = binary(interp, &mask, BinaryOp::BitOr, &bit)?;
        }
    }
    Ok(mask)
}

/// `super(<class_name>, self).unregister(fileobj)` followed by re-raising
/// `error` — the body of a bare `except:` clause.
fn unregister_then_raise<T>(
    interp: &mut Interpreter,
    class_name: &str,
    this: &Value,
    fileobj: &Value,
    error: PyError,
) -> Result<T> {
    let unregister = super_method(interp, class_name, this, "unregister")?;
    call(interp, &unregister, std::slice::from_ref(fileobj))?;
    Err(error)
}

fn poll_register(
    interp: &mut Interpreter,
    this: &Value,
    fileobj: &Value,
    events: &Value,
    data: &Value,
) -> Result<Value> {
    let register = super_method(interp, "_PollLikeSelector", this, "register")?;
    let key = call(
        interp,
        &register,
        &[fileobj.clone(), events.clone(), data.clone()],
    )?;
    let mask = poller_events(interp, this, events)?;
    let poller = attr(interp, this, "_selector")?;
    let fd = attr(interp, &key, "fd")?;
    if let Err(error) = method(interp, &poller, "register", &[fd, mask]) {
        return unregister_then_raise(interp, "_PollLikeSelector", this, fileobj, error);
    }
    Ok(key)
}

fn poll_modify(
    interp: &mut Interpreter,
    this: &Value,
    fileobj: &Value,
    events: &Value,
    data: &Value,
) -> Result<Value> {
    let found = (|| {
        let fd = lookup(interp, this, fileobj)?;
        let map = fd_to_key(interp, this)?;
        interp.eval_index(&map, fd)
    })();
    let key = key_error_to_not_registered(interp, found, fileobj)?;
    let mut changed = false;
    let key_events = attr(interp, &key, "events")?;
    if test(interp, events, BinaryOp::Ne, &key_events)? {
        let mask = poller_events(interp, this, events)?;
        let poller = attr(interp, this, "_selector")?;
        let fd = attr(interp, &key, "fd")?;
        if let Err(error) = method(interp, &poller, "modify", &[fd, mask]) {
            return unregister_then_raise(interp, "_PollLikeSelector", this, fileobj, error);
        }
        changed = true;
    }
    let key_data = attr(interp, &key, "data")?;
    if test(interp, data, BinaryOp::Ne, &key_data)? {
        changed = true;
    }
    if !changed {
        return Ok(key);
    }
    let replace = attr(interp, &key, "_replace")?;
    let replaced = interp.call_function_expanded(
        replace,
        &[
            ExpandedCallArg {
                name: Some("events".to_string()),
                value: events.clone(),
            },
            ExpandedCallArg {
                name: Some("data".to_string()),
                value: data.clone(),
            },
        ],
    )?;
    store_key(interp, this, &replaced)?;
    Ok(replaced)
}

/// Convert a `(fd, event)` list from `poll()` into `[(key, events), ...]`.
fn poll_ready(
    interp: &mut Interpreter,
    this: &Value,
    fd_event_list: &Value,
    event_read: &Value,
    event_write: &Value,
) -> Result<Value> {
    let not_read = interp.eval_unary(event_read.clone(), UnaryOp::BitNot)?;
    let not_write = interp.eval_unary(event_write.clone(), UnaryOp::BitNot)?;
    let mut ready = Vec::new();
    for item in interp.collect_iterable(fd_event_list)? {
        let pair = unpack(interp, &item, 2)?;
        let (fd, event) = (&pair[0], &pair[1]);
        let mut events = 0;
        if has_event(interp, event, &not_read)? {
            events |= EVENT_WRITE;
        }
        if has_event(interp, event, &not_write)? {
            events |= EVENT_READ;
        }
        let key = key_from_fd(interp, this, fd)?;
        if interp.truthy_value(&key)? {
            let key_events = attr(interp, &key, "events")?;
            let masked = binary(interp, &Value::int(events), BinaryOp::BitAnd, &key_events)?;
            ready.push(Value::tuple(vec![key, masked]));
        }
    }
    Ok(Value::list(ready))
}

/// `math.ceil(timeout * 1e3)`, as the poll-style selectors round away from zero.
fn ceil_milliseconds(interp: &mut Interpreter, timeout: &Value) -> Result<Value> {
    let millis = binary(interp, timeout, BinaryOp::Mul, &Value::float(1e3))?;
    let math = module_global(interp, "math")?;
    method(interp, &math, "ceil", &[millis])
}

fn poll_select(interp: &mut Interpreter, this: &Value, timeout: &Value) -> Result<Value> {
    // This is shared between poll() and epoll().  epoll() has a different
    // signature and handling of timeout parameter.
    let timeout = if timeout.is_none() {
        Value::none()
    } else if test(interp, timeout, BinaryOp::Le, &Value::int(0))? {
        Value::int(0)
    } else {
        // poll() has a resolution of 1 millisecond, round away from zero to
        // wait *at least* timeout seconds.
        ceil_milliseconds(interp, timeout)?
    };
    let poller = attr(interp, this, "_selector")?;
    let listed = match method(interp, &poller, "poll", &[timeout]) {
        Ok(listed) => listed,
        Err(error) => return interrupted_or_raise(interp, error),
    };
    let event_read = attr(interp, this, "_EVENT_READ")?;
    let event_write = attr(interp, this, "_EVENT_WRITE")?;
    poll_ready(interp, this, &listed, &event_read, &event_write)
}

/// `except InterruptedError: return ready` (an empty list), else re-raise.
fn interrupted_or_raise(interp: &mut Interpreter, error: PyError) -> Result<Value> {
    let (interrupted, error) = error_is(interp, error, &["InterruptedError"])?;
    if interrupted {
        Ok(empty_list())
    } else {
        Err(error)
    }
}

fn epoll_select(interp: &mut Interpreter, this: &Value, timeout: &Value) -> Result<Value> {
    let timeout = if timeout.is_none() {
        Value::int(-1)
    } else if test(interp, timeout, BinaryOp::Le, &Value::int(0))? {
        Value::int(0)
    } else {
        // epoll_wait() has a resolution of 1 millisecond, round away from zero
        // to wait *at least* timeout seconds.
        let millis = ceil_milliseconds(interp, timeout)?;
        binary(interp, &millis, BinaryOp::Mul, &Value::float(1e-3))?
    };
    // epoll_wait() expects `maxevents` to be greater than zero; we want to make
    // sure that `select()` can be called when no FD is registered.
    let map = fd_to_key(interp, this)?;
    let registered = builtin(interp, "len", &[map])?;
    let max_events = builtin(interp, "max", &[registered, Value::int(1)])?;
    let poller = attr(interp, this, "_selector")?;
    let listed = match method(interp, &poller, "poll", &[timeout, max_events]) {
        Ok(listed) => listed,
        Err(error) => return interrupted_or_raise(interp, error),
    };
    let select = module_global(interp, "select")?;
    let epollin = attr(interp, &select, "EPOLLIN")?;
    let epollout = attr(interp, &select, "EPOLLOUT")?;
    poll_ready(interp, this, &listed, &epollin, &epollout)
}

/// `self._selector.close(); super().close()`.
fn close_poller_then_super(
    interp: &mut Interpreter,
    class_name: &str,
    this: &Value,
) -> Result<Value> {
    let poller = attr(interp, this, "_selector")?;
    method(interp, &poller, "close", &[])?;
    let close = super_method(interp, class_name, this, "close")?;
    call(interp, &close, &[])?;
    Ok(Value::none())
}

// ── `KqueueSelector` ─────────────────────────────────────────────────────────

/// `select.kevent(fd, select.<filter>, select.<flag>)`.
fn make_kevent(interp: &mut Interpreter, fd: &Value, filter: &str, flag: &str) -> Result<Value> {
    let select = module_global(interp, "select")?;
    let kevent = attr(interp, &select, "kevent")?;
    let filter = attr(interp, &select, filter)?;
    let flag = attr(interp, &select, flag)?;
    call(interp, &kevent, &[fd.clone(), filter, flag])
}

/// `self._selector.control([kev], 0, 0)`.
fn kqueue_control(interp: &mut Interpreter, this: &Value, kev: Value) -> Result<()> {
    let queue = attr(interp, this, "_selector")?;
    method(
        interp,
        &queue,
        "control",
        &[Value::list(vec![kev]), Value::int(0), Value::int(0)],
    )?;
    Ok(())
}

fn add_max_events(interp: &mut Interpreter, this: &Value, delta: i64) -> Result<()> {
    let current = attr(interp, this, "_max_events")?;
    let updated = binary(interp, &current, BinaryOp::Add, &Value::int(delta))?;
    set_attr(interp, this, "_max_events", updated)
}

fn kqueue_register_events(
    interp: &mut Interpreter,
    this: &Value,
    key: &Value,
    events: &Value,
) -> Result<()> {
    let fd = attr(interp, key, "fd")?;
    for (flag, filter) in [
        (EVENT_READ, "KQ_FILTER_READ"),
        (EVENT_WRITE, "KQ_FILTER_WRITE"),
    ] {
        if has_event(interp, events, &Value::int(flag))? {
            let kev = make_kevent(interp, &fd, filter, "KQ_EV_ADD")?;
            kqueue_control(interp, this, kev)?;
            add_max_events(interp, this, 1)?;
        }
    }
    Ok(())
}

fn kqueue_unregister(interp: &mut Interpreter, this: &Value, fileobj: &Value) -> Result<Value> {
    let unregister = super_method(interp, "KqueueSelector", this, "unregister")?;
    let key = call(interp, &unregister, std::slice::from_ref(fileobj))?;
    let fd = attr(interp, &key, "fd")?;
    let events = attr(interp, &key, "events")?;
    for (flag, filter) in [
        (EVENT_READ, "KQ_FILTER_READ"),
        (EVENT_WRITE, "KQ_FILTER_WRITE"),
    ] {
        if has_event(interp, &events, &Value::int(flag))? {
            let kev = make_kevent(interp, &fd, filter, "KQ_EV_DELETE")?;
            add_max_events(interp, this, -1)?;
            if let Err(error) = kqueue_control(interp, this, kev) {
                // This can happen if the FD was closed since it was registered.
                let (os_error, error) = error_is(interp, error, &["OSError"])?;
                if !os_error {
                    return Err(error);
                }
            }
        }
    }
    Ok(key)
}

fn kqueue_select(interp: &mut Interpreter, this: &Value, timeout: &Value) -> Result<Value> {
    let timeout = if timeout.is_none() {
        Value::none()
    } else {
        builtin(interp, "max", &[timeout.clone(), Value::int(0)])?
    };
    // If max_ev is 0, kqueue will ignore the timeout.  For consistent behavior
    // with the other selector classes, we prevent that here (using max).
    let max_events = attr(interp, this, "_max_events")?;
    let max_events = if interp.truthy_value(&max_events)? {
        max_events
    } else {
        Value::int(1)
    };
    let queue = attr(interp, this, "_selector")?;
    let listed = match method(
        interp,
        &queue,
        "control",
        &[Value::none(), max_events, timeout],
    ) {
        Ok(listed) => listed,
        Err(error) => return interrupted_or_raise(interp, error),
    };
    let select = module_global(interp, "select")?;
    let filter_read = attr(interp, &select, "KQ_FILTER_READ")?;
    let filter_write = attr(interp, &select, "KQ_FILTER_WRITE")?;
    let mut ready = Vec::new();
    for kev in interp.collect_iterable(&listed)? {
        let fd = attr(interp, &kev, "ident")?;
        let flag = attr(interp, &kev, "filter")?;
        let mut events = 0;
        if test(interp, &flag, BinaryOp::Eq, &filter_read)? {
            events |= EVENT_READ;
        }
        if test(interp, &flag, BinaryOp::Eq, &filter_write)? {
            events |= EVENT_WRITE;
        }
        let key = key_from_fd(interp, this, &fd)?;
        if interp.truthy_value(&key)? {
            let key_events = attr(interp, &key, "events")?;
            let masked = binary(interp, &Value::int(events), BinaryOp::BitAnd, &key_events)?;
            ready.push(Value::tuple(vec![key, masked]));
        }
    }
    Ok(Value::list(ready))
}

// ── `_can_use` ───────────────────────────────────────────────────────────────

/// Check if we can use the selector depending upon the operating system.
fn can_use(interp: &mut Interpreter, method_name: &Value) -> Result<bool> {
    let select = module_global(interp, "select")?;
    let selector = builtin(
        interp,
        "getattr",
        &[select, method_name.clone(), Value::none()],
    )?;
    if selector.is_none() {
        // select module does not implement method
        return Ok(false);
    }
    // Check if the OS and Kernel actually support the method.  The call may
    // fail with `OSError: [Errno 38] Function not implemented`.
    let attempt = (|| -> Result<()> {
        let selector_obj = call(interp, &selector, &[])?;
        if test(interp, method_name, BinaryOp::Eq, &Value::string("poll"))? {
            // check that poll actually works
            method(interp, &selector_obj, "poll", &[Value::int(0)])?;
        } else {
            // close epoll, kqueue, and devpoll fd
            method(interp, &selector_obj, "close", &[])?;
        }
        Ok(())
    })();
    match attempt {
        Ok(()) => Ok(true),
        Err(error) => {
            let (os_error, error) = error_is(interp, error, &["OSError"])?;
            if os_error { Ok(false) } else { Err(error) }
        }
    }
}

// ── Post-import wiring ───────────────────────────────────────────────────────

fn class_of(module: &Rc<RefCell<PyModule>>, name: &str) -> Option<Value> {
    module.borrow().attrs.get(name).cloned()
}

fn reorder<S: std::hash::BuildHasher>(attrs: &mut IndexMap<String, Value, S>, order: &[&str]) {
    for name in order {
        if let Some(value) = attrs.shift_remove(*name) {
            attrs.insert((*name).to_string(), value);
        }
    }
}

/// Wrap `owner.name` (a native function) in an abstract-method descriptor.
fn wrap_abstract(
    interp: &mut Interpreter,
    abstract_cls: &Value,
    bound_cls: &Value,
    owner: &str,
    name: &str,
    func: Value,
) -> Result<Value> {
    let doc = interp
        .get_attr(&func, "__doc__")
        .unwrap_or_else(|_| Value::none());
    call(
        interp,
        abstract_cls,
        &[
            func,
            bound_cls.clone(),
            Value::string(name),
            Value::string(format!("{owner}.{name}")),
            doc,
        ],
    )
}

/// The module-level statements of `selectors.py` that need the interpreter.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let abc = interp.load_module("abc")?;
    let abc_meta_value = interp.get_attr(&abc, "ABCMeta")?;
    let abstractmethod = interp.get_attr(&abc, "abstractmethod")?;
    let update_abstractmethods = interp.get_attr(&abc, "update_abstractmethods")?;
    let collections = interp.load_module("collections")?;
    let namedtuple = interp.get_attr(&collections, "namedtuple")?;
    let collections_abc = interp.load_module("collections.abc")?;
    let mapping_value = interp.get_attr(&collections_abc, "Mapping")?;
    let math = interp.load_module("math")?;
    let select = interp.load_module("select")?;
    let sys = interp.load_module("sys")?;
    let (ValueKind::PyClass(abc_meta), ValueKind::PyClass(mapping)) =
        (abc_meta_value.kind(), mapping_value.kind())
    else {
        return Err(PyError::runtime(
            "selectors: abc.ABCMeta / collections.abc.Mapping are not classes".to_string(),
        ));
    };

    // SelectorKey = namedtuple('SelectorKey', ['fileobj', 'fd', 'events', 'data'])
    let fields = Value::list(
        ["fileobj", "fd", "events", "data"]
            .into_iter()
            .map(Value::string)
            .collect(),
    );
    let selector_key = call(interp, &namedtuple, &[Value::string("SelectorKey"), fields])?;
    set_attr(
        interp,
        &selector_key,
        "__module__",
        Value::string("selectors"),
    )?;
    set_attr(
        interp,
        &selector_key,
        "__doc__",
        Value::string(
            "SelectorKey(fileobj, fd, events, data)\n\n    \
             Object used to associate a file object to its backing\n    \
             file descriptor, selected event mask, and attached data.\n",
        ),
    )?;
    for (field, doc) in [
        ("fileobj", "File object registered."),
        ("fd", "Underlying file descriptor."),
        (
            "events",
            "Events that must be waited for on this file object.",
        ),
        (
            "data",
            "Optional opaque data associated to this file object.\n\
             For example, this could be used to store a per-client session ID.",
        ),
    ] {
        // Field accessors that cannot carry a docstring keep the default one.
        if let Ok(descriptor) = interp.get_attr(&selector_key, field) {
            let _ = set_attr(interp, &descriptor, "__doc__", Value::string(doc));
        }
    }

    // `if hasattr(select, 'poll'): class PollSelector ...` and friends.
    for &(class_name, select_attr, event_in, event_out) in PLATFORM_SELECTORS {
        let Some(class_value) = class_of(module, class_name) else {
            continue;
        };
        let ValueKind::PyClass(class) = class_value.kind() else {
            continue;
        };
        let Ok(factory) = interp.get_attr(&select, select_attr) else {
            module.borrow_mut().attrs.shift_remove(class_name);
            continue;
        };
        if !event_in.is_empty() {
            let readable = interp.get_attr(&select, event_in)?;
            let writable = interp.get_attr(&select, event_out)?;
            let mut class = class.borrow_mut();
            class.attrs.insert("_selector_cls".to_string(), factory);
            class.attrs.insert("_EVENT_READ".to_string(), readable);
            class.attrs.insert("_EVENT_WRITE".to_string(), writable);
            class.bump_mutation_version();
        }
    }

    // `_select = select.select` (every platform but Windows, where the
    // `_select` method of `SelectSelector` is used instead).
    #[cfg(not(windows))]
    {
        if let Some(class_value) = class_of(module, "SelectSelector")
            && let ValueKind::PyClass(class) = class_value.kind()
        {
            let function = interp.get_attr(&select, "select")?;
            let mut class = class.borrow_mut();
            class.attrs.insert("_select".to_string(), function);
            class.bump_mutation_version();
        }
    }

    // Abstract methods of `BaseSelector` (`@abstractmethod`).
    let take = |name: &str| -> Result<Value> {
        class_of(module, name).ok_or_else(|| PyError::runtime(format!("selectors: missing {name}")))
    };
    let abstract_cls = take("_AbstractMethod")?;
    let bound_cls = take("_BoundAbstractMethod")?;
    if let ValueKind::PyClass(base) = take("BaseSelector")?.kind() {
        for name in ["register", "unregister", "select", "get_map"] {
            let func = base.borrow().attrs.get(name).cloned();
            let Some(func) = func else { continue };
            let wrapped = wrap_abstract(
                interp,
                &abstract_cls,
                &bound_cls,
                "BaseSelector",
                name,
                func,
            )?;
            base.borrow_mut().attrs.insert(name.to_string(), wrapped);
        }
    }

    // `class _SelectorMapping(Mapping)` and `metaclass=ABCMeta`.
    if let ValueKind::PyClass(selector_mapping) = take("_SelectorMapping")?.kind() {
        link_class_bases(selector_mapping, vec![Rc::clone(mapping)]);
    }
    for &name in ABC_CLASSES {
        let Some(class_value) = class_of(module, name) else {
            continue;
        };
        let ValueKind::PyClass(class) = class_value.kind() else {
            continue;
        };
        {
            let mut class = class.borrow_mut();
            class.metatype = Some(Rc::clone(abc_meta));
            class.attrs.insert(
                "__abstractmethods__".to_string(),
                pyrust_builtins::frozenset::frozenset(PySet::default()),
            );
            class.attrs.insert(
                "_abc_impl".to_string(),
                crate::builtin_modules::abc::new_abc_impl(),
            );
            class.bump_mutation_version();
        }
        call(
            interp,
            &update_abstractmethods,
            std::slice::from_ref(&class_value),
        )?;
    }

    {
        let mut module = module.borrow_mut();
        module.attrs.shift_remove("_AbstractMethod");
        module.attrs.shift_remove("_BoundAbstractMethod");
        for (name, value) in [
            ("ABCMeta", abc_meta_value.clone()),
            ("abstractmethod", abstractmethod),
            ("namedtuple", namedtuple),
            ("Mapping", mapping_value.clone()),
            ("math", math),
            ("select", select),
            ("sys", sys),
            ("SelectorKey", selector_key),
        ] {
            module.attrs.insert(name.to_string(), value);
        }
    }

    // Choose the best implementation, roughly: epoll|kqueue|devpoll > poll >
    // select.  select() also can't accept a FD > FD_SETSIZE (usually around
    // 1024).
    let mut default_selector = None;
    for (method_name, class_name) in [
        ("kqueue", "KqueueSelector"),
        ("epoll", "EpollSelector"),
        ("devpoll", "DevpollSelector"),
        ("poll", "PollSelector"),
    ] {
        if can_use(interp, &Value::string(method_name))?
            && let Some(class) = class_of(module, class_name)
        {
            default_selector = Some(class);
            break;
        }
    }
    let default_selector = match default_selector {
        Some(class) => class,
        None => take("SelectSelector")?,
    };
    let mut module = module.borrow_mut();
    module
        .attrs
        .insert("DefaultSelector".to_string(), default_selector);
    reorder(&mut module.attrs, MODULE_ORDER);
    Ok(())
}

// ── Abstract-method descriptors (hidden helper classes) ──────────────────────

/// State of an abstract-method descriptor: the wrapped native function, the
/// bound-method class to build on instance access, and the metadata a Python
/// function would expose.
struct AbstractMethodState {
    func: Value,
    bound_cls: Value,
    name: String,
    qualname: String,
    doc: Value,
}

struct BoundAbstractMethodState {
    func: Value,
    instance: Value,
}

fn call_with_args_kwargs(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &IndexMap<String, Value>,
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    for (name, value) in kw {
        expanded.push(ExpandedCallArg {
            name: Some(name.clone()),
            value: value.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "EVENT_READ" => Value::int(EVENT_READ),
        "EVENT_WRITE" => Value::int(EVENT_WRITE),
    }

    /// Return a file descriptor from a file object.
    fn _fileobj_to_fd(args) -> Result<Value> {
        let mut bound = args::bind(&SIG_FILEOBJ_TO_FD, args)?;
        let fileobj = bound.take_or_none(0);
        fileobj_to_fd(_interp, &fileobj)
    }

    /// Check if we can use the selector depending upon the operating system.
    fn _can_use(args) -> Result<Value> {
        let mut bound = args::bind(&SIG_CAN_USE, args)?;
        let method_name = bound.take_or_none(0);
        Ok(Value::bool_(can_use(_interp, &method_name)?))
    }

    #[native_state(AbstractMethodState)]
    class _AbstractMethod {
        constants {
            "__isabstractmethod__" => Value::bool_(true),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValue,
            bound_cls: PyValue,
            name: PyValue,
            qualname: PyValue,
            doc: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                AbstractMethodState {
                    func: func.0,
                    bound_cls: bound_cls.0,
                    name: name.0.as_str().unwrap_or_default().to_string(),
                    qualname: qualname.0.as_str().unwrap_or_default().to_string(),
                    doc: doc.0,
                },
            )?;
            Ok(Value::none())
        }

        fn __get__(
            #[positional_only] self_val: PyValueRef<'_>,
            obj: PyValue,
            #[default(OptionalArg::Omitted)] _objtype: OptValueRef<'_>,
        ) -> Result<Value> {
            if obj.0.is_none() {
                return Ok(self_val.0.clone());
            }
            let (func, bound_cls) =
                with_state(self_val.0, |st| Ok((st.func.clone(), st.bound_cls.clone())))?;
            call(_interp, &bound_cls, &[func, obj.0])
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let func = with_state(self_val.0, |st| Ok(st.func.clone()))?;
            call_with_args_kwargs(_interp, func, args, &kwargs)
        }

        #[getter]
        fn __name__(#[state] st: &AbstractMethodState) -> Result<Value> {
            Ok(Value::string(&st.name))
        }

        #[getter]
        fn __qualname__(#[state] st: &AbstractMethodState) -> Result<Value> {
            Ok(Value::string(&st.qualname))
        }

        #[getter]
        fn __doc__(#[state] st: &AbstractMethodState) -> Result<Value> {
            Ok(st.doc.clone())
        }
    }

    #[native_state(BoundAbstractMethodState)]
    class _BoundAbstractMethod {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValue,
            instance: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                BoundAbstractMethodState {
                    func: func.0,
                    instance: instance.0,
                },
            )?;
            Ok(Value::none())
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let (func, inst) =
                with_state(self_val.0, |st| Ok((st.func.clone(), st.instance.clone())))?;
            let mut full_pos = Vec::with_capacity(args.len() + 1);
            full_pos.push(inst);
            full_pos.extend_from_slice(args);
            call_with_args_kwargs(_interp, func, &full_pos, &kwargs)
        }
    }

    class _SelectorMapping {
        constants {
            "__doc__" => Value::string("Mapping of file objects to selector keys."),
        }

        fn __init__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_MAPPING_INIT, args)?;
            let this = bound.take_or_none(0);
            let selector = bound.take_or_none(1);
            set_attr(_interp, &this, "_selector", selector)?;
            Ok(Value::none())
        }

        fn __len__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_MAPPING_LEN, args)?;
            let this = bound.take_or_none(0);
            let selector = attr(_interp, &this, "_selector")?;
            let map = fd_to_key(_interp, &selector)?;
            builtin(_interp, "len", &[map])
        }

        fn __getitem__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_MAPPING_GETITEM, args)?;
            let this = bound.take_or_none(0);
            let fileobj = bound.take_or_none(1);
            let found = (|| {
                let selector = attr(_interp, &this, "_selector")?;
                let fd = lookup(_interp, &selector, &fileobj)?;
                let map = fd_to_key(_interp, &selector)?;
                _interp.eval_index(&map, fd)
            })();
            key_error_to_not_registered(_interp, found, &fileobj)
        }

        fn __iter__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_MAPPING_ITER, args)?;
            let this = bound.take_or_none(0);
            let selector = attr(_interp, &this, "_selector")?;
            let map = fd_to_key(_interp, &selector)?;
            builtin(_interp, "iter", &[map])
        }
    }

    class BaseSelector {
        constants {
            "__doc__" => Value::string(
                "Selector abstract base class.\n\n    \
                 A selector supports registering file objects to be monitored for specific\n    \
                 I/O events.\n\n    \
                 A file object is a file descriptor or any object with a `fileno()` method.\n    \
                 An arbitrary object can be attached to the file object, which can be used\n    \
                 for example to store context information, a callback, etc.\n\n    \
                 A selector can use various implementations (select(), poll(), epoll()...)\n    \
                 depending on the platform. The default `Selector` class uses the most\n    \
                 efficient implementation on the current platform.\n    "
            ),
        }

        /// Register a file object.
        fn register(args) -> Result<Value> {
            args::bind(&SIG_BASE_REGISTER, args)?;
            not_implemented()
        }

        /// Unregister a file object.
        fn unregister(args) -> Result<Value> {
            args::bind(&SIG_BASE_UNREGISTER, args)?;
            not_implemented()
        }

        /// Change a registered file object monitored events or attached data.
        fn modify(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_BASE_MODIFY, args)?;
            let this = bound.take_or_none(0);
            let fileobj = bound.take_or_none(1);
            let events = bound.take_or_none(2);
            let data = bound.take_or_none(3);
            method(_interp, &this, "unregister", std::slice::from_ref(&fileobj))?;
            method(_interp, &this, "register", &[fileobj, events, data])
        }

        /// Perform the actual selection, until some monitored file objects are
        /// ready or a timeout expires.
        fn select(args) -> Result<Value> {
            args::bind(&SIG_BASE_SELECT, args)?;
            not_implemented()
        }

        /// Close the selector.
        fn close(args) -> Result<Value> {
            args::bind(&SIG_BASE_CLOSE, args)?;
            Ok(Value::none())
        }

        /// Return the key associated to a registered file object.
        fn get_key(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_BASE_GET_KEY, args)?;
            let this = bound.take_or_none(0);
            let fileobj = bound.take_or_none(1);
            let mapping = method(_interp, &this, "get_map", &[])?;
            if mapping.is_none() {
                return Err(PyError::named("RuntimeError", "Selector is closed"));
            }
            let found = _interp.eval_index(&mapping, fileobj.clone());
            key_error_to_not_registered(_interp, found, &fileobj)
        }

        /// Return a mapping of file objects to selector keys.
        fn get_map(args) -> Result<Value> {
            args::bind(&SIG_BASE_GET_MAP, args)?;
            not_implemented()
        }

        fn __enter__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_BASE_ENTER, args)?;
            Ok(bound.take_or_none(0))
        }

        fn __exit__(args) -> Result<Value> {
            let this = args.first().map(|a| a.value.clone()).unwrap_or_else(Value::none);
            method(_interp, &this, "close", &[])?;
            Ok(Value::none())
        }
    }

    class _BaseSelectorImpl("BaseSelector") {
        constants {
            "__doc__" => Value::string("Base selector implementation."),
        }

        fn __init__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_IMPL_INIT, args)?;
            let this = bound.take_or_none(0);
            // this maps file descriptors to keys
            set_attr(_interp, &this, "_fd_to_key", Value::dict(PyDict::default()))?;
            // read-only mapping returned by get_map()
            let mapping_class = module_global(_interp, "_SelectorMapping")?;
            let mapping = call(_interp, &mapping_class, std::slice::from_ref(&this))?;
            set_attr(_interp, &this, "_map", mapping)?;
            Ok(Value::none())
        }

        /// Return a file descriptor from a file object.
        ///
        /// This wraps _fileobj_to_fd() to do an exhaustive search in case
        /// the object is invalid but we still have it in our map.  This
        /// is used by unregister() so we can unregister an object that
        /// was previously registered even if it is closed.  It is also
        /// used by _SelectorMapping.
        fn _fileobj_lookup(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_IMPL_LOOKUP, args)?;
            let this = bound.take_or_none(0);
            let fileobj = bound.take_or_none(1);
            let to_fd = module_global(_interp, "_fileobj_to_fd")?;
            match call(_interp, &to_fd, std::slice::from_ref(&fileobj)) {
                Ok(fd) => Ok(fd),
                Err(error) => {
                    let (invalid, error) = error_is(_interp, error, &["ValueError"])?;
                    if !invalid {
                        return Err(error);
                    }
                    // Do an exhaustive search.
                    let map = fd_to_key(_interp, &this)?;
                    let keys = method(_interp, &map, "values", &[])?;
                    for key in _interp.collect_iterable(&keys)? {
                        let registered = attr(_interp, &key, "fileobj")?;
                        if is_same(_interp, &registered, &fileobj)? {
                            return attr(_interp, &key, "fd");
                        }
                    }
                    // Raise ValueError after all.
                    Err(error)
                }
            }
        }

        fn register(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_IMPL_REGISTER, args)?;
            let this = bound.take_or_none(0);
            let fileobj = bound.take_or_none(1);
            let events = bound.take_or_none(2);
            let data = bound.take_or_none(3);
            impl_register(_interp, &this, &fileobj, &events, &data)
        }

        fn unregister(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_IMPL_UNREGISTER, args)?;
            let this = bound.take_or_none(0);
            let fileobj = bound.take_or_none(1);
            impl_unregister(_interp, &this, &fileobj)
        }

        fn modify(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_IMPL_MODIFY, args)?;
            let this = bound.take_or_none(0);
            let fileobj = bound.take_or_none(1);
            let events = bound.take_or_none(2);
            let data = bound.take_or_none(3);
            impl_modify(_interp, &this, &fileobj, &events, &data)
        }

        fn close(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_IMPL_CLOSE, args)?;
            let this = bound.take_or_none(0);
            let map = fd_to_key(_interp, &this)?;
            method(_interp, &map, "clear", &[])?;
            set_attr(_interp, &this, "_map", Value::none())?;
            Ok(Value::none())
        }

        fn get_map(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_IMPL_GET_MAP, args)?;
            let this = bound.take_or_none(0);
            attr(_interp, &this, "_map")
        }

        /// Return the key associated to a given file descriptor.
        ///
        /// Parameters:
        /// fd -- file descriptor
        ///
        /// Returns:
        /// corresponding key, or None if not found
        fn _key_from_fd(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_IMPL_KEY_FROM_FD, args)?;
            let this = bound.take_or_none(0);
            let fd = bound.take_or_none(1);
            let map = fd_to_key(_interp, &this)?;
            match _interp.eval_index(&map, fd) {
                Ok(key) => Ok(key),
                Err(error) => {
                    let (missing, error) = error_is(_interp, error, &["KeyError"])?;
                    if missing { Ok(Value::none()) } else { Err(error) }
                }
            }
        }
    }

    class SelectSelector("_BaseSelectorImpl") {
        constants {
            "__doc__" => Value::string("Select-based selector."),
        }

        fn __init__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_SELECT_INIT, args)?;
            let this = bound.take_or_none(0);
            let init = super_method(_interp, "SelectSelector", &this, "__init__")?;
            call(_interp, &init, &[])?;
            let readers = builtin(_interp, "set", &[])?;
            set_attr(_interp, &this, "_readers", readers)?;
            let writers = builtin(_interp, "set", &[])?;
            set_attr(_interp, &this, "_writers", writers)?;
            Ok(Value::none())
        }

        fn register(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_SELECT_REGISTER, args)?;
            let this = bound.take_or_none(0);
            let fileobj = bound.take_or_none(1);
            let events = bound.take_or_none(2);
            let data = bound.take_or_none(3);
            let register = super_method(_interp, "SelectSelector", &this, "register")?;
            let key = call(_interp, &register, &[fileobj, events.clone(), data])?;
            for (flag, set_name) in [(EVENT_READ, "_readers"), (EVENT_WRITE, "_writers")] {
                if has_event(_interp, &events, &Value::int(flag))? {
                    let set = attr(_interp, &this, set_name)?;
                    let fd = attr(_interp, &key, "fd")?;
                    method(_interp, &set, "add", &[fd])?;
                }
            }
            Ok(key)
        }

        fn unregister(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_SELECT_UNREGISTER, args)?;
            let this = bound.take_or_none(0);
            let fileobj = bound.take_or_none(1);
            let unregister = super_method(_interp, "SelectSelector", &this, "unregister")?;
            let key = call(_interp, &unregister, &[fileobj])?;
            for set_name in ["_readers", "_writers"] {
                let set = attr(_interp, &this, set_name)?;
                let fd = attr(_interp, &key, "fd")?;
                method(_interp, &set, "discard", &[fd])?;
            }
            Ok(key)
        }

        #[cfg(windows)]
        fn _select(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_SELECT_SELECT_HOOK, args)?;
            let _this = bound.take_or_none(0);
            let readers = bound.take_or_none(1);
            let writers = bound.take_or_none(2);
            let _ = bound.take(3);
            let timeout = bound.take_or_none(4);
            // Winsock reports failed connects through the exceptional set.
            let select = module_global(_interp, "select")?;
            let outcome = method(
                _interp,
                &select,
                "select",
                &[readers, writers.clone(), writers, timeout],
            )?;
            let lists = unpack(_interp, &outcome, 3)?;
            let writable = binary(_interp, &lists[1], BinaryOp::Add, &lists[2])?;
            Ok(Value::tuple(vec![lists[0].clone(), writable, empty_list()]))
        }

        fn select(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_SELECT_SELECT, args)?;
            let this = bound.take_or_none(0);
            let timeout = bound.take_or_none(1);
            select_select(_interp, &this, &timeout)
        }
    }

    class _PollLikeSelector("_BaseSelectorImpl") {
        constants {
            "__doc__" => Value::string(
                "Base class shared between poll, epoll and devpoll selectors."
            ),
            "_selector_cls" => Value::none(),
            "_EVENT_READ" => Value::none(),
            "_EVENT_WRITE" => Value::none(),
        }

        fn __init__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_POLL_INIT, args)?;
            let this = bound.take_or_none(0);
            let init = super_method(_interp, "_PollLikeSelector", &this, "__init__")?;
            call(_interp, &init, &[])?;
            let factory = attr(_interp, &this, "_selector_cls")?;
            let poller = call(_interp, &factory, &[])?;
            set_attr(_interp, &this, "_selector", poller)?;
            Ok(Value::none())
        }

        fn register(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_POLL_REGISTER, args)?;
            let this = bound.take_or_none(0);
            let fileobj = bound.take_or_none(1);
            let events = bound.take_or_none(2);
            let data = bound.take_or_none(3);
            poll_register(_interp, &this, &fileobj, &events, &data)
        }

        fn unregister(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_POLL_UNREGISTER, args)?;
            let this = bound.take_or_none(0);
            let fileobj = bound.take_or_none(1);
            let unregister = super_method(_interp, "_PollLikeSelector", &this, "unregister")?;
            let key = call(_interp, &unregister, &[fileobj])?;
            let poller = attr(_interp, &this, "_selector")?;
            let fd = attr(_interp, &key, "fd")?;
            if let Err(error) = method(_interp, &poller, "unregister", &[fd]) {
                // This can happen if the FD was closed since it was registered.
                let (os_error, error) = error_is(_interp, error, &["OSError"])?;
                if !os_error {
                    return Err(error);
                }
            }
            Ok(key)
        }

        fn modify(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_POLL_MODIFY, args)?;
            let this = bound.take_or_none(0);
            let fileobj = bound.take_or_none(1);
            let events = bound.take_or_none(2);
            let data = bound.take_or_none(3);
            poll_modify(_interp, &this, &fileobj, &events, &data)
        }

        fn select(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_POLL_SELECT, args)?;
            let this = bound.take_or_none(0);
            let timeout = bound.take_or_none(1);
            poll_select(_interp, &this, &timeout)
        }
    }

    class PollSelector("_PollLikeSelector") {
        constants {
            "__doc__" => Value::string("Poll-based selector."),
        }
    }

    class EpollSelector("_PollLikeSelector") {
        constants {
            "__doc__" => Value::string("Epoll-based selector."),
        }

        fn fileno(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_EPOLL_FILENO, args)?;
            let this = bound.take_or_none(0);
            let poller = attr(_interp, &this, "_selector")?;
            method(_interp, &poller, "fileno", &[])
        }

        fn select(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_EPOLL_SELECT, args)?;
            let this = bound.take_or_none(0);
            let timeout = bound.take_or_none(1);
            epoll_select(_interp, &this, &timeout)
        }

        fn close(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_EPOLL_CLOSE, args)?;
            let this = bound.take_or_none(0);
            close_poller_then_super(_interp, "EpollSelector", &this)
        }
    }

    class KqueueSelector("_BaseSelectorImpl") {
        constants {
            "__doc__" => Value::string("Kqueue-based selector."),
        }

        fn __init__(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_KQUEUE_INIT, args)?;
            let this = bound.take_or_none(0);
            let init = super_method(_interp, "KqueueSelector", &this, "__init__")?;
            call(_interp, &init, &[])?;
            let select = module_global(_interp, "select")?;
            let kqueue = attr(_interp, &select, "kqueue")?;
            let queue = call(_interp, &kqueue, &[])?;
            set_attr(_interp, &this, "_selector", queue)?;
            set_attr(_interp, &this, "_max_events", Value::int(0))?;
            Ok(Value::none())
        }

        fn fileno(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_KQUEUE_FILENO, args)?;
            let this = bound.take_or_none(0);
            let queue = attr(_interp, &this, "_selector")?;
            method(_interp, &queue, "fileno", &[])
        }

        fn register(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_KQUEUE_REGISTER, args)?;
            let this = bound.take_or_none(0);
            let fileobj = bound.take_or_none(1);
            let events = bound.take_or_none(2);
            let data = bound.take_or_none(3);
            let register = super_method(_interp, "KqueueSelector", &this, "register")?;
            let key = call(_interp, &register, &[fileobj.clone(), events.clone(), data])?;
            if let Err(error) = kqueue_register_events(_interp, &this, &key, &events) {
                return unregister_then_raise(_interp, "KqueueSelector", &this, &fileobj, error);
            }
            Ok(key)
        }

        fn unregister(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_KQUEUE_UNREGISTER, args)?;
            let this = bound.take_or_none(0);
            let fileobj = bound.take_or_none(1);
            kqueue_unregister(_interp, &this, &fileobj)
        }

        fn select(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_KQUEUE_SELECT, args)?;
            let this = bound.take_or_none(0);
            let timeout = bound.take_or_none(1);
            kqueue_select(_interp, &this, &timeout)
        }

        fn close(args) -> Result<Value> {
            let mut bound = args::bind(&SIG_KQUEUE_CLOSE, args)?;
            let this = bound.take_or_none(0);
            close_poller_then_super(_interp, "KqueueSelector", &this)
        }
    }
}
