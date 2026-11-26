// Default instance deep-copy protocol (`deep_copy_via_protocol`, #2968):
// handles pre-memo constructor-arg backings, empty-seeded post-memo mutable
// backings, native C-style slots, and `__getstate__` / `__setstate__`.

use std::cell::RefCell;
use std::rc::Rc;

use super::deep::{deep_copy, deep_copy_key, deep_copy_storage_elements};
use super::memo::memo_insert_obj;
use super::protocol::{capture_state, restore_state};
use crate::error::Result;
use crate::interpreter::Interpreter;
use crate::value::{InstanceAttrs, PyDict, PyInstance, PyKey, Value, ValueKind};

enum PostMemoBacking {
    None,
    List {
        target: Value,
        items: Vec<Value>,
    },
    Dict {
        target: Value,
        pairs: Vec<(PyKey, Value)>,
    },
    Deque {
        target: Value,
        items: Vec<Value>,
    },
}

/// Default deep-copy path for an instance (#2968).
///
/// Mirrors CPython 3.12's `copy._reconstruct` ordering so an adversarial
/// `__deepcopy__` inspecting `memo.get(id(orig))` never observes a half-built
/// instance lacking its internal backing:
///   - Pre-memo constructor-arg payloads (`Counter`, `set`/`tuple`/`frozenset`
///     subclasses, combinatoric `_cursor` iterators, `defaultdict.default_factory`)
///     deep-copy their constructor inputs *before* `new_val` is memoised.
///   - Post-memo mutable containers (`list`/`dict`/`OrderedDict`/`defaultdict`
///     subclasses and `deque` / `deque` subclasses) install a valid *empty*
///     backing before `memo_insert`, then populate that backing in place so
///     both adversarial observers (`len(seen) == 0`) and self-referential
///     cycles (`s.append(s)`, `d['self'] = d`, `dq.append(dq)`) work cleanly.
///   - Atomic / buffer-backed payloads (`bytearray` and scalar subclasses) copy
///     and install their backing before `memo_insert` (no element recursion).
pub(super) fn deep_copy_via_protocol(
    rc: &Rc<RefCell<PyInstance>>,
    obj: &Value,
    memo: &Value,
    interp: &mut Interpreter,
) -> Result<Value> {
    let (class, builtin_data, native_slots) = {
        let borrow = rc.borrow();
        (
            Rc::clone(&borrow.class),
            borrow.attrs.builtin_data().cloned(),
            borrow.attrs.slots_snapshot(),
        )
    };
    let is_counter =
        crate::builtin_modules::collections::is_canonical_counter_class_or_subclass(&class);
    let new_rc = Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::new(),
    }));
    let new_val = Value::py_instance(Rc::clone(&new_rc));

    let mut post_memo = PostMemoBacking::None;

    // 1. Handle `builtin_data` (pre-memo or empty-seeded post-memo).
    if let Some(backing) = builtin_data {
        match backing.kind() {
            ValueKind::List(items) => {
                let snapshot: Vec<Value> = items.iter().cloned().collect();
                drop(items);
                let empty = Value::list(Vec::with_capacity(snapshot.len()));
                new_rc.borrow_mut().attrs.set_builtin_data(empty.clone());
                post_memo = PostMemoBacking::List {
                    target: empty,
                    items: snapshot,
                };
            }
            ValueKind::Dict(d) => {
                let pairs: Vec<(PyKey, Value)> =
                    d.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                drop(d);
                if is_counter {
                    // CPython reduces Counter as `(cls, (dict(self),))`:
                    // deep-copy items before memoising `new_val`.
                    let mut new_map =
                        PyDict::with_capacity_and_hasher(pairs.len(), Default::default());
                    for (k, v) in pairs {
                        let deep_k = deep_copy_key(k, memo, interp)?;
                        let deep_v = deep_copy(v, memo, interp)?;
                        new_map.insert(deep_k, deep_v);
                    }
                    new_rc
                        .borrow_mut()
                        .attrs
                        .set_builtin_data(Value::dict(new_map));
                } else {
                    let empty = Value::dict(PyDict::with_capacity_and_hasher(
                        pairs.len(),
                        Default::default(),
                    ));
                    new_rc.borrow_mut().attrs.set_builtin_data(empty.clone());
                    post_memo = PostMemoBacking::Dict {
                        target: empty,
                        pairs,
                    };
                }
            }
            _ => {
                // `set`/`tuple`/`frozenset` subclasses, `bytearray`, and scalar
                // immutable backings (`int`, `float`, `str`, `bytes`, …).
                let deep_backing = deep_copy(backing.clone(), memo, interp)?;
                new_rc.borrow_mut().attrs.set_builtin_data(deep_backing);
            }
        }
    }

    // 2. Handle native C-style slots (`_items` on deque, `_cursor` on
    //    combinatoric iterators, `default_factory`, `maxlen`, `_buf`, …).
    for (slot_name, slot_value) in native_slots {
        if pyrust_builtins::deque_storage::is_storage(&slot_value) {
            let items = pyrust_builtins::deque_storage::snapshot(&slot_value).unwrap_or_default();
            let empty = pyrust_builtins::deque_storage::deque_storage(Vec::new());
            new_rc
                .borrow_mut()
                .attrs
                .insert_slot(slot_name.as_ref(), empty.clone());
            post_memo = PostMemoBacking::Deque {
                target: empty,
                items,
            };
        } else if let ValueKind::BuiltinObject { ops, state } = slot_value.kind()
            && ops.is_internal_storage()
        {
            if let Some(copied_cursor) = ops.copy_storage(state) {
                deep_copy_storage_elements(ops, state, &copied_cursor, memo, interp)?;
                new_rc
                    .borrow_mut()
                    .attrs
                    .insert_slot(slot_name.as_ref(), copied_cursor);
            }
        } else {
            let copied_slot = deep_copy(slot_value, memo, interp)?;
            new_rc
                .borrow_mut()
                .attrs
                .insert_slot(slot_name.as_ref(), copied_slot);
        }
    }

    // 3. Memoise the valid (populated or empty-backed) instance *before*
    //    recursing into user `__dict__` state and post-memo container items.
    memo_insert_obj(memo, obj, &new_val);

    // 4. Capture state from the original, deep-copy it, then restore.
    let state = capture_state(rc, interp)?;
    let deep_state = deep_copy(state, memo, interp)?;
    restore_state(&new_rc, deep_state, interp)?;

    // 5. Populate post-memo mutable container backings in place.
    match post_memo {
        PostMemoBacking::None => {}
        PostMemoBacking::List { target, items } => {
            for item in items {
                let copied = deep_copy(item, memo, interp)?;
                target.list_push(copied)?;
            }
        }
        PostMemoBacking::Dict { target, pairs } => {
            for (k, v) in pairs {
                let deep_k = deep_copy_key(k, memo, interp)?;
                let deep_v = deep_copy(v, memo, interp)?;
                let _ = target.dict_insert(deep_k, deep_v);
            }
        }
        PostMemoBacking::Deque { target, items } => {
            if let Some(data) = pyrust_builtins::deque_storage::data(&target) {
                for item in items {
                    let copied = deep_copy(item, memo, interp)?;
                    data.borrow_mut().push_back(copied);
                }
            }
        }
    }

    Ok(new_val)
}
