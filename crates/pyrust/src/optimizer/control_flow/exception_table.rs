// ─── Zero-cost exception table ─────────────────────────────────────────────────

/// Sentinel in an exception table meaning "no handler covers this pc".
pub(crate) const EXC_NO_HANDLER: u32 = u32::MAX;

/// Build a per-pc exception-handler table from the (PC-balanced)
/// `SetupExcept`/`PopExcept` structure, then strip those two instructions from
/// the stream (CPython 3.11 "zero-cost" model).
///
/// Returns `(new_insns, exc_table)` where `exc_table[pc]` is the absolute target
/// PC (in the *new*, post-strip instruction space) of the innermost exception
/// handler active when an exception is raised at `pc`, or [`EXC_NO_HANDLER`] if
/// none.  `SetupExcept`/`PopExcept` are removed entirely, so entering/leaving a
/// `try` costs nothing at runtime — the cost moves to the rare raise/unwind,
/// which does a single O(1) table lookup instead of a per-frame block push/pop.
///
/// On the bail path (handler stack not statically consistent at every PC) the
/// stream is returned unchanged with an **empty** table; the VM then keeps the
/// dynamic `SetupExcept`/`PopExcept` handler stack.  The compiler emits balanced,
/// properly-nested `SetupExcept`/`PopExcept`, so in practice this never bails;
/// the check is a safety net that guarantees we never produce an incorrect table.
#[cfg(test)]
fn build_exc_table(insns: Vec<Insn>) -> (Vec<Insn>, Vec<u32>) {
    let source_prefix_len = insns.len();
    let (insns, exc_table, _) = build_exc_table_with_source_prefix(insns, source_prefix_len);
    (insns, exc_table)
}

/// [`build_exc_table`] with an explicit boundary between source-derived
/// instructions and optimizer-appended copies.
///
/// Stripping `SetupExcept`/`PopExcept` compacts the instruction stream, so the
/// boundary must be remapped through the same old-to-new index map as branch
/// targets. Returning it alongside the compacted stream prevents downstream
/// source mapping from guessing optimizer-specific bytecode shapes.
fn build_exc_table_with_source_prefix(
    insns: Vec<Insn>,
    source_prefix_len: usize,
) -> (Vec<Insn>, Vec<u32>, usize) {
    let n = insns.len();
    debug_assert!(
        source_prefix_len <= n,
        "source prefix {source_prefix_len} exceeds instruction count {n}"
    );
    let source_prefix_len = source_prefix_len.min(n);

    // Fast out: no exception handlers at all → empty table, nothing to strip.
    if !insns.iter().any(|i| matches!(i, Insn::SetupExcept(_))) {
        return (insns, vec![EXC_NO_HANDLER; n], source_prefix_len);
    }

    // Safety net: a statically inconsistent handler stack means the per-pc table
    // would be ambiguous.  Hand the (unstripped) stream back with an empty table
    // and let the VM keep the dynamic SetupExcept/PopExcept handler stack.
    let Some(stack_in) = analyze_active_handler_stacks(&insns) else {
        return (insns, Vec::new(), source_prefix_len);
    };

    // `handler_at[pc]` (original PC space) = innermost active handler at `pc`,
    // or `usize::MAX` for none / unreachable.
    let mut handler_at: Vec<usize> = vec![usize::MAX; n];
    for pc in 0..n {
        if let Some(stack) = &stack_in[pc]
            && let Some(&h) = stack.last()
        {
            handler_at[pc] = h;
        }
    }

    // Strip SetupExcept/PopExcept and retarget all jumps via the shared compact
    // machinery, then remap `handler_at` into the new PC space.
    let keep: Vec<bool> = insns
        .iter()
        .map(|i| !matches!(i, Insn::SetupExcept(_) | Insn::PopExcept))
        .collect();

    // Replicate compact's old→new index map so we can remap handler targets.
    let mut to_new = vec![0usize; n + 1];
    let mut cnt = 0usize;
    for i in 0..n {
        to_new[i] = cnt;
        if keep[i] {
            cnt += 1;
        }
    }
    to_new[n] = cnt;

    let new_insns = compact(insns, &keep);
    let new_len = new_insns.len();
    debug_assert_eq!(new_len, cnt);

    let mut exc_table = vec![EXC_NO_HANDLER; new_len];
    for old_pc in 0..n {
        if !keep[old_pc] {
            continue;
        }
        let new_pc = to_new[old_pc];
        let h = handler_at[old_pc];
        exc_table[new_pc] = if h == usize::MAX {
            EXC_NO_HANDLER
        } else {
            // A handler target is always a SetupExcept jump target, i.e. the
            // first kept instruction at-or-after the removed SetupExcept's
            // destination — exactly compact's redirect rule.
            to_new[h] as u32
        };
    }

    let new_source_prefix_len = to_new[source_prefix_len];
    (new_insns, exc_table, new_source_prefix_len)
}

/// True for either a populated zero-cost table or a conservative dynamic-stack
/// fallback stream.  On the latter path `build_exc_table` intentionally returns
/// an empty table while retaining `SetupExcept`; treating that code as
/// handler-free would incorrectly admit it to call/generator trampolines.
fn has_exception_handlers(insns: &[Insn], exc_table: &[u32]) -> bool {
    exc_table.iter().any(|&target| target != EXC_NO_HANDLER)
        || insns
            .iter()
            .any(|insn| matches!(insn, Insn::SetupExcept(_)))
}
