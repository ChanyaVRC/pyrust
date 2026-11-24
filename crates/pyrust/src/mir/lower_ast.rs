//! AST -> SSA MIR entry points and `--emit mir` driver.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::ast::Stmt;
use crate::bytecode::Reg;
use crate::error::PyError;
use crate::interpreter::{Interpreter, collect_local_names};

use super::ir::MirFn;
use super::passes::run_mir_passes;
use super::print::format_mir_fn;
use super::ssa::lower_fncode_to_mir;
#[cfg(debug_assertions)]
use super::verify::verify_mir;

/// Lower a parsed AST statement list into an unoptimized SSA [`MirFn`].
pub fn lower_ast_to_mir(stmts: &[Stmt], linenos: &[u32], filename: &str) -> Result<MirFn, PyError> {
    let empty: HashSet<String> = HashSet::new();
    let names = collect_local_names(&[], stmts, &empty, &empty);
    let local_index: Rc<HashMap<String, Reg>> = Rc::new(
        (0u32..)
            .zip(names.iter())
            .map(|(i, n)| (n.clone(), i))
            .collect(),
    );
    let fn_code =
        crate::compiler::compile_script_with_linenos(stmts, local_index, false, linenos, filename)?;
    let mir = lower_fncode_to_mir(&fn_code);
    #[cfg(debug_assertions)]
    verify_mir(&mir).map_err(|e| PyError::runtime(format!("MIR verifier error: {e}")))?;
    Ok(mir)
}

/// Compile AST statements through SSA MIR construction and all SSA MIR passes.
pub fn compile_stmts_to_mir(
    stmts: &[Stmt],
    linenos: &[u32],
    filename: &str,
) -> Result<MirFn, PyError> {
    let mut mir = lower_ast_to_mir(stmts, linenos, filename)?;
    run_mir_passes(&mut mir);
    #[cfg(debug_assertions)]
    verify_mir(&mir)
        .map_err(|e| PyError::runtime(format!("MIR verifier error after passes: {e}")))?;
    Ok(mir)
}

/// Format `mir` and recursively format all nested function / class prototypes
/// after running the SSA MIR optimization passes on each.
fn emit_mir_recursive(mir: &MirFn, out: &mut String) -> Result<(), PyError> {
    out.push_str(&format_mir_fn(mir));
    for proto in &mir.fn_protos {
        let mut child_mir = lower_fncode_to_mir(&proto.code);
        #[cfg(debug_assertions)]
        verify_mir(&child_mir).map_err(|e| {
            PyError::runtime(format!(
                "MIR verifier error in '{}' before passes: {e}",
                proto.qualname
            ))
        })?;
        run_mir_passes(&mut child_mir);
        #[cfg(debug_assertions)]
        verify_mir(&child_mir).map_err(|e| {
            PyError::runtime(format!(
                "MIR verifier error in '{}' after passes: {e}",
                proto.qualname
            ))
        })?;
        out.push('\n');
        emit_mir_recursive(&child_mir, out)?;
    }
    Ok(())
}

/// Parse Python source, lower to SSA MIR, run all MIR passes on the module and
/// every nested function/class, and return the pretty-printed MIR text.
///
/// Used by CLI `--emit mir` and Rust unit tests.
#[allow(dead_code)]
pub fn compile_source_to_mir(src: &str) -> Result<String, PyError> {
    compile_source_to_mir_with_filename(src, "<string>")
}

/// Like [`compile_source_to_mir`], recording `filename` on compiled code objects.
pub fn compile_source_to_mir_with_filename(src: &str, filename: &str) -> Result<String, PyError> {
    let (program, linenos) = Interpreter::parse_source_to_stmts_with_linenos(src)?;
    let mir = compile_stmts_to_mir(&program, &linenos, filename)?;
    let mut out = String::new();
    emit_mir_recursive(&mir, &mut out)?;
    Ok(out)
}
