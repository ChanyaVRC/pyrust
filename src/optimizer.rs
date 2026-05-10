use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::bytecode::{FnCode, FnProto, Insn};
use crate::value::{Value, ValueKind};

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
            let inner = Rc::try_unwrap(proto.code).unwrap_or_else(|rc| (*rc).clone());
            proto.code = Rc::new(optimize_fn_code(inner));
            proto
        })
        .collect();

    let num_locals = code.num_locals;
    let mut consts = code.consts;
    let insns = pass_thread_jumps(code.insns);
    let insns = pass_binop_const_fusion(insns, num_locals);
    let insns = pass_cmpjump_fusion(insns, num_locals);
    let insns = pass_const_fold(insns, &mut consts);
    let insns = pass_algebraic_simplify(insns, &mut consts);
    let insns = pass_unary_fold(insns, num_locals, &mut consts);
    let insns = pass_dead_code(insns);
    let insns = pass_trivial_nop(insns);

    FnCode {
        insns,
        consts,
        names: code.names,
        num_regs: code.num_regs,
        num_iters: code.num_iters,
        num_locals,
        fn_protos,
        cell_vars: code.cell_vars,
    }
}

// ─── Jump threading ────────────────────────────────────────────────────────────

/// Thread chains of unconditional `Jump`s so that any jump whose target is
/// itself a `Jump` is redirected to the chain's final non-`Jump` destination.
/// Conditional jumps have only their taken-branch target threaded; the
/// fallthrough path is unchanged.  No instructions are removed in this pass.
fn pass_thread_jumps(insns: Vec<Insn>) -> Vec<Insn> {
    // Follow a chain of unconditional Jumps from `start`, returning the index
    // of the first instruction that is NOT an unconditional Jump.
    // A visited-set guards against infinite loops (self-referential jumps).
    fn follow(insns: &[Insn], start: usize) -> usize {
        let mut pc = start;
        let mut seen = HashSet::new();
        loop {
            if pc >= insns.len() || !seen.insert(pc) {
                break;
            }
            match &insns[pc] {
                Insn::Jump(k) => pc = (pc as i64 + 1 + *k as i64) as usize,
                _ => break,
            }
        }
        pc
    }

    insns
        .iter()
        .enumerate()
        .map(|(i, insn)| {
            let thread = |k: i32| -> i32 {
                let raw = (i as i64 + 1 + k as i64) as usize;
                let final_t = follow(&insns, raw);
                (final_t as i64 - i as i64 - 1) as i32
            };
            use Insn::*;
            match insn.clone() {
                Jump(k) => Jump(thread(k)),
                JumpIfFalse(r, k) => JumpIfFalse(r, thread(k)),
                JumpIfTrue(r, k) => JumpIfTrue(r, thread(k)),
                CmpJumpIfFalse(a, op, b, k) => CmpJumpIfFalse(a, op, b, thread(k)),
                CmpJumpIfTrue(a, op, b, k) => CmpJumpIfTrue(a, op, b, thread(k)),
                CmpJumpIfFalseConst(r, op, c, k) => CmpJumpIfFalseConst(r, op, c, thread(k)),
                CmpJumpIfTrueConst(r, op, c, k) => CmpJumpIfTrueConst(r, op, c, thread(k)),
                ForIter(dst, slot, k) => ForIter(dst, slot, thread(k)),
                ForCountReg(v, op, stop, step, k) => ForCountReg(v, op, stop, step, thread(k)),
                ForCountConst(v, op, stop, step, k) => ForCountConst(v, op, stop, step, thread(k)),
                SetupExcept(k) => SetupExcept(thread(k)),
                MatchExcept(r, k) => MatchExcept(r, thread(k)),
                other => other,
            }
        })
        .collect()
}

// ─── BinOp-const fusion ────────────────────────────────────────────────────────

/// Fuse `LoadConst(r, c) + BinOp(dst, lhs, op, r)` → `BinOpConst(dst, lhs, op, c)`
/// when `r` is a temp register (`r >= num_locals`) and `r != dst`.
///
/// Temps are write-once in the compiler's SSA-like register allocation, so removing
/// the `LoadConst` is safe: no other instruction can read `r` after `BinOp` consumes it.
fn pass_binop_const_fusion(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    let n = insns.len();
    let mut transformed = insns;
    let mut keep = vec![true; n];

    let mut i = 0;
    while i + 1 < n {
        if let (Insn::LoadConst(lc_reg, c_idx), Insn::BinOp(dst, lhs, op, rhs)) =
            (&transformed[i], &transformed[i + 1])
        {
            let (lc_reg, c_idx) = (*lc_reg, *c_idx);
            let (dst, lhs, op, rhs) = (*dst, *lhs, *op, *rhs);
            if rhs == lc_reg && lc_reg >= num_locals && lc_reg != dst {
                keep[i] = false;
                transformed[i + 1] = Insn::BinOpConst(dst, lhs, op, c_idx);
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    compact(transformed, &keep)
}

// ─── CmpJump fusion ────────────────────────────────────────────────────────────

/// Fuse a comparison result into the following conditional jump:
/// - `BinOp(r, lhs, op, rhs) + JumpIfFalse(r, k)` → `CmpJumpIfFalse(lhs, op, rhs, k)`
/// - `BinOp(r, lhs, op, rhs) + JumpIfTrue(r, k)`  → `CmpJumpIfTrue(lhs, op, rhs, k)`
/// - `BinOpConst(r, lhs, op, c) + JumpIfFalse(r, k)` → `CmpJumpIfFalseConst(lhs, op, c, k)`
/// - `BinOpConst(r, lhs, op, c) + JumpIfTrue(r, k)`  → `CmpJumpIfTrueConst(lhs, op, c, k)`
///
/// Only fuses when `r >= num_locals` (temp register — not a named local).
fn pass_cmpjump_fusion(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    let n = insns.len();
    let mut transformed = insns;
    let mut keep = vec![true; n];

    let mut i = 0;
    while i + 1 < n {
        let fused: Option<Insn> = match (&transformed[i], &transformed[i + 1]) {
            (Insn::BinOpConst(r, lhs, op, c), Insn::JumpIfFalse(cond, k))
                if *r == *cond && *r >= num_locals =>
            {
                Some(Insn::CmpJumpIfFalseConst(*lhs, *op, *c, *k))
            }
            (Insn::BinOpConst(r, lhs, op, c), Insn::JumpIfTrue(cond, k))
                if *r == *cond && *r >= num_locals =>
            {
                Some(Insn::CmpJumpIfTrueConst(*lhs, *op, *c, *k))
            }
            (Insn::BinOp(r, lhs, op, rhs), Insn::JumpIfFalse(cond, k))
                if *r == *cond && *r >= num_locals =>
            {
                Some(Insn::CmpJumpIfFalse(*lhs, *op, *rhs, *k))
            }
            (Insn::BinOp(r, lhs, op, rhs), Insn::JumpIfTrue(cond, k))
                if *r == *cond && *r >= num_locals =>
            {
                Some(Insn::CmpJumpIfTrue(*lhs, *op, *rhs, *k))
            }
            _ => None,
        };
        if let Some(new_insn) = fused {
            keep[i] = false;
            transformed[i + 1] = new_insn;
            i += 2;
        } else {
            i += 1;
        }
    }
    compact(transformed, &keep)
}

// ─── Constant folding ──────────────────────────────────────────────────────────

/// Forward dataflow constant folding.
///
/// Tracks registers whose values are statically known (`known: reg → const_idx`).
/// When both operands of a `BinOp` or `BinOpConst` are known, replace the
/// instruction with a `LoadConst` of the folded result.  Also propagates known
/// values through `Move(dst, src)`.
///
/// The map is cleared at branch/loop instructions where we cannot guarantee
/// which path was taken at runtime.
fn pass_const_fold(insns: Vec<Insn>, consts: &mut Vec<Value>) -> Vec<Insn> {
    let mut known: HashMap<u32, u16> = HashMap::new();
    let mut out = Vec::with_capacity(insns.len());

    for insn in insns {
        match insn {
            Insn::LoadConst(dst, c) => {
                known.insert(dst, c);
                out.push(Insn::LoadConst(dst, c));
            }
            Insn::Move(dst, src) => {
                match known.get(&src).copied() {
                    Some(c) => { known.insert(dst, c); }
                    None => { known.remove(&dst); }
                }
                out.push(Insn::Move(dst, src));
            }
            Insn::BinOp(dst, lhs, op, rhs) => {
                let folded = known.get(&lhs).and_then(|&cl| {
                    known.get(&rhs).and_then(|&cr| {
                        crate::compiler::fold_binop(
                            &consts[cl as usize], op, &consts[cr as usize],
                        )
                        .and_then(|v| intern_const_in_pool(consts, v))
                    })
                });
                if let Some(nc) = folded {
                    known.insert(dst, nc);
                    out.push(Insn::LoadConst(dst, nc));
                } else {
                    known.remove(&dst);
                    out.push(Insn::BinOp(dst, lhs, op, rhs));
                }
            }
            Insn::BinOpConst(dst, lhs, op, c) => {
                let folded = known.get(&lhs).and_then(|&cl| {
                    crate::compiler::fold_binop(
                        &consts[cl as usize], op, &consts[c as usize],
                    )
                    .and_then(|v| intern_const_in_pool(consts, v))
                });
                if let Some(nc) = folded {
                    known.insert(dst, nc);
                    out.push(Insn::LoadConst(dst, nc));
                } else {
                    known.remove(&dst);
                    out.push(Insn::BinOpConst(dst, lhs, op, c));
                }
            }
            // Branch/loop/raise: clear the map — values may differ per path.
            insn @ (Insn::Jump(_)
            | Insn::JumpIfFalse(..)
            | Insn::JumpIfTrue(..)
            | Insn::CmpJumpIfFalse(..)
            | Insn::CmpJumpIfTrue(..)
            | Insn::CmpJumpIfFalseConst(..)
            | Insn::CmpJumpIfTrueConst(..)
            | Insn::ForIter(..)
            | Insn::ForCountReg(..)
            | Insn::ForCountConst(..)
            | Insn::SetupExcept(_)
            | Insn::MatchExcept(..)
            | Insn::Return(_)
            | Insn::ReturnNone
            | Insn::RaiseValue(_)
            | Insn::RaiseFrom(..)
            | Insn::RaiseReRaise
            | Insn::RaiseAssert(_)
            | Insn::Unpack(..)) => {
                known.clear();
                out.push(insn);
            }
            // Any other instruction: invalidate dst if we can identify it.
            insn => {
                if let Some(dst) = writable_dst(&insn) {
                    known.remove(&dst);
                }
                out.push(insn);
            }
        }
    }
    out
}

/// Return the single destination register of `insn`, if any.
/// Used to precisely invalidate the `known` map without clearing it entirely.
fn writable_dst(insn: &Insn) -> Option<u32> {
    use Insn::*;
    match insn {
        LoadGlobal(r, _)
        | LoadNone(r)
        | BinOpInPlace(r, _, _, _)
        | UnaryOp(r, _, _)
        | GetAttr(r, _, _)
        | GetItem(r, _, _)
        | Call(r, _)
        | CallMemo(r, _)
        | BuildList(r, _, _)
        | BuildTuple(r, _, _)
        | BuildDict(r, _, _)
        | MakeFunction(r, _, _, _)
        | ImportModule(r, _)
        | LoadExc(r)
        | MakeClass(r, _, _, _, _) => Some(*r),
        CallMethod { dst, .. } | CallMethodExpanded { dst, .. } => Some(*dst),
        _ => None,
    }
}

/// Look up or insert `val` in the const pool; return its index.
/// Returns `None` if the pool is full (>= u16::MAX entries).
fn intern_const_in_pool(consts: &mut Vec<Value>, val: Value) -> Option<u16> {
    // Type-exact linear scan to avoid Bool/Int key collisions.
    for (i, existing) in consts.iter().enumerate() {
        let same = match (existing.kind(), val.kind()) {
            (ValueKind::Int(a), ValueKind::Int(b)) => a == b,
            (ValueKind::Float(a), ValueKind::Float(b)) => a.to_bits() == b.to_bits(),
            (ValueKind::Bool(a), ValueKind::Bool(b)) => a == b,
            (ValueKind::None, ValueKind::None) => true,
            _ => false,
        };
        if same {
            return Some(i as u16);
        }
    }
    if consts.len() >= u16::MAX as usize {
        return None;
    }
    let idx = consts.len() as u16;
    consts.push(val);
    Some(idx)
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

// ─── Algebraic simplification ──────────────────────────────────────────────────

/// Simplify algebraic identities on integer operands (integers only — float and
/// string arithmetic may have different identity semantics):
///
/// | Pattern                         | Result                     |
/// |---------------------------------|----------------------------|
/// | `BinOpConst(dst, lhs, Add, 0)`  | `Move(dst, lhs)`           |
/// | `BinOpConst(dst, lhs, Sub, 0)`  | `Move(dst, lhs)`           |
/// | `BinOpConst(dst, lhs, Mul, 1)`  | `Move(dst, lhs)`           |
/// | `BinOpConst(dst, lhs, Mul, 0)`  | `LoadConst(dst, idx_0)`    |
/// | `BinOpConst(dst, lhs, Pow, 1)`  | `Move(dst, lhs)`           |
/// | `BinOpConst(dst, lhs, Pow, 0)`  | `LoadConst(dst, idx_1)`    |
///
/// Commutative identities (0+x, 1*x, 0*x) are produced by `pass_binop_const_fusion`
/// only when the constant is on the right, so we don't need separate cases for them.
fn pass_algebraic_simplify(insns: Vec<Insn>, consts: &mut Vec<Value>) -> Vec<Insn> {
    use crate::ast::BinaryOp::*;

    insns
        .into_iter()
        .map(|insn| {
            if let Insn::BinOpConst(dst, lhs, op, c_idx) = insn {
                let c_val = match consts[c_idx as usize].kind() {
                    ValueKind::Int(n) => n,
                    _ => return Insn::BinOpConst(dst, lhs, op, c_idx),
                };
                match (op, c_val) {
                    (Add, 0) | (Sub, 0) | (Mul, 1) | (Pow, 1) => Insn::Move(dst, lhs),
                    (Mul, 0) => {
                        let idx = intern_const_in_pool(consts, Value::int(0))
                            .unwrap_or(c_idx);
                        Insn::LoadConst(dst, idx)
                    }
                    (Pow, 0) => {
                        let idx = intern_const_in_pool(consts, Value::int(1))
                            .unwrap_or(c_idx);
                        Insn::LoadConst(dst, idx)
                    }
                    _ => Insn::BinOpConst(dst, lhs, op, c_idx),
                }
            } else {
                insn
            }
        })
        .collect()
}

// ─── Unary constant folding ────────────────────────────────────────────────────

/// Fuse `LoadConst(r, c) + UnaryOp(dst, op, r)` → `LoadConst(dst, op(c))`
/// when `r >= num_locals` (temp register).
///
/// Handles `Neg`, `Not`, and `BitNot` applied to integer or float constants.
fn pass_unary_fold(insns: Vec<Insn>, num_locals: u32, consts: &mut Vec<Value>) -> Vec<Insn> {
    use crate::ast::UnaryOp;

    let n = insns.len();
    let mut transformed = insns;
    let mut keep = vec![true; n];

    let mut i = 0;
    while i + 1 < n {
        let fused: Option<(u32, Value)> =
            match (&transformed[i], &transformed[i + 1]) {
                (Insn::LoadConst(lc_reg, c_idx), Insn::UnaryOp(dst, op, src))
                    if *src == *lc_reg && *lc_reg >= num_locals =>
                {
                    let c = &consts[*c_idx as usize];
                    let result = match op {
                        UnaryOp::Neg => match c.kind() {
                            ValueKind::Int(n) => Some(Value::int(n.wrapping_neg())),
                            ValueKind::Float(f) => Some(Value::float(-f)),
                            _ => None,
                        },
                        UnaryOp::Not => Some(Value::bool_(!c.truthy())),
                        UnaryOp::BitNot => match c.kind() {
                            ValueKind::Int(n) => Some(Value::int(!n)),
                            _ => None,
                        },
                        _ => None,
                    };
                    result.map(|v| (*dst, v))
                }
                _ => None,
            };

        if let Some((dst, val)) = fused {
            if let Some(new_c) = intern_const_in_pool(consts, val) {
                keep[i] = false;
                transformed[i + 1] = Insn::LoadConst(dst, new_c);
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    compact(transformed, &keep)
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

    fn compile_fn(src: &str) -> FnCode {
        use crate::{interpreter::collect_local_names, lexer::Lexer, parser::Parser};
        use std::collections::HashSet;
        let tokens = Lexer::new(src).unwrap().into_tokens();
        let mut parser = Parser::new(tokens);
        let stmts = parser.parse_program().unwrap();
        let empty: HashSet<String> = HashSet::new();
        let names = collect_local_names(&[], &stmts, &empty, &empty);
        let local_index = std::rc::Rc::new(
            (0u32..)
                .zip(names.iter())
                .map(|(i, n)| (n.clone(), i))
                .collect(),
        );
        crate::compiler::compile_script(&stmts, local_index, false).unwrap()
    }

    // ── pass_binop_const_fusion ───────────────────────────────────────────────

    #[test]
    fn binop_const_fusion_fuses_loadconst_binop() {
        use crate::ast::BinaryOp;
        // LoadConst(r=5, c=0)  BinOp(dst=1, lhs=0, Add, r=5)  where num_locals=2
        // r=5 >= num_locals=2, r != dst  → fuse to BinOpConst(1, 0, Add, 0), drop LoadConst
        let insns = vec![
            Insn::LoadConst(5, 0), // temp reg 5, const index 0
            Insn::BinOp(1, 0, BinaryOp::Add, 5),
            Insn::Return(1),
        ];
        let out = pass_binop_const_fusion(insns, 2);
        assert_eq!(out.len(), 2, "LoadConst should be removed");
        assert!(
            matches!(out[0], Insn::BinOpConst(1, 0, BinaryOp::Add, 0)),
            "BinOp should become BinOpConst"
        );
    }

    #[test]
    fn binop_const_fusion_skips_when_reg_is_local() {
        use crate::ast::BinaryOp;
        // r=1 < num_locals=3  → must NOT fuse (register could be a local variable)
        let insns = vec![
            Insn::LoadConst(1, 0),
            Insn::BinOp(2, 0, BinaryOp::Add, 1),
            Insn::Return(2),
        ];
        let out = pass_binop_const_fusion(insns, 3);
        assert_eq!(out.len(), 3, "no fusion when reg is a local");
        assert!(matches!(out[0], Insn::LoadConst(1, 0)));
    }

    #[test]
    fn binop_const_fusion_skips_when_dst_equals_reg() {
        use crate::ast::BinaryOp;
        // dst == lc_reg: result overwrites the constant register → unsafe to remove LoadConst
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::BinOp(5, 0, BinaryOp::Add, 5), // dst == rhs == lc_reg
            Insn::Return(5),
        ];
        let out = pass_binop_const_fusion(insns, 2);
        assert_eq!(out.len(), 3, "no fusion when dst == lc_reg");
    }

    #[test]
    fn binop_const_fusion_on_compiled_code() {
        // Use a function argument so the lhs is not a compile-time constant.
        // pass_binop_const_fusion should still fuse LoadConst(r,5)+BinOp(dst,n,Add,r)
        // → BinOpConst(dst,n,Add,5), which pass_const_fold cannot fold further.
        let code = compile_fn("def f(n):\n    return n + 5\n");
        let optimized = optimize(code);
        let has_binopconst = optimized.fn_protos[0]
            .code
            .insns
            .iter()
            .any(|i| matches!(i, Insn::BinOpConst(..)));
        assert!(has_binopconst, "optimizer should fuse LoadConst+BinOp into BinOpConst for n+5");
    }

    // ── pass_unary_fold ───────────────────────────────────────────────────────

    #[test]
    fn unary_fold_neg_int() {
        use crate::ast::UnaryOp;
        use crate::value::Value;
        // LoadConst(r=5, idx=0) [consts[0]=-3]  UnaryOp(dst=1, Neg, r=5)
        // → LoadConst(dst=1, idx_3)
        let mut consts = vec![Value::int(-3)];
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::UnaryOp(1, UnaryOp::Neg, 5),
            Insn::Return(1),
        ];
        let out = pass_unary_fold(insns, 2, &mut consts);
        assert_eq!(out.len(), 2, "LoadConst should be removed");
        assert!(
            matches!(out[0], Insn::LoadConst(1, _)),
            "UnaryOp should become LoadConst"
        );
        let idx = match out[0] { Insn::LoadConst(_, i) => i, _ => panic!() };
        assert!(matches!(consts[idx as usize].kind(), crate::value::ValueKind::Int(3)));
    }

    #[test]
    fn unary_fold_not_bool() {
        use crate::ast::UnaryOp;
        use crate::value::Value;
        let mut consts = vec![Value::bool_(true)];
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::UnaryOp(1, UnaryOp::Not, 5),
            Insn::Return(1),
        ];
        let out = pass_unary_fold(insns, 2, &mut consts);
        assert_eq!(out.len(), 2, "LoadConst should be removed");
        let idx = match out[0] { Insn::LoadConst(_, i) => i, _ => panic!() };
        assert!(matches!(consts[idx as usize].kind(), crate::value::ValueKind::Bool(false)));
    }

    #[test]
    fn unary_fold_skips_local_reg() {
        use crate::ast::UnaryOp;
        use crate::value::Value;
        // r=1 < num_locals=3: should not fuse.
        let mut consts = vec![Value::int(5)];
        let insns = vec![
            Insn::LoadConst(1, 0),
            Insn::UnaryOp(2, UnaryOp::Neg, 1),
            Insn::Return(2),
        ];
        let out = pass_unary_fold(insns, 3, &mut consts);
        assert_eq!(out.len(), 3, "no fusion for local register");
    }

    #[test]
    fn unary_fold_on_compiled_literal() {
        // -5 is a literal unary-neg in the source; after removing the fold_constant check
        // from compile_expr, the optimizer should fold it via pass_unary_fold.
        let code = compile_fn("x = -5\nprint(x)\n");
        let optimized = optimize(code);
        // The constant pool should contain -5 (or the LoadConst(-5) folded from Neg+5).
        let has_neg5 = optimized
            .consts
            .iter()
            .any(|v| matches!(v.kind(), crate::value::ValueKind::Int(-5)));
        assert!(has_neg5, "constant -5 should appear after unary fold");
    }

    // ── pass_algebraic_simplify ───────────────────────────────────────────────

    #[test]
    fn algebraic_add_zero_becomes_move() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        let mut consts = vec![Value::int(0)];
        let insns = vec![Insn::BinOpConst(2, 1, BinaryOp::Add, 0)];
        let out = pass_algebraic_simplify(insns, &mut consts);
        assert!(matches!(out[0], Insn::Move(2, 1)), "x+0 should become Move");
    }

    #[test]
    fn algebraic_mul_zero_becomes_loadconst() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        let mut consts = vec![Value::int(0)];
        let insns = vec![Insn::BinOpConst(2, 1, BinaryOp::Mul, 0)];
        let out = pass_algebraic_simplify(insns, &mut consts);
        assert!(
            matches!(out[0], Insn::LoadConst(2, _)),
            "x*0 should become LoadConst(0)"
        );
    }

    #[test]
    fn algebraic_pow_zero_becomes_loadconst_one() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        let mut consts = vec![Value::int(0)];
        let insns = vec![Insn::BinOpConst(2, 1, BinaryOp::Pow, 0)];
        let out = pass_algebraic_simplify(insns, &mut consts);
        assert!(
            matches!(out[0], Insn::LoadConst(2, _)),
            "x**0 should become LoadConst(1)"
        );
        let idx = match out[0] { Insn::LoadConst(_, i) => i, _ => panic!() };
        assert!(matches!(consts[idx as usize].kind(), crate::value::ValueKind::Int(1)));
    }

    #[test]
    fn algebraic_skips_float_identity() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // 0.0 is int-0-like but we only simplify integer constants.
        let mut consts = vec![Value::float(0.0)];
        let insns = vec![Insn::BinOpConst(2, 1, BinaryOp::Add, 0)];
        let out = pass_algebraic_simplify(insns, &mut consts);
        // Should NOT simplify float: x + 0.0 has different NaN/inf semantics.
        assert!(
            matches!(out[0], Insn::BinOpConst(2, 1, BinaryOp::Add, 0)),
            "float identity should not be simplified"
        );
    }

    #[test]
    fn algebraic_on_compiled_code() {
        // x + 0 inside a function — algebraic pass should fold to Move.
        let code = compile_fn("def f(x):\n    return x + 0\n");
        let optimized = optimize(code);
        // After simplification x+0 becomes Move(dst,x), then trivial_nop removes Move(r,r).
        // Either way, there should be no BinOpConst in the output.
        let has_binopconst = optimized.fn_protos[0]
            .code
            .insns
            .iter()
            .any(|i| matches!(i, Insn::BinOpConst(..)));
        assert!(!has_binopconst, "x+0 should not leave a BinOpConst");
    }

    // ── pass_const_fold ───────────────────────────────────────────────────────

    #[test]
    fn const_fold_binopconst_with_known_lhs() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // LoadConst(r0, 0)  [consts[0]=5]
        // BinOpConst(r1, r0, Add, 1)  [consts[1]=3]
        // → LoadConst(r1, 2)  [consts[2]=8]
        let mut consts = vec![Value::int(5), Value::int(3)];
        let insns = vec![
            Insn::LoadConst(0, 0), // r0 = 5
            Insn::BinOpConst(1, 0, BinaryOp::Add, 1), // r1 = r0 + 3
            Insn::Return(1),
        ];
        let out = pass_const_fold(insns, &mut consts);
        assert!(
            matches!(out[1], Insn::LoadConst(1, _)),
            "BinOpConst with known lhs should be folded to LoadConst"
        );
        let folded_idx = match out[1] { Insn::LoadConst(_, i) => i, _ => panic!() };
        assert!(matches!(consts[folded_idx as usize].kind(), crate::value::ValueKind::Int(8)));
    }

    #[test]
    fn const_fold_binop_with_both_known() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        let mut consts = vec![Value::int(10), Value::int(2)];
        let insns = vec![
            Insn::LoadConst(0, 0), // r0 = 10
            Insn::LoadConst(1, 1), // r1 = 2
            Insn::BinOp(2, 0, BinaryOp::Mul, 1), // r2 = r0 * r1
            Insn::Return(2),
        ];
        let out = pass_const_fold(insns, &mut consts);
        assert!(
            matches!(out[2], Insn::LoadConst(2, _)),
            "BinOp with both operands known should fold to LoadConst"
        );
        let idx = match out[2] { Insn::LoadConst(_, i) => i, _ => panic!() };
        assert!(matches!(consts[idx as usize].kind(), crate::value::ValueKind::Int(20)));
    }

    #[test]
    fn const_fold_propagates_through_move() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // LoadConst(t, idx_5)  Move(x, t)  BinOpConst(y, x, Add, idx_3)
        // After propagation: known[x]=idx_5, fold BinOpConst to LoadConst(y, idx_8)
        let mut consts = vec![Value::int(5), Value::int(3)];
        let insns = vec![
            Insn::LoadConst(5, 0), // temp=5 (reg 5)
            Insn::Move(0, 5),      // x = temp
            Insn::BinOpConst(1, 0, BinaryOp::Add, 1), // y = x + 3
            Insn::Return(1),
        ];
        let out = pass_const_fold(insns, &mut consts);
        assert!(
            matches!(out[2], Insn::LoadConst(1, _)),
            "BinOpConst should fold after Move propagates known value"
        );
    }

    #[test]
    fn const_fold_clears_at_branch() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // LoadConst(r0, idx_5)  JumpIfFalse(r0, 0)  BinOpConst(r1, r0, Add, idx_3)
        // After the branch, known is cleared, so BinOpConst should NOT fold.
        let mut consts = vec![Value::int(5), Value::int(3)];
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::JumpIfFalse(0, 0),
            Insn::BinOpConst(1, 0, BinaryOp::Add, 1),
            Insn::Return(1),
        ];
        let out = pass_const_fold(insns, &mut consts);
        assert!(
            matches!(out[2], Insn::BinOpConst(1, 0, BinaryOp::Add, 1)),
            "no folding after a branch clears known map"
        );
    }

    #[test]
    fn const_fold_on_compiled_chain() {
        // x = 5; y = x * 2 — the optimizer should fold y to 10
        let code = compile_fn("x = 5\ny = x * 2\nprint(y)\n");
        let optimized = optimize(code);
        // After folding, the constant pool should contain 10
        let has_10 = optimized
            .consts
            .iter()
            .any(|v| matches!(v.kind(), crate::value::ValueKind::Int(10)));
        assert!(has_10, "constant 10 should appear in pool after folding x*2 with x=5");
    }

    // ── pass_cmpjump_fusion ───────────────────────────────────────────────────

    #[test]
    fn cmpjump_fuses_binopconst_jumpiffalse() {
        use crate::ast::BinaryOp;
        // BinOpConst(r=5, lhs=0, Gt, c=0) + JumpIfFalse(r=5, k=0)
        // k=0: if-false jumps to old_pos 1+1+0=2 = Return.
        // After fusion+compaction: CmpJumpIfFalseConst at new_pos 0, Return at new_pos 1.
        // Rewritten offset: to_new[2]-to_new[1]-1 = 1-0-1 = 0 → same k=0.
        let insns = vec![
            Insn::BinOpConst(5, 0, BinaryOp::Gt, 0),
            Insn::JumpIfFalse(5, 0),
            Insn::Return(0),
        ];
        let out = pass_cmpjump_fusion(insns, 2);
        assert_eq!(out.len(), 2, "BinOpConst should be removed");
        assert!(
            matches!(out[0], Insn::CmpJumpIfFalseConst(0, BinaryOp::Gt, 0, 0)),
            "should become CmpJumpIfFalseConst with same offset"
        );
    }

    #[test]
    fn cmpjump_fuses_binop_jumpiftrue() {
        use crate::ast::BinaryOp;
        // BinOp(r=5, lhs=0, Eq, rhs=1) + JumpIfTrue(r=5, k=0)
        // → CmpJumpIfTrue(lhs=0, Eq, rhs=1, k=0)
        let insns = vec![
            Insn::BinOp(5, 0, BinaryOp::Eq, 1),
            Insn::JumpIfTrue(5, 0),
            Insn::Return(0),
        ];
        let out = pass_cmpjump_fusion(insns, 2);
        assert_eq!(out.len(), 2, "BinOp should be removed");
        assert!(
            matches!(out[0], Insn::CmpJumpIfTrue(0, BinaryOp::Eq, 1, 0)),
            "should become CmpJumpIfTrue"
        );
    }

    #[test]
    fn cmpjump_skips_when_reg_is_local() {
        use crate::ast::BinaryOp;
        // r=1 < num_locals=3 → no fusion
        let insns = vec![
            Insn::BinOpConst(1, 0, BinaryOp::Gt, 0),
            Insn::JumpIfFalse(1, 1),
            Insn::Return(0),
        ];
        let out = pass_cmpjump_fusion(insns, 3);
        assert_eq!(out.len(), 3, "no fusion when cond reg is a local");
    }

    #[test]
    fn cmpjump_fusion_on_compiled_if() {
        let code = compile_fn("x = 5\nif x > 3:\n    print(x)\n");
        let optimized = optimize(code);
        let has_cmpjump = optimized.insns.iter().any(|i| {
            matches!(
                i,
                Insn::CmpJumpIfFalse(..)
                    | Insn::CmpJumpIfTrue(..)
                    | Insn::CmpJumpIfFalseConst(..)
                    | Insn::CmpJumpIfTrueConst(..)
            )
        });
        assert!(has_cmpjump, "optimizer should fuse comparison into conditional jump");
    }

    // ── pass_thread_jumps ─────────────────────────────────────────────────────

    #[test]
    fn thread_jumps_collapses_chain() {
        // [0] Jump(1)  [1] LoadNone(0)  [2] Jump(1)  [3] LoadNone(1)  [4] Return(1)
        // Jump at 0 targets idx 2 (0+1+1=2). idx 2 is Jump(1) → idx 4.
        // After threading: Jump at 0 should target 4 directly → offset = 4-(0+1)=3.
        let insns = vec![
            Insn::Jump(1),     // 0 → 2
            Insn::LoadNone(0), // 1
            Insn::Jump(1),     // 2 → 4
            Insn::LoadNone(1), // 3
            Insn::Return(1),   // 4
        ];
        let out = pass_thread_jumps(insns);
        assert!(
            matches!(out[0], Insn::Jump(3)),
            "Jump(1) at 0 should be threaded to Jump(3) (target idx 4)"
        );
    }

    #[test]
    fn thread_jumps_handles_self_loop() {
        // Jump(-1) loops to itself — threading must not infinite-loop.
        let insns = vec![Insn::Jump(-1)];
        let out = pass_thread_jumps(insns);
        assert_eq!(out.len(), 1);
        assert!(
            matches!(out[0], Insn::Jump(-1)),
            "self-loop must be left unchanged"
        );
    }

    #[test]
    fn thread_conditional_jump_through_unconditional() {
        // [0] JumpIfFalse(r, 1)   [1] Jump(1)   [2] LoadNone(0)   [3] Return(0)
        // JumpIfFalse at 0 targets 2. idx 2 is Jump(1) targeting idx 4 (past end).
        // After threading JumpIfFalse should target idx 4 as well.
        let insns = vec![
            Insn::JumpIfFalse(0, 1), // 0 → target 2
            Insn::Jump(1),           // 1 → target 3
            Insn::LoadNone(0),       // 2
            Insn::Return(0),         // 3
        ];
        let out = pass_thread_jumps(insns);
        // JumpIfFalse at 0 targeted 2; idx 2 is LoadNone (not a Jump) so no threading there.
        // JumpIfFalse's target is idx 2 which is NOT a Jump — offset stays 1.
        assert!(
            matches!(out[0], Insn::JumpIfFalse(0, 1)),
            "no chain to thread for the conditional"
        );
        // Jump at 1 targets 3 (Return), which is not a Jump.
        assert!(matches!(out[1], Insn::Jump(1)));
    }

    #[test]
    fn thread_jumps_no_change_when_no_chains() {
        // No jump chains → output equals input.
        let insns = vec![
            Insn::LoadNone(0),
            Insn::JumpIfFalse(0, 1),
            Insn::LoadNone(1),
            Insn::Return(0),
        ];
        let out = pass_thread_jumps(insns.clone());
        assert_eq!(out.len(), insns.len());
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
        assert!(
            matches!(out[0], Insn::Jump(0)),
            "offset rewritten: 2→1, new offset = 1-(0+1)=0"
        );
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
            Insn::Jump(0), // <- should be removed
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
            Insn::Move(2, 2), // <- should be removed
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
            Insn::Jump(1), // targets idx 3 (Return)
            Insn::Jump(0), // no-op, removed
            Insn::Return(0),
        ];
        let out = pass_trivial_nop(insns);
        assert_eq!(out.len(), 3);
        assert!(
            matches!(out[1], Insn::Jump(0)),
            "offset should decrease by 1"
        );
    }
}
