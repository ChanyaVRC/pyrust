/// Length hint for pre-sizing a list-comprehension accumulator
/// (`BuildListReserve`).
///
/// Only values whose length can be read without invoking Python code qualify.
/// This is a capacity hint, so unknown or user-observable length protocols
/// deliberately return zero.
#[inline]
pub(super) fn list_reserve_hint(src: &Value) -> usize {
    use pyrust_core::range_len;

    match src.kind() {
        ValueKind::List(items) => items.len(),
        ValueKind::Tuple(items) => items.len(),
        ValueKind::Set(items) => items.len(),
        ValueKind::Dict(items) => items.len(),
        ValueKind::Bytes(rc) => rc.len(),
        // Byte length is a safe upper bound on the number of Unicode scalars.
        ValueKind::Str(text) => text.len(),
        ValueKind::Range { start, stop, step } => {
            let len = range_len(start, stop, step);
            // A reserve hint must never turn a mathematically valid wide range
            // into a capacity-overflow panic before iteration starts.
            i64::try_from(len)
                .ok()
                .and_then(|len| usize::try_from(len).ok())
                .unwrap_or(0)
        }
        _ => 0,
    }
}

/// Build the already-formatted fragments of an f-string with one allocation.
///
/// The compiler guarantees that every fragment is a string. Keeping the
/// capacity scan and representation specialization here leaves the VM with
/// register decoding and result placement only.
#[inline]
pub(super) fn build_string_fast(
    regs: &RegSlice,
    base: crate::bytecode::Reg,
    count: u8,
    num_locals: crate::bytecode::Reg,
) -> Result<Value> {
    let count = crate::bytecode::Reg::from(count);
    let mut capacity = 0usize;
    for offset in 0..count {
        let value = vm_read(regs, base + offset, num_locals)?;
        capacity += value.as_str().map(str::len).unwrap_or(0);
    }

    let mut output = String::with_capacity(capacity);
    for offset in 0..count {
        let value = vm_read(regs, base + offset, num_locals)?;
        if let Some(fragment) = value.as_str() {
            output.push_str(fragment);
        }
    }
    Ok(Value::string(output))
}
