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
    crate::compiler::compile_script_with_linenos(&stmts, local_index, false, &[], "<test>").unwrap()
}

/// Like `compile_fn`, but supplies a per-top-level-statement line-number
/// slice so the resulting `FnCode::lineno_table` is populated (the plain
/// `compile_script` path leaves it all zeros).
fn compile_script_with_linenos_for_test(src: &str, stmt_linenos: &[u32]) -> FnCode {
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
    crate::compiler::compile_script_with_linenos(&stmts, local_index, false, stmt_linenos, "<test>")
        .unwrap()
}

// ── pass_binop_const_fusion ───────────────────────────────────────────────

mod binding_safety;
mod cleanup;
mod concat_and_loop_inversion;
mod constant_folding;
mod cross_jump;
mod cse;
mod elimination;
mod int_loop_versioning;
mod load_none_merging;
mod loop_motion;
mod metadata;
mod recursive_calls;
