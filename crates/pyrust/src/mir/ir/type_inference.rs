//! Compile-time SSA [`MirType`] provenance lattice and optimistic fixpoint
//! type inference.

use smallvec::SmallVec;

use crate::ast::{BinaryOp, UnaryOp};
use crate::value::{Value, ValueKind};

use super::eval::{eval_mir_binop, eval_mir_unary, is_mir_pure_const, mir_const_exact_eq};
use super::{MirFn, MirOp, Val};

/// Compile-time SSA type provenance lattice derived strictly from each [`Val`]'s
/// single defining operation (or block-parameter incoming join).
///
/// Crucially, [`MirType::Int`] represents exact Python `int` (`Int` or `BigInt`),
/// **never** `bool` (since `type(True + 0) is int`, not `bool`).
#[derive(Debug, Clone, PartialEq)]
pub enum MirType {
    /// No compile-time type guarantee (e.g. parameters, globals, user objects).
    Unknown,
    /// Proven exact `int` (`ValueKind::Int` or `ValueKind::BigInt`, never `bool`).
    Int,
    /// Proven exact `float`.
    Float,
    /// Proven exact `bool`.
    Bool,
    /// Proven exact `str`.
    Str,
    /// Proven `None`.
    None,
    /// Proven exact `tuple`.
    Tuple,
    /// Proven compile-time constant primitive value.
    ExactConst(Value),
}

impl MirType {
    /// Derive the tightest `MirType` for a compile-time constant `Value`.
    pub fn from_const(val: &Value) -> Self {
        if is_mir_pure_const(val) {
            MirType::ExactConst(val.clone())
        } else {
            MirType::Unknown
        }
    }

    /// True if this value is proven to be an exact Python `int` (not `bool`).
    #[inline]
    pub fn is_int(&self) -> bool {
        matches!(self, MirType::Int)
            || matches!(self, MirType::ExactConst(v) if matches!(v.kind(), ValueKind::Int(_) | ValueKind::BigInt(_)))
    }

    /// True if this value is proven to be an exact Python `float`.
    #[inline]
    pub fn is_float(&self) -> bool {
        matches!(self, MirType::Float)
            || matches!(self, MirType::ExactConst(v) if matches!(v.kind(), ValueKind::Float(_)))
    }

    /// True if this value is proven to be an exact Python `str`.
    #[inline]
    pub fn is_str(&self) -> bool {
        matches!(self, MirType::Str)
            || matches!(self, MirType::ExactConst(v) if matches!(v.kind(), ValueKind::Str(_)))
    }

    /// True if this value is proven to be a built-in primitive whose truthiness
    /// (`bool(v)`) is pure and non-raising.
    #[inline]
    pub fn is_pure_truthy_primitive(&self) -> bool {
        matches!(
            self,
            MirType::Int | MirType::Float | MirType::Bool | MirType::Str | MirType::None
        ) || matches!(self, MirType::ExactConst(v) if is_mir_pure_const(v))
    }

    /// Return the exact constant `Value` if known.
    #[inline]
    pub fn as_const(&self) -> Option<&Value> {
        match self {
            MirType::ExactConst(v) => Some(v),
            _ => None,
        }
    }

    /// Coarse type tag ignoring exact constant payload, used for lattice joins.
    fn coarse(&self) -> CoarseType {
        match self {
            MirType::Unknown => CoarseType::Unknown,
            MirType::Int => CoarseType::Int,
            MirType::Float => CoarseType::Float,
            MirType::Bool => CoarseType::Bool,
            MirType::Str => CoarseType::Str,
            MirType::None => CoarseType::None,
            MirType::Tuple => CoarseType::Tuple,
            MirType::ExactConst(v) => match v.kind() {
                ValueKind::Int(_) | ValueKind::BigInt(_) => CoarseType::Int,
                ValueKind::Float(_) => CoarseType::Float,
                ValueKind::Bool(_) => CoarseType::Bool,
                ValueKind::Str(_) => CoarseType::Str,
                ValueKind::None => CoarseType::None,
                ValueKind::Tuple(_) => CoarseType::Tuple,
                _ => CoarseType::Unknown,
            },
        }
    }

    /// Lattice join of two [`MirType`] values at a CFG merge.
    pub fn join(&self, other: &Self) -> Self {
        if let (MirType::ExactConst(a), MirType::ExactConst(b)) = (self, other)
            && mir_const_exact_eq(a, b)
        {
            return MirType::ExactConst(a.clone());
        }
        match (self.coarse(), other.coarse()) {
            (CoarseType::Int, CoarseType::Int) => MirType::Int,
            (CoarseType::Float, CoarseType::Float) => MirType::Float,
            (CoarseType::Bool, CoarseType::Bool) => MirType::Bool,
            (CoarseType::Str, CoarseType::Str) => MirType::Str,
            (CoarseType::None, CoarseType::None) => MirType::None,
            (CoarseType::Tuple, CoarseType::Tuple) => MirType::Tuple,
            _ => MirType::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoarseType {
    Unknown,
    Int,
    Float,
    Bool,
    Str,
    None,
    Tuple,
}

fn derive_op_type(op: &MirOp, lattice: &[Option<MirType>]) -> Option<MirType> {
    let get = |v: Val| lattice.get(v.0 as usize).and_then(|o| o.as_ref());
    match op {
        MirOp::Const(v) => Some(MirType::from_const(v)),
        MirOp::Copy(v) => get(*v).cloned(),
        MirOp::BinOp { op, lhs, rhs, .. } => {
            let lt = get(*lhs)?;
            let rt = get(*rhs)?;
            if let (Some(lv), Some(rv)) = (lt.as_const(), rt.as_const())
                && let Some(folded) = eval_mir_binop(lv, *op, rv)
            {
                return Some(MirType::from_const(&folded));
            }
            use BinaryOp::*;
            Some(if lt.is_int() && rt.is_int() {
                match op {
                    Add | Sub | Mul | FloorDiv | Mod | BitAnd | BitOr | BitXor | LShift
                    | RShift => MirType::Int,
                    Div => MirType::Float,
                    Eq | Ne | Lt | Le | Gt | Ge => MirType::Bool,
                    _ => MirType::Unknown,
                }
            } else if (lt.is_float() && (rt.is_float() || rt.is_int()))
                || (lt.is_int() && rt.is_float())
            {
                match op {
                    Add | Sub | Mul | Div | FloorDiv | Mod => MirType::Float,
                    Eq | Ne | Lt | Le | Gt | Ge => MirType::Bool,
                    _ => MirType::Unknown,
                }
            } else if lt.is_str() && rt.is_str() {
                match op {
                    Add => MirType::Str,
                    Eq | Ne | Lt | Le | Gt | Ge => MirType::Bool,
                    _ => MirType::Unknown,
                }
            } else {
                MirType::Unknown
            })
        }
        MirOp::UnaryOp { op, arg } => {
            let at = get(*arg)?;
            if let Some(av) = at.as_const()
                && let Some(folded) = eval_mir_unary(*op, av)
            {
                return Some(MirType::from_const(&folded));
            }
            Some(match op {
                UnaryOp::Not => MirType::Bool,
                UnaryOp::Neg | UnaryOp::Pos | UnaryOp::BitNot if at.is_int() => MirType::Int,
                UnaryOp::Neg | UnaryOp::Pos if at.is_float() => MirType::Float,
                _ => MirType::Unknown,
            })
        }
        MirOp::BuildTuple(_) => Some(MirType::Tuple),
        _ => Some(MirType::Unknown),
    }
}

/// Compute SSA [`MirType`] provenance for every [`Val`] in `func` using an
/// optimistic fixpoint over block parameters (so loop counters like
/// `i = 0; i = i + 1` converge to `MirType::Int`).
pub fn infer_mir_types(func: &mut MirFn) {
    let nvals = func.next_val as usize;
    // `None` = optimistic Top (unvisited); `Some(ty)` = proven type.
    let mut lattice: Vec<Option<MirType>> = vec![None; nvals];

    let join_opt = |a: &Option<MirType>, b: &Option<MirType>| -> Option<MirType> {
        match (a, b) {
            (None, x) | (x, None) => x.clone(),
            (Some(ta), Some(tb)) => Some(ta.join(tb)),
        }
    };

    // Build incoming arg vectors for every block parameter: param_idx -> Vec<Val>.
    let mut param_in: Vec<(Val, SmallVec<[Val; 4]>)> = Vec::new();
    let mut param_index: Vec<Option<usize>> = vec![None; nvals];
    for block in &func.blocks {
        for &p in &block.params {
            if (p.0 as usize) < nvals {
                param_index[p.0 as usize] = Some(param_in.len());
                param_in.push((p, SmallVec::new()));
            }
        }
    }
    for block in &func.blocks {
        block.term.for_each_edge(|target, args| {
            if let Some(tb) = func.blocks.get(target.0 as usize) {
                for (&p, &a) in tb.params.iter().zip(args.iter()) {
                    if let Some(idx) = param_index.get(p.0 as usize).copied().flatten() {
                        param_in[idx].1.push(a);
                    }
                }
            }
        });
    }

    // Iterate to fixpoint (bounded to prevent pathological oscillation).
    // Keep optimistic `Top` (`None`) unpolluted until an operand has a non-`Top`
    // lattice value so loop counters (`i = 0; i = i + 1`) converge to `MirType::Int`.
    for _ in 0..16 {
        let mut changed = false;

        // 1. Update block parameter types from incoming edges.
        for (p, incoming) in &param_in {
            let mut acc: Option<MirType> = None;
            if incoming.is_empty() {
                acc = Some(MirType::Unknown);
            } else {
                for &src in incoming {
                    if let Some(src_ty) = lattice.get(src.0 as usize) {
                        acc = join_opt(&acc, src_ty);
                    } else {
                        acc = Some(MirType::Unknown);
                    }
                }
            }
            if lattice[p.0 as usize] != acc {
                lattice[p.0 as usize] = acc;
                changed = true;
            }
        }

        // 2. Update instruction and terminator defs.
        for block in &func.blocks {
            for inst in &block.instrs {
                if let Some(def) = inst.def {
                    let next = derive_op_type(&inst.op, &lattice);
                    if lattice[def.0 as usize] != next {
                        lattice[def.0 as usize] = next;
                        changed = true;
                    }
                }
                if let MirOp::SideEffectInsn { defs, .. } = &inst.op {
                    for &d in defs {
                        if Some(d) != inst.def && lattice[d.0 as usize] != Some(MirType::Unknown) {
                            lattice[d.0 as usize] = Some(MirType::Unknown);
                            changed = true;
                        }
                    }
                }
            }
            block.term.for_each_def(|d| {
                if lattice[d.0 as usize] != Some(MirType::Unknown) {
                    lattice[d.0 as usize] = Some(MirType::Unknown);
                    changed = true;
                }
            });
        }

        if !changed {
            break;
        }
    }

    func.val_types = lattice
        .into_iter()
        .map(|opt| opt.unwrap_or(MirType::Unknown))
        .collect();
}
