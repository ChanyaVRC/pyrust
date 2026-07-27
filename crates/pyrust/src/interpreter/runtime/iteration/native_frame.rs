impl NativeIterFrame {
    /// Construct an unguarded iterator over an owned element snapshot.
    pub(crate) fn new(items: Vec<Value>, type_name: &'static str) -> Self {
        NativeIterFrame {
            source: NativeIterSource::Materialized(items),
            pos: 0,
            type_name,
            guard: None,
            exhausted: false,
        }
    }

    /// Construct a live dict/set cursor. `kind` is 0/1/2 for dict
    /// keys/values/items and 3 for set keys.
    pub(crate) fn live_keys(container: Value, kind: u8, type_name: &'static str) -> Self {
        let len = live_collection_len(&container).unwrap_or(0);
        let cursor = if kind == 3 {
            LiveKeyCursor::set(&container)
        } else {
            LiveKeyCursor::dict(&container, kind, len)
        };
        NativeIterFrame {
            source: NativeIterSource::LiveKeys {
                container,
                cursor: Box::new(cursor),
            },
            pos: 0,
            type_name,
            guard: None,
            exhausted: false,
        }
    }

    /// Construct an independent live iterator over an `instance_dict` proxy.
    pub(crate) fn instance_dict(proxy: Value) -> Self {
        let recorded_len = pyrust_builtins::instance_dict::iter_visible_len(&proxy)
            .expect("instance_dict iterator requires an instance_dict proxy");
        NativeIterFrame {
            source: NativeIterSource::InstanceDict {
                proxy,
                recorded_len,
                size_changed: false,
            },
            pos: 0,
            type_name: "dict_keyiterator",
            guard: None,
            exhausted: false,
        }
    }

    /// Construct an O(1) live iterator over a list or tuple.
    pub(crate) fn indexed(source: Value, type_name: &'static str) -> Self {
        debug_assert!(matches!(
            source.kind(),
            ValueKind::List(_) | ValueKind::Tuple(_)
        ));
        NativeIterFrame {
            source: NativeIterSource::Indexed(source),
            pos: 0,
            type_name,
            guard: None,
            exhausted: false,
        }
    }

    /// Construct a reverse iterator with a fixed initial length and live
    /// per-index reads.
    pub(crate) fn reverse_indexed(source: Value, len: usize, type_name: &'static str) -> Self {
        NativeIterFrame {
            source: NativeIterSource::ReverseIndexed {
                value: source,
                next_index: len,
            },
            pos: 0,
            type_name,
            guard: None,
            exhausted: false,
        }
    }

    /// Construct a dict-view iterator that keeps key order stable while
    /// resolving values from the live mapping at each step.
    pub(crate) fn dict_view(
        dict: pyrust_builtins::dict_views::DictRc,
        keys: Vec<PyKey>,
        kind: pyrust_builtins::dict_views::DictViewKind,
        type_name: &'static str,
    ) -> Self {
        NativeIterFrame {
            source: NativeIterSource::DictView { dict, keys, kind },
            pos: 0,
            type_name,
            guard: None,
            exhausted: false,
        }
    }

    /// Construct a live, allocation-free iterator over deque storage.
    pub(crate) fn deque(
        data: pyrust_builtins::deque_storage::DequeData,
        type_name: &'static str,
    ) -> Self {
        NativeIterFrame {
            source: NativeIterSource::Deque(data),
            pos: 0,
            type_name,
            guard: None,
            exhausted: false,
        }
    }

    /// Construct a lazy iterator over immutable bytes.
    pub(crate) fn bytes(value: Value, type_name: &'static str) -> Self {
        debug_assert!(matches!(value.kind(), ValueKind::Bytes(_)));
        NativeIterFrame {
            source: NativeIterSource::Bytes(value),
            pos: 0,
            type_name,
            guard: None,
            exhausted: false,
        }
    }

    /// Construct a lazy UTF-8/CESU-8 codepoint iterator.
    pub(crate) fn string(value: Value, type_name: &'static str) -> Self {
        debug_assert!(matches!(value.kind(), ValueKind::Str(_)));
        NativeIterFrame {
            source: NativeIterSource::String { value, byte_pos: 0 },
            pos: 0,
            type_name,
            guard: None,
            exhausted: false,
        }
    }

    #[inline]
    pub(crate) fn source_len(&self) -> usize {
        match &self.source {
            NativeIterSource::Materialized(items) => items.len(),
            NativeIterSource::LiveKeys { container, cursor } => live_collection_len(container)
                .unwrap_or_else(|| cursor.yielded_len())
                .max(cursor.yielded_len()),
            NativeIterSource::InstanceDict { recorded_len, .. } => *recorded_len,
            NativeIterSource::Indexed(value) => match value.kind() {
                ValueKind::List(items) => items.len(),
                ValueKind::Tuple(items) => items.len(),
                _ => 0,
            },
            NativeIterSource::ReverseIndexed { next_index, .. } => {
                self.pos.saturating_add(*next_index)
            }
            NativeIterSource::DictView { keys, .. } => keys.len(),
            NativeIterSource::Deque(data) => data.borrow().len(),
            NativeIterSource::Bytes(value) => match value.kind() {
                ValueKind::Bytes(bytes) => bytes.len(),
                _ => 0,
            },
            NativeIterSource::String { value, .. } => value.as_str().map_or(0, str::len),
            NativeIterSource::Exhausted => 0,
        }
    }

    #[inline]
    fn source_item(&mut self, index: usize) -> Result<Option<Value>> {
        match &mut self.source {
            NativeIterSource::Materialized(items) => Ok(items.get(index).cloned()),
            NativeIterSource::LiveKeys { container, cursor } => {
                let item = advance_live_key_cursor(container, cursor)?;
                Ok(item.map(|item| match item {
                    LiveDictViewItem::Item(value) => value,
                    LiveDictViewItem::Pair(key, value) => Value::tuple(vec![key, value]),
                }))
            }
            NativeIterSource::InstanceDict {
                proxy,
                recorded_len,
                size_changed,
            } => {
                let live_len =
                    pyrust_builtins::instance_dict::iter_visible_len(proxy).ok_or_else(|| {
                        PyError::Runtime("instance_dict iterator lost its proxy".into())
                    })?;
                if *size_changed || live_len != *recorded_len {
                    *size_changed = true;
                    return Err(PyError::Runtime(
                        "dictionary changed size during iteration".to_string(),
                    ));
                }
                Ok(pyrust_builtins::instance_dict::iter_visible_key_at(
                    proxy, index,
                ))
            }
            NativeIterSource::Indexed(value) => match value.kind() {
                ValueKind::List(items) => Ok(items.get(index).cloned()),
                ValueKind::Tuple(items) => Ok(items.get(index).cloned()),
                _ => Ok(None),
            },
            NativeIterSource::ReverseIndexed { value, next_index } => {
                let Some(index) = next_index.checked_sub(1) else {
                    return Ok(None);
                };
                *next_index = index;
                Ok(match value.kind() {
                    ValueKind::List(items) => items.get(index).cloned(),
                    ValueKind::Tuple(items) => items.get(index).cloned(),
                    ValueKind::Bytes(bytes) => {
                        bytes.get(index).map(|byte| Value::int(*byte as i64))
                    }
                    ValueKind::Str(_) => {
                        let len = value.str_codepoint_len_for_index();
                        if index >= len {
                            None
                        } else {
                            let (start, end) = value.str_codepoint_byte_range(index);
                            Some(value.string_slice(start, end))
                        }
                    }
                    ValueKind::BuiltinObject { ops, state }
                        if ops.canonical_class_tag()
                            == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
                    {
                        if ops.len(state).is_none_or(|len| index >= len) {
                            None
                        } else {
                            Some(ops.get_item(state, &Value::int(index as i64))?)
                        }
                    }
                    _ => None,
                })
            }
            NativeIterSource::DictView { dict, keys, kind } => {
                let Some(key) = keys.get(index) else {
                    return Ok(None);
                };
                let map = dict.borrow();
                let value = match kind {
                    pyrust_builtins::dict_views::DictViewKind::Keys => key_to_value(key.clone()),
                    pyrust_builtins::dict_views::DictViewKind::Values => {
                        map.get(key).cloned().ok_or_else(|| {
                            PyError::Runtime("dictionary keys changed during iteration".to_string())
                        })?
                    }
                    pyrust_builtins::dict_views::DictViewKind::Items => {
                        let value = map.get(key).cloned().ok_or_else(|| {
                            PyError::Runtime("dictionary keys changed during iteration".to_string())
                        })?;
                        Value::tuple(vec![key_to_value(key.clone()), value])
                    }
                };
                Ok(Some(value))
            }
            NativeIterSource::Deque(data) => Ok(data.borrow().get(index).cloned()),
            NativeIterSource::Bytes(value) => Ok(match value.kind() {
                ValueKind::Bytes(bytes) => bytes.get(index).map(|byte| Value::int(*byte as i64)),
                _ => None,
            }),
            NativeIterSource::String { value, byte_pos } => {
                let Some(text) = value.as_str() else {
                    return Ok(None);
                };
                let Some((codepoint, next_pos)) =
                    pyrust_core::cesu8_next_codepoint(text, *byte_pos)
                else {
                    return Ok(None);
                };
                *byte_pos = next_pos;
                Ok(Some(Value::string(pyrust_core::cesu8_encode_codepoint(
                    codepoint,
                ))))
            }
            NativeIterSource::Exhausted => Ok(None),
        }
    }

    /// Drain all remaining elements and permanently exhaust the iterator.
    pub(crate) fn drain_remaining(&mut self) -> Result<Vec<Value>> {
        if self.exhausted {
            return Ok(Vec::new());
        }
        self.guard_check()?;
        let len = self.source_len();
        let mut remaining = Vec::with_capacity(len.saturating_sub(self.pos));
        loop {
            let index = self.pos;
            let Some(value) = self.source_item(index)? else {
                break;
            };
            remaining.push(value);
            self.pos += 1;
        }
        self.exhausted = true;
        self.source = NativeIterSource::Exhausted;
        Ok(remaining)
    }

    fn latch_exhausted(&mut self) {
        self.exhausted = true;
        if matches!(
            self.source,
            NativeIterSource::Indexed(_)
                | NativeIterSource::ReverseIndexed { .. }
                | NativeIterSource::LiveKeys { .. }
                | NativeIterSource::InstanceDict { .. }
                | NativeIterSource::DictView { .. }
                | NativeIterSource::Deque(_)
                | NativeIterSource::Bytes(_)
                | NativeIterSource::String { .. }
        ) {
            self.source = NativeIterSource::Exhausted;
        }
    }

    /// Check a dict/set/deque/provider-tagged mapping mutation guard.
    #[inline]
    fn guard_check(&self) -> Result<()> {
        // Live-key cursors own a shared mutation generation and perform their
        // size read only when that generation changes.
        if matches!(&self.source, NativeIterSource::LiveKeys { .. }) {
            return Ok(());
        }
        let Some(guard) = &self.guard else {
            return Ok(());
        };
        let live = match &guard.kind {
            GuardVersion::Size => live_collection_len(&guard.container).map(|len| len as i64),
            GuardVersion::DequeState { counter } => Some(counter.get()),
        };
        if live != Some(guard.version) {
            let message = if guard.provider_sequence != 0 {
                ordered_mapping_guard_message(
                    &guard.container,
                    guard.version as usize,
                    guard.provider_sequence,
                )
            } else {
                guard.msg
            };
            return Err(PyError::Runtime(message.to_string()));
        }
        Ok(())
    }

    /// Advance one element while preserving permanent exhaustion and mutation
    /// guard ordering.
    #[inline]
    pub(crate) fn advance(&mut self) -> Result<Option<Value>> {
        if self.exhausted {
            return Ok(None);
        }
        if let NativeIterSource::LiveKeys { container, cursor } = &mut self.source
            && cursor.snapshot.is_some()
        {
            match advance_stable_snapshot_cursor(container, cursor)? {
                StableSnapshotAdvance::Item(item) => {
                    self.pos += 1;
                    return Ok(Some(match item {
                        LiveDictViewItem::Item(value) => value,
                        LiveDictViewItem::Pair(key, value) => Value::tuple(vec![key, value]),
                    }));
                }
                StableSnapshotAdvance::Exhausted => {
                    self.exhausted = true;
                    self.source = NativeIterSource::Exhausted;
                    return Ok(None);
                }
                StableSnapshotAdvance::Changed => {}
            }
        }
        // Most guarded iterators report structural mutation before touching a
        // key/value that may no longer exist. A provider can request
        // exhaustion-first behavior for its snapshotted cursor.
        if self.guard.as_ref().is_some_and(|guard| guard.exhaust_first)
            && self.pos >= self.source_len()
        {
            self.latch_exhausted();
            return Ok(None);
        }
        self.guard_check()?;
        let pos = self.pos;
        if let Some(item) = self.source_item(pos)? {
            self.pos += 1;
            return Ok(Some(item));
        }
        self.latch_exhausted();
        Ok(None)
    }
}
