// VM-local iterator state specializations.
//
// The opcode loop owns control transfer and generator-frame switching. This
// file owns the representation-specific one-step operations for indexed,
// guarded, and range states, including fused two-target unpack.

/// Compact result returned across the fast-path/VM boundary.
///
/// The fast-path domain writes yielded values to their destination registers
/// itself. This keeps the two-`Value` pair out of the return ABI while the
/// opcode loop stays independent of concrete iterator representations.
pub(super) enum LoopFastOutcome {
    Advanced,
    Exhausted,
    UserDefined,
    Error(Box<PyError>),
}

#[inline(always)]
pub(super) fn advance_loop_fast_state(
    state: &mut IterState,
    code: &crate::bytecode::FnCode,
    regs: &mut RegSlice,
    pc: &mut usize,
    dst: crate::bytecode::Reg,
) -> LoopFastOutcome {
    macro_rules! advanced_item {
        ($value:expr) => {{
            regs[dst as usize] = $value;
            LoopFastOutcome::Advanced
        }};
    }
    macro_rules! advanced_pair {
        ($first:expr, $second:expr) => {{
            store_loop_pair(code, regs, pc, dst, $first, $second);
            LoopFastOutcome::Advanced
        }};
    }
    macro_rules! fast_try {
        ($result:expr) => {{
            match $result {
                Ok(value) => value,
                Err(error) => return LoopFastOutcome::Error(Box::new(error)),
            }
        }};
    }

    match state {
        IterState::ValueIndexed { value, pos } => {
            let current = *pos;
            match indexed_sequence_item(value, current) {
                Some(value) => {
                    *pos = current + 1;
                    advanced_item!(value)
                }
                _ => LoopFastOutcome::Exhausted,
            }
        }
        IterState::StrAsciiIndexed { value, pos } => {
            let current = *pos;
            let string = value.as_str().unwrap_or("");
            if current < string.len() {
                let value = Value::string(&string[current..current + 1]);
                *pos = current + 1;
                advanced_item!(value)
            } else {
                LoopFastOutcome::Exhausted
            }
        }
        IterState::BytesIndexed { value, pos } => {
            let current = *pos;
            let item = match value.kind() {
                ValueKind::Bytes(bytes) => bytes.get(current).copied(),
                _ => None,
            };
            if let Some(byte) = item {
                *pos = current + 1;
                advanced_item!(Value::int(byte as i64))
            } else {
                LoopFastOutcome::Exhausted
            }
        }
        IterState::StrCodepointIndexed { value, byte_pos } => {
            let Some(text) = value.as_str() else {
                return LoopFastOutcome::Exhausted;
            };
            if let Some((codepoint, next_pos)) = pyrust_core::cesu8_next_codepoint(text, *byte_pos)
            {
                *byte_pos = next_pos;
                advanced_item!(Value::string(pyrust_core::cesu8_encode_codepoint(
                    codepoint,
                )))
            } else {
                LoopFastOutcome::Exhausted
            }
        }
        IterState::Materialized(items, pos) => {
            let current = *pos;
            if current < items.len() {
                // SAFETY: the branch checked the index against the same vector.
                let value = unsafe { items.get_unchecked(current).clone() };
                *pos = current + 1;
                advanced_item!(value)
            } else {
                LoopFastOutcome::Exhausted
            }
        }
        IterState::MaterializedGuarded {
            items,
            pos,
            container,
            recorded_len,
            msg,
            exhaust_first,
            provider_sequence,
        } => {
            if *exhaust_first && *pos >= items.len() {
                return LoopFastOutcome::Exhausted;
            }
            fast_try!(ensure_loop_collection_unchanged(
                container,
                *recorded_len,
                msg,
                *provider_sequence,
            ));
            let current = *pos;
            if current < items.len() {
                // SAFETY: the branch checked the index against the same vector.
                let value = unsafe { items.get_unchecked(current).clone() };
                *pos = current + 1;
                advanced_item!(value)
            } else {
                LoopFastOutcome::Exhausted
            }
        }
        IterState::LiveKeysGuarded { cursor, container } => {
            if let Some(value) = crate::interpreter::iteration::next_frozen_key(cursor) {
                return advanced_item!(value);
            }
            if cursor.snapshot.is_some() {
                match fast_try!(
                    crate::interpreter::iteration::advance_stable_snapshot_cursor(
                        container, cursor,
                    )
                ) {
                    crate::interpreter::iteration::StableSnapshotAdvance::Item(
                        LiveDictViewItem::Item(value),
                    ) => return advanced_item!(value),
                    crate::interpreter::iteration::StableSnapshotAdvance::Item(
                        LiveDictViewItem::Pair(key, value),
                    ) => return advanced_pair!(key, value),
                    crate::interpreter::iteration::StableSnapshotAdvance::Exhausted => {
                        return LoopFastOutcome::Exhausted;
                    }
                    crate::interpreter::iteration::StableSnapshotAdvance::Changed => {}
                }
            }
            match fast_try!(crate::interpreter::iteration::advance_live_key_cursor(
                container, cursor,
            )) {
                Some(LiveDictViewItem::Item(value)) => advanced_item!(value),
                Some(LiveDictViewItem::Pair(key, value)) => advanced_pair!(key, value),
                None => LoopFastOutcome::Exhausted,
            }
        }
        IterState::DictViewGuarded {
            keys,
            kind,
            pos,
            container,
            recorded_len,
            msg,
            exhaust_first,
            provider_sequence,
        } => {
            if *exhaust_first && *pos >= keys.len() {
                return LoopFastOutcome::Exhausted;
            }
            fast_try!(ensure_loop_collection_unchanged(
                container,
                *recorded_len,
                msg,
                *provider_sequence,
            ));
            let current = *pos;
            if current < keys.len() {
                // SAFETY: the branch checked the index against the same vector.
                let key = unsafe { keys.get_unchecked(current) };
                let item = match fast_try!(live_dict_view_item(container, key, *kind)) {
                    LiveDictViewItem::Item(value) => advanced_item!(value),
                    LiveDictViewItem::Pair(key, value) => advanced_pair!(key, value),
                };
                *pos = current + 1;
                item
            } else {
                LoopFastOutcome::Exhausted
            }
        }
        IterState::Range { cur, stop, step } => {
            let exhausted = if *step > 0 {
                *cur >= *stop
            } else {
                *cur <= *stop
            };
            if exhausted {
                LoopFastOutcome::Exhausted
            } else {
                let value = Value::int(*cur);
                // Construction promotes a cursor whose one-past-the-end value
                // cannot fit i64 to BigRange. Keep the step checked as a
                // defensive invariant boundary: if a future constructor misses
                // that promotion, overflow means the mathematical next value
                // is already beyond this i64 stop, so latching at stop safely
                // exhausts instead of wrapping into a near-infinite loop.
                *cur = cur.checked_add(*step).unwrap_or(*stop);
                advanced_item!(value)
            }
        }
        IterState::BigRange(state) => {
            let BigRangeState { cur, stop, step } = &mut **state;
            let exhausted = if step.sign() == pyrust_core::PyBigIntSign::Plus {
                *cur >= *stop
            } else {
                *cur <= *stop
            };
            if exhausted {
                LoopFastOutcome::Exhausted
            } else {
                let value = value_from_bigint(cur.clone());
                *cur += &*step;
                advanced_item!(value)
            }
        }
        IterState::EnumerateElements(cursor) => {
            match crate::interpreter::iteration::advance_enumerate_elements(cursor) {
                Some((index, item)) => advanced_pair!(index, item),
                None => LoopFastOutcome::Exhausted,
            }
        }
        IterState::UserDefined(_) => LoopFastOutcome::UserDefined,
    }
}

#[inline(always)]
fn ensure_loop_collection_unchanged(
    container: &Value,
    recorded_len: usize,
    default_message: &'static str,
    provider_sequence: u64,
) -> Result<()> {
    if live_collection_len(container) == Some(recorded_len) {
        return Ok(());
    }
    let message = if provider_sequence != 0 {
        ordered_mapping_guard_message(container, recorded_len, provider_sequence)
    } else {
        default_message
    };
    Err(PyError::Runtime(message.to_string()))
}

#[inline(always)]
pub(super) fn store_loop_pair(
    code: &crate::bytecode::FnCode,
    regs: &mut RegSlice,
    pc: &mut usize,
    dst: crate::bytecode::Reg,
    first: Value,
    second: Value,
) {
    if let Some(crate::bytecode::Insn::Unpack(base, source, 2)) = code.insns.get(*pc)
        && *source == dst
    {
        let base = *base as usize;
        regs[base] = first;
        regs[base + 1] = second;
        *pc += 1;
    } else {
        regs[dst as usize] = Value::tuple(vec![first, second]);
    }
}
