// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::approx_constant)] // 3.14 literals are deliberate test data, not π.
mod tests {
    //! Pin the strict-1:1 contract for every wrapper.  These tests double as
    //! the executable spec the `pyrust_module!` overload dispatcher relies on:
    //! if `PyFloat::matches` ever returns `true` for an `int` value, every
    //! `(PyInt, PyInt)` / `(PyFloat, PyFloat)` overload pair across the code
    //! base would silently shift behaviour, so the matches predicates get
    //! tighter coverage than `try_from_value` alone.

    use super::*;
    use crate::value::PyKey;

    // Helper — extract the `Named` exception class out of a PyError.
    fn err_class(e: &PyError) -> &str {
        match e {
            PyError::Named(cls, _) => cls.as_ref(),
            _ => "<not-named>",
        }
    }

    fn err_msg(e: &PyError) -> &str {
        match e {
            PyError::Named(_, msg) => msg.as_str(),
            _ => "",
        }
    }

    // ── PyInt — accepts both inline `Int(i64)` and heap-stored `BigInt`,
    //          since Python's int is unbounded.  Rejects bool / float / str.
    #[test]
    fn pyint_accepts_inline_int() {
        let v = Value::int(42);
        let r = PyInt::try_from_value(&v, "f", "x").expect("int accepted");
        assert_eq!(r.as_i64(), Some(42));
        assert!(!r.is_big(), "Value::int produces the Small representation");
        assert!(PyInt::matches(&v));
    }

    #[test]
    fn pyint_rejects_bool() {
        let v = Value::bool_(true);
        assert!(PyInt::try_from_value(&v, "f", "x").is_err());
        assert!(!PyInt::matches(&v));
    }

    #[test]
    fn pyint_rejects_float() {
        let v = Value::float(1.0);
        let err = PyInt::try_from_value(&v, "f", "x").unwrap_err();
        assert_eq!(err_class(&err), "TypeError");
        assert!(err_msg(&err).contains("'x' must be int"));
        assert!(!PyInt::matches(&v));
    }

    #[test]
    fn pyint_bigint_that_fits_collapses_to_small() {
        // `pyrust-core::Value::kind()` automatically downgrades a
        // heap-stored BigInt back to `ValueKind::Int` whenever the
        // magnitude fits in i64.  PyInt sees the post-`kind()` view, so
        // a BigInt wrapping `i64::MAX` arrives as `Small`, not `Big`.
        // This means builtin bodies only encounter the `Big` path for
        // genuine overflow — the common-case fast path stays cheap.
        let v = Value::bigint(PyBigInt::from(i64::MAX));
        let r = PyInt::try_from_value(&v, "f", "x").expect("bigint accepted");
        assert!(
            !r.is_big(),
            "fits-in-i64 BigInt downgrades to Small via kind()"
        );
        assert_eq!(r.as_i64(), Some(i64::MAX));
        assert!(PyInt::matches(&v));
    }

    #[test]
    fn pyint_accepts_bigint_beyond_i64() {
        // The whole point of supporting BigInt: Python's int is
        // unbounded.  `2 ** 100` doesn't fit in i64, but PyInt must
        // accept it.  `as_i64()` returns None; `to_bigint()` recovers
        // the value.
        let huge = PyBigInt::from(1u128 << 100);
        let v = Value::bigint(huge.clone());
        let r = PyInt::try_from_value(&v, "f", "x").expect("bigint accepted");
        assert!(r.is_big());
        assert_eq!(r.as_i64(), None, "out-of-range bigint must not fit i64");
        assert_eq!(r.to_bigint(), huge);
        assert!(PyInt::matches(&v));
    }

    #[test]
    fn pyint_expect_i64_raises_overflow_on_bignum() {
        // The CPython-style `OverflowError` shape for builtins that
        // genuinely need an i64 (chr, range, sleep, …).  Pinned wording.
        let huge = PyBigInt::from(1u128 << 100);
        let v = Value::bigint(huge);
        let r = PyInt::try_from_value(&v, "f", "x").unwrap();
        let err = r.expect_i64("chr", "code_point").unwrap_err();
        assert_eq!(err_class(&err), "OverflowError");
        assert!(
            err_msg(&err).contains("chr()")
                && err_msg(&err).contains("'code_point'")
                && err_msg(&err).contains("too large to fit in i64"),
            "unexpected OverflowError wording: {:?}",
            err_msg(&err),
        );
    }

    #[test]
    fn pyint_to_bigint_works_for_small_repr_too() {
        // Symmetry check: `to_bigint()` upgrades a Small to a fresh
        // BigInt without information loss.  Builtins that mix small
        // and big inputs can normalise to BigInt up front.
        let v = Value::int(-42);
        let r = PyInt::try_from_value(&v, "f", "x").unwrap();
        assert!(!r.is_big());
        assert_eq!(r.to_bigint(), PyBigInt::from(-42i64));
    }

    // ── PyFloat — strict 1:1 with `float`; rejects int, bool, …
    #[test]
    fn pyfloat_accepts_float_only() {
        let v = Value::float(3.14);
        let r = PyFloat::try_from_value(&v, "f", "x").expect("float accepted");
        assert_eq!(r.0, 3.14);
        assert!(PyFloat::matches(&v));
    }

    #[test]
    fn pyfloat_rejects_int() {
        let v = Value::int(1);
        assert!(PyFloat::try_from_value(&v, "f", "x").is_err());
        assert!(!PyFloat::matches(&v));
    }

    #[test]
    fn pyfloat_rejects_bool() {
        let v = Value::bool_(false);
        assert!(PyFloat::try_from_value(&v, "f", "x").is_err());
        assert!(!PyFloat::matches(&v));
    }

    // ── PyStr — only `str`; no __str__ coercion.
    #[test]
    fn pystr_accepts_str_only() {
        let v = Value::string("hi");
        let r = PyStr::try_from_value(&v, "f", "x").unwrap();
        assert_eq!(r.0, "hi");
        assert_eq!(&*r, "hi"); // Deref<Target = str>
        assert!(PyStr::matches(&v));
    }

    #[test]
    fn pystr_rejects_int() {
        let v = Value::int(7);
        assert!(PyStr::try_from_value(&v, "f", "x").is_err());
        assert!(!PyStr::matches(&v));
    }

    // ── PyBool / PyBytes / PyList / PyTuple / PyDict / PySet — strict.
    #[test]
    fn pybool_strict() {
        assert!(PyBool::matches(&Value::bool_(true)));
        assert!(!PyBool::matches(&Value::int(1)));
        assert!(!PyBool::matches(&Value::float(0.0)));
    }

    #[test]
    fn pylist_strict() {
        let v = Value::list(vec![Value::int(1), Value::int(2)]);
        assert!(PyList::matches(&v));
        assert!(!PyList::matches(&Value::tuple(vec![])));
        let l = PyList::try_from_value(&v, "f", "x").unwrap();
        assert_eq!(l.as_slice().len(), 2);
    }

    #[test]
    fn pytuple_strict() {
        let v = Value::tuple(vec![Value::int(1)]);
        assert!(PyTuple::matches(&v));
        assert!(!PyTuple::matches(&Value::list(vec![])));
    }

    #[test]
    fn pydict_strict() {
        let v = Value::dict(pyrust_core::PyDict::default());
        assert!(PyDict::matches(&v));
        assert!(!PyDict::matches(&Value::list(vec![])));
    }

    #[test]
    fn pyset_strict() {
        let v = Value::set(pyrust_core::PySet::default());
        assert!(PySet::matches(&v));
        assert!(!PySet::matches(&Value::list(vec![])));
    }

    // ── PyValue — always matches.
    #[test]
    fn pyvalue_matches_anything() {
        for v in [
            Value::int(0),
            Value::float(0.0),
            Value::string("s"),
            Value::list(vec![]),
            Value::none(),
        ] {
            assert!(PyValue::matches(&v));
        }
    }

    // ── PyIterable — anything iterable; structurally allocation-free `matches`,
    //                 materialising `try_from_value` via the iter callback.
    //
    // `iter_values_via_registry` reads a `OnceLock<IterValuesFn>` that the
    // interpreter installs in `Interpreter::default()`.  The unit tests below
    // run without an interpreter, so the helper here installs the same
    // callback once per test run — `OnceLock::set` ignores subsequent calls,
    // so this is harmless under cargo's parallel test scheduling.
    fn ensure_iter_registry_installed() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            pyrust_core::install_iter_values(crate::interpreter::iter_values);
        });
    }

    #[test]
    fn pyiterable_matches_builtin_iterables() {
        // Structural `matches` — no materialisation, no registry needed.
        // Each kind in the "known iterable" set must report `true`.
        let cases = [
            Value::list(vec![Value::int(1)]),
            Value::tuple(vec![Value::int(1)]),
            Value::dict(pyrust_core::PyDict::default()),
            Value::set(pyrust_core::PySet::default()),
            Value::string("abc"),
            Value::bytes(vec![1, 2, 3]),
            Value::range(0, 3, 1),
        ];
        for v in &cases {
            assert!(
                PyIterable::matches(v),
                "expected iterable: {:?}",
                pyrust_core::builtin_type_name(v),
            );
        }
    }

    #[test]
    fn pyiterable_rejects_scalars() {
        // `matches` must report `false` for the canonical non-iterable
        // kinds — guards the overload dispatcher from accidentally
        // routing `int`/`float`/`bool`/`None` through an iterable
        // overload.
        for v in [
            Value::int(0),
            Value::float(0.0),
            Value::bool_(true),
            Value::none(),
        ] {
            assert!(
                !PyIterable::matches(&v),
                "scalar should not match iterable: {:?}",
                pyrust_core::builtin_type_name(&v),
            );
        }
    }

    #[test]
    fn pyiterable_materialises_list() {
        ensure_iter_registry_installed();
        let v = Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]);
        let it = PyIterable::try_from_value(&v, "list", "iterable").unwrap();
        assert_eq!(it.len(), 3);
        assert!(!it.is_empty());
        let items = it.as_slice();
        assert_eq!(items[0].as_int(), Some(1));
        assert_eq!(items[2].as_int(), Some(3));
    }

    #[test]
    fn pyiterable_materialises_tuple() {
        ensure_iter_registry_installed();
        let v = Value::tuple(vec![Value::int(7), Value::int(8)]);
        let it = PyIterable::try_from_value(&v, "list", "iterable").unwrap();
        let items = it.into_items();
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].as_int(), Some(8));
    }

    #[test]
    fn pyiterable_materialises_dict_yields_keys() {
        // CPython parity: iterating a dict yields its keys, not its
        // items.  The interpreter's `iter_values` already does this; the
        // wrapper inherits the behaviour.
        ensure_iter_registry_installed();
        let mut map = pyrust_core::PyDict::default();
        map.insert(PyKey::str_from("a"), Value::int(1));
        map.insert(PyKey::str_from("b"), Value::int(2));
        let v = Value::dict(map);
        let it = PyIterable::try_from_value(&v, "list", "iterable").unwrap();
        let items = it.into_items();
        assert_eq!(items.len(), 2);
        // Keys arrive as strings, not (key, value) pairs.
        assert_eq!(items[0].as_str(), Some("a"));
        assert_eq!(items[1].as_str(), Some("b"));
    }

    #[test]
    fn pyiterable_materialises_set() {
        ensure_iter_registry_installed();
        let mut s = pyrust_core::PySet::default();
        s.insert(PyKey::Int(9));
        let v = Value::set(s);
        let it = PyIterable::try_from_value(&v, "list", "iterable").unwrap();
        assert_eq!(it.len(), 1);
        assert_eq!(it.as_slice()[0].as_int(), Some(9));
    }

    #[test]
    fn pyiterable_materialises_str_to_chars() {
        ensure_iter_registry_installed();
        let v = Value::string("hi");
        let it = PyIterable::try_from_value(&v, "list", "iterable").unwrap();
        let items = it.into_items();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_str(), Some("h"));
        assert_eq!(items[1].as_str(), Some("i"));
    }

    #[test]
    fn pyiterable_materialises_bytes_to_codepoints() {
        ensure_iter_registry_installed();
        let v = Value::bytes(vec![0x41, 0x42]);
        let it = PyIterable::try_from_value(&v, "list", "iterable").unwrap();
        let items = it.into_items();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_int(), Some(0x41));
        assert_eq!(items[1].as_int(), Some(0x42));
    }

    #[test]
    fn pyiterable_materialises_range() {
        ensure_iter_registry_installed();
        let v = Value::range(0, 3, 1);
        let it = PyIterable::try_from_value(&v, "list", "iterable").unwrap();
        let items = it.into_items();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].as_int(), Some(0));
        assert_eq!(items[2].as_int(), Some(2));
    }

    #[test]
    fn pyiterable_rejects_int_with_typeerror() {
        ensure_iter_registry_installed();
        let v = Value::int(5);
        let err = PyIterable::try_from_value(&v, "list", "iterable").unwrap_err();
        assert_eq!(err_class(&err), "TypeError");
        let msg = err_msg(&err);
        assert!(
            msg.contains("list()")
                && msg.contains("'iterable'")
                && msg.contains("must be iterable")
                && msg.contains("not int"),
            "unexpected error wording: {msg:?}",
        );
    }

    #[test]
    fn pyiterable_rejects_float() {
        ensure_iter_registry_installed();
        let v = Value::float(3.14);
        let err = PyIterable::try_from_value(&v, "sum", "iterable").unwrap_err();
        assert!(err_msg(&err).contains("not float"));
    }

    #[test]
    fn pyiterable_rejects_bool() {
        ensure_iter_registry_installed();
        let v = Value::bool_(false);
        let err = PyIterable::try_from_value(&v, "any", "iterable").unwrap_err();
        assert!(err_msg(&err).contains("not bool"));
    }

    #[test]
    fn pyiterable_rejects_none() {
        ensure_iter_registry_installed();
        let v = Value::none();
        let err = PyIterable::try_from_value(&v, "iter", "iterable").unwrap_err();
        assert!(err_msg(&err).contains("not NoneType"));
    }

    // ── Option<T> — accepts None or T.
    #[test]
    fn option_t_accepts_none() {
        let v = Value::none();
        let r = <Option<PyInt>>::try_from_value(&v, "f", "x").unwrap();
        assert!(r.is_none());
        assert!(<Option<PyInt>>::matches(&v));
    }

    #[test]
    fn option_t_accepts_t() {
        let v = Value::int(5);
        let r = <Option<PyInt>>::try_from_value(&v, "f", "x").unwrap();
        assert_eq!(r.unwrap().as_i64(), Some(5));
        assert!(<Option<PyInt>>::matches(&v));
    }

    #[test]
    fn option_t_rejects_wrong_inner_type() {
        let v = Value::float(5.0);
        assert!(<Option<PyInt>>::try_from_value(&v, "f", "x").is_err());
        assert!(!<Option<PyInt>>::matches(&v));
    }

    #[test]
    fn option_t_error_wording_mentions_or_none() {
        // Regression for the PR-#396 review: `Option<T>` rejection used to
        // route through `T::try_from_value` and print "must be int, not str"
        // — strictly correct for `T = PyInt` but misleading for callers who
        // can also pass `None`.  The override now says "must be int or None".
        let v = Value::string("hi");
        let err = <Option<PyInt>>::try_from_value(&v, "pow", "exp").unwrap_err();
        let msg = err_msg(&err);
        assert!(
            msg.contains("must be int or None"),
            "expected the wording to mention 'or None'; got: {msg:?}",
        );
        assert!(
            msg.contains("not str"),
            "should include the actual type: {msg:?}"
        );
    }

    // ── From impls — ergonomic `#[default(literal)]` construction.
    #[test]
    fn from_impls_for_default_values() {
        // Each strict wrapper carries a `From<inner>` impl so the macro
        // can accept `#[default(0)]` / `#[default(0.0)]` / `#[default(true)]`
        // / `#[default("r")]` without forcing the author to write the
        // wrapper constructor explicitly.
        let i: PyInt = 42i64.into();
        assert_eq!(i.as_i64(), Some(42));
        let f: PyFloat = 3.14f64.into();
        assert_eq!(f.0, 3.14);
        let b: PyBool = true.into();
        assert!(b.0);
        let s: PyStr<'static> = "r".into();
        assert_eq!(&*s, "r");
        let s2: PyStr<'static> = String::from("w").into();
        assert_eq!(&*s2, "w");
    }

    #[test]
    fn pystr_borrows_zero_copy_from_value() {
        // Regression for the Cow refactor: `try_from_value` must produce
        // `Cow::Borrowed`, not `Cow::Owned`, when extracting from a
        // `Value` — that's the whole point of the lifetime-carrying
        // wrapper.  If a future change reverts to `s.to_string()` the
        // assertion below catches it.
        let v = Value::string("hello");
        let s = PyStr::try_from_value(&v, "f", "x").unwrap();
        assert!(
            matches!(s.0, Cow::Borrowed(_)),
            "PyStr should borrow from the Value, not allocate a fresh String",
        );
        assert_eq!(&*s, "hello");
    }

    #[test]
    fn pystr_default_via_into_is_zero_copy() {
        // `"r".into()` (used by `#[default("r".into())]`) creates a
        // `Cow::Borrowed(&'static str)` — also zero-copy.  Symmetric with
        // the Value-extraction path above.
        let s: PyStr<'static> = "r".into();
        assert!(
            matches!(s.0, Cow::Borrowed(_)),
            "PyStr::from(&'static str) should produce Cow::Borrowed",
        );
    }

    // ── no_overload_matched — error wording for the overload dispatcher.
    //
    // The wording follows CPython's `unsupported operand type(s) for +:
    // 'int' and 'str'` shape — terse, actual types only, no declared-
    // overload-signature dump.  See the design review on #395
    // (comment 4443208232): CPython doesn't list candidate signatures
    // for type-dispatch failures; including them here would be a
    // usability regression for end users.
    #[test]
    fn no_overload_matched_single_arg() {
        let err =
            no_overload_matched::<()>("abs", &[std::borrow::Cow::Borrowed("str")]).unwrap_err();
        let msg = err_msg(&err);
        assert_eq!(err_class(&err), "TypeError");
        assert!(
            msg.contains("abs(): unsupported argument type(s): ('str')"),
            "expected terse 'unsupported argument type(s)' wording: {msg:?}",
        );
        // The pre-revision wording (`"no overload matches"` /
        // `"expected one of"`) is explicitly *not* used anymore.
        assert!(
            !msg.contains("expected one of"),
            "signatures must not appear in the user-facing error: {msg:?}",
        );
        assert!(
            !msg.contains("no overload"),
            "must not say 'no overload': {msg:?}",
        );
    }

    #[test]
    fn no_overload_matched_multi_arg() {
        let err = no_overload_matched::<()>(
            "pow",
            &[
                std::borrow::Cow::Borrowed("int"),
                std::borrow::Cow::Borrowed("str"),
            ],
        )
        .unwrap_err();
        let msg = err_msg(&err);
        assert!(
            msg.contains("pow(): unsupported argument type(s): ('int', 'str')"),
            "multi-arg actuals should be quoted + parenthesised: {msg:?}",
        );
    }

    // ── Error messages — CPython parity.
    #[test]
    fn typeerror_must_be_message_format() {
        let v = Value::int(5);
        let err = PyStr::try_from_value(&v, "open", "path").unwrap_err();
        let msg = err_msg(&err);
        assert!(
            msg.contains("open()") && msg.contains("'path'") && msg.contains("must be str"),
            "unexpected error message: {msg:?}"
        );
    }

    #[test]
    fn missing_arg_message_format() {
        let err = missing_arg::<()>("open", "path").unwrap_err();
        let msg = err_msg(&err);
        assert!(
            msg.contains("open()") && msg.contains("missing required argument: 'path'"),
            "unexpected error message: {msg:?}"
        );
    }

    #[test]
    fn check_positional_count_rejects_too_many_range() {
        // min < max — emit the "from M to N" range wording.
        let err = check_positional_count("open", 3, 1, 2).unwrap_err();
        assert!(
            err_msg(&err).contains("from 1 to 2 positional arguments but 3"),
            "unexpected error message: {:?}",
            err_msg(&err)
        );
        // Within bounds — no error.
        assert!(check_positional_count("open", 2, 1, 2).is_ok());
        assert!(check_positional_count("open", 1, 1, 2).is_ok());
    }

    #[test]
    fn check_positional_count_min_eq_max_singular() {
        // Regression for PR-#396 review feedback: a 1-required builtin
        // hit with 2 positional args used to print
        // "takes from 1 to 1 positional arguments but 2 were given" —
        // both nonsensical and divergent from CPython.  Now it prints
        // "takes 1 positional argument but 2 were given" with the
        // singular "argument" because max == 1.
        let err = check_positional_count("len", 2, 1, 1).unwrap_err();
        let msg = err_msg(&err);
        assert!(
            msg.contains("takes 1 positional argument but 2 were given"),
            "expected singular wording with no from-to range; got: {msg:?}",
        );
        assert!(
            !msg.contains("from 1"),
            "should not contain 'from 1 to 1': {msg:?}"
        );
    }

    #[test]
    fn check_positional_count_min_eq_max_plural() {
        // Plural wording when max > 1.
        let err = check_positional_count("divmod", 3, 2, 2).unwrap_err();
        let msg = err_msg(&err);
        assert!(
            msg.contains("takes 2 positional arguments but 3 were given"),
            "expected plural 'arguments'; got: {msg:?}",
        );
        assert!(
            !msg.contains("from 2 to 2"),
            "should not contain 'from 2 to 2': {msg:?}"
        );
    }

    #[test]
    fn check_exactly_one_argument_wording() {
        // METH_O wording (#2331): any count != 1 → "takes exactly one
        // argument (N given)"; count == 1 is accepted.
        assert!(check_exactly_one_argument("repr", 1).is_ok());
        let too_few = check_exactly_one_argument("repr", 0).unwrap_err();
        assert_eq!(
            err_msg(&too_few),
            "repr() takes exactly one argument (0 given)"
        );
        let too_many = check_exactly_one_argument("repr", 2).unwrap_err();
        assert_eq!(
            err_msg(&too_many),
            "repr() takes exactly one argument (2 given)"
        );
    }

    #[test]
    fn check_arity_expected_got_wording() {
        // METH_VARARGS wording (#2331): bare name, no trailing parens.
        assert!(check_arity_expected_got("isinstance", 2, 2, 2).is_ok());
        let fixed = check_arity_expected_got("isinstance", 1, 2, 2).unwrap_err();
        assert_eq!(err_msg(&fixed), "isinstance expected 2 arguments, got 1");
        let fixed_many = check_arity_expected_got("isinstance", 3, 2, 2).unwrap_err();
        assert_eq!(
            err_msg(&fixed_many),
            "isinstance expected 2 arguments, got 3"
        );
        // Range form: "at least" below min, "at most" above max.
        let at_least = check_arity_expected_got("getattr", 1, 2, 3).unwrap_err();
        assert_eq!(
            err_msg(&at_least),
            "getattr expected at least 2 arguments, got 1"
        );
        let at_most = check_arity_expected_got("getattr", 4, 2, 3).unwrap_err();
        assert_eq!(
            err_msg(&at_most),
            "getattr expected at most 3 arguments, got 4"
        );
        // Singular noun when the bound is 1.
        let one = check_arity_expected_got("f", 0, 1, 1).unwrap_err();
        assert_eq!(err_msg(&one), "f expected 1 argument, got 0");
    }

    #[test]
    fn unknown_kwarg_rejected() {
        let args = vec![ExpandedCallArg {
            name: Some("bogus".to_string()),
            value: Value::int(1),
        }];
        let err =
            validate_kwargs_and_collect_positional(&args, "open", &["path", "mode"]).unwrap_err();
        assert!(
            err_msg(&err).contains("unexpected keyword argument 'bogus'"),
            "unexpected error message: {:?}",
            err_msg(&err)
        );
    }

    #[test]
    fn allowed_kwarg_keeps_only_positional() {
        let args = vec![
            ExpandedCallArg {
                name: None,
                value: Value::string("/tmp/x"),
            },
            ExpandedCallArg {
                name: Some("mode".to_string()),
                value: Value::string("w"),
            },
        ];
        let positional =
            validate_kwargs_and_collect_positional(&args, "open", &["path", "mode"]).unwrap();
        assert_eq!(positional.len(), 1);
        assert_eq!(positional[0].value.as_str(), Some("/tmp/x"));
    }

    #[test]
    fn locate_arg_positional_then_keyword() {
        let args = vec![ExpandedCallArg {
            name: Some("mode".to_string()),
            value: Value::string("w"),
        }];
        let positional: Vec<&ExpandedCallArg> = args.iter().filter(|a| a.name.is_none()).collect();
        // path is absent; should return None
        let p = locate_arg(&args, &positional, "open", "path", 0, true).unwrap();
        assert!(p.is_none());
        // mode is kw-only; should resolve via the keyword path
        let m = locate_arg(&args, &positional, "open", "mode", 1, true).unwrap();
        assert_eq!(m.and_then(|v| v.as_str()), Some("w"));
    }

    #[test]
    fn locate_arg_rejects_duplicate_pos_and_kw() {
        let args = vec![
            ExpandedCallArg {
                name: None,
                value: Value::string("/tmp/x"),
            },
            ExpandedCallArg {
                name: Some("path".to_string()),
                value: Value::string("/tmp/y"),
            },
        ];
        let positional: Vec<&ExpandedCallArg> = args.iter().filter(|a| a.name.is_none()).collect();
        let err = locate_arg(&args, &positional, "open", "path", 0, true).unwrap_err();
        assert!(
            err_msg(&err).contains("multiple values for argument 'path'"),
            "unexpected error message: {:?}",
            err_msg(&err)
        );
    }
}
