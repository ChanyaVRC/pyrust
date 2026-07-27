// Cached construction planning belongs to class invocation, not VM execution.

/// Resolved per-class construction facts that `instantiate_normal_instance`
/// re-derives on *every* `Cls(...)` call: the MRO-resolved `__new__` and
/// `__init__` values, plus which primitive built-in (if any) supplies the
/// storage layout.  All three are read by walking the same base chain, so a
/// single traversal yields them together.
struct ConstructionPlan {
    /// The `__new__` resolved via the MRO (identical to
    /// `lookup_class_attr(class, "__new__")`), or `None` if none found.
    new_val: Option<Value>,
    /// The `__init__` resolved via the MRO (identical to
    /// `lookup_class_attr(class, "__init__")`), or `None`.
    init_val: Option<Value>,
    /// Primitive storage-layout base classification.
    prim: PrimitiveLayout,
}

/// Resolve `__new__`, `__init__`, and the primitive-base classification for
/// `class` in a *single* linear base-chain walk.  Returns `None` (so the caller
/// falls back to the byte-identical per-attr `lookup_class_attr` path) whenever
/// any node in the chain participates in *multiple* inheritance — attribute
/// resolution there must follow the C3 MRO, which a plain depth-first chain walk
/// does not reproduce.  For the single-inheritance case (the common path, and
/// the one whose per-construction cost scales with MRO depth) this folds the
/// three separate `lookup_class_attr` / primitive-layout traversals
/// into one.
///
/// The walk mirrors `lookup_class_attr` exactly: a name resolves to the first
/// class in the chain whose *own* `attrs` defines it, and a chain that
/// terminates without an explicit base falls through to the `object` singleton
/// (the same `!has_explicit_base && !is_primitive_class` fallback).
#[inline]
fn resolve_construction_plan(class: &Rc<RefCell<PyClass>>) -> Option<ConstructionPlan> {
    let mut new_val: Option<Value> = None;
    let mut init_val: Option<Value> = None;
    let mut prim = PrimitiveLayout::None;
    let mut cur = Rc::clone(class);
    loop {
        let is_prim = is_primitive_class(&cur);
        let current_layout = primitive_layout_for_class(&cur);
        let borrowed = cur.borrow();
        // Multiple inheritance anywhere in the chain → bail to the C3 slow path.
        if !borrowed.extra_bases.is_empty() {
            return None;
        }
        if new_val.is_none() {
            new_val = borrowed.attrs.get("__new__").cloned();
        }
        if init_val.is_none() {
            init_val = borrowed.attrs.get("__init__").cloned();
        }
        if prim == PrimitiveLayout::None && current_layout != PrimitiveLayout::None {
            prim = current_layout;
        }
        let has_explicit_base = borrowed.base.is_some();
        let next = borrowed.base.clone();
        drop(borrowed);
        match next {
            Some(b) => cur = b,
            None => {
                // Chain terminated.  Mirror `lookup_class_attr`'s implicit
                // `object` fallback: a class with no explicit base (and that is
                // not itself a primitive singleton) inherits `object`'s attrs.
                if !has_explicit_base && !is_prim {
                    let obj = object_class_singleton();
                    if !Rc::ptr_eq(&cur, &obj) {
                        let ob = obj.borrow();
                        if new_val.is_none() {
                            new_val = ob.attrs.get("__new__").cloned();
                        }
                        if init_val.is_none() {
                            init_val = ob.attrs.get("__init__").cloned();
                        }
                    }
                }
                break;
            }
        }
    }
    Some(ConstructionPlan {
        new_val,
        init_val,
        prim,
    })
}

/// Resolve the construction plan for `class`, reusing the per-class cache when it
/// is still valid (issue #2330).  The cache is validated exactly like the
/// attribute inline caches: a hit requires both the class's own
/// `mutation_version` and the global `class_epoch()` to match the values stamped
/// when the plan was last resolved.  Either changing (a direct monkeypatch of
/// this class, or *any* class mutation that bumps the global epoch — e.g. a base
/// class being patched) forces a fresh `resolve_construction_plan` walk.
///
/// Multiply-inherited classes (`resolve_construction_plan` → `None`) are never
/// cached; the caller keeps the byte-identical per-attr C3 fallback for them.
#[inline]
fn resolve_construction_plan_cached(class: &Rc<RefCell<PyClass>>) -> Option<ConstructionPlan> {
    let cache_stamp = pyrust_core::class_cache_stamp(class.borrow().mutation_version.get());
    // Fast path: a still-valid cached plan reproduces the resolved values with
    // no base-chain walk (cheap `Value` clones + a Copy `PrimitiveLayout`).
    if let Some((class_version, epoch)) = cache_stamp
        && let Some(cached) = class.borrow().construction_cache.borrow().as_deref()
        && cached.class_version == class_version
        && cached.epoch == epoch
    {
        return Some(ConstructionPlan {
            new_val: cached.new_val.clone(),
            init_val: cached.init_val.clone(),
            prim: PrimitiveLayout::from_primitive_kind(cached.primitive_tag),
        });
    }
    // Miss (or stale): re-resolve and refresh the cache.  Only single-inheritance
    // classes (the ones `resolve_construction_plan` resolves) are cacheable.
    let plan = resolve_construction_plan(class)?;
    if let Some((class_version, epoch)) = cache_stamp {
        *class.borrow().construction_cache.borrow_mut() =
            Some(Box::new(pyrust_core::CachedConstructionPlan {
                new_val: plan.new_val.clone(),
                init_val: plan.init_val.clone(),
                primitive_tag: plan.prim.primitive_kind(),
                class_version,
                epoch,
            }));
    } else {
        // A saturated version can never become cacheable again. Drop any entry
        // filled before saturation so it cannot retain constructor Values.
        class.borrow().construction_cache.borrow_mut().take();
    }
    Some(plan)
}

#[cfg(test)]
mod construction_identity_tests {
    use super::{
        BUILTIN_DATA_ATTR, Interpreter, PrimitiveLayout, PyClass, Rc, RefCell, ValueKind,
        object_class_singleton, resolve_construction_plan_cached,
    };

    #[test]
    fn renamed_primitive_base_constructs_from_typed_cached_identity() {
        let builtin = crate::interpreter::primitive_class_by_name("list").unwrap();
        let original_name =
            std::mem::replace(&mut builtin.borrow_mut().name, "renamed-list".into());
        let subclass = Rc::new(RefCell::new(PyClass::new(
            "ListChild",
            "ListChild",
            Some(Rc::clone(&builtin)),
            indexmap::IndexMap::new(),
        )));

        let plan = resolve_construction_plan_cached(&subclass).unwrap();
        let cached_tag = subclass
            .borrow()
            .construction_cache
            .borrow()
            .as_ref()
            .unwrap()
            .primitive_tag;
        let cached_plan = resolve_construction_plan_cached(&subclass).unwrap();
        let mut interp = Interpreter::default();
        let constructed = interp.call_class_expanded(Rc::clone(&subclass), &[]);

        // Restore mutable presentation metadata before asserting so a failure
        // cannot leak the renamed singleton into later tests on this thread.
        builtin.borrow_mut().name = original_name;

        assert!(matches!(
            plan.prim,
            PrimitiveLayout::Mutable(pyrust_core::CanonicalClassTag::List)
        ));
        assert_eq!(
            cached_tag,
            Some(pyrust_core::CanonicalClassTag::List),
            "construction cache must retain typed identity"
        );
        assert!(matches!(
            cached_plan.prim,
            PrimitiveLayout::Mutable(pyrust_core::CanonicalClassTag::List)
        ));

        let value = constructed.unwrap();
        let ValueKind::PyInstance(instance) = value.kind() else {
            panic!("primitive subclass construction must return an instance");
        };
        let backing = instance
            .borrow()
            .attrs
            .get(BUILTIN_DATA_ATTR)
            .cloned()
            .expect("primitive subclass must have backing storage");
        assert!(matches!(backing.kind(), ValueKind::List(items) if items.is_empty()));
    }

    #[test]
    fn same_named_user_class_has_no_primitive_construction_identity() {
        let spoof = Rc::new(RefCell::new(PyClass::new(
            "list",
            "list",
            Some(object_class_singleton()),
            indexmap::IndexMap::new(),
        )));

        let plan = resolve_construction_plan_cached(&spoof).unwrap();
        assert!(matches!(plan.prim, PrimitiveLayout::None));
        assert_eq!(
            spoof
                .borrow()
                .construction_cache
                .borrow()
                .as_ref()
                .unwrap()
                .primitive_tag,
            None
        );

        let mut interp = Interpreter::default();
        let value = interp.call_class_expanded(spoof, &[]).unwrap();
        let ValueKind::PyInstance(instance) = value.kind() else {
            panic!("user class construction must return an instance");
        };
        assert!(!instance.borrow().attrs.contains_key(BUILTIN_DATA_ATTR));
    }

    #[test]
    fn saturated_class_version_disables_construction_cache_fill() {
        let class = Rc::new(RefCell::new(PyClass::new(
            "Saturated",
            "Saturated",
            Some(object_class_singleton()),
            indexmap::IndexMap::new(),
        )));
        class.borrow().mutation_version.set(u64::MAX);

        let plan = resolve_construction_plan_cached(&class);

        assert!(plan.is_some());
        assert!(
            class.borrow().construction_cache.borrow().is_none(),
            "a saturated class version must never populate an equality cache"
        );
    }
}
