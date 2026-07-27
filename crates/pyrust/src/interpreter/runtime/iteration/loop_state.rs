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
    /// Walks a non-ASCII UTF-8/CESU-8 string with an incremental byte cursor.
    StrCodepointIndexed {
        value: Value,
        byte_pos: usize,
    },
    /// Iterator object produced by the generic Python iteration protocol.
    UserDefined(Value),
    /// Snapshot plus a live collection-size guard.
    MaterializedGuarded {
        items: Vec<Value>,
        pos: usize,
        container: Value,
        recorded_len: usize,
        msg: &'static str,
        /// Provider-tagged ordered mappings may test exhaustion first.
        exhaust_first: bool,
        /// Provider clear-registry sequence, or zero for other collections.
        provider_sequence: u64,
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
        provider_sequence: u64,
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
            Metaclass(Value),
            BuiltinIterator,
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
            ValueKind::PyClass(class) => metaclass_dunder(class, "__iter__")
                .map(IterKind::Metaclass)
                .unwrap_or(IterKind::Other),
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
            IterKind::Generator => Ok(IterState::UserDefined(source)),
            IterKind::IndexedValue => Ok(IterState::ValueIndexed {
                value: source,
                pos: 0,
            }),
            IterKind::Instance(instance) => self.make_instance_loop_state(source, instance),
            IterKind::Metaclass(iter_fn) => {
                let iterator = invoke_class_method(self, iter_fn, source, &[])?;
                Ok(IterState::UserDefined(validate_iterator_result(iterator)?))
            }
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
        if let Some(iter_method) = effective_user_iter(&class, &instance) {
            let iterator = invoke_class_method(
                self,
                iter_method,
                Value::py_instance(Rc::clone(&instance)),
                &[],
            )?;
            let iterator = validate_iterator_result(iterator)?;
            return Ok(IterState::UserDefined(iterator));
        }

        if let Some(backing) = instance_builtin_data(&instance) {
            if matches!(backing.kind(), ValueKind::List(_) | ValueKind::Tuple(_)) {
                return Ok(IterState::ValueIndexed {
                    value: backing,
                    pos: 0,
                });
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
                let provider_sequence = pyrust_builtins::ordered_mapping::clear_sequence();
                return Ok(IterState::MaterializedGuarded {
                    items,
                    pos: 0,
                    container: source,
                    recorded_len,
                    msg,
                    exhaust_first,
                    provider_sequence,
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
        let provider_sequence = if ordered_policy.is_some() {
            pyrust_builtins::ordered_mapping::clear_sequence()
        } else {
            0
        };
        Ok(IterState::MaterializedGuarded {
            items,
            pos: 0,
            container: source,
            recorded_len,
            msg,
            exhaust_first,
            provider_sequence,
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
        let provider_sequence = pyrust_builtins::ordered_mapping::clear_sequence();
        IterState::DictViewGuarded {
            keys,
            kind,
            pos: 0,
            container: source,
            recorded_len,
            msg,
            exhaust_first,
            provider_sequence,
        }
    }
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
