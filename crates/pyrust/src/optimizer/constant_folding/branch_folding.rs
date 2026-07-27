// ─── Constant-condition branch elimination ─────────────────────────────────────

/// Evaluate a comparison between two compile-time constant `Value`s.  Returns
/// `None` for combinations where the comparison is not defined at the constant
/// level (e.g. mixed types, or an op outside the comparison set).
fn eval_const_cmp(lv: &Value, op: crate::ast::BinaryOp, rv: &Value) -> Option<bool> {
    use crate::ast::BinaryOp::*;
    if let (Some(a), Some(b)) = (lv.as_int(), rv.as_int()) {
        return match op {
            Eq => Some(a == b),
            Ne => Some(a != b),
            Lt => Some(a < b),
            Le => Some(a <= b),
            Gt => Some(a > b),
            Ge => Some(a >= b),
            _ => None,
        };
    }
    if let (ValueKind::Str(ls), ValueKind::Str(rs)) = (lv.kind(), rv.kind()) {
        return match op {
            Eq => Some(ls == rs),
            Ne => Some(ls != rs),
            Lt => Some(ls < rs),
            Le => Some(ls <= rs),
            Gt => Some(ls > rs),
            Ge => Some(ls >= rs),
            _ => None,
        };
    }
    if let (ValueKind::Bool(lb), ValueKind::Bool(rb)) = (lv.kind(), rv.kind()) {
        return match op {
            Eq => Some(lb == rb),
            Ne => Some(lb != rb),
            _ => None,
        };
    }
    None
}

/// Replace conditional jumps whose condition register was just loaded from a
/// known constant with an unconditional `Jump`:
///
/// - `LoadConst(r, c) + JumpIfFalse(r, k)` → keep LoadConst; replace with `Jump(k)` if falsy, `Jump(0)` if truthy
/// - `LoadConst(r, c) + JumpIfTrue(r, k)` → keep LoadConst; replace with `Jump(k)` if truthy, `Jump(0)` if falsy
/// - `LoadConst(r, c) + CmpJumpIfFalseConst(r, op, c2, k)` → `Jump(...)` when the comparison
///   can be evaluated at compile time (e.g. after `pass_str_method_const_fold` produces
///   a known-constant lhs for an assert like `assert "Hi".casefold() == "hi"`).
///
/// The unconditional jumps are then cleaned up by `pass_dead_code` (removes
/// unreachable instructions) and `pass_trivial_nop` (removes `Jump(0)`).
fn pass_const_branch_elim(insns: Vec<Insn>, consts: &[Value]) -> Vec<Insn> {
    let n = insns.len();
    let mut out = insns;
    // A conditional can be reached without executing the immediately
    // preceding LoadConst (for example, both arms of a ternary jump to one
    // shared truth test).  In that shape the predecessor is not a dominating
    // definition, so folding from it would force the else-arm's value onto the
    // incoming then-arm path.  Keep every externally targeted conditional
    // intact, matching the same guard used by unary/cmp-jump fusion.
    let mut jump_targets: HashSet<usize> = HashSet::new();
    for (idx, insn) in out.iter().enumerate() {
        if let Some(offset) = insn_jump_off(insn) {
            let target = idx as i64 + 1 + offset as i64;
            if target >= 0 && (target as usize) < n {
                jump_targets.insert(target as usize);
            }
        }
    }

    let mut i = 0;
    while i + 1 < n {
        if jump_targets.contains(&(i + 1)) {
            i += 1;
            continue;
        }
        if let (Insn::LoadConst(lc_reg, c_idx), jump) = (&out[i], &out[i + 1]) {
            let (lc_reg, c_idx) = (*lc_reg, *c_idx);
            let lv = &consts[c_idx as usize];
            let truthy = lv.truthy_raw();
            let replacement: Option<Insn> = match jump {
                Insn::JumpIfFalse(cond, k) if *cond == lc_reg => Some(if truthy {
                    Insn::Jump(0)
                } else {
                    Insn::Jump(*k)
                }),
                Insn::JumpIfTrue(cond, k) if *cond == lc_reg => Some(if truthy {
                    Insn::Jump(*k)
                } else {
                    Insn::Jump(0)
                }),
                Insn::CmpJumpIfFalseConst(lhs, op, rhs_idx, k) if *lhs == lc_reg => {
                    let rv = &consts[*rhs_idx as usize];
                    eval_const_cmp(lv, *op, rv)
                        .map(|cond| if !cond { Insn::Jump(*k) } else { Insn::Jump(0) })
                }
                Insn::CmpJumpIfTrueConst(lhs, op, rhs_idx, k) if *lhs == lc_reg => {
                    let rv = &consts[*rhs_idx as usize];
                    eval_const_cmp(lv, *op, rv)
                        .map(|cond| if cond { Insn::Jump(*k) } else { Insn::Jump(0) })
                }
                _ => None,
            };
            if let Some(new_jump) = replacement {
                out[i + 1] = new_jump;
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    out
}
