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

#[cfg(test)]
mod object_id_tests {
    use super::values_are_identical;
    use pyrust_core::{PyDict, PySet, Value};

    /// One representative value per identity representation, plus the pairs
    /// that used to collide on id `0`: `None` / `0` / `0.0` / `-0.0` /
    /// `False`, two separately boxed NaNs, and two equal complexes.
    fn matrix() -> Vec<Value> {
        vec![
            Value::none(),
            Value::ellipsis(),
            Value::not_implemented(),
            Value::bool_(false),
            Value::bool_(true),
            Value::int(0),
            Value::int(1),
            Value::int(-1),
            Value::float(0.0),
            Value::float(-0.0),
            Value::float(1.0),
            Value::float(1.5),
            Value::float(-1.5),
            Value::float(f64::INFINITY),
            Value::float(f64::NEG_INFINITY),
            Value::float(f64::NAN),
            Value::float(f64::NAN),
            // The two smallest subnormals: their raw bit patterns are `1` and
            // `2`, which is the range the monotonic list/tuple/set counter
            // hands out.  See `float_bits_never_collide_with_a_heap_id`.
            Value::float(f64::from_bits(1)),
            Value::float(f64::from_bits(2)),
            Value::complex(0.0, 0.0),
            Value::complex(1.0, 2.0),
            Value::complex(1.0, 2.0),
            Value::complex(2.0, 1.0),
            Value::complex(f64::NAN, 0.0),
            Value::string(""),
            Value::string("abc"),
            Value::string("a string far too long to be stored inline"),
            Value::bytes(vec![1, 2]),
            Value::list(vec![]),
            Value::list(vec![]),
            Value::tuple(vec![]),
            Value::tuple(vec![Value::int(1), Value::int(2)]),
            Value::set(PySet::default()),
            Value::dict(PyDict::default()),
        ]
    }

    /// `id()` is `Value::object_id`, so `id()` and `is` agree exactly when
    /// `object_id` equality agrees with `values_are_identical` (#2956).  This
    /// is the guard that keeps the two definitions from drifting apart again.
    ///
    /// Two known holes are deliberately left out of the matrix; both are `is`
    /// bugs rather than `id` bugs.  `range` has no arm in
    /// `values_are_identical` at all (so even `r is r` is False), and an
    /// `instance_dict` proxy compares by proxy *target*, which lives in
    /// `pyrust-builtins` and is invisible to `pyrust-core`.
    #[test]
    fn object_id_agrees_with_values_are_identical() {
        let values = matrix();
        for (i, left) in values.iter().enumerate() {
            for (j, right) in values.iter().enumerate() {
                assert_eq!(
                    values_are_identical(left, right),
                    left.object_id() == right.object_id(),
                    "`is` and `id()` disagree for matrix[{i}] vs matrix[{j}]"
                );
            }
        }
    }

    /// An alias is the same object, so a clone must keep the id.
    #[test]
    fn clones_keep_their_object_id() {
        for value in matrix() {
            let alias = value.clone();
            assert!(values_are_identical(&value, &alias));
            assert_eq!(value.object_id(), alias.object_id());
        }
    }

    /// The old `id()` returned `0` for every kind it had not enumerated,
    /// which is what gave all floats and complexes one shared id.  Nothing
    /// reaches `0` any more: the tagged kinds carry their tag, and the two
    /// bit-derived kinds are seeded past the finaliser's `0 -> 0` fixed point.
    #[test]
    fn no_value_falls_back_to_a_zero_id() {
        for value in matrix() {
            assert_ne!(value.object_id(), 0, "unexpected zero id");
        }
    }

    /// A heap id is a small number — the monotonic list/tuple/set counter
    /// starts at `1` and an `Rc` address fits in 48 bits — and every such
    /// pattern is also a subnormal `f64`.  Taking a float's id from its raw
    /// box bits therefore collided with live objects in one step, because
    /// `5e-324 * n` is exactly the subnormal whose bits are `n`:
    /// `id(5e-324) == id((1, 2, 3))` and `id(5e-324 * id(d)) == id(d)` were
    /// both True while `is` was False (#2956 review).
    #[test]
    fn float_bits_never_collide_with_a_heap_id() {
        let heap = vec![
            Value::list(vec![Value::int(1)]),
            Value::tuple(vec![Value::int(1), Value::int(2), Value::int(3)]),
            Value::set(PySet::default()),
            Value::dict(PyDict::default()),
            Value::bytes(vec![1, 2]),
            Value::string("a string far too long to be stored inline"),
        ];
        for object in &heap {
            // The float whose bit pattern *is* this object's id — the value a
            // Python program reaches by multiplying out `5e-324`.
            let twin = Value::float(f64::from_bits(object.object_id()));
            assert!(!values_are_identical(object, &twin));
            assert_ne!(
                object.object_id(),
                twin.object_id(),
                "a float's id collided with a live heap object's id"
            );
        }
    }
}
