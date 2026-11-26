// Issue #3296: the optional feature states (`gc`, `tracemalloc`,
// `faulthandler`, `sys.monitoring`, `warnings`, `atexit`, `sys` runtime hooks)
// live in one lazily-initialized `SharedFeatureStates` container that is
// shared by `Rc` with filesystem-import children and spawned threads.

fn feature_temp_dir(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
    let unique = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "pyrust-feature-states-{tag}-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn default_interpreter_leaves_every_feature_slot_uninitialized() {
    let mut interp = Interpreter::default();
    assert!(
        interp.features().initialized_slots().is_empty(),
        "fresh interpreter materialized {:?}",
        interp.features().initialized_slots()
    );

    interp.exec_source("pass\n", None, None).unwrap();
    interp
        .exec_source(
            concat!(
                "import sys\n",
                "def f(n):\n",
                "    return n * 2\n",
                "class C:\n",
                "    def m(self):\n",
                "        return [f(i) for i in range(10)]\n",
                "total = sum(C().m())\n",
                "try:\n",
                "    raise ValueError('x')\n",
                "except ValueError:\n",
                "    caught = True\n",
                "limit = sys.getrecursionlimit()\n",
                "interval = sys.getswitchinterval()\n",
                "trace = sys.gettrace()\n",
                "profile = sys.getprofile()\n",
                "depth = sys.get_coroutine_origin_tracking_depth()\n",
                "hooks = sys.get_asyncgen_hooks()\n",
                "sys.audit('pyrust.test.event', 1)\n",
            ),
            None,
            None,
        )
        .unwrap();
    assert_eq!(interp.lookup_name("total").unwrap(), Some(Value::int(90)));
    assert_eq!(
        interp.lookup_name("interval").unwrap(),
        Some(Value::float(0.005))
    );
    assert_eq!(interp.lookup_name("trace").unwrap(), Some(Value::none()));
    assert_eq!(interp.lookup_name("depth").unwrap(), Some(Value::int(0)));

    // Shutdown with no registered callbacks must stay a zero-allocation no-op.
    interp.run_atexit_callbacks();
    interp.sync_vm_hooks_flags();
    assert!(
        interp.features().initialized_slots().is_empty(),
        "read-only defaults materialized {:?}",
        interp.features().initialized_slots()
    );
}

#[test]
fn feature_slots_initialize_only_when_their_feature_is_used() {
    let cases: [(&str, &str); 7] = [
        ("gc", "import gc\ngc.disable()\n"),
        (
            "tracemalloc",
            "import tracemalloc\ntracemalloc.start()\ntracemalloc.stop()\n",
        ),
        (
            "faulthandler",
            "import faulthandler\nfaulthandler.enable()\nfaulthandler.disable()\n",
        ),
        (
            "monitoring",
            "import sys\nsys.monitoring.use_tool_id(3, 'probe')\n",
        ),
        (
            "warnings",
            "import warnings\nwarnings.simplefilter('ignore')\n",
        ),
        (
            "atexit",
            "import atexit\natexit.register(print)\natexit._clear()\n",
        ),
        ("sys", "import sys\nsys.setswitchinterval(0.01)\n"),
    ];
    for (slot, source) in cases {
        let mut interp = Interpreter::default();
        interp.exec_source(source, None, None).unwrap();
        let initialized = interp.features().initialized_slots();
        assert!(
            initialized.contains(&slot),
            "{slot}: expected slot to be initialized by {source:?}, got {initialized:?}"
        );
    }
}

#[test]
fn imported_python_module_initializes_feature_states_shared_with_parent() {
    let temp_dir = feature_temp_dir("child");
    std::fs::write(
        temp_dir.join("feature_states_child.py"),
        concat!(
            "import gc, atexit, warnings, tracemalloc, faulthandler, sys\n",
            "gc.disable()\n",
            "gc.set_threshold(123, 4, 5)\n",
            "def _goodbye():\n",
            "    pass\n",
            "atexit.register(_goodbye)\n",
            "warnings.simplefilter('error')\n",
            "tracemalloc.start(7)\n",
            "faulthandler.enable()\n",
            "sys.monitoring.use_tool_id(4, 'child-tool')\n",
            "sys.setswitchinterval(0.025)\n",
            "sys.set_coroutine_origin_tracking_depth(3)\n",
        ),
    )
    .unwrap();

    let script_path = temp_dir.join("main.py");
    let mut parent = Interpreter::with_script_path_and_args(script_path.to_str().unwrap(), &[]);
    assert!(
        parent.features().initialized_slots().is_empty(),
        "parent materialized {:?} before the child ran",
        parent.features().initialized_slots()
    );

    parent.load_module("feature_states_child").unwrap();

    let mut initialized = parent.features().initialized_slots();
    initialized.sort_unstable();
    assert_eq!(
        initialized,
        [
            "atexit",
            "faulthandler",
            "gc",
            "monitoring",
            "sys",
            "tracemalloc",
            "warnings"
        ],
        "child-initialized slots must be visible through the parent handle"
    );
    assert_eq!(parent.features().atexit_len(), 1);

    parent
        .exec_source(
            concat!(
                "import gc, warnings, tracemalloc, faulthandler, sys\n",
                "gc_enabled = gc.isenabled()\n",
                "gc_threshold = gc.get_threshold()[0]\n",
                "try:\n",
                "    warnings.warn('child policy')\n",
                "except UserWarning:\n",
                "    child_filter_visible = True\n",
                "else:\n",
                "    child_filter_visible = False\n",
                "tm_tracing = tracemalloc.is_tracing()\n",
                "tm_limit = tracemalloc.get_traceback_limit()\n",
                "tracemalloc.stop()\n",
                "fh_enabled = faulthandler.is_enabled()\n",
                "faulthandler.disable()\n",
                "tool = sys.monitoring.get_tool(4)\n",
                "sys.monitoring.free_tool_id(4)\n",
                "interval = sys.getswitchinterval()\n",
                "depth = sys.get_coroutine_origin_tracking_depth()\n",
                "warnings.resetwarnings()\n",
                "import atexit\n",
                "atexit._clear()\n",
            ),
            None,
            None,
        )
        .unwrap();

    assert_eq!(
        parent.lookup_name("gc_enabled").unwrap(),
        Some(Value::bool_(false))
    );
    assert_eq!(
        parent.lookup_name("gc_threshold").unwrap(),
        Some(Value::int(123))
    );
    assert_eq!(
        parent.lookup_name("child_filter_visible").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        parent.lookup_name("tm_tracing").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(parent.lookup_name("tm_limit").unwrap(), Some(Value::int(7)));
    assert_eq!(
        parent.lookup_name("fh_enabled").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        parent.lookup_name("tool").unwrap(),
        Some(Value::string("child-tool"))
    );
    assert_eq!(
        parent.lookup_name("interval").unwrap(),
        Some(Value::float(0.025))
    );
    assert_eq!(parent.lookup_name("depth").unwrap(), Some(Value::int(3)));
    assert_eq!(parent.features().atexit_len(), 0);

    std::fs::remove_dir_all(temp_dir).unwrap();
}

#[test]
fn feature_states_are_isolated_between_root_interpreters() {
    let mut first = Interpreter::default();
    first
        .exec_source(
            "import gc, atexit, sys\ngc.disable()\natexit.register(print)\nsys.setswitchinterval(0.5)\n",
            None,
            None,
        )
        .unwrap();
    let mut second = Interpreter::default();
    second
        .exec_source(
            "import gc, sys\nenabled = gc.isenabled()\ninterval = sys.getswitchinterval()\n",
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        second.lookup_name("enabled").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        second.lookup_name("interval").unwrap(),
        Some(Value::float(0.005))
    );
    assert_eq!(second.features().atexit_len(), 0);
    assert!(!second.features().initialized_slots().contains(&"sys"));
    first
        .exec_source("import atexit\natexit._clear()\n", None, None)
        .unwrap();
}

#[test]
fn shared_feature_states_and_interpreter_stay_compact() {
    use std::mem::size_of;

    eprintln!(
        "STRUCT SIZES (#3296): SharedFeatureStates={} B, Interpreter={} B, InterpreterColdState={} B",
        size_of::<SharedFeatureStates>(),
        size_of::<Interpreter>(),
        size_of::<InterpreterColdState>(),
    );
    assert!(
        size_of::<SharedFeatureStates>() <= 128,
        "SharedFeatureStates grew to {} B (guard <= 128 B; keep slots boxed)",
        size_of::<SharedFeatureStates>()
    );
    assert!(
        size_of::<Interpreter>() <= 256,
        "Interpreter grew to {} B (guard <= 256 B after #3296 moved two Rc fields out)",
        size_of::<Interpreter>()
    );
}

#[test]
fn cli_startup_with_and_without_site_leaves_feature_slots_uninitialized() {
    for no_site in [1, 0] {
        let mut interp = Interpreter::with_command_args(&[]);
        interp.set_runtime_config(RuntimeConfig {
            no_site,
            ..RuntimeConfig::default()
        });
        interp.initialize_cli_startup().unwrap();
        interp.exec_source("pass\n", None, None).unwrap();
        interp.run_atexit_callbacks();
        assert!(
            interp.features().initialized_slots().is_empty(),
            "`-c pass` (no_site={no_site}) materialized {:?}",
            interp.features().initialized_slots()
        );
    }
}

#[test]
fn spawned_thread_initializes_feature_states_shared_with_parent() {
    let mut interp = Interpreter::default();
    interp
        .exec_source(
            concat!(
                "import threading\n",
                "def worker():\n",
                "    import gc, sys\n",
                "    gc.disable()\n",
                "    sys.setswitchinterval(0.02)\n",
                "t = threading.Thread(target=worker)\n",
                "t.start()\n",
                "t.join()\n",
                "import gc, sys\n",
                "gc_enabled = gc.isenabled()\n",
                "interval = sys.getswitchinterval()\n",
            ),
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        interp.lookup_name("gc_enabled").unwrap(),
        Some(Value::bool_(false))
    );
    assert_eq!(
        interp.lookup_name("interval").unwrap(),
        Some(Value::float(0.02))
    );
}

#[test]
fn gc_get_referents_of_builtins_includes_deferred_classes() {
    let mut interp = Interpreter::default();
    interp
        .exec_source(
            concat!(
                "import gc, builtins\n",
                "refs = gc.get_referents(builtins)\n",
                "has_zde = any(r is builtins.ZeroDivisionError for r in refs)\n",
                "has_bytearray = any(r is builtins.bytearray for r in refs)\n",
            ),
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        interp.lookup_name("has_zde").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        interp.lookup_name("has_bytearray").unwrap(),
        Some(Value::bool_(true))
    );
}
