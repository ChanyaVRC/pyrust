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
            PyError::Runtime(format!(
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
        let mut handled = false;
        {
            let cache = code.attr_cache.borrow();
            if let AttrCacheEntry::SetInstanceAttr {
                class_ptr,
                class_version,
                epoch,
            } = &cache[pc - 1]
                && let Some(inst_rc) = obj_val.as_py_instance_rc()
            {
                let (same_class, current_class_version) = {
                    let inst = inst_rc.borrow();
                    (
                        Rc::as_ptr(&inst.class) == class_ptr.as_ptr(),
                        inst.class.borrow().mutation_version.get(),
                    )
                };
                if same_class
                    && pyrust_core::class_cache_stamp_matches(
                        current_class_version,
                        *class_version,
                        *epoch,
                    )
                {
                    inst_rc.borrow_mut().attrs.insert(name, val_val.clone());
                    handled = true;
                }
            }
        }
        if !handled {
            self.assign_attr(obj_val.clone(), name, val_val)?;
            // Fill / update the cache after the slow path.
            let mut cache = code.attr_cache.borrow_mut();
            match &cache[pc - 1] {
                AttrCacheEntry::Megamorphic => {}
                AttrCacheEntry::SetInstanceAttr {
                    class_ptr: existing_ptr,
                    ..
                } => {
                    if let Some(inst_rc) = obj_val.as_py_instance_rc() {
                        let new_ptr = Rc::as_ptr(&inst_rc.borrow().class);
                        if new_ptr != existing_ptr.as_ptr() {
                            cache[pc - 1] = AttrCacheEntry::Megamorphic;
                        } else {
                            cache[pc - 1] = AttrCacheEntry::Empty;
                        }
                    }
                }
                AttrCacheEntry::Empty => {
                    if let Some(class) = write_attribute_cache_class(&obj_val, name) {
                        let Some((class_version, epoch)) =
                            pyrust_core::class_cache_stamp(class.borrow().mutation_version.get())
                        else {
                            return Ok(());
                        };
                        cache[pc - 1] = AttrCacheEntry::SetInstanceAttr {
                            class_ptr: Rc::downgrade(&class),
                            class_version,
                            epoch,
                        };
                    }
                }
                // ClassAttr / InstanceAttr are GetAttr-only entries; a
                // SetAttr site never produces them.
                _ => {}
            }
        }
        Ok(())
    }
}
