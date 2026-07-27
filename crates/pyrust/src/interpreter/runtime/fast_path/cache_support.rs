/// Cache-fill for the `GetAttr` inline cache, called from `exec_get_attr`'s
/// slow-path Miss arm. `#[inline(always)]` keeps cache maintenance adjacent to
/// the hot opcode path (a `#[cold]` out-of-line split was measured to regress
/// bound-method calls). Fills instance/class/native-classmethod shapes,
/// deopts polymorphic or unsupported class-descriptor sites to `Megamorphic`,
/// and resets stale same-class entries for revalidation.
#[inline(always)]
fn fill_get_attr_cache(code: &crate::bytecode::FnCode, pc: usize, name: &str, obj_val: &Value) {
    use crate::bytecode::AttrCacheEntry;
    // Once a site deopts, generic lookup is authoritative forever. Avoid
    // recomputing descriptor policy after every successful slow-path read.
    if matches!(
        code.attr_cache.borrow()[pc - 1],
        AttrCacheEntry::Megamorphic | AttrCacheEntry::Uncacheable
    ) {
        return;
    }
    let plan = read_attribute_cache_plan(obj_val, name);
    let mut cache = code.attr_cache.borrow_mut();
    match &cache[pc - 1] {
        AttrCacheEntry::Megamorphic | AttrCacheEntry::Uncacheable => {}
        AttrCacheEntry::ClassAttr {
            class_ptr: existing_ptr,
            ..
        }
        | AttrCacheEntry::InstanceAttr {
            class_ptr: existing_ptr,
            ..
        }
        | AttrCacheEntry::SlotAttr {
            class_ptr: existing_ptr,
            ..
        } => {
            if let Some(inst_rc) = obj_val.as_py_instance_rc() {
                let new_ptr = Rc::as_ptr(&inst_rc.borrow().class);
                if new_ptr != existing_ptr.as_ptr() {
                    // Different class at this call site — go megamorphic.
                    cache[pc - 1] = AttrCacheEntry::Megamorphic;
                } else {
                    // Same class but version changed, or the resolution flipped
                    // between instance/class attr (e.g. an instance attr was
                    // deleted and now resolves to a method).  Reset to Empty so
                    // the next slow-path execution refills.
                    cache[pc - 1] = AttrCacheEntry::Empty;
                }
            } else if matches!(obj_val.kind(), ValueKind::PyClass(_)) {
                cache[pc - 1] = AttrCacheEntry::Megamorphic;
            }
        }
        AttrCacheEntry::NativeClassMethod {
            class_ptr: existing_ptr,
            ..
        } => {
            if let ValueKind::PyClass(class) = obj_val.kind() {
                if Rc::as_ptr(class) != existing_ptr.as_ptr() {
                    cache[pc - 1] = AttrCacheEntry::Megamorphic;
                } else {
                    // The same target was mutated, or an ancestor/metaclass
                    // changed. Refill only after the slow path has revalidated
                    // descriptor ownership and precedence.
                    cache[pc - 1] = AttrCacheEntry::Empty;
                }
            } else if obj_val.as_py_instance_rc().is_some() {
                cache[pc - 1] = AttrCacheEntry::Megamorphic;
            }
        }
        AttrCacheEntry::SetInstanceAttr { .. } => {
            // A SetAttr-only entry should never appear at a GetAttr site; if it
            // somehow does, drop it.
            cache[pc - 1] = AttrCacheEntry::Empty;
        }
        AttrCacheEntry::Empty => {
            if matches!(&plan, ReadAttributeCachePlan::Uncacheable)
                && matches!(obj_val.kind(), ValueKind::PyClass(_))
            {
                // Ordinary Python descriptors on class objects remain fully
                // dynamic. Deopt the bytecode site after its first
                // classification so this policy does not add a second MRO
                // lookup to every subsequent generic attribute read.
                cache[pc - 1] = AttrCacheEntry::Megamorphic;
                return;
            }
            if let ReadAttributeCachePlan::NativeClassMethod(plan) = plan {
                let ValueKind::PyClass(class) = obj_val.kind() else {
                    return;
                };
                let Some((class_version, epoch)) =
                    pyrust_core::class_cache_stamp(class.borrow().mutation_version.get())
                else {
                    return;
                };
                cache[pc - 1] = AttrCacheEntry::NativeClassMethod {
                    class_ptr: Rc::downgrade(class),
                    class_version,
                    epoch,
                    plan,
                };
                return;
            }
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
            cache[pc - 1] = match plan {
                ReadAttributeCachePlan::Uncacheable => AttrCacheEntry::Empty,
                ReadAttributeCachePlan::Instance => AttrCacheEntry::InstanceAttr {
                    class_ptr,
                    class_version,
                    epoch,
                },
                ReadAttributeCachePlan::Slot(slot_id) => AttrCacheEntry::SlotAttr {
                    class_ptr,
                    class_version,
                    epoch,
                    slot_id,
                },
                ReadAttributeCachePlan::Class(value) => {
                    AttrCacheEntry::class_attr(&inst.class, class_version, epoch, &value)
                }
                ReadAttributeCachePlan::NativeClassMethod(_) => {
                    unreachable!("native classmethod plans handled before instance cache fill")
                }
            };
        }
    }
}
