impl Compiler {
    /// Initialize compile-time cell slots for a function / lambda /
    /// comprehension / genexpr scope (issues #452 / #2339 / #2348).
    ///
    /// Populates `0..own_cell_count` with this function's own cells in
    /// lexicographical order (`co_cellvars`) and eagerly reserves freevar slots
    /// for all `nonlocal` declarations so even unread/unwritten `nonlocal`
    /// names appear in `co_freevars` and `__closure__`.
    fn init_function_cells(
        &mut self,
        enclosing_cells: HashSet<Symbol>,
        globals: &HashSet<String>,
        nonlocals: &HashSet<String>,
    ) {
        let mut own: Vec<Symbol> = self.cell_vars.iter().copied().collect();
        own.sort_unstable_by_key(|sym| sym.as_str());
        for sym in own {
            let idx = self.cell_slots.len() as u16;
            self.cell_slots.push(CellSlotMeta {
                name: sym,
                is_own_cell: true,
                is_free_error: false,
            });
            self.cell_slot_map.insert(sym, idx);
        }
        self.own_cell_count = self.cell_slots.len() as u16;
        self.enclosing_cell_names = enclosing_cells;
        self.global_names = globals.iter().map(|s| Symbol::intern(s)).collect();
        self.nonlocal_names = nonlocals.clone();
        let mut sorted_nonlocals: Vec<&String> = nonlocals.iter().collect();
        sorted_nonlocals.sort();
        for name in sorted_nonlocals {
            self.ensure_free_cell(Symbol::intern(name));
        }
    }

    /// Initialize compile-time cell slots for a class body. Class bodies own no
    /// cells (`own_cell_count == 0`), but capture enclosing function cells for
    /// class-body `LoadClassDeref`, `nonlocal` statements, and transitive
    /// pass-through to nested methods/classes.
    fn init_class_cells(
        &mut self,
        enclosing_cells: HashSet<Symbol>,
        globals: &HashSet<String>,
        nonlocals: &HashSet<String>,
    ) {
        self.own_cell_count = 0;
        self.enclosing_cell_names = enclosing_cells;
        self.global_names = globals.iter().map(|s| Symbol::intern(s)).collect();
        self.nonlocal_names = nonlocals.clone();
        let mut sorted_nonlocals: Vec<&String> = nonlocals.iter().collect();
        sorted_nonlocals.sort();
        for name in sorted_nonlocals {
            self.ensure_free_cell(Symbol::intern(name));
        }
    }

    /// Compute the set of enclosing-function cell names visible to a child scope
    /// compiled inside `self`, excluding any `extra_shadowed` (e.g. PEP 695
    /// type parameters of the child) and any currently active
    /// `self.shadowed_type_params`.
    fn child_enclosing_cell_names(&self, extra_shadowed: &[Symbol]) -> HashSet<Symbol> {
        if self.is_module_scope {
            return HashSet::new();
        }
        let mut visible = self.enclosing_cell_names.clone();
        if self.is_function_scope {
            // Non-captured local variables of `self` shadow outer cells of the
            // same name; captured locals (`own_cell_slots`) replace them.
            for local in self.local_index.keys() {
                let sym = Symbol::intern(local);
                if !self.cell_vars.contains(&sym) {
                    visible.remove(&sym);
                }
            }
            for meta in &self.cell_slots[..self.own_cell_count as usize] {
                visible.insert(meta.name);
            }
        }
        for sym in &self.shadowed_type_params {
            visible.remove(sym);
        }
        for sym in extra_shadowed {
            visible.remove(sym);
        }
        visible
    }

    /// Ensure `sym` has a cell slot in `self.cell_slots` (returning an existing
    /// own-cell or freevar slot if already present, otherwise allocating a new
    /// freevar slot).
    fn ensure_free_cell(&mut self, sym: Symbol) -> u16 {
        if let Some(&idx) = self.cell_slot_map.get(&sym) {
            return idx;
        }
        if self.cell_slots.len() >= u16::MAX as usize {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg =
                    Some("too many closure cells in one scope (max 65535)".to_string());
            }
            return 0;
        }
        let is_free_error = !(self.is_inlined_comp
            && self
                .comp_enclosing_locals
                .as_ref()
                .is_some_and(|locals| locals.contains(&sym)));
        let idx = self.cell_slots.len() as u16;
        self.cell_slots.push(CellSlotMeta {
            name: sym,
            is_own_cell: false,
            is_free_error,
        });
        self.cell_slot_map.insert(sym, idx);
        idx
    }

    /// Ensure every lexically referenced enclosing cell in `body` (including
    /// inside compile-time-false branches like `if False: return x`) receives a
    /// freevar slot so `co_freevars` and `__closure__` match CPython 3.12.
    fn ensure_lexical_free_cells(&mut self, body: &[Stmt]) {
        if self.enclosing_cell_names.is_empty() {
            return;
        }
        let mut lexical_free_names = HashSet::new();
        collect_free_var_reads_in_stmts(body, &mut lexical_free_names);
        collect_transitive_free_vars_in_stmts(body, &mut lexical_free_names);
        let mut sorted: Vec<String> = lexical_free_names.into_iter().collect();
        sorted.sort();
        for candidate in sorted {
            if self.local_index.contains_key(&candidate) {
                continue;
            }
            let sym = Symbol::intern(&candidate);
            if self.global_names.contains(&sym) || !self.enclosing_cell_names.contains(&sym) {
                continue;
            }
            self.ensure_free_cell(sym);
        }
    }

    /// Resolve a child's `free_var_slots()` against `self`, allocating
    /// transitive pass-through freevar slots in `self` whenever the child
    /// captures an ancestor's cell that `self` itself did not touch.
    fn build_capture_map(&mut self, child_code: &FnCode) -> SmallVec<[CaptureSource; 4]> {
        let mut map = SmallVec::with_capacity(child_code.free_var_slots().len());
        for meta in child_code.free_var_slots() {
            let parent_slot = if let Some(&idx) = self.cell_slot_map.get(&meta.name) {
                idx
            } else {
                self.ensure_free_cell(meta.name)
            };
            map.push(CaptureSource::EnclosingCell(parent_slot));
        }
        map
    }

    /// Resolve `name` to a cell slot index for a bare variable **read** inside a
    /// function scope. Returns `None` for module globals, builtins, and PEP 695
    /// type parameters (which must use `LoadGlobal`).
    fn resolve_cell_slot_for_read(&mut self, name: &str) -> Option<u16> {
        if !self.is_function_scope {
            return None;
        }
        let sym = Symbol::intern(name);
        if self.global_names.contains(&sym) || self.shadowed_type_params.contains(&sym) {
            return None;
        }
        if let Some(&idx) = self.cell_slot_map.get(&sym) {
            return Some(idx);
        }
        if self.enclosing_cell_names.contains(&sym) && !self.local_index.contains_key(name) {
            return Some(self.ensure_free_cell(sym));
        }
        None
    }

    /// Resolve `name` to a cell slot index for a **write** or **delete**
    /// (own cell in a function, or `nonlocal` in a function or class body).
    fn resolve_cell_slot_for_write(&mut self, name: &str) -> Option<u16> {
        let sym = Symbol::intern(name);
        if self.is_function_scope {
            if self.global_names.contains(&sym) || self.shadowed_type_params.contains(&sym) {
                return None;
            }
            if let Some(&idx) = self.cell_slot_map.get(&sym)
                && (idx < self.own_cell_count || self.nonlocal_names.contains(name))
            {
                return Some(idx);
            }
            None
        } else if self.is_class_body && self.nonlocal_names.contains(name) {
            self.cell_slot_map.get(&sym).copied()
        } else {
            None
        }
    }

    /// Resolve `name` for a class-body `LOAD_CLASSDEREF` (reading a captured
    /// enclosing function cell without a class-local slot or `global`/`nonlocal`).
    fn resolve_class_body_deref(&mut self, name: &str) -> Option<u16> {
        if !self.is_class_body {
            return None;
        }
        let sym = Symbol::intern(name);
        if self.enclosing_cell_names.contains(&sym) && !self.shadowed_type_params.contains(&sym) {
            Some(self.ensure_free_cell(sym))
        } else {
            None
        }
    }

    /// True if `name` is kept off fastlocal registers (own cell var in function
    /// scope, or env-backed var at module scope).
    fn is_cell(&self, name: &str) -> bool {
        !self.cell_vars.is_empty() && self.cell_vars.contains(&Symbol::intern(name))
    }

    /// Register index for a local variable, or None if the name is global/nonlocal/cell.
    fn local_reg(&self, name: &str) -> Option<Reg> {
        if self.is_cell(name) {
            return None;
        }
        self.local_index.get(name).copied()
    }
}
