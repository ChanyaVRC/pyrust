// ─── Partial Redundancy Elimination (PRE / Lazy Code Motion) ──────────────────

/// Exact primitive runtime type proven by forward dataflow analysis for PRE.
///
/// Only exact built-in primitives (never user subclasses, never unknown
/// parameters) are represented.  This guarantees that candidate arithmetic and
/// comparison instructions cannot invoke user dunders (`__mul__`, `__add__`,
/// `__bool__`, …), mutate shared namespaces, or introduce runtime exceptions
/// when hoisted to a compensation point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreExactType {
    Int,
    Float,
    Bool,
    Str,
    NoneType,
}

/// Canonical key for a pure, exception-free computational expression eligible
/// for Partial Redundancy Elimination.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PreExpr {
    BinOp {
        lhs: u32,
        op: crate::ast::BinaryOp,
        rhs: u32,
    },
    BinOpConst {
        lhs: u32,
        op: crate::ast::BinaryOp,
        const_idx: u16,
    },
    BinOpImm {
        lhs: u32,
        op: crate::ast::BinaryOp,
        imm: i16,
    },
    UnaryOp {
        op: crate::ast::UnaryOp,
        src: u32,
    },
}

impl PreExpr {
    /// Emit the bytecode instruction computing this expression into `dst`.
    fn to_insn(self, dst: u32) -> Insn {
        match self {
            PreExpr::BinOp { lhs, op, rhs } => Insn::BinOp(dst, lhs, op, rhs),
            PreExpr::BinOpConst { lhs, op, const_idx } => {
                Insn::BinOpConst(dst, lhs, op, const_idx, false)
            }
            PreExpr::BinOpImm { lhs, op, imm } => Insn::BinOpImm(dst, lhs, op, imm, false),
            PreExpr::UnaryOp { op, src } => Insn::UnaryOp(dst, op, src),
        }
    }

    #[inline]
    fn reads_reg(self, r: u32) -> bool {
        match self {
            PreExpr::BinOp { lhs, rhs, .. } => lhs == r || rhs == r,
            PreExpr::BinOpConst { lhs, .. } | PreExpr::BinOpImm { lhs, .. } => lhs == r,
            PreExpr::UnaryOp { src, .. } => src == r,
        }
    }

    #[inline]
    fn reads_named_local(self, num_locals: u32) -> bool {
        match self {
            PreExpr::BinOp { lhs, rhs, .. } => lhs < num_locals || rhs < num_locals,
            PreExpr::BinOpConst { lhs, .. } | PreExpr::BinOpImm { lhs, .. } => lhs < num_locals,
            PreExpr::UnaryOp { src, .. } => src < num_locals,
        }
    }
}

/// Exact primitive type of a constant-pool entry, if it is one of the exact
/// built-in types tracked by PRE.
#[inline]
fn pre_const_exact_type(val: &Value) -> Option<PreExactType> {
    match val.kind() {
        ValueKind::Int(_) | ValueKind::BigInt(_) => Some(PreExactType::Int),
        ValueKind::Float(_) => Some(PreExactType::Float),
        ValueKind::Bool(_) => Some(PreExactType::Bool),
        ValueKind::Str(_) => Some(PreExactType::Str),
        ValueKind::None => Some(PreExactType::NoneType),
        _ => None,
    }
}

/// Whether `op` is commutative on exact `Int` / `Float` / `Str` pairs.
#[inline]
fn pre_op_is_commutative(op: crate::ast::BinaryOp) -> bool {
    use crate::ast::BinaryOp::*;
    matches!(op, Add | Mul | BitAnd | BitOr | BitXor | Eq | Ne)
}

/// Result type of a binary operation when both operands have proven exact
/// primitive types, AND whether the operation is guaranteed exception-free
/// (never raises `ZeroDivisionError`, `ValueError`, `TypeError`, or
/// `OverflowError`, and never invokes user dunders).
///
/// Returns `Some((result_type, exception_free))` when the operation is a pure
/// primitive operation on exact types (no user dunder callback possible), or
/// `None` if user protocols could be invoked.
fn pre_classify_binop(
    lt: PreExactType,
    op: crate::ast::BinaryOp,
    rt: PreExactType,
    rhs_const: Option<&Value>,
) -> Option<(PreExactType, bool)> {
    use crate::ast::BinaryOp::*;
    match (lt, op, rt) {
        // Exact Int/Int arithmetic & bitwise that never raise in Python
        // (int promotes to BigInt on overflow).
        (PreExactType::Int, Add | Sub | Mul | BitAnd | BitOr | BitXor, PreExactType::Int) => {
            Some((PreExactType::Int, true))
        }
        // Exact Int/Int floor-div / mod: pure primitive (no dunder), exception-free
        // only when the RHS is a provably non-zero constant integer.
        (PreExactType::Int, FloorDiv | Mod, PreExactType::Int) => {
            let non_zero = rhs_const.is_some_and(|v| match v.kind() {
                ValueKind::Int(n) => n != 0,
                ValueKind::BigInt(b) => !pyrust_core::PyZero::is_zero(b),
                _ => false,
            });
            Some((PreExactType::Int, non_zero))
        }
        // Exact Int/Int true division: returns Float; exception-free only when
        // divisor is provably non-zero AND fits in finite float range (skip
        // hoisting Int/Int true div since huge BigInt->float can raise
        // OverflowError; still pure primitive for type propagation).
        (PreExactType::Int, Div, PreExactType::Int) => Some((PreExactType::Float, false)),
        // Exact Int/Int shifts: pure primitive; exception-free only for a
        // bounded non-negative constant shift count.
        (PreExactType::Int, LShift | RShift, PreExactType::Int) => {
            let safe_shift = rhs_const.is_some_and(|v| matches!(v.kind(), ValueKind::Int(0..=64)));
            Some((PreExactType::Int, safe_shift))
        }
        // Exact Float/Float IEEE-754 arithmetic (+, -, *) never raises in Python.
        (PreExactType::Float, Add | Sub | Mul, PreExactType::Float) => {
            Some((PreExactType::Float, true))
        }
        // Exact Float/Float division: exception-free only for provably non-zero
        // constant divisor.
        (PreExactType::Float, Div, PreExactType::Float) => {
            let non_zero =
                rhs_const.is_some_and(|v| matches!(v.kind(), ValueKind::Float(f) if f != 0.0));
            Some((PreExactType::Float, non_zero))
        }
        // Exact Int/Int, Float/Float, Str/Str comparisons: never raise, never
        // invoke user protocols.
        (PreExactType::Int, Eq | Ne | Lt | Le | Gt | Ge, PreExactType::Int)
        | (PreExactType::Float, Eq | Ne | Lt | Le | Gt | Ge, PreExactType::Float)
        | (PreExactType::Str, Eq | Ne | Lt | Le | Gt | Ge, PreExactType::Str) => {
            Some((PreExactType::Bool, true))
        }
        (PreExactType::Bool, Eq | Ne, PreExactType::Bool) => Some((PreExactType::Bool, true)),
        _ => None,
    }
}

/// Result type and exception-freedom of a unary operation on a proven exact
/// primitive type.
fn pre_classify_unary(op: crate::ast::UnaryOp, st: PreExactType) -> Option<(PreExactType, bool)> {
    use crate::ast::UnaryOp::*;
    match (op, st) {
        (Neg | Pos | BitNot, PreExactType::Int) => Some((PreExactType::Int, true)),
        (Neg | Pos, PreExactType::Float) => Some((PreExactType::Float, true)),
        (
            Not,
            PreExactType::Int
            | PreExactType::Float
            | PreExactType::Bool
            | PreExactType::Str
            | PreExactType::NoneType,
        ) => Some((PreExactType::Bool, true)),
        _ => None,
    }
}

/// Check whether `expr` is both pure and exception-free given the proven exact
/// register types in `types`.
fn pre_expr_is_safe(expr: PreExpr, types: &HashMap<u32, PreExactType>, consts: &[Value]) -> bool {
    match expr {
        PreExpr::BinOp { lhs, op, rhs } => {
            let (Some(&lt), Some(&rt)) = (types.get(&lhs), types.get(&rhs)) else {
                return false;
            };
            pre_classify_binop(lt, op, rt, None).is_some_and(|(_, safe)| safe)
        }
        PreExpr::BinOpConst { lhs, op, const_idx } => {
            let Some(&lt) = types.get(&lhs) else {
                return false;
            };
            let Some(rv) = consts.get(const_idx as usize) else {
                return false;
            };
            let Some(rt) = pre_const_exact_type(rv) else {
                return false;
            };
            pre_classify_binop(lt, op, rt, Some(rv)).is_some_and(|(_, safe)| safe)
        }
        PreExpr::BinOpImm { lhs, op, imm } => {
            let Some(&lt) = types.get(&lhs) else {
                return false;
            };
            let rv = Value::int(i64::from(imm));
            pre_classify_binop(lt, op, PreExactType::Int, Some(&rv)).is_some_and(|(_, safe)| safe)
        }
        PreExpr::UnaryOp { op, src } => {
            let Some(&st) = types.get(&src) else {
                return false;
            };
            pre_classify_unary(op, st).is_some_and(|(_, safe)| safe)
        }
    }
}

/// Try to extract a pure, exception-free `(dst, PreExpr)` from `insn` given
/// the current proven exact types.
fn pre_extract_expr(
    insn: &Insn,
    types: &HashMap<u32, PreExactType>,
    consts: &[Value],
) -> Option<(u32, PreExpr)> {
    match insn {
        Insn::BinOp(dst, lhs, op, rhs) => {
            let (Some(&lt), Some(&rt)) = (types.get(lhs), types.get(rhs)) else {
                return None;
            };
            if !pre_classify_binop(lt, *op, rt, None).is_some_and(|(_, safe)| safe) {
                return None;
            }
            let (a, b) = if pre_op_is_commutative(*op) && lt == rt && lhs > rhs {
                (*rhs, *lhs)
            } else {
                (*lhs, *rhs)
            };
            Some((
                *dst,
                PreExpr::BinOp {
                    lhs: a,
                    op: *op,
                    rhs: b,
                },
            ))
        }
        Insn::BinOpConst(dst, lhs, op, const_idx, _) => {
            let expr = PreExpr::BinOpConst {
                lhs: *lhs,
                op: *op,
                const_idx: *const_idx,
            };
            pre_expr_is_safe(expr, types, consts).then_some((*dst, expr))
        }
        Insn::BinOpImm(dst, lhs, op, imm, _) => {
            let expr = PreExpr::BinOpImm {
                lhs: *lhs,
                op: *op,
                imm: *imm,
            };
            pre_expr_is_safe(expr, types, consts).then_some((*dst, expr))
        }
        Insn::UnaryOp(dst, op, src) => {
            let expr = PreExpr::UnaryOp { op: *op, src: *src };
            pre_expr_is_safe(expr, types, consts).then_some((*dst, expr))
        }
        _ => None,
    }
}

/// Apply one instruction to the forward exact-type map `types`, returning
/// `(is_pure_non_reentrant, may_raise)`.
///
/// Pure primitive instructions (exact `Int`/`Float`/`Str`/`Bool` arithmetic,
/// comparisons, truth tests, `LoadConst`, `Move`, `Jump`, etc.) cannot invoke
/// user Python code and therefore do not invalidate named locals.
fn pre_step_types(
    insn: &Insn,
    types: &mut HashMap<u32, PreExactType>,
    consts: &[Value],
    num_locals: u32,
) -> (bool, bool) {
    match insn {
        Insn::LoadConst(dst, c) => {
            if let Some(t) = consts.get(*c as usize).and_then(pre_const_exact_type) {
                types.insert(*dst, t);
            } else {
                types.remove(dst);
            }
            (true, false)
        }
        Insn::LoadNone(dst) => {
            types.insert(*dst, PreExactType::NoneType);
            (true, false)
        }
        Insn::LoadNoneRange { start, count } => {
            for offset in 0..*count as u32 {
                types.insert(start + offset, PreExactType::NoneType);
            }
            (true, false)
        }
        Insn::Move(dst, src) | Insn::CopyReg(dst, src) => {
            match types.get(src).copied() {
                Some(t) => {
                    types.insert(*dst, t);
                }
                None => {
                    types.remove(dst);
                }
            }
            (true, false)
        }
        Insn::BinOp(dst, lhs, op, rhs) | Insn::BinOpInPlace(dst, lhs, op, rhs) => {
            if let (Some(&lt), Some(&rt)) = (types.get(lhs), types.get(rhs))
                && let Some((res_t, safe)) = pre_classify_binop(lt, *op, rt, None)
            {
                types.insert(*dst, res_t);
                return (true, !safe);
            }
            types.remove(dst);
            types.retain(|&r, _| r >= num_locals);
            (false, true)
        }
        Insn::BinOpConst(dst, lhs, op, c, _) => {
            if let Some(&lt) = types.get(lhs)
                && let Some(rv) = consts.get(*c as usize)
                && let Some(rt) = pre_const_exact_type(rv)
                && let Some((res_t, safe)) = pre_classify_binop(lt, *op, rt, Some(rv))
            {
                types.insert(*dst, res_t);
                return (true, !safe);
            }
            types.remove(dst);
            types.retain(|&r, _| r >= num_locals);
            (false, true)
        }
        Insn::BinOpImm(dst, lhs, op, imm, _) => {
            let rv = Value::int(i64::from(*imm));
            if let Some(&lt) = types.get(lhs)
                && let Some((res_t, safe)) =
                    pre_classify_binop(lt, *op, PreExactType::Int, Some(&rv))
            {
                types.insert(*dst, res_t);
                return (true, !safe);
            }
            types.remove(dst);
            types.retain(|&r, _| r >= num_locals);
            (false, true)
        }
        Insn::UnaryOp(dst, op, src) => {
            if let Some(&st) = types.get(src)
                && let Some((res_t, safe)) = pre_classify_unary(*op, st)
            {
                types.insert(*dst, res_t);
                return (true, !safe);
            }
            types.remove(dst);
            types.retain(|&r, _| r >= num_locals);
            (false, true)
        }
        Insn::Jump(_)
        | Insn::SetupExcept(_)
        | Insn::PopExcept
        | Insn::Return(_)
        | Insn::ReturnNone => (true, false),
        Insn::JumpIfFalse(cond, _) | Insn::JumpIfTrue(cond, _) => {
            if types.contains_key(cond) {
                (true, false)
            } else {
                types.retain(|&r, _| r >= num_locals);
                (false, true)
            }
        }
        Insn::CmpJumpIfFalse(lhs, op, rhs, _) | Insn::CmpJumpIfTrue(lhs, op, rhs, _) => {
            if let (Some(&lt), Some(&rt)) = (types.get(lhs), types.get(rhs))
                && let Some((_, safe)) = pre_classify_binop(lt, *op, rt, None)
            {
                return (true, !safe);
            }
            types.retain(|&r, _| r >= num_locals);
            (false, true)
        }
        Insn::CmpJumpIfFalseConst(lhs, op, c, _) | Insn::CmpJumpIfTrueConst(lhs, op, c, _) => {
            if let Some(&lt) = types.get(lhs)
                && let Some(rv) = consts.get(*c as usize)
                && let Some(rt) = pre_const_exact_type(rv)
                && let Some((_, safe)) = pre_classify_binop(lt, *op, rt, Some(rv))
            {
                return (true, !safe);
            }
            types.retain(|&r, _| r >= num_locals);
            (false, true)
        }
        insn => {
            for_each_write(insn, |w| {
                types.remove(&w);
            });
            let pure = !may_invalidate_named_locals(insn);
            if !pure {
                types.retain(|&r, _| r >= num_locals);
            }
            (pure, sccp_insn_may_raise(insn))
        }
    }
}

/// Partial Redundancy Elimination (PRE / Lazy Code Motion, issue #350).
///
/// Eliminates partially redundant pure primitive computations across branches
/// and loops by inserting compensation copies at the latest safe points and
/// reusing the computed value via `Move(dst, temp)`.
///
/// ## Soundness guarantees
/// 1. **Exact primitive type proof**: A forward type analysis proves exact
///    built-in primitive operand types (`Int`, `Float`, `Bool`, `Str`,
///    `NoneType`), starting with all registers `Unknown` at entry (covering
///    function parameters and uninitialized locals).  Only pure, exception-free
///    operations on proven exact primitives are admitted — user objects with
///    custom `__mul__`/`__add__`, division by unknown/zero divisors, and
///    potentially-raising ops are never hoisted or deduplicated.
/// 2. **Down-safety (Anticipatability)**: Expressions are only hoisted to
///    blocks/edges where EVERY forward path evaluates the expression before
///    any operand redefinition, named-local re-entrancy barrier, or exit.
/// 3. **Exception-region isolation**: Expressions never cross `SetupExcept` /
///    `PopExcept` / handler region boundaries, and potentially-raising
///    instructions inside a `try` region act as barriers to anticipation and
///    availability.
/// 4. **Forward MUST self-verification**: Before committing any candidate
///    expression, a forward MUST check verifies that the dedicated temp
///    register is initialized and fresh along every executable path reaching
///    each `Move(dst, temp)` replacement and that at least one original
///    computation is strictly eliminated.
fn pass_pre(insns: Vec<Insn>, consts: &[Value], num_locals: u32, num_regs: &mut u32) -> Vec<Insn> {
    debug_assert_no_late_stage_insns(&insns, "pass_pre");
    const MAX_PRE_EXPRS: usize = 64;

    let n = insns.len();
    if n < 3 {
        return insns;
    }

    // Step 0: Derive exception-handler region ID per PC (0 = no active handler).
    let has_except = insns.iter().any(|i| matches!(i, Insn::SetupExcept(_)));
    let (region_at, handler_pc_at): (Vec<u32>, Vec<usize>) = if has_except {
        let Some(stack_in) = analyze_active_handler_stacks(&insns) else {
            return insns;
        };
        let mut region_map: HashMap<Vec<usize>, u32> = HashMap::new();
        region_map.insert(Vec::new(), 0);
        let mut regions = vec![0u32; n];
        let mut handlers = vec![usize::MAX; n];
        for (pc, stack) in stack_in.into_iter().enumerate() {
            if let Some(s) = stack {
                if let Some(&h) = s.last() {
                    handlers[pc] = h;
                }
                let next_id = region_map.len() as u32;
                let id = *region_map.entry(s).or_insert(next_id);
                regions[pc] = id;
            }
        }
        (regions, handlers)
    } else {
        (vec![0u32; n], Vec::new())
    };

    // Step 1: Partition into basic blocks and build CFG adjacency.
    let mut is_leader_buf = InlineBoolBuf::new(n, false);
    let is_leader = &mut is_leader_buf.as_mut_slice()[..n];
    is_leader[0] = true;
    for (pc, insn) in insns.iter().enumerate() {
        if let Some(off) = insn_jump_off(insn) {
            let target = pc as i64 + 1 + i64::from(off);
            if target >= 0 && (target as usize) < n {
                is_leader[target as usize] = true;
            }
        }
        if sccp_ends_basic_block(insn) && pc + 1 < n {
            is_leader[pc + 1] = true;
        }
    }

    let mut blocks: Vec<(usize, usize)> = Vec::new();
    let mut pc_to_block = vec![0usize; n];
    let mut start = 0usize;
    for pc in 0..n {
        if pc > 0 && is_leader[pc] {
            blocks.push((start, pc));
            start = pc;
        }
        pc_to_block[pc] = blocks.len();
    }
    blocks.push((start, n));
    let num_blocks = blocks.len();

    let jump_target = |pc: usize, off: i32| -> usize { (pc as i64 + 1 + i64::from(off)) as usize };
    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); num_blocks];
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); num_blocks];
    let mut is_exit = vec![false; num_blocks];
    let mut block_region = vec![0u32; num_blocks];

    for (b, &(b_start, b_end)) in blocks.iter().enumerate() {
        block_region[b] = region_at[b_start];
        let last_pc = b_end - 1;
        let mut add_succ = |target_pc: usize| {
            if target_pc < n {
                let tb = pc_to_block[target_pc];
                if !succs[b].contains(&tb) {
                    succs[b].push(tb);
                    preds[tb].push(b);
                }
            } else {
                is_exit[b] = true;
            }
        };
        match &insns[last_pc] {
            Insn::Jump(k) => add_succ(jump_target(last_pc, *k)),
            Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => {
                add_succ(last_pc + 1);
                add_succ(jump_target(last_pc, *k));
            }
            Insn::Return(_)
            | Insn::ReturnNone
            | Insn::RaiseValue(_)
            | Insn::RaiseExceptStarResidual(_)
            | Insn::RaiseFrom(..)
            | Insn::RaiseReRaise
            | Insn::RaiseAssert(_)
            | Insn::RaiseAssertNoMsg => {
                is_exit[b] = true;
            }
            _ => add_succ(b_end),
        }
    }

    // Step 2: Forward exact-type analysis (worklist fixpoint).
    type TypeMap = HashMap<u32, PreExactType>;
    let mut type_in: Vec<Option<TypeMap>> = vec![None; num_blocks];
    let mut type_out_before_term: Vec<Option<TypeMap>> = vec![None; num_blocks];
    let mut in_wl = vec![false; num_blocks];
    let mut wl = std::collections::VecDeque::new();

    type_in[0] = Some(HashMap::new());
    in_wl[0] = true;
    wl.push_back(0);

    let propagate_types = |target_b: usize,
                           incoming: &TypeMap,
                           type_in: &mut [Option<TypeMap>],
                           in_wl: &mut [bool],
                           wl: &mut std::collections::VecDeque<usize>| {
        match &mut type_in[target_b] {
            None => {
                type_in[target_b] = Some(incoming.clone());
                if !in_wl[target_b] {
                    in_wl[target_b] = true;
                    wl.push_back(target_b);
                }
            }
            Some(existing) => {
                if !existing.is_empty() {
                    let before = existing.len();
                    existing.retain(|r, t| incoming.get(r) == Some(&*t));
                    if existing.len() < before && !in_wl[target_b] {
                        in_wl[target_b] = true;
                        wl.push_back(target_b);
                    }
                }
            }
        }
    };

    while let Some(b) = wl.pop_front() {
        in_wl[b] = false;
        let Some(mut state) = type_in[b].clone() else {
            continue;
        };
        let (b_start, b_end) = blocks[b];
        let has_term = sccp_ends_basic_block(&insns[b_end - 1]);
        let body_end = if has_term { b_end - 1 } else { b_end };

        for pc in b_start..body_end {
            let insn = &insns[pc];
            let pre_state = if has_except && handler_pc_at[pc] < n {
                Some(state.clone())
            } else {
                None
            };
            let (pure, may_raise) = pre_step_types(insn, &mut state, consts, num_locals);
            if may_raise && let Some(mut exc_state) = pre_state {
                let hb = pc_to_block[handler_pc_at[pc]];
                if !pure {
                    exc_state.retain(|&r, _| r >= num_locals);
                }
                for_each_write(insn, |w| {
                    exc_state.remove(&w);
                });
                propagate_types(hb, &exc_state, &mut type_in, &mut in_wl, &mut wl);
            }
        }

        type_out_before_term[b] = Some(state.clone());

        if has_term {
            let last_pc = b_end - 1;
            let term = &insns[last_pc];
            let pre_state = if has_except && handler_pc_at[last_pc] < n {
                Some(state.clone())
            } else {
                None
            };
            let (pure, may_raise) = pre_step_types(term, &mut state, consts, num_locals);
            if may_raise && let Some(mut exc_state) = pre_state {
                let hb = pc_to_block[handler_pc_at[last_pc]];
                if !pure {
                    exc_state.retain(|&r, _| r >= num_locals);
                }
                for_each_write(term, |w| {
                    exc_state.remove(&w);
                });
                propagate_types(hb, &exc_state, &mut type_in, &mut in_wl, &mut wl);
            }
            match term {
                Insn::ForIter(dst, _, k) => {
                    // Exit edge does not write `dst`; fallthrough edge writes `dst` = Unknown.
                    let exit_pc = jump_target(last_pc, *k);
                    if exit_pc < n {
                        propagate_types(
                            pc_to_block[exit_pc],
                            &state,
                            &mut type_in,
                            &mut in_wl,
                            &mut wl,
                        );
                    }
                    state.remove(dst);
                    if last_pc + 1 < n {
                        propagate_types(
                            pc_to_block[last_pc + 1],
                            &state,
                            &mut type_in,
                            &mut in_wl,
                            &mut wl,
                        );
                    }
                }
                _ => {
                    for &s in &succs[b] {
                        propagate_types(s, &state, &mut type_in, &mut in_wl, &mut wl);
                    }
                }
            }
        } else {
            for &s in &succs[b] {
                propagate_types(s, &state, &mut type_in, &mut in_wl, &mut wl);
            }
        }
    }

    // Step 3: Discover candidate pure expressions and filter to those that can
    // possibly be redundant (appear >= 2 times, or appear in a block reachable
    // by a back-edge / cycle).
    let mut expr_occurrences: HashMap<PreExpr, Vec<(usize, usize, u32)>> = HashMap::new();
    let mut pc_expr: Vec<Option<(u32, PreExpr)>> = vec![None; n];

    // Identify blocks with any incoming back-edge (or reachable from a loop
    // back-edge): conservatively check whether `b` can reach itself via a cycle.
    let mut has_back_edge_in_fn = false;
    for b in 0..num_blocks {
        if succs[b].iter().any(|&s| s <= b) {
            has_back_edge_in_fn = true;
            break;
        }
    }

    for (b, &(b_start, b_end)) in blocks.iter().enumerate() {
        let Some(mut state) = type_in[b].clone() else {
            continue;
        };
        for pc in b_start..b_end {
            let insn = &insns[pc];
            if let Some((dst, expr)) = pre_extract_expr(insn, &state, consts) {
                pc_expr[pc] = Some((dst, expr));
                expr_occurrences.entry(expr).or_default().push((b, pc, dst));
            }
            pre_step_types(insn, &mut state, consts, num_locals);
        }
    }

    // Keep expressions that appear at least twice, OR appear in a function with
    // loops (so loop-invariant PRE can hoist single-occurrence loop expressions).
    let mut exprs: Vec<PreExpr> = Vec::new();
    let mut expr_to_bit: HashMap<PreExpr, usize> = HashMap::new();
    for (expr, occs) in &expr_occurrences {
        if occs.len() < 2 && !has_back_edge_in_fn {
            continue;
        }
        if exprs.len() >= MAX_PRE_EXPRS {
            break;
        }
        let bit = exprs.len();
        expr_to_bit.insert(*expr, bit);
        exprs.push(*expr);
    }

    let num_exprs = exprs.len();
    if num_exprs == 0 {
        return insns;
    }

    let all_mask: u64 = if num_exprs == 64 {
        u64::MAX
    } else {
        (1u64 << num_exprs) - 1
    };

    // Precompute per-expression helper masks.
    let mut named_local_expr_mask = 0u64;
    for (bit, &expr) in exprs.iter().enumerate() {
        if expr.reads_named_local(num_locals) {
            named_local_expr_mask |= 1u64 << bit;
        }
    }

    // Step 4: Compute per-block local dataflow bitmasks (`gen_ant`, `comp`,
    // `transp_body`, `term_kills`, `type_safe_in`, `type_safe_out`).
    let mut gen_ant = vec![0u64; num_blocks];
    let mut comp = vec![0u64; num_blocks];
    let mut transp_body = vec![all_mask; num_blocks];
    let mut term_kills = vec![0u64; num_blocks];
    let mut type_safe_in = vec![0u64; num_blocks];
    let mut type_safe_out = vec![0u64; num_blocks];

    let mask_for_write = |w: u32, exprs: &[PreExpr]| -> u64 {
        let mut m = 0u64;
        for (bit, &expr) in exprs.iter().enumerate() {
            if expr.reads_reg(w) {
                m |= 1u64 << bit;
            }
        }
        m
    };

    for (b, &(b_start, b_end)) in blocks.iter().enumerate() {
        let Some(mut state) = type_in[b].clone() else {
            transp_body[b] = 0;
            continue;
        };
        let in_try = block_region[b] != 0;
        for (bit, &expr) in exprs.iter().enumerate() {
            if pre_expr_is_safe(expr, &state, consts) {
                type_safe_in[b] |= 1u64 << bit;
            }
        }
        if let Some(out_state) = &type_out_before_term[b] {
            for (bit, &expr) in exprs.iter().enumerate() {
                if pre_expr_is_safe(expr, out_state, consts) {
                    type_safe_out[b] |= 1u64 << bit;
                }
            }
        }

        let has_term = sccp_ends_basic_block(&insns[b_end - 1]);
        let body_end = if has_term { b_end - 1 } else { b_end };

        let mut killed_so_far = 0u64;
        let mut comp_cur = 0u64;
        for pc in b_start..body_end {
            let insn = &insns[pc];
            let expr_bit = pc_expr[pc].and_then(|(_, e)| expr_to_bit.get(&e).copied());

            if let Some(bit) = expr_bit {
                let bit_mask = 1u64 << bit;
                if (killed_so_far & bit_mask) == 0 && (type_safe_in[b] & bit_mask) != 0 {
                    gen_ant[b] |= bit_mask;
                }
            }

            let (pure, may_raise) = pre_step_types(insn, &mut state, consts, num_locals);
            let mut insn_kill = 0u64;
            if in_try && may_raise {
                insn_kill = all_mask;
            } else {
                if !pure {
                    insn_kill |= named_local_expr_mask;
                }
                for_each_write(insn, |w| {
                    insn_kill |= mask_for_write(w, &exprs);
                });
            }

            // Record availability of `expr_bit` if this instruction's own write
            // did not clobber one of its operands (e.g. `a = a + 1`).
            if let Some(bit) = expr_bit {
                let bit_mask = 1u64 << bit;
                if (insn_kill & bit_mask) == 0 {
                    comp_cur |= bit_mask;
                }
            }
            comp_cur &= !insn_kill;
            killed_so_far |= insn_kill;
        }
        transp_body[b] = all_mask & !killed_so_far;

        if has_term {
            let last_pc = b_end - 1;
            let term = &insns[last_pc];
            let (pure, may_raise) = pre_step_types(term, &mut state, consts, num_locals);
            let mut tk = 0u64;
            if in_try && may_raise {
                tk = all_mask;
            } else {
                if !pure {
                    tk |= named_local_expr_mask;
                }
                for_each_write(term, |w| {
                    tk |= mask_for_write(w, &exprs);
                });
            }
            term_kills[b] = tk;
        }
        comp[b] = comp_cur & !term_kills[b] & type_safe_out[b];
    }

    let safe_edge = |u: usize, v: usize| -> bool { block_region[u] == block_region[v] };

    // Step 5: Anticipated (down-safe, backward MUST).
    let mut ant_in = vec![0u64; num_blocks];
    let mut ant_out = vec![0u64; num_blocks];
    for b in 0..num_blocks {
        if type_in[b].is_some() && !is_exit[b] {
            ant_in[b] = type_safe_in[b];
            ant_out[b] = type_safe_out[b];
        } else if type_in[b].is_some() {
            ant_in[b] = gen_ant[b];
        }
    }
    loop {
        let mut changed = false;
        for b in (0..num_blocks).rev() {
            if type_in[b].is_none() {
                continue;
            }
            let new_out = if is_exit[b] || succs[b].is_empty() {
                0
            } else {
                let mut acc = all_mask;
                for &s in &succs[b] {
                    if safe_edge(b, s) && type_in[s].is_some() {
                        acc &= ant_in[s];
                    } else {
                        acc = 0;
                        break;
                    }
                }
                acc & !term_kills[b] & type_safe_out[b]
            };
            let new_in = (gen_ant[b] | (new_out & transp_body[b])) & type_safe_in[b];
            if new_out != ant_out[b] || new_in != ant_in[b] {
                ant_out[b] = new_out;
                ant_in[b] = new_in;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Step 6: Real availability (forward MUST) and partial availability
    // (forward MAY) BEFORE any PRE insertions.
    let mut real_av_in = vec![0u64; num_blocks];
    let mut real_av_out = vec![0u64; num_blocks];
    for b in 1..num_blocks {
        if type_in[b].is_some() {
            real_av_in[b] = all_mask;
            real_av_out[b] = all_mask;
        }
    }
    real_av_out[0] = comp[0];
    loop {
        let mut changed = false;
        for b in 0..num_blocks {
            if type_in[b].is_none() {
                continue;
            }
            let new_in = if b == 0 {
                0
            } else {
                let mut acc = all_mask;
                let mut any_pred = false;
                for &p in &preds[b] {
                    if type_in[p].is_none() {
                        continue;
                    }
                    any_pred = true;
                    if safe_edge(p, b) {
                        acc &= real_av_out[p];
                    } else {
                        acc = 0;
                        break;
                    }
                }
                if any_pred { acc } else { 0 }
            };
            let new_out = (comp[b] | (new_in & transp_body[b])) & !term_kills[b] & type_safe_out[b];
            if new_in != real_av_in[b] || new_out != real_av_out[b] {
                real_av_in[b] = new_in;
                real_av_out[b] = new_out;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut pav_in = vec![0u64; num_blocks];
    let mut pav_out = vec![0u64; num_blocks];
    loop {
        let mut changed = false;
        for b in 0..num_blocks {
            if type_in[b].is_none() {
                continue;
            }
            let new_in = if b == 0 {
                0
            } else {
                let mut acc = 0u64;
                for &p in &preds[b] {
                    if type_in[p].is_some() && safe_edge(p, b) {
                        acc |= pav_out[p];
                    }
                }
                acc
            };
            let new_out = (comp[b] | (new_in & transp_body[b])) & !term_kills[b] & type_safe_out[b];
            if new_in != pav_in[b] || new_out != pav_out[b] {
                pav_in[b] = new_in;
                pav_out[b] = new_out;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Step 7: Plan compensation insertions & verify strict safety +
    // profitability independently for each candidate expression `bit`.
    //
    // Insertion locations:
    // - `exit_inserts[b]`: expressions to compute right before `b`'s terminator
    //   (or at the end of `b` if `b` has no terminator).
    // - `fallthrough_edge_inserts[p]`: expressions to compute on the fallthrough
    //   edge leaving conditional block `p` (inserted right after `p`'s
    //   terminator, before `b_start`).
    // - `jump_edge_inserts[(p, b)]`: expressions to compute on a critical jump
    //   edge `p -> b` (emitted via an appended trampoline when both `p` and `b`
    //   are outside `try` regions).
    struct ExprPlan {
        temp_reg: u32,
        exit_blocks: Vec<usize>,
        fallthrough_edges: Vec<usize>,
        jump_edges: Vec<(usize, usize)>,
        replace_pcs: Vec<usize>,
        capture_pcs: Vec<usize>,
    }

    // Ensure fresh temp registers never collide with existing registers.
    let mut max_reg_used = (*num_regs).max(num_locals);
    for insn in &insns {
        for_each_read(insn, |r| {
            if r + 1 > max_reg_used {
                max_reg_used = r + 1;
            }
        });
        for_each_write(insn, |w| {
            if w + 1 > max_reg_used {
                max_reg_used = w + 1;
            }
        });
    }

    let mut plans: Vec<(usize, ExprPlan)> = Vec::new();
    let mut next_temp = max_reg_used;

    for (bit, &expr) in exprs.iter().enumerate() {
        let mask = 1u64 << bit;

        // Seed blocks where `e` is both anticipated (down-safe) AND partially
        // available (redundant on at least one incoming path).
        let mut need_in = vec![false; num_blocks];
        let mut any_seed = false;
        for b in 0..num_blocks {
            if type_in[b].is_some() && ((ant_in[b] & pav_in[b]) & mask) != 0 {
                need_in[b] = true;
                any_seed = true;
            }
        }
        if !any_seed {
            continue;
        }

        // Determine compensation insertions for predecessors of `need_in` blocks
        // that do not already make `e` available (`real_av_out[p] == 0`).
        let mut exit_insert = vec![false; num_blocks];
        let mut ft_insert = vec![false; num_blocks];
        let mut jump_insert: Vec<(usize, usize)> = Vec::new();
        let mut aborted = false;

        'plan_loop: for b in 0..num_blocks {
            if !need_in[b] {
                continue;
            }
            for &p in &preds[b] {
                if type_in[p].is_none() {
                    continue;
                }
                if !safe_edge(p, b) {
                    aborted = true;
                    break 'plan_loop;
                }
                if (real_av_out[p] & mask) != 0 {
                    // Already available at exit of `p`.
                    continue;
                }
                if (ant_out[p] & mask) != 0 {
                    // Down-safe at exit of `p` (covers both single-successor
                    // blocks and diamond headers where ALL successors anticipate
                    // `e`).  Insert once at the exit of `p`.
                    exit_insert[p] = true;
                } else {
                    // Critical edge `p -> b`: `p` has another successor that
                    // does NOT anticipate `e`.
                    let p_last = blocks[p].1 - 1;
                    let b_start = blocks[b].0;
                    if p_last + 1 == b_start && succs[p].len() > 1 {
                        // Fallthrough edge from conditional branch in `p`:
                        // safe to insert inline right after `p_last` and before
                        // `b_start` (only executed on `p -> b` fallthrough!).
                        if (type_safe_out[p] & mask) != 0 && (term_kills[p] & mask) == 0 {
                            ft_insert[p] = true;
                        } else {
                            aborted = true;
                            break 'plan_loop;
                        }
                    } else if block_region[p] == 0
                        && block_region[b] == 0
                        && (type_safe_out[p] & mask) != 0
                        && (term_kills[p] & mask) == 0
                    {
                        if !jump_insert.contains(&(p, b)) {
                            jump_insert.push((p, b));
                        }
                    } else {
                        aborted = true;
                        break 'plan_loop;
                    }
                }
            }
        }
        if aborted {
            continue;
        }

        // Forward MUST verification: verify `temp_reg` availability across the
        // entire CFG given the planned compensation insertions and original
        // evaluations of `e`.  Any original evaluation where `temp_reg` is
        // already available becomes a `Move(dst, temp_reg)`; any other becomes
        // a capture (`Move(temp_reg, dst)` right after the original insn).
        let mut t_in = vec![false; num_blocks];
        let mut t_out = vec![false; num_blocks];
        for b in 1..num_blocks {
            if type_in[b].is_some() {
                t_in[b] = true;
                t_out[b] = true;
            }
        }
        let edge_has_insert = |p: usize, b: usize| -> bool {
            let p_last = blocks[p].1 - 1;
            let b_start = blocks[b].0;
            (ft_insert[p] && p_last + 1 == b_start) || jump_insert.contains(&(p, b))
        };

        t_out[0] = (comp[0] & mask) != 0 || exit_insert[0];
        loop {
            let mut changed = false;
            for b in 0..num_blocks {
                if type_in[b].is_none() {
                    continue;
                }
                let new_in = if b == 0 {
                    false
                } else {
                    let mut all_ok = true;
                    let mut any_p = false;
                    for &p in &preds[b] {
                        if type_in[p].is_none() {
                            continue;
                        }
                        any_p = true;
                        let edge_avail = safe_edge(p, b) && (t_out[p] || edge_has_insert(p, b));
                        if !edge_avail {
                            all_ok = false;
                            break;
                        }
                    }
                    any_p && all_ok
                };
                let body_transp = (transp_body[b] & mask) != 0;
                let term_ok = (term_kills[b] & mask) == 0;
                let new_out = ((comp[b] & mask) != 0
                    || (new_in && body_transp && term_ok)
                    || (exit_insert[b] && term_ok))
                    && (type_safe_out[b] & mask) != 0;
                if new_in != t_in[b] || new_out != t_out[b] {
                    t_in[b] = new_in;
                    t_out[b] = new_out;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        // Classify every reachable evaluation of `e` as either a replacement
        // (strictly redundant!) or a capture.
        let mut replace_pcs = Vec::new();
        let mut capture_pcs = Vec::new();
        for (b, &(b_start, b_end)) in blocks.iter().enumerate() {
            let Some(mut state) = type_in[b].clone() else {
                continue;
            };
            let in_try = block_region[b] != 0;
            let has_term = sccp_ends_basic_block(&insns[b_end - 1]);
            let body_end = if has_term { b_end - 1 } else { b_end };
            let mut avail = t_in[b];

            for pc in b_start..body_end {
                let insn = &insns[pc];
                let is_this_expr = pc_expr[pc].is_some_and(|(_, e)| e == expr);
                if is_this_expr {
                    if avail {
                        replace_pcs.push(pc);
                    } else {
                        capture_pcs.push(pc);
                    }
                }
                let (pure, may_raise) = pre_step_types(insn, &mut state, consts, num_locals);
                let mut killed = false;
                if in_try && may_raise {
                    killed = true;
                } else {
                    if !pure && expr.reads_named_local(num_locals) {
                        killed = true;
                    }
                    for_each_write(insn, |w| {
                        if expr.reads_reg(w) {
                            killed = true;
                        }
                    });
                }
                if is_this_expr && !killed {
                    avail = true;
                } else if killed {
                    avail = false;
                }
            }
        }

        // Profitability & soundness guard: commit only when at least one
        // original evaluation is genuinely replaced by `Move(dst, temp_reg)`.
        if replace_pcs.is_empty() {
            continue;
        }

        let temp_reg = next_temp;
        next_temp += 1;
        let exit_blocks: Vec<usize> = exit_insert
            .into_iter()
            .enumerate()
            .filter_map(|(b, flag)| flag.then_some(b))
            .collect();
        let fallthrough_edges: Vec<usize> = ft_insert
            .into_iter()
            .enumerate()
            .filter_map(|(b, flag)| flag.then_some(b))
            .collect();

        plans.push((
            bit,
            ExprPlan {
                temp_reg,
                exit_blocks,
                fallthrough_edges,
                jump_edges: jump_insert,
                replace_pcs,
                capture_pcs,
            },
        ));
    }

    if plans.is_empty() {
        return insns;
    }

    // Step 8: Materialize the transformed instruction stream with accurate jump
    // offset rewriting and critical-edge trampolines (if any).
    *num_regs = (*num_regs).max(next_temp);

    let mut before_pc: Vec<Vec<Insn>> = vec![Vec::new(); n];
    let mut after_pc: Vec<Vec<Insn>> = vec![Vec::new(); n];
    let mut replace_map: HashMap<usize, Insn> = HashMap::new();
    let mut after_block_fallthrough: Vec<Vec<Insn>> = vec![Vec::new(); num_blocks];
    let mut trampolines: HashMap<(usize, usize), Vec<Insn>> = HashMap::new();

    for (bit, plan) in &plans {
        let expr = exprs[*bit];
        for &b in &plan.exit_blocks {
            let last_pc = blocks[b].1 - 1;
            if sccp_ends_basic_block(&insns[last_pc]) {
                before_pc[last_pc].push(expr.to_insn(plan.temp_reg));
            } else {
                after_pc[last_pc].push(expr.to_insn(plan.temp_reg));
            }
        }
        for &p in &plan.fallthrough_edges {
            after_block_fallthrough[p].push(expr.to_insn(plan.temp_reg));
        }
        for &(p, b) in &plan.jump_edges {
            trampolines
                .entry((p, b))
                .or_default()
                .push(expr.to_insn(plan.temp_reg));
        }
        for &pc in &plan.replace_pcs {
            let (dst, _) = pc_expr[pc].expect("replacement pc must hold candidate expr");
            replace_map.insert(pc, Insn::Move(dst, plan.temp_reg));
        }
        for &pc in &plan.capture_pcs {
            let (dst, _) = pc_expr[pc].expect("capture pc must hold candidate expr");
            // Skip copying back if `dst` is already `plan.temp_reg` (cannot
            // happen for freshly allocated temps, guarded for clarity).
            if dst != plan.temp_reg {
                after_pc[pc].push(Insn::Move(plan.temp_reg, dst));
            }
        }
    }

    // Build new instruction stream, recording `old_to_new[old_pc]` as the
    // entry point for any jump that targeted `old_pc`.
    // Also record `(new_idx, orig_old_pc, maybe_tramp_key)` for every jump
    // instruction whose relative offset must be rewritten.
    let mut out: Vec<Insn> = Vec::with_capacity(n + plans.len() * 4);
    let mut old_to_new = vec![0usize; n + 1];
    // `(new_pc, old_target_pc)` for normal jumps, and `(new_pc, (p, b))` for
    // jumps redirected to a critical-edge trampoline.
    let mut normal_jumps: Vec<(usize, usize)> = Vec::new();
    let mut tramp_jumps: Vec<(usize, (usize, usize))> = Vec::new();

    for (b, &(b_start, b_end)) in blocks.iter().enumerate() {
        for pc in b_start..b_end {
            old_to_new[pc] = out.len();
            for extra in &before_pc[pc] {
                out.push(extra.clone());
            }
            let emitted = replace_map
                .get(&pc)
                .cloned()
                .unwrap_or_else(|| insns[pc].clone());
            let emitted_idx = out.len();
            if let Some(off) = insn_jump_off(&emitted) {
                let old_target = jump_target(pc, off);
                if old_target < n
                    && pc == b_end - 1
                    && trampolines.contains_key(&(b, pc_to_block[old_target]))
                {
                    tramp_jumps.push((emitted_idx, (b, pc_to_block[old_target])));
                } else {
                    normal_jumps.push((emitted_idx, old_target));
                }
            }
            out.push(emitted);
            for extra in &after_pc[pc] {
                out.push(extra.clone());
            }
        }
        // Fallthrough-edge compensation copies sit AFTER `b`'s conditional
        // terminator and BEFORE the next block's `old_to_new[b_end]`, so only
        // fallthrough from `b` executes them.
        for extra in &after_block_fallthrough[b] {
            out.push(extra.clone());
        }
    }
    old_to_new[n] = out.len();

    // If any critical-edge trampolines exist, ensure normal control cannot fall
    // off the end of the original function into the trampolines.
    let mut tramp_pc_map: HashMap<(usize, usize), usize> = HashMap::new();
    if !trampolines.is_empty() {
        let last_terminates = matches!(
            out.last(),
            Some(
                Insn::Jump(_)
                    | Insn::Return(_)
                    | Insn::ReturnNone
                    | Insn::RaiseValue(_)
                    | Insn::RaiseExceptStarResidual(_)
                    | Insn::RaiseFrom(..)
                    | Insn::RaiseReRaise
                    | Insn::RaiseAssert(_)
                    | Insn::RaiseAssertNoMsg
            )
        );
        let guard_jump_idx = if !last_terminates {
            let idx = out.len();
            out.push(Insn::Jump(0));
            Some(idx)
        } else {
            None
        };

        // Emit trampolines in deterministic order sorted by `(p, b)`.
        let mut keys: Vec<(usize, usize)> = trampolines.keys().copied().collect();
        keys.sort_unstable();
        for key @ (_p, target_b) in keys {
            let tramp_start = out.len();
            tramp_pc_map.insert(key, tramp_start);
            for extra in &trampolines[&key] {
                out.push(extra.clone());
            }
            let jmp_idx = out.len();
            out.push(Insn::Jump(0));
            normal_jumps.push((jmp_idx, blocks[target_b].0));
        }

        let past_end = out.len();
        if let Some(guard_idx) = guard_jump_idx {
            set_insn_jump_off(
                &mut out[guard_idx],
                (past_end as i64 - guard_idx as i64 - 1) as i32,
            );
        }
        old_to_new[n] = past_end;
    }

    // Rewrite all jump offsets to their new targets.
    for (new_pc, old_target) in normal_jumps {
        let new_target = old_to_new[old_target.min(n)];
        let new_off = (new_target as i64 - new_pc as i64 - 1) as i32;
        set_insn_jump_off(&mut out[new_pc], new_off);
    }
    for (new_pc, key) in tramp_jumps {
        let new_target = tramp_pc_map[&key];
        let new_off = (new_target as i64 - new_pc as i64 - 1) as i32;
        set_insn_jump_off(&mut out[new_pc], new_off);
    }

    out
}

/// Update the relative jump offset stored in a branching instruction.
fn set_insn_jump_off(insn: &mut Insn, new_off: i32) {
    use Insn::*;
    match insn {
        Jump(k)
        | JumpIfFalse(_, k)
        | JumpIfTrue(_, k)
        | CmpJumpIfFalse(_, _, _, k)
        | CmpJumpIfTrue(_, _, _, k)
        | CmpJumpIfFalseConst(_, _, _, k)
        | CmpJumpIfTrueConst(_, _, _, k)
        | ForIter(_, _, k)
        | SetupExcept(k)
        | MatchExcept(_, k)
        | MatchExceptStar(_, _, _, k) => *k = new_off,
        _ => {}
    }
}
