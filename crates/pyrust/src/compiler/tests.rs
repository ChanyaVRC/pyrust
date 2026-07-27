use super::*;

fn compile_source(src: &str) -> FnCode {
    use crate::{interpreter::collect_local_names, lexer::Lexer, parser::Parser};

    let tokens = Lexer::new(src).unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let stmts = parser.parse_program().unwrap();
    let empty = HashSet::new();
    let names = collect_local_names(&[], &stmts, &empty, &empty);
    let local_index = Rc::new(
        (0u32..)
            .zip(names)
            .map(|(slot, name)| (name, slot))
            .collect(),
    );
    compile_script_with_linenos(&stmts, local_index, false, &[], "<test>").unwrap()
}

#[test]
fn shared_module_namespace_mode_does_not_replace_script_fastlocals() {
    use crate::{lexer::Lexer, parser::Parser};

    let tokens = Lexer::new("x = 1\ny = x\n").unwrap().into_tokens();
    let mut parser = Parser::new(tokens);
    let stmts = parser.parse_program().unwrap();
    let local_index = Rc::new(
        [("x".to_string(), 0), ("y".to_string(), 1)]
            .into_iter()
            .collect(),
    );

    let script = compile_script_with_linenos(&stmts, local_index, false, &[], "<script>").unwrap();
    let shared = compile_shared_namespace_module_with_linenos(&stmts, &[], "<module>").unwrap();

    assert!(
        script
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::SyncModuleGlobal(..))),
        "ordinary scripts must retain module fastlocals"
    );
    assert!(
        !script
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::StoreGlobal(..))),
        "ordinary local stores must not be lowered to the shared-global path"
    );
    assert!(
        shared
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::StoreGlobal(..))),
        "externally reachable module bodies must write the shared namespace"
    );
    assert!(
        !shared
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::SyncModuleGlobal(..))),
        "shared module bodies must not retain a second register-backed binding"
    );
}

#[test]
fn syntactic_range_call_keeps_runtime_resolution_and_conditional_target_store() {
    let code = compile_source(
        r#"for target in range(0):
    pass
"#,
    );
    let target_name = code
        .names
        .iter()
        .position(|name| name == "target")
        .expect("target name") as u16;
    let for_iter = code
        .insns
        .iter()
        .position(|insn| matches!(insn, Insn::ForIter(..)))
        .expect("ordinary iterator loop");

    assert!(
        code.insns.iter().any(|insn| matches!(insn, Insn::Call(..))),
        "range must be resolved and called at runtime so shadowing is observable"
    );
    assert!(
        code.insns.iter().enumerate().all(|(index, insn)| {
            !matches!(insn, Insn::SyncModuleGlobal(_, name_idx) if *name_idx == target_name)
                || index > for_iter
        }),
        "the iteration target must only be published after ForIter yields a value"
    );
}

#[test]
fn source_shaped_counter_while_uses_generic_repeated_condition() {
    let code = compile_source(
        r#"i = 0
def stop(value):
    return value + 3
while i < stop(i):
    i += 1
"#,
    );

    assert!(
        code.insns
            .iter()
            .any(|insn| matches!(insn, Insn::Call(..) | Insn::CallMemo(..))),
        "the source stop expression must be called from the loop condition"
    );
    assert!(
        code.insns.iter().any(|insn| matches!(
            insn,
            Insn::JumpIfFalse(..) | Insn::CmpJumpIfFalse(..) | Insn::CmpJumpIfFalseConst(..)
        )),
        "the source comparison must remain the loop's conditional branch"
    );
    let i_name = code
        .names
        .iter()
        .position(|name| name == "i")
        .expect("i name") as u16;
    assert!(
        code.insns
            .iter()
            .filter(|insn| {
                matches!(insn, Insn::SyncModuleGlobal(_, name_idx) if *name_idx == i_name)
            })
            .count()
            >= 2,
        "both the initial binding and source-level loop increment must publish module i"
    );
}

#[test]
fn analysis_helpers_keep_explicit_source_ownership() {
    use std::path::Path;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let read = |relative: &str| {
        let path = root.join(relative);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()))
    };

    let facade = read("compiler.rs");
    let free_var_reads = read("compiler/free_var_reads.rs");
    let class_scope = read("compiler/class_scope_analysis.rs");
    let comprehension_analysis = read("compiler/comprehension_analysis.rs");
    let comprehensions = read("compiler/comprehensions.rs");
    let ast = read("ast.rs");

    assert!(
        facade.contains("include!(\"compiler/free_var_reads.rs\");"),
        "the compiler facade must explicitly compose the generic free-var owner"
    );
    for function in [
        "fn collect_free_var_reads_in_stmts(",
        "fn collect_free_var_reads_in_stmt(",
        "fn collect_free_var_reads_in_expr(",
    ] {
        assert!(
            free_var_reads.contains(function),
            "free_var_reads.rs must own {function}"
        );
        assert!(
            !class_scope.contains(function),
            "class_scope_analysis.rs must not own generic walker {function}"
        );
    }

    for function in [
        "fn collect_walrus_targets_in_expr(",
        "fn collect_walrus_targets_in_stmts(",
    ] {
        assert!(
            comprehension_analysis.contains(function),
            "comprehension_analysis.rs must own {function}"
        );
        assert!(
            !comprehensions.contains(function),
            "comprehensions.rs must remain compilation-only and not own {function}"
        );
    }

    assert!(
        ast.contains("pub(crate) fn visit_evaluated_exprs("),
        "AST target/pattern shape traversal must remain centralized"
    );
    for owner in [
        "compiler/free_var_reads.rs",
        "compiler/free_variables.rs",
        "compiler/comprehension_analysis.rs",
        "compiler/class_scope_analysis.rs",
        "compiler/model/yield_scan.rs",
        "interpreter/helpers/scope_analysis/bindings.rs",
        "interpreter/helpers/scope_analysis/declarations.rs",
    ] {
        assert!(
            read(owner).contains(".visit_evaluated_exprs("),
            "{owner} must consume centralized target/pattern expression traversal"
        );
    }
}

#[test]
fn split_analysis_owners_compile_class_free_vars_and_comprehension_walrus() {
    let module = compile_source(
        r#"def outer(seed):
    total = 0
    class C:
        read = lambda self: seed
    values = [(total := total + item) for item in range(3)]
    return C, values, total
"#,
    );
    let outer = module
        .fn_protos
        .iter()
        .find(|proto| proto.name.as_ref() == "outer")
        .expect("outer prototype");

    for expected in ["seed", "total"] {
        assert!(
            outer.code.cell_vars.iter().any(|name| name == expected),
            "{expected} must remain a captured cell after the analysis split"
        );
    }
}

#[test]
fn augmented_target_expression_shape_is_centralized() {
    use crate::{lexer::Lexer, parser::Parser};

    let tokens = Lexer::new(
        "holder.value += amount\nitems[position] += amount\nitems[lower:upper:stride] += []\n",
    )
    .unwrap()
    .into_tokens();
    let stmts = Parser::new(tokens).parse_program().unwrap();
    let mut reads = HashSet::new();
    for stmt in &stmts {
        let Stmt::AugAssign { target, .. } = stmt else {
            panic!("fixture must parse as augmented assignment");
        };
        target.visit_evaluated_exprs(&mut |expr| collect_free_var_reads_in_expr(expr, &mut reads));
    }

    assert_eq!(
        reads,
        ["holder", "items", "position", "lower", "upper", "stride"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        "receiver, key, and every slice bound are evaluated target reads"
    );
}

#[test]
fn augmented_targets_and_match_patterns_promote_enclosing_cells() {
    let module = compile_source(
        r#"def outer(holder, items, position, lower, upper, stride, owner):
    def mutate(amount):
        holder.value += amount
        items[position] += amount
        items[lower:upper:stride] += []
        match amount:
            case owner.expected:
                return 1
            case _:
                return amount
    return mutate
"#,
    );
    let outer = module
        .fn_protos
        .iter()
        .find(|proto| proto.name.as_ref() == "outer")
        .expect("outer prototype");

    for expected in [
        "holder", "items", "position", "lower", "upper", "stride", "owner",
    ] {
        assert!(
            outer.code.cell_vars.iter().any(|name| name == expected),
            "{expected} appears only in an evaluated target/pattern expression and must be captured"
        );
    }
}
