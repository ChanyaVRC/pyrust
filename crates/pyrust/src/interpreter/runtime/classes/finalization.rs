impl Interpreter {
    /// Shared class-construction finalization used by both the `type()` and
    /// `type.__new__` constructor builtins.  Mirrors the `class` statement's
    /// post-body path (`exec_make_class`): adjust the namespace per CPython's
    /// `type_new` rules (__module__/__doc__/__hash__/__qualname__), process
    /// `__slots__`, build the `PyClass`, register it as a subclass of every
    /// base, run `__set_name__` on namespace descriptors, then call the base's
    /// `__init_subclass__`.  This gives `type(...)`-created classes the same
    /// hooks as a `class` statement (issues #2129 / #2130).
    pub(crate) fn build_class_via_type(
        &mut self,
        name: String,
        base: Option<Rc<RefCell<PyClass>>>,
        extra_bases: Vec<Rc<RefCell<PyClass>>>,
        mut attrs: IndexMap<String, Value>,
        metatype: Option<Rc<RefCell<PyClass>>>,
        init_subclass_kwargs: &[ExpandedCallArg],
    ) -> Result<Value> {
        let class_docstring = attrs.get("__doc__").and_then(|v| match v.kind() {
            ValueKind::Str(s) => Some(s.to_string()),
            _ => None,
        });
        let qualname =
            make_class_finalize_attrs(&mut attrs, name.clone(), class_docstring.as_deref())?;
        let slots = make_class_extract_slots(&mut attrs)?;
        let class = Rc::new(RefCell::new(PyClass {
            extra_bases: extra_bases.clone(),
            slots,
            metatype,
            ..PyClass::new(name, qualname, base.clone(), attrs)
        }));
        install_slot_member_descriptors(&class);
        class_mro_items(&class).map(|_| ())?;
        if let Some(ref b) = base {
            b.borrow()
                .subclasses
                .borrow_mut()
                .push(Rc::downgrade(&class));
        }
        for eb in &extra_bases {
            eb.borrow()
                .subclasses
                .borrow_mut()
                .push(Rc::downgrade(&class));
        }
        self.make_class_call_set_name(&class)?;
        self.make_class_call_init_subclass_with_kwargs(&class, init_subclass_kwargs)?;
        Ok(Value::py_class(class))
    }
}

/// Seed a class-body register slot with a value if the slot is allocated and
/// in range.  Used by `run_class_body` to pre-inject __qualname__/__module__/
/// __annotations__ before the body runs.
fn seed_class_reg(
    regs: &mut RegsBuf,
    slot: Option<crate::bytecode::Reg>,
    value: impl FnOnce() -> Value,
) {
    if let Some(slot) = slot {
        let slot = slot as usize;
        if slot < regs.len() {
            regs[slot] = value();
        }
    }
}

/// Build the class attrs dict from the body's fastlocal registers, in the
/// runtime store order recorded by RecordClassStore.
fn collect_class_attrs(
    local_index: &HashMap<String, crate::bytecode::Reg>,
    class_regs: &RegsBuf,
    store_order: Vec<crate::bytecode::Reg>,
    num_class_regs: usize,
) -> IndexMap<String, Value> {
    let mut slot_to_name: Vec<Option<&String>> = vec![None; num_class_regs];
    for (name, &slot) in local_index.iter() {
        if (slot as usize) < slot_to_name.len() {
            slot_to_name[slot as usize] = Some(name);
        }
    }
    let mut attrs = IndexMap::new();
    for slot in store_order {
        let Some(name) = slot_to_name.get(slot as usize).and_then(|n| *n) else {
            continue;
        };
        if let Some(v) = class_regs.get(slot as usize)
            && !v.is_unset()
        {
            attrs.insert(name.clone(), v.clone());
        }
    }
    attrs
}

/// Apply CPython's type_new attrs adjustments and return the resolved
/// __qualname__: wrap a bare __init_subclass__ as a classmethod, pop and
/// validate __qualname__, and seed __module__/__doc__/__hash__/__dict__/
/// __weakref__.
fn make_class_finalize_attrs(
    attrs: &mut IndexMap<String, Value>,
    proto_qualname: String,
    class_docstring: Option<&str>,
) -> Result<String> {
    // A bare __init_subclass__ defined in the body is implicitly a classmethod
    // (issue #1047) so super().__init_subclass__() binds cls correctly.
    let isc_wrapped = attrs.get("__init_subclass__").and_then(|v| {
        if let ValueKind::UserFunction(f) = v.kind()
            && f.kind == pyrust_core::UserFunctionKind::Regular
        {
            Some(Value::class_method(Rc::clone(f)))
        } else {
            None
        }
    });
    if let Some(wrapped) = isc_wrapped {
        attrs.insert("__init_subclass__".to_string(), wrapped);
    }
    // __qualname__ lives on `type` as a descriptor, not in the attrs dict, so
    // pop it; an explicit non-str assignment is a TypeError (issue #553).
    let qualname = match attrs.shift_remove("__qualname__") {
        None => proto_qualname,
        Some(v) => match v.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                let tname = pyrust_core::builtin_type_name(&v).into_owned();
                return Err(pyrust_core::type_err!(
                    "type __qualname__ must be a str, not {tname}"
                ));
            }
        },
    };
    attrs
        .entry("__module__".to_string())
        .or_insert_with(|| Value::string("__main__"));
    attrs.entry("__doc__".to_string()).or_insert_with(|| {
        class_docstring
            .map(Value::string)
            .unwrap_or_else(Value::none)
    });
    // A class defining __eq__ but not __hash__ is unhashable (CPython rule).
    if attrs.contains_key("__eq__") && !attrs.contains_key("__hash__") {
        attrs.insert("__hash__".to_string(), Value::none());
    }
    attrs
        .entry("__dict__".to_string())
        .or_insert_with(Value::none);
    attrs
        .entry("__weakref__".to_string())
        .or_insert_with(Value::none);
    Ok(qualname)
}

/// Extract the declared `__slots__` names (string / tuple / list of strings)
/// from the attrs dict.  Returns `None` when no `__slots__` is declared (the
/// instance gets a full __dict__); `Some(set)` restricts instance attributes.
/// When __slots__ is present without a `'__dict__'` slot, the `__dict__` /
/// `__weakref__` class entries are removed so slotted instances have no
/// per-instance dict (CPython parity).
///
/// Once the `PyClass` identity exists, [`install_slot_member_descriptors`]
/// installs one identity-bearing data descriptor per concrete slot.
fn make_class_extract_slots(
    attrs: &mut IndexMap<String, Value>,
) -> Result<Option<indexmap::IndexSet<String>>> {
    let Some(slots_val) = attrs.get("__slots__") else {
        return Ok(None);
    };
    let collect = |items: &[Value]| -> Vec<String> {
        items
            .iter()
            .filter_map(|v| match v.kind() {
                ValueKind::Str(s) => Some(s.to_string()),
                _ => None,
            })
            .collect()
    };
    let slot_names: Vec<String> = match slots_val.kind() {
        ValueKind::Str(s) => vec![s.to_string()],
        ValueKind::Tuple(items) => collect(items),
        ValueKind::List(items) => collect(&items),
        _ => vec![],
    };
    let set: indexmap::IndexSet<String> = slot_names.into_iter().collect();
    // Issue #1971: a slot name that also has a class-variable assignment in
    // the class body is an error (CPython raises ValueError at type creation).
    // `__dict__` / `__weakref__` are handled specially by CPython before the
    // conflict loop, so they are exempt.  The `__dict__` sentinel below is
    // inserted only after this check so it never counts as a class variable.
    for slot in &set {
        if slot == "__dict__" || slot == "__weakref__" {
            continue;
        }
        if attrs.contains_key(slot) {
            return Err(pyrust_core::value_err!(
                "'{slot}' in __slots__ conflicts with class variable"
            ));
        }
    }
    // An all-slots class (no `'__dict__'` slot) has neither a `__dict__` nor a
    // `__weakref__` entry in its class namespace (issue #2076): CPython only
    // adds those getset_descriptors when the layout actually carries a per-
    // instance dict / weakref.  `make_class_finalize_attrs` inserts them
    // unconditionally for the common case, so strip them back out here.  When
    // `'__dict__'` IS a declared slot, keep the `__dict__` entry so
    // `'__dict__' in S.__dict__` stays True.
    if !set.contains("__dict__") {
        attrs.shift_remove("__dict__");
        attrs.shift_remove("__weakref__");
    }
    Ok(Some(set))
}

/// Install the native member descriptors after the owning `PyClass` has a
/// stable identity. Building them while the namespace was still only a
/// `(name, attrs)` pair forced receiver validation to trust a spoofable name.
fn install_slot_member_descriptors(class: &Rc<RefCell<PyClass>>) {
    let slots = class
        .borrow()
        .slots
        .as_ref()
        .map(|slots| slots.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let descriptors = slots
        .into_iter()
        .filter(|slot| slot != "__dict__" && slot != "__weakref__")
        .map(|slot| {
            let descriptor = make_slot_member_descriptor(&slot, class);
            (slot, descriptor)
        })
        .collect::<Vec<_>>();
    class.borrow_mut().attrs.extend(descriptors);
}

#[cfg(test)]
mod primitive_layout_tests {
    use super::{
        IndexMap, PrimitiveLayout, PyClass, Rc, RefCell, object_class_singleton,
        primitive_layout_for_class,
    };

    #[test]
    fn primitive_layout_uses_singleton_identity_not_visible_name() {
        let builtin = crate::interpreter::primitive_class_by_name("list").unwrap();
        let original_name = std::mem::replace(&mut builtin.borrow_mut().name, "renamed".into());
        assert!(matches!(
            primitive_layout_for_class(&builtin),
            PrimitiveLayout::Mutable(pyrust_core::CanonicalClassTag::List)
        ));
        assert_eq!(
            super::super::primitive_owned_object_dunder(&builtin, "__repr__"),
            Some("list.__repr__")
        );
        builtin.borrow_mut().name = original_name;

        let spoof = Rc::new(RefCell::new(PyClass::new(
            "list",
            "list",
            Some(object_class_singleton()),
            IndexMap::new(),
        )));
        assert!(matches!(
            primitive_layout_for_class(&spoof),
            PrimitiveLayout::None
        ));
        assert_eq!(
            super::super::primitive_owned_object_dunder(&spoof, "__repr__"),
            None
        );
    }
}
