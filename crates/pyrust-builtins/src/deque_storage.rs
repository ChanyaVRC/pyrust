//! Internal storage for `collections.deque`.
//!
//! `deque` itself is a Python class implemented by the `collections` module,
//! but its payload must not be a Python `list`: removing index zero from a
//! `Vec` makes `popleft()` linear.  This opaque built-in object keeps the
//! storage responsibility in `pyrust-builtins` and gives the module body a
//! small, typed `VecDeque` API.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use pyrust_core::{BuiltinState, BuiltinTypeOps, Value, ValueKind, builtin_ops_is};

pub const TYPE_NAME: &str = "_deque_storage";
pub const DEQUE_STORAGE_OPS: &DequeStorageOps = &DequeStorageOps;

/// Shared backing buffer.  Cloning the `Rc` preserves Python object identity
/// while allowing native iterators to keep the storage alive independently of
/// the instance attribute that owns it.
pub type DequeData = Rc<RefCell<VecDeque<Value>>>;

/// Shared structural-mutation counter observed by live deque iterators.
///
/// This is Rust-only state. Keeping it beside the opaque storage prevents
/// Python code from resizing or replacing the counter cell while an iterator
/// is reading it.
pub type DequeMutationState = Rc<Cell<i64>>;

pub struct DequeStorageState {
    data: DequeData,
    mutation_state: DequeMutationState,
}

pub struct DequeStorageOps;

impl BuiltinTypeOps for DequeStorageOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, _state: &BuiltinState) -> String {
        "<internal deque storage>".to_string()
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        storage_data(state).is_some_and(|data| !data.borrow().is_empty())
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        storage_data(state).map(|data| data.borrow().len())
    }
}

fn storage_data(state: &BuiltinState) -> Option<DequeData> {
    let borrow = state.borrow();
    borrow
        .downcast_ref::<DequeStorageState>()
        .map(|storage| Rc::clone(&storage.data))
}

fn storage_mutation_state(state: &BuiltinState) -> Option<DequeMutationState> {
    let borrow = state.borrow();
    borrow
        .downcast_ref::<DequeStorageState>()
        .map(|storage| Rc::clone(&storage.mutation_state))
}

/// Construct opaque, shared `VecDeque` storage from items in logical order.
pub fn deque_storage(items: Vec<Value>) -> Value {
    let state = DequeStorageState {
        data: Rc::new(RefCell::new(VecDeque::from(items))),
        mutation_state: Rc::new(Cell::new(0)),
    };
    Value::builtin_object(DEQUE_STORAGE_OPS, Box::new(state))
}

/// Return the shared buffer when `value` is internal deque storage.
pub fn data(value: &Value) -> Option<DequeData> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<DequeStorageOps>(ops) {
        return None;
    }
    storage_data(state)
}

/// Return the shared structural-mutation state for internal deque storage.
pub fn mutation_state(value: &Value) -> Option<DequeMutationState> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<DequeStorageOps>(ops) {
        return None;
    }
    storage_mutation_state(state)
}

/// Bump the structural-mutation state, wrapping after `i64::MAX`.
pub fn bump_mutation_state(value: &Value) -> Option<()> {
    let state = mutation_state(value)?;
    state.set(state.get().wrapping_add(1));
    Some(())
}

pub fn is_storage(value: &Value) -> bool {
    data(value).is_some()
}

pub fn snapshot(value: &Value) -> Option<Vec<Value>> {
    data(value).map(|items| items.borrow().iter().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_double_ended_storage() {
        let value = deque_storage(vec![Value::int(1), Value::int(2)]);
        let alias = value.clone();
        let data = data(&value).expect("deque storage");
        let version = mutation_state(&value).expect("deque mutation state");
        assert_eq!(version.get(), 0);
        data.borrow_mut().push_front(Value::int(0));
        data.borrow_mut().push_back(Value::int(3));
        bump_mutation_state(&value).expect("bump mutation state");

        let values = snapshot(&alias).expect("shared storage");
        assert_eq!(
            values.iter().map(Value::repr_raw).collect::<Vec<_>>(),
            ["0", "1", "2", "3"]
        );
        assert_eq!(
            mutation_state(&alias).expect("shared mutation state").get(),
            1
        );
    }
}
