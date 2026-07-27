//! Native cursor adapters used by `collections.Counter`.
//!
//! Keeping the finite-repeat state here makes its partial-consumption and
//! error timing part of the owning module. The generic runtime sees only the
//! provider iterator interface.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{Interpreter, ProviderIterator};
use crate::value::{PyClass, Value};

struct CounterRepeatState {
    object: Value,
    remaining: usize,
}

pub(super) fn repeat(object: Value, remaining: usize) -> Value {
    Value::generator(Box::new(ProviderIterator::new(
        Box::new(CounterRepeatState { object, remaining }),
        advance_repeat,
        "itertools.repeat",
        "repeat",
        None,
    )))
}

fn advance_repeat(
    _interp: &mut Interpreter,
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<Option<Value>> {
    let mut state = state_rc.borrow_mut();
    let state = state.downcast_mut::<CounterRepeatState>().ok_or_else(|| {
        PyError::Runtime("internal: Counter repeat iterator state corrupted".to_string())
    })?;
    if state.remaining == 0 {
        return Ok(None);
    }
    state.remaining -= 1;
    Ok(Some(state.object.clone()))
}

pub(super) fn elements(class: Option<Rc<RefCell<PyClass>>>, repeaters: Value) -> Value {
    crate::builtin_modules::itertools::native_iterators::chain_from_outer(class, repeaters)
}
