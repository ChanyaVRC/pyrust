//! Core SSA-based Mid-Level IR (MIR) data structures, type provenance, and
//! compile-time constant evaluation helpers.
//!
//! Every [`Val`] in a [`MirFn`] is defined at most once (either as a basic-block
//! parameter in [`MirBlock::params`] or as an instruction's [`MirInst::def`] or
//! a value-producing [`Terminator`]).  Control flow uses Cranelift-style block
//! parameters as SSA phi nodes, making dominance, use-def chains, and
//! [`MirType`] provenance explicit by construction (issue #457).

mod eval;
mod type_inference;

pub use eval::{
    eval_mir_binop, eval_mir_unary, is_mir_pure_const, is_proven_non_raising_binop,
    is_proven_non_raising_unary, mir_const_exact_eq, mir_const_truthy,
};
pub use type_inference::{MirType, infer_mir_types};

use std::cell::{OnceCell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use smallvec::SmallVec;

use crate::ast::{BinaryOp, CaretSpan, Symbol, UnaryOp};
use crate::bytecode::{CellSlotMeta, FnProto, Insn, Reg};
use crate::value::Value;

/// SSA value identifier, defined at most once within a [`MirFn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Val(pub u32);

/// Basic block identifier within a [`MirFn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u32);

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

macro_rules! for_each_term_edge {
    ($term:expr, $f:expr) => {
        match $term {
            Terminator::Jump { target, args } => ($f)(target, args),
            Terminator::Branch {
                then_bb,
                then_args,
                else_bb,
                else_args,
                ..
            } => {
                ($f)(then_bb, then_args);
                ($f)(else_bb, else_args);
            }
            Terminator::ForIter {
                body_bb,
                body_args,
                exit_bb,
                exit_args,
                ..
            } => {
                ($f)(body_bb, body_args);
                ($f)(exit_bb, exit_args);
            }
            Terminator::SetupExcept {
                try_bb,
                try_args,
                handler_bb,
                handler_args,
            } => {
                ($f)(try_bb, try_args);
                ($f)(handler_bb, handler_args);
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
                ($f)(matched_bb, matched_args);
                ($f)(nomatch_bb, nomatch_args);
            }
            Terminator::Return(_)
            | Terminator::Raise(_)
            | Terminator::RaiseSpecial { .. }
            | Terminator::Unreachable => {}
        }
    };
}

macro_rules! for_each_term_direct_use {
    ($term:expr, $f:expr) => {
        match $term {
            Terminator::Branch { cond, .. } => ($f)(cond),
            Terminator::Return(v) | Terminator::Raise(Some(v)) => ($f)(v),
            Terminator::RaiseSpecial { uses, .. } => {
                for v in uses {
                    ($f)(v);
                }
            }
            Terminator::MatchExcept { type_val, .. } => ($f)(type_val),
            Terminator::MatchExceptStar {
                type_val,
                src_group,
                ..
            } => {
                ($f)(type_val);
                ($f)(src_group);
            }
            Terminator::Jump { .. }
            | Terminator::Raise(None)
            | Terminator::ForIter { .. }
            | Terminator::SetupExcept { .. }
            | Terminator::Unreachable => {}
        }
    };
}

impl Terminator {
    /// Visit every SSA [`Val`] read by this terminator.
    pub fn for_each_use(&self, mut f: impl FnMut(Val)) {
        for_each_term_direct_use!(self, |v: &Val| f(*v));
        self.for_each_edge(|_, args| {
            for &v in args {
                f(v);
            }
        });
    }

    /// Rewrite every SSA [`Val`] read by this terminator via `f`.
    pub fn remap_uses(&mut self, mut f: impl FnMut(Val) -> Val) {
        for_each_term_direct_use!(&mut *self, |v: &mut Val| *v = f(*v));
        self.for_each_edge_mut(|_, args| {
            for v in args {
                *v = f(*v);
            }
        });
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
        for_each_term_edge!(self, |bb: &BlockId, args: &'a Vec<Val>| f(*bb, args));
    }

    /// Mutably visit every outgoing `(target_block, edge_args)` CFG edge.
    pub fn for_each_edge_mut(&mut self, mut f: impl FnMut(&mut BlockId, &mut Vec<Val>)) {
        for_each_term_edge!(self, |bb: &mut BlockId, args: &mut Vec<Val>| f(bb, args));
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

macro_rules! for_each_op_use {
    ($op:expr, $f:expr) => {
        match $op {
            MirOp::Const(_)
            | MirOp::LoadLocal(_)
            | MirOp::LoadGlobal { .. }
            | MirOp::LoadCell { .. } => {}
            MirOp::Copy(v)
            | MirOp::UnaryOp { arg: v, .. }
            | MirOp::StoreLocal { val: v, .. }
            | MirOp::StoreGlobal { val: v, .. }
            | MirOp::StoreCell { val: v, .. }
            | MirOp::GetAttr { obj: v, .. } => ($f)(v),
            MirOp::BinOp { lhs, rhs, .. }
            | MirOp::GetItem {
                obj: lhs, key: rhs, ..
            }
            | MirOp::SetAttr {
                obj: lhs, val: rhs, ..
            } => {
                ($f)(lhs);
                ($f)(rhs);
            }
            MirOp::SetItem { obj, key, val } => {
                ($f)(obj);
                ($f)(key);
                ($f)(val);
            }
            MirOp::Call { callee, args, .. } => {
                ($f)(callee);
                for a in args {
                    ($f)(a);
                }
            }
            MirOp::BuildTuple(elems) | MirOp::BuildList(elems) => {
                for e in elems {
                    ($f)(e);
                }
            }
            MirOp::SideEffectInsn { uses, .. } => {
                for u in uses {
                    ($f)(u);
                }
            }
        }
    };
}

impl MirInst {
    /// Visit every SSA [`Val`] read by this instruction.
    pub fn for_each_use(&self, mut f: impl FnMut(Val)) {
        for_each_op_use!(&self.op, |v: &Val| f(*v));
    }

    /// Rewrite every SSA [`Val`] read by this instruction via `f`.
    pub fn remap_uses(&mut self, mut f: impl FnMut(Val) -> Val) {
        for_each_op_use!(&mut self.op, |v: &mut Val| *v = f(*v));
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

/// Build an empty `RefCell`-initialized inline cache vector of length `len`.
#[inline]
pub(crate) fn empty_cache_vec<T: Clone>(len: usize, val: T) -> RefCell<Vec<T>> {
    RefCell::new(vec![val; len])
}
