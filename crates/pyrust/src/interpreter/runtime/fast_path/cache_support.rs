/// Cache-fill for the `GetAttr` inline cache, called from `exec_get_attr`'s
/// slow-path Miss arm. `#[inline(always)]` keeps cache maintenance adjacent to
/// the hot opcode path (a `#[cold]` out-of-line split was measured to regress
/// bound-method calls). Fills instance/class/native-classmethod shapes,
/// deopts polymorphic or unsupported class-descriptor sites to `Megamorphic`,
/// and resets stale same-class entries for revalidation.
#[inline(always)]
fn fill_get_attr_cache(code: &crate::bytecode::FnCode, pc: usize, name: &str, obj_val: &Value) {
    use crate::bytecode::{
        AttrCacheEntry, NativeClassMethodCacheEntry, SlotAttrCacheEntry, is_attr_cache_site,
    };
    let site_pc = pc - 1;
    // Once a site deopts, generic lookup is authoritative forever. Avoid
    // recomputing descriptor policy after every successful slow-path read.
    if matches!(
        code.attr_cache.borrow().get(site_pc),
        Some(AttrCacheEntry::Megamorphic | AttrCacheEntry::Uncacheable)
    ) {
        return;
    }
    let plan = read_attribute_cache_plan(obj_val, name);
    let mut cache = code.attr_cache.borrow_mut();
    let new_entry = match cache.get(site_pc) {
        Some(AttrCacheEntry::Megamorphic | AttrCacheEntry::Uncacheable) => return,
        Some(AttrCacheEntry::ClassAttr(entry)) => {
            if let Some(inst_rc) = obj_val.as_py_instance_rc() {
                let new_ptr = Rc::as_ptr(&inst_rc.borrow().class);
                if new_ptr != entry.class_ptr.as_ptr() {
                    AttrCacheEntry::Megamorphic
                } else {
                    AttrCacheEntry::Empty
                }
            } else if matches!(obj_val.kind(), ValueKind::PyClass(_)) {
                AttrCacheEntry::Megamorphic
            } else {
                return;
            }
        }
        Some(AttrCacheEntry::InstanceAttr {
            class_ptr: existing_ptr,
            ..
        }) => {
            if let Some(inst_rc) = obj_val.as_py_instance_rc() {
                let new_ptr = Rc::as_ptr(&inst_rc.borrow().class);
                if new_ptr != existing_ptr.as_ptr() {
                    AttrCacheEntry::Megamorphic
                } else {
                    AttrCacheEntry::Empty
                }
            } else if matches!(obj_val.kind(), ValueKind::PyClass(_)) {
                AttrCacheEntry::Megamorphic
            } else {
                return;
            }
        }
        Some(AttrCacheEntry::SlotAttr(entry)) => {
            if let Some(inst_rc) = obj_val.as_py_instance_rc() {
                let new_ptr = Rc::as_ptr(&inst_rc.borrow().class);
                if new_ptr != entry.class_ptr.as_ptr() {
                    AttrCacheEntry::Megamorphic
                } else {
                    AttrCacheEntry::Empty
                }
            } else if matches!(obj_val.kind(), ValueKind::PyClass(_)) {
                AttrCacheEntry::Megamorphic
            } else {
                return;
            }
        }
        Some(AttrCacheEntry::NativeClassMethod(entry)) => {
            if let ValueKind::PyClass(class) = obj_val.kind() {
                if Rc::as_ptr(class) != entry.class_ptr.as_ptr() {
                    AttrCacheEntry::Megamorphic
                } else {
                    AttrCacheEntry::Empty
                }
            } else if obj_val.as_py_instance_rc().is_some() {
                AttrCacheEntry::Megamorphic
            } else {
                return;
            }
        }
        Some(AttrCacheEntry::SetInstanceAttr { .. }) => {
            // A SetAttr-only entry should never appear at a GetAttr site; if it
            // somehow does, drop it.
            AttrCacheEntry::Empty
        }
        None | Some(AttrCacheEntry::Empty) => {
            if matches!(&plan, ReadAttributeCachePlan::Uncacheable)
                && matches!(obj_val.kind(), ValueKind::PyClass(_))
            {
                // Ordinary Python descriptors on class objects remain fully
                // dynamic. Deopt the bytecode site after its first
                // classification so this policy does not add a second MRO
                // lookup to every subsequent generic attribute read.
                AttrCacheEntry::Megamorphic
            } else if let ReadAttributeCachePlan::NativeClassMethod(plan) = plan {
                let ValueKind::PyClass(class) = obj_val.kind() else {
                    return;
                };
                let Some((class_version, epoch)) =
                    pyrust_core::class_cache_stamp(class.borrow().mutation_version.get())
                else {
                    return;
                };
                AttrCacheEntry::NativeClassMethod(Box::new(NativeClassMethodCacheEntry {
                    class_ptr: Rc::downgrade(class),
                    class_version,
                    epoch,
                    plan,
                }))
            } else {
                let Some(inst_rc) = obj_val.as_py_instance_rc() else {
                    return;
                };
                let inst = inst_rc.borrow();
                let class_ptr = Rc::downgrade(&inst.class);
                let Some((class_version, epoch)) =
                    pyrust_core::class_cache_stamp(inst.class.borrow().mutation_version.get())
                else {
                    return;
                };
                match plan {
                    ReadAttributeCachePlan::Uncacheable => return,
                    ReadAttributeCachePlan::Instance => AttrCacheEntry::InstanceAttr {
                        class_ptr,
                        class_version,
                        epoch,
                    },
                    ReadAttributeCachePlan::Slot(slot_id) => {
                        AttrCacheEntry::SlotAttr(Box::new(SlotAttrCacheEntry {
                            class_ptr,
                            class_version,
                            epoch,
                            slot_id,
                        }))
                    }
                    ReadAttributeCachePlan::Class(value) => {
                        AttrCacheEntry::class_attr(&inst.class, class_version, epoch, &value)
                    }
                    ReadAttributeCachePlan::NativeClassMethod(_) => {
                        unreachable!("native classmethod plans handled before instance cache fill")
                    }
                }
            }
        }
    };
    cache.set_or_init(
        &code.insns,
        site_pc,
        is_attr_cache_site,
        || AttrCacheEntry::Empty,
        new_entry,
    );
}
