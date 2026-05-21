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
// parameter is accepted but ignored in this v1 implementation (Python programs
// rarely pass it explicitly; it is there for API compatibility).
//
// Reference: <https://docs.python.org/3/library/copy.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{invoke_class_method, lookup_class_attr, ExpandedCallArg};
use crate::value::{PyClass, PyInstance, PyKey, Value, ValueKind};
use indexmap::{IndexMap, IndexSet};
use pyrust_derive::pyrust_module;

// ── copy.Error class singleton ────────────────────────────────────────────────

thread_local! {
    static COPY_ERROR_CLASS: Rc<RefCell<PyClass>> = {
        let exception_base = crate::interpreter::lookup_exc_class("Exception")
            .expect("EXC_CLASS_CACHE must contain Exception");
        Rc::new(RefCell::new(PyClass {
            name: "Error".to_string(),
            qualname: "copy.Error".to_string(),
            base: Some(exception_base),
            attrs: IndexMap::new(),
        }))
    };
}

fn copy_error_class_value() -> Value {
    COPY_ERROR_CLASS.with(|c| Value::py_class(Rc::clone(c)))
}

pyrust_module! {
    constants {
        // copy.Error — subclass of Exception, raised on deepcopy failures.
        "Error" => copy_error_class_value(),
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
    /// copied.  `memo` is accepted but ignored (circular-reference tracking is
    /// out of scope for v1).
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
        // `memo` (second arg) is accepted and ignored.
        let obj = args[0].value.clone();
        deep_copy(obj, _interp)
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
/// Mutable containers are recursively copied.
fn deep_copy(obj: Value, interp: &mut crate::interpreter::Interpreter) -> Result<Value> {
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

        // frozenset is immutable — same object.
        ValueKind::BuiltinObject { ops, .. } if ops.type_name() == "frozenset" => {
            Ok(obj.clone())
        }

        // tuple — CPython deepcopies each element into a new tuple even though
        // tuples are immutable, for consistency with user-defined types that may
        // be tuple subclasses.
        ValueKind::Tuple(items) => {
            // Collect to owned Vec first so the borrow (&[Value]) is released
            // before we recursively call deep_copy (which may re-enter match).
            let items_vec: Vec<Value> = items.iter().cloned().collect();
            // items is &[Value] — a raw reference, no Ref guard to drop.
            let mut new_items = Vec::with_capacity(items_vec.len());
            for item in items_vec {
                new_items.push(deep_copy(item, interp)?);
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
                new_items.push(deep_copy(item, interp)?);
            }
            Ok(Value::list(new_items))
        }

        // dict — new dict with deeply-copied values.  Keys are PyKey (hashable
        // and immutable by construction) so they need no deep copy.
        ValueKind::Dict(d) => {
            let pairs: Vec<(PyKey, Value)> =
                d.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            drop(d);
            let mut new_dict: IndexMap<PyKey, Value> = IndexMap::with_capacity(pairs.len());
            for (k, v) in pairs {
                new_dict.insert(k, deep_copy(v, interp)?);
            }
            Ok(Value::dict(new_dict))
        }

        // set — set keys are PyKey (immutable/hashable), so the elements
        // themselves need no deep copy; a new independent set is sufficient.
        ValueKind::Set(items) => {
            let new_items: IndexSet<PyKey> = items.clone();
            drop(items);
            Ok(Value::set(new_items))
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
                // Call `__deepcopy__(self, memo)` — pass None for memo.
                // invoke_class_method binds `self` via bound_prefix and
                // appends the remaining args after.
                invoke_class_method(
                    interp,
                    method,
                    Value::py_instance(Rc::clone(&rc)),
                    &[ExpandedCallArg {
                        name: None,
                        value: Value::none(),
                    }],
                )
            } else {
                // Default: deep-copy each attribute value.
                let (class, attrs_snapshot) = {
                    let borrow = rc.borrow();
                    (Rc::clone(&borrow.class), borrow.attrs.clone())
                };
                let mut new_attrs: IndexMap<String, Value> =
                    IndexMap::with_capacity(attrs_snapshot.len());
                for (k, v) in attrs_snapshot {
                    new_attrs.insert(k, deep_copy(v, interp)?);
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
