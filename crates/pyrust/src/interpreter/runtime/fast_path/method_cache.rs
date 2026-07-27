impl Interpreter {
    /// Fill or update the `CallMethod` inline cache after a slow-path dispatch.
    /// Mirrors the `GetAttr` cache policy: a different class at this site goes
    /// `Megamorphic`; a stale `ClassAttr` resets to `Empty` for a refill; an
    /// `Empty` slot caches the resolved unbound method when it is a cacheable
    /// Regular `UserFunction` / `BuiltinFunction`, or a native classmethod
    /// descriptor, approved by the attribute domain's typed cache plan.
    pub(super) fn update_call_method_cache(
        &self,
        obj_val: &Value,
        method: &str,
        code: &crate::bytecode::FnCode,
        call_site_pc: usize,
    ) {
        use crate::bytecode::AttrCacheEntry;

        // A deoptimized call site always uses generic lookup. In particular,
        // avoid re-running PyClass descriptor classification after an
        // ordinary Python classmethod/property has already been rejected.
        if matches!(
            code.attr_cache.borrow()[call_site_pc],
            AttrCacheEntry::Megamorphic | AttrCacheEntry::Uncacheable
        ) {
            return;
        }
        let plan = read_method_cache_plan(obj_val, method);
        let mut cache = code.attr_cache.borrow_mut();
        match &cache[call_site_pc] {
            AttrCacheEntry::Megamorphic | AttrCacheEntry::Uncacheable => {}
            AttrCacheEntry::ClassAttr {
                class_ptr: existing_ptr,
                ..
            } => {
                if let Some(inst_rc) = obj_val.as_py_instance_rc() {
                    let new_ptr = Rc::as_ptr(&inst_rc.borrow().class);
                    if new_ptr != existing_ptr.as_ptr() {
                        // Different class at this call site — go megamorphic.
                        cache[call_site_pc] = AttrCacheEntry::Megamorphic;
                    } else {
                        // Same class but version changed (or instance shadow exists).
                        // Reset to Empty so the next slow-path execution refills
                        // with the current class version and updated method value.
                        cache[call_site_pc] = AttrCacheEntry::Empty;
                    }
                } else if matches!(obj_val.kind(), ValueKind::PyClass(_)) {
                    cache[call_site_pc] = AttrCacheEntry::Megamorphic;
                }
            }
            AttrCacheEntry::NativeClassMethod {
                class_ptr: existing_ptr,
                ..
            } => {
                if let ValueKind::PyClass(class) = obj_val.kind() {
                    if Rc::as_ptr(class) != existing_ptr.as_ptr() {
                        cache[call_site_pc] = AttrCacheEntry::Megamorphic;
                    } else {
                        cache[call_site_pc] = AttrCacheEntry::Empty;
                    }
                } else if obj_val.as_py_instance_rc().is_some() {
                    cache[call_site_pc] = AttrCacheEntry::Megamorphic;
                }
            }
            AttrCacheEntry::Empty => {
                cache[call_site_pc] = match plan {
                    ReadMethodCachePlan::Class(unbound_value) => {
                        let Some(inst_rc) = obj_val.as_py_instance_rc() else {
                            return;
                        };
                        let inst = inst_rc.borrow();
                        let Some((class_version, epoch)) = pyrust_core::class_cache_stamp(
                            inst.class.borrow().mutation_version.get(),
                        ) else {
                            return;
                        };
                        AttrCacheEntry::class_attr(
                            &inst.class,
                            class_version,
                            epoch,
                            &unbound_value,
                        )
                    }
                    ReadMethodCachePlan::NativeClassMethod(plan) => {
                        let ValueKind::PyClass(class) = obj_val.kind() else {
                            return;
                        };
                        let Some((class_version, epoch)) =
                            pyrust_core::class_cache_stamp(class.borrow().mutation_version.get())
                        else {
                            return;
                        };
                        AttrCacheEntry::NativeClassMethod {
                            class_ptr: Rc::downgrade(class),
                            class_version,
                            epoch,
                            plan,
                        }
                    }
                    ReadMethodCachePlan::Uncacheable => {
                        if matches!(obj_val.kind(), ValueKind::PyClass(_)) {
                            AttrCacheEntry::Megamorphic
                        } else {
                            AttrCacheEntry::Uncacheable
                        }
                    }
                };
            }
            // InstanceAttr / SlotAttr / SetInstanceAttr are GetAttr / SetAttr-only
            // entries (#1912 / #2207 / #1998).  A CallMethod site never produces
            // them, but be defensive: drop to Empty so the next pass refills.
            AttrCacheEntry::InstanceAttr { .. }
            | AttrCacheEntry::SlotAttr { .. }
            | AttrCacheEntry::SetInstanceAttr { .. } => {
                cache[call_site_pc] = AttrCacheEntry::Empty;
            }
        }
    }
    /// Inline-cache fast path for `obj.method(...)` on a user-defined
    /// `PyInstance`, plus native classmethod calls on a `PyClass`. Returns
    /// `Ok(Some(result))` on a hit and `Ok(None)` on a miss so the caller takes
    /// generic `get_attr` + call dispatch. Instance entries validate shadowing;
    /// both shapes validate class identity, mutation version, and epoch.
    fn try_call_method_cached(
        &mut self,
        regs: &RegSlice,
        obj: crate::bytecode::Reg,
        method: &str,
        args: &[Value],
        code: &crate::bytecode::FnCode,
        call_site_pc: usize,
    ) -> Result<Option<Value>> {
        use crate::bytecode::AttrCacheEntry;

        // Resolve the cached method + receiver under a SHORT-LIVED borrow of
        // `code.attr_cache`, then DROP it before invoking anything.  The cache
        // borrow must not span `invoke_class_method` / a backing-primitive
        // dispatch: those run user code, and a recursive method (one whose
        // body executes on this same `code` object) re-enters `GetAttr` and
        // calls `code.attr_cache.borrow_mut()` in `fill_get_attr_cache`,
        // panicking with "RefCell already borrowed" if the outer borrow is
        // still alive.  `re_py.py`'s deeply recursive `_Matcher` methods made
        // this re-entrant collision reachable (issue #2625).
        enum CachedMethodResolution {
            Instance(Value, Rc<std::cell::RefCell<pyrust_core::PyInstance>>),
            NativeClassMethod(
                NativeClassMethodCachePlan,
                Rc<std::cell::RefCell<pyrust_core::PyClass>>,
            ),
            Miss,
        }
        let resolved = {
            let cache = code.attr_cache.borrow();
            match &cache[call_site_pc] {
                AttrCacheEntry::ClassAttr {
                    class_ptr,
                    class_version,
                    epoch,
                    value: unbound,
                } => {
                    if let Some(inst_rc) = regs[obj as usize].as_py_instance_rc() {
                        let inst = inst_rc.borrow();
                        let same_class = Rc::as_ptr(&inst.class) == class_ptr.as_ptr();
                        let no_shadow = !inst.attrs.contains_key(method);
                        let stamp_ok = pyrust_core::class_cache_stamp_matches(
                            inst.class.borrow().mutation_version.get(),
                            *class_version,
                            *epoch,
                        );
                        if same_class && no_shadow && stamp_ok {
                            match unbound.upgrade() {
                                Some(unbound) => {
                                    let inst_rc_clone = Rc::clone(inst_rc);
                                    drop(inst);
                                    CachedMethodResolution::Instance(unbound, inst_rc_clone)
                                }
                                None => CachedMethodResolution::Miss,
                            }
                        } else {
                            CachedMethodResolution::Miss
                        }
                    } else {
                        CachedMethodResolution::Miss
                    }
                }
                AttrCacheEntry::NativeClassMethod {
                    class_ptr,
                    class_version,
                    epoch,
                    plan,
                } => {
                    if let ValueKind::PyClass(class) = regs[obj as usize].kind() {
                        let same_class = Rc::as_ptr(class) == class_ptr.as_ptr();
                        let stamp_ok = pyrust_core::class_cache_stamp_matches(
                            class.borrow().mutation_version.get(),
                            *class_version,
                            *epoch,
                        );
                        if same_class && stamp_ok {
                            CachedMethodResolution::NativeClassMethod(
                                plan.clone(),
                                Rc::clone(class),
                            )
                        } else {
                            CachedMethodResolution::Miss
                        }
                    } else {
                        CachedMethodResolution::Miss
                    }
                }
                _ => CachedMethodResolution::Miss,
            }
            // `cache` borrow dropped here at end of block scope.
        };
        let (unbound, inst_rc_clone) = match resolved {
            CachedMethodResolution::NativeClassMethod(plan, class) => {
                let Some(result) = self.try_call_cached_native_class_method(&plan, &class, args)
                else {
                    return Ok(None);
                };
                return result.map(Some);
            }
            CachedMethodResolution::Instance(unbound, instance) => (unbound, instance),
            CachedMethodResolution::Miss => return Ok(None),
        };
        // Exact builtin sentinel decoding and backing-storage selection belong
        // to `builtin_methods`; this cache layer delegates through Values and
        // remains independent of concrete Python method names.
        if let Some(result) =
            self.try_dispatch_cached_builtin_method(&unbound, &inst_rc_clone, args)
        {
            return result.map(Some);
        }
        let inst_val = Value::py_instance(inst_rc_clone);
        let mut buf = std::mem::take(&mut self.call_arg_buf);
        buf.clear();
        for arg in args.iter() {
            buf.push(ExpandedCallArg {
                name: None,
                value: arg.clone(),
            });
        }
        let r = invoke_class_method(self, unbound, inst_val, &buf);
        self.call_arg_buf = buf;
        Ok(Some(r?))
    }

    /// Trampoline-resolution fast path for `o.m(...)` (#2345).  On an inline-cache
    /// hit whose cached value is a *Regular* `UserFunction` (a plain Python
    /// method), returns the unbound method and the receiver so the VM dispatch
    /// loop can trampoline the call — binding the receiver to `self` and looping
    /// instead of re-entering `call_user_function_expanded` natively, matching
    /// the speedup plain function calls already enjoy.
    ///
    /// Returns `None` (caller falls back to `exec_call_method`) when: the cache
    /// is not a `ClassAttr` hit, the receiver shadows the method, the class
    /// version/epoch changed, the cached value is a `BuiltinFunction` or a
    /// backing-primitive method, or the object is not a `PyInstance`.  Pure
    /// resolution: it does not touch `attr_cache` (a populated cache means a
    /// prior slow pass already filled it) or invoke anything.
    #[inline]
    pub(super) fn resolve_method_cached(
        &self,
        regs: &RegSlice,
        obj: crate::bytecode::Reg,
        method: &str,
        code: &crate::bytecode::FnCode,
        call_site_pc: usize,
    ) -> Option<(
        Rc<UserFunction>,
        Rc<std::cell::RefCell<pyrust_core::PyInstance>>,
    )> {
        use crate::bytecode::AttrCacheEntry;
        let cache = code.attr_cache.borrow();
        let AttrCacheEntry::ClassAttr {
            class_ptr,
            class_version,
            epoch,
            value: unbound,
        } = &cache[call_site_pc]
        else {
            return None;
        };
        let inst_rc = regs[obj as usize].as_py_instance_rc()?;
        let inst = inst_rc.borrow();
        let same_class = Rc::as_ptr(&inst.class) == class_ptr.as_ptr();
        let no_shadow = !inst.attrs.contains_key(method);
        let stamp_ok = pyrust_core::class_cache_stamp_matches(
            inst.class.borrow().mutation_version.get(),
            *class_version,
            *epoch,
        );
        if !(same_class && no_shadow && stamp_ok) {
            return None;
        }
        // Upgrade directly to the function backing after all guards pass.
        // Constructing a temporary Value here would allocate an OpaqueSlot on
        // every trampolined method call only to extract this same Rc again.
        let f = unbound.upgrade_user_function()?;
        // Only Regular user methods are trampolinable; BuiltinFunction methods
        // (list.append / backing-primitive routes) need the native path.
        if !matches!(f.kind, pyrust_core::UserFunctionKind::Regular) {
            return None;
        }
        let recv = Rc::clone(inst_rc);
        drop(inst);
        Some((f, recv))
    }
}
