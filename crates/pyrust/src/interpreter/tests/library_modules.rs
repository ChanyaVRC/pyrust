#[test]
fn itertools_chain_concatenates_iterables() {
    let interp = run_program(
        "from itertools import chain\n\
             a = list(chain([1, 2], [3, 4], [5]))\n\
             b = list(chain([]))\n\
             c = list(chain())\n",
    );
    assert_eq!(
        interp.lookup_name("a").unwrap(),
        Some(Value::list((1..=5).map(Value::int).collect()))
    );
    assert_eq!(interp.lookup_name("b").unwrap(), Some(Value::list(vec![])));
    assert_eq!(interp.lookup_name("c").unwrap(), Some(Value::list(vec![])));
}

#[test]
fn itertools_islice_covers_all_arities() {
    // `islice(seq, stop)`, `islice(seq, start, stop)`, and
    // `islice(seq, start, stop, step)` — plus `None` in each slot
    // (which means "default": 0 / drain / 1).
    let interp = run_program(
        "from itertools import islice\n\
             a = list(islice([0,1,2,3,4,5,6,7,8,9], 5))\n\
             b = list(islice([0,1,2,3,4,5,6,7,8,9], 2, 7))\n\
             c = list(islice([0,1,2,3,4,5,6,7,8,9], 0, 10, 2))\n\
             d = list(islice(range(5), None, 10))\n\
             e = list(islice(range(5), 1, None))\n",
    );
    assert_eq!(
        interp.lookup_name("a").unwrap(),
        Some(Value::list((0..5).map(Value::int).collect()))
    );
    assert_eq!(
        interp.lookup_name("b").unwrap(),
        Some(Value::list((2..7).map(Value::int).collect()))
    );
    assert_eq!(
        interp.lookup_name("c").unwrap(),
        Some(Value::list((0..10).step_by(2).map(Value::int).collect()))
    );
    assert_eq!(
        interp.lookup_name("d").unwrap(),
        Some(Value::list((0..5).map(Value::int).collect()))
    );
    assert_eq!(
        interp.lookup_name("e").unwrap(),
        Some(Value::list((1..5).map(Value::int).collect()))
    );
}

#[test]
fn collections_counter_tallies_iterables() {
    // Counter is now a real Python class (defined via `pyrust_module!`'s
    // `class { … }` block).  Pin the counts via `c[key]` (which routes
    // through `__getitem__`) and `len(c)` rather than comparing to a
    // plain dict — Counter instances are PyInstances, not dicts.
    let interp = run_program(
        "from collections import Counter\n\
             a = Counter([1, 2, 1, 3, 2, 1])\n\
             a_one = a[1]\n\
             a_two = a[2]\n\
             a_three = a[3]\n\
             a_missing = a[99]\n\
             a_len = len(a)\n\
             b = Counter('aabcccd')\n\
             b_a = b['a']\n\
             b_c = b['c']\n\
             c = Counter()\n\
             c_len = len(c)\n\
             c_missing = c['anything']\n\
             d = Counter({'x': 5, 'y': 3})\n\
             d_x = d['x']\n\
             d_y = d['y']\n",
    );
    // Counter([1, 2, 1, 3, 2, 1])
    assert_eq!(interp.lookup_name("a_one").unwrap(), Some(Value::int(3)));
    assert_eq!(interp.lookup_name("a_two").unwrap(), Some(Value::int(2)));
    assert_eq!(interp.lookup_name("a_three").unwrap(), Some(Value::int(1)));
    // Missing-key returns 0 (the dict-subclass quirk).
    assert_eq!(
        interp.lookup_name("a_missing").unwrap(),
        Some(Value::int(0))
    );
    assert_eq!(interp.lookup_name("a_len").unwrap(), Some(Value::int(3)));
    // Counter('aabcccd')
    assert_eq!(interp.lookup_name("b_a").unwrap(), Some(Value::int(2)));
    assert_eq!(interp.lookup_name("b_c").unwrap(), Some(Value::int(3)));
    // Counter() — empty.
    assert_eq!(interp.lookup_name("c_len").unwrap(), Some(Value::int(0)));
    assert_eq!(
        interp.lookup_name("c_missing").unwrap(),
        Some(Value::int(0))
    );
    // Counter({'x': 5, 'y': 3}) — mapping form preserves the values.
    assert_eq!(interp.lookup_name("d_x").unwrap(), Some(Value::int(5)));
    assert_eq!(interp.lookup_name("d_y").unwrap(), Some(Value::int(3)));
}

#[test]
fn os_path_dotted_import_works_via_parent_package() {
    // The `os` parent module is synthesised in `bodies/os.rs` and
    // its `path` constant points at the `os.path` module, so the
    // bare `import os.path; os.path.join(...)` pattern (which the
    // compiler binds under the topmost component `os`) resolves.
    let interp = run_program("import os.path\nresult = os.path.join('a', 'b')\n");
    let sep = std::path::MAIN_SEPARATOR;
    assert_eq!(
        interp.lookup_name("result").unwrap(),
        Some(Value::string(format!("a{sep}b")))
    );
}

#[test]
fn itertools_islice_is_lazy_over_huge_source() {
    // The point of moving islice off the eager `Vec<Value>` path is
    // that it must *not* drain a huge source when the consumer only
    // asks for a handful.  Pull three elements from a 100k-range and
    // confirm the rest never materialises by relying on the test
    // wall-clock; with the eager implementation, the same test ran
    // visibly slower because it had to walk the full range.
    let interp = run_program(
        "from itertools import islice\n\
             it = islice(range(100000), 3)\n\
             a = next(it); b = next(it); c = next(it)\n",
    );
    assert_eq!(interp.lookup_name("a").unwrap(), Some(Value::int(0)));
    assert_eq!(interp.lookup_name("b").unwrap(), Some(Value::int(1)));
    assert_eq!(interp.lookup_name("c").unwrap(), Some(Value::int(2)));
}

#[test]
fn collections_counter_exposes_full_method_surface() {
    // Counter is now a real BuiltinObject — pin each of the methods
    // that lights up only with the BuiltinTypeOps implementation
    // (missing-key returns 0, most_common, elements, update,
    // subtract, copy independence).
    let interp = run_program(
        "from collections import Counter\n\
             c = Counter('aabbc')\n\
             missing = c['z']\n\
             top2 = c.most_common(2)\n\
             elts = list(c.elements())\n\
             c.update('aa')\n\
             after_update = c['a']\n\
             c.subtract('aaaaa')\n\
             after_subtract = c['a']\n\
             c2 = c.copy()\n\
             c2['a'] = 999\n\
             original_a = c['a']\n\
             copy_a = c2['a']\n",
    );
    assert_eq!(interp.lookup_name("missing").unwrap(), Some(Value::int(0)));
    assert_eq!(
        interp.lookup_name("top2").unwrap(),
        Some(Value::list(vec![
            Value::tuple(vec![Value::string("a"), Value::int(2)]),
            Value::tuple(vec![Value::string("b"), Value::int(2)]),
        ]))
    );
    // elements() lists 'a' twice, 'b' twice, 'c' once — insertion
    // order preserved.
    assert_eq!(
        interp.lookup_name("elts").unwrap(),
        Some(Value::list(vec![
            Value::string("a"),
            Value::string("a"),
            Value::string("b"),
            Value::string("b"),
            Value::string("c"),
        ]))
    );
    assert_eq!(
        interp.lookup_name("after_update").unwrap(),
        Some(Value::int(4))
    );
    assert_eq!(
        interp.lookup_name("after_subtract").unwrap(),
        Some(Value::int(-1))
    );
    assert_eq!(
        interp.lookup_name("original_a").unwrap(),
        Some(Value::int(-1))
    );
    assert_eq!(interp.lookup_name("copy_a").unwrap(), Some(Value::int(999)));
}

#[test]
fn collections_defaultdict_runs_factory_on_missing_key() {
    // Two complementary uses pin the missing-key dispatch:
    //
    //   - `defaultdict(int)` for the canonical `counts[c] += 1` idiom
    //     — `+=` re-binds via `set_item` so the increment persists
    //     across iterations, matching CPython.
    //   - `defaultdict(None)` falls through to KeyError, matching a
    //     plain dict — there's no factory to call.
    let interp = run_program(
        "from collections import defaultdict\n\
             counts = defaultdict(int)\n\
             for c in 'aabbbc':\n    \
             counts[c] += 1\n\
             a = counts['a']\n\
             b = counts['b']\n\
             c = counts['c']\n",
    );
    assert_eq!(interp.lookup_name("a").unwrap(), Some(Value::int(2)));
    assert_eq!(interp.lookup_name("b").unwrap(), Some(Value::int(3)));
    assert_eq!(interp.lookup_name("c").unwrap(), Some(Value::int(1)));
}

#[test]
fn collections_defaultdict_none_factory_raises_key_error() {
    // `defaultdict(None)` matches plain dict semantics: missing key
    // raises KeyError instead of running a factory.  The behaviour
    // is driven by `defaultdict.__missing__` checking
    // `self.default_factory is None` and short-circuiting to
    // KeyError when so — pin both halves of that branch.
    let err = run_program_expect_error(
        "from collections import defaultdict\nd = defaultdict(None)\nd['missing']\n",
    );
    let msg = err.to_string();
    assert!(msg.contains("KeyError"), "expected KeyError, got: {msg}");
}

#[test]
fn collections_counter_iterates_keys_in_insertion_order() {
    // This pins the original bug that motivated migrating Counter to a
    // class-based implementation: the previous `BuiltinTypeOps` Counter
    // returned `None` from `iter_next`, so `for k in c` and `list(c)`
    // both silently yielded nothing.  With `__iter__` defined as a
    // dunder, iteration goes through pyrust's normal class machinery.
    let interp = run_program(
        "from collections import Counter\n\
             c = Counter('aab')\n\
             keys_list = list(c)\n\
             # Re-iteration must work too (each iter(c) takes a fresh snapshot).\n\
             keys_again = list(c)\n",
    );
    // Insertion order: 'a' (first seen), 'b' (second seen).
    assert_eq!(
        interp.lookup_name("keys_list").unwrap(),
        Some(Value::list(vec![Value::string("a"), Value::string("b")]))
    );
    assert_eq!(
        interp.lookup_name("keys_again").unwrap(),
        Some(Value::list(vec![Value::string("a"), Value::string("b")]))
    );
}

#[test]
fn collections_counter_dunder_dispatch_exercises_each_site() {
    // The dispatch unification in `invoke_class_method` routes
    // `__contains__`, `__setitem__`, `__len__`, and `__getitem__`
    // through the same helper.  One end-to-end Python program
    // exercising each ensures the helper handles every dispatch
    // site (we'd otherwise only cover `__iter__` and
    // `__getitem__` via the existing tests).
    let interp = run_program(
        "from collections import Counter\n\
             c = Counter('aab')\n\
             a_present = 'a' in c\n\
             z_missing = 'z' in c\n\
             length = len(c)\n\
             before = c['a']\n\
             c['a'] = 99\n\
             after = c['a']\n\
             # __setitem__ propagation: a fresh `[]` lookup should see\n\
             # the new value (which proves set_item routed through the\n\
             # class dunder rather than landing on a clone).\n\
             after_again = c['a']\n",
    );
    assert_eq!(
        interp.lookup_name("a_present").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        interp.lookup_name("z_missing").unwrap(),
        Some(Value::bool_(false))
    );
    assert_eq!(interp.lookup_name("length").unwrap(), Some(Value::int(2)));
    assert_eq!(interp.lookup_name("before").unwrap(), Some(Value::int(2)));
    assert_eq!(interp.lookup_name("after").unwrap(), Some(Value::int(99)));
    assert_eq!(
        interp.lookup_name("after_again").unwrap(),
        Some(Value::int(99))
    );
}

#[test]
fn collections_counter_corrupted_counts_surfaces_type_error() {
    // `c.__builtin_data__ = "lol"` overwrites the internal storage with
    // a non-dict.  (Issue #2010 moved the backing dict to the
    // `__builtin_data__` slot that the generic dict-subclass machinery
    // reads, since Counter is now a real `dict` subclass.)  The next
    // `c[k]` access should surface a TypeError pointing at the user's
    // tampering — not a `Runtime("internal: …")` that looks like an
    // interpreter bug.
    let err = run_program_expect_error(
        "from collections import Counter\nc = Counter('a')\nc.__builtin_data__ = 'lol'\nc['a']\n",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("TypeError"),
        "expected TypeError diagnostic, got: {msg}"
    );
    assert!(
        msg.contains("backing store"),
        "error should describe the corrupted backing store, got: {msg}"
    );
}

#[test]
fn collections_counter_is_a_class_instance() {
    // After the migration to `pyrust_module!`'s `class { … }` block,
    // `Counter(...)` returns a real PyInstance whose class name is
    // exactly `"Counter"` (not `"collections.Counter"`).  This pins the
    // `class_name_lit` codepath in the macro's class emission.
    let interp = run_program(
        "from collections import Counter\n\
             c = Counter([1, 2])\n\
             tname = type(c).__name__\n",
    );
    assert_eq!(
        interp.lookup_name("tname").unwrap(),
        Some(Value::string("Counter"))
    );
}
#[test]
fn sys_modules_is_owned_by_each_root_interpreter() {
    let mut first = Interpreter::default();
    let first_registry = first.import_module_registry().unwrap();
    first_registry
        .dict_insert(PyKey::str_from("_first_only"), Value::int(1))
        .unwrap();
    let first_sys = first.load_module("sys").unwrap();
    let ValueKind::PyModule(first_sys) = first_sys.kind() else {
        panic!("sys must be a module");
    };
    let first_exposed = first_sys
        .borrow()
        .get_attr_value("modules")
        .expect("sys.modules");
    assert_eq!(first_exposed.value_id(), first_registry.value_id());

    let mut second = Interpreter::default();
    let second_registry = second.import_module_registry().unwrap();
    assert_ne!(first_registry.value_id(), second_registry.value_id());
    assert_eq!(
        second_registry.dict_with(|modules| modules.contains_key(&PyKey::str_from("_first_only"))),
        Some(false)
    );

    let second_sys = second.load_module("sys").unwrap();
    let ValueKind::PyModule(second_sys) = second_sys.kind() else {
        panic!("sys must be a module");
    };
    let second_exposed = second_sys
        .borrow()
        .get_attr_value("modules")
        .expect("sys.modules");
    assert_eq!(second_exposed.value_id(), second_registry.value_id());
}

#[test]
fn sys_modules_attribute_replacement_controls_imports() {
    let mut interpreter = Interpreter::default();
    let system_module = interpreter.load_module("sys").unwrap();
    let replacement = Value::dict(PyDict::default());

    interpreter
        .assign_attr(system_module.clone(), "modules", replacement.clone())
        .unwrap();
    let math_module = interpreter.load_module("math").unwrap();
    let registered_math = replacement
        .dict_with(|modules| modules.get(&PyKey::str_from("math")).cloned())
        .flatten()
        .expect("the replacement sys.modules must receive the import");
    assert_eq!(registered_math.value_id(), math_module.value_id());

    interpreter
        .assign_attr(system_module.clone(), "modules", Value::int(1))
        .unwrap();
    let invalid_error = interpreter.load_module("math").unwrap_err();
    assert!(invalid_error.class_name_is("AttributeError"));
    assert!(
        invalid_error
            .to_string()
            .contains("'int' object has no attribute 'get'")
    );

    interpreter
        .assign_attr(system_module.clone(), "modules", replacement)
        .unwrap();
    interpreter.delete_attr(system_module, "modules").unwrap();
    let missing_error = interpreter.load_module("math").unwrap_err();
    assert!(missing_error.class_name_is("AttributeError"));
    assert!(
        missing_error
            .to_string()
            .contains("module 'sys' has no attribute 'modules'")
    );
}

#[test]
fn sys_live_dict_alias_controls_imports_and_vars_identity() {
    let interpreter = run_program(
        r#"import sys
original = sys.modules
replacement = {}
namespace = sys.__dict__
same_vars = vars(sys) is namespace
namespace["modules"] = replacement
import math
replacement_used = sys.modules is replacement and replacement["math"] is math
del namespace["modules"]
try:
    import math
except AttributeError:
    deletion_observed = True
else:
    deletion_observed = False
namespace["modules"] = original
"#,
    );
    assert_eq!(
        interpreter.lookup_name("same_vars").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        interpreter.lookup_name("replacement_used").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        interpreter.lookup_name("deletion_observed").unwrap(),
        Some(Value::bool_(true))
    );
}

#[test]
fn sys_modules_preserves_dict_subclass_protocol_overrides() {
    let interpreter = run_program(
        r#"import sys
original = sys.modules
class Registry(dict):
    def get(self, name, default=None):
        self.get_seen = name == "virtual_registry_module" or self.get_seen
        if name == "virtual_registry_module":
            return 42
        return dict.get(self, name, default)
    def __setitem__(self, name, value):
        self.set_seen = name == "math" or self.set_seen
        return dict.__setitem__(self, name, value)
replacement = Registry(original)
replacement.get_seen = False
replacement.set_seen = False
replacement.pop("math", None)
sys.modules = replacement
import virtual_registry_module
import math
subclass_used = replacement["math"] is math
get_override_used = virtual_registry_module == 42 and replacement.get_seen
set_override_used = replacement.set_seen
sys.modules = original
"#,
    );
    assert_eq!(
        interpreter.lookup_name("subclass_used").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        interpreter.lookup_name("get_override_used").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        interpreter.lookup_name("set_override_used").unwrap(),
        Some(Value::bool_(true))
    );
}

#[test]
fn sys_modules_accepts_arbitrary_mapping_protocol() {
    let interpreter = run_program(
        r#"import sys
original = sys.modules
class Registry:
    def __init__(self, initial):
        self.data = dict(initial)
    def get(self, name, default=None):
        if name == "virtual_protocol_registry_module":
            return 84
        return self.data.get(name, default)
    def __setitem__(self, name, value):
        self.data[name] = value
    def __delitem__(self, name):
        del self.data[name]
replacement = Registry(original)
replacement.data.pop("math", None)
sys.modules = replacement
import virtual_protocol_registry_module
import math
virtual_used = virtual_protocol_registry_module == 84
mapping_received_module = replacement.data["math"] is math
sys.modules = original
"#,
    );
    assert_eq!(
        interpreter.lookup_name("virtual_used").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        interpreter.lookup_name("mapping_received_module").unwrap(),
        Some(Value::bool_(true))
    );
}

#[test]
fn replacement_registry_does_not_override_or_republish_internal_sys() {
    let mut interpreter = Interpreter::default();
    let original = interpreter.load_module("sys").unwrap();
    let replacement = Value::dict(PyDict::default());
    interpreter
        .assign_attr(original.clone(), "modules", replacement.clone())
        .unwrap();

    let imported_again = interpreter.load_module("sys").unwrap();
    assert_eq!(imported_again.value_id(), original.value_id());
    assert_eq!(
        replacement.dict_with(|modules| modules.contains_key(&PyKey::str_from("sys"))),
        Some(false),
        "an interpreter-cache hit must not publish into a replacement mapping"
    );
}

#[test]
fn deleting_sys_from_original_registry_creates_a_new_import_identity() {
    let mut interpreter = Interpreter::default();
    let original = interpreter.load_module("sys").unwrap();
    interpreter
        .bootstrap_module_registry
        .dict_shift_remove(&PyKey::str_from("sys"))
        .unwrap();

    let imported_again = interpreter.load_module("sys").unwrap();
    assert_ne!(imported_again.value_id(), original.value_id());
    let registered = interpreter
        .bootstrap_module_registry
        .dict_with(|modules| modules.get(&PyKey::str_from("sys")).cloned())
        .flatten()
        .expect("the fresh sys identity must be registered in the original dict");
    assert_eq!(registered.value_id(), imported_again.value_id());
}

#[test]
fn replacement_registry_only_controls_names_missing_from_internal_cache() {
    let mut interpreter = Interpreter::default();
    let system_module = interpreter.load_module("sys").unwrap();
    let original_math = interpreter.load_module("math").unwrap();
    let fake_math = Value::int(42);
    let replacement = Value::dict(PyDict::default());
    replacement
        .dict_insert(PyKey::str_from("math"), fake_math.clone())
        .unwrap();
    interpreter
        .assign_attr(system_module, "modules", replacement)
        .unwrap();

    let still_internal = interpreter.load_module("math").unwrap();
    assert_eq!(still_internal.value_id(), original_math.value_id());

    interpreter
        .bootstrap_module_registry
        .dict_shift_remove(&PyKey::str_from("math"))
        .unwrap();
    assert_eq!(interpreter.load_module("math").unwrap(), fake_math);
}

#[test]
fn none_in_sys_modules_halts_import_before_internal_cache_recovery() {
    let interpreter = run_program(
        r#"import sys
sys.modules["sys"] = None
try:
    import sys as blocked
except Exception as error:
    error_type = type(error).__name__
    error_text = str(error)
"#,
    );
    assert_eq!(
        interpreter.lookup_name("error_type").unwrap(),
        Some(Value::string("ModuleNotFoundError"))
    );
    assert_eq!(
        interpreter.lookup_name("error_text").unwrap(),
        Some(Value::string("import of sys halted; None in sys.modules"))
    );
}

#[test]
fn import_registry_fast_path_does_not_retain_replaced_dictionary() {
    let mut interpreter = Interpreter::default();
    let system_module = interpreter.load_module("sys").unwrap();
    let replacement = Value::dict(PyDict::default());
    let replacement_backing = Rc::downgrade(
        replacement
            .get_dict_rc()
            .expect("replacement must be a dict"),
    );

    interpreter
        .assign_attr(system_module.clone(), "modules", replacement.clone())
        .unwrap();
    let resolved = interpreter.import_module_registry().unwrap();
    assert_eq!(resolved.value_id(), replacement.value_id());
    drop(resolved);

    interpreter
        .assign_attr(system_module, "modules", Value::dict(PyDict::default()))
        .unwrap();
    drop(replacement);
    assert!(
        replacement_backing.upgrade().is_none(),
        "the registry fast path must be weak so replacement preserves object lifetimes"
    );
}

#[test]
fn replacement_registry_disables_module_class_cache_and_respects_internal_priority() {
    let mut interpreter = Interpreter::default();
    let system_module = interpreter.load_module("sys").unwrap();
    let typing_module = interpreter.load_module("typing").unwrap();
    let cache_slot = ModuleClassCacheSlot::new(0);
    let original_alias_class = interpreter
        .cached_module_class(cache_slot, "typing", "_GenericAlias")
        .unwrap();
    assert!(
        interpreter
            .module_class_cache
            .as_ref()
            .and_then(|cache| cache.entries[cache_slot.0].as_ref())
            .is_some(),
        "the ordinary original-dict path should retain its hot cache"
    );

    let ValueKind::PyModule(typing_module) = typing_module.kind() else {
        panic!("typing must be a module");
    };
    let replacement_alias_class = typing_module
        .borrow()
        .get_attr_value("Protocol")
        .and_then(|value| match value.kind() {
            ValueKind::PyClass(class) => Some(Rc::clone(class)),
            _ => None,
        })
        .expect("typing.Protocol must be a class");
    assert!(!Rc::ptr_eq(&original_alias_class, &replacement_alias_class));

    let mut replacement_attrs = crate::value::ModuleAttrs::default();
    replacement_attrs.insert(
        "_GenericAlias".to_string(),
        Value::py_class(Rc::clone(&replacement_alias_class)),
    );
    let replacement_typing = Value::py_module(Rc::new(RefCell::new(PyModule::new(
        "typing".to_string(),
        replacement_attrs,
    ))));
    let replacement_registry = Value::dict(PyDict::default());
    replacement_registry
        .dict_insert(PyKey::str_from("typing"), replacement_typing)
        .unwrap();
    interpreter
        .assign_attr(system_module, "modules", replacement_registry)
        .unwrap();

    let resolved_after_replacement = interpreter
        .cached_module_class(cache_slot, "typing", "_GenericAlias")
        .unwrap();
    assert!(Rc::ptr_eq(
        &resolved_after_replacement,
        &original_alias_class
    ));
    assert!(
        interpreter
            .module_class_cache
            .as_ref()
            .and_then(|cache| cache.entries[cache_slot.0].as_ref())
            .is_none(),
        "a replacement mapping requires two content guards and stays uncached"
    );

    interpreter
        .bootstrap_module_registry
        .dict_shift_remove(&PyKey::str_from("typing"))
        .unwrap();
    let resolved_after_internal_deletion = interpreter
        .cached_module_class(cache_slot, "typing", "_GenericAlias")
        .unwrap();
    assert!(Rc::ptr_eq(
        &resolved_after_internal_deletion,
        &replacement_alias_class
    ));
    assert!(
        interpreter
            .module_class_cache
            .as_ref()
            .and_then(|cache| cache.entries[cache_slot.0].as_ref())
            .is_none()
    );
}

#[test]
fn invalid_visible_registry_does_not_break_cached_module_class_internal_hits() {
    let mut interpreter = Interpreter::default();
    let system_module = interpreter.load_module("sys").unwrap();
    interpreter.load_module("typing").unwrap();
    let cache_slot = ModuleClassCacheSlot::new(0);
    let original_alias_class = interpreter
        .cached_module_class(cache_slot, "typing", "_GenericAlias")
        .unwrap();
    assert!(
        interpreter
            .module_class_cache
            .as_ref()
            .and_then(|cache| cache.entries[cache_slot.0].as_ref())
            .is_some()
    );

    interpreter
        .delete_attr(system_module.clone(), "modules")
        .unwrap();
    let resolved_without_visible_registry = interpreter
        .cached_module_class(cache_slot, "typing", "_GenericAlias")
        .unwrap();
    assert!(Rc::ptr_eq(
        &resolved_without_visible_registry,
        &original_alias_class
    ));
    assert!(
        interpreter
            .module_class_cache
            .as_ref()
            .and_then(|cache| cache.entries[cache_slot.0].as_ref())
            .is_none(),
        "a missing visible registry must use the internal module without caching"
    );

    interpreter
        .assign_attr(system_module, "modules", Value::int(1))
        .unwrap();
    let resolved_with_invalid_visible_registry = interpreter
        .cached_module_class(cache_slot, "typing", "_GenericAlias")
        .unwrap();
    assert!(Rc::ptr_eq(
        &resolved_with_invalid_visible_registry,
        &original_alias_class
    ));
    assert!(
        interpreter
            .module_class_cache
            .as_ref()
            .and_then(|cache| cache.entries[cache_slot.0].as_ref())
            .is_none(),
        "an invalid visible registry must use the internal module without caching"
    );
}

#[test]
fn imported_python_module_inherits_parent_recursion_limit() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
    let unique = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let temp_dir = std::env::temp_dir().join(format!(
        "pyrust-recursion-limit-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();
    std::fs::write(
        temp_dir.join("recursion_limit_child.py"),
        "import sys\nobserved_limit = sys.getrecursionlimit()\n",
    )
    .unwrap();

    let script_path = temp_dir.join("main.py");
    let mut parent = Interpreter::with_script_path_and_args(script_path.to_str().unwrap(), &[]);
    set_recursion_limit(&mut parent, 432);
    let module = parent.load_module("recursion_limit_child").unwrap();
    let ValueKind::PyModule(module) = module.kind() else {
        panic!("filesystem import must return a module");
    };
    assert_eq!(
        module.borrow().get_attr_value("observed_limit"),
        Some(Value::int(432))
    );

    std::fs::remove_dir_all(temp_dir).unwrap();
}

#[test]
fn deferred_module_catch_retains_imported_filename() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
    let unique = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let temp_dir = std::env::temp_dir().join(format!(
        "pyrust-deferred-module-catch-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();
    std::fs::write(
        temp_dir.join("deferred_catch_child.py"),
        "try:\n    raise RuntimeError('child')\nexcept RuntimeError as caught:\n    saved = caught\n",
    )
    .unwrap();

    let script_path = temp_dir.join("main.py");
    let mut parent = Interpreter::with_script_path_and_args(script_path.to_str().unwrap(), &[]);
    parent
        .exec_source(
            "import deferred_catch_child as child\n\
             catch_code = child.saved.__traceback__.tb_frame.f_code\n\
             child_filename_retained = catch_code.co_filename.endswith('deferred_catch_child.py')\n\
             child_frame_name = catch_code.co_name\n",
            None,
            None,
        )
        .unwrap();

    assert_eq!(
        parent.lookup_name("child_filename_retained").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        parent.lookup_name("child_frame_name").unwrap(),
        Some(Value::string("<module>"))
    );

    std::fs::remove_dir_all(temp_dir).unwrap();
}

#[test]
fn int_max_str_digits_isolated_between_root_interpreters_and_applied_to_core() {
    // Pin a non-default host TLS value so every Interpreter entry must install
    // its own setting and restore the host value afterward.
    let _host_limit = pyrust_core::scoped_int_max_str_digits(0);

    let mut first = Interpreter::default();
    first
        .exec_source(
            concat!(
                "import sys\n",
                "startup_flag_before = sys.flags.int_max_str_digits\n",
                "digits = '9' * 700\n",
                "sys.set_int_max_str_digits(0)\n",
                "large = int(digits)\n",
                "sys.set_int_max_str_digits(640)\n",
                "try:\n",
                "    str(large)\n",
                "except ValueError:\n",
                "    first_format_limited = True\n",
                "else:\n",
                "    first_format_limited = False\n",
                "try:\n",
                "    int(digits)\n",
                "except ValueError:\n",
                "    first_parse_limited = True\n",
                "else:\n",
                "    first_parse_limited = False\n",
                "startup_flag_after = sys.flags.int_max_str_digits\n",
            ),
            None,
            None,
        )
        .unwrap();
    assert_eq!(pyrust_core::get_int_max_str_digits(), 0);
    assert_eq!(
        first.lookup_name("first_format_limited").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        first.lookup_name("first_parse_limited").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        first.lookup_name("startup_flag_before").unwrap(),
        Some(Value::int(pyrust_core::INT_MAX_STR_DIGITS_DEFAULT as i64))
    );
    assert_eq!(
        first.lookup_name("startup_flag_after").unwrap(),
        Some(Value::int(pyrust_core::INT_MAX_STR_DIGITS_DEFAULT as i64))
    );
    let decimal_literal = "9".repeat(700);
    let literal_error = first
        .exec_source(
            &format!("oversized_literal = {decimal_literal}\n"),
            None,
            None,
        )
        .unwrap_err();
    let PyError::Named(literal_error_class, literal_error_message) = literal_error else {
        panic!("oversized decimal literal must raise SyntaxError");
    };
    assert_eq!(literal_error_class.as_ref(), "SyntaxError");
    assert!(literal_error_message.contains("640 digits"));
    assert!(literal_error_message.contains("value has 700 digits"));
    assert_eq!(pyrust_core::get_int_max_str_digits(), 0);

    // A second root on the same thread starts at the default, not the first
    // root's 640-digit policy, and core parse/format operations see that value.
    let mut second = Interpreter::default();
    second
        .exec_source(
            concat!(
                "import sys\n",
                "second_limit = sys.get_int_max_str_digits()\n",
                "digits = '9' * 700\n",
                "second_roundtrip_digits = len(str(int(digits)))\n",
            ),
            None,
            None,
        )
        .unwrap();
    assert_eq!(pyrust_core::get_int_max_str_digits(), 0);
    assert_eq!(
        second.lookup_name("second_limit").unwrap(),
        Some(Value::int(pyrust_core::INT_MAX_STR_DIGITS_DEFAULT as i64))
    );
    assert_eq!(
        second.lookup_name("second_roundtrip_digits").unwrap(),
        Some(Value::int(700))
    );
    second
        .exec_source(
            &format!("second_literal_digits = len(str({decimal_literal}))\n"),
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        second.lookup_name("second_literal_digits").unwrap(),
        Some(Value::int(700))
    );

    // Re-entering the first root reinstalls its own persisted setting.
    first
        .exec_source(
            concat!(
                "persisted_limit = sys.get_int_max_str_digits()\n",
                "exec('sys.set_int_max_str_digits(0)')\n",
                "nested_exec_limit = sys.get_int_max_str_digits()\n",
                "nested_exec_roundtrip_digits = len(str(int(digits)))\n",
                "sys.set_int_max_str_digits(640)\n",
                "try:\n",
                "    int(digits)\n",
                "except ValueError:\n",
                "    parse_still_limited = True\n",
                "else:\n",
                "    parse_still_limited = False\n",
                "try:\n",
                "    str(large)\n",
                "except ValueError:\n",
                "    format_still_limited = True\n",
                "else:\n",
                "    format_still_limited = False\n",
            ),
            None,
            None,
        )
        .unwrap();
    assert_eq!(pyrust_core::get_int_max_str_digits(), 0);
    assert_eq!(
        first.lookup_name("persisted_limit").unwrap(),
        Some(Value::int(640))
    );
    assert_eq!(
        first.lookup_name("nested_exec_limit").unwrap(),
        Some(Value::int(0))
    );
    assert_eq!(
        first.lookup_name("nested_exec_roundtrip_digits").unwrap(),
        Some(Value::int(700))
    );
    assert_eq!(
        first.lookup_name("parse_still_limited").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        first.lookup_name("format_still_limited").unwrap(),
        Some(Value::bool_(true))
    );
}

#[test]
fn imported_python_module_inherits_and_updates_parent_int_max_str_digits() {
    use std::sync::atomic::{AtomicU64, Ordering};

    let _host_limit = pyrust_core::scoped_int_max_str_digits(0);
    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
    let unique = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let temp_dir = std::env::temp_dir().join(format!(
        "pyrust-int-max-str-digits-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();
    std::fs::write(
        temp_dir.join("int_limit_child.py"),
        concat!(
            "import sys\n",
            "observed_limit = sys.get_int_max_str_digits()\n",
            "digits = '9' * 700\n",
            "try:\n",
            "    int(digits)\n",
            "except ValueError:\n",
            "    inherited_parse_limit = True\n",
            "else:\n",
            "    inherited_parse_limit = False\n",
            "sys.set_int_max_str_digits(0)\n",
            "unlimited_roundtrip_digits = len(str(int(digits)))\n",
        ),
    )
    .unwrap();

    let script_path = temp_dir.join("main.py");
    let mut parent = Interpreter::with_script_path_and_args(script_path.to_str().unwrap(), &[]);
    parent
        .exec_source("import sys\nsys.set_int_max_str_digits(640)\n", None, None)
        .unwrap();
    let module = parent.load_module("int_limit_child").unwrap();
    let ValueKind::PyModule(module) = module.kind() else {
        panic!("filesystem import must return a module");
    };
    assert_eq!(
        module.borrow().get_attr_value("observed_limit"),
        Some(Value::int(640))
    );
    assert_eq!(
        module.borrow().get_attr_value("inherited_parse_limit"),
        Some(Value::bool_(true))
    );
    assert_eq!(
        module.borrow().get_attr_value("unlimited_roundtrip_digits"),
        Some(Value::int(700))
    );

    // The child executes in the same Python interpreter. Its sys.set call
    // therefore remains visible to the parent after import returns.
    parent
        .exec_source(
            concat!(
                "parent_limit_after_import = sys.get_int_max_str_digits()\n",
                "parent_roundtrip_digits = len(str(int('9' * 700)))\n",
            ),
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        parent.lookup_name("parent_limit_after_import").unwrap(),
        Some(Value::int(0))
    );
    assert_eq!(
        parent.lookup_name("parent_roundtrip_digits").unwrap(),
        Some(Value::int(700))
    );
    assert_eq!(pyrust_core::get_int_max_str_digits(), 0);

    std::fs::remove_dir_all(temp_dir).unwrap();
}

#[test]
fn warnings_state_is_isolated_between_root_interpreters() {
    let mut first = Interpreter::default();
    first
        .exec_source(
            "import warnings\n\
             warnings.resetwarnings()\n\
             warnings.simplefilter('error')\n",
            None,
            None,
        )
        .unwrap();

    // A second root on the same host thread must not inherit the first root's
    // error policy.
    let mut second = Interpreter::default();
    second
        .exec_source(
            concat!(
                "import warnings\n",
                "try:\n",
                "    warnings.warn('independent root')\n",
                "    saw_foreign_filter = False\n",
                "except UserWarning:\n",
                "    saw_foreign_filter = True\n",
            ),
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        second.lookup_name("saw_foreign_filter").unwrap(),
        Some(Value::bool_(false))
    );

    // Conversely, resetting the first root must not clear a policy installed
    // by the second root.
    second
        .exec_source(
            "warnings.resetwarnings()\n\
             warnings.simplefilter('error')\n",
            None,
            None,
        )
        .unwrap();
    first
        .exec_source("warnings.resetwarnings()\n", None, None)
        .unwrap();
    second
        .exec_source(
            concat!(
                "try:\n",
                "    warnings.warn('still error')\n",
                "except UserWarning:\n",
                "    own_filter_survived = True\n",
                "else:\n",
                "    own_filter_survived = False\n",
            ),
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        second.lookup_name("own_filter_survived").unwrap(),
        Some(Value::bool_(true))
    );

    // Keep a recording context open in the first root while the second emits.
    // The second warning must not be appended to the first root's log.
    first
        .exec_source(
            "recording = warnings.catch_warnings(record=True)\n\
             first_log = recording.__enter__()\n",
            None,
            None,
        )
        .unwrap();
    second
        .exec_source(
            "warnings.resetwarnings()\n\
             warnings.warn('must not cross roots')\n",
            None,
            None,
        )
        .unwrap();
    first
        .exec_source(
            "foreign_recorded = len(first_log)\n\
             warnings.warn('owned by first')\n\
             own_recorded = len(first_log)\n\
             recording.__exit__(None, None, None)\n",
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        first.lookup_name("foreign_recorded").unwrap(),
        Some(Value::int(0))
    );
    assert_eq!(
        first.lookup_name("own_recorded").unwrap(),
        Some(Value::int(1))
    );
}

#[test]
fn imported_python_module_shares_parent_warnings_state() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
    let unique = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let temp_dir = std::env::temp_dir().join(format!(
        "pyrust-warnings-state-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();
    std::fs::write(
        temp_dir.join("warnings_state_child.py"),
        "import warnings\nwarnings.simplefilter('error')\n",
    )
    .unwrap();

    let script_path = temp_dir.join("main.py");
    let mut parent = Interpreter::with_script_path_and_args(script_path.to_str().unwrap(), &[]);
    parent
        .exec_source("import warnings\nwarnings.resetwarnings()\n", None, None)
        .unwrap();
    parent.load_module("warnings_state_child").unwrap();
    parent
        .exec_source(
            concat!(
                "try:\n",
                "    warnings.warn('child policy')\n",
                "except UserWarning:\n",
                "    child_filter_visible = True\n",
                "else:\n",
                "    child_filter_visible = False\n",
            ),
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        parent.lookup_name("child_filter_visible").unwrap(),
        Some(Value::bool_(true))
    );

    std::fs::remove_dir_all(temp_dir).unwrap();
}

#[test]
fn collections_captures_chain_from_its_own_root_interpreter() {
    let mut first = Interpreter::default();
    first
        .exec_source(
            "import itertools\nfirst_chain = itertools.chain\n",
            None,
            None,
        )
        .unwrap();

    // Importing itertools in another root updates process-local helper state,
    // but must not influence modules subsequently loaded from `first`'s
    // independent cache.
    let mut second = Interpreter::default();
    second
        .exec_source("import itertools\n", None, None)
        .unwrap();

    first
        .exec_source(
            "import collections\n\
             elements = collections.Counter(a=1).elements()\n\
             same_chain = type(elements) is first_chain\n",
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        first.lookup_name("same_chain").unwrap(),
        Some(Value::bool_(true))
    );
}

#[test]
fn functools_factories_are_owned_by_their_root_and_import_generation() {
    let mut first = Interpreter::default();
    first
        .exec_source(
            r#"import functools

first_lru_cache = functools.lru_cache
first_factory = functools.lru_cache(maxsize=1)
first_wraps = functools.wraps

def compare(left, right):
    return left - right

first_key_factory = functools.cmp_to_key(compare)

def first_update_wrapper(wrapper, wrapped):
    wrapper.factory_generation = "first"
    return wrapper

functools.update_wrapper = first_update_wrapper

def identity(value):
    return value

first_dispatch = functools.singledispatch(identity)
first_dispatch_owned = first_dispatch.factory_generation == "first"
"#,
            None,
            None,
        )
        .unwrap();

    let mut second = Interpreter::default();
    second
        .exec_source(
            r#"import functools

def second_update_wrapper(wrapper, wrapped):
    wrapper.factory_generation = "second"
    return wrapper

functools.update_wrapper = second_update_wrapper

def identity(value):
    return value

second_dispatch = functools.singledispatch(identity)
second_dispatch_owned = second_dispatch.factory_generation == "second"
second_wrapper = functools.lru_cache(identity)
second_info_type = type(second_wrapper.cache_info())
"#,
            None,
            None,
        )
        .unwrap();

    // Exercise retained first-root callables only after the second root has
    // imported its own fresh classes. A thread-global "newest generation"
    // registry would construct every one of these from `second`.
    first
        .exec_source(
            r#"first_wrapper = first_lru_cache(identity)
first_factory_wrapper = first_factory(identity)
first_wraps_instance = first_wraps(identity)
first_key = first_key_factory(1)
first_info_type = type(first_wrapper.cache_info())
first_classes_owned = (
    type(first_wrapper) is functools._lru_cache_wrapper
    and type(first_factory_wrapper) is functools._lru_cache_wrapper
    and type(first_wraps_instance) is functools._wraps_partial
    and type(first_key) is functools._cmp_key
)
"#,
            None,
            None,
        )
        .unwrap();

    assert_eq!(
        first.lookup_name("first_dispatch_owned").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        second.lookup_name("second_dispatch_owned").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        first.lookup_name("first_classes_owned").unwrap(),
        Some(Value::bool_(true))
    );
    let first_info_type = first
        .lookup_name("first_info_type")
        .unwrap()
        .expect("first CacheInfo class");
    let second_info_type = second
        .lookup_name("second_info_type")
        .unwrap()
        .expect("second CacheInfo class");
    let (ValueKind::PyClass(first_info_type), ValueKind::PyClass(second_info_type)) =
        (first_info_type.kind(), second_info_type.kind())
    else {
        panic!("CacheInfo types must be Python classes");
    };
    assert!(!Rc::ptr_eq(first_info_type, second_info_type));
}

#[test]
fn typing_alias_factories_are_owned_by_the_active_root() {
    let mut first = Interpreter::default();
    first
        .exec_source(
            "import typing\nfirst_alias_class = typing._GenericAlias\n",
            None,
            None,
        )
        .unwrap();

    let mut second = Interpreter::default();
    second
        .exec_source(
            "import typing\n\
             second_alias_class = typing._GenericAlias\n\
             second_alias_owned = type(typing.Generic[int]) is second_alias_class\n",
            None,
            None,
        )
        .unwrap();

    // Re-enter the first root only after the second root has installed its
    // generation. Alias construction must consult the active root's module,
    // not a thread-global "latest typing generation".
    first
        .exec_source(
            "first_alias_owned = type(typing.Generic[int]) is first_alias_class\n",
            None,
            None,
        )
        .unwrap();

    assert_eq!(
        first.lookup_name("first_alias_owned").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        second.lookup_name("second_alias_owned").unwrap(),
        Some(Value::bool_(true))
    );
}

#[test]
fn collections_reimports_reuse_static_class_getitem_dispatch_names() {
    let interp = run_program(
        r#"import collections
import sys
old_counter = collections.Counter
old_alias_ok = old_counter[int].__origin__ is old_counter
reload_ok = True
for _ in range(8):
    del sys.modules["collections"]
    import collections
    reload_ok = reload_ok and collections.Counter[int].__origin__ is collections.Counter
    reload_ok = reload_ok and collections.UserList[int].__origin__ is collections.UserList
old_alias_still_ok = old_counter[str].__origin__ is old_counter
"#,
    );
    assert_eq!(
        interp.lookup_name("old_alias_ok").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        interp.lookup_name("reload_ok").unwrap(),
        Some(Value::bool_(true))
    );
    assert_eq!(
        interp.lookup_name("old_alias_still_ok").unwrap(),
        Some(Value::bool_(true))
    );
}
