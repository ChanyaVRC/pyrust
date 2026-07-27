// ─── Loop inversion ────────────────────────────────────────────────────────────

/// Eliminate the unconditional back-edge `Jump` from `while COND: body` loops.
///
/// ## Pattern (VM offset semantics: `jump_pc!(k) = current_pc + 1 + k`)
///
/// A while-loop compiled to:
/// ```text
/// [j]   CmpJumpIfFalse*(r, op, b_or_c, k)   ; exit to [j+k+1] if cond false
/// [j+1] body_insn_1
/// …
/// [j+k] Jump(-(k+1))                         ; back-edge to [j]
/// ```
///
/// is transformed to:
/// ```text
/// [j]   CmpJumpIfFalse*(r, op, b_or_c, k)   ; initial guard — unchanged
/// [j+1] body_insn_1
/// …
/// [j+k] CmpJumpIfTrue*(r, op, b_or_c, -k)   ; re-check; if true, jump to [j+1]
/// ```
///
/// The back-edge `Jump` is replaced by a conditional jump that re-checks the
/// loop condition and either loops back to `[j+1]` (body start) or falls through
/// to `[j+k+1]` (exit).  This saves one full VM dispatch per iteration on the
/// hot path.
///
/// ## Correctness notes
///
/// - **`continue`**: generates `Jump(-(m+1))` targeting `[j]` (the initial guard).
///   After inversion the initial guard at `[j]` still exists, so `continue` works
///   correctly — the guard re-checks the condition and falls through to `[j+1]`.
/// - **`break`**: generates a forward jump to `[j+k+1]`.  Unchanged.
/// - **0-iteration loops**: the initial guard at `[j]` exits to `[j+k+1]` immediately.
/// - **Nested loops**: each is handled independently, innermost first.
///
/// ## Guards
///
/// - `k >= 2`: at least one real body instruction before the back-edge Jump.
///   (`k == 1` would mean the body is just the Jump itself, which pass_dead_code
///   would already have removed or which is a degenerate loop with no real body.)
/// - `j + k < n`: the back-edge index must be in bounds.
fn pass_loop_inversion(insns: Vec<Insn>) -> Vec<Insn> {
    let mut out = insns;
    let n = out.len();
    for j in 0..n {
        // Match the loop header: CmpJumpIfFalseConst, CmpJumpIfFalse,
        // CmpJumpIfTrueConst, or CmpJumpIfTrue.
        // Extract (register, op, rhs-operand, forward-offset k).
        match out[j].clone() {
            Insn::CmpJumpIfFalseConst(r, op, c, k) => {
                if k < 2 {
                    continue;
                }
                let back = j + k as usize;
                if back >= n {
                    continue;
                }
                // Verify the back-edge: Jump(-(k+1)) at [j+k] targets [j].
                // Proof: (j+k) + 1 + (-(k+1)) = j. ✓
                match out[back] {
                    Insn::Jump(bk) if bk == -(k + 1) => {}
                    _ => continue,
                }
                // Replace unconditional back-edge with CmpJumpIfTrueConst targeting [j+1].
                // new_offset = -k  because  (j+k) + 1 + (-k) = j+1. ✓
                out[back] = Insn::CmpJumpIfTrueConst(r, op, c, -k);
            }
            Insn::CmpJumpIfFalse(r, op, b, k) => {
                if k < 2 {
                    continue;
                }
                let back = j + k as usize;
                if back >= n {
                    continue;
                }
                match out[back] {
                    Insn::Jump(bk) if bk == -(k + 1) => {}
                    _ => continue,
                }
                out[back] = Insn::CmpJumpIfTrue(r, op, b, -k);
            }
            // Case: `while True: if cond: break; body` shape.
            //
            // The header CmpJumpIfTrueConst(r, op, c, k) exits to [j+k+1] when
            // the break condition is TRUE.  The back-edge Jump(-(k+1)) at [j+k]
            // unconditionally returns to [j].  We replace the Jump with
            // CmpJumpIfFalseConst(r, op, c, -k): when the condition is FALSE
            // (not yet time to break), jump to (j+k+1)+(-k) = j+1 (body start);
            // when the condition is TRUE (time to break), fall through to [j+k+1]
            // (the exit).  No operator negation needed — just swap True↔False
            // variant with the same (r, op, c).
            Insn::CmpJumpIfTrueConst(r, op, c, k) => {
                if k < 2 {
                    continue;
                }
                let back = j + k as usize;
                if back >= n {
                    continue;
                }
                // Verify the back-edge: Jump(-(k+1)) at [j+k] targets [j].
                match out[back] {
                    Insn::Jump(bk) if bk == -(k + 1) => {}
                    _ => continue,
                }
                // Replace with CmpJumpIfFalseConst targeting [j+1].
                // new_offset = -k  because  (j+k) + 1 + (-k) = j+1. ✓
                out[back] = Insn::CmpJumpIfFalseConst(r, op, c, -k);
            }
            Insn::CmpJumpIfTrue(r, op, b, k) => {
                if k < 2 {
                    continue;
                }
                let back = j + k as usize;
                if back >= n {
                    continue;
                }
                match out[back] {
                    Insn::Jump(bk) if bk == -(k + 1) => {}
                    _ => continue,
                }
                out[back] = Insn::CmpJumpIfFalse(r, op, b, -k);
            }
            _ => {}
        }
    }
    out
}

fn pass_trivial_nop(insns: Vec<Insn>) -> Vec<Insn> {
    let keep: Vec<bool> = insns
        .iter()
        .map(|insn| match insn {
            Insn::Jump(0) => false,
            Insn::Move(dst, src) | Insn::CopyReg(dst, src) => dst != src,
            _ => true,
        })
        .collect();
    compact(insns, &keep)
}
