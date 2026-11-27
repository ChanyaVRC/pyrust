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
fn dict_literals_encode_static_key_kind_without_runtime_scans() {
    for (source, expected) in [
        ("d = {'a': 1, f'b': 2}\n", DictKeyKindHint::Unicode),
        ("d = {name: 1}\n", DictKeyKindHint::Unknown),
        ("d = {name: 1, 2: 2}\n", DictKeyKindHint::General),
    ] {
        let code = compile_source(source);
        let hint = code.insns.iter().find_map(|insn| match insn {
            Insn::BuildDict(_, _, _, hint) => Some(*hint),
            _ => None,
        });
        assert_eq!(hint, Some(expected), "source: {source}");
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
    let shared = compile_shared_namespace_module_with_linenos(
        &stmts,
        &[],
        "<module>",
        CompileOptions::default(),
    )
    .unwrap();

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
        assert_eq!(exposed.line_at(checked_load), source.lines().count() as u32);
        assert!(exposed.col_at(checked_load).is_some());
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
    assert_checked(
        "import builtins as b\naccessors = b.__dict__\nexpose = accessors['globals']\nvalue = 1\nvalue\n",
    );
    assert_checked(
        "from builtins import __dict__ as accessors\nexpose = accessors['globals']\nvalue = 1\nvalue\n",
    );
    assert_checked("value = 1\ndef expose():\n    import builtins as b\nvalue\n");

    for source in [
        "value = 1\nnamespace = holder.__dict__\nvalue\n",
        "value = 1\nnamespace = holder.__dict__['other']\nvalue\n",
        "from other import globals as expose\nvalue = 1\nvalue\n",
        "import other as b\nvalue = 1\nvalue\n",
        "from other import __dict__ as accessors\nvalue = 1\nvalue\n",
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
        code.insns.iter().any(|insn| match insn {
            Insn::CallExArgs(desc_idx) => {
                let desc = code.descs[*desc_idx as usize].as_call_ex_args();
                desc.npos == 0 && desc.nkw == 0
            }
            _ => false,
        }),
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
    let comprehension_analysis = [
        read("compiler/comprehension_analysis.rs"),
        read("compiler/comprehension_analysis/captures.rs"),
        read("compiler/comprehension_analysis/validation.rs"),
    ]
    .join("\n");
    let comprehensions = [
        read("compiler/comprehensions.rs"),
        read("compiler/comprehensions/scope.rs"),
        read("compiler/comprehensions/inlined.rs"),
        read("compiler/comprehensions/genexpr.rs"),
    ]
    .join("\n");
    let ast = [
        read("ast/mod.rs"),
        read("ast/span.rs"),
        read("ast/expr.rs"),
        read("ast/stmt.rs"),
        read("ast/pattern.rs"),
    ]
    .join("\n");

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
            "comprehension_analysis must own {function}"
        );
        assert!(
            !comprehensions.contains(function),
            "comprehensions must remain compilation-only and not own {function}"
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
        "compiler/comprehension_analysis/captures.rs",
        "compiler/comprehension_analysis/validation.rs",
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
            outer
                .code
                .own_cell_slots()
                .iter()
                .any(|slot| slot.name == expected),
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
            proto
                .code
                .own_cell_slots()
                .iter()
                .any(|slot| slot.name == "x"),
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
        outer
            .code
            .own_cell_slots()
            .iter()
            .all(|slot| slot.name != "x"),
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
            outer
                .code
                .own_cell_slots()
                .iter()
                .any(|slot| slot.name == expected),
            "{expected} appears only in an evaluated target/pattern expression and must be captured"
        );
    }
}

/// The canonical-builtin guard of a reduction lowering in `code`, as the name
/// of the builtin the callee value is compared against.
fn genexpr_reduction_guard(code: &FnCode) -> Option<&'static str> {
    code.insns.iter().find_map(|insn| match insn {
        Insn::CmpJumpIfFalseConst(_, BinaryOp::Is, const_idx, _) => {
            match code.consts[*const_idx as usize].kind() {
                ValueKind::BuiltinFunction(name) => Some(name),
                _ => None,
            }
        }
        _ => None,
    })
}

fn genexpr_protos(code: &FnCode) -> Vec<&FnCode> {
    code.fn_protos
        .iter()
        .filter(|proto| proto.name.as_ref() == "<genexpr>")
        .map(|proto| proto.code.as_ref())
        .collect()
}

#[test]
fn genexpr_reduction_guards_the_builtin_and_keeps_the_original_call() {
    for (source, builtin) in [
        ("r = sum(x for x in xs)\n", "sum"),
        ("r = sum((x for x in xs), 10)\n", "sum"),
        ("r = any(x for x in xs)\n", "any"),
        ("r = all(x for x in xs if x)\n", "all"),
        ("r = min(x for x in xs for y in x)\n", "min"),
        ("r = max((x for x in xs), default=None)\n", "max"),
    ] {
        let code = compile_source(source);
        assert_eq!(
            genexpr_reduction_guard(&code),
            Some(builtin),
            "{source:?} must compare the loaded callee against the canonical builtin"
        );

        let protos = genexpr_protos(&code);
        assert_eq!(
            protos.len(),
            2,
            "{source:?} needs the reduction frame and the original generator"
        );
        let yields = |code: &FnCode| {
            code.insns
                .iter()
                .filter(|insn| matches!(insn, Insn::Yield { .. }))
                .count()
        };
        assert!(
            protos.iter().all(|proto| proto.is_generator),
            "{source:?}: both frames must stay generator frames"
        );
        let reduction = protos[0];
        let original = protos[1];
        assert_eq!(
            yields(original),
            1,
            "{source:?}: the fallback generator yields each element"
        );
        assert!(
            reduction
                .insns
                .iter()
                .any(|insn| matches!(insn, Insn::SetupExcept(_))),
            "{source:?}: the reduction step must be protected so its \
             exception can be re-raised from the enclosing frame"
        );
        assert_eq!(
            reduction
                .insns
                .iter()
                .any(|insn| matches!(insn, Insn::SumStep(..))),
            builtin == "sum",
            "{source:?}: only `sum` folds through SumStep"
        );
        assert!(
            original
                .insns
                .iter()
                .all(|insn| !matches!(insn, Insn::SumStep(..) | Insn::SetupExcept(_))),
            "{source:?}: the fallback generator must be the unmodified genexpr"
        );

        // The enclosing frame drives the reduction generator itself and
        // re-raises a failed step; the fallback is a plain builtin call.
        assert_eq!(
            code.insns
                .iter()
                .filter(|insn| matches!(insn, Insn::ForIter(..)))
                .count(),
            2,
            "{source:?}: one resume for the result, one to finish the frame"
        );
        assert!(
            code.insns
                .iter()
                .any(|insn| matches!(insn, Insn::RaiseValue(_))),
            "{source:?}: a failed step is re-raised from the enclosing frame"
        );
        assert!(
            code.insns
                .iter()
                .any(|insn| matches!(insn, Insn::Call(..) | Insn::CallKw { .. })),
            "{source:?}: the original call must remain as the deopt path"
        );
    }
}

#[test]
fn genexpr_reduction_is_skipped_for_shadowed_names_and_other_shapes() {
    for source in [
        // The name is bound in this scope: statically not the builtin.
        "sum = len\nr = sum(x for x in xs)\n",
        "def f(any):\n    return any(x for x in xs)\n",
        "def f():\n    min = max\n    def g():\n        return min(x for x in xs)\n    return g\n",
        // Shapes the lowering does not cover.
        "r = sum((x for x in xs), start=1)\n",
        "r = min((x for x in xs), key=abs)\n",
        "r = max((x for x in xs), key=abs, default=0)\n",
        "r = min((x for x in xs), (y for y in xs))\n",
        "r = sum([x for x in xs])\n",
        "r = sum(*[(x for x in xs)])\n",
        "r = sorted(x for x in xs)\n",
        "r = m.sum(x for x in xs)\n",
        // An asynchronous generator expression is not iterable by `sum`.
        "async def f():\n    return sum(await x for x in xs)\n",
        "async def f():\n    return any(x async for x in xs)\n",
    ] {
        fn visit<'a>(code: &'a FnCode, out: &mut Vec<&'a FnCode>) {
            out.push(code);
            for proto in &code.fn_protos {
                visit(proto.code.as_ref(), out);
            }
        }
        let module = compile_source(source);
        let mut codes = Vec::new();
        visit(&module, &mut codes);
        for code in codes {
            assert_eq!(
                genexpr_reduction_guard(code),
                None,
                "{source:?} must keep the ordinary call"
            );
            assert!(
                code.insns
                    .iter()
                    .all(|insn| !matches!(insn, Insn::SumStep(..))),
                "{source:?} must not fold through SumStep"
            );
        }
    }
}

#[test]
fn genexpr_reduction_evaluates_the_iterable_once_before_the_guard() {
    let code = compile_source("r = sum((x for x in make()), start_value())\n");
    let position = |pred: &dyn Fn(&Insn) -> bool| {
        code.insns
            .iter()
            .position(pred)
            .expect("instruction must be present")
    };
    let guard = position(&|insn| matches!(insn, Insn::CmpJumpIfFalseConst(_, BinaryOp::Is, ..)));
    let calls_before_guard = code.insns[..guard]
        .iter()
        .filter(|insn| matches!(insn, Insn::Call(..)))
        .count();
    assert_eq!(
        calls_before_guard, 2,
        "the outermost iterable and the start value are each evaluated once, \
         in source order, before the paths split"
    );
    let make = code
        .names
        .iter()
        .position(|name| name == "make")
        .expect("make name") as u16;
    assert_eq!(
        code.insns
            .iter()
            .filter(|insn| matches!(insn, Insn::LoadGlobal(_, idx) if *idx == make))
            .count(),
        1,
        "neither path may re-evaluate the outermost iterable"
    );
}

/// Every code object nested in `code` (itself included), depth first.
fn all_code_objects(code: &FnCode) -> Vec<&FnCode> {
    let mut out = vec![code];
    for proto in &code.fn_protos {
        out.extend(all_code_objects(&proto.code));
    }
    out
}

/// `(LoadSuperMethod, CallSuperMethod, Call, CallMethod)` instruction counts
/// over `code` and everything nested in it.
fn super_call_shape(code: &FnCode) -> (usize, usize, usize, usize) {
    let count = |pred: &dyn Fn(&Insn) -> bool| {
        all_code_objects(code)
            .iter()
            .flat_map(|code| code.insns.iter())
            .filter(|insn| pred(insn))
            .count()
    };
    (
        count(&|insn| matches!(insn, Insn::LoadSuperMethod { .. })),
        count(&|insn| matches!(insn, Insn::CallSuperMethod(_))),
        count(&|insn| matches!(insn, Insn::Call(..))),
        count(&|insn| matches!(insn, Insn::CallMethod { .. })),
    )
}

/// Issue #3424: `super().name(<positional args>)` in a function scope lowers
/// to the fused pair, with `super` still loaded as a global so a rebinding is
/// decided at run time.
#[test]
fn zero_arg_super_method_call_lowers_to_the_fused_pair() {
    // (source, plain `Call`s the source itself spells besides `super()`)
    for (source, source_calls) in [
        (
            "class B(A):\n    def f(self, x):\n        return super().f(x)\n",
            0,
        ),
        (
            "class B(A):\n    def f(self):\n        return super().f()\n",
            0,
        ),
        (
            "class B(A):\n    def f(self, x, y):\n        return super().f(x, y + 1)\n",
            0,
        ),
        // Comprehension, lambda, and nested-function scopes resolve the frame
        // at run time, so they are fused as well.
        (
            "class B(A):\n    def f(self):\n        return [super().g(i) for i in xs]\n",
            1,
        ),
        (
            "class B(A):\n    def f(self):\n        return (lambda me: super().g(1))(self)\n",
            1,
        ),
        (
            "class B(A):\n    def f(self):\n        def inner(me):\n            return super().g(1)\n        return inner(self)\n",
            1,
        ),
        (
            "class B(A):\n    async def f(self):\n        return super().g(1)\n",
            0,
        ),
        (
            "class B(A):\n    def f(self):\n        yield super().g(1)\n",
            0,
        ),
        // A `global super` declaration still reads the global.
        (
            "class B(A):\n    def f(self):\n        global super\n        return super().g(1)\n",
            0,
        ),
    ] {
        let code = compile_source(source);
        let (loads, calls, plain_calls, method_calls) = super_call_shape(&code);
        assert_eq!(
            (loads, calls),
            (1, 1),
            "{source:?} must lower to one LoadSuperMethod/CallSuperMethod pair"
        );
        assert_eq!(
            method_calls, 0,
            "{source:?}: the generic CallMethod must be gone"
        );
        assert_eq!(
            plain_calls, source_calls,
            "{source:?}: no plain Call may remain for `super()`"
        );
        let fused = all_code_objects(&code)
            .into_iter()
            .find(|code| {
                code.insns
                    .iter()
                    .any(|insn| matches!(insn, Insn::LoadSuperMethod { .. }))
            })
            .expect("a code object holds the fused pair");
        let load_at = fused
            .insns
            .iter()
            .position(|insn| matches!(insn, Insn::LoadSuperMethod { .. }))
            .expect("LoadSuperMethod");
        let Insn::LoadSuperMethod {
            func,
            recv,
            callee,
            name_idx,
        } = fused.insns[load_at]
        else {
            unreachable!()
        };
        assert!(
            matches!(
                fused.insns[load_at - 1],
                Insn::LoadGlobal(dst, idx) if dst == callee && fused.names[idx as usize] == "super"
            ),
            "{source:?}: `super` must be loaded as a global right before the fused load"
        );
        assert_ne!(func, recv, "{source:?}: the pair needs two registers");
        let call = fused
            .insns
            .iter()
            .find_map(|insn| match insn {
                Insn::CallSuperMethod(idx) => {
                    Some(*fused.descs[*idx as usize].as_call_super_method())
                }
                _ => None,
            })
            .expect("CallSuperMethod");
        assert_eq!((call.func, call.recv), (func, recv), "{source:?}");
        assert_eq!(call.name_idx, name_idx, "{source:?}");
    }
}

/// Issue #3424: every shape the fused pair does not cover keeps the generic
/// `Call` + `CallMethod` (or keyword/variadic) lowering untouched.
#[test]
fn super_method_call_is_not_fused_for_shadowed_names_and_other_shapes() {
    for source in [
        // `super` is a local, a parameter, or a captured variable.
        "class B(A):\n    def f(self):\n        super = other\n        return super().g(1)\n",
        "class B(A):\n    def f(self, super):\n        return super().g(1)\n",
        "class B(A):\n    def f(self):\n        super = other\n        def inner(me):\n            return super().g(1)\n        return inner\n",
        // Explicit arguments to `super` build a real super object.
        "class B(A):\n    def f(self):\n        return super(B, self).g(1)\n",
        // Module and class-body scopes have no function frame.
        "r = super().g(1)\n",
        "class B(A):\n    x = super().g(1)\n",
        // Attribute access without a call, and non-positional calls.
        "class B(A):\n    def f(self):\n        return super().g\n",
        "class B(A):\n    def f(self):\n        return super().g(k=1)\n",
        "class B(A):\n    def f(self, *a):\n        return super().g(*a)\n",
        "class B(A):\n    def f(self, **k):\n        return super().g(**k)\n",
        // Some other callee.
        "class B(A):\n    def f(self):\n        return sup().g(1)\n",
    ] {
        let code = compile_source(source);
        let (loads, calls, _, _) = super_call_shape(&code);
        assert_eq!((loads, calls), (0, 0), "{source:?} must not be fused");
    }
}

/// Issue #3424: the fused pair survives the SSA round trip and the bytecode
/// optimizer with each call still consuming the pair its own load defined.
#[test]
fn fused_super_method_call_survives_optimization() {
    let source = "\
class B(A):
    def f(self, x):
        y = super().f(x)
        return super().g(y, x) + super().f(y)
";
    let raw = compile_source(source);
    for (stage, code) in [
        ("mir", crate::mir::optimize_fncode_via_mir(raw.clone())),
        ("optimizer", crate::optimizer::optimize(raw.clone())),
    ] {
        let (loads, calls, plain_calls, method_calls) = super_call_shape(&code);
        assert_eq!(
            (loads, calls),
            (3, 3),
            "{stage}: all three sites stay fused"
        );
        assert_eq!((plain_calls, method_calls), (0, 0), "{stage}");
        let method = all_code_objects(&code)
            .into_iter()
            .find(|code| {
                code.insns
                    .iter()
                    .any(|insn| matches!(insn, Insn::CallSuperMethod(_)))
            })
            .expect("method body");
        let mut pending: Vec<(Reg, Reg, u16)> = Vec::new();
        for insn in &method.insns {
            match insn {
                Insn::LoadSuperMethod {
                    func,
                    recv,
                    name_idx,
                    ..
                } => {
                    assert_ne!(func, recv, "{stage}: the pair needs two registers");
                    pending.push((*func, *recv, *name_idx));
                }
                Insn::CallSuperMethod(idx) => {
                    let call = method.descs[*idx as usize].as_call_super_method();
                    let at = pending
                        .iter()
                        .position(|pair| *pair == (call.func, call.recv, call.name_idx))
                        .unwrap_or_else(|| {
                            panic!("{stage}: CallSuperMethod must consume a loaded pair: {call:?}")
                        });
                    pending.remove(at);
                    assert!(
                        (call.args_base..call.args_base + Reg::from(call.nargs))
                            .all(|arg| arg != call.func && arg != call.recv),
                        "{stage}: the argument window must not overlap the pair"
                    );
                }
                _ => {}
            }
        }
        assert!(pending.is_empty(), "{stage}: every load is consumed");
    }
}

/// A handler copies the caught exception into a register only when it binds
/// it (`as VAR`): an unbound copy would keep the exception alive in a dead
/// temporary until the frame exits (issue #3426).
#[test]
fn except_handlers_load_the_exception_only_when_one_binds_it() {
    let loads = |source: &str| {
        compile_source(source)
            .insns
            .iter()
            .filter(|insn| matches!(insn, Insn::LoadExc(_)))
            .count()
    };

    for unbound in [
        "try:\n    f()\nexcept ValueError:\n    pass\n",
        "try:\n    f()\nexcept:\n    pass\n",
        "try:\n    f()\nexcept (KeyError, IndexError):\n    g()\nexcept ValueError:\n    h()\nelse:\n    k()\n",
    ] {
        assert_eq!(loads(unbound), 0, "{unbound:?}");
        // Matching and the unmatched re-raise read the active exception.
        let code = compile_source(unbound);
        assert!(
            code.insns
                .iter()
                .any(|insn| matches!(insn, Insn::RaiseReRaise)),
            "{unbound:?}"
        );
        assert!(
            code.insns
                .iter()
                .any(|insn| matches!(insn, Insn::EndExcept)),
            "{unbound:?}"
        );
    }

    // One load serves every handler of the statement once any of them binds.
    for bound in [
        "try:\n    f()\nexcept ValueError as err:\n    g(err)\n",
        "try:\n    f()\nexcept KeyError:\n    pass\nexcept ValueError as err:\n    g(err)\n",
    ] {
        assert_eq!(loads(bound), 1, "{bound:?}");
    }
}
