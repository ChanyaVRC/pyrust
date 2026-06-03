// `copy` module — `copy.copy` (shallow copy) and `copy.deepcopy` (deep copy).
//
// ## Design
//
// `Value::clone` in pyrust shares the backing `Rc` for mutable containers
// (list, set, dict) — i.e. `b = a` aliases rather than copies.  To produce
// an independent shallow copy we must allocate a new container with the same
// top-level elements (which are themselves shared by reference).
//
// Immutable types (int, float, str, bool, None, bytes, tuple, frozenset)
// are safe to return as-is — CPython does the same (copy.copy of an immutable
// returns the original object).
//
// For `PyInstance`:
//   - If the class defines `__copy__`, call it with no extra args.
//   - Otherwise clone the attrs map one level (each attr Value is shared by
//     its Rc — no element-level copies).
//
// `deepcopy` recurses into the elements of mutable containers.  The `memo`
// dict is forwarded to any `__deepcopy__(self, memo)` dunder calls so user
// code receives a proper dict argument.  Circular-reference tracking via the
// memo dict is out of scope for v1.
//
// Reference: <https://docs.python.org/3/library/copy.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{invoke_class_method, key_to_value, lookup_class_attr, ExpandedCallArg};
use crate::value::{InstanceAttrs, PyClass, PyInstance, PyKey, Value, ValueKind};
use indexmap::{IndexMap, IndexSet};
use pyrust_derive::pyrust_module;

// ── copy.Error class singleton ────────────────────────────────────────────────

thread_local! {
    static COPY_ERROR_CLASS: Rc<RefCell<PyClass>> = {
        let exception_base = crate::interpreter::lookup_exc_class("Exception")
            .expect("EXC_CLASS_CACHE must contain Exception");
        let class = Rc::new(RefCell::new(PyClass::new(
            "Error",
            "Error",
            Some(Rc::clone(&exception_base)),
            IndexMap::new(),
        )));
        exception_base.borrow().subclasses.borrow_mut().push(Rc::downgrade(&class));
        class
    };
}

fn copy_error_class_value() -> Value {
    COPY_ERROR_CLASS.with(|c| Value::py_class(Rc::clone(c)))
}

pyrust_module! {
    constants {
        // copy.Error — subclass of Exception, raised on deepcopy failures.
        "Error" => copy_error_class_value(),
        // CPython's `copy.py` keeps a lowercase alias for backward
        // compatibility: `error = Error`.  Both names must resolve to the
        // same class so `copy.error is copy.Error` is `True`.
        "error" => copy_error_class_value(),
    }
    /// CPython: copy.copy(x) — shallow copy.
    ///
    /// Immutable types (int, float, str, bool, None, bytes, tuple, frozenset)
    /// are returned as-is.  Mutable containers (list, dict, set) get a new
    /// top-level container with the same elements.  For `PyInstance`, calls
    /// `__copy__` if defined, otherwise clones the attr map shallowly.
    /// <https://docs.python.org/3/library/copy.html#copy.copy>
    fn copy(args) -> Result<Value> {
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument ({} given)", args.len()),
            ));
        }
        let obj = args[0].value.clone();
        shallow_copy(obj, _interp)
    }

    /// CPython: copy.deepcopy(x, memo=None) — deep copy.
    ///
    /// Immutable types are returned as-is.  Mutable containers are recursively
    /// copied.  `memo` is forwarded to `__deepcopy__` calls (as a dict) so
    /// user-defined `__deepcopy__(self, memo)` methods receive a proper dict
    /// argument.  Circular-reference tracking via the memo dict is out of scope
    /// for v1.
    /// <https://docs.python.org/3/library/copy.html#copy.deepcopy>
    fn deepcopy(args) -> Result<Value> {
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() takes 1 or 2 arguments ({} given)",
                    args.len()
                ),
            ));
        }
        let obj = args[0].value.clone();
        // Use the caller-supplied memo dict when present; otherwise start with
        // an empty dict.  CPython always passes a dict (never None) to
        // __deepcopy__, so we must forward a proper dict here.
        let memo = if args.len() == 2 {
            args[1].value.clone()
        } else {
            Value::dict(IndexMap::new())
        };
        deep_copy(obj, memo, _interp)
    }
}

// ── shallow_copy ──────────────────────────────────────────────────────────────

/// Produce a shallow copy of `obj`.  Immutable types are returned as-is;
/// mutable containers get a new top-level allocation with shared elements.
fn shallow_copy(obj: Value, interp: &mut crate::interpreter::Interpreter) -> Result<Value> {
    match obj.kind() {
        // Immutable scalars — return the same value.
        ValueKind::None
        | ValueKind::Bool(_)
        | ValueKind::Int(_)
        | ValueKind::BigInt(_)
        | ValueKind::Float(_)
        | ValueKind::Str(_)
        | ValueKind::Bytes(_)
        | ValueKind::Complex(_, _)
        | ValueKind::Range { .. } => Ok(obj.clone()),

        // Immutable sequences / sets — return the same value.
        // Tuple: CPython's copy.copy returns the same object for tuples.
        ValueKind::Tuple(_) => Ok(obj.clone()),

        // frozenset is immutable — same object.
        ValueKind::BuiltinObject { ops, .. } if ops.type_name() == "frozenset" => {
            Ok(obj.clone())
        }

        // list — new list with the same elements (shallow: element Values share Rc).
        ValueKind::List(items) => {
            let new_items: Vec<Value> = items.iter().cloned().collect();
            // Drop the Ref borrow guard before constructing the new Value.
            drop(items);
            Ok(Value::list(new_items))
        }

        // dict — new dict with the same key-value pairs.
        ValueKind::Dict(d) => {
            let new_dict: IndexMap<PyKey, Value> = d.clone();
            drop(d);
            Ok(Value::dict(new_dict))
        }

        // set — new set with the same keys.
        ValueKind::Set(items) => {
            let new_items: IndexSet<PyKey> = items.clone();
            drop(items);
            Ok(Value::set(new_items))
        }

        // PyInstance — check for __copy__ dunder first (MRO-aware).
        ValueKind::PyInstance(rc) => {
            let rc = Rc::clone(rc);
            // Look up __copy__ on the class via MRO so inherited dunders
            // are found.  Do not hold any borrow across the call.
            let copy_method = {
                let borrow = rc.borrow();
                let class = Rc::clone(&borrow.class);
                drop(borrow);
                lookup_class_attr(&class, "__copy__")
            };
            if let Some(method) = copy_method {
                // Call `__copy__(self)` — use invoke_class_method so that
                // UserFunction and BuiltinFunction are both handled and
                // `self` is bound via the bound_prefix slot.
                invoke_class_method(
                    interp,
                    method,
                    Value::py_instance(Rc::clone(&rc)),
                    &[],
                )
            } else {
                // Default: clone the instance with a shallowly-cloned attr map.
                let (class, attrs) = {
                    let borrow = rc.borrow();
                    (Rc::clone(&borrow.class), borrow.attrs.clone())
                };
                Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
                    class,
                    attrs,
                }))))
            }
        }

        // Anything else (functions, classes, modules, generators, …) — return
        // as-is, matching CPython's behaviour for non-copyable objects that don't
        // define __copy__ (they are returned unchanged).
        _ => Ok(obj.clone()),
    }
}

// ── deep_copy ─────────────────────────────────────────────────────────────────

/// Produce a deep copy of `obj`.  Immutable types are returned as-is.
/// Mutable containers are recursively copied.  `memo` is forwarded to any
/// `__deepcopy__(self, memo)` dunder calls so user code receives a proper dict.
fn deep_copy(
    obj: Value,
    memo: Value,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<Value> {
    match obj.kind() {
        // Immutable scalars.
        ValueKind::None
        | ValueKind::Bool(_)
        | ValueKind::Int(_)
        | ValueKind::BigInt(_)
        | ValueKind::Float(_)
        | ValueKind::Str(_)
        | ValueKind::Bytes(_)
        | ValueKind::Complex(_, _)
        | ValueKind::Range { .. } => Ok(obj.clone()),

        // frozenset — CPython deep-copies the elements even though the
        // container itself is immutable; user-defined hashable objects inside
        // the frozenset may have __deepcopy__ methods.  Extract the inner
        // IndexSet<PyKey>, convert each key to a Value, deep-copy it, then
        // convert back via value_to_pykey and rebuild a new frozenset.
        ValueKind::BuiltinObject { ops, .. } if ops.type_name() == "frozenset" => {
            let items_rc = pyrust_builtins::frozenset::as_items(&obj)
                .expect("frozenset arm: as_items must succeed");
            // Snapshot before borrowing mutably through interp.
            let keys: Vec<PyKey> = items_rc.iter().cloned().collect();
            drop(items_rc);
            let mut new_set: IndexSet<PyKey> = IndexSet::with_capacity(keys.len());
            for k in keys {
                let v = key_to_value(k);
                let deep_v = deep_copy(v, memo.clone(), interp)?;
                let new_k = interp.value_to_pykey(&deep_v)?;
                new_set.insert(new_k);
            }
            Ok(pyrust_builtins::frozenset::frozenset(new_set))
        }

        // tuple — CPython deepcopies each element into a new tuple even though
        // tuples are immutable, for consistency with user-defined types that may
        // be tuple subclasses.
        ValueKind::Tuple(items) => {
            // Collect to owned Vec first so the borrow (&[Value]) is released
            // before we recursively call deep_copy (which may re-enter match).
            let items_vec: Vec<Value> = items.to_vec();
            // items is &[Value] — a raw reference, no Ref guard to drop.
            let mut new_items = Vec::with_capacity(items_vec.len());
            for item in items_vec {
                new_items.push(deep_copy(item, memo.clone(), interp)?);
            }
            Ok(Value::tuple(new_items))
        }

        // list — new list with deeply-copied elements.
        ValueKind::List(items) => {
            // Collect before dropping the Ref borrow.
            let items_vec: Vec<Value> = items.iter().cloned().collect();
            drop(items);
            let mut new_items = Vec::with_capacity(items_vec.len());
            for item in items_vec {
                new_items.push(deep_copy(item, memo.clone(), interp)?);
            }
            Ok(Value::list(new_items))
        }

        // dict — new dict with deeply-copied keys and values.  Keys are PyKey
        // but PyKey::Object can hold user instances with __deepcopy__; CPython
        // deep-copies both keys and values (matching `copy.deepcopy({obj: v})`).
        ValueKind::Dict(d) => {
            let pairs: Vec<(PyKey, Value)> =
                d.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            drop(d);
            let mut new_dict: IndexMap<PyKey, Value> = IndexMap::with_capacity(pairs.len());
            for (k, v) in pairs {
                let deep_k = {
                    let kv = key_to_value(k);
                    let deep_kv = deep_copy(kv, memo.clone(), interp)?;
                    interp.value_to_pykey(&deep_kv)?
                };
                new_dict.insert(deep_k, deep_copy(v, memo.clone(), interp)?);
            }
            Ok(Value::dict(new_dict))
        }

        // set — elements are PyKey but PyKey::Object holds user instances that
        // may define __deepcopy__; CPython deep-copies set elements.  Convert
        // each key to a Value, deep-copy it, then convert back via
        // value_to_pykey and rebuild a new independent set.
        ValueKind::Set(items) => {
            let keys: Vec<PyKey> = items.iter().cloned().collect();
            drop(items);
            let mut new_set: IndexSet<PyKey> = IndexSet::with_capacity(keys.len());
            for k in keys {
                let v = key_to_value(k);
                let deep_v = deep_copy(v, memo.clone(), interp)?;
                let new_k = interp.value_to_pykey(&deep_v)?;
                new_set.insert(new_k);
            }
            Ok(Value::set(new_set))
        }

        // PyInstance — check for __deepcopy__ dunder first (MRO-aware).
        ValueKind::PyInstance(rc) => {
            let rc = Rc::clone(rc);
            // Look up __deepcopy__ on the class via MRO so inherited
            // dunders are found.  Do not hold any borrow across the call.
            let deepcopy_method = {
                let borrow = rc.borrow();
                let class = Rc::clone(&borrow.class);
                drop(borrow);
                lookup_class_attr(&class, "__deepcopy__")
            };
            if let Some(method) = deepcopy_method {
                // Call `__deepcopy__(self, memo)` — forward the memo dict so
                // user code receives a proper dict argument (CPython never
                // passes None here).  invoke_class_method binds `self` via
                // bound_prefix and appends the remaining args after.
                invoke_class_method(
                    interp,
                    method,
                    Value::py_instance(Rc::clone(&rc)),
                    &[ExpandedCallArg {
                        name: None,
                        value: memo,
                    }],
                )
            } else {
                // Default: deep-copy each attribute value.
                let (class, attrs_snapshot) = {
                    let borrow = rc.borrow();
                    (Rc::clone(&borrow.class), borrow.attrs.clone())
                };
                let mut new_attrs = InstanceAttrs::with_capacity(attrs_snapshot.len());
                for (k, v) in attrs_snapshot.iter() {
                    new_attrs.insert(k.clone(), deep_copy(v.clone(), memo.clone(), interp)?);
                }
                Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
                    class,
                    attrs: new_attrs,
                }))))
            }
        }

        // Anything else — return as-is.
        _ => Ok(obj.clone()),
    }
}
