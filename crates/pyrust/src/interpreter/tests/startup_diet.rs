// Issue #3293: CLI startup compiles no Python (`abc` is native), does not
// import `zipimport`, and builds none of the deferred `sys` attributes until a
// script observes them.

/// A fresh interpreter taken through the same startup the `pyrust` binary
/// runs before executing `-c` code; `no_site` is the `-S` flag.
fn interpreter_after_cli_startup(no_site: bool) -> Interpreter {
    let mut interpreter = Interpreter::default();
    interpreter.set_runtime_config(crate::interpreter::RuntimeConfig {
        no_site: i64::from(no_site),
        ..crate::interpreter::RuntimeConfig::default()
    });
    interpreter
        .initialize_cli_startup()
        .expect("CLI startup must succeed");
    interpreter
}

fn run_source(interpreter: &mut Interpreter, src: &str) {
    let tokens = Lexer::new(src).unwrap().into_tokens();
    let program = Parser::new(tokens).parse_program().unwrap();
    interpreter.exec_program(&program, false).unwrap();
}

fn module_is_loaded(interpreter: &Interpreter, name: &str) -> bool {
    interpreter.cold.module_cache.borrow().contains_key(name)
}

fn cached_sys(interpreter: &Interpreter) -> Rc<RefCell<PyModule>> {
    let sys = interpreter
        .cold
        .module_cache
        .borrow()
        .get("sys")
        .cloned()
        .expect("startup imports sys");
    let ValueKind::PyModule(module) = sys.kind() else {
        panic!("sys must be a module");
    };
    Rc::clone(module)
}

const DEFERRED_SYS_ATTRS: [&str; 10] = [
    "version_info",
    "flags",
    "float_info",
    "int_info",
    "hash_info",
    "implementation",
    "builtin_module_names",
    "stdlib_module_names",
    "thread_info",
    "monitoring",
];

fn assert_sys_is_untouched(interpreter: &Interpreter) {
    let sys = cached_sys(interpreter);
    let sys = sys.borrow();
    assert!(sys.has_pending_lazy_attrs());
    for name in DEFERRED_SYS_ATTRS {
        assert!(
            sys.deferred_attr_is_unresolved(name),
            "sys.{name} was built during startup"
        );
    }
    assert_eq!(
        crate::builtin_modules::sys::deferred_slot_builds(),
        0,
        "startup must not build any deferred sys attribute"
    );
    assert!(
        sys.has_deferred_init(),
        "the zipimport path hook must still be deferred"
    );
}

#[test]
fn cli_startup_without_site_compiles_nothing_and_defers_zipimport_and_sys() {
    on_fresh_runtime_thread(|| {
        let lexed_before = crate::lexer::sources_lexed();
        let mut interpreter = interpreter_after_cli_startup(true);
        assert_eq!(
            crate::lexer::sources_lexed(),
            lexed_before,
            "`-S` startup must not tokenize any Python source"
        );
        run_source(&mut interpreter, "pass\n");

        assert!(module_is_loaded(&interpreter, "sys"));
        assert!(!module_is_loaded(&interpreter, "zipimport"));
        assert!(!module_is_loaded(&interpreter, "site"));
        assert_sys_is_untouched(&interpreter);
        // `zipimport.ZipImportError(ImportError)` used to force the whole
        // exception hierarchy into existence on every startup.
        assert!(
            !exc_class_cache_is_initialized(),
            "`-S -c pass` must not build a single exception class"
        );
    });
}

#[test]
fn default_cli_startup_compiles_no_python_and_defers_zipimport_and_sys() {
    on_fresh_runtime_thread(|| {
        let lexed_before = crate::lexer::sources_lexed();
        let mut interpreter = interpreter_after_cli_startup(false);
        assert_eq!(
            crate::lexer::sources_lexed(),
            lexed_before,
            "default startup (site -> os -> abc) must not tokenize any Python source"
        );
        run_source(&mut interpreter, "pass\n");

        // `site` still publishes the modules CPython's `site.py` imports.
        for name in ["sys", "site", "os", "os.path", "abc", "_sitebuiltins"] {
            assert!(
                module_is_loaded(&interpreter, name),
                "{name} must be loaded"
            );
        }
        assert!(!module_is_loaded(&interpreter, "zipimport"));
        assert_sys_is_untouched(&interpreter);
    });
}

#[test]
fn observing_sys_modules_or_path_hooks_installs_the_zipimport_hook() {
    for probe in [
        "import sys\nseen = 'zipimport' in sys.modules\n",
        "import sys\nseen = len(sys.path_hooks) == 1\n",
        "import sys\nseen = 'path_hooks' in vars(sys)\n",
        "import sys\nseen = 'zipimport' in sys.__dict__['modules']\n",
        "from sys import path_hooks\nseen = len(path_hooks) == 1\n",
        "import sys\nseen = len(getattr(sys, 'path_hooks')) == 1\n",
        "import zipimport, sys\nseen = sys.path_hooks[0] is zipimport.zipimporter\n",
    ] {
        on_fresh_runtime_thread(move || {
            let mut interpreter = interpreter_after_cli_startup(true);
            assert!(!module_is_loaded(&interpreter, "zipimport"));
            run_source(&mut interpreter, probe);
            assert!(global_is_true(&interpreter, "seen"), "probe: {probe}");
            assert!(
                module_is_loaded(&interpreter, "zipimport"),
                "probe: {probe}"
            );
            assert!(!cached_sys(&interpreter).borrow().has_deferred_init());

            // The hook is installed exactly once, as the first entry.
            run_source(
                &mut interpreter,
                "import sys, zipimport\n\
                 hooks = [hook is zipimport.zipimporter for hook in sys.path_hooks]\n\
                 once = hooks == [True]\n",
            );
            assert!(global_is_true(&interpreter, "once"), "probe: {probe}");
        });
    }
}

#[test]
fn rebinding_or_deleting_path_hooks_runs_the_deferred_step_first() {
    on_fresh_runtime_thread(|| {
        let mut interpreter = interpreter_after_cli_startup(true);
        run_source(&mut interpreter, "import sys\nsys.path_hooks = []\n");
        // CPython would have had `zipimport` imported before the rebind too.
        assert!(module_is_loaded(&interpreter, "zipimport"));
        run_source(&mut interpreter, "ok = sys.path_hooks == []\n");
        assert!(global_is_true(&interpreter, "ok"));
    });
    on_fresh_runtime_thread(|| {
        let mut interpreter = interpreter_after_cli_startup(true);
        run_source(
            &mut interpreter,
            "import sys\ndel sys.path_hooks\nok = not hasattr(sys, 'path_hooks')\n",
        );
        assert!(global_is_true(&interpreter, "ok"));
        assert!(module_is_loaded(&interpreter, "zipimport"));
    });
}

#[test]
fn unrelated_sys_access_keeps_zipimport_and_other_slots_deferred() {
    on_fresh_runtime_thread(|| {
        let mut interpreter = interpreter_after_cli_startup(true);
        run_source(
            &mut interpreter,
            "import sys\n\
             a = sys.argv\n\
             b = sys.path\n\
             c = sys.maxsize\n\
             d = sys.getrecursionlimit()\n\
             e = hasattr(sys, 'no_such_attribute')\n\
             sys.custom = 1\n\
             f = sys.path_importer_cache\n\
             g = sys.meta_path\n",
        );
        assert!(!module_is_loaded(&interpreter, "zipimport"));
        assert_sys_is_untouched(&interpreter);

        // Reading one deferred slot builds exactly that slot.
        run_source(&mut interpreter, "limit = sys.flags.no_site == 1\n");
        assert!(global_is_true(&interpreter, "limit"));
        assert_eq!(crate::builtin_modules::sys::deferred_slot_builds(), 0);
        let sys = cached_sys(&interpreter);
        assert!(!sys.borrow().deferred_attr_is_unresolved("flags"));
        assert!(sys.borrow().deferred_attr_is_unresolved("monitoring"));
        assert!(
            sys.borrow()
                .deferred_attr_is_unresolved("stdlib_module_names")
        );
        assert!(sys.borrow().has_pending_lazy_attrs());
        assert!(!module_is_loaded(&interpreter, "zipimport"));
    });
}

#[test]
fn deferred_sys_attrs_follow_dict_semantics_for_rebind_and_delete() {
    on_fresh_runtime_thread(|| {
        let mut interpreter = interpreter_after_cli_startup(true);
        run_source(
            &mut interpreter,
            "import sys\n\
             first = sys.monitoring\n\
             same = sys.monitoring is first\n\
             del sys.monitoring\n\
             gone = not hasattr(sys, 'monitoring')\n\
             unlisted = 'monitoring' not in dir(sys) and 'monitoring' not in vars(sys)\n\
             others = hasattr(sys, 'float_info') and len(sys.stdlib_module_names) == 300\n\
             try:\n\
             \x20   del sys.monitoring\n\
             \x20   twice = False\n\
             except AttributeError:\n\
             \x20   twice = True\n\
             sys.monitoring = 'rebound'\n\
             rebound = sys.monitoring == 'rebound' and list(vars(sys))[-1] == 'monitoring'\n",
        );
        for name in ["same", "gone", "unlisted", "others", "twice", "rebound"] {
            assert!(global_is_true(&interpreter, name), "{name}");
        }
        assert!(!cached_sys(&interpreter).borrow().has_pending_lazy_attrs());
    });
    on_fresh_runtime_thread(|| {
        let mut interpreter = interpreter_after_cli_startup(true);
        run_source(
            &mut interpreter,
            "import sys\n\
             sys.stdlib_module_names = frozenset({'only'})\n\
             kept = sys.stdlib_module_names == frozenset({'only'})\n\
             names = list(vars(sys))\n\
             in_place = names.index('stdlib_module_names') < names.index('monitoring')\n\
             head = names[:5] == ['__name__', '__doc__', '__package__', '__loader__', '__spec__']\n\
             live = sys.__dict__ is vars(sys) and vars(sys)['flags'] is sys.flags\n",
        );
        for name in ["kept", "in_place", "head", "live"] {
            assert!(global_is_true(&interpreter, name), "{name}");
        }
    });
}

#[test]
fn sys_flags_reflect_the_runtime_configuration_when_first_read() {
    on_fresh_runtime_thread(|| {
        let mut interpreter = Interpreter::default();
        interpreter.set_runtime_config(crate::interpreter::RuntimeConfig {
            no_site: 1,
            optimize: 2,
            dont_write_bytecode: 1,
            isolated: 1,
            ignore_environment: 1,
            no_user_site: 1,
            safe_path: true,
            ..crate::interpreter::RuntimeConfig::default()
        });
        interpreter.initialize_cli_startup().unwrap();
        assert!(
            cached_sys(&interpreter)
                .borrow()
                .deferred_attr_is_unresolved("flags")
        );
        run_source(
            &mut interpreter,
            "import sys\n\
             f = sys.flags\n\
             ok = (f.optimize, f.dont_write_bytecode, f.isolated, f.ignore_environment,\n\
             \x20     f.no_user_site, f.no_site, f.safe_path) == (2, 1, 1, 1, 1, 1, True)\n\
             seq = isinstance(f, tuple) and len(f) == 18 and f is sys.flags\n",
        );
        assert!(global_is_true(&interpreter, "ok"));
        assert!(global_is_true(&interpreter, "seq"));
    });
}

#[test]
fn a_zip_archive_on_sys_path_installs_the_hook_without_touching_path_hooks() {
    on_fresh_runtime_thread(|| {
        let dir = std::env::temp_dir().join(format!(
            "pyrust_startup_diet_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("bundle_3293.zip");
        let missing = dir.join("does_not_exist");
        let literal = |path: &std::path::Path| format!("{:?}", path.to_string_lossy());

        let mut interpreter = interpreter_after_cli_startup(true);
        // A directory and a missing entry are rejected by `zipimporter`
        // either way, so probing them leaves the hook deferred; the missing
        // entry is still recorded as having no finder, as with the hook.
        run_source(
            &mut interpreter,
            &format!(
                "import sys\n\
                 sys.path.insert(0, {missing})\n\
                 sys.path.insert(0, {dir})\n\
                 try:\n\
                 \x20   import no_such_module_3293\n\
                 except ImportError:\n\
                 \x20   pass\n\
                 cached_none = sys.path_importer_cache.get({missing}, 0) is None\n",
                missing = literal(&missing),
                dir = literal(&dir),
            ),
        );
        assert!(global_is_true(&interpreter, "cached_none"));
        assert!(!module_is_loaded(&interpreter, "zipimport"));

        run_source(
            &mut interpreter,
            &format!(
                "import zipfile\n\
                 with zipfile.ZipFile({archive}, 'w') as zf:\n\
                 \x20   zf.writestr('zipped_mod_3293.py', 'VALUE = 41 + 1\\n')\n",
                archive = literal(&archive),
            ),
        );
        assert!(!module_is_loaded(&interpreter, "zipimport"));
        run_source(
            &mut interpreter,
            &format!(
                "sys.path.insert(0, {archive})\n\
                 import zipped_mod_3293\n\
                 ok = zipped_mod_3293.VALUE == 42\n\
                 finder = type(sys.path_importer_cache[{archive}]).__name__ == 'zipimporter'\n",
                archive = literal(&archive),
            ),
        );
        assert!(global_is_true(&interpreter, "ok"));
        assert!(global_is_true(&interpreter, "finder"));
        assert!(module_is_loaded(&interpreter, "zipimport"));
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn abc_is_native_and_enforces_abstract_instantiation() {
    on_fresh_runtime_thread(|| {
        let lexed_before = crate::lexer::sources_lexed();
        let mut interpreter = Interpreter::default();
        interpreter.load_module("abc").unwrap();
        interpreter.load_module("os").unwrap();
        assert_eq!(
            crate::lexer::sources_lexed(),
            lexed_before,
            "importing abc / os must not compile Python source"
        );
        run_source(
            &mut interpreter,
            "import abc, os\n\
             class Shape(abc.ABC):\n\
             \x20   @abc.abstractmethod\n\
             \x20   def area(self): ...\n\
             try:\n\
             \x20   Shape()\n\
             \x20   message = ''\n\
             except TypeError as exc:\n\
             \x20   message = str(exc)\n\
             class Square(Shape):\n\
             \x20   def area(self): return 4\n\
             class Duck: pass\n\
             token = abc.get_cache_token()\n\
             Shape.register(Duck)\n\
             checks = (\n\
             \x20   Square().area() == 4 and isinstance(Duck(), Shape)\n\
             \x20   and issubclass(Duck, Shape) and not issubclass(int, Shape)\n\
             \x20   and abc.get_cache_token() == token + 1\n\
             \x20   and type(Shape) is abc.ABCMeta and type(os.PathLike) is abc.ABCMeta\n\
             \x20   and '__call__' not in vars(abc.ABCMeta) and '_abc_impl' in vars(Shape)\n\
             )\n",
        );
        assert_eq!(
            global_str(&interpreter, "message"),
            "Can't instantiate abstract class Shape without an implementation for \
             abstract method 'area'"
        );
        assert!(global_is_true(&interpreter, "checks"));
    });
}
