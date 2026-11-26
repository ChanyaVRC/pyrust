impl Interpreter {
    /// Full Insn::SetAttr body: the SetInstanceAttr write-cache hit path
    /// followed by the slow-path assign_attr call and cache maintenance.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn exec_set_attr(
        &mut self,
        regs: &mut RegSlice,
        code: &crate::bytecode::FnCode,
        pc: usize,
        obj: crate::bytecode::Reg,
        name_idx: u16,
        val: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        use crate::bytecode::AttrCacheEntry;
        let obj_val = vm_read(regs, obj, num_locals)?;
        let val_val = vm_read(regs, val, num_locals)?;
        let name = code.names.get(name_idx as usize).ok_or_else(|| {
            PyError::runtime(format!(
                "bytecode error: name index {} out of range (pool size {})",
                name_idx,
                code.names.len()
            ))
        })?;

        // Write inline cache fast path (#1998): a monomorphic site proven to be
        // a plain instance-dict write (no __setattr__ override, no __set__ data
        // descriptor on the MRO, no __slots__ restriction, not __class__/__dict__)
        // writes straight into inst.attrs, skipping the MRO walk in
        // assign_attr_instance.
        let site_pc = pc - 1;
        let mut handled = false;
        {
            let cache = code.attr_cache.borrow();
            match cache.get(site_pc) {
                Some(AttrCacheEntry::SetInstanceAttr {
                    class_ptr,
                    class_version,
                    epoch,
                }) => {
                    if let Some(inst_rc) = obj_val.as_py_instance_rc()
                        && set_cache_guards_hold(inst_rc, class_ptr, *class_version, *epoch)
                    {
                        inst_rc.borrow_mut().attrs.insert(name, val_val.clone());
                        handled = true;
                    }
                }
                // `__slots__` member write (#3378): the cached fact is that
                // `name` resolves to this class's `member_descriptor`, so a
                // hit is exactly `member_descriptor.__set__`.  The displaced
                // value is dropped after the instance borrow is released.
                Some(AttrCacheEntry::SetSlotAttr(entry)) => {
                    if let Some(inst_rc) = obj_val.as_py_instance_rc()
                        && set_cache_guards_hold(
                            inst_rc,
                            &entry.class_ptr,
                            entry.class_version,
                            entry.epoch,
                        )
                    {
                        let old = inst_rc
                            .borrow_mut()
                            .attrs
                            .insert_member_slot(entry.slot_id, val_val.clone());
                        drop(old);
                        handled = true;
                    }
                }
                _ => {}
            }
        }
        if !handled {
            self.assign_attr(obj_val.clone(), name, val_val)?;
            // Fill / update the cache after the slow path.
            let mut cache = code.attr_cache.borrow_mut();
            let new_entry = match cache.get(site_pc) {
                Some(AttrCacheEntry::Megamorphic) => None,
                Some(AttrCacheEntry::SetInstanceAttr {
                    class_ptr: existing_ptr,
                    ..
                }) => obj_val.as_py_instance_rc().map(|inst_rc| {
                    let new_ptr = Rc::as_ptr(&inst_rc.borrow().class);
                    if new_ptr != existing_ptr.as_ptr() {
                        AttrCacheEntry::Megamorphic
                    } else {
                        AttrCacheEntry::Empty
                    }
                }),
                Some(AttrCacheEntry::SetSlotAttr(entry)) => {
                    obj_val.as_py_instance_rc().map(|inst_rc| {
                        let new_ptr = Rc::as_ptr(&inst_rc.borrow().class);
                        if new_ptr != entry.class_ptr.as_ptr() {
                            AttrCacheEntry::Megamorphic
                        } else {
                            AttrCacheEntry::Empty
                        }
                    })
                }
                None | Some(AttrCacheEntry::Empty) => {
                    let (class, slot_id) = match write_attribute_cache_plan(&obj_val, name) {
                        WriteAttributeCachePlan::Uncacheable => (None, None),
                        WriteAttributeCachePlan::Instance(class) => (Some(class), None),
                        WriteAttributeCachePlan::Slot(class, slot_id) => {
                            (Some(class), Some(slot_id))
                        }
                    };
                    if let Some(class) = class {
                        let Some((class_version, epoch)) =
                            pyrust_core::class_cache_stamp(class.borrow().mutation_version.get())
                        else {
                            return Ok(());
                        };
                        let class_ptr = Rc::downgrade(&class);
                        Some(match slot_id {
                            Some(slot_id) => AttrCacheEntry::SetSlotAttr(Box::new(
                                crate::bytecode::SlotAttrCacheEntry {
                                    class_ptr,
                                    class_version,
                                    epoch,
                                    slot_id,
                                },
                            )),
                            None => AttrCacheEntry::SetInstanceAttr {
                                class_ptr,
                                class_version,
                                epoch,
                            },
                        })
                    } else {
                        None
                    }
                }
                // ClassAttr / InstanceAttr are GetAttr-only entries; a
                // SetAttr site never produces them.
                _ => None,
            };
            if let Some(entry) = new_entry {
                cache.set_or_init(
                    &code.insns,
                    site_pc,
                    crate::bytecode::is_attr_cache_site,
                    || AttrCacheEntry::Empty,
                    entry,
                );
            }
        }
        Ok(())
    }
}

/// Shared hit guard for the `SetAttr` write caches: same class allocation, and
/// the class mutation version / global epoch stamp still match.
#[inline(always)]
fn set_cache_guards_hold(
    inst_rc: &Rc<std::cell::RefCell<pyrust_core::PyInstance>>,
    class_ptr: &std::rc::Weak<std::cell::RefCell<pyrust_core::PyClass>>,
    class_version: u64,
    epoch: u64,
) -> bool {
    let inst = inst_rc.borrow();
    Rc::as_ptr(&inst.class) == class_ptr.as_ptr()
        && pyrust_core::class_cache_stamp_matches(
            inst.class.borrow().mutation_version.get(),
            class_version,
            epoch,
        )
}
