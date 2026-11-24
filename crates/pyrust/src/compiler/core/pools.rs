impl Compiler {
    fn new(
        local_index: Rc<HashMap<String, Reg>>,
        def_bound_mask: u64,
        cell_vars: Vec<CellVar>,
    ) -> Self {
        let n = local_index.len();
        let cell_set: HashSet<Symbol> = cell_vars.into_iter().collect();
        // base_temp must cover ALL local_index slots (including cell vars) so
        // that temp registers never overlap with local-variable slot numbers.
        let base_temp = Reg::try_from(n).unwrap_or(Reg::MAX);

        Self {
            local_index,
            cell_vars: cell_set,
            cell_slots: SmallVec::new(),
            own_cell_count: 0,
            cell_slot_map: HashMap::new(),
            enclosing_cell_names: HashSet::new(),
            global_names: HashSet::new(),
            shadowed_type_params: SmallVec::new(),
            nonlocal_names: HashSet::new(),
            class_direct_env_names: HashSet::new(),
            insns: Vec::new(),
            lineno_table: Vec::new(),
            col_table: Vec::new(),
            // (0, 0, 0, 0) sentinel = no anchor (#2411).
            current_lineno: 0,
            current_col_span: (0, 0, 0, 0),
            first_lineno: 0,
            filename: std::sync::Arc::from("<unknown>"),
            qualname: std::sync::Arc::from("<module>"),
            consts: Vec::new(),
            const_index: HashMap::new(),
            names: Vec::new(),
            name_map: HashMap::new(),
            next_temp: base_temp,
            base_temp,
            iter_depth: 0,
            max_iter: 0,
            max_reg: if n > 0 {
                Reg::try_from(n).unwrap_or(Reg::MAX).saturating_sub(1)
            } else {
                0
            },
            loops: Vec::new(),
            except_cleanups: SmallVec::new(),
            failed: n > Reg::MAX as usize,
            error_msg: if n > Reg::MAX as usize {
                Some(format!("too many local variables (max {})", Reg::MAX))
            } else {
                None
            },
            def_set: def_bound_mask,
            fn_protos: Vec::new(),
            pure_locals: HashSet::new(),
            is_class_body: false,
            is_class_method: false,
            qualname_prefix: String::new(),
            outer_locals: SmallVec::new(),
            is_function_scope: false,
            is_async_function: false,
            is_async_generator_fn: false,
            is_syntax_error: false,
            is_module_scope: false,
            print_exprs: false,
            module_namespace_may_be_exposed: false,
            past_future_zone: false,
            future_annotations: false,
            has_dead_yield: false,
            is_set_comp: false,
            is_list_comp: false,
            list_comp_presize: false,
            is_inlined_comp: false,
            comp_enclosing_locals: None,
            comp_outer_for_span: None,
        }
    }

    /// If this compiler is producing a class body and `reg` is one of the
    /// class-body's local slots, emit a `RecordClassStore(reg)` insn so the
    /// VM can update class-namespace ordering and any materialized live dict.
    /// No-op outside class bodies and for temp / cell registers — keeping
    /// regular function compilation untouched.
    fn maybe_record_class_store(&mut self, reg: Reg) {
        if self.is_class_body && reg < self.base_temp {
            self.emit(Insn::RecordClassStore(reg));
        }
    }

    /// Companion to `maybe_record_class_store`: emit a `RecordClassDel`
    /// after a `DeleteLocal` so the slot is removed from the class-namespace
    /// store-order list while preserving the order of remaining entries.
    fn maybe_record_class_del(&mut self, reg: Reg) {
        if self.is_class_body && reg < self.base_temp {
            self.emit(Insn::RecordClassDel(reg));
        }
    }

    fn mark_def(&mut self, reg: Reg) {
        if (reg as usize) < 64 {
            self.def_set |= 1u64 << reg;
        }
    }

    fn mark_target_def(&mut self, target: &AssignTarget) {
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    self.mark_def(reg);
                }
            }
            AssignTarget::Tuple(targets) => {
                for t in targets {
                    self.mark_target_def(t);
                }
            }
            AssignTarget::Starred(inner) => {
                self.mark_target_def(inner);
            }
            _ => {}
        }
    }

    fn intern_symbol(&mut self, sym: Symbol) -> u16 {
        if let Some(&idx) = self.name_map.get(&sym) {
            return idx;
        }
        if self.names.len() >= u16::MAX as usize {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some(format!("too many distinct names (max {})", u16::MAX));
            }
            return 0;
        }
        let idx = self.names.len() as u16;
        self.names.push(sym);
        self.name_map.insert(sym, idx);
        idx
    }

    fn intern_name(&mut self, name: &str) -> u16 {
        self.intern_symbol(Symbol::intern(name))
    }

    fn intern_const(&mut self, val: Value) -> u16 {
        // PyKey treats `Bool(b)` and `Int(b as i64)` as hash/eq-equal (matching
        // CPython's `True == 1`), so they would collide in the constant pool's
        // hash index even though they are type-distinct values.  Likewise,
        // `Float(1.0)` and `Int(1)` are now hash/eq-equal in PyKey so that
        // dict/set keys respect CPython's numeric equality invariant.  Complex
        // values with zero imaginary part map to `PyKey::Float` via `to_key()`,
        // which would collide with integer-valued floats and ints.  In all these
        // cases the constant pool must keep the values distinct, so we skip the
        // hash-map fast path and fall through to the type-exact linear scan.
        let is_bool = matches!(val.kind(), ValueKind::Bool(_));
        let is_float = matches!(val.kind(), ValueKind::Float(_));
        let is_complex = matches!(val.kind(), ValueKind::Complex(_, _));
        if !is_bool
            && !is_float
            && !is_complex
            && let Some(key) = val.to_key()
        {
            if let Some(&idx) = self.const_index.get(&key) {
                return idx;
            }
            if self.consts.len() >= u16::MAX as usize {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some(format!("too many constants (max {})", u16::MAX));
                }
                return 0;
            }
            let idx = self.consts.len() as u16;
            self.const_index.insert(key, idx);
            self.consts.push(val);
            return idx;
        }
        // Non-hashable constants, booleans, and floats: type-exact linear scan.
        for (i, v) in self.consts.iter().enumerate() {
            if const_eq(v, &val) {
                return i as u16;
            }
        }
        if self.consts.len() >= u16::MAX as usize {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some(format!("too many constants (max {})", u16::MAX));
            }
            return 0;
        }
        let idx = self.consts.len() as u16;
        self.consts.push(val);
        idx
    }
}
