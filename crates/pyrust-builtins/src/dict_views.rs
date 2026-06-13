//! `dict_keys` / `dict_values` / `dict_items` views.
//!
//! Eliminated from `pyrust-core`'s Tier 1 (#296): they're returned by method
//! calls (`d.keys()`, `d.values()`, `d.items()`), not constructed by literal
//! syntax, and their payload is the same `Rc<RefCell<IndexMap>>` as the
//! parent dict.  They live here as `BuiltinObject`s with the IndexMap rc.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use pyrust_core::{BuiltinState, BuiltinTypeOps, PyDict, Result, Value, key_repr};

pub type DictRc = Rc<RefCell<PyDict>>;

pub struct DictView {
    pub items: DictRc,
    /// `true` when the view was produced from a `collections.OrderedDict`
    /// (or a subclass of it).  Only consumed by the size-mutation guard
    /// (issue #2436) to pick CPython's `"OrderedDict mutated during
    /// iteration"` wording instead of plain dict's `"dictionary changed
    /// size during iteration"`.  Plain-dict and other dict-subclass views
    /// leave it `false`.
    pub ordered: bool,
}

// ── keys ─────────────────────────────────────────────────────────────────────

pub struct DictKeysOps;
pub const DICT_KEYS_OPS: &DictKeysOps = &DictKeysOps;
pub const DICT_KEYS_TYPE_NAME: &str = "dict_keys";

impl BuiltinTypeOps for DictKeysOps {
    fn type_name(&self) -> &'static str {
        DICT_KEYS_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let view = borrow_view(state).expect("dict_keys state");
        let map = view.borrow();
        let keys: Vec<String> = map.keys().map(key_repr).collect();
        format!("dict_keys([{}])", keys.join(", "))
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        borrow_view(state)
            .map(|rc| !rc.borrow().is_empty())
            .unwrap_or(false)
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        borrow_view(state).map(|rc| rc.borrow().len())
    }

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        let rc = borrow_view(state).ok_or_else(|| {
            pyrust_core::PyError::Runtime("internal: bad dict_keys state".to_string())
        })?;
        match item.to_key() {
            Some(k) => Ok(rc.borrow().contains_key(&k)),
            None => Err(pyrust_core::PyError::Runtime("unhashable type".to_string())),
        }
    }

    // `dict_keys` is set-like: `isdisjoint` is dispatched on the interpreter
    // side (it iterates the argument and probes this view's membership), but
    // `hasattr`/attribute access must surface it as a method (issue #1891).
    // `__reversed__` is exposed since dict views are reversible (issue #2093).
    fn has_method(&self, name: &str) -> bool {
        name == "isdisjoint" || name == "__reversed__"
    }
}

pub fn dict_keys(rc: DictRc) -> Value {
    dict_keys_tagged(rc, false)
}

/// Like [`dict_keys`] but records whether the source is an OrderedDict so the
/// mutation guard can pick the OrderedDict wording (issue #2436).
pub fn dict_keys_tagged(rc: DictRc, ordered: bool) -> Value {
    let state: Box<dyn Any> = Box::new(DictView { items: rc, ordered });
    Value::builtin_object(DICT_KEYS_OPS, state)
}

// ── values ───────────────────────────────────────────────────────────────────

pub struct DictValuesOps;
pub const DICT_VALUES_OPS: &DictValuesOps = &DictValuesOps;
pub const DICT_VALUES_TYPE_NAME: &str = "dict_values";

impl BuiltinTypeOps for DictValuesOps {
    fn type_name(&self) -> &'static str {
        DICT_VALUES_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let view = borrow_view(state).expect("dict_values state");
        let map = view.borrow();
        let vals: Vec<String> = map.values().map(|v| v.repr()).collect();
        format!("dict_values([{}])", vals.join(", "))
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        borrow_view(state)
            .map(|rc| !rc.borrow().is_empty())
            .unwrap_or(false)
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        borrow_view(state).map(|rc| rc.borrow().len())
    }

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        let rc = borrow_view(state).ok_or_else(|| {
            pyrust_core::PyError::Runtime("internal: bad dict_values state".to_string())
        })?;
        Ok(rc.borrow().values().any(|v| v == item))
    }

    // `dict_values` is reversible by insertion order (issue #2093); expose
    // `__reversed__` for `hasattr`/attribute access.
    fn has_method(&self, name: &str) -> bool {
        name == "__reversed__"
    }
}

pub fn dict_values(rc: DictRc) -> Value {
    dict_values_tagged(rc, false)
}

/// OrderedDict-aware counterpart of [`dict_values`] (issue #2436).
pub fn dict_values_tagged(rc: DictRc, ordered: bool) -> Value {
    let state: Box<dyn Any> = Box::new(DictView { items: rc, ordered });
    Value::builtin_object(DICT_VALUES_OPS, state)
}

// ── items ────────────────────────────────────────────────────────────────────

pub struct DictItemsOps;
pub const DICT_ITEMS_OPS: &DictItemsOps = &DictItemsOps;
pub const DICT_ITEMS_TYPE_NAME: &str = "dict_items";

impl BuiltinTypeOps for DictItemsOps {
    fn type_name(&self) -> &'static str {
        DICT_ITEMS_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let view = borrow_view(state).expect("dict_items state");
        let map = view.borrow();
        let items: Vec<String> = map
            .iter()
            .map(|(k, v)| format!("({}, {})", key_repr(k), v.repr()))
            .collect();
        format!("dict_items([{}])", items.join(", "))
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        borrow_view(state)
            .map(|rc| !rc.borrow().is_empty())
            .unwrap_or(false)
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        borrow_view(state).map(|rc| rc.borrow().len())
    }

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        let rc = borrow_view(state).ok_or_else(|| {
            pyrust_core::PyError::Runtime("internal: bad dict_items state".to_string())
        })?;
        match item.kind() {
            pyrust_core::ValueKind::Tuple(kv) if kv.len() == 2 => {
                let key = kv[0]
                    .to_key()
                    .ok_or_else(|| pyrust_core::PyError::Runtime("unhashable type".to_string()))?;
                let map = rc.borrow();
                Ok(map.get(&key).is_some_and(|v| v == &kv[1]))
            }
            _ => Ok(false),
        }
    }

    // `dict_items` is set-like: `isdisjoint` is dispatched on the interpreter
    // side; expose it for `hasattr`/attribute access (issue #1891).
    // `__reversed__` is exposed since dict views are reversible (issue #2093).
    fn has_method(&self, name: &str) -> bool {
        name == "isdisjoint" || name == "__reversed__"
    }
}

pub fn dict_items(rc: DictRc) -> Value {
    dict_items_tagged(rc, false)
}

/// OrderedDict-aware counterpart of [`dict_items`] (issue #2436).
pub fn dict_items_tagged(rc: DictRc, ordered: bool) -> Value {
    let state: Box<dyn Any> = Box::new(DictView { items: rc, ordered });
    Value::builtin_object(DICT_ITEMS_OPS, state)
}

// ── extraction ───────────────────────────────────────────────────────────────

/// If `value` is one of the three dict views, return its backing IndexMap Rc.
pub fn as_dict_rc(value: &Value) -> Option<DictRc> {
    let pyrust_core::ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    let n = ops.type_name();
    if n != DICT_KEYS_TYPE_NAME && n != DICT_VALUES_TYPE_NAME && n != DICT_ITEMS_TYPE_NAME {
        return None;
    }
    borrow_view(state)
}

/// `true` if `value` is a dict view tagged as backed by a `collections.
/// OrderedDict` (or subclass).  Drives the OrderedDict-specific mutation-
/// during-iteration message (issue #2436); non-views and plain-dict views
/// return `false`.
pub fn is_ordered_view(value: &Value) -> bool {
    let pyrust_core::ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return false;
    };
    let n = ops.type_name();
    if n != DICT_KEYS_TYPE_NAME && n != DICT_VALUES_TYPE_NAME && n != DICT_ITEMS_TYPE_NAME {
        return false;
    }
    state
        .borrow()
        .downcast_ref::<DictView>()
        .map(|v| v.ordered)
        .unwrap_or(false)
}

/// Returns the view kind: 0=keys, 1=values, 2=items, or None if not a view.
pub fn view_kind(value: &Value) -> Option<u8> {
    let pyrust_core::ValueKind::BuiltinObject { ops, .. } = value.kind() else {
        return None;
    };
    match ops.type_name() {
        DICT_KEYS_TYPE_NAME => Some(0),
        DICT_VALUES_TYPE_NAME => Some(1),
        DICT_ITEMS_TYPE_NAME => Some(2),
        _ => None,
    }
}

fn borrow_view(state: &BuiltinState) -> Option<DictRc> {
    let borrow = state.borrow();
    borrow
        .downcast_ref::<DictView>()
        .map(|v| Rc::clone(&v.items))
}
