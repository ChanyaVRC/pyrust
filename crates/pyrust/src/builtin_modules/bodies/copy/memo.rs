// Object identity, memo table helpers, `copy.Error` generation tracking, and
// native recursion guard for `copy.deepcopy`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use crate::error::{PyError, Result};
use crate::value::{PyClass, PyKey, Value, ValueKind};
use indexmap::IndexMap;

// Thread-local justification:
// (a) Why `thread_local!`: `COPY_ERROR_CLASSES` holds `!Send`
//     `Weak<RefCell<PyClass>>` generations and `DEEPCOPY_DEPTH` tracks the
//     active native call-stack recursion depth for `copy.deepcopy` on the
//     current OS thread.
// (b) Multi-interpreter / multi-threaded impact: Weak `copy.Error` entries
//     isolate distinct import generations across interpreters on the same
//     thread; `DEEPCOPY_DEPTH` guards total native stack depth across nested
//     calls on one thread while separate threads are completely independent.
thread_local! {
    /// `copy` is a Python module in CPython, so each fresh import generation
    /// owns a distinct Error class. Weak storage keeps old classes alive only
    /// while an old module/class/instance still references them.
    static COPY_ERROR_CLASSES: RefCell<Vec<Weak<RefCell<PyClass>>>> =
        const { RefCell::new(Vec::new()) };

    /// Active `copy.deepcopy` recursion depth, guarding against unbounded
    /// native recursion on unmemoized pre-construction cycles such as a
    /// self-referential `Counter` (#2968).
    static DEEPCOPY_DEPTH: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn new_copy_error_class_value() -> Value {
    let exception_base = crate::interpreter::lookup_exc_class("Exception")
        .expect("EXC_CLASS_CACHE must contain Exception");
    let mut attrs = IndexMap::new();
    attrs.insert("__module__".to_string(), Value::string("copy"));
    let class = Rc::new(RefCell::new(PyClass::new(
        "Error",
        "Error",
        Some(Rc::clone(&exception_base)),
        attrs,
    )));
    exception_base
        .borrow()
        .subclasses
        .borrow_mut()
        .push(Rc::downgrade(&class));
    COPY_ERROR_CLASSES.with(|classes| {
        let mut classes = classes.borrow_mut();
        classes.retain(|registered| registered.strong_count() > 0);
        classes.push(Rc::downgrade(&class));
    });
    Value::py_class(class)
}

pub(super) fn current_copy_error_class_value() -> Value {
    let class = COPY_ERROR_CLASSES.with(|classes| {
        let mut classes = classes.borrow_mut();
        classes.retain(|registered| registered.strong_count() > 0);
        classes.iter().rev().find_map(Weak::upgrade)
    });
    class.map_or_else(new_copy_error_class_value, Value::py_class)
}

pub(super) struct DeepCopyDepthGuard;

impl DeepCopyDepthGuard {
    pub(super) fn enter(interp: &crate::interpreter::Interpreter) -> Result<Self> {
        // Cap below the native debug stack ceiling while still honouring
        // smaller user-configured `sys.setrecursionlimit()` values.
        const NATIVE_DEEPCOPY_CAP: usize = 200;
        let limit = crate::interpreter::get_recursion_limit(interp).min(NATIVE_DEEPCOPY_CAP);
        let next = DEEPCOPY_DEPTH.with(|depth| {
            let next = depth.get() + 1;
            if next > limit {
                return Err(PyError::named(
                    "RecursionError",
                    "maximum recursion depth exceeded".to_string(),
                ));
            }
            depth.set(next);
            Ok(next)
        })?;
        let _ = next;
        Ok(Self)
    }
}

impl Drop for DeepCopyDepthGuard {
    fn drop(&mut self) {
        DEEPCOPY_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Object identity used as the `memo` key (`id(x)`).  Uses the single canonical
/// `Value::object_id` encoding shared with Python's `id()` and `is`, so
/// identity-proxied built-in views such as `obj.__dict__` (`InstanceDict`)
/// memoise under a stable identity distinct from `obj` itself (#2946).
/// Returns `None` for atomic scalar values that are returned as-is anyway.
pub(super) fn value_identity(obj: &Value) -> Option<i64> {
    match obj.kind() {
        ValueKind::None
        | ValueKind::Bool(_)
        | ValueKind::Int(_)
        | ValueKind::Float(_)
        | ValueKind::Complex(_, _) => None,
        _ => obj.object_id().as_int(),
    }
}

/// Did `deep_copy(orig)` leave the element unchanged (CPython's `k is j`)?
pub(super) fn deepcopy_kept_identity(orig: &Value, copied: &Value) -> bool {
    orig.is_identical_to(copied)
}

/// Look up an already-copied object in the memo by identity.
pub(super) fn memo_get(memo: &Value, id: i64) -> Option<Value> {
    memo.dict_with(|d| d.get(&PyKey::Int(id)).cloned())
        .flatten()
}

/// Record `copy` as the deep copy of the object with identity `id`.
pub(super) fn memo_insert(memo: &Value, id: i64, copy: Value) {
    let _ = memo.dict_insert(PyKey::Int(id), copy);
}

/// Record `copy` in `memo` under `obj`'s identity when `obj` is identifiable.
pub(super) fn memo_insert_obj(memo: &Value, obj: &Value, copy: &Value) {
    if let Some(id) = value_identity(obj) {
        memo_insert(memo, id, copy.clone());
    }
}
