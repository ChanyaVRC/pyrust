//! Opaque native cursors for the iterator forms that need interpreter-aware
//! advancement.
//!
//! Their algorithms, private state, diagnostics, and presentation identity are
//! part of `itertools`; the generic runtime sees only `ProviderIterator`.

use std::any::Any;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{Interpreter, NativeIterFrame, ProviderIterator, make_iterator};
use crate::value::{PyClass, Value, ValueKind};

struct ChainFromIterableState {
    outer: Value,
    inner: Option<Value>,
    done: bool,
}

pub(crate) fn chain_from_outer(class: Option<Rc<RefCell<PyClass>>>, outer: Value) -> Value {
    Value::generator(Box::new(ProviderIterator::new(
        Box::new(ChainFromIterableState {
            outer,
            inner: None,
            done: false,
        }),
        advance_chain_from_iterable,
        "itertools.chain",
        "chain",
        class,
    )))
}

fn advance_chain_from_iterable(
    interp: &mut Interpreter,
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<Option<Value>> {
    loop {
        let inner = {
            let state = state_rc.borrow();
            let state = state
                .downcast_ref::<ChainFromIterableState>()
                .ok_or_else(|| {
                    PyError::Runtime(
                        "internal: chain.from_iterable iterator state corrupted".to_string(),
                    )
                })?;
            if state.done {
                return Ok(None);
            }
            state.inner.clone()
        };

        if let Some(inner) = inner {
            // Preserve the existing native-frame specialization without
            // exposing this provider's cursor to generic iteration.
            if let ValueKind::Generator(inner_rc) = inner.kind()
                && let Ok(mut inner_borrow) = inner_rc.try_borrow_mut()
                && let Some(native) = inner_borrow.downcast_mut::<NativeIterFrame>()
            {
                if let Some(value) = native.advance()? {
                    return Ok(Some(value));
                }
                drop(inner_borrow);
                chain_state_mut(state_rc)?.inner = None;
                continue;
            }

            match interp.call_next(&inner, None) {
                Ok(value) => return Ok(Some(value)),
                Err(error) if super::is_stop_iteration(&error) => {
                    chain_state_mut(state_rc)?.inner = None;
                    continue;
                }
                // Retain the current inner after a non-exhaustion error so a
                // caught exception resumes the same iterator.
                Err(error) => return Err(error),
            }
        }

        let outer = {
            let state = state_rc.borrow();
            state
                .downcast_ref::<ChainFromIterableState>()
                .ok_or_else(|| {
                    PyError::Runtime(
                        "internal: chain.from_iterable iterator state corrupted".to_string(),
                    )
                })?
                .outer
                .clone()
        };
        let next_iterable = match interp.call_next(&outer, None) {
            Ok(value) => value,
            Err(error) if super::is_stop_iteration(&error) => {
                chain_state_mut(state_rc)?.done = true;
                return Ok(None);
            }
            Err(error) => {
                // The outer source is permanently released after its first
                // non-exhaustion failure, matching the previous lazy cursor.
                chain_state_mut(state_rc)?.done = true;
                return Err(error);
            }
        };
        chain_state_mut(state_rc)?.inner = Some(make_iterator(interp, &next_iterable)?);
    }
}

fn chain_state_mut(
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<std::cell::RefMut<'_, ChainFromIterableState>> {
    std::cell::RefMut::filter_map(state_rc.borrow_mut(), |state| {
        state.downcast_mut::<ChainFromIterableState>()
    })
    .map_err(|_| {
        PyError::Runtime("internal: chain.from_iterable iterator state corrupted".to_string())
    })
}

struct TeeSharedState {
    source: Value,
    buffer: VecDeque<Value>,
    base: usize,
    positions: Vec<Option<usize>>,
    exhausted: bool,
    advancing: bool,
}

impl TeeSharedState {
    fn reclaim_consumed_prefix(&mut self) {
        let next_base = self
            .positions
            .iter()
            .flatten()
            .copied()
            .min()
            .unwrap_or_else(|| self.base.saturating_add(self.buffer.len()));
        let discard = next_base.saturating_sub(self.base).min(self.buffer.len());
        for _ in 0..discard {
            self.buffer.pop_front();
        }
        self.base = next_base;
    }
}

struct TeeCursor {
    shared: Rc<RefCell<TeeSharedState>>,
    slot: usize,
}

impl Drop for TeeCursor {
    fn drop(&mut self) {
        if let Ok(mut shared) = self.shared.try_borrow_mut() {
            if let Some(position) = shared.positions.get_mut(self.slot) {
                *position = None;
            }
            shared.reclaim_consumed_prefix();
        }
    }
}

pub(super) fn tee_iterators(
    interp: &mut Interpreter,
    iterable: &Value,
    count: usize,
) -> Result<Vec<Value>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let source = make_iterator(interp, iterable)?;
    let shared = Rc::new(RefCell::new(TeeSharedState {
        source,
        buffer: VecDeque::new(),
        base: 0,
        positions: vec![Some(0); count],
        exhausted: false,
        advancing: false,
    }));
    Ok((0..count)
        .map(|slot| {
            Value::generator(Box::new(ProviderIterator::new(
                Box::new(TeeCursor {
                    shared: Rc::clone(&shared),
                    slot,
                }),
                advance_tee,
                "itertools._tee",
                "_tee",
                None,
            )))
        })
        .collect())
}

fn advance_tee(
    interp: &mut Interpreter,
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<Option<Value>> {
    let (shared_rc, slot) = {
        let state = state_rc.borrow();
        let iterator = state.downcast_ref::<TeeCursor>().ok_or_else(|| {
            PyError::Runtime("internal: itertools tee iterator state corrupted".to_string())
        })?;
        (Rc::clone(&iterator.shared), iterator.slot)
    };

    let source = {
        let mut shared = shared_rc.borrow_mut();
        let position = shared
            .positions
            .get(slot)
            .and_then(|position| *position)
            .ok_or_else(|| PyError::Runtime("inactive tee iterator".to_string()))?;
        let buffered_end = shared.base.saturating_add(shared.buffer.len());
        if position < buffered_end {
            let value = shared.buffer[position - shared.base].clone();
            shared.positions[slot] = Some(position.saturating_add(1));
            shared.reclaim_consumed_prefix();
            return Ok(Some(value));
        }
        if shared.exhausted {
            return Ok(None);
        }
        if shared.advancing {
            return Err(PyError::Runtime(
                "cannot re-enter the tee iterator".to_string(),
            ));
        }
        shared.advancing = true;
        shared.source.clone()
    };

    let next = interp.call_next(&source, None);
    let mut shared = shared_rc.borrow_mut();
    shared.advancing = false;
    match next {
        Ok(value) => {
            let position = shared
                .positions
                .get(slot)
                .and_then(|position| *position)
                .ok_or_else(|| PyError::Runtime("inactive tee iterator".to_string()))?;
            shared.buffer.push_back(value.clone());
            shared.positions[slot] = Some(position.saturating_add(1));
            shared.reclaim_consumed_prefix();
            Ok(Some(value))
        }
        Err(error) if super::is_stop_iteration(&error) => {
            shared.exhausted = true;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}
