/// Per-frame iterator slots. The execution loop stores these slots, while the
/// iteration domain owns the state variants placed in them. Inline-one keeps
/// suspended generator frames compact; nested loops spill additional slots to
/// the heap.
pub(crate) type ItersBuf = smallvec::SmallVec<[Option<IterState>; 1]>;

/// Per-slot cache of guarded `__next__` resolutions used by the loop opcode.
pub(crate) type IterCacheBuf = smallvec::SmallVec<[Option<IterNextCacheEntry>; 4]>;

/// A `ForIter` cache entry for a user iterator's unbound `__next__` method.
///
/// The method is reusable only while the iterator keeps the same class and no
/// class in its MRO has mutated.  Keeping the class allocation weak avoids
/// retaining an otherwise-dead iterator type through a suspended frame.
#[derive(Clone)]
pub(crate) struct IterNextCacheEntry {
    pub(crate) class: std::rc::Weak<std::cell::RefCell<pyrust_core::PyClass>>,
    pub(crate) class_version: u64,
    pub(crate) epoch: u64,
    pub(crate) method: Option<Value>,
}

/// Boxed payload for [`IterState::BigRange`] so the uncommon arbitrary-
/// precision variant does not inflate every VM frame.
#[derive(Clone)]
pub(crate) struct BigRangeState {
    pub(crate) cur: PyBigInt,
    pub(crate) stop: PyBigInt,
    pub(crate) step: PyBigInt,
}

/// Canonical state consumed by the VM's `ForIter` instruction.
#[derive(Clone)]
pub(crate) enum IterState {
    Materialized(Vec<Value>, usize),
    Range {
        cur: i64,
        stop: i64,
        step: i64,
    },
    BigRange(Box<BigRangeState>),
    /// Owns the exact list/tuple selected by `iter(source)`.  The iterator must
    /// retain this value even if the source variable is rebound or deleted.
    ValueIndexed {
        value: Value,
        pos: usize,
    },
    /// Walks an immutable ASCII string by byte offset.
    StrAsciiIndexed {
        value: Value,
        pos: usize,
    },
    /// Walks immutable bytes without allocating an element snapshot.
    BytesIndexed {
        value: Value,
        pos: usize,
    },
    /// Walks a `bytearray`'s live buffer by index, re-reading its size on every
    /// step exactly as CPython's `bytearray_iterator` does (#2921).
    ///
    /// The storage cell is the retained source: a `bytearray` allocates it once
    /// and only ever writes it in place, so the walk keeps reading the object's
    /// live buffer after the source variable is rebound or deleted.
    BytearrayIndexed {
        data: ByteArrayBuffer,
        pos: usize,
    },
    /// Walks a non-ASCII UTF-8/CESU-8 string with an incremental byte cursor.
    StrCodepointIndexed {
        value: Value,
        byte_pos: usize,
    },
    /// Iterator object produced by the generic Python iteration protocol.
    UserDefined(Value),
    /// `enumerate(...)` over a built-in element iterator or a range cursor.
    /// The counter and the element position stay in the cells the enumerate
    /// object and its inner iterator own, so this is a cost specialization
    /// only.
    EnumerateElements(EnumerateElementCursor),
    /// Snapshot plus a live collection-size guard.
    MaterializedGuarded {
        items: Vec<Value>,
        pos: usize,
        container: Value,
        recorded_len: usize,
        msg: &'static str,
        /// Provider-tagged ordered mappings may test exhaustion first.
        exhaust_first: bool,
        /// Provider-owned guard state for a tagged ordered mapping; `None`
        /// for every other guarded collection.
        ordered_watch: Option<OrderedIterationWatch>,
    },
    /// Live key walk for plain dict/set containers and plain dict views.
    LiveKeysGuarded {
        cursor: LiveKeyCursor,
        container: Value,
    },
    /// Dict-view key snapshot with live value lookup on each step.
    DictViewGuarded {
        keys: Vec<PyKey>,
        kind: pyrust_builtins::dict_views::DictViewKind,
        pos: usize,
        container: Value,
        recorded_len: usize,
        msg: &'static str,
        exhaust_first: bool,
        ordered_watch: Option<OrderedIterationWatch>,
    },
}

impl Interpreter {
    /// Build the state consumed by the VM's `ForIter` instruction.
    ///
    /// The VM owns the state slot; this domain owns Python iteration
    /// classification, user protocol dispatch, concrete iterator adapters, and
    /// live-collection mutation guards.
    pub(crate) fn make_loop_iter_state(&mut self, source: Value) -> Result<IterState> {
        if is_coroutine_value(&source) {
            let type_name = full_type_name_str(&source);
            return Err(pyrust_core::type_err!(
                "'{type_name}' object is not iterable"
            ));
        }

        enum IterKind {
            Range(i64, i64, i64),
            BigRange(PyBigInt, PyBigInt, PyBigInt),
            Generator,
            Instance(Rc<RefCell<PyInstance>>),
            Metaclass(Rc<RefCell<PyClass>>),
            BuiltinIterator,
            MappingProxy(Value),
            IndexedValue,
            Other,
        }

        let kind = match source.kind() {
            ValueKind::Range { start, stop, step } => IterKind::Range(start, stop, step),
            ValueKind::BigRange { start, stop, step } => {
                IterKind::BigRange(start.clone(), stop.clone(), step.clone())
            }
            ValueKind::Generator(_) => IterKind::Generator,
            ValueKind::PyInstance(instance) => IterKind::Instance(Rc::clone(instance)),
            ValueKind::PyClass(class)
                if metaclass_dunder(class, "__iter__").is_some()
                    || metaclass_dunder(class, "__getitem__").is_some() =>
            {
                IterKind::Metaclass(Rc::clone(class))
            }
            ValueKind::BuiltinObject { ops, state }
                if pyrust_builtins::mapping_proxy::is_object_proxy_ops(ops) =>
            {
                IterKind::MappingProxy(
                    pyrust_builtins::mapping_proxy::owner_from_state(state)
                        .expect("object mappingproxy state"),
                )
            }
            ValueKind::BuiltinObject { ops, .. } if ops.is_iterator() => IterKind::BuiltinIterator,
            ValueKind::List(_) | ValueKind::Tuple(_) => IterKind::IndexedValue,
            _ => IterKind::Other,
        };

        match kind {
            IterKind::Range(start, stop, step) => {
                if step == 0 {
                    return Err(pyrust_core::value_err!("range() arg 3 must not be zero"));
                }
                if i64_range_native_cursor_safe(start, stop, step) {
                    Ok(IterState::Range {
                        cur: start,
                        stop,
                        step,
                    })
                } else {
                    Ok(IterState::BigRange(Box::new(BigRangeState {
                        cur: PyBigInt::from(start),
                        stop: PyBigInt::from(stop),
                        step: PyBigInt::from(step),
                    })))
                }
            }
            IterKind::BigRange(start, stop, step) => {
                Ok(IterState::BigRange(Box::new(BigRangeState {
                    cur: start,
                    stop,
                    step,
                })))
            }
            IterKind::Generator => Ok(match enumerate_element_cursor(&source) {
                Some(cursor) => IterState::EnumerateElements(cursor),
                None => IterState::UserDefined(source),
            }),
            IterKind::IndexedValue => Ok(IterState::ValueIndexed {
                value: source,
                pos: 0,
            }),
            IterKind::Instance(instance) => self.make_instance_loop_state(source, instance),
            IterKind::Metaclass(class) => match resolve_metaclass_iter_slot(self, &class)? {
                ResolvedIterSlot::Iterator(iterator) => {
                    Ok(IterState::UserDefined(validate_iterator_result(iterator)?))
                }
                ResolvedIterSlot::LookupFailed | ResolvedIterSlot::Missing => {
                    let getitem = metaclass_dunder_for_call(&class, "__getitem__").transpose()?;
                    match resolve_iter_fallback(&source, getitem)? {
                        IterFallback::GetItem(method) => Ok(IterState::UserDefined(
                            make_getitem_iterator(source, method),
                        )),
                        IterFallback::Missing => Err(pyrust_core::type_err!(
                            "'{}' object is not iterable",
                            value_type_name_str(&source)
                        )),
                        IterFallback::NativeBacking(_) => unreachable!(),
                    }
                }
                ResolvedIterSlot::NonIterable => Err(pyrust_core::type_err!(
                    "'{}' object is not iterable",
                    value_type_name_str(&source)
                )),
            },
            IterKind::MappingProxy(owner) => self.make_loop_iter_state(owner),
            IterKind::BuiltinIterator => {
                if pyrust_builtins::bytearray::iter_elements(&source).is_some() {
                    Ok(IterState::Materialized(iter_values(&source)?, 0))
                } else {
                    Ok(IterState::UserDefined(source))
                }
            }
            IterKind::Other => self.make_builtin_loop_state(source),
        }
    }

    fn make_instance_loop_state(
        &mut self,
        source: Value,
        instance: Rc<RefCell<PyInstance>>,
    ) -> Result<IterState> {
        let class = Rc::clone(&instance.borrow().class);
        let iter_method = effective_user_iter(&class, &instance);
        let lookup_failed_backing = match resolve_instance_iter_slot(self, &instance, iter_method)?
        {
            ResolvedIterSlot::Iterator(iterator) => {
                let iterator = validate_iterator_result(iterator)?;
                return Ok(IterState::UserDefined(iterator));
            }
            ResolvedIterSlot::LookupFailed => {
                let getitem = lookup_class_attr(&class, "__getitem__");
                match resolve_iter_fallback(&source, getitem)? {
                    IterFallback::GetItem(method) => {
                        return Ok(IterState::UserDefined(make_getitem_iterator(
                            source, method,
                        )));
                    }
                    IterFallback::NativeBacking(backing) => Some(backing),
                    IterFallback::Missing => {
                        return Err(pyrust_core::type_err!(
                            "'{}' object is not iterable",
                            class.borrow().name
                        ));
                    }
                }
            }
            ResolvedIterSlot::NonIterable => {
                return Err(pyrust_core::type_err!(
                    "'{}' object is not iterable",
                    class.borrow().name
                ));
            }
            ResolvedIterSlot::Missing => None,
        };

        if let Some(backing) = lookup_failed_backing.or_else(|| instance_builtin_data(&instance)) {
            if matches!(backing.kind(), ValueKind::List(_) | ValueKind::Tuple(_)) {
                return Ok(IterState::ValueIndexed {
                    value: backing,
                    pos: 0,
                });
            }
            if let Some(data) = pyrust_builtins::bytearray::as_bytearray_rc(&backing) {
                return Ok(IterState::BytearrayIndexed { data, pos: 0 });
            }
            if backing.is_dict() {
                let recorded_len = backing.dict_len().unwrap_or(0);
                let ordered_policy = pyrust_builtins::ordered_mapping::class_policy(&class);
                let (msg, exhaust_first) = ordered_policy
                    .map(|policy| (policy.mutation_message, policy.exhaust_first))
                    .unwrap_or(("dictionary changed size during iteration", false));
                if ordered_policy.is_none() {
                    return Ok(IterState::LiveKeysGuarded {
                        cursor: LiveKeyCursor::dict(&source, 0, recorded_len)
                            .with_size_change_message(msg),
                        container: source,
                    });
                }
                let items = iter_values(&backing)?;
                let ordered_watch = Some(ordered_iteration_watch(&backing));
                return Ok(IterState::MaterializedGuarded {
                    items,
                    pos: 0,
                    container: source,
                    recorded_len,
                    msg,
                    exhaust_first,
                    ordered_watch,
                });
            }
            if backing.is_set() {
                return Ok(IterState::LiveKeysGuarded {
                    cursor: LiveKeyCursor::set(&source),
                    container: source,
                });
            }
            return Ok(IterState::Materialized(iter_values(&source)?, 0));
        }

        if lookup_class_attr(&class, "__getitem__").is_some() {
            return Ok(IterState::UserDefined(self.make_getitem_iter(instance)?));
        }
        Ok(IterState::Materialized(iter_values(&source)?, 0))
    }

    fn make_builtin_loop_state(&mut self, source: Value) -> Result<IterState> {
        if pyrust_builtins::instance_dict::is_instance_dict(&source) {
            return Ok(IterState::UserDefined(make_iterator(self, &source)?));
        }
        if pyrust_builtins::dict_views::view_kind(&source).is_some() {
            return Ok(self.make_dict_view_loop_state(source));
        }
        if matches!(source.kind(), ValueKind::Str(_)) && source.str_is_ascii() {
            return Ok(IterState::StrAsciiIndexed {
                value: source,
                pos: 0,
            });
        }
        if matches!(source.kind(), ValueKind::Str(_)) {
            return Ok(IterState::StrCodepointIndexed {
                value: source,
                byte_pos: 0,
            });
        }
        if matches!(source.kind(), ValueKind::Bytes(_)) {
            return Ok(IterState::BytesIndexed {
                value: source,
                pos: 0,
            });
        }
        if let Some(data) = pyrust_builtins::bytearray::as_bytearray_rc(&source) {
            return Ok(IterState::BytearrayIndexed { data, pos: 0 });
        }
        if let Some(recorded_len) = source.dict_len() {
            return Ok(IterState::LiveKeysGuarded {
                cursor: LiveKeyCursor::dict(&source, 0, recorded_len)
                    .with_size_change_message("dictionary changed size during iteration"),
                container: source,
            });
        }
        if source.set_len().is_some() {
            return Ok(IterState::LiveKeysGuarded {
                cursor: LiveKeyCursor::set(&source),
                container: source,
            });
        }

        let items = iter_values(&source)?;
        let Some(recorded_len) = live_collection_len(&source) else {
            return Ok(IterState::Materialized(items, 0));
        };
        let ordered_policy = pyrust_builtins::ordered_mapping::view_policy(&source);
        let (msg, exhaust_first) = if source.set_len().is_some() {
            ("Set changed size during iteration", false)
        } else {
            ordered_policy
                .map(|policy| (policy.mutation_message, policy.exhaust_first))
                .unwrap_or(("dictionary changed size during iteration", false))
        };
        let ordered_watch = ordered_policy
            .is_some()
            .then(|| ordered_iteration_watch(&source));
        Ok(IterState::MaterializedGuarded {
            items,
            pos: 0,
            container: source,
            recorded_len,
            msg,
            exhaust_first,
            ordered_watch,
        })
    }

    fn make_dict_view_loop_state(&self, source: Value) -> IterState {
        let kind =
            pyrust_builtins::dict_views::view_kind(&source).expect("dict view has a known kind");
        let dict =
            pyrust_builtins::dict_views::as_dict_rc(&source).expect("dict view has a backing dict");
        let recorded_len = pyrust_builtins::dict_views::backing_len(&dict);
        let ordered_policy = pyrust_builtins::ordered_mapping::view_policy(&source);
        let (msg, exhaust_first) = ordered_policy
            .map(|policy| (policy.mutation_message, policy.exhaust_first))
            .unwrap_or(("dictionary changed size during iteration", false));
        if ordered_policy.is_none() {
            // The live cursor owns its own key-order policy, so a plain view
            // must not pay for a key vector it would immediately discard.
            return IterState::LiveKeysGuarded {
                cursor: LiveKeyCursor::dict(&source, kind.live_cursor_code(), recorded_len),
                container: source,
            };
        }
        let keys = pyrust_builtins::dict_views::backing_keys(&dict);
        let ordered_watch = Some(ordered_iteration_watch(&source));
        IterState::DictViewGuarded {
            keys,
            kind,
            pos: 0,
            container: source,
            recorded_len,
            msg,
            exhaust_first,
            ordered_watch,
        }
    }
}

/// Classify `enumerate(sequence)` / `enumerate(range(...))` for the loop's
/// inline pair step.
///
/// Succeeds whenever the inner iterator is an unguarded element walk over a
/// list, tuple, snapshot, `bytes`, `bytearray`, or `str`, or the i64-backed
/// range cursor — including a partly consumed one: the specialization retains
/// both shared cells rather than moving their cursors, so there is nothing to
/// prove about aliasing. Every other inner iterator — a generator, `map`, an
/// arbitrary-precision `BigRangeIter`, a guarded dict/set cursor, a user
/// iterator — keeps the generic adapter path, whose per-step protocol dispatch
/// is exactly the observable behaviour.
fn enumerate_element_cursor(source: &Value) -> Option<EnumerateElementCursor> {
    let ValueKind::Generator(enumerate) = source.kind() else {
        return None;
    };
    let inner = {
        let borrow = enumerate.try_borrow().ok()?;
        borrow.downcast_ref::<EnumerateIter>()?.source.clone()
    };
    let ValueKind::Generator(cell) = inner.kind() else {
        return None;
    };
    let borrow = cell.try_borrow().ok()?;
    let inner = if let Some(frame) = borrow.downcast_ref::<NativeIterFrame>() {
        if !frame.is_unguarded_element_source() {
            return None;
        }
        EnumerateInnerCursor::Frame(Rc::clone(cell))
    } else if borrow.is::<RangeIter>() {
        EnumerateInnerCursor::Range(Rc::clone(cell))
    } else {
        return None;
    };
    Some(EnumerateElementCursor {
        enumerate: Rc::clone(enumerate),
        inner,
    })
}

pub(crate) enum LiveDictViewItem {
    Item(Value),
    Pair(Value, Value),
}

pub(crate) fn live_dict_view_item(
    container: &Value,
    key: &PyKey,
    kind: pyrust_builtins::dict_views::DictViewKind,
) -> Result<LiveDictViewItem> {
    let dict = pyrust_builtins::dict_views::as_dict_rc(container)
        .ok_or_else(|| PyError::Runtime("dict view lost its backing dict".to_string()))?;
    let map = dict.borrow();
    match kind {
        pyrust_builtins::dict_views::DictViewKind::Keys => {
            Ok(LiveDictViewItem::Item(key_to_value(key.clone())))
        }
        pyrust_builtins::dict_views::DictViewKind::Values => map
            .get(key)
            .cloned()
            .map(LiveDictViewItem::Item)
            .ok_or_else(|| {
                PyError::Runtime("dictionary keys changed during iteration".to_string())
            }),
        pyrust_builtins::dict_views::DictViewKind::Items => map
            .get(key)
            .cloned()
            .map(|value| LiveDictViewItem::Pair(key_to_value(key.clone()), value))
            .ok_or_else(|| {
                PyError::Runtime("dictionary keys changed during iteration".to_string())
            }),
    }
}
