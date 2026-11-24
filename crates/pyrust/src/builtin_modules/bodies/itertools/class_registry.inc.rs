// Module finalization and generation-local `groupby` / `_grouper` identities.

struct GroupByClassGeneration {
    groupby: Weak<RefCell<PyClass>>,
    // `groupby` is the weak lifetime key. A retained parent keeps its private
    // sibling available without leaking the generation after the parent dies.
    grouper: Rc<RefCell<PyClass>>,
}

const ITERTOOLS_ERROR_NAMES: [(&str, &str); 19] = [
    ("count", "itertools.count"),
    ("cycle", "itertools.cycle"),
    ("repeat", "itertools.repeat"),
    ("chain", "itertools.chain"),
    ("islice", "itertools.islice"),
    ("zip_longest", "itertools.zip_longest"),
    ("product", "itertools.product"),
    ("accumulate", "itertools.accumulate"),
    ("groupby", "itertools.groupby"),
    ("permutations", "itertools.permutations"),
    ("combinations", "itertools.combinations"),
    (
        "combinations_with_replacement",
        "itertools.combinations_with_replacement",
    ),
    ("starmap", "itertools.starmap"),
    ("takewhile", "itertools.takewhile"),
    ("dropwhile", "itertools.dropwhile"),
    ("filterfalse", "itertools.filterfalse"),
    ("compress", "itertools.compress"),
    ("pairwise", "itertools.pairwise"),
    ("batched", "itertools.batched"),
];

// Thread-local justification:
// (a) Why `thread_local!`: Tracks `!Send` `Rc`/`Weak<RefCell<PyClass>>`
//     generation pairs (`groupby` -> `_grouper`) and weak unpicklable class
//     identities (`pairwise`, `batched`).
// (b) Multi-interpreter / multi-threaded impact: Weak generation tracking
//     isolates distinct `itertools` imports across interpreters on one thread
//     and avoids leaking old modules; separate threads are isolated.
thread_local! {
    /// Every still-live imported `groupby` → `_grouper` class generation.
    ///
    /// Weak storage allows `del sys.modules["itertools"]` followed by a fresh
    /// import without leaking the old module. The sibling is retained only
    /// while its owning `groupby` class (or an instance/subclass) remains live.
    static GROUPBY_CLASS_GENERATIONS: RefCell<Vec<GroupByClassGeneration>> =
        const { RefCell::new(Vec::new()) };

    /// Weak registry of canonical `itertools.pairwise` and `itertools.batched`
    /// classes across module import generations. Neither supports pickling or
    /// copying in CPython 3.12 (issue #2967).
    static UNPICKLABLE_ITERTOOLS_CLASSES: RefCell<Vec<Weak<RefCell<PyClass>>>> =
        const { RefCell::new(Vec::new()) };
}

fn register_unpicklable_itertools_class(class: &Rc<RefCell<PyClass>>) {
    UNPICKLABLE_ITERTOOLS_CLASSES.with(|classes| {
        let mut classes = classes.borrow_mut();
        classes.retain(|registered| registered.strong_count() > 0);
        if !classes
            .iter()
            .any(|registered| registered.as_ptr() == Rc::as_ptr(class))
        {
            classes.push(Rc::downgrade(class));
        }
    });
}

/// Whether `class` is `itertools.pairwise`, `itertools.batched`, or a subclass
/// thereof (all of which refuse `copy.copy` / `copy.deepcopy` in CPython 3.12).
pub(crate) fn is_unpicklable_itertools_class(class: &Rc<RefCell<PyClass>>) -> bool {
    UNPICKLABLE_ITERTOOLS_CLASSES.with(|classes| {
        let mut classes = classes.borrow_mut();
        classes.retain(|registered| registered.strong_count() > 0);
        classes
            .iter()
            .filter_map(Weak::upgrade)
            .any(|registered| crate::interpreter::class_is_subclass_of(class, &registered))
    })
}

fn is_groupby_class(class: &Rc<RefCell<PyClass>>) -> bool {
    GROUPBY_CLASS_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        generations.retain(|generation| generation.groupby.strong_count() > 0);
        generations.iter().any(|generation| {
            generation
                .groupby
                .upgrade()
                .is_some_and(|gb| crate::interpreter::class_is_subclass_of(class, &gb))
        })
    })
}

fn is_grouper_class(class: &Rc<RefCell<PyClass>>) -> bool {
    GROUPBY_CLASS_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        generations.retain(|generation| generation.groupby.strong_count() > 0);
        generations
            .iter()
            .any(|generation| crate::interpreter::class_is_subclass_of(class, &generation.grouper))
    })
}

/// Copy or deep-copy an `itertools.groupby` or `itertools._grouper` instance
/// using CPython 3.12's exact `__reduce__` semantics (issue #2967):
///
/// - `groupby` omits its `(currvalue, currkey, tgtkey)` state whenever
///   `_has_curr` is `false` (fresh or after `_grouper.__next__` consumed the
///   lookahead), so the copy starts with `_has_tgt = false` and yields any
///   remaining elements of the current key as a new group.
/// - `_grouper` reduces to `iter(())` when stale (`_id != parent._id`), and
///   when live reconstructs `(_grouper, (parent, tgtkey))`, bumping
///   `new_parent._id` so on shallow copy the original grouper becomes stale
///   and the copy becomes the sole live grouper, while on deep copy the copied
///   parent gets its own fresh id.
pub(crate) fn copy_groupby_or_grouper(
    rc: &Rc<RefCell<PyInstance>>,
    interp: &mut crate::interpreter::Interpreter,
    copy_child: &mut dyn FnMut(Value, &mut crate::interpreter::Interpreter) -> Result<Value>,
    memo_insert: &mut dyn FnMut(i64, Value),
    outer_obj_id: Option<i64>,
) -> Option<Result<Value>> {
    let class = Rc::clone(&rc.borrow().class);
    if is_groupby_class(&class) {
        return Some(copy_groupby_instance(
            rc,
            class,
            interp,
            copy_child,
            memo_insert,
            outer_obj_id,
        ));
    }
    if is_grouper_class(&class) {
        return Some(copy_grouper_instance(
            rc,
            class,
            interp,
            copy_child,
            memo_insert,
            outer_obj_id,
        ));
    }
    None
}

fn copy_groupby_instance(
    rc: &Rc<RefCell<PyInstance>>,
    class: Rc<RefCell<PyClass>>,
    interp: &mut crate::interpreter::Interpreter,
    copy_child: &mut dyn FnMut(Value, &mut crate::interpreter::Interpreter) -> Result<Value>,
    memo_insert: &mut dyn FnMut(i64, Value),
    outer_obj_id: Option<i64>,
) -> Result<Value> {
    const INTERNAL_ATTRS: [&str; 9] = [
        "_iter",
        "_keyfn",
        "_currkey",
        "_currvalue",
        "_has_curr",
        "_tgtkey",
        "_has_tgt",
        "_id",
        "_exhausted",
    ];
    let (iter_val, keyfn_val, has_curr, currvalue, currkey, has_tgt, tgtkey, extras) = {
        let borrow = rc.borrow();
        let iter_val = borrow
            .attrs
            .get("_iter")
            .cloned()
            .ok_or_else(|| internal("groupby"))?;
        let keyfn_val = borrow
            .attrs
            .get("_keyfn")
            .cloned()
            .unwrap_or_else(Value::none);
        let has_curr = matches!(
            borrow.attrs.get("_has_curr").map(|v| v.kind()),
            Some(ValueKind::Bool(true))
        );
        let currvalue = borrow
            .attrs
            .get("_currvalue")
            .cloned()
            .unwrap_or_else(Value::none);
        let currkey = borrow
            .attrs
            .get("_currkey")
            .cloned()
            .unwrap_or_else(Value::none);
        let has_tgt = matches!(
            borrow.attrs.get("_has_tgt").map(|v| v.kind()),
            Some(ValueKind::Bool(true))
        );
        let tgtkey = borrow
            .attrs
            .get("_tgtkey")
            .cloned()
            .unwrap_or_else(Value::none);
        let extras: Vec<(String, Value)> = borrow
            .attrs
            .items_snapshot()
            .into_iter()
            .filter(|(name, _)| !INTERNAL_ATTRS.contains(&name.as_ref()))
            .map(|(name, value)| (name.to_string(), value))
            .collect();
        (
            iter_val, keyfn_val, has_curr, currvalue, currkey, has_tgt, tgtkey, extras,
        )
    };
    let new_rc = Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::new(),
    }));
    let new_val = Value::py_instance(Rc::clone(&new_rc));
    if let Some(id) = outer_obj_id {
        memo_insert(id, new_val.clone());
    }
    let copied_iter = copy_child(iter_val, interp)?;
    let copied_keyfn = copy_child(keyfn_val, interp)?;
    let (copied_currval, copied_currkey, copied_tgtkey, restored_has_tgt) = if has_curr {
        (
            copy_child(currvalue, interp)?,
            copy_child(currkey, interp)?,
            copy_child(tgtkey, interp)?,
            has_tgt,
        )
    } else {
        (Value::none(), Value::none(), Value::none(), false)
    };
    {
        let mut attrs = new_rc.borrow_mut();
        attrs.attrs.insert("_iter", copied_iter);
        attrs.attrs.insert("_keyfn", copied_keyfn);
        attrs.attrs.insert("_currvalue", copied_currval);
        attrs.attrs.insert("_currkey", copied_currkey);
        attrs.attrs.insert("_has_curr", Value::bool_(has_curr));
        attrs.attrs.insert("_tgtkey", copied_tgtkey);
        attrs
            .attrs
            .insert("_has_tgt", Value::bool_(restored_has_tgt));
        attrs.attrs.insert("_id", Value::int(0));
        attrs.attrs.insert("_exhausted", Value::bool_(false));
    }
    for (name, val) in extras {
        let copied = copy_child(val, interp)?;
        new_rc.borrow_mut().attrs.insert(&name, copied);
    }
    Ok(new_val)
}

fn copy_grouper_instance(
    rc: &Rc<RefCell<PyInstance>>,
    class: Rc<RefCell<PyClass>>,
    interp: &mut crate::interpreter::Interpreter,
    copy_child: &mut dyn FnMut(Value, &mut crate::interpreter::Interpreter) -> Result<Value>,
    memo_insert: &mut dyn FnMut(i64, Value),
    outer_obj_id: Option<i64>,
) -> Result<Value> {
    let (parent_val, my_tgtkey, my_id) = {
        let borrow = rc.borrow();
        (
            borrow
                .attrs
                .get("_parent")
                .cloned()
                .ok_or_else(|| internal("_grouper"))?,
            borrow
                .attrs
                .get("_tgtkey")
                .cloned()
                .ok_or_else(|| internal("_grouper"))?,
            match borrow.attrs.get("_id").map(|v| v.kind()) {
                Some(ValueKind::Int(n)) => n,
                _ => return Err(internal("_grouper")),
            },
        )
    };
    let is_live = match parent_val.kind() {
        ValueKind::PyInstance(parent_rc) => matches!(
            parent_rc.borrow().attrs.get("_id").map(|v| v.kind()),
            Some(ValueKind::Int(pid)) if pid == my_id
        ),
        _ => false,
    };
    if !is_live {
        let empty = crate::interpreter::make_iterator(interp, &Value::tuple(Vec::new()))?;
        if let Some(id) = outer_obj_id {
            memo_insert(id, empty.clone());
        }
        return Ok(empty);
    }
    let copied_parent = copy_child(parent_val, interp)?;
    let copied_tgtkey = copy_child(my_tgtkey, interp)?;
    let new_id = match copied_parent.kind() {
        ValueKind::PyInstance(parent_rc) => {
            let mut borrow = parent_rc.borrow_mut();
            let cur = match borrow.attrs.get("_id").map(|v| v.kind()) {
                Some(ValueKind::Int(n)) => n,
                _ => 0,
            };
            let next = cur + 1;
            borrow.attrs.insert("_id", Value::int(next));
            next
        }
        _ => return Err(internal("_grouper")),
    };
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_parent", copied_parent);
    attrs.insert("_tgtkey", copied_tgtkey);
    attrs.insert("_id", Value::int(new_id));
    let rebuilt = Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })));
    if let Some(id) = outer_obj_id {
        memo_insert(id, rebuilt.clone());
    }
    Ok(rebuilt)
}

fn register_groupby_generation(groupby: &Rc<RefCell<PyClass>>, grouper: &Rc<RefCell<PyClass>>) {
    GROUPBY_CLASS_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        generations.retain(|generation| generation.groupby.strong_count() > 0);
        if !generations
            .iter()
            .any(|generation| generation.groupby.as_ptr() == Rc::as_ptr(groupby))
        {
            generations.push(GroupByClassGeneration {
                groupby: Rc::downgrade(groupby),
                grouper: Rc::clone(grouper),
            });
        }
    });
}

fn registered_grouper_class() -> Option<Rc<RefCell<PyClass>>> {
    GROUPBY_CLASS_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        generations.retain(|generation| generation.groupby.strong_count() > 0);
        generations
            .iter()
            .rev()
            .map(|generation| Rc::clone(&generation.grouper))
            .next()
    })
}

fn grouper_class_for_groupby(groupby: &Rc<RefCell<PyClass>>) -> Option<Rc<RefCell<PyClass>>> {
    GROUPBY_CLASS_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        generations.retain(|generation| generation.groupby.strong_count() > 0);
        generations.iter().rev().find_map(|generation| {
            let registered_groupby = generation.groupby.upgrade()?;
            crate::interpreter::class_is_subclass_of(groupby, &registered_groupby)
                .then(|| Rc::clone(&generation.grouper))
        })
    })
}

/// Set `__module__ = "itertools"` on every class exposed by this module so
/// that `type(x).__module__` and the generic instance repr
/// (`<itertools.X object at 0x..>`) match CPython, instead of defaulting to
/// `__main__` (issue #2098). The macro-generated `module()` builds bare
/// `PyClass`es with no `__module__` entry; patch them at the finalization
/// boundary shared by all module consumers.
pub(crate) fn patch_class_modules(module_value: &Value) {
    let ValueKind::PyModule(module) = module_value.kind() else {
        return;
    };
    for value in module.borrow().attrs.values() {
        if let ValueKind::PyClass(class) = value.kind() {
            class
                .borrow_mut()
                .attrs
                .insert("__module__".to_string(), Value::string("itertools"));
        }
    }
    for (export_name, error_name) in ITERTOOLS_ERROR_NAMES {
        let value = module.borrow().attrs.get(export_name).cloned();
        if let Some(value) = value
            && let ValueKind::PyClass(class) = value.kind()
        {
            let mut class_mut = class.borrow_mut();
            class_mut.error_name = Some(error_name);
            // CPython's combinatoric iterators are static C types with no
            // instance `__dict__`; subclasses without `__slots__` still gain
            // an instance `__dict__` automatically (#2968).
            if matches!(
                export_name,
                "combinations" | "combinations_with_replacement" | "permutations" | "product"
            ) {
                class_mut.slots = Some(indexmap::IndexSet::new());
            }
        }
    }
}

/// Finalize one imported itertools generation and register its private class
/// identities and diagnostic name for internal factories.
pub(crate) fn prepare_module_classes(module_value: &Value) {
    patch_class_modules(module_value);
    let ValueKind::PyModule(module) = module_value.kind() else {
        return;
    };
    let groupby = module.borrow().attrs.get("groupby").cloned();
    let grouper = module.borrow().attrs.get("_grouper").cloned();
    if let (Some(groupby), Some(grouper)) = (groupby, grouper)
        && let (ValueKind::PyClass(groupby), ValueKind::PyClass(grouper)) =
            (groupby.kind(), grouper.kind())
    {
        groupby.borrow_mut().slots = Some(indexmap::IndexSet::new());
        grouper.borrow_mut().error_name = Some("itertools._grouper");
        grouper.borrow_mut().slots = Some(indexmap::IndexSet::new());
        register_groupby_generation(groupby, grouper);
    }
    for name in ["pairwise", "batched"] {
        let cls_val = module.borrow().attrs.get(name).cloned();
        if let Some(cls_val) = cls_val
            && let ValueKind::PyClass(cls) = cls_val.kind()
        {
            register_unpicklable_itertools_class(cls);
        }
    }
}

/// Build an itertools module with class qualifiers and private identities
/// prepared. Stored builtin callables use this only as a fallback after their
/// original imported generation has died.
pub(crate) fn module_with_qualifiers() -> Value {
    let module_value = module();
    prepare_module_classes(&module_value);
    module_value
}

/// Mint the private `_grouper` sibling belonging to a concrete `groupby`
/// receiver generation, without running `__init__`.
fn make_grouper_instance(
    groupby_class: &Rc<RefCell<PyClass>>,
    attrs: InstanceAttrs,
) -> Result<Value> {
    let class = if let Some(class) = grouper_class_for_groupby(groupby_class) {
        class
    } else {
        // Direct internal construction can precede module finalization. Build
        // one fallback generation only when the receiver has no association.
        let module_value = module_with_qualifiers();
        let class = grouper_class_for_groupby(groupby_class)
            .or_else(registered_grouper_class)
            .ok_or_else(|| {
                PyError::Runtime("internal: itertools class _grouper missing".to_string())
            })?;
        drop(module_value);
        class
    };
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs,
    }))))
}

#[cfg(test)]
mod groupby_generation_tests {
    use super::*;

    fn test_class(name: &str, base: Option<Rc<RefCell<PyClass>>>) -> Rc<RefCell<PyClass>> {
        Rc::new(RefCell::new(PyClass {
            name: name.to_string(),
            qualname: name.to_string(),
            base,
            ..PyClass::default()
        }))
    }

    #[test]
    fn retained_groupby_resolves_grouper_from_its_own_generation() {
        let old_groupby = test_class("old groupby", None);
        let old_grouper = test_class("old grouper", None);
        register_groupby_generation(&old_groupby, &old_grouper);

        let new_groupby = test_class("new groupby", None);
        let new_grouper = test_class("new grouper", None);
        register_groupby_generation(&new_groupby, &new_grouper);

        let old_groupby_subclass =
            test_class("old groupby subclass", Some(Rc::clone(&old_groupby)));
        assert!(Rc::ptr_eq(
            &grouper_class_for_groupby(&old_groupby).unwrap(),
            &old_grouper,
        ));
        assert!(Rc::ptr_eq(
            &grouper_class_for_groupby(&old_groupby_subclass).unwrap(),
            &old_grouper,
        ));
        assert!(Rc::ptr_eq(
            &grouper_class_for_groupby(&new_groupby).unwrap(),
            &new_grouper,
        ));
    }
}
