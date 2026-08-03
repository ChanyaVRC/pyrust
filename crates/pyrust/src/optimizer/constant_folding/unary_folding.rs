// ─── Unary constant folding ────────────────────────────────────────────────────

/// Fuse `LoadConst(r, c) + UnaryOp(dst, op, r)` → `LoadConst(dst, op(c))`
/// when `r >= num_locals` (temp register).
///
/// Handles `Neg`, `Not`, and `BitNot` applied to integer or float constants.
fn pass_unary_fold(insns: Vec<Insn>, num_locals: u32, consts: &mut Vec<Value>) -> Vec<Insn> {
    let n = insns.len();
    let mut transformed = insns;
    let mut keep = vec![true; n];

    // Indices that are the target of some (forward or backward) jump.  The
    // `UnaryOp` of a fusion candidate must not be such a target: fusing folds the
    // preceding `LoadConst`'s value into the op and drops the load, but a
    // control-flow edge landing directly on the `UnaryOp` reaches it without ever
    // executing that load (issue #2565: a ternary's then- and else-branch each
    // emit their own `LoadConst` feeding a shared trailing `UnaryOp`; fusing the
    // else-branch load made the then-branch — which jumps onto the `UnaryOp` —
    // use the else-branch constant).  Same guard as `pass_binop_const_fusion` /
    // `pass_cmpjump_fusion`.
    let mut jump_targets: HashSet<usize> = HashSet::new();
    let mut needs_dead_source = false;
    for (idx, insn) in transformed.iter().enumerate() {
        if let Some(k) = insn_jump_off(insn) {
            let target = idx as i64 + 1 + k as i64;
            if target >= 0 && (target as usize) < n {
                jump_targets.insert(target as usize);
            }
        }
        needs_dead_source |= matches!(
            (insn, transformed.get(idx + 1)),
            (
                Insn::LoadConst(src, _),
                Some(Insn::UnaryOp(dst, _, unary_src))
            ) if src == unary_src && dst != src && *src >= num_locals
        );
    }

    // Most compiler-emitted literal negations write the result back to the
    // source temp, which preserves that register's value after fusion.  Only
    // compute the more expensive cross-block liveness proof when a candidate
    // writes a distinct destination.  The shared helper uses two linear scans,
    // avoiding the per-candidate tail scans that made a later, unrelated back
    // edge reject every literal negation in the function (issue #2907).
    let dead_after_use = needs_dead_source
        .then(|| load_const_dead_after_use(&transformed, num_locals, &jump_targets));

    let mut i = 0;
    while i + 1 < n {
        if jump_targets.contains(&(i + 1)) {
            i += 1;
            continue;
        }
        let fused: Option<(u32, Value)> = match (&transformed[i], &transformed[i + 1]) {
            (Insn::LoadConst(lc_reg, c_idx), Insn::UnaryOp(dst, op, src))
                if *src == *lc_reg
                    && *lc_reg >= num_locals
                    // When dst==lc_reg the fusion overwrites lc_reg with the result,
                    // so any later read of lc_reg will see the correct folded value.
                    // When dst!=lc_reg, lc_reg would become uninitialized after removal.
                    && (*dst == *lc_reg
                        || dead_after_use
                            .as_ref()
                            .is_some_and(|dead| dead[i])) =>
            {
                let c = &consts[*c_idx as usize];
                // Fold through canonical built-in unary semantics rather than
                // re-implementing the per-kind arms here, so the compile-time
                // constant can never drift from the runtime result (issue
                // #458).  A runtime error (e.g. `~1.5` → TypeError) returns
                // `None`, leaving the UnaryOp in the bytecode to raise at
                // runtime — never at compile time.
                let result = crate::interpreter::eval_builtin_unary(*op, c.clone()).ok();
                result.map(|v| (*dst, v))
            }
            _ => None,
        };

        if let Some((dst, val)) = fused
            && let Some(new_c) = intern_const_in_pool(consts, val)
        {
            keep[i] = false;
            transformed[i + 1] = Insn::LoadConst(dst, new_c);
            i += 2;
            continue;
        }
        i += 1;
    }
    compact(transformed, &keep)
}
