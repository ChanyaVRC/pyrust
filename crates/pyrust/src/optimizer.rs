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
    let mut num_regs = code.num_regs;
    let mut consts = code.consts;
    let insns = pass_thread_jumps(code.insns);
    let insns = pass_binop_const_fusion(insns, num_locals);
    let insns = pass_fold_const_tuple(insns, num_locals, &mut consts);
    let insns = pass_const_fold(insns, &mut consts);
    let insns = pass_algebraic_simplify(insns, &mut consts);
    let insns = pass_unary_fold(insns, num_locals, &mut consts);
    let insns = pass_ivsr(insns, &mut consts, &mut num_regs);
    let insns = pass_const_branch_elim(insns, &consts);
    let insns = pass_cmpjump_fusion(insns, num_locals);
    let insns = pass_not_invert(insns, num_locals);
    let insns = pass_binopinplace_downgrade(insns, num_locals);
    let insns = pass_exit_inline(insns);
    let insns = pass_licm(insns);
    let insns = pass_cse(insns);
    let insns = pass_dead_code(insns);
    let insns = pass_dead_store_elim(insns, num_locals);
    let insns = pass_copy_prop(insns);
    let insns = pass_trivial_nop(insns);
    let insns = pass_self_tail_call(insns);
    let (insns, consts) = pass_compact_consts(insns, consts);

    FnCode {
        insns,
        consts,
        names: code.names,
        num_regs,
        num_iters: code.num_iters,
        num_locals,
        fn_protos,
        cell_vars: code.cell_vars,
        is_generator: code.is_generator,
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
    let mut keep = vec![true; n];

    let mut i = 0;
    while i + 1 < n {
        if let (Insn::LoadConst(lc_reg, c_idx), Insn::BinOp(dst, lhs, op, rhs)) =
            (&transformed[i], &transformed[i + 1])
        {
            let (lc_reg, c_idx) = (*lc_reg, *c_idx);
            let (dst, lhs, op, rhs) = (*dst, *lhs, *op, *rhs);
            // Case 1: const is the RHS operand → BinOpConst(dst, lhs, op, c)
            if rhs == lc_reg
                && lhs != lc_reg
                && lc_reg >= num_locals
                && !slice_has_back_edge(&transformed[i + 2..])
                && (dst == lc_reg || !reg_is_read_in(&transformed[i + 2..], lc_reg))
            {
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

// ─── Constant tuple folding ────────────────────────────────────────────────────

/// Fold a sequence of `LoadConst` instructions feeding a `BuildTuple` into a
/// single `LoadConst` pointing to a pre-built tuple constant.
///
/// ## Pattern
///
/// ```text
/// LoadConst(base+0, c0)
/// LoadConst(base+1, c1)
/// ...
/// LoadConst(base+n-1, c_{n-1})
/// BuildTuple(dst, base, n)
/// ```
/// → replaced with a single `LoadConst(dst, tuple_pool_idx)` where
///   `consts[tuple_pool_idx]` is `Value::tuple([consts[c0], ..., consts[c_{n-1}]])`.
///
/// ## Guards
///
/// - Only `BuildTuple`, not `BuildList` (lists are mutable, tuples are immutable
///   constants and safe to deduplicate).
/// - `n >= 1 && n <= 16` — avoids unbounded look-back.
/// - All `base+j >= num_locals` — the element registers must be temporaries, not
///   named locals that could have been written by non-`LoadConst` instructions.
/// - `insns[i-n .. i]` are exactly `LoadConst(base+j, c_j)` for j in 0..n — the
///   look-back must be a perfect, contiguous, in-order match.
fn pass_fold_const_tuple(insns: Vec<Insn>, num_locals: u32, consts: &mut Vec<Value>) -> Vec<Insn> {
    let n = insns.len();
    let mut transformed = insns;
    let mut keep = vec![true; n];

    let mut i = 0;
    while i < n {
        if let Insn::BuildTuple(dst, base, argc) = transformed[i] {
            let argc = argc as usize;
            if (1..=16).contains(&argc) && i >= argc {
                // Check that insns[i-argc .. i] are LoadConst(base+j, c_j) for j in 0..argc
                // and that all base+j >= num_locals.
                let mut all_match = true;
                let mut c_indices: Vec<u16> = Vec::with_capacity(argc);
                for j in 0..argc {
                    let slot = i - argc + j;
                    match transformed[slot] {
                        Insn::LoadConst(reg, c_idx)
                            if reg == base + j as u32 && reg >= num_locals =>
                        {
                            c_indices.push(c_idx);
                        }
                        _ => {
                            all_match = false;
                            break;
                        }
                    }
                }

                if all_match {
                    // Build the tuple value from the pooled constants.
                    let elems: Vec<Value> = c_indices
                        .iter()
                        .map(|&ci| consts[ci as usize].clone())
                        .collect();
                    let tuple_val = Value::tuple(elems);

                    // Intern the tuple in the const pool (always append — tuples are
                    // not deduplicated by intern_const_in_pool which only handles
                    // scalars, and identity equality on tuples is object-level).
                    if consts.len() < u16::MAX as usize {
                        let new_idx = consts.len() as u16;
                        consts.push(tuple_val);

                        // Mark the n LoadConst predecessors as removed.
                        for j in 0..argc {
                            keep[i - argc + j] = false;
                        }
                        // Replace BuildTuple with LoadConst(dst, new_idx).
                        transformed[i] = Insn::LoadConst(dst, new_idx);
                    }
                }
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
/// which path was taken at runtime, and also at loop headers (targets of
/// backward jumps) to avoid incorrectly folding loop conditions.
fn pass_const_fold(insns: Vec<Insn>, consts: &mut Vec<Value>) -> Vec<Insn> {
    // Pre-pass: collect every instruction index that is the target of a backward
    // jump.  At these loop headers the known-constant map must be cleared so we
    // do not fold values that differ across iterations.
    let mut loop_headers: HashSet<usize> = HashSet::new();
    for (i, insn) in insns.iter().enumerate() {
        let k: Option<i32> = match insn {
            Insn::Jump(k) => Some(*k),
            Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::SetupExcept(k) => Some(*k),
            _ => None,
        };
        if let Some(k) = k
            && k < 0
        {
            let target = (i as i64 + 1 + k as i64) as usize;
            loop_headers.insert(target);
        }
    }

    let mut known: HashMap<u32, u16> = HashMap::new();
    let mut out = Vec::with_capacity(insns.len());

    for (i, insn) in insns.into_iter().enumerate() {
        if loop_headers.contains(&i) {
            known.clear();
        }
        match insn {
            Insn::LoadConst(dst, c) => {
                known.insert(dst, c);
                out.push(Insn::LoadConst(dst, c));
            }
            Insn::Move(dst, src) => {
                match known.get(&src).copied() {
                    Some(c) => {
                        known.insert(dst, c);
                    }
                    None => {
                        known.remove(&dst);
                    }
                }
                out.push(Insn::Move(dst, src));
            }
            Insn::BinOp(dst, lhs, op, rhs) => {
                let folded = known.get(&lhs).and_then(|&cl| {
                    known.get(&rhs).and_then(|&cr| {
                        crate::compiler::fold_binop(&consts[cl as usize], op, &consts[cr as usize])
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
                    crate::compiler::fold_binop(&consts[cl as usize], op, &consts[c as usize])
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
            | Insn::Unpack(..)
            | Insn::UnpackEx { .. }) => {
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
        | DeleteLocal(r)
        | BinOp(r, _, _, _)
        | BinOpConst(r, _, _, _)
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
        // Loop instructions write to their first register on each iteration.
        // Without these arms, pass_const_fold would fail to invalidate the
        // known-constant map entry for the destination, producing stale folds
        // if the blanket known.clear() at loop instructions were ever removed.
        ForIter(dst, _, _) => Some(*dst),
        ForCountReg(var, _, _, _, _) => Some(*var),
        ForCountConst(var, _, _, _, _) => Some(*var),
        // CopyReg is emitted by the CSE pass; it writes to dst just like Move.
        CopyReg(r, _) => Some(*r),
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
            | Insn::RaiseAssert(_)
            | Insn::TailCall { .. } => {}

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
                        let idx = intern_const_in_pool(consts, Value::int(0)).unwrap_or(c_idx);
                        Insn::LoadConst(dst, idx)
                    }
                    (Pow, 0) => {
                        let idx = intern_const_in_pool(consts, Value::int(1)).unwrap_or(c_idx);
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
        let fused: Option<(u32, Value)> = match (&transformed[i], &transformed[i + 1]) {
            (Insn::LoadConst(lc_reg, c_idx), Insn::UnaryOp(dst, op, src))
                if *src == *lc_reg
                    && *lc_reg >= num_locals
                    && !slice_has_back_edge(&transformed[i + 2..])
                    // When dst==lc_reg the fusion overwrites lc_reg with the result,
                    // so any later read of lc_reg will see the correct folded value.
                    // When dst!=lc_reg, lc_reg would become uninitialized after removal.
                    && (*dst == *lc_reg || !reg_is_read_in(&transformed[i + 2..], *lc_reg)) =>
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

// ─── Constant-condition branch elimination ─────────────────────────────────────

/// Replace conditional jumps whose condition register was just loaded from a
/// known constant with an unconditional `Jump`:
///
/// - `LoadConst(r, c) + JumpIfFalse(r, k)` → keep LoadConst; replace with `Jump(k)` if falsy, `Jump(0)` if truthy
/// - `LoadConst(r, c) + JumpIfTrue(r, k)` → keep LoadConst; replace with `Jump(k)` if truthy, `Jump(0)` if falsy
///
/// The unconditional jumps are then cleaned up by `pass_dead_code` (removes
/// unreachable instructions) and `pass_trivial_nop` (removes `Jump(0)`).
fn pass_const_branch_elim(insns: Vec<Insn>, consts: &[Value]) -> Vec<Insn> {
    let n = insns.len();
    let mut out = insns;

    let mut i = 0;
    while i + 1 < n {
        if let (Insn::LoadConst(lc_reg, c_idx), jump) = (&out[i], &out[i + 1]) {
            let (lc_reg, c_idx) = (*lc_reg, *c_idx);
            let truthy = consts[c_idx as usize].truthy();
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

// ─── Register liveness helpers ────────────────────────────────────────────────

/// Returns `true` if register `r` is read by any instruction in `insns`.
/// Returns `true` if `insns` contains a backward jump (negative offset).
///
/// A back-edge means the slice re-enters an earlier instruction, so a forward
/// liveness scan alone cannot prove a register is dead — the register may be
/// read on the next loop iteration.  Passes that would remove a `LoadConst`
/// based solely on `reg_is_read_in` must guard with this check.
fn slice_has_back_edge(insns: &[Insn]) -> bool {
    insns.iter().any(|insn| {
        matches!(insn,
            Insn::Jump(k)
            | Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            if *k < 0
        )
    })
}

/// Used as a forward liveness guard before removing a `LoadConst` that produced `r`.
fn reg_is_read_in(insns: &[Insn], r: u32) -> bool {
    insns.iter().any(|insn| insn_reads_reg(insn, r))
}

/// Returns `true` if `insn` reads the value of register `r`.
fn insn_reads_reg(insn: &Insn, r: u32) -> bool {
    use Insn::*;
    match insn {
        // No register sources.
        LoadConst(..) | LoadGlobal(..) | LoadNone(..) | LoadExc(..) | ImportModule(..)
        | DeleteName(..) | DeleteLocal(..) | Jump(..) | SetupExcept(..) | PopExcept | EndExcept
        | ReturnNone | RaiseReRaise | ForIter(..) | ForCountConst(..) => false,

        // One source register.
        StoreGlobal(_, s)
        | Move(_, s)
        | CopyReg(_, s)
        | UnaryOp(_, _, s)
        | Return(s)
        | PrintExpr(s)
        | RaiseValue(s)
        | RaiseAssert(s)
        | JumpIfFalse(s, _)
        | JumpIfTrue(s, _)
        | GetIter(_, s)
        | Unpack(_, s, _)
        | CheckLocal(s, _)
        | GetAttr(_, s, _)
        | DeleteAttr(s, _)
        | BinOpConst(_, s, _, _)
        | CmpJumpIfFalseConst(s, _, _, _)
        | CmpJumpIfTrueConst(s, _, _, _)
        | MatchExcept(s, _) => *s == r,

        // Two source registers.
        BinOp(_, a, _, b)
        | BinOpInPlace(_, a, _, b)
        | CmpJumpIfFalse(a, _, b, _)
        | CmpJumpIfTrue(a, _, b, _)
        | RaiseFrom(a, b)
        | SetAdd(a, b)
        | ListAppend(a, b)
        | ListExtend(a, b)
        | DictUpdate(a, b)
        | GetItem(_, a, b)
        | DeleteItem(a, b) => *a == r || *b == r,

        SetAttr(obj, _, val) => *obj == r || *val == r,
        ForCountReg(_, _, stop, _, _) => *stop == r,

        // Three source registers.
        SetItem(a, b, c) => *a == r || *b == r || *c == r,

        // Range-based: func + args live in consecutive registers.
        Call(base, argc) | CallMemo(base, argc) => r >= *base && r <= *base + *argc as u32,
        TailCall { args_base, nargs } => {
            r == args_base.wrapping_sub(1) || (r >= *args_base && r < *args_base + *nargs as u32)
        }
        BuildList(_, base, n) | BuildTuple(_, base, n) => r >= *base && r < *base + *n as u32,
        // BuildDict stores n key-value PAIRS — each pair occupies 2 registers,
        // so the live range is base .. base + 2*n (not base + n).
        BuildDict(_, base, n) => r >= *base && r < *base + 2 * *n as u32,

        CallMethod {
            obj,
            args_base,
            nargs,
            ..
        } => *obj == r || (r >= *args_base && r < *args_base + *nargs as u32),
        CallMethodExpanded {
            obj,
            pos_list,
            kw_dict,
            ..
        } => *obj == r || *pos_list == r || *kw_dict == r,

        MakeFunction(_, _, defs_base, defs_n) => r >= *defs_base && r < *defs_base + *defs_n as u32,
        MakeClass(_, _, bases_base, bases_n, _) => {
            r >= *bases_base && r < *bases_base + *bases_n as u32
        }

        // Yield reads src and writes dst.
        Yield { src, dst: _ } => *src == r,

        // UnpackEx reads src.
        UnpackEx { src, .. } => *src == r,
    }
}

// ─── BinOpInPlace → BinOp downgrade ───────────────────────────────────────────

/// Replace `BinOpInPlace(dst, lhs, op, rhs)` with `BinOp(dst, lhs, op, rhs)`
/// when `lhs` is a temp register that is dead after this instruction.
///
/// ## Why this helps
///
/// `BinOpInPlace` dispatches `__i<op>__` (e.g. `__iadd__`) first and falls back
/// to `__<op>__` on failure.  For immutable built-in types (int, float, str) the
/// `__iadd__` lookup always fails, adding a method-resolution step per execution.
/// Downgrading to plain `BinOp` skips that wasted dispatch.
///
/// ## Guards
///
/// - `lhs >= num_locals`: restrict to temp registers.  Named locals (0..num_locals)
///   can hold user-defined objects with custom `__iadd__`; downgrading those would
///   silently change semantics.
/// - `!reg_is_read_in(&insns[i+1..], lhs)`: `lhs` must be dead after this
///   instruction.  If `lhs` is live, the in-place semantics (writing back the
///   result to `lhs`) may matter to downstream reads — but since we emit `BinOp`
///   which writes to `dst`, this is only safe when `lhs` is not read further.
///   (If `dst == lhs` the result is in the same register either way.)
///
/// Reference: GCC algebraic simplification; classical augmented-assignment lowering.
fn pass_binopinplace_downgrade(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    insns
        .iter()
        .enumerate()
        .map(|(i, insn)| {
            if let Insn::BinOpInPlace(dst, lhs, op, rhs) = insn
                && *lhs >= num_locals
                && (*dst == *lhs || !reg_is_read_in(&insns[i + 1..], *lhs))
            {
                return Insn::BinOp(*dst, *lhs, *op, *rhs);
            }
            insn.clone()
        })
        .collect()
}

// ─── Exit-block inlining ───────────────────────────────────────────────────────

/// Replace an unconditional `Jump(k)` with the instruction it targets when that
/// target is a single-instruction terminal (`Return(r)` or `ReturnNone`).
///
/// ## Rationale
///
/// Compiled `if/else` branches often look like:
///
/// ```text
/// // true branch
/// LoadConst(r, 1)
/// Jump(k)           ← points to epilogue Return
/// // false branch
/// LoadConst(r, 0)
/// Jump(k)           ← same epilogue Return
/// // epilogue
/// Return(r)
/// ```
///
/// Replacing both Jumps with `Return(r)` directly — a 1-for-1 substitution that
/// leaves all other offsets intact — eliminates two taken branches.  The now-dead
/// epilogue `Return` is removed in the subsequent `pass_dead_code`.
///
/// Only single-instruction terminals are inlined to avoid shifting instruction
/// offsets (which would require a separate offset-fixup pass).
fn pass_exit_inline(insns: Vec<Insn>) -> Vec<Insn> {
    let n = insns.len();
    insns
        .iter()
        .enumerate()
        .map(|(i, insn)| {
            if let Insn::Jump(k) = insn {
                let target = (i as i64 + 1 + *k as i64) as usize;
                if target < n && target != i {
                    match &insns[target] {
                        t @ (Insn::Return(_) | Insn::ReturnNone) => return t.clone(),
                        _ => {}
                    }
                }
            }
            insn.clone()
        })
        .collect()
}

// ─── Loop-Invariant Code Motion (LICM) ────────────────────────────────────────

/// Hoist loop-invariant pure instructions out of loop bodies to just before the
/// loop header.
///
/// ## What is hoisted
///
/// Only instructions that are definitely free of observable side effects:
/// - `LoadConst(dst, idx)` — always loop-invariant (constant value).
/// - `BinOpConst(dst, src, op, idx)` — loop-invariant when `src` is not written
///   anywhere in the loop body.
/// - `UnaryOp(dst, op, src)` — loop-invariant when `src` is not written in the
///   loop body.
///
/// ## What is NOT hoisted
///
/// `BinOp`, `Call`, `CallMethod`, `GetAttr`, `SetAttr`, `LoadGlobal`,
/// `StoreGlobal`, store instructions, and all loop/branch/exception instructions
/// are left in place because they may have side effects or their correct
/// behaviour depends on the iteration context.
///
/// ## Loop detection
///
/// A back edge is any `Jump(k)` where `k < 0` (the target is before the current
/// instruction).  Each back edge `(latch_pc, header_pc)` defines a natural loop
/// `[header_pc, latch_pc]`.  Nested loops are handled individually: the inner
/// loop's back edge produces an inner `[header, latch]` range whose hoisting
/// point is just before the inner header, not before the outer header.
///
/// ## Exception handlers
///
/// If `SetupExcept` or `PopExcept` appears anywhere inside `[header, latch]` the
/// entire loop is skipped — hoisting across exception regions is not safe.
///
/// ## Fixed-point iteration
///
/// The pass repeats the hoist loop until no new instructions are moved, so that
/// an instruction whose invariant inputs were themselves just hoisted can also be
/// hoisted in the same call.
fn pass_licm(insns: Vec<Insn>) -> Vec<Insn> {
    let n = insns.len();
    if n == 0 {
        return insns;
    }

    // Collect all back edges: (header_pc, latch_pc).
    // A Jump(k) at position i is a back edge when the target (i+1+k) <= i.
    let mut back_edges: Vec<(usize, usize)> = Vec::new();
    for (i, insn) in insns.iter().enumerate() {
        if let Insn::Jump(k) = insn {
            let target = (i as i64 + 1 + *k as i64) as usize;
            if target <= i {
                back_edges.push((target, i)); // (header, latch)
            }
        }
    }

    if back_edges.is_empty() {
        return insns;
    }

    // Work on a mutable copy; `hoist` marks which positions to move out.
    let mut insns = insns;

    for (header, latch) in &back_edges {
        let (header, latch) = (*header, *latch);

        // Skip loops that contain exception handling — hoisting across
        // SetupExcept/PopExcept is not safe.
        let has_except = insns[header..=latch]
            .iter()
            .any(|i| matches!(i, Insn::SetupExcept(_) | Insn::PopExcept));
        if has_except {
            continue;
        }

        // Fixed-point: keep hoisting until nothing new moves.
        //
        // `body_start` tracks the current start of the loop body after successive
        // rounds of hoisting.  Each round moves some instructions to the pre-header
        // block, so the actual loop body shrinks from the front.  `body_end` (=latch)
        // never changes because instructions after the latch are not touched.
        let mut body_start = header;

        loop {
            // Rebuild the write count map for the current loop body [body_start..=latch].
            // `write_count[r]` = number of instructions in the body that write `r`.
            // We need counts (not just a set) so we can check whether a candidate
            // instruction is the *sole* writer of its destination — a necessary
            // condition for safe hoisting.
            let mut write_count: HashMap<u32, usize> = HashMap::new();
            for insn in &insns[body_start..=latch] {
                let mut tmp: HashSet<u32> = HashSet::new();
                collect_writes(insn, &mut tmp);
                for r in tmp {
                    *write_count.entry(r).or_insert(0) += 1;
                }
            }
            // Derive the flat write set (union of all writes) for source checking.
            let written: HashSet<u32> = write_count.keys().copied().collect();

            // Find the "safe hoist boundary": the exclusive upper bound of the
            // straight-line prefix of the loop body that is guaranteed to execute
            // on every iteration.
            //
            // Starting from `body_start` (the loop header, e.g. ForIter), we
            // advance past the header itself and then scan for the first
            // *additional* conditional branch.  Instructions strictly before that
            // branch are always executed — they dominate the back edge — so they
            // are safe to hoist regardless of runtime values.  Instructions at or
            // after a conditional branch are only executed on some iterations.
            //
            // The loop header itself (ForIter, ForCountConst, etc.) is an implicit
            // conditional (it exits the loop when the iterator is exhausted), but
            // it is NOT included in the hoist set; we advance past it first.
            let hoist_bound = {
                // Start just after the loop header.
                let mut bound = body_start + 1;
                for pc in (body_start + 1)..=latch {
                    match &insns[pc] {
                        // Unconditional jump is safe to pass through (it's the
                        // back edge or a structural jump, not a branch).
                        Insn::Jump(_) => {}
                        // Any conditional jump ends the safe prefix.
                        Insn::JumpIfFalse(..)
                        | Insn::JumpIfTrue(..)
                        | Insn::CmpJumpIfFalse(..)
                        | Insn::CmpJumpIfTrue(..)
                        | Insn::CmpJumpIfFalseConst(..)
                        | Insn::CmpJumpIfTrueConst(..)
                        | Insn::ForIter(..)
                        | Insn::ForCountReg(..)
                        | Insn::ForCountConst(..) => {
                            bound = pc;
                            break;
                        }
                        _ => {
                            bound = pc + 1; // extend bound to include this instruction
                        }
                    }
                }
                bound
            };

            // Collect indices (in order) of instructions to hoist.
            // Only consider instructions strictly within [body_start .. hoist_bound).
            // These are the instructions that dominate the back edge (guaranteed to
            // execute every iteration) and are pure (LoadConst, BinOpConst, UnaryOp).
            let mut to_hoist: Vec<usize> = Vec::new();
            for pc in body_start..hoist_bound {
                if is_loop_invariant(&insns[pc], &written, &write_count) {
                    to_hoist.push(pc);
                }
            }

            if to_hoist.is_empty() {
                break; // fixed-point reached — nothing new to hoist
            }

            // Strategy: reorder instructions so that the hoisted ones appear at
            // [body_start .. body_start+num_hoisted) — i.e. they slide to the
            // very beginning of the current body — and the remaining body follows
            // at [body_start+num_hoisted .. latch+1).  Instructions before
            // `body_start` and after `latch` are untouched (offsets still rewritten).
            let num_hoisted = to_hoist.len();
            let hoist_set: HashSet<usize> = to_hoist.iter().copied().collect();

            // Build old→new index map (size n+1 for the past-the-end sentinel).
            let mut old_to_new = vec![0usize; n + 1];

            // Before-body region: indices unchanged.
            for i in 0..body_start {
                old_to_new[i] = i;
            }
            // Hoisted instructions land at [body_start .. body_start+num_hoisted).
            {
                let mut slot = body_start;
                for &pc in &to_hoist {
                    old_to_new[pc] = slot;
                    slot += 1;
                }
            }
            // Non-hoisted loop body: [body_start+num_hoisted .. latch+1).
            {
                let mut slot = body_start + num_hoisted;
                for pc in body_start..=latch {
                    if !hoist_set.contains(&pc) {
                        old_to_new[pc] = slot;
                        slot += 1;
                    }
                }
            }
            // After-latch region: indices unchanged.
            for i in (latch + 1)..n {
                old_to_new[i] = i;
            }
            // Past-the-end sentinel.
            old_to_new[n] = n;

            // Scatter instructions into their new positions and fix jump offsets.
            let mut new_insns: Vec<Insn> = vec![Insn::ReturnNone; n];
            for (old_i, insn) in insns.iter().enumerate() {
                let new_i = old_to_new[old_i];
                new_insns[new_i] = rewrite_offsets(insn.clone(), old_i, &old_to_new);
            }
            insns = new_insns;

            // Advance body_start past the just-hoisted instructions: they now live
            // at [old_body_start .. old_body_start+num_hoisted) and are no longer
            // part of the loop body.
            body_start += num_hoisted;

            // Loop again: re-examine the updated body for newly invariant insns
            // whose source registers were themselves just hoisted.
        }
    }

    insns
}

/// Collect all registers *written* (defined) by `insn` into `written`.
fn collect_writes(insn: &Insn, written: &mut HashSet<u32>) {
    use Insn::*;
    match insn {
        LoadConst(r, _)
        | LoadGlobal(r, _)
        | LoadNone(r)
        | LoadExc(r)
        | ImportModule(r, _)
        | MakeFunction(r, _, _, _)
        | MakeClass(r, _, _, _, _)
        | BuildList(r, _, _)
        | BuildTuple(r, _, _)
        | BuildDict(r, _, _)
        | BinOp(r, _, _, _)
        | BinOpInPlace(r, _, _, _)
        | BinOpConst(r, _, _, _)
        | UnaryOp(r, _, _)
        | GetAttr(r, _, _)
        | GetItem(r, _, _)
        | Call(r, _)
        | CallMemo(r, _)
        | Move(r, _)
        | CopyReg(r, _)
        | DeleteLocal(r) => {
            written.insert(*r);
        }
        CallMethod { dst, .. } | CallMethodExpanded { dst, .. } | Yield { dst, .. } => {
            written.insert(*dst);
        }
        ForIter(dst, _, _) => {
            written.insert(*dst);
        }
        ForCountReg(var, _, _, _, _) | ForCountConst(var, _, _, _, _) => {
            written.insert(*var);
        }
        Unpack(base, _, n) => {
            for i in 0..*n {
                written.insert(base + i);
            }
        }
        UnpackEx {
            dst_base,
            before,
            after,
            ..
        } => {
            for i in 0..(*before as u32 + 1 + *after as u32) {
                written.insert(dst_base + i);
            }
        }
        // Instructions that don't write to any register.
        StoreGlobal(..)
        | SetAttr(..)
        | SetItem(..)
        | DeleteAttr(..)
        | DeleteItem(..)
        | DeleteName(..)
        | GetIter(..)
        | Jump(..)
        | JumpIfFalse(..)
        | JumpIfTrue(..)
        | CmpJumpIfFalse(..)
        | CmpJumpIfTrue(..)
        | CmpJumpIfFalseConst(..)
        | CmpJumpIfTrueConst(..)
        | Return(..)
        | ReturnNone
        | RaiseValue(..)
        | RaiseFrom(..)
        | RaiseReRaise
        | RaiseAssert(..)
        | SetupExcept(..)
        | PopExcept
        | EndExcept
        | MatchExcept(..)
        | CheckLocal(..)
        | PrintExpr(..)
        | SetAdd(..)
        | ListAppend(..)
        | ListExtend(..)
        | DictUpdate(..)
        | TailCall { .. } => {}
    }
}

/// Returns `true` if `insn` is a pure, loop-invariant instruction given the
/// set of registers written anywhere inside the loop body.
///
/// An instruction is loop-invariant when:
/// 1. It is one of the safe-to-hoist variants (`LoadConst`, `BinOpConst`, `UnaryOp`).
/// 2. None of its *source* registers appear in `written`.
/// 3. Its *destination* register is written only by this instruction inside the
///    loop body (`write_count[dst] == 1`).  If another instruction in the body
///    also writes `dst`, hoisting would change the value seen by instructions
///    that execute between the hoist point and the in-body write — incorrect.
fn is_loop_invariant(
    insn: &Insn,
    written: &HashSet<u32>,
    write_count: &HashMap<u32, usize>,
) -> bool {
    // True when `dst` is the sole writer of that register inside the body.
    let sole_writer = |dst: u32| write_count.get(&dst).copied().unwrap_or(0) == 1;

    match insn {
        // LoadConst has no register source; invariant if this is the sole write of dst.
        Insn::LoadConst(dst, _) => sole_writer(*dst),
        // BinOpConst reads `src`; invariant if `src` not written AND sole write of dst.
        Insn::BinOpConst(dst, src, _, _) => !written.contains(src) && sole_writer(*dst),
        // UnaryOp reads `src`; invariant if `src` not written AND sole write of dst.
        Insn::UnaryOp(dst, _, src) => !written.contains(src) && sole_writer(*dst),
        // Everything else: not hoisted.
        _ => false,
    }
}

// ─── Trivial no-op removal ─────────────────────────────────────────────────────

// Remove instructions that have no observable effect:
// - `Jump(0)` — offset 0 means the next instruction; equivalent to falling through
// - `Move(r, r)` — a register copied into itself
// ─── NOT-inversion ─────────────────────────────────────────────────────────────

/// Absorb `UnaryOp(r, Not, src)` into the following conditional jump by
///   inverting the branch sense, eliminating the boolean intermediate register.
///
/// ## Patterns
///
/// ```text
/// UnaryOp(r, Not, src) + JumpIfFalse(r, k)  →  JumpIfTrue(src, k)
/// UnaryOp(r, Not, src) + JumpIfTrue(r, k)   →  JumpIfFalse(src, k)
/// ```
///
/// ## Guards
/// - `r >= num_locals`: only fuse temp registers (named locals could be inspected
///   after the branch, e.g. in closures).
/// - `!reg_is_read_in(&insns[i+2..], r)`: `r` must be dead after the jump;
///   the liveness check reuses the existing `reg_is_read_in` helper.
///
/// ## Correctness
/// `not x` returns `bool`; the branch only tests truthiness.  Because
/// `bool(not x)` has the same truthiness as `not x`, inverting the branch
/// and removing the `UnaryOp` is semantically equivalent.
///
/// Reference: Lua `lcode.c` `jumponcond()`.
fn pass_not_invert(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    use crate::ast::UnaryOp;

    let n = insns.len();
    let mut transformed = insns;
    let mut keep = vec![true; n];

    let mut i = 0;
    while i + 1 < n {
        let fused: Option<Insn> = match (&transformed[i], &transformed[i + 1]) {
            (Insn::UnaryOp(r, UnaryOp::Not, src), Insn::JumpIfFalse(cond, k))
                if *r == *cond
                    && *r >= num_locals
                    && !reg_is_read_in(&transformed[i + 2..], *r) =>
            {
                Some(Insn::JumpIfTrue(*src, *k))
            }
            (Insn::UnaryOp(r, UnaryOp::Not, src), Insn::JumpIfTrue(cond, k))
                if *r == *cond
                    && *r >= num_locals
                    && !reg_is_read_in(&transformed[i + 2..], *r) =>
            {
                Some(Insn::JumpIfFalse(*src, *k))
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

// ─── Dead store elimination ────────────────────────────────────────────────────

/// Is register `r` read by any instruction in `insns` before the first
/// instruction that writes `r`?  This is strictly more precise than
/// `reg_is_read_in` for dead-store analysis: it stops as soon as it sees a
/// write to `r`, because that write kills any previous value.
fn reg_is_read_before_next_write(insns: &[Insn], r: u32) -> bool {
    for insn in insns {
        if insn_reads_reg(insn, r) {
            return true;
        }
        // Stop at the next write to r (the old value is dead from here on).
        if writable_dst(insn) == Some(r) {
            return false;
        }
        if matches!(insn, Insn::LoadConst(dst, _) | Insn::LoadNone(dst) | Insn::LoadGlobal(dst, _)
                         | Insn::Move(dst, _) | Insn::CopyReg(dst, _) if *dst == r)
        {
            return false;
        }
    }
    false
}

/// Remove writes to temp registers whose stored value is never read before
/// the next write to the same register.
///
/// ## Safety restrictions
///
/// - Only temp registers (`>= num_locals`) are considered; named locals may
///   escape via closures.
/// - Only *pure* instructions are removed: `LoadConst`, `LoadNone`,
///   `LoadGlobal`, `Move`, `BinOp`, `BinOpConst`, `UnaryOp`.  Instructions
///   with potential side effects (`Call`, `GetAttr`, `BinOpInPlace`, …) are
///   always preserved.
/// - A back-edge guard (`slice_has_back_edge`) prevents removing a store that
///   is the initial value consumed by a later loop iteration.
fn pass_dead_store_elim(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    let n = insns.len();
    let mut keep = vec![true; n];

    for i in 0..n {
        let dst = match &insns[i] {
            Insn::LoadConst(r, _)
            | Insn::LoadNone(r)
            | Insn::LoadGlobal(r, _)
            | Insn::Move(r, _)
            | Insn::CopyReg(r, _)
            | Insn::BinOp(r, _, _, _)
            | Insn::BinOpConst(r, _, _, _)
            | Insn::UnaryOp(r, _, _)
                if *r >= num_locals =>
            {
                *r
            }
            _ => continue,
        };

        // Conservative: skip if a back-edge could carry the value into the
        // next loop iteration (the forward scan below would miss that use).
        if slice_has_back_edge(&insns[i + 1..]) {
            continue;
        }

        if !reg_is_read_before_next_write(&insns[i + 1..], dst) {
            keep[i] = false;
        }
    }

    compact(insns, &keep)
}

// ─── Common subexpression elimination ─────────────────────────────────────────

/// Eliminate redundant computations within each basic block.
///
/// Within a straight-line sequence of instructions (a *basic block* — no jumps
/// in or out), if two instructions compute exactly the same value from the same
/// inputs, the second one is redundant.  This pass replaces the second with
/// `CopyReg(dst2, dst1)`, pointing `dst2` at the already-computed result.
///
/// ## Tracked expressions
///
/// Only *pure* instruction forms are tracked:
/// - `LoadConst(dst, idx)` — two loads of the same pool entry are identical.
/// - `BinOpConst(dst, src, op, idx)` — same operator, same source register,
///   same constant operand.
/// - `UnaryOp(dst, op, src)` — same operator, same source register.
///
/// `BinOp` is intentionally excluded: it could invoke user-defined `__add__`
/// which may have side effects.
///
/// ## CSE key and invalidation
///
/// The *CSE key* for a tracked instruction is `(discriminant, src_regs..., const_idx)`.
/// The map is cleared at every basic-block boundary (any branch, jump, or
/// exception instruction, as well as any instruction that is a jump *target*).
///
/// Whenever any register `r` is written by any instruction (whether tracked or
/// not), every CSE table entry whose key contains `r` as a source operand is
/// removed.  This prevents stale entries from matching if an input was mutated
/// between the two computations.
///
/// ## Interaction with later passes
///
/// The emitted `CopyReg` instructions are subsequently cleaned up by
/// `pass_dead_store_elim` (if the original dst is never read) and
/// `pass_trivial_nop` (if dst == src, which cannot happen here but is guarded
/// for safety).  `pass_copy_prop` does *not* chase through `CopyReg` — that
/// keeps the pass order simple and avoids invalidating other CSE entries.
///
/// Reference: Aho, Lam, Sethi, Ullman *Compilers* §9.1 (available expressions);
/// Kennedy *A Survey of Data-Flow Analysis Techniques* §3 (CSE).
fn pass_cse(insns: Vec<Insn>) -> Vec<Insn> {
    use std::collections::HashMap;

    /// Discriminator tag for a CSE key — keeps `LoadConst`, `BinOpConst`, and
    /// `UnaryOp` entries distinct even if their integer fields happen to overlap.
    #[derive(Eq, PartialEq, Hash, Clone)]
    enum CseKey {
        /// `LoadConst(_, idx)` — two loads of the same pool entry.
        LoadConst(u16),
        /// `BinOpConst(_, src, op, idx)`.
        BinOpConst(u32, crate::ast::BinaryOp, u16),
        /// `UnaryOp(_, op, src)`.
        UnaryOp(crate::ast::UnaryOp, u32),
    }

    let n = insns.len();
    if n == 0 {
        return insns;
    }

    // Pre-pass: mark every instruction that is a jump target so we can clear
    // the CSE table at basic-block boundaries.
    let mut is_bb_start = vec![false; n + 1];
    is_bb_start[0] = true;
    for (i, insn) in insns.iter().enumerate() {
        let k: Option<i32> = match insn {
            Insn::Jump(k)
            | Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k) => Some(*k),
            _ => None,
        };
        if let Some(k) = k {
            let target = (i as i64 + 1 + k as i64) as usize;
            if target <= n {
                is_bb_start[target] = true;
            }
        }
    }

    // `table`: CSE key → (original dst register that holds the result).
    let mut table: HashMap<CseKey, u32> = HashMap::new();
    let mut result: Vec<Insn> = Vec::with_capacity(n);

    for (i, insn) in insns.into_iter().enumerate() {
        // Clear CSE state at basic-block boundaries.
        if is_bb_start[i] {
            table.clear();
        }

        // Build the CSE key for this instruction, if it is a tracked pure form.
        let key: Option<(CseKey, u32)> = match &insn {
            Insn::LoadConst(dst, idx) => Some((CseKey::LoadConst(*idx), *dst)),
            Insn::BinOpConst(dst, src, op, idx) => {
                Some((CseKey::BinOpConst(*src, *op, *idx), *dst))
            }
            Insn::UnaryOp(dst, op, src) => Some((CseKey::UnaryOp(*op, *src), *dst)),
            _ => None,
        };

        // Determine which register (if any) this instruction writes to, so we
        // can evict stale CSE table entries BEFORE the match check.  Eviction
        // must happen regardless of whether the instruction is later replaced
        // by a CopyReg, because the CopyReg itself still writes to `dst`.
        let written_reg: Option<u32> = match &insn {
            Insn::LoadConst(r, _) | Insn::LoadNone(r) | Insn::LoadGlobal(r, _) => Some(*r),
            // Move writes its destination register; must evict stale CSE entries
            // that recorded `prev_dst == dst` from an earlier computation.
            Insn::Move(dst, _) => Some(*dst),
            Insn::Unpack(base, _, n) => {
                // Handled separately below; use sentinel None here.
                let _ = (base, n);
                None
            }
            _ => writable_dst(&insn),
        };

        // Evict stale entries: any entry whose *output* register is being
        // overwritten is no longer valid.  Also evict entries whose *input*
        // register is being overwritten (their computed value is now stale).
        // We do this BEFORE the CSE match check so the new entry (if any) is
        // not immediately invalidated by its own write.
        if let Insn::Unpack(base, _, n) = &insn {
            let lo = *base;
            let hi = base + n;
            table.retain(|k, prev_dst| {
                if *prev_dst >= lo && *prev_dst < hi {
                    return false;
                }
                match k {
                    CseKey::LoadConst(_) => true,
                    CseKey::BinOpConst(src, _, _) => *src < lo || *src >= hi,
                    CseKey::UnaryOp(_, src) => *src < lo || *src >= hi,
                }
            });
        } else if let Some(w) = written_reg {
            table.retain(|k, prev_dst| {
                if *prev_dst == w {
                    return false;
                }
                match k {
                    CseKey::LoadConst(_) => true,
                    CseKey::BinOpConst(src, _, _) => *src != w,
                    CseKey::UnaryOp(_, src) => *src != w,
                }
            });
        }

        // Check for a previous matching computation.
        let replaced = if let Some((ref k, dst)) = key {
            if let Some(&prev_dst) = table.get(k) {
                if prev_dst != dst {
                    // Replace this instruction with a register copy from the
                    // earlier result.  The original instruction is discarded.
                    result.push(Insn::CopyReg(dst, prev_dst));
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        if !replaced {
            // Record the expression in the CSE table.  Eviction already happened
            // above before the match check, so the new entry will not be removed.
            if let Some((k, dst)) = key {
                table.insert(k, dst);
            }

            result.push(insn);
        }

        // After a basic-block-terminating instruction, clear the table so the
        // next block starts fresh.  (We also clear at the *start* of targets via
        // is_bb_start, but this handles the fall-through path of conditionals.)
        let is_terminator = matches!(
            result.last().unwrap(),
            Insn::Jump(_)
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
        );
        if is_terminator {
            table.clear();
        }
    }

    result
}

// ─── Induction variable strength reduction ─────────────────────────────────────

/// Replace `BinOpConst(r_dst, r_iv, Mul, c_K)` inside a `ForCountConst` loop body
/// with a running accumulator, turning a multiply-per-iteration into an add-per-iteration.
///
/// ## Pattern (preconditions)
///
/// - Loop is `ForCountConst(iv, Lt, stop_c, step_c, k_exit)` with `consts[step_c] == 1`.
/// - The instruction immediately before the loop header is `LoadConst(iv, c_pre)` where
///   `consts[c_pre]` is an integer (the pre-loop value `start − step`).
/// - The loop body `[h+1, latch)` contains exactly one `BinOpConst(r_dst, iv, Mul, c_K)`.
/// - `r_dst != iv` (no clobbering the induction variable).
/// - No jump in the body targets the loop header `h` (no `continue` jumps back mid-body
///   and skipping the accumulator increment).
///
/// ## Transformation
///
/// ```text
/// // Before
/// LoadConst(iv, c_pre)                             // iv = start − 1
/// ForCountConst(iv, Lt, stop_c, c_1, k_exit)
///     BinOpConst(r_dst, iv, Mul, c_K)
///     …
/// Jump(k_back)
///
/// // After (two instructions inserted; offsets rewritten)
/// LoadConst(iv, c_pre)
/// LoadConst(r_acc, c_init)                         // r_acc = (start−1)*K  (NEW)
/// ForCountConst(iv, Lt, stop_c, c_1, k_exit)
///     Move(r_dst, r_acc)                           // replaced
///     …
///     BinOpConst(r_acc, r_acc, Add, c_K)           // r_acc += K  (NEW)
/// Jump(k_back)
/// ```
///
/// Only one `BinOpConst` per loop is strength-reduced per invocation.  For multiple
/// patterns in the same loop, run the optimizer a second time (handled by
/// `optimize_fn_code` running the full pipeline once).
fn pass_ivsr(insns: Vec<Insn>, consts: &mut Vec<Value>, num_regs: &mut u32) -> Vec<Insn> {
    use crate::ast::BinaryOp;

    let n = insns.len();
    if n < 3 {
        return insns;
    }

    for h in 0..n {
        // Must be ForCountConst with Lt and step=1
        let (iv, step_c) = match &insns[h] {
            Insn::ForCountConst(v, BinaryOp::Lt, _, sc, _) => (*v, *sc),
            _ => continue,
        };
        let step_int = match consts.get(step_c as usize) {
            Some(v) => match v.kind() {
                ValueKind::Int(1) => 1i64,
                _ => continue,
            },
            _ => continue,
        };
        let _ = step_int; // always 1; kept for readability

        // The instruction before the header must initialise iv: LoadConst(iv, c_pre)
        if h == 0 {
            continue;
        }
        let iv_init_val = match &insns[h - 1] {
            Insn::LoadConst(r, c) if *r == iv => match consts.get(*c as usize) {
                Some(v) => match v.kind() {
                    ValueKind::Int(i) => i,
                    _ => continue,
                },
                None => continue,
            },
            _ => continue,
        };

        // Find the back-edge: a Jump targeting header h
        let latch = match (h + 1..n).find(|&l| {
            if let Insn::Jump(k) = &insns[l] {
                (l as i64 + 1 + *k as i64) as usize == h
            } else {
                false
            }
        }) {
            Some(l) => l,
            None => continue,
        };

        // Safety: no jump inside the body targets the header (no continue-to-header)
        let has_continue = (h + 1..latch).any(|i| {
            let target = match &insns[i] {
                Insn::Jump(k)
                | Insn::JumpIfFalse(_, k)
                | Insn::JumpIfTrue(_, k)
                | Insn::CmpJumpIfFalse(_, _, _, k)
                | Insn::CmpJumpIfTrue(_, _, _, k)
                | Insn::CmpJumpIfFalseConst(_, _, _, k)
                | Insn::CmpJumpIfTrueConst(_, _, _, k) => Some((i as i64 + 1 + *k as i64) as usize),
                _ => None,
            };
            target == Some(h)
        });
        if has_continue {
            continue;
        }

        // Skip loops with exception handling
        if (h + 1..latch).any(|i| matches!(insns[i], Insn::SetupExcept(_) | Insn::PopExcept)) {
            continue;
        }

        // Find the first BinOpConst(r_dst, iv, Mul, c_K) in the body
        let (b, r_dst, c_k) = match (h + 1..latch).find_map(|i| match &insns[i] {
            Insn::BinOpConst(dst, src, BinaryOp::Mul, ck) if *src == iv && *dst != iv => {
                Some((i, *dst, *ck))
            }
            _ => None,
        }) {
            Some(t) => t,
            None => continue,
        };

        let k_val = match consts.get(c_k as usize) {
            Some(v) => match v.kind() {
                ValueKind::Int(k) => k,
                _ => continue,
            },
            None => continue,
        };

        // ForCountConst increments iv BEFORE the body runs, so the first body
        // execution sees iv = iv_init_val + 1 (= range start).  The accumulator
        // must equal that value * K on entry to the body, not iv_init_val * K.
        let acc_init = (iv_init_val + 1) * k_val;
        let c_acc_init = {
            if let Some(idx) = consts
                .iter()
                .position(|v| matches!(v.kind(), ValueKind::Int(i) if i == acc_init))
            {
                idx as u16
            } else {
                let idx = consts.len() as u16;
                consts.push(Value::int(acc_init));
                idx
            }
        };

        // Allocate a fresh accumulator register
        let r_acc = *num_regs;
        *num_regs += 1;

        // Build old→new position map:
        //   [0, h)          : unchanged
        //   [h, latch)      : +1  (LoadConst inserted before h)
        //   [latch, n]      : +2  (LoadConst before h AND BinOpConst before latch)
        let old_to_new: Vec<usize> = (0..=n)
            .map(|i| {
                if i < h {
                    i
                } else if i < latch {
                    i + 1
                } else {
                    i + 2
                }
            })
            .collect();

        // Rebuild the instruction list
        let mut new_insns: Vec<Insn> = Vec::with_capacity(n + 2);
        for i in 0..n {
            // Insert accumulator initialisation before the loop header
            if i == h {
                new_insns.push(Insn::LoadConst(r_acc, c_acc_init));
            }
            // Insert accumulator increment before the back-edge jump
            if i == latch {
                new_insns.push(Insn::BinOpConst(r_acc, r_acc, BinaryOp::Add, c_k));
            }
            // Replace the multiplication or rewrite offsets for everything else
            let insn = if i == b {
                Insn::Move(r_dst, r_acc)
            } else {
                rewrite_offsets(insns[i].clone(), i, &old_to_new)
            };
            new_insns.push(insn);
        }

        return new_insns; // one reduction per pass invocation
    }

    insns
}

// ─── Trivial no-op removal ─────────────────────────────────────────────────────

// ─── Copy propagation ─────────────────────────────────────────────────────────

/// Eliminate `Move(dst, src)` instructions by substituting `src` for all reads
/// of `dst` within the same basic block.
///
/// Algorithm (forward dataflow within basic blocks):
/// 1. Maintain a `copies` map: `dst → canonical_src`.
/// 2. At each jump target (instruction reachable from >1 predecessor), clear
///    `copies` — we cannot guarantee what was in `src` on all incoming paths.
/// 3. For each instruction: substitute reads of any key in `copies` with the
///    canonical source, kill entries whose key or value is overwritten, and
///    record new `Move(dst, src)` pairs.
///
/// After substitution, `Move(r, r)` becomes trivial and is removed by the
/// subsequent `pass_trivial_nop`.
///
/// Reference: GCC `-ftree-copy-prop`; Shi/Gregg/Beatty/Ertl *VEE'05*.
fn pass_copy_prop(insns: Vec<Insn>) -> Vec<Insn> {
    use std::collections::HashMap;

    let n = insns.len();
    if n == 0 {
        return insns;
    }

    // Step 1: mark all jump target indices so we can reset copies there.
    let mut is_target = vec![false; n + 1];
    is_target[0] = true; // entry point is always a target
    for (i, insn) in insns.iter().enumerate() {
        let offset: Option<i32> = match insn {
            Insn::Jump(k)
            | Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::ForIter(_, _, k)
            | Insn::ForCountReg(_, _, _, _, k)
            | Insn::ForCountConst(_, _, _, _, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k) => Some(*k),
            _ => None,
        };
        if let Some(k) = offset {
            let target = (i as i64 + 1 + k as i64) as usize;
            if target <= n {
                is_target[target] = true;
            }
        }
    }

    // Step 2: forward pass.
    let s = |copies: &HashMap<u32, u32>, r: u32| -> u32 { *copies.get(&r).unwrap_or(&r) };

    let mut copies: HashMap<u32, u32> = HashMap::new();
    let mut result: Vec<Insn> = Vec::with_capacity(n);

    for (i, insn) in insns.into_iter().enumerate() {
        if is_target[i] {
            copies.clear();
        }

        // Substitute source registers and collect the (possibly modified) instruction.
        let insn = match insn {
            Insn::Move(dst, src) => Insn::Move(dst, s(&copies, src)),
            // CopyReg: substitute the source register (may itself be an alias) but do
            // NOT record a new copy-propagation alias — downstream passes should see
            // CopyReg as an opaque assignment, not a transparent rename.
            Insn::CopyReg(dst, src) => Insn::CopyReg(dst, s(&copies, src)),
            Insn::Return(src) => Insn::Return(s(&copies, src)),
            Insn::PrintExpr(v) => Insn::PrintExpr(s(&copies, v)),
            Insn::RaiseValue(v) => Insn::RaiseValue(s(&copies, v)),
            Insn::RaiseAssert(v) => Insn::RaiseAssert(s(&copies, v)),
            Insn::RaiseFrom(exc, cause) => Insn::RaiseFrom(s(&copies, exc), s(&copies, cause)),
            Insn::JumpIfFalse(cond, k) => Insn::JumpIfFalse(s(&copies, cond), k),
            Insn::JumpIfTrue(cond, k) => Insn::JumpIfTrue(s(&copies, cond), k),
            Insn::UnaryOp(dst, op, src) => Insn::UnaryOp(dst, op, s(&copies, src)),
            Insn::BinOp(dst, lhs, op, rhs) => {
                Insn::BinOp(dst, s(&copies, lhs), op, s(&copies, rhs))
            }
            Insn::BinOpInPlace(dst, lhs, op, rhs) => {
                Insn::BinOpInPlace(dst, s(&copies, lhs), op, s(&copies, rhs))
            }
            Insn::BinOpConst(dst, lhs, op, c) => Insn::BinOpConst(dst, s(&copies, lhs), op, c),
            Insn::CmpJumpIfFalse(lhs, op, rhs, k) => {
                Insn::CmpJumpIfFalse(s(&copies, lhs), op, s(&copies, rhs), k)
            }
            Insn::CmpJumpIfTrue(lhs, op, rhs, k) => {
                Insn::CmpJumpIfTrue(s(&copies, lhs), op, s(&copies, rhs), k)
            }
            Insn::CmpJumpIfFalseConst(lhs, op, c, k) => {
                Insn::CmpJumpIfFalseConst(s(&copies, lhs), op, c, k)
            }
            Insn::CmpJumpIfTrueConst(lhs, op, c, k) => {
                Insn::CmpJumpIfTrueConst(s(&copies, lhs), op, c, k)
            }
            // In-place mutation instructions: substitute only the VALUE arg, not the
            // container/receiver — substituting the receiver would redirect the
            // mutation to the original allocation (copy propagation is only valid for
            // reads; deep-copied containers are independent allocations).
            Insn::SetAdd(st, val) => Insn::SetAdd(st, s(&copies, val)),
            Insn::ListAppend(lst, val) => Insn::ListAppend(lst, s(&copies, val)),
            Insn::ListExtend(lst, src) => Insn::ListExtend(lst, s(&copies, src)),
            Insn::DictUpdate(dct, other) => Insn::DictUpdate(dct, s(&copies, other)),
            Insn::SetAttr(obj, n, val) => Insn::SetAttr(obj, n, s(&copies, val)),
            Insn::DeleteAttr(obj, n) => Insn::DeleteAttr(obj, n),
            Insn::SetItem(obj, idx, val) => Insn::SetItem(obj, s(&copies, idx), s(&copies, val)),
            Insn::DeleteItem(obj, idx) => Insn::DeleteItem(obj, s(&copies, idx)),
            Insn::GetAttr(dst, obj, n) => Insn::GetAttr(dst, s(&copies, obj), n),
            Insn::GetItem(dst, obj, idx) => Insn::GetItem(dst, s(&copies, obj), s(&copies, idx)),
            Insn::GetIter(slot, src) => Insn::GetIter(slot, s(&copies, src)),
            Insn::Unpack(dst, src, n) => Insn::Unpack(dst, s(&copies, src), n),
            Insn::UnpackEx {
                src,
                before,
                after,
                dst_base,
            } => Insn::UnpackEx {
                src: s(&copies, src),
                before,
                after,
                dst_base,
            },
            Insn::CheckLocal(r, n) => Insn::CheckLocal(s(&copies, r), n),
            Insn::MatchExcept(r, k) => Insn::MatchExcept(s(&copies, r), k),
            Insn::ForCountReg(var, op, stop, step_idx, k) => {
                Insn::ForCountReg(var, op, s(&copies, stop), step_idx, k)
            }
            Insn::StoreGlobal(n, src) => Insn::StoreGlobal(n, s(&copies, src)),
            // Call/BuildList/BuildTuple/etc. use a base register for a range of args;
            // do not substitute the base register as that would misalign the arg block.
            other => other,
        };

        // Kill map entries: any key or value that == dst is stale after a write.
        if let Some(dst) = writable_dst(&insn) {
            copies.retain(|k, v| *k != dst && *v != dst);
        }
        // LoadConst writes dst (not in writable_dst so handled here).
        if let Insn::LoadConst(dst, _) = &insn {
            copies.retain(|k, v| *k != *dst && *v != *dst);
        }
        // Unpack writes dst..dst+n; kill the entire range.
        if let Insn::Unpack(dst, _, n) = &insn {
            let lo = *dst;
            let hi = dst + n;
            copies.retain(|k, v| (*k < lo || *k >= hi) && (*v < lo || *v >= hi));
        }
        // UnpackEx writes dst_base..dst_base+before+1+after; kill the entire range.
        if let Insn::UnpackEx {
            before,
            after,
            dst_base,
            ..
        } = &insn
        {
            let lo = *dst_base;
            let hi = dst_base + *before as u32 + 1 + *after as u32;
            copies.retain(|k, v| (*k < lo || *k >= hi) && (*v < lo || *v >= hi));
        }
        // Move(dst, src): kill stale aliases THEN record the new copy.
        // Killing is necessary because overwriting `dst` invalidates any
        // existing alias that names `dst` as its source (e.g. `x → dst`).
        if let Insn::Move(dst, src) = &insn {
            copies.retain(|k, v| *k != *dst && *v != *dst);
            let canonical = *copies.get(src).unwrap_or(src);
            if dst != &canonical {
                copies.insert(*dst, canonical);
            }
        }

        result.push(insn);
    }
    result
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

// ─── Constant pool compaction ──────────────────────────────────────────────────

/// Remove unreferenced entries from `consts` and renumber all constant indices
/// in `insns`.
///
/// After other passes fold or dead-code-eliminate instructions, some constant
/// pool entries become unreferenced (no instruction uses their index).  Leaving
/// them in the pool wastes memory and increases the cost of `Rc<FnCode>` clones.
///
/// ## Algorithm
///
/// 1. **Scan**: walk all instructions collecting every `u16` constant index that
///    is actually referenced.
/// 2. **Remap**: build `old_to_new: Vec<Option<u16>>` for the current pool size.
///    Referenced entries get compact new indices; unreferenced entries get `None`.
/// 3. **Compact**: rebuild `consts` retaining only referenced values.
/// 4. **Rewrite**: replace every constant index in `insns` using `old_to_new`.
///
/// ## Instruction fields that carry constant indices
///
/// `LoadConst`, `BinOpConst`, `CmpJumpIfFalseConst`, `CmpJumpIfTrueConst`,
/// `ForCountConst` (stop and step), `ForCountReg` (step only).
///
/// Reference: CPython `flowgraph.c` `remove_unused_consts()`.
fn pass_compact_consts(insns: Vec<Insn>, consts: Vec<Value>) -> (Vec<Insn>, Vec<Value>) {
    let old_len = consts.len();
    if old_len == 0 {
        return (insns, consts);
    }

    // Step 1: collect referenced indices.
    let mut used = vec![false; old_len];
    let mark = |used: &mut Vec<bool>, idx: u16| {
        if (idx as usize) < used.len() {
            used[idx as usize] = true;
        }
    };
    for insn in &insns {
        match insn {
            Insn::LoadConst(_, c) => mark(&mut used, *c),
            Insn::BinOpConst(_, _, _, c) => mark(&mut used, *c),
            Insn::CmpJumpIfFalseConst(_, _, c, _) => mark(&mut used, *c),
            Insn::CmpJumpIfTrueConst(_, _, c, _) => mark(&mut used, *c),
            Insn::ForCountConst(_, _, stop, step, _) => {
                mark(&mut used, *stop);
                mark(&mut used, *step);
            }
            Insn::ForCountReg(_, _, _, step, _) => mark(&mut used, *step),
            _ => {}
        }
    }

    // Early exit if every entry is still referenced.
    if used.iter().all(|&u| u) {
        return (insns, consts);
    }

    // Step 2: build remap table.
    let mut old_to_new: Vec<Option<u16>> = vec![None; old_len];
    let mut new_consts: Vec<Value> = Vec::with_capacity(used.iter().filter(|&&u| u).count());
    for (old_idx, val) in consts.into_iter().enumerate() {
        if used[old_idx] {
            old_to_new[old_idx] = Some(new_consts.len() as u16);
            new_consts.push(val);
        }
    }

    // Step 3: rewrite constant indices in instructions.
    let remap = |c: u16| old_to_new[c as usize].expect("referenced const must have new index");
    let new_insns = insns
        .into_iter()
        .map(|insn| match insn {
            Insn::LoadConst(r, c) => Insn::LoadConst(r, remap(c)),
            Insn::BinOpConst(d, l, op, c) => Insn::BinOpConst(d, l, op, remap(c)),
            Insn::CmpJumpIfFalseConst(r, op, c, k) => Insn::CmpJumpIfFalseConst(r, op, remap(c), k),
            Insn::CmpJumpIfTrueConst(r, op, c, k) => Insn::CmpJumpIfTrueConst(r, op, remap(c), k),
            Insn::ForCountConst(v, op, stop, step, k) => {
                Insn::ForCountConst(v, op, remap(stop), remap(step), k)
            }
            Insn::ForCountReg(v, op, stop, step, k) => {
                Insn::ForCountReg(v, op, stop, remap(step), k)
            }
            other => other,
        })
        .collect();

    (new_insns, new_consts)
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

// ─── Self-tail-call optimisation ──────────────────────────────────────────────

/// Replace `Call(r, n) + Return(r)` pairs with `TailCall { args_base: r+1, nargs: n }`.
///
/// ## What this enables
///
/// When the VM encounters `TailCall`, it checks whether the callee is the same
/// function that is currently executing.  If it is, it resets the parameter
/// registers in the current frame and jumps to pc=0 instead of allocating a new
/// stack frame, turning O(n) stack growth into O(1).
///
/// If the callee turns out to be a *different* function at runtime (e.g. the name
/// was rebound), the VM falls back to a normal call+return, so correctness is
/// preserved in all cases.
///
/// ## Pattern
///
/// ```text
/// Call(r, n)    ← result lands in r
/// Return(r)     ← immediately returned
/// ```
/// →
/// ```text
/// TailCall { args_base: r + 1, nargs: n }
/// ```
///
/// The args to the call are in `R[r+1 .. r+1+n]` (per the `Call` convention);
/// `TailCall` stores only `args_base` and `nargs` — the function register `r`
/// itself is not needed because the VM already knows the current function.
///
/// ## Guards
///
/// - The pair must be adjacent (no instructions between `Call` and `Return`).
/// - The `Return` must return exactly the register that `Call` wrote (`func_reg`).
/// - Generators are excluded: a generator frame cannot be "restarted" in the same
///   way (but the is_generator flag is not available here, so we rely on the VM's
///   generator guard).
fn pass_self_tail_call(insns: Vec<Insn>) -> Vec<Insn> {
    let n = insns.len();
    if n < 2 {
        return insns;
    }
    let mut transformed = insns;
    let mut keep = vec![true; n];

    let mut i = 0;
    while i + 1 < n {
        let replace: Option<Insn> = match (&transformed[i], &transformed[i + 1]) {
            // Match both Call and CallMemo — pure functions use CallMemo but are
            // still valid candidates for self-tail-call optimisation.
            (
                &Insn::Call(func_reg, nargs) | &Insn::CallMemo(func_reg, nargs),
                &Insn::Return(ret_reg),
            ) if func_reg == ret_reg => Some(Insn::TailCall {
                args_base: func_reg + 1,
                nargs,
            }),
            _ => None,
        };
        if let Some(tail_insn) = replace {
            // Replace Call/CallMemo with TailCall, drop the Return.
            transformed[i] = tail_insn;
            keep[i + 1] = false;
            i += 2;
        } else {
            i += 1;
        }
    }
    compact(transformed, &keep)
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
        // dst == lc_reg: the result overwrites lc_reg, so lc_reg is not live after
        // the BinOp — fusion is safe and should happen.
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::BinOp(5, 0, BinaryOp::Add, 5), // dst == rhs == lc_reg
            Insn::Return(5),
        ];
        let out = pass_binop_const_fusion(insns, 2);
        assert_eq!(
            out.len(),
            2,
            "fusion is safe when dst == lc_reg (result overwrites it)"
        );
        assert!(
            matches!(out[0], Insn::BinOpConst(5, 0, BinaryOp::Add, 0)),
            "should fuse to BinOpConst"
        );
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
        assert!(
            has_binopconst,
            "optimizer should fuse LoadConst+BinOp into BinOpConst for n+5"
        );
    }

    #[test]
    fn binop_const_fusion_commutative_lhs_const() {
        use crate::ast::BinaryOp;
        // LoadConst(r=5, c=0)  BinOp(dst=1, lhs=5, Add, rhs=0)  — const on LEFT
        // Even though Add is commutative, swapping would break __radd__ dispatch,
        // so the optimization must be skipped and all 3 instructions kept.
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::BinOp(1, 5, BinaryOp::Add, 0),
            Insn::Return(1),
        ];
        let out = pass_binop_const_fusion(insns, 2);
        assert_eq!(
            out.len(),
            3,
            "const-lhs Add should NOT be fused (would break __radd__ dispatch)"
        );
    }

    #[test]
    fn binop_const_fusion_does_not_commute_non_commutative() {
        use crate::ast::BinaryOp;
        // Sub is not commutative — should not fuse when const is on left
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::BinOp(1, 5, BinaryOp::Sub, 0),
            Insn::Return(1),
        ];
        let out = pass_binop_const_fusion(insns, 2);
        assert_eq!(
            out.len(),
            3,
            "non-commutative op with const-lhs should not fuse"
        );
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
        let idx = match out[0] {
            Insn::LoadConst(_, i) => i,
            _ => panic!(),
        };
        assert!(matches!(
            consts[idx as usize].kind(),
            crate::value::ValueKind::Int(3)
        ));
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
        let idx = match out[0] {
            Insn::LoadConst(_, i) => i,
            _ => panic!(),
        };
        assert!(matches!(
            consts[idx as usize].kind(),
            crate::value::ValueKind::Bool(false)
        ));
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
        let idx = match out[0] {
            Insn::LoadConst(_, i) => i,
            _ => panic!(),
        };
        assert!(matches!(
            consts[idx as usize].kind(),
            crate::value::ValueKind::Int(1)
        ));
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
            Insn::LoadConst(0, 0),                    // r0 = 5
            Insn::BinOpConst(1, 0, BinaryOp::Add, 1), // r1 = r0 + 3
            Insn::Return(1),
        ];
        let out = pass_const_fold(insns, &mut consts);
        assert!(
            matches!(out[1], Insn::LoadConst(1, _)),
            "BinOpConst with known lhs should be folded to LoadConst"
        );
        let folded_idx = match out[1] {
            Insn::LoadConst(_, i) => i,
            _ => panic!(),
        };
        assert!(matches!(
            consts[folded_idx as usize].kind(),
            crate::value::ValueKind::Int(8)
        ));
    }

    #[test]
    fn const_fold_binop_with_both_known() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        let mut consts = vec![Value::int(10), Value::int(2)];
        let insns = vec![
            Insn::LoadConst(0, 0),               // r0 = 10
            Insn::LoadConst(1, 1),               // r1 = 2
            Insn::BinOp(2, 0, BinaryOp::Mul, 1), // r2 = r0 * r1
            Insn::Return(2),
        ];
        let out = pass_const_fold(insns, &mut consts);
        assert!(
            matches!(out[2], Insn::LoadConst(2, _)),
            "BinOp with both operands known should fold to LoadConst"
        );
        let idx = match out[2] {
            Insn::LoadConst(_, i) => i,
            _ => panic!(),
        };
        assert!(matches!(
            consts[idx as usize].kind(),
            crate::value::ValueKind::Int(20)
        ));
    }

    #[test]
    fn const_fold_propagates_through_move() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // LoadConst(t, idx_5)  Move(x, t)  BinOpConst(y, x, Add, idx_3)
        // After propagation: known[x]=idx_5, fold BinOpConst to LoadConst(y, idx_8)
        let mut consts = vec![Value::int(5), Value::int(3)];
        let insns = vec![
            Insn::LoadConst(5, 0),                    // temp=5 (reg 5)
            Insn::Move(0, 5),                         // x = temp
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
        assert!(
            has_10,
            "constant 10 should appear in pool after folding x*2 with x=5"
        );
    }

    #[test]
    fn const_fold_does_not_fold_loop_condition() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // Simulates: y = 3; while y > 0: y = y - 1
        //
        //  [0] LoadConst(0, 0)              consts[0] = 3   (y_reg = 0)
        //  [1] BinOpConst(1, 0, Gt, 1)      consts[1] = 0   (loop header — target of Jump at [4])
        //  [2] JumpIfFalse(1, 2)                             (exit: target = 2+1+2 = 5)
        //  [3] BinOpConst(0, 0, Sub, 2)     consts[2] = 1
        //  [4] Jump(-4)                                      (back to [1]: 4+1-4 = 1)
        //  [5] Return(0)
        let mut consts = vec![Value::int(3), Value::int(0), Value::int(1)];
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::BinOpConst(1, 0, BinaryOp::Gt, 1),
            Insn::JumpIfFalse(1, 2),
            Insn::BinOpConst(0, 0, BinaryOp::Sub, 2),
            Insn::Jump(-4),
            Insn::Return(0),
        ];
        let out = pass_const_fold(insns, &mut consts);
        // [1] must NOT fold to LoadConst(True) — the loop would become infinite.
        assert!(
            matches!(out[1], Insn::BinOpConst(1, 0, BinaryOp::Gt, 1)),
            "loop condition must not be folded; known map must clear at loop header"
        );
    }

    // ── pass_const_branch_elim ────────────────────────────────────────────────

    #[test]
    fn const_branch_elim_jumpiffalse_truthy_becomes_jump0() {
        use crate::value::Value;
        // LoadConst(r=0, c=0) [consts[0]=True]  JumpIfFalse(0, 5)
        // Truthy → never jumps → replace with Jump(0)
        let consts = vec![Value::bool_(true)];
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::JumpIfFalse(0, 5),
            Insn::Return(0),
        ];
        let out = pass_const_branch_elim(insns, &consts);
        assert!(
            matches!(out[1], Insn::Jump(0)),
            "truthy JumpIfFalse → Jump(0)"
        );
    }

    #[test]
    fn const_branch_elim_jumpiffalse_falsy_becomes_jump_k() {
        use crate::value::Value;
        // LoadConst(r=0, c=0) [consts[0]=False]  JumpIfFalse(0, 3)
        // Falsy → always jumps → replace with Jump(3)
        let consts = vec![Value::bool_(false)];
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::JumpIfFalse(0, 3),
            Insn::Return(0),
        ];
        let out = pass_const_branch_elim(insns, &consts);
        assert!(
            matches!(out[1], Insn::Jump(3)),
            "falsy JumpIfFalse → Jump(k)"
        );
    }

    #[test]
    fn const_branch_elim_eliminates_dead_branch_on_compiled_code() {
        // "if True: print(1)\nelse: print(2)" — the else branch should be dead
        let code = compile_fn("if True:\n    print(1)\nelse:\n    print(2)\n");
        let optimized = optimize(code);
        // After optimization, the instruction stream should not contain the
        // dead-branch code that prints 2. Check by verifying only one integer
        // constant (1) is referenced, not 2.
        // Note: the constant 2 may still exist in the pool even if the code
        // referencing it is dead — but the dead code itself should be gone.
        // Instead check that no LoadConst referencing 2 appears in reachable insns.
        // Simpler: check the insn list has no BinOp/conditional jumps (the if collapsed).
        let has_cond_jump = optimized.insns.iter().any(|i| {
            matches!(
                i,
                Insn::JumpIfFalse(..)
                    | Insn::JumpIfTrue(..)
                    | Insn::CmpJumpIfFalse(..)
                    | Insn::CmpJumpIfFalseConst(..)
                    | Insn::CmpJumpIfTrue(..)
                    | Insn::CmpJumpIfTrueConst(..)
            )
        });
        assert!(
            !has_cond_jump,
            "constant-condition if should have no conditional jumps"
        );
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
        let code = compile_fn("def f(x):\n    if x > 3:\n        print(x)\n");
        let optimized = optimize(code);
        let inner = &optimized.fn_protos[0].code;
        let has_cmpjump = inner.insns.iter().any(|i| {
            matches!(
                i,
                Insn::CmpJumpIfFalse(..)
                    | Insn::CmpJumpIfTrue(..)
                    | Insn::CmpJumpIfFalseConst(..)
                    | Insn::CmpJumpIfTrueConst(..)
            )
        });
        assert!(
            has_cmpjump,
            "optimizer should fuse comparison into conditional jump"
        );
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

    // ── pass_compact_consts ───────────────────────────────────────────────────

    #[test]
    fn compact_consts_removes_unreferenced_entry() {
        use crate::value::Value;
        // Pool: [10, 99, 20].  Only consts 0 and 2 are referenced.
        // Expected pool after compaction: [10, 20]; indices rewritten.
        let consts = vec![Value::int(10), Value::int(99), Value::int(20)];
        let insns = vec![
            Insn::LoadConst(0, 0), // references pool[0] = 10
            Insn::LoadConst(1, 2), // references pool[2] = 20
            Insn::Return(0),
        ];
        let (out_insns, out_consts) = pass_compact_consts(insns, consts);
        assert_eq!(out_consts.len(), 2, "unreferenced entry should be removed");
        assert!(matches!(
            out_consts[0].kind(),
            crate::value::ValueKind::Int(10)
        ));
        assert!(matches!(
            out_consts[1].kind(),
            crate::value::ValueKind::Int(20)
        ));
        // LoadConst(1, 2) should be rewritten to LoadConst(1, 1)
        assert!(matches!(out_insns[1], Insn::LoadConst(1, 1)));
    }

    #[test]
    fn compact_consts_noop_when_all_referenced() {
        use crate::value::Value;
        let consts = vec![Value::int(1), Value::int(2)];
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::LoadConst(1, 1),
            Insn::Return(0),
        ];
        let (out_insns, out_consts) = pass_compact_consts(insns, consts);
        assert_eq!(out_consts.len(), 2, "no change when all referenced");
        assert!(matches!(out_insns[0], Insn::LoadConst(0, 0)));
        assert!(matches!(out_insns[1], Insn::LoadConst(1, 1)));
    }

    #[test]
    fn compact_consts_on_compiled_dead_branch() {
        // "if True: x=1\nelse: x=2" — the else branch is dead.
        // pass_const_fold+pass_dead_code should eliminate the else body.
        // pass_compact_consts should then remove the orphaned constant 2 from the pool.
        let code = compile_fn("if True:\n    x = 1\nelse:\n    x = 2\n");
        let optimized = optimize(code);
        // After optimization, the constant 2 should not appear in the pool
        // (the dead branch referencing it was removed, then the pool was compacted).
        let has_2 = optimized
            .consts
            .iter()
            .any(|v| matches!(v.kind(), crate::value::ValueKind::Int(2)));
        assert!(
            !has_2,
            "orphaned constant 2 should be removed by pool compaction"
        );
    }

    // ── pass_not_invert ───────────────────────────────────────────────────────

    #[test]
    fn not_invert_jumpiffalse_becomes_jumpiftrue() {
        use crate::ast::UnaryOp;
        // [0] UnaryOp(r=5, Not, src=0)   keep=false
        // [1] JumpIfFalse(5, k=1)         target = 1+1+1 = 3 (past-end sentinel)
        // [2] Return(0)
        // After fusion: [0] JumpIfTrue(0, 1)  [1] Return(0)
        // Offset rewrite: old_target=3, to_new[3]=2, new_src=to_new[1]=0 → k=2-0-1=1
        let insns = vec![
            Insn::UnaryOp(5, UnaryOp::Not, 0),
            Insn::JumpIfFalse(5, 1),
            Insn::Return(0),
        ];
        let out = pass_not_invert(insns, 2);
        assert_eq!(out.len(), 2, "UnaryOp should be removed");
        assert!(
            matches!(out[0], Insn::JumpIfTrue(0, 1)),
            "JumpIfFalse(not x) should become JumpIfTrue(x)"
        );
    }

    #[test]
    fn not_invert_jumpiftrue_becomes_jumpiffalse() {
        use crate::ast::UnaryOp;
        // Same layout; k=1 → past-end target.
        let insns = vec![
            Insn::UnaryOp(5, UnaryOp::Not, 0),
            Insn::JumpIfTrue(5, 1),
            Insn::Return(0),
        ];
        let out = pass_not_invert(insns, 2);
        assert_eq!(out.len(), 2, "UnaryOp should be removed");
        assert!(
            matches!(out[0], Insn::JumpIfFalse(0, 1)),
            "JumpIfTrue(not x) should become JumpIfFalse(x)"
        );
    }

    #[test]
    fn not_invert_skips_when_reg_is_local() {
        use crate::ast::UnaryOp;
        // r=1 < num_locals=3 → must not fuse
        let insns = vec![
            Insn::UnaryOp(1, UnaryOp::Not, 0),
            Insn::JumpIfFalse(1, 1),
            Insn::Return(0),
        ];
        let out = pass_not_invert(insns, 3);
        assert_eq!(out.len(), 3, "no fusion when r is a local");
    }

    #[test]
    fn not_invert_skips_when_reg_read_after() {
        use crate::ast::UnaryOp;
        // r=5 is read again after the branch → must not fuse
        let insns = vec![
            Insn::UnaryOp(5, UnaryOp::Not, 0),
            Insn::JumpIfFalse(5, 0),
            Insn::Return(5), // reads r=5 → live
        ];
        let out = pass_not_invert(insns, 2);
        assert_eq!(out.len(), 3, "no fusion when r is live after branch");
    }

    #[test]
    fn not_invert_fuses_when_reg_not_reused() {
        use crate::ast::UnaryOp;
        // Build a case where the Not result register (r=5) is genuinely dead after
        // the branch: src=0 (x), result=5, jump target uses a different register.
        //
        // [0] UnaryOp(5, Not, 0)   r5 = not r0
        // [1] JumpIfFalse(5, 1)    if r5 false: jump past-end
        // [2] Move(2, 0)           r2 = r0  (r5 not read here)
        // [3] Return(2)
        let insns = vec![
            Insn::UnaryOp(5, UnaryOp::Not, 0),
            Insn::JumpIfFalse(5, 1),
            Insn::Move(2, 0),
            Insn::Return(2),
        ];
        let out = pass_not_invert(insns, 2);
        // UnaryOp should be removed; JumpIfFalse→JumpIfTrue
        assert_eq!(out.len(), 3, "UnaryOp should be removed");
        assert!(
            matches!(out[0], Insn::JumpIfTrue(0, _)),
            "JumpIfFalse(not r0) should become JumpIfTrue(r0)"
        );
    }

    // ── pass_binopinplace_downgrade ───────────────────────────────────────────

    #[test]
    fn binopinplace_downgrades_dead_temp_lhs() {
        use crate::ast::BinaryOp;
        // BinOpInPlace(dst=2, lhs=5, Add, rhs=1); r5 not read after → BinOp
        let insns = vec![Insn::BinOpInPlace(2, 5, BinaryOp::Add, 1), Insn::Return(2)];
        let out = pass_binopinplace_downgrade(insns, 2);
        assert!(
            matches!(out[0], Insn::BinOp(2, 5, BinaryOp::Add, 1)),
            "BinOpInPlace with dead temp lhs should become BinOp"
        );
    }

    #[test]
    fn binopinplace_skips_local_lhs() {
        use crate::ast::BinaryOp;
        // lhs=1 < num_locals=3 → user object may have __iadd__, must not downgrade
        let insns = vec![Insn::BinOpInPlace(2, 1, BinaryOp::Add, 0), Insn::Return(2)];
        let out = pass_binopinplace_downgrade(insns, 3);
        assert!(
            matches!(out[0], Insn::BinOpInPlace(2, 1, BinaryOp::Add, 0)),
            "BinOpInPlace with local lhs must not be downgraded"
        );
    }

    #[test]
    fn binopinplace_skips_live_lhs() {
        use crate::ast::BinaryOp;
        // lhs=5 is read after by Return(5) → live, must not downgrade
        let insns = vec![Insn::BinOpInPlace(2, 5, BinaryOp::Add, 1), Insn::Return(5)];
        let out = pass_binopinplace_downgrade(insns, 2);
        assert!(
            matches!(out[0], Insn::BinOpInPlace(2, 5, BinaryOp::Add, 1)),
            "BinOpInPlace with live lhs must not be downgraded"
        );
    }

    #[test]
    fn binopinplace_downgrades_dst_equals_lhs() {
        use crate::ast::BinaryOp;
        // dst == lhs: result lands in same register, always safe to downgrade
        let insns = vec![Insn::BinOpInPlace(5, 5, BinaryOp::Mul, 1), Insn::Return(5)];
        let out = pass_binopinplace_downgrade(insns, 2);
        assert!(
            matches!(out[0], Insn::BinOp(5, 5, BinaryOp::Mul, 1)),
            "BinOpInPlace(dst==lhs) should always downgrade to BinOp"
        );
    }

    // ── slice_has_back_edge ────────────────────────────────────────────────────

    #[test]
    fn back_edge_detected_on_negative_jump() {
        // Jump(-2) is a backward edge
        let insns = vec![Insn::Jump(-2)];
        assert!(slice_has_back_edge(&insns));
    }

    #[test]
    fn no_back_edge_in_forward_only_slice() {
        // All jumps are non-negative → no back-edge
        let insns = vec![Insn::JumpIfFalse(0, 1), Insn::Return(0)];
        assert!(!slice_has_back_edge(&insns));
    }

    #[test]
    fn binop_const_fusion_skips_when_back_edge_present() {
        use crate::ast::BinaryOp;
        use crate::value::Value;
        // LoadConst(r5, 0) + BinOp(r3, r2, Mul, r5) + ForIter(r6, 0, -2)
        // The ForIter has a negative offset → back-edge; fusion must not remove LoadConst.
        let consts = vec![Value::int(4)];
        let insns = vec![
            Insn::LoadConst(5, 0),
            Insn::BinOp(3, 2, BinaryOp::Mul, 5),
            Insn::ForIter(6, 0, -2),
            Insn::Return(3),
        ];
        let out = pass_binop_const_fusion(insns, 2);
        // LoadConst must survive — r5 is live on the back-edge
        assert!(
            matches!(out[0], Insn::LoadConst(5, 0)),
            "LoadConst must not be removed when a back-edge is present"
        );
        let _ = consts; // suppress unused warning
    }

    // ── pass_copy_prop ─────────────────────────────────────────────────────────

    #[test]
    fn copy_prop_eliminates_move() {
        use crate::ast::BinaryOp;
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::Move(1, 0),
            Insn::BinOp(2, 1, BinaryOp::Add, 3),
            Insn::Return(2),
        ];
        let out = pass_copy_prop(insns);
        assert!(
            matches!(out[2], Insn::BinOp(2, 0, BinaryOp::Add, 3)),
            "r1 should be substituted with r0 in BinOp"
        );
    }

    #[test]
    fn copy_prop_kills_alias_on_move_overwrite() {
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::LoadConst(2, 1),
            Insn::Move(1, 0),
            Insn::Move(0, 2),
            Insn::Return(1),
        ];
        let out = pass_copy_prop(insns);
        assert!(
            matches!(out[4], Insn::Return(1)),
            "r1 alias must be killed when r0 is overwritten"
        );
    }

    #[test]
    fn copy_prop_kills_alias_on_binop_write() {
        use crate::ast::BinaryOp;
        let insns = vec![
            Insn::LoadConst(0, 0),
            Insn::LoadConst(2, 1),
            Insn::Move(1, 0),
            Insn::BinOp(0, 0, BinaryOp::Add, 2),
            Insn::Return(1),
        ];
        let out = pass_copy_prop(insns);
        assert!(
            matches!(out[4], Insn::Return(1)),
            "r1→r0 alias must be killed when BinOp writes r0"
        );
    }

    #[test]
    fn copy_prop_does_not_substitute_dict_update_receiver() {
        let insns = vec![
            Insn::BuildDict(2, 3, 0),
            Insn::Move(5, 2),
            Insn::BuildDict(4, 3, 0),
            Insn::Move(6, 4),
            Insn::DictUpdate(5, 6),
            Insn::Return(5),
        ];
        let out = pass_copy_prop(insns);
        assert!(
            matches!(out[4], Insn::DictUpdate(5, 4)),
            "DictUpdate: receiver unchanged, src substituted"
        );
    }

    // ── pass_fold_const_tuple ─────────────────────────────────────────────────

    #[test]
    fn fold_const_tuple_two_consts() {
        use crate::value::Value;
        let mut consts = vec![Value::int(10), Value::int(20)];
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::LoadConst(3, 1),
            Insn::BuildTuple(4, 2, 2),
            Insn::Return(4),
        ];
        let out = pass_fold_const_tuple(insns, 2, &mut consts);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Insn::LoadConst(4, _)));
        let new_idx = match out[0] {
            Insn::LoadConst(_, i) => i,
            _ => panic!("expected LoadConst"),
        };
        let elems = consts[new_idx as usize]
            .as_tuple()
            .expect("new constant should be a tuple");
        assert_eq!(elems.len(), 2);
        assert!(matches!(elems[0].kind(), crate::value::ValueKind::Int(10)));
        assert!(matches!(elems[1].kind(), crate::value::ValueKind::Int(20)));
    }

    #[test]
    fn fold_const_tuple_skips_local_regs() {
        use crate::value::Value;
        let mut consts = vec![Value::int(1), Value::int(2)];
        let insns = vec![
            Insn::LoadConst(1, 0),
            Insn::LoadConst(2, 1),
            Insn::BuildTuple(5, 1, 2),
            Insn::Return(5),
        ];
        let out = pass_fold_const_tuple(insns, 3, &mut consts);
        assert_eq!(
            out.len(),
            4,
            "should not fold when base register is a local"
        );
        assert!(matches!(out[2], Insn::BuildTuple(5, 1, 2)));
    }

    // ── pass_dead_store_elim ──────────────────────────────────────────────────

    #[test]
    fn dse_removes_overwritten_load_const() {
        // LoadConst(r2, 0) immediately overwritten by LoadConst(r2, 1); first is dead.
        let insns = vec![
            Insn::LoadConst(2, 0), // dead — r2 written again before any read
            Insn::LoadConst(2, 1), // live — r2 used by Return
            Insn::Return(2),
        ];
        let out = pass_dead_store_elim(insns, 2);
        assert_eq!(out.len(), 2, "dead LoadConst should be removed");
        assert!(matches!(out[0], Insn::LoadConst(2, 1)));
        assert!(matches!(out[1], Insn::Return(2)));
    }

    #[test]
    fn dse_keeps_store_that_is_read() {
        // LoadConst(r2, 0) is read by Return — must be kept.
        let insns = vec![Insn::LoadConst(2, 0), Insn::Return(2)];
        let out = pass_dead_store_elim(insns, 2);
        assert_eq!(out.len(), 2, "live LoadConst must not be removed");
    }

    #[test]
    fn dse_keeps_local_register_writes() {
        // Register r0 < num_locals=2 — locals must not be eliminated.
        let insns = vec![
            Insn::LoadConst(0, 0), // local reg — keep even if "dead"
            Insn::LoadConst(0, 1), // overwrites r0
            Insn::Return(0),
        ];
        let out = pass_dead_store_elim(insns, 2);
        assert_eq!(out.len(), 3, "local register writes must not be removed");
    }

    #[test]
    fn dse_skips_when_back_edge_present() {
        // LoadConst(r2, 0) followed by a loop back-edge — conservatively kept.
        let insns = vec![
            Insn::LoadConst(2, 0), // candidate, but back-edge below
            Insn::Jump(-1),        // back-edge (negative offset)
        ];
        let out = pass_dead_store_elim(insns, 2);
        assert_eq!(
            out.len(),
            2,
            "must not remove store when back-edge is present"
        );
    }

    #[test]
    fn dse_removes_dead_move() {
        use crate::ast::BinaryOp;
        // Move(r3, r2) followed immediately by BinOp(r3, ...) — Move is dead.
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::Move(3, 2),                    // dead — r3 overwritten below
            Insn::BinOp(3, 2, BinaryOp::Add, 2), // overwrites r3
            Insn::Return(3),
        ];
        let out = pass_dead_store_elim(insns, 2);
        assert_eq!(out.len(), 3, "dead Move should be removed");
        assert!(matches!(out[1], Insn::BinOp(3, 2, BinaryOp::Add, 2)));
    }

    // ── pass_exit_inline ─────────────────────────────────────────────────────

    #[test]
    fn exit_inline_jump_to_return() {
        // Jump(0) at index 0 targets index 1 (Return(r)) → replaced with Return(r).
        let insns = vec![Insn::Jump(0), Insn::Return(5)];
        let out = pass_exit_inline(insns);
        assert_eq!(out.len(), 2, "no instructions removed — only inlined");
        assert!(
            matches!(out[0], Insn::Return(5)),
            "Jump targeting Return should be replaced with Return"
        );
    }

    #[test]
    fn exit_inline_jump_to_return_none() {
        let insns = vec![Insn::Jump(1), Insn::LoadConst(0, 0), Insn::ReturnNone];
        let out = pass_exit_inline(insns);
        // Jump(1) at index 0 targets index 2 (ReturnNone)
        assert!(
            matches!(out[0], Insn::ReturnNone),
            "Jump targeting ReturnNone should be replaced with ReturnNone"
        );
    }

    #[test]
    fn exit_inline_skips_non_terminal_target() {
        use crate::ast::BinaryOp;
        // Jump(0) targets LoadConst — not a terminal, must not be replaced.
        let insns = vec![Insn::Jump(0), Insn::LoadConst(3, 0), Insn::Return(3)];
        let out = pass_exit_inline(insns);
        assert!(
            matches!(out[0], Insn::Jump(0)),
            "Jump to non-terminal should be kept as-is"
        );
        // Suppress unused-import lint
        let _ = BinaryOp::Add;
    }

    #[test]
    fn exit_inline_skips_conditional_jumps() {
        // JumpIfFalse is NOT an unconditional Jump — must not be modified.
        let insns = vec![Insn::JumpIfFalse(0, 0), Insn::Return(0)];
        let out = pass_exit_inline(insns);
        assert!(
            matches!(out[0], Insn::JumpIfFalse(0, 0)),
            "conditional jumps must not be inlined"
        );
    }

    // ── pass_licm ─────────────────────────────────────────────────────────────

    /// Build a minimal loop with a back edge:
    ///
    /// ```text
    /// [0]  LoadConst(r5, 0)         ← invariant: hoistable
    /// [1]  ForCountConst(r0, Lt, 0, 1, 2)   ← loop header (target of back edge at [3])
    ///                                         if counter exhausted: jump to [4]
    /// [2]  BinOp(r1, r1, Add, r0)   ← body: uses r5 indirectly via BinOp(not hoistable)
    /// [3]  Jump(-3)                 ← back edge → [1]
    /// [4]  Return(r1)
    /// ```
    ///
    /// After LICM, LoadConst(r5, 0) should appear before the loop header (at index 0
    /// in the pre-header block, which is before old index 1).
    #[test]
    fn licm_hoists_loadconst_before_loop() {
        use crate::ast::BinaryOp;

        // Layout (raw, before LICM):
        //  [0] LoadConst(r5, 0)           — invariant
        //  [1] ForCountConst(r0, Lt, 0, 1, 2) — header; jumps to [4] when done
        //  [2] BinOp(r1, r1, Add, r0)     — body
        //  [3] Jump(-3)                   — back edge → [1]
        //  [4] Return(r1)
        //
        // Back edge: Jump(-3) at old index 3 → target = 3+1-3 = 1 → header=1, latch=3
        // Write set for [1..=3]: {r0} (ForCountConst writes r0), {r1} (BinOp writes r1)
        // LoadConst(r5, 0) is at index 0 — OUTSIDE the loop [1..=3], so LICM does
        // nothing here since 0 < header=1.
        //
        // Adjusted test: put LoadConst INSIDE the loop body, so LICM moves it out.
        //
        //  [0] ForCountConst(r0, Lt, 0, 1, 3) — header; jumps to [4] when done
        //  [1] LoadConst(r5, 0)               — invariant (inside loop body)
        //  [2] BinOp(r1, r1, Add, r0)         — not invariant (r0 is written by header)
        //  [3] Jump(-4)                        — back edge → [0]
        //  [4] Return(r1)
        let insns = vec![
            Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, 3), // [0] header, exits to [4]
            Insn::LoadConst(5, 0),                         // [1] invariant
            Insn::BinOp(1, 1, BinaryOp::Add, 0),           // [2] not invariant
            Insn::Jump(-4),                                // [3] back edge → [0]
            Insn::Return(1),                               // [4]
        ];
        let out = pass_licm(insns);

        // After hoisting LoadConst(r5, 0) before header [0], the new layout is:
        //  [0] LoadConst(r5, 0)               — hoisted
        //  [1] ForCountConst(r0, Lt, 0, 1, 2) — header (offset adjusted: was 3, now 2)
        //  [2] BinOp(r1, r1, Add, r0)
        //  [3] Jump(-3)                        — back edge → [1]
        //  [4] Return(r1)
        assert_eq!(out.len(), 5, "instruction count must not change");
        assert!(
            matches!(out[0], Insn::LoadConst(5, 0)),
            "LoadConst should be hoisted to position 0 (before loop header); got {:?}",
            out[0]
        );
        // The loop header must still be present and its exit offset adjusted to
        // land on the Return (still at the end of the 5-instruction list).
        assert!(
            matches!(out[1], Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, _)),
            "loop header should remain at position 1"
        );
    }

    /// `BinOpConst(dst, src, op, c)` is loop-invariant when `src` is not written
    /// inside the loop body.  Verify it is hoisted.
    #[test]
    fn licm_hoists_binopconst_with_invariant_src() {
        use crate::ast::BinaryOp;

        // Loop layout:
        //  [0] ForCountConst(r0, Lt, 0, 1, 3) — header, exits to [4]
        //  [1] BinOpConst(r5, r2, Add, 0)      — r2 NOT written in loop → invariant
        //  [2] BinOp(r1, r1, Add, r0)           — uses r0 (written) → not invariant
        //  [3] Jump(-4)                          — back edge → [0]
        //  [4] Return(r1)
        //
        // r0 is written by ForCountConst; r1 by BinOp; r2 is untouched.
        let insns = vec![
            Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, 3), // [0]
            Insn::BinOpConst(5, 2, BinaryOp::Add, 0),      // [1] r2 not in write set → hoist
            Insn::BinOp(1, 1, BinaryOp::Add, 0),           // [2] r0 written → keep
            Insn::Jump(-4),                                // [3]
            Insn::Return(1),                               // [4]
        ];
        let out = pass_licm(insns);

        assert_eq!(out.len(), 5);
        assert!(
            matches!(out[0], Insn::BinOpConst(5, 2, BinaryOp::Add, 0)),
            "BinOpConst with invariant src should be hoisted before header; got {:?}",
            out[0]
        );
        assert!(
            matches!(out[1], Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, _)),
            "loop header must follow the hoisted instruction"
        );
    }

    /// `BinOpConst(dst, src, op, c)` where `src` IS written in the loop must NOT
    /// be hoisted.
    #[test]
    fn licm_does_not_hoist_binopconst_with_variant_src() {
        use crate::ast::BinaryOp;

        // r0 is the loop counter (written by ForCountConst); BinOpConst reads r0 → variant.
        //  [0] ForCountConst(r0, Lt, 0, 1, 3)
        //  [1] BinOpConst(r5, r0, Add, 0)      — r0 IS written → NOT invariant
        //  [2] BinOp(r1, r1, Add, r5)
        //  [3] Jump(-4)
        //  [4] Return(r1)
        let insns = vec![
            Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, 3), // [0]
            Insn::BinOpConst(5, 0, BinaryOp::Add, 0),      // [1] r0 in write set → keep
            Insn::BinOp(1, 1, BinaryOp::Add, 5),           // [2]
            Insn::Jump(-4),                                // [3]
            Insn::Return(1),                               // [4]
        ];
        let before = insns.clone();
        let out = pass_licm(insns);

        // Nothing should move: BinOpConst reads r0 which is written by ForCountConst.
        assert_eq!(
            out.len(),
            before.len(),
            "instruction count should not change"
        );
        assert!(
            matches!(out[0], Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, _)),
            "loop header must remain at position 0 (nothing hoisted)"
        );
        assert!(
            matches!(out[1], Insn::BinOpConst(5, 0, BinaryOp::Add, 0)),
            "variant BinOpConst must stay in loop body"
        );
    }

    /// Nested loops: an invariant in the inner loop that is also invariant wrt the
    /// outer loop ends up hoisted all the way out of both loops (before the outer
    /// header).  Verify that it is no longer inside the inner loop body.
    #[test]
    fn licm_hoists_inner_invariant_out_of_inner_loop() {
        use crate::ast::BinaryOp;

        // Outer loop: back edge at [7] → header [0].
        // Inner loop: back edge at [5] → inner header [2].
        //
        //  [0] ForCountConst(r0, Lt, 0, 1, 7)  — outer header, exits to [8]
        //  [1] BinOp(r1, r1, Add, r0)           — outer body (r0 written by outer)
        //  [2] ForCountConst(r3, Lt, 2, 3, 3)   — inner header, exits to [6]
        //  [3] LoadConst(r9, 0)                 — invariant wrt both loops
        //  [4] BinOp(r4, r4, Add, r3)           — uses r3 (written by inner) → variant
        //  [5] Jump(-4)                          — inner back edge → [2]
        //  [6] BinOp(r1, r1, Add, r4)           — outer body (after inner)
        //  [7] Jump(-8)                          — outer back edge → [0]
        //  [8] Return(r1)
        let insns = vec![
            Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, 7), // [0] outer header
            Insn::BinOp(1, 1, BinaryOp::Add, 0),           // [1]
            Insn::ForCountConst(3, BinaryOp::Lt, 2, 3, 3), // [2] inner header
            Insn::LoadConst(9, 0),                         // [3] invariant wrt inner
            Insn::BinOp(4, 4, BinaryOp::Add, 3),           // [4] variant wrt inner (r3 written)
            Insn::Jump(-4),                                // [5] inner back edge → [2]
            Insn::BinOp(1, 1, BinaryOp::Add, 4),           // [6]
            Insn::Jump(-8),                                // [7] outer back edge → [0]
            Insn::Return(1),                               // [8]
        ];
        let out = pass_licm(insns);

        assert_eq!(out.len(), 9, "total instruction count unchanged");

        // LoadConst(r9, 0) is invariant wrt both loops.  The inner loop processes
        // first and hoists it before the inner header; the outer loop then hoists it
        // again before the outer header.  Either way, it must not remain inside the
        // inner loop body [inner_header..inner_latch].
        //
        // Find the inner header (ForCountConst for r3) and latch (Jump with negative
        // offset targeting the inner header).  LoadConst(r9, _) must not appear
        // between them.
        let inner_header_pos = out
            .iter()
            .position(|i| matches!(i, Insn::ForCountConst(3, BinaryOp::Lt, 2, 3, _)))
            .expect("inner header must still exist");
        let inner_latch_pos = out
            .iter()
            .enumerate()
            .position(|(i, insn)| {
                if let Insn::Jump(k) = insn {
                    let target = i as i64 + 1 + *k as i64;
                    target == inner_header_pos as i64
                } else {
                    false
                }
            })
            .expect("inner back-edge Jump must still exist");

        let loadconst_inside_inner = out[inner_header_pos..=inner_latch_pos]
            .iter()
            .any(|i| matches!(i, Insn::LoadConst(9, _)));
        assert!(
            !loadconst_inside_inner,
            "LoadConst(r9) must be hoisted out of the inner loop body \
             (inner_header={inner_header_pos}, inner_latch={inner_latch_pos})"
        );

        // The overall structure must remain intact: outer header and inner header
        // must still exist.
        assert!(
            out.iter()
                .any(|i| matches!(i, Insn::ForCountConst(0, BinaryOp::Lt, 0, 1, _))),
            "outer loop header must still exist"
        );
    }

    // ── pass_cse ──────────────────────────────────────────────────────────────

    #[test]
    fn cse_duplicate_loadconst_becomes_copyreg() {
        // Two loads of the same constant index → second becomes CopyReg.
        // LoadConst(r2, 0)  LoadConst(r3, 0)  Return(r2)
        // Expected: LoadConst(r2, 0)  CopyReg(r3, r2)  Return(r2)
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::LoadConst(3, 0), // duplicate
            Insn::Return(2),
        ];
        let out = pass_cse(insns);
        assert_eq!(out.len(), 3, "instruction count unchanged");
        assert!(
            matches!(out[0], Insn::LoadConst(2, 0)),
            "first LoadConst must be kept"
        );
        assert!(
            matches!(out[1], Insn::CopyReg(3, 2)),
            "second LoadConst should become CopyReg(r3, r2)"
        );
    }

    #[test]
    fn cse_duplicate_binopconst_becomes_copyreg() {
        use crate::ast::BinaryOp;
        // BinOpConst(r4, r0, Add, 1)  …  BinOpConst(r5, r0, Add, 1)
        // The second should become CopyReg(r5, r4).
        let insns = vec![
            Insn::BinOpConst(4, 0, BinaryOp::Add, 1),
            Insn::BinOpConst(5, 0, BinaryOp::Add, 1), // duplicate
            Insn::Return(4),
        ];
        let out = pass_cse(insns);
        assert_eq!(out.len(), 3);
        assert!(
            matches!(out[0], Insn::BinOpConst(4, 0, BinaryOp::Add, 1)),
            "first BinOpConst must be kept"
        );
        assert!(
            matches!(out[1], Insn::CopyReg(5, 4)),
            "second BinOpConst should become CopyReg(r5, r4)"
        );
    }

    #[test]
    fn cse_intervening_write_invalidates_entry() {
        use crate::ast::BinaryOp;
        // BinOpConst(r4, r0, Add, 1)
        // LoadConst(r0, 2)        ← writes r0, the input of the BinOpConst
        // BinOpConst(r5, r0, Add, 1)   ← r0 is now a different value; NOT a duplicate
        let insns = vec![
            Insn::BinOpConst(4, 0, BinaryOp::Add, 1),
            Insn::LoadConst(0, 2), // clobbers r0
            Insn::BinOpConst(5, 0, BinaryOp::Add, 1),
            Insn::Return(4),
        ];
        let out = pass_cse(insns);
        assert_eq!(out.len(), 4, "no elimination when input clobbered");
        assert!(
            matches!(out[2], Insn::BinOpConst(5, 0, BinaryOp::Add, 1)),
            "second BinOpConst must not be replaced after input clobber"
        );
    }

    #[test]
    fn cse_does_not_cross_basic_block_boundary() {
        use crate::ast::BinaryOp;
        // LoadConst(r2, 0)
        // JumpIfFalse(r1, 0)   ← ends basic block; target is next instruction
        // LoadConst(r3, 0)     ← same const, but different basic block → NOT replaced
        // Return(r2)
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::JumpIfFalse(1, 0), // offset 0 → target = idx 2
            Insn::LoadConst(3, 0),   // idx 2 is a BB start (jump target)
            Insn::Return(2),
        ];
        let out = pass_cse(insns);
        // The third instruction (idx 2) is a BB start, so the CSE table is cleared
        // before it is processed; the second LoadConst(r3, 0) must NOT be replaced.
        assert!(
            matches!(out[2], Insn::LoadConst(3, 0)),
            "CSE must not cross basic-block boundary"
        );
    }

    #[test]
    fn cse_unary_op_duplicate_becomes_copyreg() {
        use crate::ast::UnaryOp;
        // UnaryOp(r4, Neg, r0)  UnaryOp(r5, Neg, r0)  → second becomes CopyReg(r5, r4)
        let insns = vec![
            Insn::UnaryOp(4, UnaryOp::Neg, 0),
            Insn::UnaryOp(5, UnaryOp::Neg, 0), // duplicate
            Insn::Return(4),
        ];
        let out = pass_cse(insns);
        assert_eq!(out.len(), 3);
        assert!(
            matches!(out[0], Insn::UnaryOp(4, UnaryOp::Neg, 0)),
            "first UnaryOp must be kept"
        );
        assert!(
            matches!(out[1], Insn::CopyReg(5, 4)),
            "second UnaryOp should become CopyReg(r5, r4)"
        );
    }

    #[test]
    fn cse_output_clobber_invalidates_entry() {
        // If the output register of a CSE candidate is overwritten, subsequent
        // identical computations cannot be replaced by a CopyReg pointing to it.
        //
        // LoadConst(r2, 0)        ← r2 holds consts[0]
        // LoadNone(r2)            ← clobbers r2; CSE entry for consts[0] removed
        // LoadConst(r3, 0)        ← must NOT be replaced by CopyReg(r3, r2)
        // Return(r3)
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::LoadNone(2), // clobbers the output register r2
            Insn::LoadConst(3, 0),
            Insn::Return(3),
        ];
        let out = pass_cse(insns);
        assert_eq!(out.len(), 4, "no elimination when output clobbered");
        assert!(
            matches!(out[2], Insn::LoadConst(3, 0)),
            "LoadConst must not be replaced after its output register is clobbered"
        );
    }

    // ── pass_ivsr ─────────────────────────────────────────────────────────────

    #[test]
    fn ivsr_replaces_induction_var_mul_with_accumulator() {
        use crate::ast::BinaryOp;
        // Simulates: for i in range(10): r_dst = i * 3
        //
        // [0] LoadConst(r_iv=2, c_neg1=0)        iv init = start - step = -1
        // [1] ForCountConst(2, Lt, c_10=1, c_1=2, k_exit=2)  → jumps to [4]
        // [2] BinOpConst(r_dst=3, 2, Mul, c_3=3)
        // [3] Jump(-3)                             back to [1]
        // [4] Return(3)
        let mut consts = vec![Value::int(-1), Value::int(10), Value::int(1), Value::int(3)];
        let mut num_regs = 4u32;
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::ForCountConst(2, BinaryOp::Lt, 1, 2, 2),
            Insn::BinOpConst(3, 2, BinaryOp::Mul, 3),
            Insn::Jump(-3),
            Insn::Return(3),
        ];
        let out = pass_ivsr(insns, &mut consts, &mut num_regs);

        // Expected (7 instructions): LoadConst(iv), LoadConst(acc), ForCountConst,
        //   Move(dst, acc), BinOpConst(acc += K), Jump(back), Return
        assert_eq!(out.len(), 7, "two instructions inserted");
        assert_eq!(num_regs, 5, "one new register allocated");
        // [1] = LoadConst(r_acc=4, c_neg3)  — acc init = -1 * 3 = -3
        assert!(
            matches!(out[1], Insn::LoadConst(4, _)),
            "accumulator init inserted before loop header"
        );
        // [2] = ForCountConst — exit offset now points to [6] (was [4])
        assert!(
            matches!(out[2], Insn::ForCountConst(2, BinaryOp::Lt, 1, 2, 3)),
            "ForCountConst exit offset adjusted (2 → 3)"
        );
        // [3] = Move(r_dst, r_acc)
        assert!(
            matches!(out[3], Insn::Move(3, 4)),
            "BinOpConst replaced by Move(dst, acc)"
        );
        // [4] = BinOpConst(r_acc, r_acc, Add, c_K)
        assert!(
            matches!(out[4], Insn::BinOpConst(4, 4, BinaryOp::Add, 3)),
            "accumulator increment inserted before back-edge"
        );
        // [5] = Jump — back-edge: old offset -3 (h=1, latch=3), new = h+1 - (latch+2) - 1 = 2-5-1 = -4
        assert!(
            matches!(out[5], Insn::Jump(-4)),
            "back-edge offset adjusted"
        );
        // Accumulator init value = 0 (= (-1 + 1) * 3 = start * K for range(10))
        let acc_init_in_consts = consts.iter().any(|v| matches!(v.kind(), ValueKind::Int(0)));
        assert!(
            acc_init_in_consts,
            "const 0 added for accumulator init ((-1+1)*3=0)"
        );
    }

    #[test]
    fn ivsr_skips_when_step_not_one() {
        use crate::ast::BinaryOp;
        // step = 2 → not eligible
        let mut consts = vec![Value::int(-2), Value::int(10), Value::int(2), Value::int(3)];
        let mut num_regs = 4u32;
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::ForCountConst(2, BinaryOp::Lt, 1, 2, 2), // step_c=2 → consts[2]=2 ≠ 1
            Insn::BinOpConst(3, 2, BinaryOp::Mul, 3),
            Insn::Jump(-3),
            Insn::Return(3),
        ];
        let out = pass_ivsr(insns, &mut consts, &mut num_regs);
        assert_eq!(out.len(), 5, "not eligible: step ≠ 1");
        assert_eq!(num_regs, 4, "no new register allocated");
    }

    #[test]
    fn ivsr_skips_when_mul_uses_non_induction_var() {
        use crate::ast::BinaryOp;
        // BinOpConst uses r_other=5, not r_iv=2 → not eligible
        let mut consts = vec![Value::int(-1), Value::int(10), Value::int(1), Value::int(3)];
        let mut num_regs = 6u32;
        let insns = vec![
            Insn::LoadConst(2, 0),
            Insn::ForCountConst(2, BinaryOp::Lt, 1, 2, 2),
            Insn::BinOpConst(3, 5, BinaryOp::Mul, 3), // r_src=5 ≠ iv=2
            Insn::Jump(-3),
            Insn::Return(3),
        ];
        let out = pass_ivsr(insns, &mut consts, &mut num_regs);
        assert_eq!(out.len(), 5, "not eligible: src ≠ iv");
    }

    // ── pass_self_tail_call ───────────────────────────────────────────────────

    #[test]
    fn self_tail_call_fuses_call_return() {
        // Call(func_reg=2, nargs=2) + Return(2) → TailCall { args_base: 3, nargs: 2 }
        // The Return is dropped; one instruction remains.
        let insns = vec![Insn::Call(2, 2), Insn::Return(2)];
        let out = pass_self_tail_call(insns);
        assert_eq!(out.len(), 1, "Return should be dropped");
        assert!(
            matches!(
                out[0],
                Insn::TailCall {
                    args_base: 3,
                    nargs: 2
                }
            ),
            "Call+Return should become TailCall with args_base=func_reg+1"
        );
    }

    #[test]
    fn self_tail_call_skips_when_return_uses_different_reg() {
        // Call writes to r2, but Return reads r3 → not a tail call pattern.
        let insns = vec![
            Insn::Call(2, 2),
            Insn::Return(3), // different register
        ];
        let out = pass_self_tail_call(insns);
        assert_eq!(
            out.len(),
            2,
            "no fusion when Return reads a different register"
        );
        assert!(matches!(out[0], Insn::Call(2, 2)));
        assert!(matches!(out[1], Insn::Return(3)));
    }

    #[test]
    fn self_tail_call_skips_when_not_adjacent() {
        // Call is NOT immediately followed by Return.
        let insns = vec![
            Insn::Call(2, 1),
            Insn::Move(0, 2), // intervening instruction
            Insn::Return(0),
        ];
        let out = pass_self_tail_call(insns);
        assert_eq!(
            out.len(),
            3,
            "no fusion when Call and Return are not adjacent"
        );
        assert!(matches!(out[0], Insn::Call(2, 1)));
    }

    #[test]
    fn self_tail_call_nargs_zero() {
        // Call(func_reg=0, nargs=0) + Return(0) → TailCall { args_base: 1, nargs: 0 }
        let insns = vec![Insn::Call(0, 0), Insn::Return(0)];
        let out = pass_self_tail_call(insns);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            Insn::TailCall {
                args_base: 1,
                nargs: 0
            }
        ));
    }

    #[test]
    fn self_tail_call_on_compiled_factorial() {
        // def factorial(n, acc=1):
        //     if n <= 1: return acc
        //     return factorial(n - 1, acc * n)
        let code = compile_fn(
            "def factorial(n, acc=1):\n    if n <= 1:\n        return acc\n    return factorial(n - 1, acc * n)\n",
        );
        let optimized = optimize(code);
        let inner = &optimized.fn_protos[0].code;
        let has_tailcall = inner
            .insns
            .iter()
            .any(|i| matches!(i, Insn::TailCall { .. }));
        assert!(
            has_tailcall,
            "recursive tail call in factorial should be optimised to TailCall"
        );
        // There should be no Call+Return pair left — the optimiser fused them all.
        let has_plain_call = inner
            .insns
            .iter()
            .any(|i| matches!(i, Insn::Call(..) | Insn::CallMemo(..)));
        assert!(
            !has_plain_call,
            "after TCO all recursive calls should be TailCall, not Call/CallMemo"
        );
    }
}
