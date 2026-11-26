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
        // A class with a finalizer is never cached either: its constructions
        // are counted on the uncached path (issue #3341), which keeps that
        // bookkeeping entirely off the cached fast path.
        if borrowed.attrs.contains_key("__del__") {
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

/// Validated construction plan for a *plain* class, cached at one
/// `Insn::Call` site (issue #3383).
///
/// `Cls(...)` normally re-derives, on every call, that the class is not a
/// typing marker or interpreter-owned adapter class, that its metaclass does
/// not override `__call__`, that it is neither abstract nor an exception, and
/// how `__new__` / `__init__` resolve.  A plan records that all of those
/// answers were the default ones:
///
/// - no metaclass `__call__` override (#1956 / #2939 / #2944 / #2947),
/// - no non-empty `__abstractmethods__` (#3293),
/// - not an exception class, not a `type` subclass, no primitive storage base,
/// - `__new__` resolves to `object.__new__` and its slot was never wrapped
///   (#1143 / #2335),
/// - `__init__` resolves to `object.__init__` or to an ordinary Python
///   function.
///
/// Every one of those facts is a function of class attributes along the class
/// and metaclass MROs, and every class-attribute write or delete bumps both
/// the mutated class's `mutation_version` and the global `class_epoch()`.  The
/// plan is therefore valid exactly while its `(class version, epoch)` stamp
/// still matches — the same contract as the per-class construction cache and
/// the attribute inline caches.
///
/// The class and the `__init__` function are held weakly so a call site never
/// extends the lifetime of either: a current stamp proves the function is
/// still stored on a class in the (live) callee's MRO.
#[derive(Debug)]
pub(crate) struct PlainClassPlan {
    /// Weak allocation identity of the planned class (no ABA, no ownership).
    class: std::rc::Weak<RefCell<PyClass>>,
    /// `(PyClass::mutation_version, class_epoch())` the facts were read under.
    /// A `Cell` so a re-resolution that reaches the same facts (an unrelated
    /// class was mutated) refreshes the stamp without reallocating the plan.
    stamp: std::cell::Cell<(u64, u64)>,
    /// `None` when `__init__` resolves to `object.__init__`: construction is
    /// allocation only and any argument is an error.
    init: Option<PlainClassInit>,
    /// Some class in the MRO defines `__del__`, so each construction is
    /// offered to the finalizer bookkeeping (#3341 / #3353).
    has_finalizer: bool,
}

/// The resolved ordinary-function `__init__` of a [`PlainClassPlan`].
#[derive(Debug)]
struct PlainClassInit {
    function: std::rc::Weak<UserFunction>,
    /// Parameter count (including `self`) when every parameter is a plain
    /// positional-or-keyword one, so an all-positional call binds each
    /// argument (and any trailing default) straight into its register.
    /// `usize::MAX` when the signature has `*args`, `**kwargs`, or
    /// keyword-only parameters and always needs the general binder.
    positional_parameter_count: usize,
}

impl PlainClassPlan {
    /// Does this plan describe exactly `class` (allocation identity)?
    #[inline]
    pub(crate) fn is_for_class(&self, class: &Rc<RefCell<PyClass>>) -> bool {
        std::ptr::eq(self.class.as_ptr(), Rc::as_ptr(class))
    }

    /// Is the stamp the facts were read under still current for `class`?
    #[inline]
    pub(crate) fn is_current(&self, class: &Rc<RefCell<PyClass>>) -> bool {
        let (class_version, epoch) = self.stamp.get();
        pyrust_core::class_cache_stamp_matches(
            class.borrow().mutation_version.get(),
            class_version,
            epoch,
        )
    }

    /// Construction is allocation only: `__init__` is `object.__init__`.
    #[inline]
    pub(crate) fn is_allocation_only(&self) -> bool {
        self.init.is_none()
    }

    /// Whether constructions are offered to the finalizer bookkeeping.
    #[cfg(test)]
    pub(crate) fn has_finalizer(&self) -> bool {
        self.has_finalizer
    }

    /// Adopt `fresh`'s stamp when it resolved to the very same construction
    /// facts, so an unrelated class mutation costs one re-resolution instead
    /// of a new plan allocation.  Returns `false` when the facts changed.
    #[inline]
    pub(crate) fn refresh_from(&self, fresh: &PlainClassPlan) -> bool {
        let same_init = match (&self.init, &fresh.init) {
            (None, None) => true,
            (Some(current), Some(new)) => current.function.ptr_eq(&new.function),
            _ => false,
        };
        if !same_init || self.has_finalizer != fresh.has_finalizer {
            return false;
        }
        self.stamp.set(fresh.stamp.get());
        true
    }
}

/// Is `value` an immutable built-in collection (`tuple` / `frozenset`)?
///
/// Decided by typed identity, never by a Python-visible type name.
fn is_immutable_builtin_collection(value: &Value) -> bool {
    match value.kind() {
        ValueKind::Tuple(_) => true,
        ValueKind::BuiltinObject { ops, .. } => {
            ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Frozenset)
        }
        _ => false,
    }
}

impl Interpreter {
    /// Resolve the plain-class construction plan for `class`, or `None` when
    /// any step of `Cls(...)` deviates from default `type.__call__` allocation
    /// plus an ordinary-function `__init__`.
    ///
    /// This is the single place that decides plain-class eligibility; it
    /// mirrors, in order, the checks of `call_class_after_primitive_miss`,
    /// `call_class_expanded`, `default_construct`, and
    /// `instantiate_normal_instance`.  It runs no Python code, so the stamp it
    /// records cannot go stale while the facts are being read.
    pub(crate) fn resolve_plain_class_plan(
        &mut self,
        class: &Rc<RefCell<PyClass>>,
    ) -> Option<PlainClassPlan> {
        // Typing markers and canonical adapter classes never reach generic
        // construction; the adapter domain owns that routing decision.
        if Self::class_has_construction_adapter(class) {
            return None;
        }
        let (stamp, abstracts) = {
            let borrowed = class.borrow();
            // Interpreter-owned classes are constructed by their own adapters.
            if borrowed.canonical_tag.is_some() || borrowed.builtin_type_tag.is_some() {
                return None;
            }
            (
                pyrust_core::class_cache_stamp(borrowed.mutation_version.get())?,
                borrowed.attrs.get("__abstractmethods__").cloned(),
            )
        };
        // Only an *immutable* empty `__abstractmethods__` is provably harmless:
        // the generic path re-reads its truthiness on every construction, so a
        // mutable (or user-defined) value could turn truthy without any class
        // mutation to invalidate the plan.
        if let Some(abstracts) = abstracts
            && (!is_immutable_builtin_collection(&abstracts)
                || self.truthy_value(&abstracts).unwrap_or(true))
        {
            return None;
        }
        // Any `__call__` on a user metaclass (function, descriptor, or anything
        // else) keeps the class on the generic dispatch path.
        if crate::interpreter::metaclass_dunder(class, "__call__").is_some() {
            return None;
        }
        if is_exception_class(class) || class_is_subclass_of(class, &type_class_singleton()) {
            return None;
        }
        Self::resolve_plain_class_slots(class, stamp)
    }

    /// The `__new__` / `__init__` / `__del__` half of
    /// [`Self::resolve_plain_class_plan`], mirroring
    /// `instantiate_normal_instance`.
    fn resolve_plain_class_slots(
        class: &Rc<RefCell<PyClass>>,
        stamp: (u64, u64),
    ) -> Option<PlainClassPlan> {
        let (new_val, init_val, prim, has_finalizer) = match resolve_construction_plan_cached(class)
        {
            // The per-class plan is only ever cached for a finalizer-free chain.
            Some(plan) => (plan.new_val, plan.init_val, plan.prim, false),
            None => (
                lookup_class_attr(class, "__new__"),
                lookup_class_attr(class, "__init__"),
                classify_primitive_layout(class),
                lookup_class_attr(class, "__del__").is_some(),
            ),
        };
        if prim != PrimitiveLayout::None {
            return None;
        }
        if new_val.is_some_and(|value| {
            !crate::interpreter::value_is_canonical_slot(
                &value,
                crate::interpreter::CanonicalSlot::ObjectNew,
            )
        }) {
            return None;
        }
        // A wrapped `__new__` slot changes the excess-argument diagnostics even
        // though the attribute resolves back to `object.__new__` (#2335).
        if class_chain_new_slot_wrapped(class) {
            return None;
        }
        let init = match init_val {
            None => None,
            Some(value)
                if crate::interpreter::value_is_canonical_slot(
                    &value,
                    crate::interpreter::CanonicalSlot::ObjectInit,
                ) =>
            {
                None
            }
            Some(value) => match value.kind() {
                ValueKind::UserFunction(function)
                    if function.kind == pyrust_core::UserFunctionKind::Regular =>
                {
                    let simple_signature = !function
                        .params
                        .iter()
                        .any(|param| param.is_args || param.is_kwargs || param.is_keyword_only);
                    Some(PlainClassInit {
                        function: Rc::downgrade(function),
                        positional_parameter_count: if simple_signature {
                            function.params.len()
                        } else {
                            usize::MAX
                        },
                    })
                }
                // Descriptors, classmethod / staticmethod slots, builtins, and
                // non-callables keep the shared `invoke_class_method` dispatch.
                _ => return None,
            },
        };
        Some(PlainClassPlan {
            class: Rc::downgrade(class),
            stamp: std::cell::Cell::new(stamp),
            init,
            has_finalizer,
        })
    }
}

#[cfg(test)]
mod construction_identity_tests {
    use super::{
        Interpreter, PrimitiveLayout, PyClass, Rc, RefCell, ValueKind, object_class_singleton,
        resolve_construction_plan_cached,
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
            .builtin_data()
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
        assert!(instance.borrow().attrs.builtin_data().is_none());
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
