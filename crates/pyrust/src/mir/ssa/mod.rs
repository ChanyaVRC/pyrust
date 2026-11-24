//! CFG construction and SSA promotion (`FnCode` -> `MirFn`).
//!
//! Converts compiler-emitted register bytecode into true Cranelift-style SSA
//! MIR with block parameters as phi nodes (issue #457):
//! 1. [`cfg`]: Splits `FnCode::insns` into basic blocks with explicit CFG edges
//!    and `SetupExcept`/`PopExcept` exception-depth tracking.
//! 2. [`analysis`]: Computes register liveness and strict `definitely_bound`
//!    dataflow so block parameters are inserted only where a register is
//!    guaranteed defined along every incoming path, and places block parameters
//!    on dominance frontiers (Cytron et al.).
//! 3. [`construct`] & [`lift`]: Runs a single dominator-tree renaming pass with
//!    write-through `StoreLocal` for pinned canonical locals (`0..num_locals`)
//!    and exception-escaping temps, plus edge bridges for multi-pred
//!    terminator-defined edges.

mod analysis;
mod cfg;
mod construct;
mod lift;

use crate::bytecode::{FnCode, Insn};
use crate::value::Value;

use super::ir::{BlockId, MirBlock, MirFn, MirInst, MirOp, Terminator, infer_mir_types};

use analysis::analyze_and_place_phis;
use cfg::build_raw_cfg;
use construct::{SsaRenamer, insert_pinned_edge_bridges};

/// Lower a compiled [`FnCode`] into true SSA [`MirFn`] form.
pub fn lower_fncode_to_mir(code: &FnCode) -> MirFn {
    let insns = &code.insns;
    let num_locals = code.num_locals;
    let num_regs = (code.num_regs as usize).max(num_locals as usize);
    let aliased_locals = !code.is_function_scope
        || insns.iter().any(|i| {
            matches!(
                i,
                Insn::SyncModuleGlobal(..)
                    | Insn::RecordClassStore(..)
                    | Insn::RecordClassDel(..)
                    | Insn::LoadClassName(..)
            )
        });

    let mut mir = MirFn {
        qualname: code.qualname.clone(),
        filename: code.filename.clone(),
        first_lineno: code.first_lineno,
        blocks: Vec::new(),
        entry: BlockId(0),
        val_types: Vec::new(),
        next_val: 0,
        num_locals,
        num_iters: code.num_iters,
        consts: code.consts.clone(),
        names: code.names.clone(),
        descs: code.descs.clone(),
        cell_slots: code.cell_slots.clone(),
        own_cell_count: code.own_cell_count,
        fn_protos: code.fn_protos.clone(),
        free_var_candidates: code.free_var_candidates.clone(),
        is_generator: code.is_generator,
        is_coroutine: code.is_coroutine,
        is_class_method: code.is_class_method,
        is_function_scope: code.is_function_scope,
        is_inlined_comp: code.is_inlined_comp,
        comp_enclosing_locals: code.comp_enclosing_locals.clone(),
    };

    if insns.is_empty() {
        let v_none = mir.fresh_val();
        mir.blocks.push(MirBlock {
            id: BlockId(0),
            params: Vec::new(),
            instrs: vec![MirInst {
                def: Some(v_none),
                op: MirOp::Const(Value::none()),
                lineno: 0,
                col_span: (0, 0, 0, 0),
            }],
            term: Terminator::Return(v_none),
            term_lineno: 0,
            term_col_span: (0, 0, 0, 0),
            preds: Vec::new(),
            exc_depth: 0,
        });
        infer_mir_types(&mut mir);
        return mir;
    }

    let compact_raw = build_raw_cfg(insns);
    let analysis = analyze_and_place_phis(
        &compact_raw,
        insns,
        &code.descs,
        num_locals,
        num_regs,
        aliased_locals,
        &mut mir,
    );

    let mut renamer = SsaRenamer::new(
        &compact_raw,
        code,
        num_locals,
        num_regs,
        aliased_locals,
        &analysis,
        &mut mir,
    );
    renamer.rename_block(0);

    insert_pinned_edge_bridges(&compact_raw, insns, &code.descs, &analysis.pinned, &mut mir);
    mir.rebuild_preds();
    infer_mir_types(&mut mir);
    mir
}
