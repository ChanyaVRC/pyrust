// Issue #3294: `builtins` is built in one pass, and its class objects (the
// exception hierarchy and the built-in types) are deferred attributes that are
// resolved on first use and materialized in insertion order on observation.

/// Run `body` on a brand-new thread, so the per-thread `builtins` provider and
/// the class-singleton caches all start out unbuilt.
fn on_fresh_runtime_thread<R: Send + 'static>(body: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(body)
        .expect("spawn runtime thread")
        .join()
        .expect("runtime thread must not panic")
}

fn canonical_builtins() -> Rc<RefCell<PyModule>> {
    let builtins = cached_builtins_module();
    let ValueKind::PyModule(module) = builtins.kind() else {
        panic!("canonical builtins provider must be a module");
    };
    Rc::clone(module)
}

fn global_str(interpreter: &Interpreter, name: &str) -> String {
    interpreter
        .lookup_name(name)
        .unwrap()
        .unwrap_or_else(|| panic!("global `{name}` must be bound"))
        .as_str()
        .unwrap_or_else(|| panic!("global `{name}` must be a str"))
        .to_string()
}

fn global_is_true(interpreter: &Interpreter, name: &str) -> bool {
    interpreter.lookup_name(name).unwrap() == Some(Value::bool_(true))
}

/// Names `builtins` binds before `site` runs, after the five module slots.
/// Function and type names sit at their family declaration position; the
/// exception classes follow in `BUILTINS_EXCEPTION_NAMES` order.
const STARTUP_BUILTINS_HEAD: &[&str] = &[
    "__name__",
    "__doc__",
    "__package__",
    "__loader__",
    "__spec__",
    "chr",
    "ord",
    "bin",
    "oct",
    "hex",
    "ascii",
    "id",
    "repr",
    "hash",
    "abs",
    "divmod",
    "pow",
    "sum",
    "any",
    "all",
    "enumerate",
    "zip",
    "reversed",
    "map",
    "filter",
    "iter",
    "next",
    "aiter",
    "anext",
    "issubclass",
    "delattr",
    "isinstance",
    "type",
    "hasattr",
    "getattr",
    "setattr",
    "vars",
    "globals",
    "locals",
    "__build_class__",
    "exec",
    "eval",
    "compile",
    "dir",
    "len",
    "sorted",
    "min",
    "max",
    "round",
    "list",
    "tuple",
    "bytes",
    "bytearray",
    "complex",
    "set",
    "frozenset",
    "str",
    "int",
    "float",
    "bool",
    "dict",
    "input",
    "print",
    "range",
    "open",
    "format",
    "classmethod",
    "staticmethod",
    "property",
    "super",
    "callable",
    "breakpoint",
    "slice",
    "memoryview",
    "__import__",
    "None",
    "True",
    "False",
    "Ellipsis",
    "NotImplemented",
    "__debug__",
    "object",
];

fn startup_builtins_listing() -> String {
    STARTUP_BUILTINS_HEAD
        .iter()
        .chain(BUILTINS_EXCEPTION_NAMES)
        .copied()
        .collect::<Vec<_>>()
        .join(",")
}

#[test]
fn pass_leaves_exception_hierarchy_and_deferred_builtin_classes_unbuilt() {
    on_fresh_runtime_thread(|| {
        let _interpreter = run_program("pass\n");
        assert!(
            !exc_class_cache_is_initialized(),
            "running `pass` must not build a single exception class"
        );
        assert_eq!(
            crate::builtin_modules::builtins::deferred_class_resolutions(),
            0,
            "running `pass` must not resolve any deferred builtins class"
        );

        let builtins = canonical_builtins();
        let builtins = builtins.borrow();
        assert!(builtins.has_pending_lazy_attrs());
        for (name, value) in &builtins.attrs {
            assert!(
                !matches!(value.kind(), ValueKind::PyClass(_)),
                "builtins.{name} was stored as a class at startup"
            );
        }
        for deferred in [
            "ValueError",
            "BaseException",
            "memoryview",
            "bytearray",
            "int",
        ] {
            assert!(!builtins.attrs.contains_key(deferred));
        }
    });
}

#[test]
fn eager_builtin_functions_do_not_build_exception_classes() {
    on_fresh_runtime_thread(|| {
        let interpreter = run_program(
            "total = len([1, 2, 3]) + abs(-4) + max(1, 2)\nlabel = repr(total) + chr(33)\n",
        );
        assert_eq!(
            interpreter.lookup_name("total").unwrap(),
            Some(Value::int(9))
        );
        assert_eq!(global_str(&interpreter, "label"), "9!");
        assert!(!exc_class_cache_is_initialized());
        assert_eq!(
            crate::builtin_modules::builtins::deferred_class_resolutions(),
            0
        );
    });
}

/// Naming one deferred class resolves that one alone: a rare type does not
/// drag in the exception hierarchy.
#[test]
fn naming_a_builtin_type_leaves_the_exception_hierarchy_unbuilt() {
    on_fresh_runtime_thread(|| {
        let interpreter = run_program(
            "view = memoryview\nimport builtins\nsame = builtins.memoryview is view\n\
             present = hasattr(builtins, 'frozenset')\nmissing = hasattr(builtins, 'nope')\n",
        );
        assert!(global_is_true(&interpreter, "same"));
        assert!(global_is_true(&interpreter, "present"));
        assert!(!global_is_true(&interpreter, "missing"));
        assert!(crate::builtin_modules::builtins::deferred_class_resolutions() > 0);
        assert!(
            !exc_class_cache_is_initialized(),
            "resolving a builtin type must not build the exception hierarchy"
        );
        assert!(canonical_builtins().borrow().has_pending_lazy_attrs());
    });
}

#[test]
fn raising_and_catching_work_from_an_unmaterialized_builtins() {
    on_fresh_runtime_thread(|| {
        assert!(!exc_class_cache_is_initialized());
        let interpreter = run_program(
            "try:\n    1 / 0\nexcept ZeroDivisionError as error:\n    caught = type(error).__name__\n\
             try:\n    raise ValueError('boom')\nexcept (KeyError, ValueError) as error:\n    message = str(error)\n\
             try:\n    undefined_name\nexcept NameError as error:\n    missing = error.name\n\
             import builtins\nidentical = builtins.ZeroDivisionError is ZeroDivisionError\n\
             chained = issubclass(ZeroDivisionError, ArithmeticError)\n",
        );
        assert_eq!(global_str(&interpreter, "caught"), "ZeroDivisionError");
        assert_eq!(global_str(&interpreter, "message"), "boom");
        assert_eq!(global_str(&interpreter, "missing"), "undefined_name");
        assert!(global_is_true(&interpreter, "identical"));
        assert!(global_is_true(&interpreter, "chained"));
        assert!(exc_class_cache_is_initialized());
        // Reads resolve the per-thread singletons; they never materialize.
        assert!(canonical_builtins().borrow().has_pending_lazy_attrs());
    });
}

/// Issue #2918: the namespace lists in its startup order no matter which
/// deferred names were touched first, and in whichever way it is observed.
#[test]
fn vars_builtins_keeps_startup_order_after_out_of_order_access() {
    let expected = startup_builtins_listing();

    let untouched = on_fresh_runtime_thread(|| {
        let interpreter = run_program("import builtins\nnames = ','.join(vars(builtins))\n");
        global_str(&interpreter, "names")
    });
    assert_eq!(
        untouched, expected,
        "builtins startup order changed; update STARTUP_BUILTINS_HEAD only for an intentional \
         change to the builtin families"
    );

    let accessed_early = on_fresh_runtime_thread(|| {
        let interpreter = run_program(
            "import builtins\n\
             touched = (ValueError, zip, ExceptionGroup, memoryview, BaseException, object, int)\n\
             also = (builtins.KeyError, getattr(builtins, 'OSError'), hasattr(builtins, 'slice'))\n\
             names = ','.join(vars(builtins))\n\
             same_dict = vars(builtins) is builtins.__dict__\n\
             dict_names = ','.join(builtins.__dict__)\n\
             count_matches = len(vars(builtins)) == len(names.split(','))\n",
        );
        assert!(global_is_true(&interpreter, "same_dict"));
        assert!(global_is_true(&interpreter, "count_matches"));
        assert_eq!(
            global_str(&interpreter, "dict_names"),
            global_str(&interpreter, "names")
        );
        global_str(&interpreter, "names")
    });
    assert_eq!(accessed_early, expected);

    let listed_by_dir = on_fresh_runtime_thread(|| {
        let interpreter = run_program(
            "import builtins\ntouched = (TypeError, bytearray)\nnames = ','.join(dir(builtins))\n",
        );
        global_str(&interpreter, "names")
    });
    let mut sorted_expected: Vec<&str> = expected.split(',').collect();
    sorted_expected.sort_unstable();
    assert_eq!(listed_by_dir, sorted_expected.join(","));
}

/// Mutations made while the classes are still deferred land exactly where a
/// `dict` would put them once the namespace is materialized.
#[test]
fn builtins_mutations_before_materialization_follow_dict_ordering() {
    on_fresh_runtime_thread(|| {
        let interpreter = run_program(
            "import builtins\n\
             builtins.added_first = 1\n\
             saved_len = builtins.len\n\
             del builtins.len\n\
             builtins.len = saved_len\n\
             builtins.abs = builtins.abs\n\
             pending = ','.join(n for n in dir(builtins) if n in ('added_first', 'ValueError'))\n\
             saved_key_error = builtins.KeyError\n\
             del builtins.KeyError\n\
             builtins.KeyError = saved_key_error\n\
             builtins.OSError = OSError\n\
             names = ','.join(vars(builtins))\n\
             del builtins.added_first\n",
        );
        assert_eq!(
            global_str(&interpreter, "pending"),
            "ValueError,added_first"
        );
        let expected: Vec<&str> = STARTUP_BUILTINS_HEAD
            .iter()
            .chain(BUILTINS_EXCEPTION_NAMES)
            .copied()
            .filter(|name| !matches!(*name, "len" | "KeyError"))
            .chain(["added_first", "len", "KeyError"])
            .collect();
        assert_eq!(global_str(&interpreter, "names"), expected.join(","));
    });
}

#[test]
fn rebinding_a_deferred_builtin_wins_for_every_lookup_path() {
    on_fresh_runtime_thread(|| {
        let interpreter = run_program(
            "import builtins\n\
             def read():\n    return ValueError\n\
             original = read()\n\
             class Custom(Exception):\n    pass\n\
             builtins.ValueError = Custom\n\
             via_global = read() is Custom and ValueError is Custom\n\
             via_attr = builtins.ValueError is Custom and getattr(builtins, 'ValueError') is Custom\n\
             via_dict = vars(builtins)['ValueError'] is Custom\n\
             position_kept = list(vars(builtins)).index('ValueError') < list(vars(builtins)).index('NameError')\n\
             class Other(Exception):\n    pass\n\
             builtins.__dict__['TypeError'] = Other\n\
             dict_write_wins = TypeError is Other and builtins.TypeError is Other\n\
             builtins.ValueError = original\n\
             restored = read() is original\n",
        );
        for flag in [
            "via_global",
            "via_attr",
            "via_dict",
            "position_kept",
            "dict_write_wins",
            "restored",
        ] {
            assert!(global_is_true(&interpreter, flag), "{flag}");
        }
        assert!(!canonical_builtins().borrow().has_pending_lazy_attrs());
    });
}

#[test]
fn deleting_a_deferred_builtin_never_rematerializes_it() {
    on_fresh_runtime_thread(|| {
        let interpreter = run_program(
            "import builtins\n\
             def read():\n    return ValueError\n\
             original = read()\n\
             del builtins.ValueError\n\
             def raises(kind, action):\n    try:\n        action()\n    except kind:\n        return True\n    return False\n\
             global_gone = raises(NameError, read)\n\
             attr_gone = raises(AttributeError, lambda: builtins.ValueError)\n\
             getattr_default = getattr(builtins, 'ValueError', None) is None\n\
             hasattr_gone = not hasattr(builtins, 'ValueError')\n\
             vars_gone = 'ValueError' not in vars(builtins)\n\
             dir_gone = 'ValueError' not in dir(builtins)\n\
             dict_gone = 'ValueError' not in builtins.__dict__\n\
             second_del_fails = raises(AttributeError, lambda: delattr(builtins, 'ValueError'))\n\
             still_gone = raises(NameError, read) and not hasattr(builtins, 'ValueError')\n\
             others_intact = builtins.TypeError is TypeError and KeyError is builtins.KeyError\n\
             class Replacement(Exception):\n    pass\n\
             builtins.ValueError = Replacement\n\
             rebound = read() is Replacement and builtins.ValueError is Replacement\n\
             appended = list(vars(builtins))[-1] == 'ValueError'\n\
             popped = builtins.__dict__.pop('KeyError')\n\
             pop_gone = raises(NameError, lambda: KeyError) and not hasattr(builtins, 'KeyError')\n\
             del builtins.__dict__['OSError']\n\
             dict_del_gone = raises(NameError, lambda: OSError) and 'OSError' not in dir(builtins)\n\
             builtins.ValueError = original\n\
             builtins.KeyError = popped\n",
        );
        for flag in [
            "global_gone",
            "attr_gone",
            "getattr_default",
            "hasattr_gone",
            "vars_gone",
            "dir_gone",
            "dict_gone",
            "second_del_fails",
            "still_gone",
            "others_intact",
            "rebound",
            "appended",
            "pop_gone",
            "dict_del_gone",
        ] {
            assert!(global_is_true(&interpreter, flag), "{flag}");
        }
    });
}

/// Deletion with the classes still deferred, observed only through name and
/// attribute lookup (no `vars`/`dir`/`__dict__` in between).
#[test]
fn deleting_a_deferred_builtin_holds_without_namespace_introspection() {
    on_fresh_runtime_thread(|| {
        let interpreter = run_program(
            "import builtins\n\
             del builtins.memoryview\n\
             def probe():\n    try:\n        memoryview\n    except NameError:\n        return True\n    return False\n\
             first = probe()\n\
             second = probe() and not hasattr(builtins, 'memoryview')\n\
             builtins.memoryview = 42\n\
             rebound = memoryview == 42 and builtins.memoryview == 42\n\
             del builtins.memoryview\n\
             gone_again = probe()\n",
        );
        for flag in ["first", "second", "rebound", "gone_again"] {
            assert!(global_is_true(&interpreter, flag), "{flag}");
        }
    });
}

/// Deferred state lives in the module object.  Each thread owns its own
/// canonical provider, so deleting a name for one interpreter's provider
/// leaves another provider's deferred classes intact.
#[test]
fn deferred_builtins_state_is_per_provider() {
    let deleting = on_fresh_runtime_thread(|| {
        let interpreter = run_program(
            "import builtins\ndel builtins.ValueError\ngone = not hasattr(builtins, 'ValueError')\n",
        );
        global_is_true(&interpreter, "gone")
    });
    assert!(deleting);

    on_fresh_runtime_thread(|| {
        let interpreter = run_program(
            "import builtins\nintact = builtins.ValueError is ValueError and 'ValueError' in dir(builtins)\n",
        );
        assert!(global_is_true(&interpreter, "intact"));
    });

    // Two provider objects on one thread are just as independent.
    on_fresh_runtime_thread(|| {
        let first = crate::builtin_modules::load_builtin_module("builtins").expect("builtins");
        let second = crate::builtin_modules::load_builtin_module("builtins").expect("builtins");
        let (ValueKind::PyModule(first), ValueKind::PyModule(second)) =
            (first.kind(), second.kind())
        else {
            panic!("builtins providers must be modules");
        };
        assert!(first.borrow_mut().remove_attr("ValueError").is_some());
        assert_eq!(first.borrow().get_attr_value("ValueError"), None);
        assert!(!first.borrow().attrs_snapshot().contains_key("ValueError"));

        assert!(second.borrow().has_pending_lazy_attrs());
        assert!(second.borrow().get_attr_value("ValueError").is_some());
        assert!(second.borrow().attrs_snapshot().contains_key("ValueError"));
    });
}

/// The static name table must stay in step with the class cache: it is the
/// cache's order, minus dotted registry keys and module-owned classes.
#[test]
fn builtins_exception_names_match_the_exception_class_cache() {
    let classes = build_exc_class_map();
    let module_owned: HashSet<_> = classes
        .iter()
        .filter(|(name, _)| name.contains('.'))
        .map(|(_, class)| Rc::as_ptr(class))
        .collect();
    let published: Vec<&str> = classes
        .iter()
        .filter(|(name, class)| !name.contains('.') && !module_owned.contains(&Rc::as_ptr(class)))
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(published, BUILTINS_EXCEPTION_NAMES);
}
