impl Compiler {
    fn compile_type_alias(&mut self, name: &str, type_params: &[TypeParam], value: &Expr) {
        // ── Step 1: create TypeVar objects for each type parameter ───────────
        // Each TypeVar is bound (via StoreGlobal) in a dedicated type-parameter
        // env so the RHS expression and any bound/constraint can reference it by
        // name.  Binding into a child env (rather than the enclosing namespace)
        // keeps the parameter names from leaking after the statement, while the
        // env stays alive for the lazy bound/constraint thunks created below —
        // PEP 695 evaluates those on first `__bound__` / `__constraints__`
        // access (#2290), by which time the captured env must still resolve
        // every type parameter (e.g. `type X[T, U: T] = ...`).
        if !type_params.is_empty() {
            self.emit(Insn::PushTypeParamEnv);
        }
        // Phase 1: create every TypeVar (unbounded) and bind its name, so the RHS
        // and any bound/constraint can reference every parameter by name.
        let mut typevar_regs: Vec<Reg> = Vec::with_capacity(type_params.len());
        for param in type_params {
            let tv_reg = self.alloc_temp();
            self.emit_make_typevar(tv_reg, &param.name);
            // Bind the TypeVar to the param name so the RHS expression — and the
            // lazily-evaluated bound/constraint thunks — can load it via
            // LoadGlobal.  StoreGlobal writes into the active (type-param) env's
            // `values`, exactly where a free-variable LoadGlobal looks.
            let param_name_idx = self.intern_name(&param.name);
            self.emit(Insn::StoreGlobal(param_name_idx, tv_reg));
            typevar_regs.push(tv_reg);
        }
        // Phase 2: with every name bound, attach each bound/constraint thunk to
        // the corresponding TypeVar.  Doing this after binding all names lets a
        // self/forward-referential bound (`type X[T, U: T] = ...`) resolve when
        // the thunk runs.
        for (param, &tv_reg) in type_params.iter().zip(typevar_regs.iter()) {
            self.emit_typevar_bound(tv_reg, param);
        }

        // ── Step 2: build the __type_params__ tuple ──────────────────────────
        // BuildTuple(dst, base, n) reads R[base..base+n].  The TypeVar regs are
        // guaranteed to be contiguous from alloc_temp calls above *only* if no
        // other allocation happened in between; compile_store_name may emit
        // SyncModuleGlobal which doesn't allocate regs, so they stay contiguous.
        // However to be safe we copy them to a fresh contiguous block.
        let params_reg = if type_params.is_empty() {
            // Empty tuple: use a literal empty tuple constant.
            let empty_tuple = crate::value::Value::tuple(vec![]);
            let const_idx = self.intern_const(empty_tuple);
            let r = self.alloc_temp();
            self.emit(Insn::LoadConst(r, const_idx));
            r
        } else {
            let base = self.alloc_temp();
            // We already allocated typevar_regs[0] as a temp.  If the first
            // TypeVar reg equals `base` we can reuse the block; otherwise we
            // need to copy.  In practice alloc_temp increments sequentially, so
            // after alloc_temp() for `base` the next regs would conflict.
            // The simplest safe approach: copy all TypeVar values into a fresh
            // contiguous range.
            let n = type_params.len() as Reg;
            // base is the first slot of the contiguous block we'll pass to
            // BuildTuple.  Allocate n-1 more slots after it.
            for _ in 1..n as usize {
                self.alloc_temp();
            }
            // Copy each TypeVar into the contiguous range.
            for (i, &tv_reg) in typevar_regs.iter().enumerate() {
                let slot = base + i as Reg;
                if slot != tv_reg {
                    self.emit(Insn::Move(slot, tv_reg));
                }
            }
            let tuple_dst = self.alloc_temp();
            self.emit(Insn::BuildTuple(tuple_dst, base, n));
            // Free the contiguous block (but not tuple_dst which we return).
            for i in 0..n as usize {
                self.free_temp(base + i as Reg);
            }
            tuple_dst
        };
        // Free the individual TypeVar regs (the tuple holds the values via clone).
        for tv_reg in &typevar_regs {
            self.free_temp(*tv_reg);
        }

        // ── Step 3: evaluate the RHS ─────────────────────────────────────────
        // TypeVar names are bound in the active type-param env, so LoadGlobal
        // for e.g. `T` resolves to the TypeVar object.
        let val_reg = self.compile_expr(value);

        // ── Step 4: leave the type-parameter env ─────────────────────────────
        // Mirrors CPython's hidden annotation scope: type params must NOT be
        // visible in the enclosing scope after the type alias statement.  The
        // popped env stays alive via the Rc captured by each lazy bound thunk
        // (reachable from `__type_params__`), so a later `__bound__` access can
        // still resolve a forward/self reference.
        if !type_params.is_empty() {
            self.emit(Insn::PopTypeParamEnv);
        }

        // ── Step 5: intern the alias name and emit MakeTypeAlias ────────────
        let name_str = crate::value::Value::string(name);
        let name_idx = self.intern_const(name_str);
        let dst = self.alloc_temp();
        self.emit(Insn::MakeTypeAlias(dst, name_idx, val_reg, params_reg));
        self.free_temp(val_reg);
        self.free_temp(params_reg);

        // ── Step 6: store the alias under `name` ─────────────────────────────
        let target = crate::ast::AssignTarget::Name(name.to_string());
        if let Some(reg) = self.local_reg(name) {
            if reg != dst {
                self.emit(Insn::Move(reg, dst));
            }
            self.maybe_record_class_store(reg);
            if self.is_module_scope {
                let name_idx = self.intern_name(name);
                self.emit(Insn::SyncModuleGlobal(reg, name_idx));
            }
        } else {
            let name_idx = self.intern_name(name);
            self.emit(Insn::StoreGlobal(name_idx, dst));
        }
        self.free_temp(dst);
        self.mark_target_def(&target);
    }

    // ── PEP 695 generic type parameters helper ────────────────────────────────

    /// PEP 695: bind each generic type parameter to a fresh `TypeVar` object in
    /// the current scope (via `StoreGlobal`) so that annotations, base-class
    /// expressions, and method/function bodies that reference the parameter name
    /// (e.g. `def f[T](x: T)`) resolve it instead of raising `NameError`.
    ///
    /// Returns the contiguous register block `(base, n)` holding the live
    /// TypeVar objects so the caller can reuse them when building the
    /// `__type_params__` tuple — CPython keeps the *same* TypeVar object in both
    /// `__type_params__` and the annotations (`f.__type_params__[0] is
    /// f.__annotations__['x']`).  The caller must keep `next_temp > base + n`
    /// until it has emitted the `__type_params__` tuple, then is free to reclaim
    /// the slots.
    ///
    /// Returns `(0, 0)` when there are no type parameters (caller must skip the
    /// reuse path in that case).
    ///
    /// Note: the bound names are intentionally *not* deleted afterwards. Unlike
    /// the type-alias path (whose RHS is fully evaluated inline), a generic
    /// function/class body references its type parameters lazily at call time via
    /// `LoadGlobal`, so the binding must outlive the definition statement. This
    /// leaks the parameter name into the enclosing namespace, which CPython hides
    /// behind a dedicated annotation scope; see the deferred note in the PR.
    fn emit_bind_type_params(&mut self, type_params: &[TypeParam]) -> (Reg, Reg) {
        let n = type_params.len() as Reg;
        if n == 0 {
            return (0, 0);
        }
        if self.next_temp.checked_add(n as u32).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("too many registers for type params".to_string());
            }
            return (0, 0);
        }
        let base = self.next_temp;
        self.next_temp += n as u32;
        if self.next_temp - 1 > self.max_reg {
            self.max_reg = self.next_temp - 1;
        }
        // Phase 1: create every TypeVar (initially unbounded) and bind its name.
        // All type parameters must exist and be in scope before any bound is
        // evaluated, so a self/forward-referential bound (`def f[T: T]`,
        // `def g[T, U: T]`) can resolve every parameter name — PEP 695 evaluates
        // bounds/constraints lazily in a scope where all the type params (and the
        // enclosing name) are visible.
        for (i, param) in type_params.iter().enumerate() {
            let tv_reg = base + i as Reg;
            self.emit_make_typevar(tv_reg, &param.name);
            // Bind via StoreGlobal so the name lands in `env.values`, which is
            // exactly where a body's `LoadGlobal` for a free variable looks.
            let name_idx = self.intern_name(&param.name);
            self.emit(Insn::StoreGlobal(name_idx, tv_reg));
        }
        // Phase 2: with every name now bound, evaluate each bound/constraint and
        // store it onto the already-created TypeVar.
        for (i, param) in type_params.iter().enumerate() {
            let tv_reg = base + i as Reg;
            self.emit_typevar_bound(tv_reg, param);
        }
        (base, n)
    }

    /// Emit a `MakeTypeVar` into `tv_reg` for the type parameter named `name`.
    /// The TypeVar is created unbounded (`__bound__ == None`, `__constraints__
    /// == ()`); any bound/constraint clause is populated later by
    /// `emit_typevar_bound`, once every type parameter is in scope.
    fn emit_make_typevar(&mut self, tv_reg: Reg, name: &str) {
        let name_const = self.intern_const(crate::value::Value::string(name));
        self.emit(Insn::MakeTypeVar(tv_reg, name_const));
    }

    /// Attach a PEP 695 lazy bound/constraint *thunk* to an already-created
    /// TypeVar in `tv_reg`.  CPython evaluates a type parameter's bound or
    /// constraints lazily — not at def/class/alias time, but on first access of
    /// `__bound__` / `__constraints__` — in a deferred annotation scope where
    /// every type parameter (and the enclosing names) is visible.
    ///
    /// We mirror this by compiling the clause expression into a zero-argument
    /// closure (`lambda: <expr>`) that captures the active type-parameter env,
    /// and storing it on the TypeVar's internal `__evaluate_bound__` /
    /// `__evaluate_constraints__` slot.  The thunk is invoked once, on first
    /// read of `__bound__` / `__constraints__`, and its result cached (see
    /// `get_attr_instance_raw`).  Self- and forward-referential bounds
    /// (`T: T`, `U: T`) still resolve because the captured env binds every
    /// parameter name.  A bare parameter (no clause) leaves the eager defaults
    /// (`__bound__ == None`, `__constraints__ == ()`) untouched.
    fn emit_typevar_bound(&mut self, tv_reg: Reg, param: &TypeParam) {
        match &param.bound {
            None => {}
            Some(TypeParamBound::Bound(expr)) => {
                let thunk_reg = self.compile_lambda(&[], expr);
                let attr_idx = self.intern_name("__evaluate_bound__");
                self.emit(Insn::SetTypeVarAttr(tv_reg, attr_idx, thunk_reg));
                self.free_temp(thunk_reg);
            }
            Some(TypeParamBound::Constraints(elems)) => {
                let tuple_expr = Expr::Tuple(elems.to_vec());
                let thunk_reg = self.compile_lambda(&[], &tuple_expr);
                let attr_idx = self.intern_name("__evaluate_constraints__");
                self.emit(Insn::SetTypeVarAttr(tv_reg, attr_idx, thunk_reg));
                self.free_temp(thunk_reg);
            }
        }
    }

    /// Build the `__type_params__` tuple from an already-bound contiguous block
    /// of TypeVar registers (produced by `emit_bind_type_params`) and store it on
    /// `obj_reg`.  Reusing the bound registers preserves TypeVar object identity
    /// between `__type_params__` and the annotations that reference the names.
    fn emit_type_params_attr_from_regs(&mut self, obj_reg: Reg, base: Reg, n: Reg) {
        let saved_next = self.next_temp;
        if self.next_temp <= base + n {
            self.next_temp = base + n;
        }
        let tuple_reg = self.alloc_temp();
        self.emit(Insn::BuildTuple(tuple_reg, base, n));
        let attr_name_idx = self.intern_name("__type_params__");
        self.emit(Insn::SetAttr(obj_reg, attr_name_idx, tuple_reg));
        // The TypeVar block and the tuple slot are dead after SetAttr.
        self.next_temp = saved_next;
    }

    // ── Assignment ────────────────────────────────────────────────────────────
}
