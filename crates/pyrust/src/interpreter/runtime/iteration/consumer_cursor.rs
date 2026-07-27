/// Internal cursor for built-ins that consume an iterable one item at a time.
///
/// Exact lists and tuples keep the observed source `Value` plus a live index,
/// avoiding the generator/`Any` wrapper paid by [`make_iterator`]. Everything
/// else retains the canonical iterator object so custom protocols, subclasses,
/// and already-created iterators keep their existing identity and semantics.
pub(crate) struct ConsumerIterator {
    backend: ConsumerIteratorBackend,
}

enum ConsumerIteratorBackend {
    Indexed {
        source: Value,
        pos: usize,
        exhausted: bool,
    },
    Protocol(Value),
}

/// Read one indexed sequence item without letting a list's `RefCell` guard
/// escape the operation. Callers may therefore invoke arbitrary Python code
/// immediately after this function returns.
#[inline(always)]
pub(crate) fn indexed_sequence_item(value: &Value, index: usize) -> Option<Value> {
    if let Some(items) = value.as_list() {
        items.get(index).cloned()
    } else {
        value.as_tuple().and_then(|items| items.get(index).cloned())
    }
}

#[inline]
fn advance_indexed(source: &Value, pos: &mut usize, exhausted: &mut bool) -> Option<Value> {
    if *exhausted {
        return None;
    }
    let current = *pos;
    if let Some(item) = indexed_sequence_item(source, current) {
        *pos = current + 1;
        Some(item)
    } else {
        // Like a real list/tuple iterator, exhaustion is permanent even if an
        // aliased list is subsequently extended.
        *exhausted = true;
        None
    }
}

impl ConsumerIterator {
    /// Acquire an iterable for an internal streaming consumer.
    ///
    /// The exact-list/tuple branch is unobservable: those built-in types cannot
    /// override `__iter__`. Subclasses are `PyInstance` values and therefore
    /// take the protocol branch below.
    pub(crate) fn new(interp: &mut Interpreter, source: &Value) -> Result<Self> {
        let is_exact_indexed = matches!(source.kind(), ValueKind::List(_) | ValueKind::Tuple(_));
        let backend = if is_exact_indexed {
            ConsumerIteratorBackend::Indexed {
                source: source.clone(),
                pos: 0,
                exhausted: false,
            }
        } else {
            ConsumerIteratorBackend::Protocol(make_iterator(interp, source)?)
        };
        Ok(Self { backend })
    }

    /// Advance one item, translating only a real `StopIteration` into normal
    /// exhaustion. Every other exception propagates immediately.
    #[inline]
    pub(crate) fn next(&mut self, interp: &mut Interpreter) -> Result<Option<Value>> {
        match &mut self.backend {
            ConsumerIteratorBackend::Indexed {
                source,
                pos,
                exhausted,
            } => Ok(advance_indexed(source, pos, exhausted)),
            ConsumerIteratorBackend::Protocol(iterator) => match interp.call_next(iterator, None) {
                Ok(value) => Ok(Some(value)),
                Err(ref error) if is_stop_iteration_error(error) => Ok(None),
                Err(error) => Err(error),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::advance_indexed;
    use crate::value::Value;

    #[test]
    fn indexed_list_cursor_observes_live_changes_and_latches_exhaustion() {
        let source = Value::list(vec![Value::int(1), Value::int(2)]);
        let mut pos = 0;
        let mut exhausted = false;

        assert_eq!(
            advance_indexed(&source, &mut pos, &mut exhausted).and_then(|value| value.as_int()),
            Some(1)
        );
        source.list_push(Value::int(3)).expect("append succeeds");
        source.list_pop_at(1).expect("delete succeeds");
        assert_eq!(
            advance_indexed(&source, &mut pos, &mut exhausted).and_then(|value| value.as_int()),
            Some(3)
        );
        assert!(advance_indexed(&source, &mut pos, &mut exhausted).is_none());

        source
            .list_push(Value::int(4))
            .expect("append after exhaustion succeeds");
        assert!(advance_indexed(&source, &mut pos, &mut exhausted).is_none());
    }

    #[test]
    fn indexed_tuple_cursor_preserves_order() {
        let source = Value::tuple(vec![Value::int(4), Value::int(5)]);
        let mut pos = 0;
        let mut exhausted = false;

        assert_eq!(
            advance_indexed(&source, &mut pos, &mut exhausted).and_then(|value| value.as_int()),
            Some(4)
        );
        assert_eq!(
            advance_indexed(&source, &mut pos, &mut exhausted).and_then(|value| value.as_int()),
            Some(5)
        );
        assert!(advance_indexed(&source, &mut pos, &mut exhausted).is_none());
    }
}
