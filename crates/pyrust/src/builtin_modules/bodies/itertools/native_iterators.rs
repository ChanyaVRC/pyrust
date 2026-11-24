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
    outer: Option<Value>,
    inner: Option<Value>,
    done: bool,
}

pub(crate) fn chain_from_outer(class: Option<Rc<RefCell<PyClass>>>, outer: Value) -> Value {
    Value::generator(Box::new(
        ProviderIterator::new(
            Box::new(ChainFromIterableState {
                outer: Some(outer),
                inner: None,
                done: false,
            }),
            advance_chain_from_iterable,
            "itertools.chain",
            "chain",
            class,
        )
        .with_copy_hook(copy_chain_from_iterable, false),
    ))
}

fn copy_chain_from_iterable(
    orig: &ProviderIterator,
    deep: bool,
    copy_child: &mut dyn FnMut(Value) -> Result<Value>,
    _memo_get: &mut dyn FnMut(i64) -> Option<Value>,
    memo_insert: &mut dyn FnMut(i64, Value),
    outer_obj_id: Option<i64>,
) -> Result<Value> {
    let (outer, inner, done) = {
        let state = orig.state_rc().borrow();
        let state = state
            .downcast_ref::<ChainFromIterableState>()
            .ok_or_else(|| {
                PyError::runtime(
                    "internal: chain.from_iterable iterator state corrupted".to_string(),
                )
            })?;
        (state.outer.clone(), state.inner.clone(), state.done)
    };
    if !deep {
        return Ok(Value::generator(Box::new(
            ProviderIterator::new(
                Box::new(ChainFromIterableState { outer, inner, done }),
                advance_chain_from_iterable,
                orig.full_type_name(),
                orig.fallback_class_name(),
                orig.class(),
            )
            .with_copy_hook(copy_chain_from_iterable, false),
        )));
    }
    let provider = ProviderIterator::new(
        Box::new(ChainFromIterableState {
            outer: None,
            inner: None,
            done,
        }),
        advance_chain_from_iterable,
        orig.full_type_name(),
        orig.fallback_class_name(),
        orig.class(),
    )
    .with_copy_hook(copy_chain_from_iterable, false);
    let state_rc = Rc::clone(provider.state_rc());
    let rebuilt = Value::generator(Box::new(provider));
    if let Some(id) = outer_obj_id {
        memo_insert(id, rebuilt.clone());
    }
    if !done {
        let copied_outer = match outer {
            Some(v) => Some(copy_child(v)?),
            None => None,
        };
        let copied_inner = match inner {
            Some(v) => Some(copy_child(v)?),
            None => None,
        };
        let mut state = chain_state_mut(&state_rc)?;
        state.outer = copied_outer;
        state.inner = copied_inner;
    }
    Ok(rebuilt)
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
                    PyError::runtime(
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

        let Some(outer) = ({
            let state = state_rc.borrow();
            state
                .downcast_ref::<ChainFromIterableState>()
                .ok_or_else(|| {
                    PyError::runtime(
                        "internal: chain.from_iterable iterator state corrupted".to_string(),
                    )
                })?
                .outer
                .clone()
        }) else {
            chain_state_mut(state_rc)?.done = true;
            return Ok(None);
        };
        let next_iterable = match interp.call_next(&outer, None) {
            Ok(value) => value,
            Err(error) if super::is_stop_iteration(&error) => {
                let mut state = chain_state_mut(state_rc)?;
                state.done = true;
                state.outer = None;
                state.inner = None;
                return Ok(None);
            }
            Err(error) => {
                // The outer source is permanently released after its first
                // non-exhaustion failure, matching the previous lazy cursor.
                let mut state = chain_state_mut(state_rc)?;
                state.done = true;
                state.outer = None;
                state.inner = None;
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
        PyError::runtime("internal: chain.from_iterable iterator state corrupted".to_string())
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

fn make_tee_value(shared: Rc<RefCell<TeeSharedState>>, slot: usize) -> Value {
    Value::generator(Box::new(
        ProviderIterator::new(
            Box::new(TeeCursor { shared, slot }),
            advance_tee,
            "itertools._tee",
            "_tee",
            None,
        )
        .with_copy_hook(copy_tee, true),
    ))
}

fn clone_tee_slot(shared: &Rc<RefCell<TeeSharedState>>, pos: usize) -> Value {
    let slot = {
        let mut s = shared.borrow_mut();
        let slot = s.positions.len();
        s.positions.push(Some(pos));
        slot
    };
    make_tee_value(Rc::clone(shared), slot)
}

fn tee_cursor_parts(value: &Value) -> Option<(Rc<RefCell<TeeSharedState>>, usize)> {
    let ValueKind::Generator(gen_rc) = value.kind() else {
        return None;
    };
    let borrow = gen_rc.borrow();
    let provider = borrow.downcast_ref::<ProviderIterator>()?;
    let state = provider.state_rc().borrow();
    let cursor = state.downcast_ref::<TeeCursor>()?;
    Some((Rc::clone(&cursor.shared), cursor.slot))
}

fn copy_tee(
    orig: &ProviderIterator,
    deep: bool,
    copy_child: &mut dyn FnMut(Value) -> Result<Value>,
    memo_get: &mut dyn FnMut(i64) -> Option<Value>,
    memo_insert: &mut dyn FnMut(i64, Value),
    outer_obj_id: Option<i64>,
) -> Result<Value> {
    let (shared_rc, slot) = {
        let state = orig.state_rc().borrow();
        let cursor = state.downcast_ref::<TeeCursor>().ok_or_else(|| {
            PyError::runtime("internal: itertools tee iterator state corrupted".to_string())
        })?;
        (Rc::clone(&cursor.shared), cursor.slot)
    };
    let position = shared_rc
        .borrow()
        .positions
        .get(slot)
        .and_then(|pos| *pos)
        .ok_or_else(|| PyError::runtime("inactive tee iterator".to_string()))?;
    if !deep {
        return Ok(clone_tee_slot(&shared_rc, position));
    }
    // Deep copy: share the deep-copied TeeSharedState across sibling branches
    // within the same deepcopy memo (matching CPython's `_teedataobject`
    // memoisation in `deepcopy((t1, t2))`).
    let shared_id = Rc::as_ptr(&shared_rc) as i64;
    if let Some(existing_tee) = memo_get(shared_id)
        && let Some((copied_shared, _)) = tee_cursor_parts(&existing_tee)
    {
        let rebuilt = clone_tee_slot(&copied_shared, position);
        if let Some(id) = outer_obj_id {
            memo_insert(id, rebuilt.clone());
        }
        return Ok(rebuilt);
    }
    let (source, buffer_items, base, exhausted) = {
        let shared = shared_rc.borrow();
        (
            shared.source.clone(),
            shared.buffer.iter().cloned().collect::<Vec<_>>(),
            shared.base,
            shared.exhausted,
        )
    };
    let new_shared = Rc::new(RefCell::new(TeeSharedState {
        source: Value::none(),
        buffer: VecDeque::new(),
        base,
        positions: vec![Some(position)],
        exhausted,
        advancing: false,
    }));
    let rebuilt = make_tee_value(Rc::clone(&new_shared), 0);
    if let Some(id) = outer_obj_id {
        memo_insert(id, rebuilt.clone());
    }
    memo_insert(shared_id, rebuilt.clone());
    // CPython's `_teedataobject.__reduce__` retains the source iterator even
    // after exhaustion, so deepcopying `tee(gen)` raises `cannot pickle
    // 'generator' object` in both live and exhausted states.
    let copied_source = copy_child(source)?;
    let mut copied_buffer = VecDeque::with_capacity(buffer_items.len());
    for item in buffer_items {
        copied_buffer.push_back(copy_child(item)?);
    }
    {
        let mut s = new_shared.borrow_mut();
        s.source = copied_source;
        s.buffer = copied_buffer;
    }
    Ok(rebuilt)
}

pub(super) fn tee_iterators(
    interp: &mut Interpreter,
    iterable: &Value,
    count: usize,
) -> Result<Vec<Value>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    // CPython's `itertools.tee(existing_tee, n)` reuses `existing_tee` as the
    // first output and calls `existing_tee.__copy__()` for the remaining `n-1`
    // outputs so the existing buffer is shared instead of double-buffered.
    if let Some((shared_rc, slot)) = tee_cursor_parts(iterable) {
        let position = shared_rc
            .borrow()
            .positions
            .get(slot)
            .and_then(|pos| *pos)
            .ok_or_else(|| PyError::runtime("inactive tee iterator".to_string()))?;
        let mut result = Vec::with_capacity(count);
        result.push(iterable.clone());
        for _ in 1..count {
            result.push(clone_tee_slot(&shared_rc, position));
        }
        return Ok(result);
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
        .map(|slot| make_tee_value(Rc::clone(&shared), slot))
        .collect())
}

fn advance_tee(
    interp: &mut Interpreter,
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<Option<Value>> {
    let (shared_rc, slot) = {
        let state = state_rc.borrow();
        let iterator = state.downcast_ref::<TeeCursor>().ok_or_else(|| {
            PyError::runtime("internal: itertools tee iterator state corrupted".to_string())
        })?;
        (Rc::clone(&iterator.shared), iterator.slot)
    };

    let source = {
        let mut shared = shared_rc.borrow_mut();
        let position = shared
            .positions
            .get(slot)
            .and_then(|position| *position)
            .ok_or_else(|| PyError::runtime("inactive tee iterator".to_string()))?;
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
            return Err(PyError::runtime(
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
                .ok_or_else(|| PyError::runtime("inactive tee iterator".to_string()))?;
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
