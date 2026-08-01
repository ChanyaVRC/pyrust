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

fn compile_source_with_positions(src: &str) -> FnCode {
    use crate::{interpreter::collect_local_names, lexer::Lexer, parser::Parser};

    let (tokens, line_nos, cols, cols_end) = Lexer::new(src).unwrap().into_tokens_with_pos();
    let mut parser = Parser::new_with_pos(tokens, line_nos, cols, cols_end);
    let (stmts, stmt_linenos) = parser.parse_program_with_linenos().unwrap();
    let empty = HashSet::new();
    let names = collect_local_names(&[], &stmts, &empty, &empty);
    let local_index = Rc::new(
        (0u32..)
            .zip(names)
            .map(|(slot, name)| (name, slot))
            .collect(),
    );
    compile_script_with_linenos(&stmts, local_index, false, &stmt_linenos, "<test>").unwrap()
}

fn compile_source_error(src: &str) -> String {
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
    match compile_script_with_linenos(&stmts, local_index, false, &[], "<test>") {
        Ok(_) => panic!("source must fail compilation"),
        Err(error) => error.to_string(),
    }
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
fn discarded_module_name_only_uses_checked_load_when_namespace_may_escape() {
    let assignment_only = compile_source_with_positions("value = 1\n");
    let unexposed = compile_source_with_positions("value = 1\nvalue\n");
    let value_name = unexposed
        .names
        .iter()
        .position(|name| name == "value")
        .expect("value name") as u16;
    assert_eq!(
        unexposed.insns.len(),
        assignment_only.insns.len(),
        "an unexposed namespace must retain zero-instruction discarded reads"
    );
    assert!(
        unexposed
            .insns
            .iter()
            .all(|insn| !matches!(insn, Insn::LoadGlobal(_, name) if *name == value_name)),
        "an unexposed namespace must retain the definitely-bound register fast path"
    );

    for source in [
        "value = 1\nglobals()\nvalue\n",
        "value = 1\nlocals()\nvalue\n",
        "value = 1\nvars()\nvalue\n",
        "value = 1\nexec('pass')\nvalue\n",
        "value = 1\neval('None')\nvalue\n",
        "value = 1\nexpose = globals\nvalue\n",
        "import sys\nvalue = 1\nsys._getframe\nvalue\n",
        "value = 1\nframe.f_locals\nvalue\n",
        "import sys\nvalue = 1\nsys._getframe().f_locals\nvalue\n",
        "value = 1\nframe.f_globals\nvalue\n",
        "value = 1\ndef expose():\n    globals()\nvalue\n",
        "value = 1\nclass Expose:\n    namespace = locals()\nvalue\n",
    ] {
        let exposed = compile_source_with_positions(source);
        let value_name = exposed
            .names
            .iter()
            .position(|name| name == "value")
            .expect("value name") as u16;
        let checked_load = exposed
            .insns
            .iter()
            .position(|insn| matches!(insn, Insn::LoadGlobal(_, name) if *name == value_name))
            .expect("an exposed module name must perform a checked global load");
        assert_eq!(
            exposed.lineno_table[checked_load],
            source.lines().count() as u32
        );
        assert_ne!(exposed.col_table[checked_load], (0, 0, 0, 0));
    }

    let consumed = compile_source("value = 1\nglobals()\ncopy = value\n");
    let value_name = consumed
        .names
        .iter()
        .position(|name| name == "value")
        .expect("value name") as u16;
    assert!(
        consumed
            .insns
            .iter()
            .all(|insn| !matches!(insn, Insn::LoadGlobal(_, name) if *name == value_name)),
        "consumed module reads must retain the definitely-bound register fast path"
    );

    let module = compile_source("globals()\ndef f():\n    value = 1\n    value\n");
    let function = module
        .fn_protos
        .iter()
        .find(|proto| proto.name.as_ref() == "f")
        .expect("f prototype");
    assert!(
        function
            .code
            .insns
            .iter()
            .all(|insn| !matches!(insn, Insn::LoadGlobal(..) | Insn::CheckLocal(..))),
        "function-scope discarded locals must retain the zero-instruction fast path"
    );
}

#[test]
fn namespace_accessor_aliases_gate_discarded_module_reads() {
    fn assert_checked(source: &str) {
        let code = compile_source_with_positions(source);
        let value_name = code
            .names
            .iter()
            .position(|name| name == "value")
            .expect("value name") as u16;
        assert!(
            code.insns
                .iter()
                .any(|insn| matches!(insn, Insn::LoadGlobal(_, name) if *name == value_name)),
            "namespace exposure syntax must check the final discarded read:\n{source}"
        );
    }

    for accessor in ["globals", "locals", "vars", "exec", "eval"] {
        assert_checked(&format!(
            "from builtins import {accessor} as expose\nvalue = 1\nvalue\n"
        ));
        assert_checked(&format!(
            "import builtins\nexpose = builtins.{accessor}\nvalue = 1\nvalue\n"
        ));
    }
    assert_checked("from sys import _getframe as get_frame\nvalue = 1\nvalue\n");
    assert_checked(
        "value = 1\ndef expose():\n    from builtins import globals as get_globals\nvalue\n",
    );
    assert_checked("import builtins as b\nexpose = getattr(b, 'globals')\nvalue = 1\nvalue\n");
    assert_checked("def owner():\n    pass\nnamespace = owner.__globals__\nvalue = 1\nvalue\n");
    assert_checked("import builtins as b\nexpose = b.__dict__['globals']\nvalue = 1\nvalue\n");
    assert_checked("expose = __builtins__['globals']\nvalue = 1\nvalue\n");

    for source in [
        "value = 1\nnamespace = holder.__dict__\nvalue\n",
        "value = 1\nnamespace = holder.__dict__['other']\nvalue\n",
        "from other import globals as expose\nvalue = 1\nvalue\n",
    ] {
        let unrelated = compile_source(source);
        let value_name = unrelated
            .names
            .iter()
            .position(|name| name == "value")
            .expect("value name") as u16;
        assert!(
            unrelated
                .insns
                .iter()
                .all(|insn| !matches!(insn, Insn::LoadGlobal(_, name) if *name == value_name)),
            "unrelated introspection/import syntax must not expose this namespace:\n{source}"
        );
    }
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
fn generic_variadic_call_uses_typed_vm_transport() {
    let code = compile_source(
        r#"def sink(*args, **kwargs):
    return args, kwargs
sink(*[1], *[2], answer=42)
"#,
    );

    assert!(
        code.names.iter().all(|name| name != "__vcall__"),
        "call syntax must not resolve an implementation helper through Python globals"
    );
    assert!(
        code.insns.iter().any(|insn| matches!(
            insn,
            Insn::CallExArgs {
                npos: 0,
                nkw: 0,
                ..
            }
        )),
        "the materialized positional list and keyword dict must use CallExArgs directly"
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
    for visitor in [
        "pub(crate) fn visit_type_parameter_scope_exprs(",
        "pub(crate) fn visit_deferred_annotation_exprs(",
        "pub(crate) fn visit_scope_dependency_exprs(",
    ] {
        assert!(
            ast.contains(visitor),
            "PEP 695 scope ownership must remain explicit via {visitor}"
        );
    }
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
fn lambda_default_nested_closure_promotes_the_enclosing_cell() {
    let module = compile_source(
        r#"def direct():
    x = 1
    return (lambda x=(lambda: x): x)()()

def through_class():
    x = 2
    class C:
        f = lambda y=(lambda: x): y
    return C.f()()

def through_definition_header():
    x = 3
    def nested(callback=(lambda: x)):
        return callback()
    return nested()
"#,
    );

    for function in ["direct", "through_class", "through_definition_header"] {
        let proto = module
            .fn_protos
            .iter()
            .find(|proto| proto.name.as_ref() == function)
            .unwrap_or_else(|| panic!("{function} prototype"));
        assert!(
            proto.code.cell_vars.iter().any(|name| name == "x"),
            "{function} must retain x as a cell when a lambda default creates a nested closure"
        );
    }
}

#[test]
fn comprehension_lambda_parameter_does_not_promote_shadowed_outer_name() {
    let module = compile_source(
        r#"def outer():
    x = 1
    return [lambda x: x for _ in ()]
"#,
    );
    let outer = module
        .fn_protos
        .iter()
        .find(|proto| proto.name.as_ref() == "outer")
        .expect("outer prototype");

    assert!(
        outer.code.cell_vars.iter().all(|name| name != "x"),
        "a lambda-local parameter nested in a comprehension must not capture outer x"
    );
}

#[test]
fn comprehension_iterable_rejects_assignment_expressions_across_nested_scopes() {
    let expected =
        "SyntaxError: assignment expression cannot be used in a comprehension iterable expression";
    for source in [
        "result = [i for i in (seen := ())]\n",
        "result = {i for i in (seen := ())}\n",
        "result = {i: i for i in (seen := ())}\n",
        "result = (i for i in (seen := ()))\n",
        "result = [j for i in () for j in (seen := ())]\n",
        "result = [i for i in (lambda: (seen := ()))()]\n",
        "result = [i for i in (lambda value=(seen := ()): ())()]\n",
        "result = [i for i in [seen := j for j in ()]]\n",
    ] {
        assert_eq!(compile_source_error(source), expected, "source: {source}");
    }

    // The prohibition belongs specifically to iterable syntax. Assignment
    // expressions in a result expression, including a nested lambda body, are
    // valid when they do not rebind an iteration variable.
    compile_source(
        "first = [(seen := i) for i in ()]\n\
         second = [(lambda: (inner := 1))() for i in ()]\n",
    );
}

#[test]
fn pep695_annotation_scopes_reject_direct_stateful_expressions() {
    for (source, expected) in [
        (
            "def f[T: (bound := int)]():\n    pass\n",
            "SyntaxError: named expression cannot be used within a TypeVar bound",
        ),
        (
            "def f[T: (yield int)]():\n    pass\n",
            "SyntaxError: yield expression cannot be used within a TypeVar bound",
        ),
        (
            "async def f[T: (await make_bound())]():\n    pass\n",
            "SyntaxError: await expression cannot be used within a TypeVar bound",
        ),
        (
            "type Alias = (value := int)\n",
            "SyntaxError: named expression cannot be used within a type alias",
        ),
        (
            "type Alias = (yield int)\n",
            "SyntaxError: yield expression cannot be used within a type alias",
        ),
        (
            "type Alias = (await make_value())\n",
            "SyntaxError: await expression cannot be used within a type alias",
        ),
        (
            "type Alias = [item for item in () if (seen := item)]\n",
            "SyntaxError: assignment expression within a comprehension cannot be used in a type alias",
        ),
        (
            "def f[T: [item for item in () if (seen := item)]]():\n    pass\n",
            "SyntaxError: assignment expression within a comprehension cannot be used in a TypeVar bound",
        ),
    ] {
        assert_eq!(compile_source_error(source), expected, "source: {source}");
    }
}

#[test]
fn pep695_validation_respects_lambda_and_comprehension_scope_boundaries() {
    // A nested lambda body is an ordinary function scope and may contain all
    // three expression forms. Its defaults remain in the annotation scope.
    compile_source(
        r#"type Alias = lambda: (value := int)
def f[T: (lambda: (yield int))]():
    pass
"#,
    );
    assert_eq!(
        compile_source_error("type Alias = lambda value=(seen := int): value\n"),
        "SyntaxError: named expression cannot be used within a type alias"
    );

    // The root comprehension's first iterable belongs to the annotation scope;
    // later iterables belong to the implicit comprehension function.
    assert_eq!(
        compile_source_error("type Alias = [item for item in (seen := ())]\n"),
        "SyntaxError: named expression cannot be used within a type alias"
    );
    assert_eq!(
        compile_source_error("type Alias = [inner for outer in () for inner in (seen := ())]\n"),
        "SyntaxError: assignment expression cannot be used in a comprehension iterable expression"
    );
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
