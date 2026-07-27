impl Interpreter {
    /// Start tracking the annotation environments created by type aliases in
    /// one class body. The reverse slot map keeps class-store synchronization
    /// independent from compiler name ordering and avoids a hash-map scan on
    /// every assignment.
    fn push_class_annotation_scopes(
        &mut self,
        local_index: &HashMap<String, crate::bytecode::Reg>,
        num_class_regs: usize,
        has_type_alias: bool,
    ) {
        if !has_type_alias {
            self.class_annotation_scopes
                .push(ActiveClassAnnotationScopes {
                    slot_names: Vec::new(),
                    scopes: Vec::new(),
                });
            return;
        }
        let mut slot_names = vec![None; num_class_regs];
        for (name, &slot) in local_index {
            if let Some(target) = slot_names.get_mut(slot as usize) {
                *target = Some(name.clone());
            }
        }
        self.class_annotation_scopes
            .push(ActiveClassAnnotationScopes {
                slot_names,
                scopes: Vec::new(),
            });
    }

    /// Register a class-body type alias's dedicated annotation environment and
    /// seed it from the class fastlocals that have been assigned so far.
    pub(crate) fn register_class_annotation_evaluator(
        &mut self,
        evaluator: &Value,
        regs: &RegSlice,
    ) {
        if !self
            .vm_frame_views
            .last()
            .is_some_and(|frame| frame.kind == FrameKind::Class)
        {
            return;
        }
        let ValueKind::UserFunction(function) = evaluator.kind() else {
            return;
        };
        let env = Rc::clone(&function.env);
        env.borrow_mut().initialize_class_annotation_scope();

        let Some(active) = self.class_annotation_scopes.last_mut() else {
            return;
        };
        for (slot, name) in active.slot_names.iter().enumerate() {
            let Some(name) = name else {
                continue;
            };
            if slot >= regs.len() || regs[slot].is_unset() {
                continue;
            }
            env.borrow_mut()
                .set_class_annotation_binding(name, regs[slot].clone());
        }
        active.scopes.retain(|scope| scope.strong_count() > 0);
        if !active
            .scopes
            .iter()
            .filter_map(Weak::upgrade)
            .any(|scope| Rc::ptr_eq(&scope, &env))
        {
            active.scopes.push(Rc::downgrade(&env));
        }
    }

    /// Record one class-body store and mirror it into every type-alias
    /// annotation environment created by this exact class body.
    pub(crate) fn record_class_namespace_store(
        &mut self,
        regs: &RegSlice,
        slot: crate::bytecode::Reg,
    ) {
        if let Some(order) = self.class_store_order.last_mut()
            && !order.contains(&slot)
        {
            order.push(slot);
        }
        let Some(active) = self.class_annotation_scopes.last_mut() else {
            return;
        };
        let Some(name) = active
            .slot_names
            .get(slot as usize)
            .and_then(Option::as_deref)
        else {
            return;
        };
        let Some(value) = regs.get(slot as usize).filter(|value| !value.is_unset()) else {
            return;
        };
        active.scopes.retain(|scope| {
            let Some(scope) = scope.upgrade() else {
                return false;
            };
            scope
                .borrow_mut()
                .set_class_annotation_binding(name, value.clone());
            true
        });
    }

    /// Record one class-body deletion and remove it from every active
    /// type-alias annotation namespace.
    pub(crate) fn record_class_namespace_delete(&mut self, slot: crate::bytecode::Reg) {
        if let Some(order) = self.class_store_order.last_mut() {
            order.retain(|stored| *stored != slot);
        }
        let Some(active) = self.class_annotation_scopes.last_mut() else {
            return;
        };
        let Some(name) = active
            .slot_names
            .get(slot as usize)
            .and_then(Option::as_deref)
        else {
            return;
        };
        active.scopes.retain(|scope| {
            let Some(scope) = scope.upgrade() else {
                return false;
            };
            scope.borrow_mut().remove_class_annotation_binding(name);
            true
        });
    }

    fn pop_class_annotation_scopes(&mut self) -> Vec<Weak<RefCell<Environment>>> {
        self.class_annotation_scopes
            .pop()
            .expect("class annotation-scope stack popped to empty")
            .scopes
    }

    /// During a plain-dict metaclass call, annotation scopes retain the actual
    /// prepared namespace so mutations made by `__new__` remain visible.
    fn bind_class_annotation_mapping(
        scopes: &[Weak<RefCell<Environment>>],
        namespace: &Value,
    ) {
        if !namespace.is_dict() {
            return;
        }
        for scope in scopes.iter().filter_map(Weak::upgrade) {
            scope
                .borrow_mut()
                .bind_class_annotation_mapping(namespace.clone());
        }
    }

    /// Once a real class exists, switch every alias evaluator to its live class
    /// namespace. Attribute updates and deletions are then observed directly.
    fn bind_class_annotation_owner(
        scopes: &[Weak<RefCell<Environment>>],
        class: &Rc<RefCell<PyClass>>,
    ) {
        for scope in scopes.iter().filter_map(Weak::upgrade) {
            scope.borrow_mut().bind_class_annotation_owner(class);
        }
    }
}
