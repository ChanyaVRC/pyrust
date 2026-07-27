// Module finalization and generation-local `groupby` / `_grouper` identities.

struct GroupByClassGeneration {
    groupby: Weak<RefCell<PyClass>>,
    // `groupby` is the weak lifetime key. A retained parent keeps its private
    // sibling available without leaking the generation after the parent dies.
    grouper: Rc<RefCell<PyClass>>,
}

thread_local! {
    /// Every still-live imported `groupby` → `_grouper` class generation.
    ///
    /// Weak storage allows `del sys.modules["itertools"]` followed by a fresh
    /// import without leaking the old module. The sibling is retained only
    /// while its owning `groupby` class (or an instance/subclass) remains live.
    static GROUPBY_CLASS_GENERATIONS: RefCell<Vec<GroupByClassGeneration>> =
        const { RefCell::new(Vec::new()) };
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
}

/// Finalize one imported itertools generation and register its private class
/// identities for internal factories.
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
        register_groupby_generation(groupby, grouper);
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
