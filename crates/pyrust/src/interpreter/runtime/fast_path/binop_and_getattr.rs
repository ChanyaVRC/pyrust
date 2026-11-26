// ---------------------------------------------------------------------------
// BinOp / GetAttr / SetAttr inline-cache machinery, extracted from the `vm.rs`
// `run_bytecode_inner_impl` dispatch loop.
//
// These three `#[inline(always)]` methods carry the FULL body of the
// `Insn::BinOp` / `Insn::GetAttr` / `Insn::SetAttr` dispatch arms — both the
// cache-hit fast path and the slow-path cache fill / invalidation.  The dispatch
// arm in `vm.rs` becomes a single `vm_try!(self.exec_*(...))` call, so any error
// the slow path returns is routed through the active exception handler stack
// exactly as the old inline `vm_try!(...)` did.
//
// The bodies are a mechanical relocation of the old inline blocks:
//   - `vm_try!(expr)` → `expr?` (error propagation is now via `?`, and the
//     caller's `vm_try!` does the handler-stack routing);
//   - `pool_get!(code.names, idx, "name")` → an inline `code.names.get(..)`
//     with the identical out-of-range `PyError::Runtime` message.
// No cache-fill / invalidation / megamorphic logic changed — the inline caches
// are correctness-critical (#1912 / #1998 / #2102 / #2108) and must stay
// byte-identical.  `#[inline(always)]` keeps codegen at the dispatch site
// identical to when these lived inline; the PR bench table confirms zero
// regression on the hot int-arith / float / str / attr-read / attr-write paths.
// ---------------------------------------------------------------------------

#[inline]
fn set_binop_cache(
    code: &crate::bytecode::FnCode,
    slot: usize,
    entry: crate::bytecode::BinOpCacheEntry,
) {
    let mut cache = code.binop_cache.borrow_mut();
    if cache.is_empty() {
        cache.resize(code.insns.len(), crate::bytecode::BinOpCacheEntry::Empty);
    }
    if let Some(cell) = cache.get_mut(slot) {
        *cell = entry;
    }
}

impl Interpreter {
    /// Cold fallback for `exec_binop`: evaluates `eval_binary` without changing
    /// the site's cache state (e.g. edge cases like float div-by-zero or
    /// unsupported ops on specialized types).
    #[cold]
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn exec_binop_eval_slow(
        &mut self,
        regs: &mut RegSlice,
        dst: crate::bytecode::Reg,
        lhs: crate::bytecode::Reg,
        op: BinaryOp,
        rhs: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let l = vm_read(regs, lhs, num_locals)?;
        let r = vm_read(regs, rhs, num_locals)?;
        regs[dst as usize] = self.eval_binary(l, op, r)?;
        Ok(())
    }

    /// Cold deopt fallback for `exec_binop`: transitions a specialized site to
    /// `Megamorphic` on a type mismatch and delegates to `eval_binary`.
    #[cold]
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn exec_binop_deopt_slow(
        &mut self,
        regs: &mut RegSlice,
        code: &crate::bytecode::FnCode,
        cache_slot: usize,
        dst: crate::bytecode::Reg,
        lhs: crate::bytecode::Reg,
        op: BinaryOp,
        rhs: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        set_binop_cache(
            code,
            cache_slot,
            crate::bytecode::BinOpCacheEntry::Megamorphic,
        );
        self.exec_binop_eval_slow(regs, dst, lhs, op, rhs, num_locals)
    }

    /// Cold cache-state handler for `exec_binop`: warms `Empty` / `Counting`
    /// sites and evaluates `Megamorphic` / `Specialized(Int | Other)` through
    /// `eval_binary`.
    #[cold]
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn exec_binop_cold_state(
        &mut self,
        regs: &mut RegSlice,
        code: &crate::bytecode::FnCode,
        cache_slot: usize,
        cache_entry: crate::bytecode::BinOpCacheEntry,
        dst: crate::bytecode::Reg,
        lhs: crate::bytecode::Reg,
        op: BinaryOp,
        rhs: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        use crate::bytecode::{BINOP_SPEC_THRESHOLD, BinOpCacheEntry};
        match cache_entry {
            BinOpCacheEntry::Counting { tag, count } => {
                let observed = classify_binop_tag(&regs[lhs as usize], &regs[rhs as usize]);
                let new_entry = if observed == tag {
                    let new_count = count + 1;
                    if new_count >= BINOP_SPEC_THRESHOLD {
                        BinOpCacheEntry::Specialized(tag)
                    } else {
                        BinOpCacheEntry::Counting {
                            tag,
                            count: new_count,
                        }
                    }
                } else {
                    BinOpCacheEntry::Megamorphic
                };
                set_binop_cache(code, cache_slot, new_entry);
            }
            BinOpCacheEntry::Empty => {
                let observed = classify_binop_tag(&regs[lhs as usize], &regs[rhs as usize]);
                set_binop_cache(
                    code,
                    cache_slot,
                    BinOpCacheEntry::Counting {
                        tag: observed,
                        count: 1,
                    },
                );
            }
            // Megamorphic, Specialized(Int) (int_int_fast already missed on
            // overflow or unsupported op), and Specialized(Other) all keep
            // their current cache state and fall through to eval_binary.
            BinOpCacheEntry::Megamorphic | BinOpCacheEntry::Specialized(_) => {}
        }
        let l = vm_read(regs, lhs, num_locals)?;
        let r = vm_read(regs, rhs, num_locals)?;
        regs[dst as usize] = self.eval_binary(l, op, r)?;
        Ok(())
    }

    /// Full `Insn::BinOp` body: the unconditional int–int fast path followed by
    /// the specialized Float / NumMixed / Str inline-cache hit arms.  Cold
    /// cache-warming (`Empty` / `Counting`), deopt, and `eval_binary` fallbacks
    /// live out of line in `#[cold] #[inline(never)]` helpers (#3163).
    #[inline(always)]
    // Hot-path VM instruction handler: the arg list is the decoded `BinOp`
    // operands (dst/lhs/op/rhs + regs/code/pc/num_locals). Bundling into a struct
    // would add an indirection on the dispatch loop's hottest path — do not refactor.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn exec_binop(
        &mut self,
        regs: &mut RegSlice,
        code: &crate::bytecode::FnCode,
        pc: usize,
        dst: crate::bytecode::Reg,
        lhs: crate::bytecode::Reg,
        op: BinaryOp,
        rhs: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        use crate::bytecode::{BinOpCacheEntry, BinopTypeTag};
        // Hot path: `as_int()` is a tagged-u64 check that bypasses `kind()`'s
        // scoped RefCell borrow for the List/Dict/Set kinds (#450).  Unlike
        // `Insn::Move` (where #441 showed the int specialization is a wash), the
        // BinOp fast path also short-circuits the entire `eval_binary` dispatch
        // for int–int ops, so the savings are real.  This check runs
        // unconditionally (no cache overhead) so int-int loops pay nothing.
        if let (Some(a), Some(b)) = (regs[lhs as usize].as_int(), regs[rhs as usize].as_int())
            && let Some(result) = int_int_fast(a, b, op)
        {
            regs[dst as usize] = result;
            return Ok(());
        }
        // Adaptive inline cache for float / str specialisation.  Consulted after
        // the int fast path misses, so int-int code never pays cache-lookup
        // overhead.
        //
        // Deopt policy: only deopt (→ Megamorphic) on a *type* mismatch.
        // Edge-case values (e.g. div-by-zero in the Float path) fall through to
        // eval_binary while keeping the cache Specialized so subsequent calls
        // still hit the fast path.
        let cache_slot = pc - 1;
        let cache_entry = code
            .binop_cache
            .borrow()
            .get(cache_slot)
            .copied()
            .unwrap_or(BinOpCacheEntry::Empty);
        match cache_entry {
            BinOpCacheEntry::Specialized(BinopTypeTag::Float) => {
                let lv = &regs[lhs as usize];
                let rv = &regs[rhs as usize];
                if lv.is_float() && rv.is_float() {
                    let a = lv.as_float_raw();
                    let b = rv.as_float_raw();
                    if let Some(result) = float_float_fast(a, b, op) {
                        regs[dst as usize] = result;
                        return Ok(());
                    }
                    // Fast path returned None (e.g. div-by-zero, unsupported
                    // op): site is still Float/Float, so keep the Specialized
                    // state and fall through to eval_binary for this edge case.
                    return self.exec_binop_eval_slow(regs, dst, lhs, op, rhs, num_locals);
                }
                // Actual type mismatch: deopt to Megamorphic.
                self.exec_binop_deopt_slow(regs, code, cache_slot, dst, lhs, op, rhs, num_locals)
            }
            BinOpCacheEntry::Specialized(BinopTypeTag::NumMixed) => {
                let lv = &regs[lhs as usize];
                let rv = &regs[rhs as usize];
                // Still one int/bool + one float?
                if (lv.is_float() && rv.as_int().is_some())
                    || (lv.as_int().is_some() && rv.is_float())
                {
                    if let Some(result) = num_mixed_fast(lv, rv, op) {
                        regs[dst as usize] = result;
                        return Ok(());
                    }
                    // Unsupported op (comparison / Pow) or a div-by-zero edge:
                    // site is still mixed-numeric, so keep Specialized and fall
                    // through to eval_binary for this invocation.
                    return self.exec_binop_eval_slow(regs, dst, lhs, op, rhs, num_locals);
                }
                // Actual type mismatch: deopt to Megamorphic.
                self.exec_binop_deopt_slow(regs, code, cache_slot, dst, lhs, op, rhs, num_locals)
            }
            BinOpCacheEntry::Specialized(BinopTypeTag::Str) => {
                let lv = &regs[lhs as usize];
                let rv = &regs[rhs as usize];
                if lv.is_str() && rv.is_str() {
                    let a = lv.as_str().unwrap();
                    let b = rv.as_str().unwrap();
                    if let Some(result) = str_str_fast(a, b, op) {
                        regs[dst as usize] = result;
                        return Ok(());
                    }
                    // Unsupported op for str (e.g. str * str): keep Specialized,
                    // fall through to eval_binary which will raise the proper
                    // TypeError.
                    return self.exec_binop_eval_slow(regs, dst, lhs, op, rhs, num_locals);
                }
                // Type mismatch: deopt.
                self.exec_binop_deopt_slow(regs, code, cache_slot, dst, lhs, op, rhs, num_locals)
            }
            other => self.exec_binop_cold_state(
                regs, code, cache_slot, other, dst, lhs, op, rhs, num_locals,
            ),
        }
    }

    /// Full `Insn::GetAttr` body: the InstanceAttr / ClassAttr inline-cache hit
    /// path followed by the slow-path `get_attr` call + cache fill / invalidation.
    /// The cache machinery (#1912, epoch/version guards #2102/#2108) is verbatim.
    #[inline(always)]
    // Hot-path VM instruction handler: the arg list is the decoded `GetAttr`
    // operands (dst/obj/name_idx + regs/code/pc/num_locals) feeding the inline
    // attribute cache; do not bundle into a struct on this dispatch-loop path.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn exec_get_attr(
        &mut self,
        regs: &mut RegSlice,
        code: &crate::bytecode::FnCode,
        pc: usize,
        dst: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        name_idx: u16,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        use crate::bytecode::AttrCacheEntry;
        use pyrust_core::UserFunctionKind;

        // Inline cache fast path: only for PyInstance objects.  Properties,
        // __class__, __dict__, cached_property, and megamorphic sites all fall
        // through to the slow path.
        enum AttrFastResult {
            Hit(Value),
            /// Plain `@property` getter hit (#3382): the cache borrow must be
            /// released before `fget` runs, because the getter may re-enter code
            /// that fills or invalidates this very cache.
            Property(Rc<pyrust_core::UserFunction>, Value),
            Miss,
        }
        let fast = {
            let cache = code.attr_cache.borrow();
            match cache.get(pc - 1) {
                // Instance-attribute read cache (#1912): the name has no
                // data-descriptor shadow on the class, so the instance __dict__
                // takes priority.  Probe it directly, skipping the MRO walk in
                // get_attr_instance_raw.
                Some(AttrCacheEntry::InstanceAttr {
                    class_ptr,
                    class_version,
                    epoch,
                }) => {
                    if let Some(inst_rc) = regs[obj as usize].as_py_instance_rc() {
                        let inst = inst_rc.borrow();
                        let same_class = Rc::as_ptr(&inst.class) == class_ptr.as_ptr();
                        let stamp_ok = pyrust_core::class_cache_stamp_matches(
                            inst.class.borrow().mutation_version.get(),
                            *class_version,
                            *epoch,
                        );
                        if same_class && stamp_ok {
                            let name = code.names.get(name_idx as usize).map(|s| s.as_str());
                            match name.and_then(|n| inst.attrs.get(n)) {
                                Some(v) => AttrFastResult::Hit(v.clone()),
                                // Not in the instance dict — fall to the slow
                                // path (method / non-data descriptor /
                                // __getattr__ / AttributeError).
                                None => AttrFastResult::Miss,
                            }
                        } else {
                            AttrFastResult::Miss
                        }
                    } else {
                        AttrFastResult::Miss
                    }
                }
                // `__slots__` slot read (#2207): probe the physically separate
                // slot backing directly, exactly as `member_descriptor.__get__`
                // does. An unset slot falls through so the descriptor raises
                // the proper `AttributeError` (and any `__getattr__` runs).
                Some(AttrCacheEntry::SlotAttr(entry)) => {
                    if let Some(inst_rc) = regs[obj as usize].as_py_instance_rc() {
                        let inst = inst_rc.borrow();
                        let same_class = Rc::as_ptr(&inst.class) == entry.class_ptr.as_ptr();
                        let stamp_ok = pyrust_core::class_cache_stamp_matches(
                            inst.class.borrow().mutation_version.get(),
                            entry.class_version,
                            entry.epoch,
                        );
                        if same_class && stamp_ok {
                            match inst.attrs.get_member_slot(entry.slot_id) {
                                Some(v) => AttrFastResult::Hit(v.clone()),
                                // Unset slot — slow path raises AttributeError.
                                None => AttrFastResult::Miss,
                            }
                        } else {
                            AttrFastResult::Miss
                        }
                    } else {
                        AttrFastResult::Miss
                    }
                }
                // Plain `@property` read (#3382): `property` is a data
                // descriptor, so it wins over the instance `__dict__` and the
                // hit does not probe `inst.attrs`.  The class guards below are
                // the only validation needed; the fill policy already
                // excluded `__getattribute__` / `__getattr__` / `property`
                // subclasses / non-function getters.
                Some(AttrCacheEntry::PropertyGet(entry)) => {
                    if let Some(inst_rc) = regs[obj as usize].as_py_instance_rc() {
                        let inst = inst_rc.borrow();
                        let same_class = Rc::as_ptr(&inst.class) == entry.class_ptr.as_ptr();
                        let stamp_ok = pyrust_core::class_cache_stamp_matches(
                            inst.class.borrow().mutation_version.get(),
                            entry.class_version,
                            entry.epoch,
                        );
                        if same_class && stamp_ok {
                            drop(inst);
                            entry
                                .fget
                                .upgrade_user_function()
                                .map_or(AttrFastResult::Miss, |fget| {
                                    AttrFastResult::Property(fget, regs[obj as usize].clone())
                                })
                        } else {
                            AttrFastResult::Miss
                        }
                    } else {
                        AttrFastResult::Miss
                    }
                }
                Some(AttrCacheEntry::NativeClassMethod(entry)) => {
                    if let ValueKind::PyClass(class) = regs[obj as usize].kind() {
                        let same_class = Rc::as_ptr(class) == entry.class_ptr.as_ptr();
                        let stamp_ok = pyrust_core::class_cache_stamp_matches(
                            class.borrow().mutation_version.get(),
                            entry.class_version,
                            entry.epoch,
                        );
                        if same_class && stamp_ok {
                            bind_cached_native_class_method(&entry.plan, Rc::clone(class))
                                .map_or(AttrFastResult::Miss, AttrFastResult::Hit)
                        } else {
                            AttrFastResult::Miss
                        }
                    } else {
                        AttrFastResult::Miss
                    }
                }
                Some(AttrCacheEntry::ClassAttr(entry)) => {
                    // Check: object is a PyInstance, same class, no instance
                    // shadow, class not mutated, and the global class-mutation
                    // epoch unchanged (catches base-class mutations that don't
                    // bump the leaf class version).
                    if let Some(inst_rc) = regs[obj as usize].as_py_instance_rc() {
                        let inst = inst_rc.borrow();
                        let name_opt = code.names.get(name_idx as usize).map(|s| s.as_str());
                        let same_class = Rc::as_ptr(&inst.class) == entry.class_ptr.as_ptr();
                        // If name_idx is somehow out of range (bytecode
                        // invariant violation), treat as a shadow present —
                        // forces slow path.
                        let no_shadow = name_opt.is_some_and(|n| !inst.attrs.contains_key(n));
                        let stamp_ok = pyrust_core::class_cache_stamp_matches(
                            inst.class.borrow().mutation_version.get(),
                            entry.class_version,
                            entry.epoch,
                        );
                        if same_class && no_shadow && stamp_ok {
                            // Rebind the unbound class attr to this instance —
                            // same logic as get_attr's regular path, but avoids
                            // the MRO walk.
                            let inst_rc_clone = Rc::clone(inst_rc);
                            let class_rc = Rc::clone(&inst.class);
                            drop(inst);
                            enum Tag {
                                Regular(std::rc::Rc<pyrust_core::UserFunction>),
                                ClassMethod(std::rc::Rc<pyrust_core::UserFunction>),
                                StaticMethod(std::rc::Rc<pyrust_core::UserFunction>),
                                Builtin(Value),
                                Other(Value),
                                Dead,
                            }
                            // Upgrade regular Python functions directly to
                            // their Rc backing. Reconstructing a temporary
                            // opaque Value here would allocate once per
                            // successful attribute-cache hit, only to unwrap it
                            // immediately while binding.
                            let unbound = &entry.value;
                            let tag = match unbound.upgrade_user_function() {
                                Some(function) => match function.kind {
                                    UserFunctionKind::Regular => Tag::Regular(function),
                                    UserFunctionKind::ClassMethod => Tag::ClassMethod(function),
                                    UserFunctionKind::StaticMethod => Tag::StaticMethod(function),
                                    UserFunctionKind::Builtin(_) => {
                                        unbound.upgrade().map(Tag::Builtin).unwrap_or(Tag::Dead)
                                    }
                                },
                                None => unbound.upgrade().map(Tag::Other).unwrap_or(Tag::Dead),
                            };
                            let bound = match tag {
                                Tag::Regular(f) => Some(Value::bound_method(f, inst_rc_clone)),
                                Tag::ClassMethod(f) => Some(Value::class_bound_method(f, class_rc)),
                                Tag::StaticMethod(f) => Some({
                                    if let Some(inner) = f.wrapped_func.as_ref() {
                                        Value::user_function(Rc::clone(inner))
                                    } else {
                                        Value::with_function_kind(
                                            f,
                                            pyrust_core::UserFunctionKind::Regular,
                                        )
                                    }
                                }),
                                Tag::Builtin(unbound) => {
                                    bind_builtin_attribute(unbound, inst_rc_clone)
                                }
                                Tag::Other(unbound) => Some(unbound),
                                Tag::Dead => None,
                            };
                            bound.map_or(AttrFastResult::Miss, AttrFastResult::Hit)
                        } else {
                            AttrFastResult::Miss
                        }
                    } else {
                        AttrFastResult::Miss
                    }
                }
                _ => AttrFastResult::Miss,
            }
        };
        match fast {
            AttrFastResult::Hit(result) => {
                regs[dst as usize] = result;
            }
            AttrFastResult::Property(fget, receiver) => {
                let result = self.call_user_function_expanded(
                    fget,
                    &[ExpandedCallArg {
                        name: None,
                        value: receiver,
                    }],
                    &[],
                )?;
                regs[dst as usize] = result;
            }
            AttrFastResult::Miss => {
                let name = code.names.get(name_idx as usize).ok_or_else(|| {
                    PyError::runtime(format!(
                        "bytecode error: name index {} out of range (pool size {})",
                        name_idx,
                        code.names.len()
                    ))
                })?;
                let obj_val = vm_read(regs, obj, num_locals)?;
                let result = self.get_attr(&obj_val, name)?;
                regs[dst as usize] = result;
                // Fill the cache after the slow path.  `fill_get_attr_cache` is
                // `#[inline(always)]`, so this is byte-identical to the old
                // inline `vm.rs` fill — a `#[cold]` out-of-line split was
                // measured to *regress* the bound-method hot path by ~6% (it
                // perturbed LLVM's codegen of the hot ClassAttr arm).
                fill_get_attr_cache(code, pc, name, &obj_val);
            }
        }
        Ok(())
    }
}
