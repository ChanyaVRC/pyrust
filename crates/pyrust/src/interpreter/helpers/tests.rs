#[cfg(test)]
mod purity_tests {
    use super::{is_memo_pure_body, is_memo_pure_function_body};
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use std::collections::{HashMap, HashSet};

    fn parse_body(src: &str) -> Vec<crate::ast::Stmt> {
        let tokens = Lexer::new(src).expect("lex failed").into_tokens();
        Parser::new(tokens).parse_program().expect("parse failed")
    }

    fn parse_function(
        src: &str,
    ) -> (
        String,
        Vec<crate::ast::FunctionParam>,
        Vec<crate::ast::Stmt>,
    ) {
        let mut program = parse_body(src);
        assert_eq!(program.len(), 1, "expected one function definition");
        let crate::ast::Stmt::Def {
            name, params, body, ..
        } = program.remove(0)
        else {
            panic!("expected a function definition");
        };
        (name, params, body)
    }

    /// Locals helper: treat the given names as function-local registers so a
    /// test exercises operator/call purity rather than free-variable reads.
    fn locals_of(names: &[&str]) -> HashMap<String, u32> {
        (0u32..)
            .zip(names.iter())
            .map(|(i, n)| (n.to_string(), i))
            .collect()
    }

    /// A registry entry proves what the canonical module function does, not
    /// that a Python-visible module attribute still points to it.
    #[test]
    fn module_namespaced_calls_need_a_binding_guard_for_memo_purity() {
        let body = parse_body("y = math.sqrt(x)\nz = math.sin(y)\nreturn z\n");
        assert!(
            !is_memo_pure_body(&body, &HashSet::new(), &locals_of(&["x", "y", "z"])),
            "mutable module attributes must not establish memo purity"
        );
    }

    /// Bare builtin names are equally mutable through globals and custom
    /// `__builtins__` providers.
    #[test]
    fn bare_builtin_calls_need_a_binding_guard_for_memo_purity() {
        let body = parse_body("return abs(x)\n");
        assert!(
            !is_memo_pure_body(&body, &HashSet::new(), &locals_of(&["x"])),
            "a registry spelling must not be mistaken for the active binding"
        );
    }

    /// The compiler supplies the current function's own name as the one
    /// identity-stable recursive binding accepted by memo-purity analysis.
    #[test]
    fn identity_stable_recursive_call_remains_memo_pure() {
        let body = parse_body("return fib(n - 1)\n");
        let pure_fns = HashSet::from(["fib".to_string()]);
        assert!(
            is_memo_pure_body(&body, &pure_fns, &locals_of(&["n"])),
            "direct self-recursion must remain eligible for CallMemo"
        );
    }

    #[test]
    fn recursive_call_must_bind_every_positional_parameter_for_memo_purity() {
        for source in [
            "def recurse(n, value=1):\n    return recurse(n - 1)\n",
            // `__defaults__` may add a default to an originally-required
            // parameter, so source-declared defaults are not the complete set
            // of mutable dependencies.
            "def recurse(n, value):\n    return recurse(n - 1)\n",
        ] {
            let (name, params, body) = parse_function(source);
            let pure_fns = HashSet::from([name.clone()]);
            assert!(
                !is_memo_pure_function_body(
                    &body,
                    &pure_fns,
                    &locals_of(&["n", "value"]),
                    &name,
                    &params,
                ),
                "a recursive call that can use mutable __defaults__ must not be memo-pure"
            );
        }

        let (name, params, body) =
            parse_function("def recurse(n, value=1):\n    return recurse(n - 1, value)\n");
        let pure_fns = HashSet::from([name.clone()]);
        assert!(
            is_memo_pure_function_body(
                &body,
                &pure_fns,
                &locals_of(&["n", "value"]),
                &name,
                &params,
            ),
            "explicitly binding every positional parameter keeps recursion memo-pure"
        );
    }

    /// Method calls on values must remain impure — the receiver can be
    /// any user instance whose method has side effects, and we don't
    /// know the receiver's type at AST-purity time.
    #[test]
    fn value_method_calls_stay_impure() {
        let body = parse_body("y = obj.frobnicate(x)\nreturn y\n");
        assert!(
            !is_memo_pure_body(&body, &HashSet::new(), &HashMap::new()),
            "obj.method(...) calls must be conservatively impure"
        );
    }

    /// Attribute callees don't qualify: every component can be rebound and
    /// the purity analysis has no identity guard for the resolved callable.
    #[test]
    fn nested_attribute_calls_stay_impure() {
        let body = parse_body("y = a.b.c(x)\nreturn y\n");
        assert!(
            !is_memo_pure_body(&body, &HashSet::new(), &HashMap::new()),
            "a.b.c(...) calls must be conservatively impure"
        );
    }

    /// A bare call without an identity guard remains impure regardless of the
    /// canonical builtin that normally owns its spelling.
    #[test]
    fn impure_builtins_are_rejected() {
        let body = parse_body("print(x)\nreturn x\n");
        assert!(
            !is_memo_pure_body(&body, &HashSet::new(), &HashMap::new()),
            "print(...) has no identity guard and must NOT pass the gate"
        );
    }

    /// A body containing a nested `def` must be classified impure.
    /// Each execution of `def f(): ...` allocates a fresh Rc<UserFunction>
    /// with a unique identity; memoising the outer call would skip that
    /// allocation and return the same function object every time, breaking
    /// `f1 is f2 == False` (#769).
    #[test]
    fn nested_def_is_impure() {
        let body = parse_body("def f(): pass\nreturn f\n");
        assert!(
            !is_memo_pure_body(&body, &HashSet::new(), &HashMap::new()),
            "body with nested def must be impure (fixes #769)"
        );
    }

    /// A body containing a nested `class` must be classified impure for the
    /// same reason as `nested_def_is_impure`: each class statement allocates
    /// a fresh PyClass object whose identity is observable via `is` / `id()`.
    #[test]
    fn nested_class_is_impure() {
        let body = parse_body("class C: pass\nreturn C\n");
        assert!(
            !is_memo_pure_body(&body, &HashSet::new(), &HashMap::new()),
            "body with nested class must be impure (fixes #769)"
        );
    }

    /// A `lambda` expression allocates a fresh Rc<UserFunction> on every
    /// evaluation, so a body that contains a lambda return is not pure in
    /// the identity sense.
    #[test]
    fn lambda_expr_is_impure() {
        let body = parse_body("return lambda x: x + 1\n");
        assert!(
            !is_memo_pure_body(&body, &HashSet::new(), &HashMap::new()),
            "body returning a lambda must be impure (fixes #769)"
        );
    }

    // ── Memo-purity operator coverage (issue #2523) ─────────────────────────

    /// Comparisons remain memo-pure because the runtime cache only accepts
    /// integer arguments, which have deterministic builtin comparison slots.
    #[test]
    fn comparison_is_memo_pure() {
        let body = parse_body("a < a\n");
        let locals = locals_of(&["a"]);
        assert!(
            is_memo_pure_body(&body, &HashSet::new(), &locals),
            "`a < a` must be memo-pure"
        );
    }

    /// Unary operators have the same integer-only memoization guarantee.
    #[test]
    fn unary_op_is_memo_pure() {
        let body = parse_body("-a\n");
        let locals = locals_of(&["a"]);
        assert!(
            is_memo_pure_body(&body, &HashSet::new(), &locals),
            "`-a` must be memo-pure"
        );
    }

    /// A raise-capable binary op `a / a` (ZeroDivisionError, or a user
    /// `__truediv__`) is not memo-pure. Non-raising arithmetic remains
    /// memo-pure.
    #[test]
    fn raise_capable_binop_is_not_memo_pure() {
        let div = parse_body("a / a\n");
        let add = parse_body("a + 1\n");
        let locals = locals_of(&["a"]);
        assert!(
            !is_memo_pure_body(&div, &HashSet::new(), &locals),
            "`a / a` must NOT be memo-pure (raise-capable)"
        );
        assert!(
            is_memo_pure_body(&add, &HashSet::new(), &locals),
            "`a + 1` must be memo-pure"
        );
    }

    #[test]
    fn augmented_assignment_target_effects_gate_memo_purity() {
        let locals = locals_of(&["amount", "local"]);
        for source in [
            "state.value += amount\nreturn amount\n",
            "state[0] += amount\nreturn amount\n",
            "state[0:1] += [amount]\nreturn amount\n",
        ] {
            let body = parse_body(source);
            assert!(
                !is_memo_pure_body(&body, &HashSet::new(), &locals),
                "protocol-dispatching augmented target must be impure: {source}"
            );
        }

        let local = parse_body("local += amount\nreturn local\n");
        assert!(
            is_memo_pure_body(&local, &HashSet::new(), &locals),
            "integer-only local-name augmented assignment remains memo-pure"
        );
    }

    #[test]
    fn nonlocal_unpack_store_is_not_memo_pure() {
        let body = parse_body("(state.value,) = (1,)\nreturn 1\n");
        assert!(
            !is_memo_pure_body(&body, &HashSet::new(), &HashMap::new()),
            "an attribute nested in an unpack target mutates external state"
        );
    }

    #[test]
    fn match_pattern_and_guard_dependencies_gate_memo_purity() {
        let locals = locals_of(&["value"]);
        let guarded = parse_body(
            "match value:\n    case 1 if enabled:\n        return 1\n    case _:\n        return 0\n",
        );
        assert!(
            !is_memo_pure_body(&guarded, &HashSet::new(), &locals),
            "a match guard can observe a mutable outer binding"
        );

        let dotted = parse_body(
            "match value:\n    case Owner.expected:\n        return 1\n    case _:\n        return 0\n",
        );
        assert!(
            !is_memo_pure_body(&dotted, &HashSet::new(), &locals),
            "a value pattern performs a mutable dotted lookup"
        );

        let literal = parse_body(
            "match value:\n    case 1:\n        return 1\n    case _:\n        return 0\n",
        );
        assert!(
            is_memo_pure_body(&literal, &HashSet::new(), &locals),
            "literal/capture-only matching remains eligible"
        );
    }

    #[test]
    fn except_kind_dependency_gates_memo_purity() {
        let body = parse_body("try:\n    return value\nexcept DynamicError:\n    return 0\n");
        assert!(
            !is_memo_pure_body(&body, &HashSet::new(), &locals_of(&["value"])),
            "except kind is evaluated dynamically when a handler is selected"
        );
    }
}

#[cfg(test)]
mod singleton_method_name_tests {
    use super::registered_builtin_method_name;

    #[test]
    fn legacy_dispatch_names_are_interned_once_across_threads() {
        let first = std::thread::spawn(|| {
            registered_builtin_method_name("int", "bit_length").as_ptr() as usize
        })
        .join()
        .expect("first singleton-name thread panicked");
        let second = std::thread::spawn(|| {
            registered_builtin_method_name("int", "bit_length").as_ptr() as usize
        })
        .join()
        .expect("second singleton-name thread panicked");

        assert_eq!(first, second);
    }
}
