#[test]
fn super_two_arg_form_works_after_py_name_migration() {
    // `super` migrated from `calls.rs` into `bodies/builtins.rs` via
    // `#[py_name = "super"]` because `super` is a strict Rust keyword
    // that can't be a raw ident.  This pins both classic two-arg uses.
    let interp = run_program(
        "class A:\n    def f(self): return 'A'\n\
             class B(A):\n    def f(self): return 'B+' + super(B, self).f()\n\
             instance_chain = B().f()\n\
             class C:\n    @classmethod\n    def cm(cls): return 'C'\n\
             class D(C):\n    @classmethod\n    def cm(cls): return 'D+' + super(D, cls).cm()\n\
             class_chain = D.cm()\n",
    );
    assert_eq!(
        interp.lookup_name("instance_chain").unwrap(),
        Some(Value::string("B+A"))
    );
    assert_eq!(
        interp.lookup_name("class_chain").unwrap(),
        Some(Value::string("D+C"))
    );
}

#[test]
fn print_and_str_use_shared_dunder_render() {
    // `print(x)` and `str(x)` route through the same `render_instance_str`
    // helper, so they must produce identical text for instances that
    // define `__str__`, `__repr__`, both, or neither.  Capturing print's
    // output is awkward here, so we verify the `str()` path with each
    // priority tier and trust the shared call site.
    let interp = run_program(
        "class Neither: pass\n\
             class StrOnly:\n    def __str__(self): return 'S'\n\
             class ReprOnly:\n    def __repr__(self): return 'R'\n\
             class Both:\n    def __str__(self): return 'BS'\n    def __repr__(self): return 'BR'\n\
             a = str(Neither())\nb = str(StrOnly())\nc = str(ReprOnly())\nd = str(Both())\n",
    );
    // __str__ wins over __repr__; falls through to __repr__ when only it
    // exists; falls all the way to `<ClassName object>` when neither does.
    assert!(
        matches!(
            interp.lookup_name("a").unwrap(),
            Some(v) if matches!(v.kind(), ValueKind::Str(s) if s.contains("Neither object"))
        ),
        "Neither instance should render as `<Neither object>`"
    );
    assert_eq!(interp.lookup_name("b").unwrap(), Some(Value::string("S")));
    assert_eq!(interp.lookup_name("c").unwrap(), Some(Value::string("R")));
    assert_eq!(interp.lookup_name("d").unwrap(), Some(Value::string("BS")));
}

#[test]
fn dir_covers_every_pyrust_builtins_methods_entry() {
    fn dir_names(interp: &Interpreter, name: &str) -> Vec<String> {
        let v = interp.lookup_name(name).unwrap().unwrap();
        match v.kind() {
            ValueKind::List(items) => items
                .iter()
                .map(|s| match s.kind() {
                    ValueKind::Str(rc) => rc.to_string(),
                    _ => panic!("dir() must return list of str"),
                })
                .collect(),
            _ => panic!("dir() must return a list"),
        }
    }

    let interp = run_program(
        "ds = dir(\"\")\n\
             dl = dir([])\n\
             dt = dir(())\n\
             dd = dir({})\n\
             dset = dir(set())\n",
    );

    let cases: &[(&str, &[&str])] = &[
        ("ds", pyrust_builtins::string::METHODS),
        ("dl", pyrust_builtins::list::METHODS),
        ("dt", pyrust_builtins::tuple::METHODS),
        ("dd", pyrust_builtins::dict::METHODS),
        ("dset", pyrust_builtins::set::METHODS),
    ];

    for (var, expected) in cases {
        let got = dir_names(&interp, var);
        for name in *expected {
            assert!(
                got.iter().any(|g| g == name),
                "dir({var}) missing {name:?}; got {got:?}"
            );
        }
    }

    for (var, attrs) in [
        ("ds", &pyrust_builtins::string::CLASS_ATTRS),
        ("dl", &pyrust_builtins::list::CLASS_ATTRS),
        ("dt", &pyrust_builtins::tuple::CLASS_ATTRS),
        ("dd", &pyrust_builtins::dict::CLASS_ATTRS),
        ("dset", &pyrust_builtins::set::CLASS_ATTRS),
    ] {
        let got = dir_names(&interp, var);
        for attr in attrs.iter() {
            assert!(
                got.iter().any(|name| name == attr.name),
                "dir({var}) missing provider-owned {:?}; got {got:?}",
                attr.name
            );
        }
    }
}

// ── stdlib phase-2 modules (issue #250) ──────────────────────────────────
//
// os.path / functools / itertools / collections live in
// `crates/pyrust/src/builtin_modules/bodies/`.  The tests below pin one
// representative behaviour per public surface — full method coverage
// belongs in CPython-parity test suites once we wire one up.

#[test]
fn os_path_join_handles_absolute_components_like_cpython() {
    // CPython quirk: any absolute component resets the running path.
    // Expected output is platform-specific because `Path::is_absolute`
    // disagrees across OSes — on Unix `/abs` is absolute (so `b` and
    // `c` reset to `/abs/...`); on Windows `/abs` isn't absolute (no
    // drive prefix), so it's just another non-resetting component and
    // separators get mixed.  Mirror CPython by computing the expected
    // strings with the same `PathBuf` ops the impl uses — that keeps
    // the test honest on both platforms without skipping coverage.
    let interp = run_program(
        "import os.path as op\n\
             a = op.join('a', 'b', 'c')\n\
             b = op.join('/abs', 'rel')\n\
             c = op.join('rel', '/abs', 'tail')\n",
    );
    let expect = |parts: &[&str]| {
        let mut p = std::path::PathBuf::new();
        for part in parts {
            let q = std::path::Path::new(part);
            if q.is_absolute() {
                p = q.to_path_buf();
            } else {
                p.push(q);
            }
        }
        Value::string(p.to_string_lossy())
    };
    assert_eq!(
        interp.lookup_name("a").unwrap(),
        Some(expect(&["a", "b", "c"]))
    );
    assert_eq!(
        interp.lookup_name("b").unwrap(),
        Some(expect(&["/abs", "rel"]))
    );
    assert_eq!(
        interp.lookup_name("c").unwrap(),
        Some(expect(&["rel", "/abs", "tail"]))
    );
}

#[test]
fn os_path_splitext_treats_leading_dots_as_basename() {
    // `.bashrc` → ('.bashrc', '') — a leading dot is *not* an
    // extension separator (CPython rule).  Pinning this because it's
    // the easy-to-regress branch in `splitext`.
    let interp = run_program(
        "from os.path import splitext\n\
             a = splitext('foo.tar.gz')\n\
             b = splitext('.bashrc')\n\
             c = splitext('no_ext')\n",
    );
    assert_eq!(
        interp.lookup_name("a").unwrap(),
        Some(Value::tuple(vec![
            Value::string("foo.tar"),
            Value::string(".gz"),
        ]))
    );
    assert_eq!(
        interp.lookup_name("b").unwrap(),
        Some(Value::tuple(vec![
            Value::string(".bashrc"),
            Value::string(""),
        ]))
    );
    assert_eq!(
        interp.lookup_name("c").unwrap(),
        Some(Value::tuple(vec![
            Value::string("no_ext"),
            Value::string("")
        ]))
    );
}

#[test]
fn functools_reduce_with_and_without_initializer() {
    let interp = run_program(
        "from functools import reduce\n\
             a = reduce(lambda x, y: x + y, [1, 2, 3, 4])\n\
             b = reduce(lambda x, y: x + y, [1, 2, 3, 4], 100)\n\
             c = reduce(lambda x, y: x * y, [1, 2, 3, 4])\n\
             d = reduce(lambda x, y: x + y, [], 'seed')\n",
    );
    assert_eq!(interp.lookup_name("a").unwrap(), Some(Value::int(10)));
    assert_eq!(interp.lookup_name("b").unwrap(), Some(Value::int(110)));
    assert_eq!(interp.lookup_name("c").unwrap(), Some(Value::int(24)));
    assert_eq!(
        interp.lookup_name("d").unwrap(),
        Some(Value::string("seed"))
    );
}

#[test]
fn functools_reduce_empty_without_initializer_is_type_error() {
    let err =
        run_program_expect_error("from functools import reduce\nreduce(lambda x, y: x + y, [])\n");
    let msg = err.to_string();
    assert!(
        msg.contains("of empty iterable with no initial value"),
        "expected canonical CPython error wording, got: {msg}"
    );
}
