// Shared descriptor invocation support.
/// Handles both `property` (BuiltinObject with fget) and user-defined
/// descriptors (PyInstance with a class `__get__` method).
fn call_descriptor_get(
    interp: &mut Interpreter,
    descriptor: &Value,
    instance: Value,
    owner: Value,
    // Retained for call-site clarity; messages use the property's __set_name__
    // name (`prop_name`) rather than the lookup name.
    _attr_name: &str,
) -> Result<Value> {
    // Unbound `super(cls)` descriptor (#2704): binding it to the instance
    // yields `super(cls, instance)` (mirroring CPython's `super_descr_get`).
    // Class-level access (`instance is None`) leaves it unchanged.  `owner` is
    // unused here because supercheck binds against the instance's own type.
    if let ValueKind::SuperProxyUnbound { class } = descriptor.kind() {
        let class = Rc::clone(class);
        let _ = &owner;
        return interp.bind_unbound_super(class, instance);
    }
    // `__slots__` member_descriptor: read the instance's slot storage; an unset
    // slot raises AttributeError (issue #2084).  Class-level access (`S.x`)
    // never reaches here — get_attr_class returns the descriptor itself.
    if let Some(info) = pyrust_builtins::member_descriptor::as_member_descriptor_full(descriptor) {
        if instance.is_none() {
            return Ok(
                pyrust_builtins::member_descriptor::export_member_descriptor(descriptor)
                    .unwrap_or_else(|| descriptor.clone()),
            );
        }
        member_descriptor_check_receiver(
            &info.attr_name,
            &info.owner_name,
            info.owner.as_ref(),
            &instance,
        )?;
        return member_descriptor_get(&instance, info.slot_id, &info.attr_name);
    }
    // property special-case: use the stored fget directly.
    if let Some((fget, partial_slot, prop_name)) =
        pyrust_builtins::property::with_property(descriptor, |s| {
            (Rc::clone(&s.fget), s.partial_slot, s.name.clone())
        })
        && partial_slot.is_none()
    {
        // Class-level access (`instance is None`, e.g. `super(B, B).prop`):
        // CPython's `property.__get__(None, owner)` returns the property
        // itself rather than invoking the getter, mirroring `B.prop`.
        if instance.is_none() {
            return Ok(descriptor.clone());
        }
        return if fget.is_none() {
            // CPython 3.12: `property 'x' of 'C' object has no getter` (issue #1845).
            // The name comes from __set_name__ (issue #1846); anonymous
            // properties (never bound in a class body) use the unnamed form.
            let owner = value_type_name_str(&instance);
            let prop_desc = property_description(interp, prop_name.as_ref())?;
            Err(pyrust_core::py_err!(
                "AttributeError",
                "{prop_desc} of '{owner}' object has no getter"
            ))
        } else {
            let getter = (*fget).clone();
            interp.call_function_expanded(
                getter,
                &[ExpandedCallArg {
                    name: None,
                    value: instance,
                }],
            )
        };
    }
    // General user-defined descriptor: look up __get__ on the descriptor's class.
    if let ValueKind::PyInstance(inst) = descriptor.kind() {
        let desc_class = Rc::clone(&inst.borrow().class);
        if let Some(get_fn) = lookup_class_attr(&desc_class, "__get__") {
            return invoke_class_method(
                interp,
                get_fn,
                descriptor.clone(),
                &[
                    ExpandedCallArg {
                        name: None,
                        value: instance,
                    },
                    ExpandedCallArg {
                        name: None,
                        value: owner,
                    },
                ],
            );
        }
    }
    // Fallback: return the descriptor itself (shouldn't happen if callers
    // check is_data_descriptor / is_non_data_descriptor first, but be safe).
    Ok(descriptor.clone())
}

/// Try to call `descriptor.__set__(instance, value)` for a data descriptor.
///
/// Returns `Some(Ok(()))` if the descriptor handled the set,
/// `Some(Err(_))` if it raised, or `None` if the class attribute is not a
/// data descriptor (caller should fall through to instance dict write).
fn call_descriptor_set(
    interp: &mut Interpreter,
    class_val: &Value,
    instance: Value,
    value: Value,
    // Retained for call-site clarity; messages use the property's __set_name__
    // name (`prop_name`) rather than the lookup name.
    _attr_name: &str,
) -> Result<Option<Result<()>>> {
    // `__slots__` member_descriptor: store into the instance's slot storage
    // (issue #2084).
    if let Some(info) = pyrust_builtins::member_descriptor::as_member_descriptor_full(class_val) {
        member_descriptor_check_receiver(
            &info.attr_name,
            &info.owner_name,
            info.owner.as_ref(),
            &instance,
        )?;
        if let ValueKind::PyInstance(inst) = instance.kind() {
            inst.borrow_mut()
                .attrs
                .insert_member_slot(info.slot_id, value);
            return Ok(Some(Ok(())));
        }
        return Ok(Some(Ok(())));
    }
    // property special-case.
    if let Some((fset, partial_slot, prop_name)) =
        pyrust_builtins::property::with_property(class_val, |s| {
            (Rc::clone(&s.fset), s.partial_slot, s.name.clone())
        })
        && partial_slot.is_none()
    {
        return Ok(Some(if fset.is_none() {
            // CPython 3.12: `property 'x' of 'C' object has no setter` (issue #1845).
            // The name comes from __set_name__ (issue #1846); anonymous
            // properties (never bound in a class body) use the unnamed form.
            let owner = value_type_name_str(&instance);
            let prop_desc = property_description(interp, prop_name.as_ref())?;
            Err(pyrust_core::py_err!(
                "AttributeError",
                "{prop_desc} of '{owner}' object has no setter"
            ))
        } else {
            let setter = (*fset).clone();
            interp.call_function_expanded(
                setter,
                &[
                    ExpandedCallArg {
                        name: None,
                        value: instance,
                    },
                    ExpandedCallArg { name: None, value },
                ],
            )?;
            Ok(())
        }));
    }
    // General user-defined data descriptor: look up __set__ on the descriptor's class.
    if let ValueKind::PyInstance(inst) = class_val.kind() {
        let desc_class = Rc::clone(&inst.borrow().class);
        if let Some(set_fn) = lookup_class_attr(&desc_class, "__set__") {
            let result = invoke_class_method(
                interp,
                set_fn,
                class_val.clone(),
                &[
                    ExpandedCallArg {
                        name: None,
                        value: instance,
                    },
                    ExpandedCallArg { name: None, value },
                ],
            );
            return Ok(Some(result.map(|_| ())));
        }
        // CPython: a descriptor with __delete__ but no __set__ is still a data
        // descriptor and blocks assignment.  Raise AttributeError: __set__
        // (CPython's exact message) rather than falling through to instance dict.
        if lookup_class_attr(&desc_class, "__delete__").is_some() {
            return Ok(Some(Err(pyrust_core::py_err!("AttributeError", "__set__"))));
        }
    }
    Ok(None)
}

/// Try to call `descriptor.__delete__(instance)` for a data descriptor.
///
/// Returns `Some(Ok(()))` if handled, `Some(Err(_))` if it raised, or
/// `None` if no `__delete__` is found (caller falls through to instance
/// dict removal).
fn call_descriptor_delete(
    interp: &mut Interpreter,
    class_val: &Value,
    instance: Value,
    // Retained for call-site clarity; messages use the property's __set_name__
    // name (`prop_name`) rather than the lookup name.
    _attr_name: &str,
) -> Result<Option<Result<()>>> {
    // `__slots__` member_descriptor: clear the instance's slot storage; an
    // already-unset slot raises AttributeError (issue #2084).
    if let Some(info) = pyrust_builtins::member_descriptor::as_member_descriptor_full(class_val) {
        member_descriptor_check_receiver(
            &info.attr_name,
            &info.owner_name,
            info.owner.as_ref(),
            &instance,
        )?;
        if let ValueKind::PyInstance(inst) = instance.kind() {
            let removed = inst
                .borrow_mut()
                .attrs
                .shift_remove_member_slot(info.slot_id)
                .is_some();
            if !removed {
                // CPython's `member_delete` raises AttributeError with just the
                // slot name as the message (not the full "'C' object has no
                // attribute 'x'" form), issue #2084.
                let slot = &info.attr_name;
                return Ok(Some(Err(pyrust_core::py_err!("AttributeError", "{slot}"))));
            }
        }
        return Ok(Some(Ok(())));
    }
    // property special-case.
    if let Some((fdel, partial_slot, prop_name)) =
        pyrust_builtins::property::with_property(class_val, |s| {
            (Rc::clone(&s.fdel), s.partial_slot, s.name.clone())
        })
        && partial_slot.is_none()
    {
        return Ok(Some(if fdel.is_none() {
            // CPython 3.12: `property 'x' of 'C' object has no deleter` (issue #1845).
            // The name comes from __set_name__ (issue #1846); anonymous
            // properties (never bound in a class body) use the unnamed form.
            let owner = value_type_name_str(&instance);
            let prop_desc = property_description(interp, prop_name.as_ref())?;
            Err(pyrust_core::py_err!(
                "AttributeError",
                "{prop_desc} of '{owner}' object has no deleter"
            ))
        } else {
            let deleter = (*fdel).clone();
            interp.call_function_expanded(
                deleter,
                &[ExpandedCallArg {
                    name: None,
                    value: instance,
                }],
            )?;
            Ok(())
        }));
    }
    // General user-defined data descriptor: look up __delete__ on the descriptor's class.
    if let ValueKind::PyInstance(inst) = class_val.kind() {
        let desc_class = Rc::clone(&inst.borrow().class);
        if let Some(del_fn) = lookup_class_attr(&desc_class, "__delete__") {
            let result = invoke_class_method(
                interp,
                del_fn,
                class_val.clone(),
                &[ExpandedCallArg {
                    name: None,
                    value: instance,
                }],
            );
            return Ok(Some(result.map(|_| ())));
        }
    }
    Ok(None)
}

/// Compute the MRO (method resolution order) for a class using C3 linearization.
///
/// Implements the C3 superclass linearization algorithm as used by CPython:
///
///   L[C(B1, B2, ...)] = C + merge(L[B1], L[B2], ..., [B1, B2, ...])
///
/// The merge operation repeatedly selects the head of the first list whose
/// head does not appear in the tail of any other list.  If no such head
/// exists the bases are inconsistent and a TypeError is returned.
///
/// Used by both `__mro__` (returns a tuple) and `mro()` (returns a list).
pub(super) fn class_mro_items(class: &Rc<RefCell<PyClass>>) -> Result<Vec<Value>> {
    /// Compute L[c] recursively.  Returns a `Vec` of class pointers in MRO
    /// order; the first element is always `c` itself.
    fn c3_linearize(
        c: &Rc<RefCell<PyClass>>,
        obj_ptr: *const RefCell<PyClass>,
    ) -> Result<Vec<Rc<RefCell<PyClass>>>> {
        let (base, extra_bases) = {
            let borrowed = c.borrow();
            (borrowed.base.clone(), borrowed.extra_bases.clone())
        };

        // Collect all direct bases in declaration order.
        let mut all_bases: Vec<Rc<RefCell<PyClass>>> = Vec::new();
        if let Some(ref b) = base {
            all_bases.push(Rc::clone(b));
        }
        for eb in &extra_bases {
            all_bases.push(Rc::clone(eb));
        }

        if all_bases.is_empty() {
            // No explicit bases: just [c].  The object singleton will be
            // appended by the outer function after the merge.
            return Ok(vec![Rc::clone(c)]);
        }

        // Build the lists to merge: L[B1], L[B2], ..., [B1, B2, ...]
        let mut lists: Vec<Vec<Rc<RefCell<PyClass>>>> = Vec::new();
        for b in &all_bases {
            lists.push(c3_linearize(b, obj_ptr)?);
        }
        // The final list is the sequence of direct bases.
        lists.push(all_bases.clone());

        // C3 merge.
        let mut result: Vec<Rc<RefCell<PyClass>>> = vec![Rc::clone(c)];
        loop {
            // Remove all empty lists.
            lists.retain(|l| !l.is_empty());
            if lists.is_empty() {
                break;
            }

            // Find a good head: first element of some list that does not
            // appear in the tail of any other list.
            //
            // `object` is deferred: it is the universal root, so it must never
            // be chosen while any non-`object` head is still available.  pyrust
            // represents the implicit `object` base inconsistently — primitive
            // classes carry an explicit `object` base (so their inner
            // linearization ends in `object`) while base-less user classes do
            // not — which can otherwise let `object` win ahead of a sibling
            // user base when mixing the two (`class E(int, UserBase)` would
            // linearize as `[E, int, object, UserBase]` instead of the correct
            // `[E, int, UserBase, object]`).  Deferring `object` restores the
            // correct order without forcing `object` into every inner list
            // (which would create spurious conflicts for the abc-`extra_bases`
            // carried by `dict`/`list`/… — issue #2611).
            let mut chosen: Option<Rc<RefCell<PyClass>>> = None;
            let mut deferred_object: Option<Rc<RefCell<PyClass>>> = None;
            'outer: for list in &lists {
                let head_ptr = Rc::as_ptr(&list[0]);
                // Check that head_ptr does not appear in the tail of any list.
                for other in &lists {
                    for tail_item in other.iter().skip(1) {
                        if Rc::as_ptr(tail_item) == head_ptr {
                            continue 'outer;
                        }
                    }
                }
                if head_ptr == obj_ptr {
                    // Valid head, but defer it in case a non-object head exists.
                    deferred_object = Some(Rc::clone(&list[0]));
                    continue;
                }
                chosen = Some(Rc::clone(&list[0]));
                break;
            }
            // No non-object head was found: fall back to a deferred `object`.
            if chosen.is_none() {
                chosen = deferred_object;
            }

            let chosen = match chosen {
                Some(c) => c,
                None => {
                    // No consistent linearization exists.
                    // Collect base names for the error message (skip object).
                    let base_names: Vec<String> = all_bases
                        .iter()
                        .filter(|b| Rc::as_ptr(b) != obj_ptr)
                        .map(|b| b.borrow().name.clone())
                        .collect();
                    let bases_str = base_names.join(", ");
                    return Err(pyrust_core::type_err!(
                        "Cannot create a consistent method resolution\norder (MRO) for bases {bases_str}"
                    ));
                }
            };

            let chosen_ptr = Rc::as_ptr(&chosen);
            result.push(chosen);
            // Remove chosen from the front of every list where it appears.
            for list in &mut lists {
                if !list.is_empty() && Rc::as_ptr(&list[0]) == chosen_ptr {
                    list.remove(0);
                }
            }
        }

        Ok(result)
    }

    let obj = object_class_singleton();
    let obj_ptr = Rc::as_ptr(&obj);
    let mut mro = c3_linearize(class, obj_ptr)?;

    // Append the `object` singleton if it is not already present.
    if !mro.iter().any(|c| Rc::as_ptr(c) == obj_ptr) {
        mro.push(obj);
    }

    Ok(mro.into_iter().map(Value::py_class).collect())
}

/// Index in `mro` at which a `super()` lookup should begin: the entry *after*
/// `class` (found by pointer identity), or `0` if `class` is not present.
///
/// Shared by the instance and classmethod `super()` paths, which both walk the
/// receiver's full MRO from the position following the defining class
/// (cooperative multiple inheritance).
fn mro_search_start(mro: &[Value], class: &Rc<RefCell<PyClass>>) -> usize {
    let class_ptr = Rc::as_ptr(class);
    mro.iter()
        .position(|v| match v.kind() {
            ValueKind::PyClass(c) => Rc::as_ptr(c) == class_ptr,
            _ => false,
        })
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// Returns the list of direct subclasses of `class`, pruning stale weak refs.
/// Used by `__subclasses__()` dispatch (issue #1354).
pub(super) fn class_direct_subclasses(class: &Rc<RefCell<PyClass>>) -> Vec<Value> {
    let borrowed = class.borrow();
    let mut subclasses = borrowed.subclasses.borrow_mut();
    // Retain only live weak refs and collect as Values.
    let mut result = Vec::new();
    subclasses.retain(|weak| {
        if let Some(rc) = weak.upgrade() {
            result.push(Value::py_class(rc));
            true
        } else {
            false
        }
    });
    result
}
