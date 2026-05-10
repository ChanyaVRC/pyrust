use std::rc::Rc;

use crate::bytecode::{FnCode, FnProto, Insn};

/// Optimize a compiled `FnCode` and all nested function prototypes.
/// Applies a sequence of peephole passes over each instruction list.
pub fn optimize(code: FnCode) -> FnCode {
    optimize_fn_code(code)
}

fn optimize_fn_code(code: FnCode) -> FnCode {
    // Recursively optimize nested function / class bodies first.
    let fn_protos: Vec<FnProto> = code
        .fn_protos
        .into_iter()
        .map(|mut proto| {
            let inner = Rc::try_unwrap(proto.code)
                .unwrap_or_else(|rc| (*rc).clone());
            proto.code = Rc::new(optimize_fn_code(inner));
            proto
        })
        .collect();

    let insns = pass_dead_code(code.insns);
    let insns = pass_trivial_nop(insns);

    FnCode {
        insns,
        consts: code.consts,
        names: code.names,
        num_regs: code.num_regs,
        num_iters: code.num_iters,
        num_locals: code.num_locals,
        fn_protos,
        cell_vars: code.cell_vars,
    }
}

// ─── Dead code elimination ─────────────────────────────────────────────────────

/// Remove instructions that are unreachable from `pc = 0`.
/// Uses a BFS reachability pass that follows all possible instruction successors
/// (both fallthrough and jump targets, including exception handler targets).
fn pass_dead_code(insns: Vec<Insn>) -> Vec<Insn> {
    let n = insns.len();
    let mut reachable = vec![false; n];
    let mut queue = vec![0usize];

    while let Some(pc) = queue.pop() {
        if pc >= n || reachable[pc] {
            continue;
        }
        reachable[pc] = true;

        let jt = |k: i32| (pc as i64 + 1 + k as i64) as usize;

        match &insns[pc] {
            Insn::Jump(k) => queue.push(jt(*k)),

            Insn::Return(_)
            | Insn::ReturnNone
            | Insn::RaiseValue(_)
            | Insn::RaiseReRaise
            | Insn::RaiseFrom(_, _)
            | Insn::RaiseAssert(_) => {}

            Insn::JumpIfFalse(_, k) | Insn::JumpIfTrue(_, k) => {
                queue.push(pc + 1);
                queue.push(jt(*k));
            }
            Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::MatchExcept(_, k) => {
                queue.push(pc + 1);
                queue.push(jt(*k));
            }
            Insn::SetupExcept(k) => {
                queue.push(pc + 1);
                queue.push(jt(*k));
            }
            _ => queue.push(pc + 1),
        }
    }

    compact(insns, &reachable)
}

// ─── Trivial no-op removal ─────────────────────────────────────────────────────

/// Remove instructions that have no observable effect:
/// - `Jump(0)` — offset 0 means the next instruction; equivalent to falling through
/// - `Move(r, r)` — a register copied into itself
fn pass_trivial_nop(insns: Vec<Insn>) -> Vec<Insn> {
    let keep: Vec<bool> = insns
        .iter()
        .map(|insn| match insn {
            Insn::Jump(0) => false,
            Insn::Move(dst, src) => dst != src,
            _ => true,
        })
        .collect();
    compact(insns, &keep)
}

// ─── Compaction helper ─────────────────────────────────────────────────────────

/// Remove instructions where `keep[i]` is `false` and rewrite all jump offsets.
///
/// Removed instructions are treated as transparent: any jump whose target is a
/// removed instruction is redirected to the first kept instruction that follows it.
/// This is correct for no-op removals where "jumping to the no-op" is equivalent
/// to "jumping to whatever comes after it".
pub(crate) fn compact(insns: Vec<Insn>, keep: &[bool]) -> Vec<Insn> {
    let n = insns.len();
    debug_assert_eq!(n, keep.len());

    // to_new[i] = new index of the first kept instruction at or after old index i.
    // to_new[n] = total kept count (past-the-end sentinel for jumps to code.insns.len()).
    let mut to_new = vec![0usize; n + 1];
    let mut cnt = 0usize;
    for i in 0..n {
        to_new[i] = cnt;
        if keep[i] {
            cnt += 1;
        }
    }
    to_new[n] = cnt;

    insns
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(old_i, insn)| rewrite_offsets(insn, old_i, &to_new))
        .collect()
}

/// Rewrite all jump offsets in `insn` using the old→new index table.
/// `old_i` is the pre-compaction index of `insn` (which is guaranteed to be kept).
pub(crate) fn rewrite_offsets(insn: Insn, old_i: usize, to_new: &[usize]) -> Insn {
    let fix = |k: i32| -> i32 {
        let old_target = (old_i as i64 + 1 + k as i64) as usize;
        let new_src = to_new[old_i];
        let new_target = to_new[old_target];
        (new_target as i64 - new_src as i64 - 1) as i32
    };
    use Insn::*;
    match insn {
        Jump(k) => Jump(fix(k)),
        JumpIfFalse(r, k) => JumpIfFalse(r, fix(k)),
        JumpIfTrue(r, k) => JumpIfTrue(r, fix(k)),
        CmpJumpIfFalse(a, op, b, k) => CmpJumpIfFalse(a, op, b, fix(k)),
        CmpJumpIfTrue(a, op, b, k) => CmpJumpIfTrue(a, op, b, fix(k)),
        CmpJumpIfFalseConst(r, op, c, k) => CmpJumpIfFalseConst(r, op, c, fix(k)),
        CmpJumpIfTrueConst(r, op, c, k) => CmpJumpIfTrueConst(r, op, c, fix(k)),
        ForIter(dst, slot, k) => ForIter(dst, slot, fix(k)),
        ForCountReg(v, op, stop, step, k) => ForCountReg(v, op, stop, step, fix(k)),
        ForCountConst(v, op, stop, step, k) => ForCountConst(v, op, stop, step, fix(k)),
        SetupExcept(k) => SetupExcept(fix(k)),
        MatchExcept(r, k) => MatchExcept(r, fix(k)),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::BinaryOp;

    fn compile_fn(src: &str) -> FnCode {
        use crate::{interpreter::collect_local_names, lexer::Lexer, parser::Parser};
        use std::collections::HashSet;
        let tokens = Lexer::new(src).unwrap().into_tokens();
        let mut parser = Parser::new(tokens);
        let stmts = parser.parse_program().unwrap();
        let empty: HashSet<String> = HashSet::new();
        let names = collect_local_names(&[], &stmts, &empty, &empty);
        let local_index = std::rc::Rc::new(
            (0u32..).zip(names.iter()).map(|(i, n)| (n.clone(), i)).collect(),
        );
        crate::compiler::compile_script(&stmts, local_index, false).unwrap()
    }

    // ── pass_dead_code ────────────────────────────────────────────────────────

    #[test]
    fn dce_removes_instructions_after_return() {
        // Instructions after Return are unreachable.
        let insns = vec![
            Insn::LoadNone(0),
            Insn::Return(0),
            Insn::LoadNone(1), // unreachable
            Insn::Return(1),   // unreachable
        ];
        let out = pass_dead_code(insns);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Insn::LoadNone(0)));
        assert!(matches!(out[1], Insn::Return(0)));
    }

    #[test]
    fn dce_keeps_instructions_after_conditional_jump() {
        // Both branches of a conditional jump are reachable.
        // [0] JumpIfFalse(r0, 1)  [1] LoadNone(1)  [2] Return(0)
        // Both fallthrough (idx 1) and target (idx 1+1+1=3 — end) are successors,
        // so nothing is removed.
        let insns = vec![
            Insn::JumpIfFalse(0, 1), // target = 0+1+1 = 2 (Return)
            Insn::LoadNone(1),
            Insn::Return(0),
        ];
        let out = pass_dead_code(insns);
        assert_eq!(out.len(), 3, "all instructions reachable");
    }

    #[test]
    fn dce_removes_dead_code_after_unconditional_jump() {
        // [0] Jump(1)   [1] LoadNone(0) <dead>   [2] Return(1)
        let insns = vec![
            Insn::Jump(1),     // jumps to idx 2
            Insn::LoadNone(0), // unreachable
            Insn::Return(1),
        ];
        let out = pass_dead_code(insns);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Insn::Jump(0)), "offset rewritten: 2→1, new offset = 1-(0+1)=0");
        assert!(matches!(out[1], Insn::Return(1)));
    }

    #[test]
    fn dce_on_compiled_function_with_early_return() {
        let code = compile_fn("def f(x):\n    if x > 0:\n        return 1\n    return 0\n");
        let before = code.fn_protos[0].code.insns.len();
        let optimized = optimize(code);
        let after = optimized.fn_protos[0].code.insns.len();
        assert!(
            after <= before,
            "optimizer should not increase instruction count ({before} → {after})"
        );
    }

    // ── pass_trivial_nop ──────────────────────────────────────────────────────

    #[test]
    fn trivial_nop_removes_jump0() {
        // Jump(0) is a no-op: it jumps to the next instruction.
        let insns = vec![
            Insn::LoadNone(0),
            Insn::Jump(0),  // <- should be removed
            Insn::Return(0),
        ];
        let out = pass_trivial_nop(insns);
        assert_eq!(out.len(), 2, "Jump(0) should be removed");
        assert!(matches!(out[0], Insn::LoadNone(0)));
        assert!(matches!(out[1], Insn::Return(0)));
    }

    #[test]
    fn trivial_nop_removes_self_move() {
        // Move(r, r) copies a register into itself — no effect.
        let insns = vec![
            Insn::LoadNone(1),
            Insn::Move(2, 2),  // <- should be removed
            Insn::Return(1),
        ];
        let out = pass_trivial_nop(insns);
        assert_eq!(out.len(), 2, "Move(r, r) should be removed");
    }

    #[test]
    fn trivial_nop_fixes_jump_over_removed() {
        // A Jump that skips over a removed Jump(0) must have its offset decremented.
        // insns: [0] LoadNone 0   [1] Jump(1)   [2] Jump(0) <removed>   [3] Return 0
        // Jump(1) at idx 1 targets idx 3 (offset 1 = idx 1 + 1 + 1).
        // After removing idx 2: Jump(1) at new-idx 1 should target new-idx 2 (old idx 3),
        // so new offset = 2 - (1+1) = 0.
        let insns = vec![
            Insn::LoadNone(0),
            Insn::Jump(1),  // targets idx 3 (Return)
            Insn::Jump(0),  // no-op, removed
            Insn::Return(0),
        ];
        let out = pass_trivial_nop(insns);
        assert_eq!(out.len(), 3);
        assert!(matches!(out[1], Insn::Jump(0)), "offset should decrease by 1");
    }
}
