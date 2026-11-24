//! Core SSA-based Mid-Level IR (MIR) data structures and type provenance.
//!
//! Every [`Val`] in a [`MirFn`] is defined at most once (either as a basic-block
//! parameter in [`MirBlock::params`] or as an instruction's [`MirInst::def`] or
//! a value-producing [`Terminator`]).  Control flow uses Cranelift-style block
//! parameters as SSA phi nodes, making dominance, use-def chains, and
//! [`MirType`] provenance explicit by construction (issue #457).

use std::cell::{OnceCell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use smallvec::SmallVec;

use crate::ast::{BinaryOp, CaretSpan, Symbol, UnaryOp};
use crate::bytecode::{CellSlotMeta, FnProto, Insn, Reg};
use crate::value::{Value, ValueKind};

/// SSA value identifier, defined at most once within a [`MirFn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Val(pub u32);

/// Basic block identifier within a [`MirFn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u32);

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

/// SSA MIR operation.
#[derive(Debug, Clone)]
pub enum MirOp {
    /// Materialize a compile-time constant value.
    Const(Value),
    /// Pure SSA copy of another value.
    Copy(Val),
    /// Binary operation (`is_inplace == true` for `+=`, `-=`, etc.).
    BinOp {
        op: BinaryOp,
        lhs: Val,
        rhs: Val,
        is_inplace: bool,
    },
    /// Unary operation (`-x`, `+x`, `not x`, `~x`).
    UnaryOp { op: UnaryOp, arg: Val },
    /// Read a canonical local register slot (`0..num_locals` or pinned reg).
    LoadLocal(Reg),
    /// Write an SSA value into a canonical local register slot (write-through).
    StoreLocal { reg: Reg, val: Val },
    /// Load a module global / builtin name (`FnCode::names[name_idx]`).
    LoadGlobal { name_idx: u16 },
    /// Store an SSA value to a module global (`FnCode::names[name_idx]`).
    StoreGlobal { name_idx: u16, val: Val },
    /// Load from an indexed closure cell (`cells[cell_idx]`).
    LoadCell { cell_idx: u16 },
    /// Store an SSA value into an indexed closure cell (`cells[cell_idx]`).
    StoreCell { cell_idx: u16, val: Val },
    /// Attribute lookup `obj.names[name_idx]`.
    GetAttr { obj: Val, name_idx: u16 },
    /// Attribute write `obj.names[name_idx] = val`.
    SetAttr { obj: Val, name_idx: u16, val: Val },
    /// Subscript lookup `obj[key]`.
    GetItem { obj: Val, key: Val },
    /// Subscript write `obj[key] = val`.
    SetItem { obj: Val, key: Val, val: Val },
    /// Positional call `callee(args...)` (`is_memo` preserves `CallMemo`).
    Call {
        callee: Val,
        args: Vec<Val>,
        is_memo: bool,
    },
    /// Construct a tuple from SSA values.
    BuildTuple(Vec<Val>),
    /// Construct a list from SSA values.
    BuildList(Vec<Val>),
    /// Typed fallback for specialized Python 3.12 VM instructions (`Yield`,
    /// `YieldFrom`, `MakeFunction`, `MakeClass`, `CallKw`, `CallMethod`,
    /// `BuildDict`, `Import*`, `PopExcept`, `EndExcept`, `CheckLocal`,
    /// `SyncModuleGlobal`, `RecordClassStore`, `Delete*`, `Unpack*`, etc.)
    /// with explicit SSA use-def edges so 100% of Python 3.12 constructs
    /// round-trip through MIR losslessly.
    SideEffectInsn {
        insn: Insn,
        uses: SmallVec<[Val; 4]>,
        defs: SmallVec<[Val; 2]>,
    },
}

/// Basic-block terminator with explicit CFG targets and Cranelift-style block
/// parameter arguments.
#[derive(Debug, Clone)]
pub enum Terminator {
    /// Unconditional jump to `target(args...)`.
    Jump { target: BlockId, args: Vec<Val> },
    /// Conditional branch on Python truthiness of `cond`.
    Branch {
        cond: Val,
        then_bb: BlockId,
        then_args: Vec<Val>,
        else_bb: BlockId,
        else_args: Vec<Val>,
    },
    /// Return `val` from the function.
    Return(Val),
    /// `raise val` (or bare `raise` when `None`).
    Raise(Option<Val>),
    /// Specialized raise (`RaiseFrom`, `RaiseAssert`, `RaiseAssertNoMsg`,
    /// `RaiseExceptStarResidual`).
    RaiseSpecial {
        insn: Insn,
        uses: SmallVec<[Val; 2]>,
    },
    /// `ForIter(dst, slot, exit_off)`: defines `dst` on the `body_bb` path,
    /// jumps to `exit_bb` when iterator `slot` is exhausted.
    ForIter {
        slot: u8,
        dst: Val,
        body_bb: BlockId,
        body_args: Vec<Val>,
        exit_bb: BlockId,
        exit_args: Vec<Val>,
    },
    /// `SetupExcept`: pushes `handler_bb` onto the active exception handler
    /// stack and continues to `try_bb(try_args)`.
    SetupExcept {
        try_bb: BlockId,
        try_args: Vec<Val>,
        handler_bb: BlockId,
        handler_args: Vec<Val>,
    },
    /// `MatchExcept`: tests active exception against `type_val`.
    MatchExcept {
        type_val: Val,
        matched_bb: BlockId,
        matched_args: Vec<Val>,
        nomatch_bb: BlockId,
        nomatch_args: Vec<Val>,
    },
    /// PEP 654 `MatchExceptStar`: filters `src_group` by `type_val`; on match
    /// defines `matched_def` and `residual_def` and transfers to `matched_bb`.
    MatchExceptStar {
        type_val: Val,
        src_group: Val,
        matched_def: Val,
        residual_def: Val,
        matched_bb: BlockId,
        matched_args: Vec<Val>,
        nomatch_bb: BlockId,
        nomatch_args: Vec<Val>,
    },
    /// Unreachable terminator (e.g. after dead branch pruning).
    Unreachable,
}

impl Terminator {
    /// Visit every SSA [`Val`] read by this terminator.
    pub fn for_each_use(&self, mut f: impl FnMut(Val)) {
        match self {
            Terminator::Jump { args, .. } => {
                for &v in args {
                    f(v);
                }
            }
            Terminator::Branch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                f(*cond);
                for &v in then_args {
                    f(v);
                }
                for &v in else_args {
                    f(v);
                }
            }
            Terminator::Return(v) => f(*v),
            Terminator::Raise(Some(v)) => f(*v),
            Terminator::Raise(None) | Terminator::Unreachable => {}
            Terminator::RaiseSpecial { uses, .. } => {
                for &v in uses {
                    f(v);
                }
            }
            Terminator::ForIter {
                body_args,
                exit_args,
                ..
            } => {
                for &v in body_args {
                    f(v);
                }
                for &v in exit_args {
                    f(v);
                }
            }
            Terminator::SetupExcept {
                try_args,
                handler_args,
                ..
            } => {
                for &v in try_args {
                    f(v);
                }
                for &v in handler_args {
                    f(v);
                }
            }
            Terminator::MatchExcept {
                type_val,
                matched_args,
                nomatch_args,
                ..
            } => {
                f(*type_val);
                for &v in matched_args {
                    f(v);
                }
                for &v in nomatch_args {
                    f(v);
                }
            }
            Terminator::MatchExceptStar {
                type_val,
                src_group,
                matched_args,
                nomatch_args,
                ..
            } => {
                f(*type_val);
                f(*src_group);
                for &v in matched_args {
                    f(v);
                }
                for &v in nomatch_args {
                    f(v);
                }
            }
        }
    }

    /// Rewrite every SSA [`Val`] read by this terminator via `f`.
    pub fn remap_uses(&mut self, mut f: impl FnMut(Val) -> Val) {
        match self {
            Terminator::Jump { args, .. } => {
                for v in args {
                    *v = f(*v);
                }
            }
            Terminator::Branch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                *cond = f(*cond);
                for v in then_args {
                    *v = f(*v);
                }
                for v in else_args {
                    *v = f(*v);
                }
            }
            Terminator::Return(v) => *v = f(*v),
            Terminator::Raise(Some(v)) => *v = f(*v),
            Terminator::Raise(None) | Terminator::Unreachable => {}
            Terminator::RaiseSpecial { uses, .. } => {
                for v in uses {
                    *v = f(*v);
                }
            }
            Terminator::ForIter {
                body_args,
                exit_args,
                ..
            } => {
                for v in body_args {
                    *v = f(*v);
                }
                for v in exit_args {
                    *v = f(*v);
                }
            }
            Terminator::SetupExcept {
                try_args,
                handler_args,
                ..
            } => {
                for v in try_args {
                    *v = f(*v);
                }
                for v in handler_args {
                    *v = f(*v);
                }
            }
            Terminator::MatchExcept {
                type_val,
                matched_args,
                nomatch_args,
                ..
            } => {
                *type_val = f(*type_val);
                for v in matched_args {
                    *v = f(*v);
                }
                for v in nomatch_args {
                    *v = f(*v);
                }
            }
            Terminator::MatchExceptStar {
                type_val,
                src_group,
                matched_args,
                nomatch_args,
                ..
            } => {
                *type_val = f(*type_val);
                *src_group = f(*src_group);
                for v in matched_args {
                    *v = f(*v);
                }
                for v in nomatch_args {
                    *v = f(*v);
                }
            }
        }
    }

    /// Visit every SSA [`Val`] defined by this terminator (e.g. `ForIter::dst`).
    pub fn for_each_def(&self, mut f: impl FnMut(Val)) {
        match self {
            Terminator::ForIter { dst, .. } => f(*dst),
            Terminator::MatchExceptStar {
                matched_def,
                residual_def,
                ..
            } => {
                f(*matched_def);
                f(*residual_def);
            }
            _ => {}
        }
    }

    /// Visit every outgoing `(target_block, edge_args)` CFG edge of this terminator.
    pub fn for_each_edge<'a>(&'a self, mut f: impl FnMut(BlockId, &'a [Val])) {
        match self {
            Terminator::Jump { target, args } => f(*target, args),
            Terminator::Branch {
                then_bb,
                then_args,
                else_bb,
                else_args,
                ..
            } => {
                f(*then_bb, then_args);
                f(*else_bb, else_args);
            }
            Terminator::ForIter {
                body_bb,
                body_args,
                exit_bb,
                exit_args,
                ..
            } => {
                f(*body_bb, body_args);
                f(*exit_bb, exit_args);
            }
            Terminator::SetupExcept {
                try_bb,
                try_args,
                handler_bb,
                handler_args,
            } => {
                f(*try_bb, try_args);
                f(*handler_bb, handler_args);
            }
            Terminator::MatchExcept {
                matched_bb,
                matched_args,
                nomatch_bb,
                nomatch_args,
                ..
            }
            | Terminator::MatchExceptStar {
                matched_bb,
                matched_args,
                nomatch_bb,
                nomatch_args,
                ..
            } => {
                f(*matched_bb, matched_args);
                f(*nomatch_bb, nomatch_args);
            }
            Terminator::Return(_)
            | Terminator::Raise(_)
            | Terminator::RaiseSpecial { .. }
            | Terminator::Unreachable => {}
        }
    }

    /// Mutably visit every outgoing `(target_block, edge_args)` CFG edge.
    pub fn for_each_edge_mut(&mut self, mut f: impl FnMut(&mut BlockId, &mut Vec<Val>)) {
        match self {
            Terminator::Jump { target, args } => f(target, args),
            Terminator::Branch {
                then_bb,
                then_args,
                else_bb,
                else_args,
                ..
            } => {
                f(then_bb, then_args);
                f(else_bb, else_args);
            }
            Terminator::ForIter {
                body_bb,
                body_args,
                exit_bb,
                exit_args,
                ..
            } => {
                f(body_bb, body_args);
                f(exit_bb, exit_args);
            }
            Terminator::SetupExcept {
                try_bb,
                try_args,
                handler_bb,
                handler_args,
            } => {
                f(try_bb, try_args);
                f(handler_bb, handler_args);
            }
            Terminator::MatchExcept {
                matched_bb,
                matched_args,
                nomatch_bb,
                nomatch_args,
                ..
            }
            | Terminator::MatchExceptStar {
                matched_bb,
                matched_args,
                nomatch_bb,
                nomatch_args,
                ..
            } => {
                f(matched_bb, matched_args);
                f(nomatch_bb, nomatch_args);
            }
            Terminator::Return(_)
            | Terminator::Raise(_)
            | Terminator::RaiseSpecial { .. }
            | Terminator::Unreachable => {}
        }
    }

    /// Collect all successor [`BlockId`]s of this terminator.
    pub fn successors(&self) -> SmallVec<[BlockId; 2]> {
        let mut out = SmallVec::new();
        self.for_each_edge(|bb, _| out.push(bb));
        out
    }
}

/// A single SSA MIR instruction carrying source line and PEP 657 caret span.
#[derive(Debug, Clone)]
pub struct MirInst {
    pub def: Option<Val>,
    pub op: MirOp,
    pub lineno: u32,
    pub col_span: CaretSpan,
}

impl MirInst {
    /// Visit every SSA [`Val`] read by this instruction.
    pub fn for_each_use(&self, mut f: impl FnMut(Val)) {
        match &self.op {
            MirOp::Const(_)
            | MirOp::LoadLocal(_)
            | MirOp::LoadGlobal { .. }
            | MirOp::LoadCell { .. } => {}
            MirOp::Copy(v)
            | MirOp::UnaryOp { arg: v, .. }
            | MirOp::StoreLocal { val: v, .. }
            | MirOp::StoreGlobal { val: v, .. }
            | MirOp::StoreCell { val: v, .. }
            | MirOp::GetAttr { obj: v, .. } => f(*v),
            MirOp::BinOp { lhs, rhs, .. }
            | MirOp::GetItem {
                obj: lhs, key: rhs, ..
            }
            | MirOp::SetAttr {
                obj: lhs, val: rhs, ..
            } => {
                f(*lhs);
                f(*rhs);
            }
            MirOp::SetItem { obj, key, val } => {
                f(*obj);
                f(*key);
                f(*val);
            }
            MirOp::Call { callee, args, .. } => {
                f(*callee);
                for &a in args {
                    f(a);
                }
            }
            MirOp::BuildTuple(elems) | MirOp::BuildList(elems) => {
                for &e in elems {
                    f(e);
                }
            }
            MirOp::SideEffectInsn { uses, .. } => {
                for &u in uses {
                    f(u);
                }
            }
        }
    }

    /// Rewrite every SSA [`Val`] read by this instruction via `f`.
    pub fn remap_uses(&mut self, mut f: impl FnMut(Val) -> Val) {
        match &mut self.op {
            MirOp::Const(_)
            | MirOp::LoadLocal(_)
            | MirOp::LoadGlobal { .. }
            | MirOp::LoadCell { .. } => {}
            MirOp::Copy(v)
            | MirOp::UnaryOp { arg: v, .. }
            | MirOp::StoreLocal { val: v, .. }
            | MirOp::StoreGlobal { val: v, .. }
            | MirOp::StoreCell { val: v, .. }
            | MirOp::GetAttr { obj: v, .. } => *v = f(*v),
            MirOp::BinOp { lhs, rhs, .. }
            | MirOp::GetItem {
                obj: lhs, key: rhs, ..
            }
            | MirOp::SetAttr {
                obj: lhs, val: rhs, ..
            } => {
                *lhs = f(*lhs);
                *rhs = f(*rhs);
            }
            MirOp::SetItem { obj, key, val } => {
                *obj = f(*obj);
                *key = f(*key);
                *val = f(*val);
            }
            MirOp::Call { callee, args, .. } => {
                *callee = f(*callee);
                for a in args {
                    *a = f(*a);
                }
            }
            MirOp::BuildTuple(elems) | MirOp::BuildList(elems) => {
                for e in elems {
                    *e = f(*e);
                }
            }
            MirOp::SideEffectInsn { uses, .. } => {
                for u in uses {
                    *u = f(*u);
                }
            }
        }
    }

    /// Visit every SSA [`Val`] defined by this instruction.
    pub fn for_each_def(&self, mut f: impl FnMut(Val)) {
        if let Some(d) = self.def {
            f(d);
        }
        if let MirOp::SideEffectInsn { defs, .. } = &self.op {
            for &d in defs {
                if Some(d) != self.def {
                    f(d);
                }
            }
        }
    }
}

/// Basic block in a [`MirFn`], using Cranelift-style block parameters for SSA phis.
#[derive(Debug, Clone)]
pub struct MirBlock {
    pub id: BlockId,
    pub params: Vec<Val>,
    pub instrs: Vec<MirInst>,
    pub term: Terminator,
    pub term_lineno: u32,
    pub term_col_span: CaretSpan,
    pub preds: Vec<BlockId>,
    pub exc_depth: u16,
}

/// SSA MIR function body together with the metadata required to lower back to
/// [`crate::bytecode::FnCode`].
#[derive(Debug, Clone)]
pub struct MirFn {
    pub qualname: Arc<str>,
    pub filename: Arc<str>,
    pub first_lineno: u32,
    pub blocks: Vec<MirBlock>,
    pub entry: BlockId,
    pub val_types: Vec<MirType>,
    pub next_val: u32,
    pub num_locals: u32,
    pub num_iters: u8,
    pub consts: Vec<Value>,
    pub names: Vec<Symbol>,
    pub cell_slots: SmallVec<[CellSlotMeta; 4]>,
    pub own_cell_count: u16,
    pub fn_protos: Vec<FnProto>,
    pub free_var_candidates: OnceCell<Vec<Symbol>>,
    pub is_generator: bool,
    pub is_coroutine: bool,
    pub is_class_method: bool,
    pub is_function_scope: bool,
    pub is_inlined_comp: bool,
    pub comp_enclosing_locals: Option<Rc<HashSet<Symbol>>>,
}

impl MirFn {
    /// Allocate a fresh SSA [`Val`] with initial type [`MirType::Unknown`].
    #[inline]
    pub fn fresh_val(&mut self) -> Val {
        let v = Val(self.next_val);
        self.next_val += 1;
        if self.val_types.len() <= v.0 as usize {
            self.val_types.resize(v.0 as usize + 1, MirType::Unknown);
        }
        v
    }

    /// Allocate a fresh SSA [`Val`] with a known [`MirType`].
    #[inline]
    pub fn fresh_val_typed(&mut self, ty: MirType) -> Val {
        let v = self.fresh_val();
        self.val_types[v.0 as usize] = ty;
        v
    }

    /// Look up the inferred [`MirType`] of `val`.
    #[inline]
    pub fn val_type(&self, val: Val) -> &MirType {
        self.val_types
            .get(val.0 as usize)
            .unwrap_or(&MirType::Unknown)
    }

    /// Recompute `preds` on every block from the current terminators.
    pub fn rebuild_preds(&mut self) {
        for block in &mut self.blocks {
            block.preds.clear();
        }
        let edges: Vec<(BlockId, BlockId)> = self
            .blocks
            .iter()
            .flat_map(|b| {
                let src = b.id;
                b.term
                    .successors()
                    .into_iter()
                    .map(move |dst| (src, dst))
                    .collect::<SmallVec<[_; 2]>>()
            })
            .collect();
        for (src, dst) in edges {
            if let Some(target) = self.blocks.get_mut(dst.0 as usize) {
                target.preds.push(src);
            }
        }
    }

    /// Create a minimal empty `MirFn` for unit testing passes directly.
    #[cfg(test)]
    pub fn empty_for_test(name: &str, num_locals: u32) -> Self {
        Self {
            qualname: Arc::from(name),
            filename: Arc::from("<mir-test>"),
            first_lineno: 1,
            blocks: Vec::new(),
            entry: BlockId(0),
            val_types: Vec::new(),
            next_val: 0,
            num_locals,
            num_iters: 0,
            consts: Vec::new(),
            names: Vec::new(),
            cell_slots: SmallVec::new(),
            own_cell_count: 0,
            fn_protos: Vec::new(),
            free_var_candidates: OnceCell::new(),
            is_generator: false,
            is_coroutine: false,
            is_class_method: false,
            is_function_scope: true,
            is_inlined_comp: false,
            comp_enclosing_locals: None,
        }
    }
}

/// True when `val` is an immutable, primitive compile-time constant whose
/// truthiness and pure operations never invoke user Python code.
#[inline]
pub fn is_mir_pure_const(val: &Value) -> bool {
    matches!(
        val.kind(),
        ValueKind::Int(_)
            | ValueKind::BigInt(_)
            | ValueKind::Float(_)
            | ValueKind::Complex(_, _)
            | ValueKind::Str(_)
            | ValueKind::Bytes(_)
            | ValueKind::Bool(_)
            | ValueKind::None
            | ValueKind::Tuple(_)
    )
}

/// Type-exact equality for compile-time constants (`Int(1) != Bool(true)`,
/// `-0.0 != +0.0`, `NaN == NaN` by bit pattern).
pub fn mir_const_exact_eq(a: &Value, b: &Value) -> bool {
    match (a.kind(), b.kind()) {
        (ValueKind::Int(x), ValueKind::Int(y)) => x == y,
        (ValueKind::BigInt(x), ValueKind::BigInt(y)) => x == y,
        (ValueKind::Float(x), ValueKind::Float(y)) => x.to_bits() == y.to_bits(),
        (ValueKind::Complex(xr, xi), ValueKind::Complex(yr, yi)) => {
            xr.to_bits() == yr.to_bits() && xi.to_bits() == yi.to_bits()
        }
        (ValueKind::Str(x), ValueKind::Str(y)) => x == y,
        (ValueKind::Bytes(x), ValueKind::Bytes(y)) => x == y,
        (ValueKind::Bool(x), ValueKind::Bool(y)) => x == y,
        (ValueKind::None, ValueKind::None) => true,
        (ValueKind::Tuple(xs), ValueKind::Tuple(ys)) => {
            xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys.iter())
                    .all(|(x, y)| mir_const_exact_eq(x, y))
        }
        _ => false,
    }
}

/// Compute pure Python truthiness of a primitive constant, returning `None`
/// for non-primitive values.
#[inline]
pub fn mir_const_truthy(val: &Value) -> Option<bool> {
    Some(match val.kind() {
        ValueKind::Bool(b) => b,
        ValueKind::Int(n) => n != 0,
        ValueKind::BigInt(n) => n.sign() != pyrust_core::PyBigIntSign::NoSign,
        ValueKind::Float(f) => f != 0.0,
        ValueKind::Complex(r, i) => r != 0.0 || i != 0.0,
        ValueKind::Str(s) => !s.is_empty(),
        ValueKind::Bytes(b) => !b.is_empty(),
        ValueKind::None => false,
        ValueKind::Tuple(t) => !t.is_empty(),
        _ => return None,
    })
}

/// Evaluate a pure binary operation between two compile-time constant `Value`s.
///
/// Delegates integer/float/string arithmetic and comparisons to the canonical
/// [`crate::compiler::fold_binop`] / `pyrust_core::NumericOps` protocol (issue
/// #458 / #421), returning `None` on any runtime error (e.g. division by zero)
/// so the operation stays in MIR and raises at runtime.
pub fn eval_mir_binop(lv: &Value, op: BinaryOp, rv: &Value) -> Option<Value> {
    if let Some(folded) = crate::compiler::fold_binop(lv, op, rv) {
        return Some(folded);
    }
    use BinaryOp::*;
    // Also support BigInt/Int mixed exact arithmetic via canonical dispatch so
    // chained overflow-promoted constants fold accurately (#421).
    if matches!(
        (lv.kind(), rv.kind()),
        (
            ValueKind::Int(_) | ValueKind::BigInt(_),
            ValueKind::Int(_) | ValueKind::BigInt(_)
        )
    ) {
        match op {
            Add | Sub | Mul | Div | FloorDiv | Mod | BitAnd | BitOr | BitXor => {
                return crate::interpreter::dispatch_numeric_binop(op, lv, rv)?.ok();
            }
            Eq | Ne | Lt | Le | Gt | Ge => {
                let cmp = crate::interpreter::binary_op_to_numeric_cmp(op)?;
                return pyrust_core::dispatch_numeric_compare(cmp, lv, rv)?
                    .ok()
                    .map(Value::bool_);
            }
            _ => {}
        }
    }
    match (lv.kind(), op, rv.kind()) {
        (ValueKind::Bool(a), Ne, ValueKind::Bool(b)) => Some(Value::bool_(a != b)),
        (ValueKind::None, Is, ValueKind::None) => Some(Value::bool_(true)),
        (ValueKind::None, IsNot, ValueKind::None) => Some(Value::bool_(false)),
        (ValueKind::None, Is, _) if is_mir_pure_const(rv) => Some(Value::bool_(false)),
        (ValueKind::None, IsNot, _) if is_mir_pure_const(rv) => Some(Value::bool_(true)),
        (_, Is, ValueKind::None) if is_mir_pure_const(lv) => Some(Value::bool_(false)),
        (_, IsNot, ValueKind::None) if is_mir_pure_const(lv) => Some(Value::bool_(true)),
        (ValueKind::None, Eq, ValueKind::None) => Some(Value::bool_(true)),
        (ValueKind::None, Ne, ValueKind::None) => Some(Value::bool_(false)),
        (ValueKind::Bool(a), Is, ValueKind::Bool(b)) => Some(Value::bool_(a == b)),
        (ValueKind::Bool(a), IsNot, ValueKind::Bool(b)) => Some(Value::bool_(a != b)),
        (ValueKind::Bool(_), Is, _) if is_mir_pure_const(rv) => Some(Value::bool_(false)),
        (ValueKind::Bool(_), IsNot, _) if is_mir_pure_const(rv) => Some(Value::bool_(true)),
        (_, Is, ValueKind::Bool(_)) if is_mir_pure_const(lv) => Some(Value::bool_(false)),
        (_, IsNot, ValueKind::Bool(_)) if is_mir_pure_const(lv) => Some(Value::bool_(true)),
        (ValueKind::Float(a), Eq, ValueKind::Float(b)) => Some(Value::bool_(a == b)),
        (ValueKind::Float(a), Ne, ValueKind::Float(b)) => Some(Value::bool_(a != b)),
        (ValueKind::Float(a), Lt, ValueKind::Float(b)) => Some(Value::bool_(a < b)),
        (ValueKind::Float(a), Le, ValueKind::Float(b)) => Some(Value::bool_(a <= b)),
        (ValueKind::Float(a), Gt, ValueKind::Float(b)) => Some(Value::bool_(a > b)),
        (ValueKind::Float(a), Ge, ValueKind::Float(b)) => Some(Value::bool_(a >= b)),
        (ValueKind::Str(a), Lt, ValueKind::Str(b)) => Some(Value::bool_(a < b)),
        (ValueKind::Str(a), Le, ValueKind::Str(b)) => Some(Value::bool_(a <= b)),
        (ValueKind::Str(a), Gt, ValueKind::Str(b)) => Some(Value::bool_(a > b)),
        (ValueKind::Str(a), Ge, ValueKind::Str(b)) => Some(Value::bool_(a >= b)),
        _ => None,
    }
}

/// Evaluate a pure unary operation on a compile-time constant using canonical
/// runtime unary semantics (issue #458).
#[inline]
pub fn eval_mir_unary(op: UnaryOp, v: &Value) -> Option<Value> {
    if !is_mir_pure_const(v) {
        return None;
    }
    crate::interpreter::eval_builtin_unary(op, v.clone()).ok()
}

/// True when `MirOp::BinOp` with operand types `(lt, rt)` is proven pure,
/// non-reentrant, and incapable of raising any Python exception.
pub fn is_proven_non_raising_binop(op: BinaryOp, lt: &MirType, rt: &MirType) -> bool {
    use BinaryOp::*;
    if lt.is_int() && rt.is_int() {
        return matches!(
            op,
            Add | Sub | Mul | BitAnd | BitOr | BitXor | Eq | Ne | Lt | Le | Gt | Ge
        );
    }
    if lt.is_float() && rt.is_float() {
        return matches!(op, Add | Sub | Mul | Eq | Ne | Lt | Le | Gt | Ge);
    }
    if lt.is_str() && rt.is_str() {
        return matches!(op, Add | Eq | Ne | Lt | Le | Gt | Ge);
    }
    false
}

/// True when `MirOp::UnaryOp` with operand type `arg_ty` is proven pure,
/// non-reentrant, and incapable of raising any Python exception.
pub fn is_proven_non_raising_unary(op: UnaryOp, arg_ty: &MirType) -> bool {
    match op {
        UnaryOp::Not => arg_ty.is_pure_truthy_primitive(),
        UnaryOp::Neg | UnaryOp::Pos | UnaryOp::BitNot => arg_ty.is_int(),
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
    // Crucially, keep optimistic `Top` (`None`) unpolluted until an operand has
    // a non-`Top` lattice value so loop counters (`i = 0; i = i + 1`) converge
    // to `MirType::Int` rather than prematurely dropping to `Unknown`.
    let derive_opt = |op: &MirOp, lattice: &[Option<MirType>]| -> Option<MirType> {
        let get = |v: Val| lattice.get(v.0 as usize).and_then(|o| o.as_ref());
        match op {
            MirOp::Const(v) => Some(MirType::from_const(v)),
            MirOp::Copy(v) => get(*v).cloned(),
            MirOp::BinOp {
                op,
                lhs,
                rhs,
                is_inplace: _,
            } => {
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
    };

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
                    let next = derive_opt(&inst.op, &lattice);
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

/// Build an empty `RefCell`-initialized inline cache vector of length `len`.
#[inline]
pub(crate) fn empty_cache_vec<T: Clone>(len: usize, val: T) -> RefCell<Vec<T>> {
    RefCell::new(vec![val; len])
}
