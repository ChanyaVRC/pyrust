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

impl Interpreter {
    /// Full `Insn::BinOp` body: the unconditional int–int fast path followed by
    /// the adaptive float/str inline cache (Empty → Counting → Specialized, with
    /// Megamorphic deopt on a type mismatch).  See the original dispatch-arm
    /// comments inlined below for the per-state rationale.
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
        use crate::bytecode::{BINOP_SPEC_THRESHOLD, BinOpCacheEntry, BinopTypeTag};
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
        let cache_entry = code.binop_cache.borrow()[cache_slot];
        match cache_entry {
            BinOpCacheEntry::Megamorphic => {
                // Permanently polymorphic site: skip classification, go straight
                // to eval_binary.
                let l = vm_read(regs, lhs, num_locals)?;
                let r = vm_read(regs, rhs, num_locals)?;
                regs[dst as usize] = self.eval_binary(l, op, r)?;
            }
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
                } else {
                    // Actual type mismatch: deopt to Megamorphic.
                    code.binop_cache.borrow_mut()[cache_slot] = BinOpCacheEntry::Megamorphic;
                }
                let l = vm_read(regs, lhs, num_locals)?;
                let r = vm_read(regs, rhs, num_locals)?;
                regs[dst as usize] = self.eval_binary(l, op, r)?;
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
                } else {
                    // Actual type mismatch: deopt to Megamorphic.
                    code.binop_cache.borrow_mut()[cache_slot] = BinOpCacheEntry::Megamorphic;
                }
                let l = vm_read(regs, lhs, num_locals)?;
                let r = vm_read(regs, rhs, num_locals)?;
                regs[dst as usize] = self.eval_binary(l, op, r)?;
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
                } else {
                    // Type mismatch: deopt.
                    code.binop_cache.borrow_mut()[cache_slot] = BinOpCacheEntry::Megamorphic;
                }
                let l = vm_read(regs, lhs, num_locals)?;
                let r = vm_read(regs, rhs, num_locals)?;
                regs[dst as usize] = self.eval_binary(l, op, r)?;
            }
            BinOpCacheEntry::Specialized(BinopTypeTag::Int) => {
                // int_int_fast already ran above and returned None (overflow or
                // unsupported op such as FloorDiv/Mod/Div/Pow).  The types are
                // still correct; keep Specialized so the fast path is retried
                // next loop iteration.  Fall through to eval_binary for this
                // invocation.
                let l = vm_read(regs, lhs, num_locals)?;
                let r = vm_read(regs, rhs, num_locals)?;
                regs[dst as usize] = self.eval_binary(l, op, r)?;
            }
            BinOpCacheEntry::Specialized(BinopTypeTag::Other) => {
                // No fast path for 'Other' types.  If the types are still
                // 'Other', stay Specialized (no deopt needed since there's no
                // fast path to lose).  Fall through.
                let l = vm_read(regs, lhs, num_locals)?;
                let r = vm_read(regs, rhs, num_locals)?;
                regs[dst as usize] = self.eval_binary(l, op, r)?;
            }
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
                code.binop_cache.borrow_mut()[cache_slot] = new_entry;
                let l = vm_read(regs, lhs, num_locals)?;
                let r = vm_read(regs, rhs, num_locals)?;
                regs[dst as usize] = self.eval_binary(l, op, r)?;
            }
            BinOpCacheEntry::Empty => {
                let observed = classify_binop_tag(&regs[lhs as usize], &regs[rhs as usize]);
                code.binop_cache.borrow_mut()[cache_slot] = BinOpCacheEntry::Counting {
                    tag: observed,
                    count: 1,
                };
                let l = vm_read(regs, lhs, num_locals)?;
                let r = vm_read(regs, rhs, num_locals)?;
                regs[dst as usize] = self.eval_binary(l, op, r)?;
            }
        }
        Ok(())
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
            Miss,
        }
        let fast = {
            let cache = code.attr_cache.borrow();
            match &cache[pc - 1] {
                // Instance-attribute read cache (#1912): the name has no
                // data-descriptor shadow on the class, so the instance __dict__
                // takes priority.  Probe it directly, skipping the MRO walk in
                // get_attr_instance_raw.
                AttrCacheEntry::InstanceAttr {
                    class_ptr,
                    class_version,
                    epoch,
                } => {
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
                AttrCacheEntry::SlotAttr {
                    class_ptr,
                    class_version,
                    epoch,
                    slot_id,
                } => {
                    if let Some(inst_rc) = regs[obj as usize].as_py_instance_rc() {
                        let inst = inst_rc.borrow();
                        let same_class = Rc::as_ptr(&inst.class) == class_ptr.as_ptr();
                        let stamp_ok = pyrust_core::class_cache_stamp_matches(
                            inst.class.borrow().mutation_version.get(),
                            *class_version,
                            *epoch,
                        );
                        if same_class && stamp_ok {
                            match inst.attrs.get_member_slot(*slot_id) {
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
                            bind_cached_native_class_method(plan, Rc::clone(class))
                                .map_or(AttrFastResult::Miss, AttrFastResult::Hit)
                        } else {
                            AttrFastResult::Miss
                        }
                    } else {
                        AttrFastResult::Miss
                    }
                }
                AttrCacheEntry::ClassAttr {
                    class_ptr,
                    class_version,
                    epoch,
                    value: unbound,
                } => {
                    // Check: object is a PyInstance, same class, no instance
                    // shadow, class not mutated, and the global class-mutation
                    // epoch unchanged (catches base-class mutations that don't
                    // bump the leaf class version).
                    if let Some(inst_rc) = regs[obj as usize].as_py_instance_rc() {
                        let inst = inst_rc.borrow();
                        let name_opt = code.names.get(name_idx as usize).map(|s| s.as_str());
                        let same_class = Rc::as_ptr(&inst.class) == class_ptr.as_ptr();
                        // If name_idx is somehow out of range (bytecode
                        // invariant violation), treat as a shadow present —
                        // forces slow path.
                        let no_shadow = name_opt.is_some_and(|n| !inst.attrs.contains_key(n));
                        let stamp_ok = pyrust_core::class_cache_stamp_matches(
                            inst.class.borrow().mutation_version.get(),
                            *class_version,
                            *epoch,
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
            AttrFastResult::Miss => {
                let name = code.names.get(name_idx as usize).ok_or_else(|| {
                    PyError::Runtime(format!(
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
