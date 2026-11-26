//! SSA MIR -> Register Bytecode (`FnCode`) backend.
//!
//! Lowers an optimized SSA [`MirFn`] back into executable [`FnCode`]:
//! 1. [`liveness`]: Computes exact SSA liveness (`live_in`/`live_out`) and
//!    RPO live intervals extended across loop back-edges.
//! 2. [`coalesce`] & [`regalloc`]: Allocates temporary registers
//!    (`num_locals..`) via linear scan with zero-copy coalescing for canonical
//!    locals, `ForIter` loop variables, named-local phis, and dying `Call`
//!    callees.
//! 3. [`de_ssa`]: De-SSAs Cranelift-style block parameters via cycle-safe
//!    parallel-copy sequentialization and critical-edge trampolines.
//! 4. [`emit`]: Emits register bytecode instructions and exact `lineno_table` /
//!    `col_table` entries from every `MirInst` and block terminator.

mod coalesce;
mod de_ssa;
mod emit;
mod emit_ops;
mod emit_term;
mod liveness;
mod regalloc;

#[allow(unused_imports)]
pub(crate) use de_ssa::emit_parallel_copies;

use std::cell::RefCell;

use crate::bytecode::{
    ColTable, FnCode, LinenoTable, MAX_FRAME_REGS, SiteCache, SparsePcCache, intern_const_pool,
};

use super::ir::MirFn;
use super::verify::DominatorTree;

use emit::emit_mir_bytecode;
use regalloc::allocate_ssa_registers;

/// Lower an SSA [`MirFn`] back into executable register bytecode [`FnCode`].
pub fn lower_mir_to_fncode(func: &MirFn) -> FnCode {
    let dom = DominatorTree::compute(func);
    let alloc = allocate_ssa_registers(func, &dom);
    let mut emitted = emit_mir_bytecode(func, &dom, alloc);
    intern_const_pool(&mut emitted.consts);

    let num_regs = (emitted.max_reg_used + 1)
        .max(func.num_locals)
        .min(MAX_FRAME_REGS);
    let global_cache_interest_masks = func
        .names
        .iter()
        .map(|name| crate::bytecode::global_cache_interest_mask(name))
        .collect();

    FnCode {
        insns: emitted.insns,
        descs: emitted.descs,
        filename: func.filename.clone(),
        qualname: func.qualname.clone(),
        lineno_table: LinenoTable::from_dense(&emitted.lineno_table),
        col_table: ColTable::from_dense(&emitted.col_table),
        first_lineno: func.first_lineno,
        consts: emitted.consts,
        names: func.names.clone(),
        num_regs,
        num_iters: func.num_iters,
        num_locals: func.num_locals,
        fn_protos: func.fn_protos.clone(),
        cell_slots: func.cell_slots.clone(),
        own_cell_count: func.own_cell_count,
        free_var_candidates: func.free_var_candidates.clone(),
        is_generator: func.is_generator,
        is_coroutine: func.is_coroutine,
        is_class_method: func.is_class_method,
        is_function_scope: func.is_function_scope,
        is_inlined_comp: func.is_inlined_comp,
        comp_enclosing_locals: func.comp_enclosing_locals.clone(),
        attr_cache: RefCell::new(SiteCache::new()),
        global_cache: RefCell::new(Vec::new()),
        global_cache_interest_masks,
        binop_cache: RefCell::new(Vec::new()),
        kwcall_cache: RefCell::new(SparsePcCache::new()),
        fmt_spec_cache: RefCell::new(SparsePcCache::new()),
        call_builtin_cache: RefCell::new(SiteCache::new()),
        cached_code_object: RefCell::new(None),
        exc_table: Vec::new(),
        has_exc_handlers: true,
        dead_temps: Default::default(),
    }
}
