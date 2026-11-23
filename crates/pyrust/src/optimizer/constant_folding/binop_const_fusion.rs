// ─── BinOp-const fusion ────────────────────────────────────────────────────────

/// Fuse `LoadConst(r, c) + BinOp(dst, lhs, op, r)` → `BinOpConst(dst, lhs, op, c)`
/// when `r` is a temp register (`r >= num_locals`), and `r` is not read by any
/// instruction after the BinOp (forward liveness check).
///
/// Only handles Case 1 where the constant is the RHS operand. When the constant
/// is the LHS operand the optimization is skipped because swapping operands would
/// call `lhs.__add__(const)` instead of `const.__add__(lhs)` / `lhs.__radd__(const)`,
/// breaking Python's reflected operator protocol.
///
/// The liveness guard is necessary for patterns like chained comparisons where the
/// same intermediate value is used as both an operand of the first comparison and
/// the left-hand side of the next comparison.
fn pass_binop_const_fusion(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    let n = insns.len();
    let mut transformed = insns;
    let mut keep_buf = InlineBoolBuf::new(n, true);
    let keep = &mut keep_buf.as_mut_slice()[..n];

    // Precompute O(1) replacements for the two per-pair tail scans that would
    // otherwise make this pass O(n²) on a long straight-line sequence (issue
    // #2002), using stack-backed flat arrays instead of `HashMap`/`HashSet` (#2905):
    //  * `back_edge_after[j]` — whether any instruction at index `>= j` is a
    //    backward branch (equivalent to `slice_has_back_edge(&insns[j..])`).
    //  * `last_read_plus1[r - num_locals]` — `j + 1` for the highest index `j`
    //    at which temp register `r` is read (0 if never read).
    //  * `is_jump_target[idx]` — whether `idx` is the target of any jump.
    let mut back_edge_after_buf = InlineBoolBuf::new(n + 1, false);
    let back_edge_after = &mut back_edge_after_buf.as_mut_slice()[..=n];
    for j in (0..n).rev() {
        back_edge_after[j] = back_edge_after[j + 1] || insn_is_back_edge(&transformed[j]);
    }
    let mut last_read_inline = [0usize; 64];
    let mut last_read_overflow: Vec<usize> = Vec::new();
    let mut is_jump_target_buf = InlineBoolBuf::new(n, false);
    let is_jump_target = &mut is_jump_target_buf.as_mut_slice()[..n];
    for (idx, insn) in transformed.iter().enumerate() {
        for_each_read(insn, |r| {
            if r >= num_locals {
                let slot = (r - num_locals) as usize;
                if slot < 64 {
                    last_read_inline[slot] = idx + 1;
                } else {
                    let ov = slot - 64;
                    if ov >= last_read_overflow.len() {
                        last_read_overflow.resize(ov + 1, 0);
                    }
                    last_read_overflow[ov] = idx + 1;
                }
            }
        });
        if let Some(k) = insn_jump_off(insn) {
            let target = idx as i64 + 1 + k as i64;
            if target >= 0 && (target as usize) < n {
                is_jump_target[target as usize] = true;
            }
        }
    }

    // Candidates the coarse guards below rejected.  They are re-decided in a
    // second phase by `load_const_dead_after_use`, whose two extra linear scans
    // are only paid for when such a candidate exists (issue #2002's O(n²)
    // lesson applies to constant factors too: a module that is one huge
    // literal has thousands of `LoadConst`s and no rejected pair at all).
    let mut rejected: Vec<usize> = Vec::new();

    let mut i = 0;
    while i + 1 < n {
        if is_jump_target[i + 1] {
            i += 1;
            continue;
        }
        if let (Insn::LoadConst(lc_reg, c_idx), Insn::BinOp(dst, lhs, op, rhs)) =
            (&transformed[i], &transformed[i + 1])
        {
            let (lc_reg, c_idx) = (*lc_reg, *c_idx);
            let (dst, lhs, op, rhs) = (*dst, *lhs, *op, *rhs);
            // Case 1: const is the RHS operand → BinOpConst(dst, lhs, op, c)
            if rhs == lc_reg && lhs != lc_reg && lc_reg >= num_locals {
                let slot = (lc_reg - num_locals) as usize;
                let last_plus1 = if slot < 64 {
                    last_read_inline[slot]
                } else {
                    last_read_overflow.get(slot - 64).copied().unwrap_or(0)
                };
                let read_after = last_plus1 >= i + 3;
                // The coarse `last_read[lc_reg]` / `back_edge_after` guards block the
                // very common "reused scratch register" shape that dominates loop
                // bodies: a temp re-`LoadConst`-ed for each operand, e.g.
                //   LoadConst(t,2); BinOp(_, i, Mul, t)   ← value of t dead here
                //   LoadConst(t,1); BinOp(_, _, Sub, t)   ← (t overwritten before read)
                // `last_read[t]` points at the *second* BinOp and the loop ends in a
                // back-edge, so neither pair ever fuses.  `scratch_dead_after` proves
                // the precise fact — the value loaded at `i` is overwritten before any
                // read OR branch in a short straight-line window — which makes fusion
                // sound regardless of the forward `last_read` estimate or a later
                // back-edge (no reachable path can observe the dead value).
                let dead_after = scratch_dead_after(&transformed, i + 2, lc_reg);
                if (dead_after || !back_edge_after[i + 2])
                    && (dst == lc_reg || !read_after || dead_after)
                {
                    keep[i] = false;
                    // Fusing a plain BinOp (never augmented) → is_aug = false.
                    transformed[i + 1] = Insn::BinOpConst(dst, lhs, op, c_idx, false);
                    i += 2;
                    continue;
                }
                rejected.push(i);
            }
        }
        i += 1;
    }

    // Second phase: the coarse guards are whole-function (`last_read`,
    // `back_edge_after`) or fixed-window (`scratch_dead_after`) approximations,
    // and they always reject the scratch temp a loop body reloads per operand —
    // including the one holding the loop bound at the header.  Deciding those
    // rejections precisely is what lets the bound's `LoadConst` disappear so the
    // back edge lands on the comparison header again, which is what
    // `pass_loop_inversion` and `pass_int_loop_version` need to see (#2889).
    //
    // The phase-1 rewrites already applied are invisible here: fusing only ever
    // drops a `BinOp`'s read of a register defined by the `LoadConst`
    // immediately before it, so no read that carries a value across an
    // instruction boundary — the only kind either scan reasons about — is lost.
    if !rejected.is_empty() {
        let dead_def_buf = load_const_dead_after_use(&transformed, num_locals, is_jump_target);
        let dead_def = dead_def_buf.as_slice();
        for i in rejected {
            if !dead_def[i] {
                continue;
            }
            if let (Insn::LoadConst(_, c_idx), Insn::BinOp(dst, lhs, op, _)) =
                (&transformed[i], &transformed[i + 1])
            {
                let insn = Insn::BinOpConst(*dst, *lhs, *op, *c_idx, false);
                keep[i] = false;
                transformed[i + 1] = insn;
            }
        }
    }

    compact(transformed, keep)
}
