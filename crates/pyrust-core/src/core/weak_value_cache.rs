/// Cache-safe representation of a [`Value`] stored by an inline cache.
///
/// Global-name caches must not become an additional Python-visible owner of the
/// resolved object: a function can point back to the `FnCode` that owns the
/// cache, and an obsolete reference-bearing global must be collectable after
/// its binding is replaced. Scalar values are copied inline. Values with an
/// `Rc`-backed identity retain only a [`Weak`] handle and reconstruct a normal
/// `Value` after a successful upgrade. Only inline strings are cacheable:
/// heap-string storage has no weak control block, and a cache must never become
/// an extra owner merely because a value appears cycle-free today. Native and
/// Python user functions uniformly use the weak function lane. Identity-free
/// opaque scalars may use the owned lane; reference-bearing wrappers deliberately
/// remain uncacheable.
#[derive(Clone)]
pub struct WeakValueCache(WeakValueCacheKind);

#[derive(Clone)]
enum WeakValueCacheKind {
    Inline(u64),
    Owned(Value),
    Tuple(Weak<TupleInner>),
    List(Weak<ListInner>),
    BigInt(Weak<BigInt>),
    Dict(Weak<RefCell<PyDict>>),
    Set(Weak<SetInner>),
    BigRange(Weak<BigRangeData>),
    UserFunction(Weak<UserFunction>),
    PyClass(Weak<RefCell<PyClass>>),
    PyInstance(Weak<RefCell<PyInstance>>),
    PyModule(Weak<RefCell<PyModule>>),
    Generator(Weak<GeneratorCell>),
    Bytes(Weak<Vec<u8>>),
    BuiltinObject {
        ops: &'static dyn BuiltinTypeOps,
        state: Weak<RefCell<Box<dyn Any>>>,
    },
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
                Opaque::PyBigInt(value) => Some(Self(Kind::BigInt(Rc::downgrade(value)))),
                Opaque::Dict(value) => Some(Self(Kind::Dict(Rc::downgrade(value)))),
                Opaque::Set(value) => Some(Self(Kind::Set(Rc::downgrade(value)))),
                Opaque::BigRange(value) => Some(Self(Kind::BigRange(Rc::downgrade(value)))),
                Opaque::UserFunction(value) => Some(Self(Kind::UserFunction(Rc::downgrade(value)))),
                Opaque::PyClass(value) => Some(Self(Kind::PyClass(Rc::downgrade(value)))),
                Opaque::PyInstance(value) => Some(Self(Kind::PyInstance(Rc::downgrade(value)))),
                Opaque::PyModule(value) => Some(Self(Kind::PyModule(Rc::downgrade(value)))),
                Opaque::Generator(value) => Some(Self(Kind::Generator(Rc::downgrade(value)))),
                Opaque::Bytes(value) => Some(Self(Kind::Bytes(Rc::downgrade(value)))),
                Opaque::BuiltinObject { ops, state } => Some(Self(Kind::BuiltinObject {
                    ops: *ops,
                    state: Rc::downgrade(state),
                })),
                // These values live only in the custom OpaqueSlot allocation.
                // Their nested Rc fields (where present) do not prove that the
                // wrapper object itself is still alive, so they cannot implement a
                // faithful weak reference.
                Opaque::Range { .. } | Opaque::Complex(..) => {
                    Some(Self(Kind::Owned(value.clone())))
                }
                Opaque::SmallTuple2 { .. }
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
    /// Script fastlocal caches use their live register locator for the other
    /// representations. That preserves the cache's non-owning policy while
    /// avoiding one OpaqueSlot allocation on every global-load hit.
    #[inline]
    pub fn upgrade_is_allocation_free(&self) -> bool {
        matches!(
            &self.0,
            WeakValueCacheKind::Inline(_)
                | WeakValueCacheKind::Owned(_)
                | WeakValueCacheKind::Tuple(_)
                | WeakValueCacheKind::List(_)
        )
    }

    /// Upgrade the cached value while its Python-visible owner is still alive.
    ///
    /// A failed upgrade is an ordinary inline-cache miss.
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
            Kind::BigInt(value) => value
                .upgrade()
                .map(|value| Value::opaque(Opaque::PyBigInt(value))),
            Kind::Dict(value) => value
                .upgrade()
                .map(|value| Value::opaque(Opaque::Dict(value))),
            Kind::Set(value) => value
                .upgrade()
                .map(|value| Value::opaque(Opaque::Set(value))),
            Kind::BigRange(value) => value
                .upgrade()
                .map(|value| Value::opaque(Opaque::BigRange(value))),
            Kind::UserFunction(value) => value.upgrade().map(Value::user_function),
            Kind::PyClass(value) => value.upgrade().map(Value::py_class),
            Kind::PyInstance(value) => value.upgrade().map(Value::py_instance),
            Kind::PyModule(value) => value.upgrade().map(Value::py_module),
            Kind::Generator(value) => value
                .upgrade()
                .map(|value| Value::opaque(Opaque::Generator(value))),
            Kind::Bytes(value) => value
                .upgrade()
                .map(|value| Value::opaque(Opaque::Bytes(value))),
            Kind::BuiltinObject { ops, state } => state
                .upgrade()
                .map(|state| Value::builtin_object_shared(*ops, state)),
        }
    }

    /// Upgrade a cached function directly to its shared backing.
    ///
    /// Method-call caches use this form to avoid allocating a temporary
    /// `OpaqueSlot` merely to inspect a regular function and extract the same
    /// `Rc` again.
    #[inline]
    pub fn upgrade_user_function(&self) -> Option<Rc<UserFunction>> {
        match &self.0 {
            WeakValueCacheKind::UserFunction(value) => value.upgrade(),
            _ => None,
        }
    }

    /// Upgrade a cached exact dict directly to its shared backing.
    ///
    /// Import-registry reads operate on the backing `Rc` and must not allocate
    /// a temporary `OpaqueSlot` merely to recover the same dictionary.
    #[inline]
    pub fn upgrade_dict(&self) -> Option<Rc<RefCell<PyDict>>> {
        match &self.0 {
            WeakValueCacheKind::Dict(value) => value.upgrade(),
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
            self_bind: None,
            local_names: Rc::new(HashSet::new()),
            local_index: Rc::new(HashMap::new()),
            global_names: Rc::new(HashSet::new()),
            nonlocal_names: Rc::new(HashSet::new()),
            env: Environment::new(None),
            is_memo_pure: false,
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
}
