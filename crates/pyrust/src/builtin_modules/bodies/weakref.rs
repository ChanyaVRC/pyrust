// `weakref` module — body for the `weakref @inject` entry in
// `pyrust_builtin_modules!` (issue #3190).
//
// Native weak-handle primitives backed by `pyrust_core`'s per-thread drop-hook
// registry, combined with the pure-Python CPython 3.12 `weakref` surface
// (`ref`, `ReferenceType`, `proxy`, `ProxyType`, `CallableProxyType`,
// `ProxyTypes`, `WeakMethod`, `WeakKeyDictionary`, `WeakValueDictionary`,
// `WeakSet`, `finalize`, `ReferenceError`) injected from `weakref_py.py`.
//
// Reference: <https://docs.python.org/3/library/weakref.html>

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyValue;
use crate::interpreter::{ExpandedCallArg, Interpreter, class_supports_weakref};
use crate::value::{PyKey, Value, ValueKind};
use pyrust_core::{BuiltinTypeOps, CanonicalClassTag, WeakTarget, builtin_ops_is};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `weakref` module.
const WEAKREF_PY_SOURCE: &str = include_str!("weakref_py.py");

/// Public names exported from `WEAKREF_PY_SOURCE` onto the `weakref` module.
const WEAKREF_PY_EXPORTS: &[&str] = &[
    "__all__",
    "ReferenceType",
    "ref",
    "ProxyType",
    "CallableProxyType",
    "ProxyTypes",
    "proxy",
    "WeakMethod",
    "KeyedRef",
    "WeakKeyDictionary",
    "WeakValueDictionary",
    "WeakSet",
    "finalize",
];

/// Private native helper names passed into `WEAKREF_PY_SOURCE`'s namespace and
/// removed from the public module dict after injection.
const WEAKREF_PRIVATE_HELPERS: &[&str] = &[
    "_find_plain_ref",
    "_create_handle",
    "_deref",
    "_get_callback",
    "_handle_repr_info",
    "_bind_method",
];

struct WeakHandleState {
    target: WeakTarget,
    target_addr: usize,
    handle_id: u64,
    alive: Rc<Cell<bool>>,
    callback: Rc<RefCell<Option<Value>>>,
    target_type_name: String,
    target_repr_name: Option<String>,
    target_id: i64,
}

impl Drop for WeakHandleState {
    fn drop(&mut self) {
        if self.alive.get() {
            pyrust_core::unregister_weakref(self.target_addr, self.handle_id);
        }
    }
}

struct WeakHandleOps;
const WEAK_HANDLE_OPS: &WeakHandleOps = &WeakHandleOps;

impl BuiltinTypeOps for WeakHandleOps {
    fn type_name(&self) -> &'static str {
        "_weakref_handle"
    }

    fn is_internal_storage(&self) -> bool {
        true
    }
}

fn downcast_handle<R>(val: &Value, f: impl FnOnce(&WeakHandleState) -> R) -> Result<R> {
    let ValueKind::BuiltinObject { ops, state } = val.kind() else {
        return Err(PyError::named(
            "TypeError",
            "expected internal weakref handle".to_string(),
        ));
    };
    if !builtin_ops_is::<WeakHandleOps>(ops) {
        return Err(PyError::named(
            "TypeError",
            "expected internal weakref handle".to_string(),
        ));
    }
    let borrowed = state.borrow();
    let handle = borrowed
        .downcast_ref::<WeakHandleState>()
        .ok_or_else(|| PyError::runtime("corrupt internal weakref handle".to_string()))?;
    Ok(f(handle))
}

/// Check CPython 3.12 weakrefability policy for `obj`.
fn is_weakrefable(obj: &Value) -> bool {
    match obj.kind() {
        ValueKind::PyInstance(inst) => {
            let class = Rc::clone(&inst.borrow().class);
            class_supports_weakref(&class)
        }
        ValueKind::PyClass(_)
        | ValueKind::UserFunction(_)
        | ValueKind::BuiltinFunction(_)
        | ValueKind::Set(_)
        | ValueKind::PyModule(_)
        | ValueKind::BoundMethod { .. }
        | ValueKind::ClassBoundMethod { .. } => true,
        ValueKind::Generator(cell) => cell.kind() != pyrust_core::GeneratorKind::Iterator,
        ValueKind::BuiltinObject { ops, .. } => {
            ops.canonical_class_tag() == Some(CanonicalClassTag::Frozenset)
        }
        _ => false,
    }
}

fn weakref_target_repr_name(obj: &Value) -> Option<String> {
    match obj.kind() {
        ValueKind::PyClass(c) => Some(c.borrow().name.clone()),
        ValueKind::UserFunction(f) => Some(f.name.to_string()),
        ValueKind::BuiltinFunction(name) => {
            let short = name.rsplit('.').next().unwrap_or(name);
            Some(short.to_string())
        }
        ValueKind::PyModule(m) => Some(m.borrow().name.clone()),
        _ => None,
    }
}

fn weakref_target_type_name(obj: &Value) -> String {
    match obj.kind() {
        ValueKind::PyInstance(inst) => inst.borrow().class.borrow().name.clone(),
        ValueKind::PyClass(_) => "type".to_string(),
        ValueKind::UserFunction(_) => "function".to_string(),
        ValueKind::BuiltinFunction(_) => "builtin_function_or_method".to_string(),
        ValueKind::Set(_) => "set".to_string(),
        ValueKind::PyModule(_) => "module".to_string(),
        ValueKind::BoundMethod { .. } | ValueKind::ClassBoundMethod { .. } => "method".to_string(),
        _ => pyrust_core::error_type_name(obj).to_string(),
    }
}

pyrust_module! {
    /// `weakref.getweakrefcount(object)` — return the number of weak references
    /// and proxies which refer to `object`.
    fn getweakrefcount(obj: PyValue) -> Result<Value> {
        if pyrust_core::has_pending_weakref_callbacks() {
            _interp.drain_pending_weakref_callbacks();
        }
        let Some(addr) = pyrust_core::weakref_target_addr(&obj.0) else {
            return Ok(Value::int(0));
        };
        Ok(Value::int(pyrust_core::get_weakref_count(addr) as i64))
    }

    /// `weakref.getweakrefs(object)` — return a list of all weak reference and
    /// proxy objects which refer to `object`.
    fn getweakrefs(obj: PyValue) -> Result<Value> {
        if pyrust_core::has_pending_weakref_callbacks() {
            _interp.drain_pending_weakref_callbacks();
        }
        let Some(addr) = pyrust_core::weakref_target_addr(&obj.0) else {
            return Ok(Value::list(Vec::new()));
        };
        Ok(Value::list(pyrust_core::get_weakrefs(addr)))
    }

    /// Internal: look up an existing plain `ref` (kind=1) or `proxy` (kind=2)
    /// for `obj`, or validate weakrefability and return `None`.
    fn _find_plain_ref(obj: PyValue, dedup_kind: PyValue) -> Result<Value> {
        if !is_weakrefable(&obj.0) {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "cannot create weak reference to '{}' object",
                    pyrust_core::error_type_name(&obj.0)
                ),
            ));
        }
        let Some(addr) = pyrust_core::weakref_target_addr(&obj.0) else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "cannot create weak reference to '{}' object",
                    pyrust_core::error_type_name(&obj.0)
                ),
            ));
        };
        let dedup = dedup_kind.0.as_int().unwrap_or(0) as u8;
        Ok(pyrust_core::find_existing_plain_ref(addr, dedup).unwrap_or_else(Value::none))
    }

    /// Internal: create and register a `WeakHandle` for `wrapper` pointing to
    /// `obj` with optional `callback` and deduplication kind (`0`/`1`/`2`).
    fn _create_handle(
        wrapper: PyValue,
        obj: PyValue,
        callback: PyValue,
        dedup_kind: PyValue,
    ) -> Result<Value> {
        if !is_weakrefable(&obj.0) {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "cannot create weak reference to '{}' object",
                    pyrust_core::error_type_name(&obj.0)
                ),
            ));
        }
        let Some((target, target_addr)) = WeakTarget::downgrade_value(&obj.0) else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "cannot create weak reference to '{}' object",
                    pyrust_core::error_type_name(&obj.0)
                ),
            ));
        };
        let ValueKind::PyInstance(wrapper_rc) = wrapper.0.kind() else {
            return Err(PyError::runtime(
                "internal: weakref wrapper must be an instance".to_string(),
            ));
        };
        let wrapper_weak = Rc::downgrade(wrapper_rc);
        let alive = Rc::new(Cell::new(true));
        let cb_opt = if callback.0.is_none() {
            None
        } else {
            Some(callback.0.clone())
        };
        let cb_cell = Rc::new(RefCell::new(cb_opt));
        let dedup = dedup_kind.0.as_int().unwrap_or(0) as u8;
        let handle_id = pyrust_core::register_weakref(
            target_addr,
            dedup,
            wrapper_weak,
            Rc::clone(&alive),
            Rc::clone(&cb_cell),
        );
        _interp.sync_vm_hooks_flags();

        let state: Box<dyn Any> = Box::new(WeakHandleState {
            target,
            target_addr,
            handle_id,
            alive,
            callback: cb_cell,
            target_type_name: weakref_target_type_name(&obj.0),
            target_repr_name: weakref_target_repr_name(&obj.0),
            target_id: obj.0.value_id().unwrap_or(target_addr as i64),
        });
        Ok(Value::builtin_object(WEAK_HANDLE_OPS, state))
    }

    /// Internal: dereference a `WeakHandle`, returning the live referent or
    /// `None` if dead.
    fn _deref(handle: PyValue) -> Result<Value> {
        downcast_handle(&handle.0, |h| {
            if !h.alive.get() {
                return Value::none();
            }
            h.target.upgrade().unwrap_or_else(Value::none)
        })
    }

    /// Internal: return the live callback registered on a `WeakHandle`, or
    /// `None` if none or referent has already died.
    fn _get_callback(handle: PyValue) -> Result<Value> {
        downcast_handle(&handle.0, |h| {
            h.callback.borrow().clone().unwrap_or_else(Value::none)
        })
    }

    /// Internal: return `(alive, target_type_name, target_id, target_repr_name)`
    /// for formatting `repr(ref)` and `repr(proxy)`.
    fn _handle_repr_info(handle: PyValue) -> Result<Value> {
        downcast_handle(&handle.0, |h| {
            let is_alive = h.alive.get() && h.target.upgrade().is_some();
            let rname = h
                .target_repr_name
                .as_deref()
                .map(Value::string)
                .unwrap_or_else(Value::none);
            Value::tuple(vec![
                Value::bool_(is_alive),
                Value::string(&h.target_type_name),
                Value::int(h.target_id),
                rname,
            ])
        })
    }

    /// Internal: bind `func` to `obj` to reconstruct a bound method for
    /// `WeakMethod.__call__()`.
    fn _bind_method(func: PyValue, obj: PyValue) -> Result<Value> {
        let bound = match (func.0.kind(), obj.0.kind()) {
            (ValueKind::UserFunction(f), ValueKind::PyInstance(inst)) => {
                Some(Value::bound_method(Rc::clone(f), Rc::clone(inst)))
            }
            (ValueKind::UserFunction(f), ValueKind::PyClass(cls)) => {
                Some(Value::class_bound_method(Rc::clone(f), Rc::clone(cls)))
            }
            _ => None,
        };
        if let Some(v) = bound {
            return Ok(v);
        }
        let get_fn = _interp.get_attr(&func.0, "__get__")?;
        _interp.call_function_expanded(
            get_fn,
            &[ExpandedCallArg {
                name: None,
                value: obj.0,
            }],
        )
    }
}

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    for helper_name in WEAKREF_PRIVATE_HELPERS {
        if let Some(helper) = module.borrow().attrs.get(*helper_name).cloned() {
            ns.dict_insert(PyKey::str_from(helper_name), helper)?;
        }
    }
    interp.exec_source(WEAKREF_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("weakref: exec namespace not a dict".into()))?;
    let mut weak_set = None;
    for name in WEAKREF_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut cls = cls_rc.borrow_mut();
                match *name {
                    "ref" | "ReferenceType" => {
                        cls.error_name = Some("weakref.ReferenceType");
                    }
                    "ProxyType" => {
                        cls.error_name = Some("weakref.ProxyType");
                    }
                    "CallableProxyType" => {
                        cls.error_name = Some("weakref.CallableProxyType");
                    }
                    "WeakSet" => {
                        drop(cls);
                        weak_set = Some(Rc::clone(cls_rc));
                    }
                    _ => {}
                }
            }
            module
                .borrow_mut()
                .attrs
                .insert((*name).to_string(), val.clone());
        }
    }
    for helper_name in WEAKREF_PRIVATE_HELPERS {
        module.borrow_mut().attrs.shift_remove(*helper_name);
    }
    // `_collections_abc.MutableSet.register(WeakSet)`.
    if let Some(weak_set) = weak_set {
        super::collections_abc::register_virtual_mutable_set(interp, &weak_set)?;
    }
    Ok(Some(ns.clone()))
}
