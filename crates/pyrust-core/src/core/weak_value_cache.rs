/// Non-owning weak handle to an existing [`OpaqueSlot`] slab.
///
/// Holds one weak count on the slot so `upgrade()` can bump `strong` and return
/// the exact existing NaN-boxed `Value` with **zero** slab allocations while
/// still letting the `Opaque` payload drop deterministically as soon as the
/// final strong `Value` disappears.
struct WeakOpaqueSlot {
    bits: u64,
    _thread_bound: ThreadBoundValueMarker,
}

impl WeakOpaqueSlot {
    #[inline]
    fn slot_ptr(&self) -> *mut OpaqueSlot {
        (self.bits & PAYLOAD_MASK) as *mut OpaqueSlot
    }

    #[inline]
    fn downgrade(value: &Value) -> Option<Self> {
        debug_assert_eq!(top16(value.0), TAG_OPAQUE);
        let ptr = unsafe { value.opaque_slot_ptr() };
        let weak = unsafe { (*ptr).weak.get() };
        debug_assert!(weak > 0, "live opaque slot missing implicit weak reference");
        if weak == u32::MAX {
            return None;
        }
        unsafe { (*ptr).weak.set(weak + 1) };
        Some(Self {
            bits: value.0,
            _thread_bound: std::marker::PhantomData,
        })
    }

    #[inline]
    fn upgrade(&self) -> Option<Value> {
        let ptr = self.slot_ptr();
        let strong = unsafe { (*ptr).strong.get() };
        if strong == 0 {
            return None;
        }
        unsafe { (*ptr).strong.set(strong.saturating_add(1)) };
        Some(Value::from_bits(self.bits))
    }

    #[inline]
    fn with_live_opaque<R>(&self, f: impl FnOnce(&Opaque) -> Option<R>) -> Option<R> {
        let ptr = self.slot_ptr();
        if unsafe { (*ptr).strong.get() } == 0 {
            return None;
        }
        let opaque = unsafe { &*std::ptr::addr_of!((*ptr).value).cast::<Opaque>() };
        f(opaque)
    }
}

impl Clone for WeakOpaqueSlot {
    #[inline]
    fn clone(&self) -> Self {
        let ptr = self.slot_ptr();
        let weak = unsafe { (*ptr).weak.get() };
        debug_assert!(weak > 0, "cloning a released weak opaque slot");
        unsafe { (*ptr).weak.set(weak.saturating_add(1)) };
        Self {
            bits: self.bits,
            _thread_bound: std::marker::PhantomData,
        }
    }
}

impl Drop for WeakOpaqueSlot {
    #[inline]
    fn drop(&mut self) {
        let ptr = self.slot_ptr();
        let weak = unsafe { (*ptr).weak.get() };
        debug_assert!(weak > 0, "dropping an already released weak opaque slot");
        if weak == u32::MAX {
            return;
        }
        if weak > 1 {
            unsafe { (*ptr).weak.set(weak - 1) };
        } else {
            debug_assert_eq!(
                unsafe { (*ptr).strong.get() },
                0,
                "deallocating opaque slot while strong refs remain"
            );
            unsafe { pool_opaque_dealloc(ptr as *mut u8) };
        }
    }
}

/// Cache-safe representation of a [`Value`] stored by an inline cache.
///
/// Global-name caches must not become an additional Python-visible owner of the
/// resolved object: a function can point back to the `FnCode` that owns the
/// cache, and an obsolete reference-bearing global must be collectable after
/// its binding is replaced. Scalar values are copied inline. Values with an
/// `Rc`-backed identity or a weak-counted [`OpaqueSlot`] retain only a non-owning
/// handle and upgrade directly to the existing `Value` with zero slab
/// allocations. Only inline strings are cacheable: heap-string storage has no
/// weak control block, and a cache must never become an extra owner merely
/// because a value appears cycle-free today. Identity-free opaque scalars may
/// use the owned lane; reference-bearing wrappers deliberately remain
/// uncacheable.
#[derive(Clone)]
pub struct WeakValueCache(WeakValueCacheKind);

impl std::fmt::Debug for WeakValueCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WeakValueCache(..)")
    }
}

#[derive(Clone)]
enum WeakValueCacheKind {
    Inline(u64),
    Owned(Value),
    Tuple(Weak<TupleInner>),
    List(Weak<ListInner>),
    Opaque(WeakOpaqueSlot),
}

impl WeakValueCache {
    /// Build a cache-safe value, or return `None` when this representation
    /// cannot be retained without either changing Python object identity or
    /// introducing an unbounded/cyclic strong reference.
    pub fn new(value: &Value) -> Option<Self> {
        use WeakValueCacheKind as Kind;

        match top16(value.0) {
            // Float/sentinels and the None/bool/int tags own no heap allocation.
            tag if tag <= TAG_INT => Some(Self(Kind::Inline(value.0))),
            TAG_STR if str_is_inline_bits(value.0) => Some(Self(Kind::Inline(value.0))),
            // Heap strings use compact intrusive strong counts without a weak
            // control block. Retaining one would make the cache an owner.
            TAG_STR => None,
            TAG_TUPLE => {
                // SAFETY: TAG_TUPLE stores one live Rc<TupleInner> strong reference.
                // Temporarily clone that reference, downgrade it, then release the
                // temporary; the original Value's ownership is unchanged.
                let tuple = unsafe {
                    let ptr = value.tuple_inner_ptr();
                    Rc::increment_strong_count(ptr);
                    Rc::from_raw(ptr)
                };
                let weak = Rc::downgrade(&tuple);
                drop(tuple);
                Some(Self(Kind::Tuple(weak)))
            }
            TAG_LIST => {
                // SAFETY: identical to the TAG_TUPLE conversion above.
                let list = unsafe {
                    let ptr = value.list_inner_ptr();
                    Rc::increment_strong_count(ptr);
                    Rc::from_raw(ptr)
                };
                let weak = Rc::downgrade(&list);
                drop(list);
                Some(Self(Kind::List(weak)))
            }
            TAG_OPAQUE => match unsafe { &*value.opaque_ptr() } {
                Opaque::PyBigInt(_)
                | Opaque::Dict(_)
                | Opaque::Set(_)
                | Opaque::BigRange(_)
                | Opaque::UserFunction(_)
                | Opaque::PyClass(_)
                | Opaque::PyInstance(_)
                | Opaque::PyModule(_)
                | Opaque::Generator(_)
                | Opaque::Bytes(_)
                | Opaque::BuiltinObject { .. } => {
                    WeakOpaqueSlot::downgrade(value).map(|slot| Self(Kind::Opaque(slot)))
                }
                // Identity-free immutable scalars with no reference-bearing
                // payload may be cloned directly.
                Opaque::Range { .. } | Opaque::Complex(..) => {
                    Some(Self(Kind::Owned(value.clone())))
                }
                Opaque::SmallTuple1 { .. }
                | Opaque::SmallTuple2 { .. }
                | Opaque::SmallTuple3 { .. }
                | Opaque::BoundMethod { .. }
                | Opaque::ClassBoundMethod { .. }
                | Opaque::SuperProxy { .. }
                | Opaque::SuperProxyClass { .. }
                | Opaque::SuperProxyUnbound { .. } => None,
            },
            _ => unreachable!("invalid Value tag"),
        }
    }

    /// Whether upgrading this representation avoids allocating a replacement
    /// opaque wrapper.
    ///
    /// Every cacheable representation now upgrades allocation-free: scalars and
    /// owned immediates copy/bump bits, `Tuple`/`List` upgrade their `Rc`
    /// directly, and `Opaque` upgrades its existing weak-counted `OpaqueSlot`.
    #[inline]
    pub fn upgrade_is_allocation_free(&self) -> bool {
        true
    }

    /// Upgrade the cached value while its Python-visible owner is still alive.
    ///
    /// A failed upgrade is an ordinary inline-cache miss.
    #[inline]
    pub fn upgrade(&self) -> Option<Value> {
        use WeakValueCacheKind as Kind;

        match &self.0 {
            Kind::Inline(bits) => Some(Value::from_bits(*bits)),
            Kind::Owned(value) => Some(value.clone()),
            Kind::Tuple(value) => value
                .upgrade()
                .map(|value| unsafe { Value::tuple_from_rc(value) }),
            Kind::List(value) => value
                .upgrade()
                .map(|value| unsafe { Value::list_from_rc(value) }),
            Kind::Opaque(slot) => slot.upgrade(),
        }
    }

    /// Upgrade a cached function directly to its shared backing without
    /// touching the slot's strong reference count.
    #[inline]
    pub fn upgrade_user_function(&self) -> Option<Rc<UserFunction>> {
        match &self.0 {
            WeakValueCacheKind::Opaque(slot) => slot.with_live_opaque(|opaque| match opaque {
                Opaque::UserFunction(value) => Some(Rc::clone(value)),
                _ => None,
            }),
            _ => None,
        }
    }

    /// Upgrade a cached exact dict directly to its shared backing without
    /// touching the slot's strong reference count.
    #[inline]
    pub fn upgrade_dict(&self) -> Option<Rc<RefCell<PyDict>>> {
        match &self.0 {
            WeakValueCacheKind::Opaque(slot) => slot.with_live_opaque(|opaque| match opaque {
                Opaque::Dict(value) => Some(Rc::clone(value)),
                _ => None,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod weak_value_cache_tests {
    use super::{
        Environment, HashMap, HashSet, IndexMap, InstanceAttrs, PyClass, PyInstance, Rc, RefCell,
        UserFunction, UserFunctionKind, Value, ValueKind, WeakValueCache, next_fn_id,
    };

    fn make_cache_user_function() -> Rc<UserFunction> {
        Rc::new(UserFunction {
            id: next_fn_id(),
            kind: UserFunctionKind::Regular,
            name: Rc::from("f"),
            qualname: Rc::from("f"),
            name_overrides: RefCell::new(None),
            module: RefCell::new(Value::unset()),
            doc: RefCell::new(Value::none()),
            attrs: RefCell::new(None),
            annotations: RefCell::new(Value::unset()),
            defaults_override: RefCell::new(None),
            params: Vec::new(),
            param_binds: Rc::new(Vec::new()),
            memo_positional_parameter_count: 0,
            trampoline_arity: UserFunction::TRAMPOLINE_INELIGIBLE,
            self_bind: None,
            local_names: Rc::new(HashSet::new()),
            local_index: Rc::new(HashMap::new()),
            global_names: Rc::new(HashSet::new()),
            nonlocal_names: Rc::new(HashSet::new()),
            env: Environment::new(None),
            closure_cells: smallvec::SmallVec::new(),
            is_memo_pure: false,
            iterable_coroutine: std::cell::Cell::new(false),
            precompiled_code: None,
            wrapped_func: None,
        })
    }

    #[test]
    fn upgraded_function_and_instance_preserve_backing_identity() {
        let function = make_cache_user_function();
        let function_value = Value::user_function(Rc::clone(&function));
        let function_cache = WeakValueCache::new(&function_value).expect("function is cacheable");
        let cached_function = function_cache.upgrade().expect("function is alive");
        let ValueKind::UserFunction(cached_function_rc) = cached_function.kind() else {
            panic!("expected cached function");
        };
        assert!(Rc::ptr_eq(&function, cached_function_rc));
        assert_eq!(function_value.value_id(), cached_function.value_id());

        let class = Rc::new(RefCell::new(PyClass::new("C", "C", None, IndexMap::new())));
        let instance = Rc::new(RefCell::new(PyInstance {
            class,
            attrs: InstanceAttrs::new(),
        }));
        let instance_value = Value::py_instance(Rc::clone(&instance));
        let instance_cache = WeakValueCache::new(&instance_value).expect("instance is cacheable");
        let cached_instance = instance_cache.upgrade().expect("instance is alive");
        let ValueKind::PyInstance(cached_instance_rc) = cached_instance.kind() else {
            panic!("expected cached instance");
        };
        assert!(Rc::ptr_eq(&instance, cached_instance_rc));
        assert_eq!(instance_value.value_id(), cached_instance.value_id());
    }

    #[test]
    fn cache_does_not_keep_function_or_instance_alive() {
        let function = make_cache_user_function();
        let function_liveness = Rc::downgrade(&function);
        let function_value = Value::user_function(Rc::clone(&function));
        let function_cache = WeakValueCache::new(&function_value).expect("function is cacheable");
        drop(function_value);
        drop(function);
        assert!(function_liveness.upgrade().is_none());
        assert!(function_cache.upgrade().is_none());

        let class = Rc::new(RefCell::new(PyClass::new("C", "C", None, IndexMap::new())));
        let instance = Rc::new(RefCell::new(PyInstance {
            class,
            attrs: InstanceAttrs::new(),
        }));
        let instance_liveness = Rc::downgrade(&instance);
        let instance_value = Value::py_instance(Rc::clone(&instance));
        let instance_cache = WeakValueCache::new(&instance_value).expect("instance is cacheable");
        drop(instance_value);
        drop(instance);
        assert!(instance_liveness.upgrade().is_none());
        assert!(instance_cache.upgrade().is_none());
    }

    #[test]
    fn heap_string_without_weak_storage_is_uncacheable() {
        let heap_string = Value::string("longer than the inline string capacity");
        assert!(WeakValueCache::new(&heap_string).is_none());
    }

    #[test]
    fn inline_string_remains_cacheable_by_value() {
        let inline_string = Value::string("short");
        let cache = WeakValueCache::new(&inline_string).expect("inline string is cacheable");
        drop(inline_string);
        assert_eq!(
            cache.upgrade().expect("inline string cache hit").as_str(),
            Some("short")
        );
    }

    #[test]
    fn reference_bearing_wrapper_remains_uncacheable() {
        let bound = Value::bound_method(
            make_cache_user_function(),
            Rc::new(RefCell::new(PyInstance {
                class: Rc::new(RefCell::new(PyClass::new("C", "C", None, IndexMap::new()))),
                attrs: InstanceAttrs::new(),
            })),
        );
        assert!(WeakValueCache::new(&bound).is_none());
    }

    #[test]
    fn native_function_uses_weak_lane_and_does_not_extend_lifetime() {
        let function = Value::fresh_builtin_function("__weak_cache_owned_test");
        let original_id = function.value_id();
        let liveness = function
            .as_function_rc()
            .map(Rc::downgrade)
            .expect("builtin function backing");
        let cache = WeakValueCache::new(&function).expect("native function is cacheable");

        let hit = cache.upgrade().expect("native function is alive");
        assert_eq!(hit.value_id(), original_id);
        let direct = cache
            .upgrade_user_function()
            .expect("native function weak lane upgrades directly");
        assert!(Rc::ptr_eq(
            function.as_function_rc().expect("builtin function backing"),
            &direct
        ));
        drop(hit);
        drop(direct);
        drop(function);
        assert!(
            liveness.upgrade().is_none(),
            "a cache must not become an owner of a native function"
        );
        assert!(cache.upgrade().is_none());
        assert!(cache.upgrade_user_function().is_none());
    }

    #[test]
    fn opaque_weak_cache_upgrades_allocate_zero_opaque_slots() {
        use crate::pool::{pool_opaque_alloc_count, pool_opaque_len};

        assert_eq!(
            std::mem::size_of::<super::OpaqueSlot>(),
            std::mem::size_of::<usize>() + std::mem::size_of::<super::Opaque>(),
            "adding weak count to OpaqueSlot must not grow it beyond its 8-byte header"
        );
        assert!(std::mem::size_of::<WeakValueCache>() <= 16);

        let user_fn = Value::user_function(make_cache_user_function());
        let builtin_fn = Value::fresh_builtin_function("__weak_zero_alloc_test");
        let py_class = Value::py_class(Rc::new(RefCell::new(PyClass::new(
            "C",
            "C",
            None,
            IndexMap::new(),
        ))));
        let py_dict = Value::dict(super::PyDict::default());

        let fn_cache = WeakValueCache::new(&user_fn).expect("user function is cacheable");
        let builtin_cache =
            WeakValueCache::new(&builtin_fn).expect("builtin function is cacheable");
        let class_cache = WeakValueCache::new(&py_class).expect("class is cacheable");
        let dict_cache = WeakValueCache::new(&py_dict).expect("dict is cacheable");
        assert!(fn_cache.upgrade_is_allocation_free());
        assert!(builtin_cache.upgrade_is_allocation_free());
        assert!(class_cache.upgrade_is_allocation_free());
        assert!(dict_cache.upgrade_is_allocation_free());

        let allocs_before = pool_opaque_alloc_count();
        let pool_len_before = pool_opaque_len();
        for _ in 0..1_000 {
            let u = fn_cache.upgrade().expect("user function alive");
            assert_eq!(u.value_id(), user_fn.value_id());
            let b = builtin_cache.upgrade().expect("builtin function alive");
            assert_eq!(b.value_id(), builtin_fn.value_id());
            let c = class_cache.upgrade().expect("class alive");
            assert_eq!(c.value_id(), py_class.value_id());
            let d = dict_cache.upgrade().expect("dict alive");
            assert_eq!(d.value_id(), py_dict.value_id());
            assert!(fn_cache.upgrade_user_function().is_some());
            assert!(dict_cache.upgrade_dict().is_some());
            drop((u, b, c, d));
        }
        assert_eq!(
            pool_opaque_alloc_count(),
            allocs_before,
            "repeated WeakValueCache::upgrade hits must allocate 0 OpaqueSlots"
        );
        assert_eq!(
            pool_opaque_len(),
            pool_len_before,
            "repeated WeakValueCache::upgrade hits must not churn POOL_OPAQUE"
        );
    }

    #[test]
    fn self_referential_opaque_drop_releases_slot_safely() {
        use crate::pool::pool_opaque_len;

        std::thread::spawn(|| {
            struct SelfWeakHolder(RefCell<Option<WeakValueCache>>);
            struct HolderOps;
            impl super::BuiltinTypeOps for HolderOps {
                fn type_name(&self) -> &'static str {
                    "SelfWeakHolder"
                }
            }
            static HOLDER_OPS: HolderOps = HolderOps;

            let holder =
                Value::builtin_object(&HOLDER_OPS, Box::new(SelfWeakHolder(RefCell::new(None))));
            let weak = WeakValueCache::new(&holder).expect("builtin object is cacheable");
            let ValueKind::BuiltinObject { state, .. } = holder.kind() else {
                panic!("expected builtin object");
            };
            *state
                .borrow()
                .downcast_ref::<SelfWeakHolder>()
                .expect("holder state")
                .0
                .borrow_mut() = Some(weak);
            assert_eq!(pool_opaque_len(), 0);
            drop(holder);
            assert_eq!(
                pool_opaque_len(),
                1,
                "dropping a self-weak-referencing OpaqueSlot must return the slab to POOL_OPAQUE"
            );
        })
        .join()
        .expect("self-referential opaque drop test thread");
    }

    #[test]
    fn try_scalar_truthiness_covers_bool_int_none_and_rejects_others() {
        assert_eq!(Value::none().try_scalar_truthiness(), Some(false));
        assert_eq!(Value::bool_(false).try_scalar_truthiness(), Some(false));
        assert_eq!(Value::bool_(true).try_scalar_truthiness(), Some(true));
        assert_eq!(Value::int(0).try_scalar_truthiness(), Some(false));
        assert_eq!(Value::int(1).try_scalar_truthiness(), Some(true));
        assert_eq!(Value::int(-1).try_scalar_truthiness(), Some(true));
        assert_eq!(
            Value::int(-(1i64 << 47)).try_scalar_truthiness(),
            Some(true)
        );
        assert_eq!(
            Value::int((1i64 << 47) - 1).try_scalar_truthiness(),
            Some(true)
        );
        // Non-immediate / non-scalar / sentinel values defer to the slow path.
        assert_eq!(Value::unset().try_scalar_truthiness(), None);
        assert_eq!(Value::not_implemented().try_scalar_truthiness(), None);
        assert_eq!(Value::ellipsis().try_scalar_truthiness(), None);
        assert_eq!(Value::float(0.0).try_scalar_truthiness(), None);
        assert_eq!(Value::float(1.0).try_scalar_truthiness(), None);
        assert_eq!(Value::string("").try_scalar_truthiness(), None);
        assert_eq!(Value::string("hi").try_scalar_truthiness(), None);
        assert_eq!(Value::int(1i64 << 60).try_scalar_truthiness(), None);
        assert_eq!(Value::list(vec![]).try_scalar_truthiness(), None);
        assert_eq!(
            Value::dict(super::PyDict::default()).try_scalar_truthiness(),
            None
        );
    }
}
