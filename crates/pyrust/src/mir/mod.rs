//! SSA-based Mid-Level IR (MIR) between AST/compilation and register bytecode
//! (issue #457).
//!
//! Architecture:
//! - [`ir`]: SSA value/block/op/terminator schema and [`MirType`] provenance lattice
//! - [`verify`]: Dominator tree + single-assignment / dominance / CFG verifier
//! - [`print`]: Clean Cranelift-style textual IR pretty-printer (`--emit mir`)
//! - [`ssa`]: CFG construction and Cytron-style SSA promotion (`FnCode` -> `MirFn`)
//! - [`passes`]: 8 SSA optimizer passes (SCCP, ConstFold, Algebraic, CopyProp,
//!   CSE/GVN, ConstBranchElim, DCE, and `#357` Reassociate)
//! - [`lower_bytecode`]: Register allocation, cycle-safe parallel-copy phi de-SSA,
//!   and exact `lineno_table`/`col_table` emission (`MirFn` -> `FnCode`)
//! - [`lower_ast`]: AST -> MIR entry points and `--emit mir` source driver

pub mod ir;
pub mod lower_ast;
pub mod lower_bytecode;
pub mod passes;
pub mod print;
pub mod ssa;
pub mod verify;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub use ir::{BlockId, MirBlock, MirFn, MirInst, MirOp, MirType, Terminator, Val};
#[allow(unused_imports)]
pub use lower_ast::{
    compile_source_to_mir, compile_source_to_mir_with_filename, compile_stmts_to_mir,
    lower_ast_to_mir,
};
#[allow(unused_imports)]
pub use lower_bytecode::lower_mir_to_fncode;
#[allow(unused_imports)]
pub use passes::{
    mir_algebraic_simplify, mir_const_branch_elim, mir_const_fold, mir_copy_prop, mir_cse, mir_dce,
    mir_reassociate, mir_sccp, run_mir_passes,
};
#[allow(unused_imports)]
pub use print::format_mir_fn;
#[allow(unused_imports)]
pub use ssa::lower_fncode_to_mir;
#[allow(unused_imports)]
pub use verify::verify_mir;

use crate::bytecode::FnCode;

/// Run the full SSA MIR optimization pipeline on `code`:
/// `FnCode -> SSA MirFn -> verify -> 8 SSA MIR passes -> verify -> FnCode`.
pub fn optimize_fncode_via_mir(code: FnCode) -> FnCode {
    let mut mir = lower_fncode_to_mir(&code);
    #[cfg(debug_assertions)]
    verify_mir(&mir).unwrap_or_else(|err| {
        panic!(
            "MIR verifier failed after SSA construction for '{}': {err}\n{}",
            mir.qualname,
            format_mir_fn(&mir)
        )
    });
    run_mir_passes(&mut mir);
    #[cfg(debug_assertions)]
    verify_mir(&mir).unwrap_or_else(|err| {
        panic!(
            "MIR verifier failed after SSA passes for '{}': {err}\n{}",
            mir.qualname,
            format_mir_fn(&mir)
        )
    });
    lower_mir_to_fncode(&mir)
}
