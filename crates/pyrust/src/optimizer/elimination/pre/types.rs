// ─── PRE types, safety classification, and forward type transfer ──────────────

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

type PreTypeMap = HashMap<u32, PreExactType>;

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

/// Basic-block CFG and exception-handler regions built for PRE.
struct PreCfg {
    has_except: bool,
    handler_pc_at: Vec<usize>,
    blocks: Vec<(usize, usize)>,
    pc_to_block: Vec<usize>,
    succs: Vec<Vec<usize>>,
    preds: Vec<Vec<usize>>,
    is_exit: Vec<bool>,
    block_region: Vec<u32>,
}

impl PreCfg {
    #[inline]
    fn num_blocks(&self) -> usize {
        self.blocks.len()
    }

    #[inline]
    fn safe_edge(&self, u: usize, v: usize) -> bool {
        self.block_region[u] == self.block_region[v]
    }
}

/// Candidate pure expressions discovered for PRE and their PC mapping.
struct PreDiscoveredExprs {
    exprs: Vec<PreExpr>,
    expr_to_bit: HashMap<PreExpr, usize>,
    pc_expr: Vec<Option<(u32, PreExpr)>>,
}

/// Per-block dataflow sets computed for the discovered candidate expressions.
struct PreDataflow {
    comp: Vec<u64>,
    transp_body: Vec<u64>,
    term_kills: Vec<u64>,
    type_safe_out: Vec<u64>,
    ant_in: Vec<u64>,
    ant_out: Vec<u64>,
    real_av_out: Vec<u64>,
    pav_in: Vec<u64>,
}

/// Verified compensation and replacement plan for one candidate expression.
struct PreExprPlan {
    temp_reg: u32,
    exit_blocks: Vec<usize>,
    fallthrough_edges: Vec<usize>,
    jump_edges: Vec<(usize, usize)>,
    replace_pcs: Vec<usize>,
    capture_pcs: Vec<usize>,
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
fn pre_expr_is_safe(expr: PreExpr, types: &PreTypeMap, consts: &[Value]) -> bool {
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
fn pre_extract_expr(insn: &Insn, types: &PreTypeMap, consts: &[Value]) -> Option<(u32, PreExpr)> {
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
    types: &mut PreTypeMap,
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
            (pure, insn_may_raise(insn))
        }
    }
}
