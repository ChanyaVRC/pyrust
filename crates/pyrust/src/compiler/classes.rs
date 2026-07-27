impl Compiler {
    /// Validate the class body's global/nonlocal/annotation rules, build its
    /// register index, compile the body as a zero-param function in a child
    /// compiler, and push the resulting class `FnProto`.  Returns the proto
    /// index, or `None` when an error was recorded and the caller must bail.
    fn build_class_proto(
        &mut self,
        name: &str,
        keywords: &[(String, Expr)],
        body: &[Stmt],
    ) -> Option<u16> {
        // Class body: zero-param function that returns its locals as class dict.
        // Collect names explicitly declared `global` in the class body so they
        // are excluded from `body_local` and routed to `Insn::StoreGlobal`
        // instead of `Insn::RecordClassStore`.  Without this, `global x; x = 42`
        // inside a class body silently stored into the class attribute dict
        // rather than the module-level global (issue #618).
        let body_global = Rc::new(crate::interpreter::collect_global_names(body));
        // Collect `nonlocal` declarations in the class body (issue #708 / #735).
        // These names must not get a class-body register slot — they are
        // stored/loaded via the enclosing function's env, not the class namespace.
        let body_nonlocal = crate::interpreter::collect_nonlocal_names(body);
        // Validate: every `nonlocal x` in the class body must have a binding in
        // some enclosing *function* scope.  Module scope and class scope do not
        // count — `nonlocal` requires an enclosing function binding (CPython 3.12
        // raises SyntaxError: no binding for nonlocal 'x' found).
        {
            let mut sorted: Vec<&String> = body_nonlocal.iter().collect();
            sorted.sort();
            for nonlocal_name in sorted {
                let found = self
                    .outer_locals
                    .iter()
                    .any(|m| m.contains_key(nonlocal_name))
                    || (self.is_function_scope && self.local_index.contains_key(nonlocal_name));
                if !found {
                    self.set_syntax_error(&format!(
                        "no binding for nonlocal '{}' found",
                        nonlocal_name
                    ));
                    return None;
                }
            }
        }
        let body_nonlocal_rc: Rc<HashSet<String>> = Rc::new(body_nonlocal.clone());

        // Validate annotation targets against global/nonlocal declarations.
        // CPython 3.12 raises SyntaxError for `class C: global x; x: int` and
        // `class C: nonlocal x; x: int`.  Declaration order does not matter —
        // the check is whole-scope (issue #770).
        let ann_targets = crate::interpreter::collect_annotation_target_names(body);
        for ann_name in &ann_targets {
            if body_global.contains(ann_name) {
                self.set_syntax_error(&format!("annotated name '{}' can't be global", ann_name));
                return None;
            }
            if body_nonlocal.contains(ann_name) {
                self.set_syntax_error(&format!("annotated name '{}' can't be nonlocal", ann_name));
                return None;
            }
        }

        // Validate ordering: global/nonlocal declarations must appear before any
        // assignment or use of the same name in the class body.
        if let Some(msg) = crate::interpreter::check_global_nonlocal_order(body) {
            self.failed = true;
            self.is_syntax_error = true;
            if self.error_msg.is_none() {
                self.error_msg = Some(msg);
            }
            return None;
        }

        let body_local =
            crate::interpreter::collect_local_names(&[], body, &body_global, &body_nonlocal_rc);
        // Allocate a register slot for every potential class-body local.
        // Slot order is **not** used to encode class-namespace insertion
        // order any more — the order CPython exposes via `vars(C)` is the
        // order stores actually executed at runtime, not source-walk order.
        // Each store now emits `Insn::RecordClassStore(slot)` and the VM
        // builds the attrs dict from that runtime trace inside `MakeClass`.
        // We still walk the body textually here so register numbers follow
        // declaration order for names that only appear inside control-flow
        // blocks (where the IndexSet insertion order and textual order agree,
        // but names inside nested blocks need the explicit walk to be seen
        // before the catch-all pass at the end).
        //
        // Issue #546: CPython pre-injects `__qualname__` and `__module__`
        // into the class namespace before the body runs.  Give them fixed
        // register slots (0 and 1) so the VM can pre-populate them and so
        // `locals()` inside the class body always includes them.  If the
        // user explicitly assigns either name in the body, `collect_local_names`
        // will have included it in `body_local` already; we skip it here to
        // avoid a duplicate slot.
        let mut ordered: Vec<String> = Vec::with_capacity(body_local.len() + 2);
        let mut seen: HashSet<String> = HashSet::new();
        // CPython injects __module__ first, __qualname__ second.
        for pre_name in ["__module__", "__qualname__"] {
            if !body_local.contains(pre_name) {
                ordered.push(pre_name.to_string());
                seen.insert(pre_name.to_string());
            }
        }
        // Issue #712: if the class body has any annotations, pre-allocate
        // a register slot for __annotations__ so compile_ann_assign can use a
        // fastlocal (RecordClassStore) rather than a LoadGlobal.
        if class_body_has_annotations(body) && !body_local.contains("__annotations__") {
            ordered.push("__annotations__".to_string());
            seen.insert("__annotations__".to_string());
        }
        collect_class_body_names_textual(body, &mut ordered, &mut seen, &body_local);
        for name in body_local.iter() {
            if seen.insert(name.clone()) {
                ordered.push(name.clone());
            }
        }
        let mut body_index: HashMap<String, Reg> = HashMap::new();
        for (i, loc) in (0u32..).zip(ordered.iter()) {
            body_index.insert(loc.clone(), i);
        }
        let body_index_rc: Rc<HashMap<String, Reg>> = Rc::new(body_index);
        // Use the class-body variant: a method's `global x` must not promote
        // the class-body name `x` to a cell var (issue #624).
        let cell_vars = collect_cell_vars_for_class_body(body, &body_index_rc);

        // Compute the full qualname for this class.
        // For `class Outer: class Inner`, `self.qualname_prefix` is `"Outer"` and
        // `class_qualname` becomes `"Outer.Inner"`.
        // The child compiler's `qualname_prefix` is set to `class_qualname` so
        // that further nested classes or functions inside it get the right prefix.
        let class_qualname = if self.qualname_prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}.{}", self.qualname_prefix, name)
        };

        let mut sub = Compiler::new(Rc::clone(&body_index_rc), 0, cell_vars);
        sub.is_class_body = true;
        // Threaded source file (#2438): methods defined in this class body inherit
        // the enclosing scope's `co_filename`.
        sub.filename = self.filename.clone();
        sub.qualname_prefix = class_qualname.clone();
        // Thread the enclosing function scope chain into the class body compiler.
        // Class scope is transparent to `nonlocal` (not a function scope), so we
        // pass through outer_locals without adding body_index_rc, and leave
        // is_function_scope = false.
        sub.outer_locals = self.outer_locals.clone();
        if self.is_function_scope {
            sub.outer_locals.push(Rc::clone(&self.local_index));
        }
        // Propagate PEP 563 lazy-annotation flag to the class body compiler.
        sub.future_annotations = self.future_annotations;
        sub.compile_block(body);
        // Add implicit ReturnNone at end of class body
        sub.emit(Insn::ReturnNone);
        let body_code = match sub.finish() {
            Ok(c) => c,
            Err(e) => {
                self.failed = true;
                if matches!(e, PyError::Named(ref cls, _) if cls.as_ref() == "SyntaxError") {
                    self.is_syntax_error = true;
                }
                if self.error_msg.is_none() {
                    self.error_msg = Some(match e {
                        PyError::Named(_, msg) | PyError::Runtime(msg) => msg,
                        other => other.to_string(),
                    });
                }
                return None;
            }
        };
        if self.fn_protos.len() >= u16::MAX as usize {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg =
                    Some("too many classes/functions in one scope (max 65535)".to_string());
            }
            return None;
        }
        let proto_idx = self.fn_protos.len() as u16;
        let local_names = Rc::new(body_index_rc.keys().cloned().collect::<HashSet<_>>());
        // Extract docstring: if the first statement in the class body is a bare
        // string literal, capture it as the class's __doc__ (CPython parity).
        let class_docstring = match body {
            [Stmt::Expr(Expr::Str(s)), ..] => Some(s.clone()),
            _ => None,
        };
        self.fn_protos.push(FnProto {
            name: Rc::from(name),
            qualname: Rc::from(class_qualname.as_str()),
            param_spec: Rc::new(FnParamSpec {
                names: SmallVec::new(),
                has_default: SmallVec::new(),
                is_args: SmallVec::new(),
                is_kwargs: SmallVec::new(),
                is_keyword_only: SmallVec::new(),
                is_positional_only: SmallVec::new(),
            }),
            code: Rc::new(body_code),
            local_index: body_index_rc,
            param_binds: Rc::new(Vec::new()),
            self_bind: None,
            local_names,
            global_names: body_global,
            nonlocal_names: body_nonlocal_rc,
            is_memo_pure: false,
            annotation_keys: SmallVec::new(),
            docstring: class_docstring,
            class_kwarg_names: keywords.iter().map(|(k, _)| k.clone()).collect(),
        });

        Some(proto_idx)
    }

    /// Compile the base-class expressions and PEP 487 keyword-argument values
    /// into two contiguous register windows.  Returns
    /// `(bases_base, bases_n, kwarg_base, kwarg_n)`, or `None` on register
    /// overflow (error already recorded).
    fn emit_class_bases_and_keywords(
        &mut self,
        bases: &[Expr],
        keywords: &[(String, Expr)],
    ) -> Option<(Reg, u32, Reg, u32)> {
        // Compile base class expressions.
        let bases_n = match u32::try_from(bases.len()) {
            Ok(count) => count,
            Err(_) => {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many base classes".to_string());
                }
                return None;
            }
        };
        let bases_base = self.next_temp;
        if bases_n > 0 {
            if self.next_temp.checked_add(Reg::from(bases_n)).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many base class registers".to_string());
                }
                return None;
            }
            self.next_temp += Reg::from(bases_n);
            if self.next_temp - 1 > self.max_reg {
                self.max_reg = self.next_temp - 1;
            }
            for (i, base_expr) in (0u32..).zip(bases.iter()) {
                let saved = self.next_temp;
                let r = self.compile_expr(base_expr);
                if r != bases_base + i {
                    self.emit(Insn::Move(bases_base + i, r));
                }
                self.next_temp = saved;
            }
        }

        // Compile PEP 487 keyword arg values into consecutive registers.
        // These are forwarded to __init_subclass__; names are stored in FnProto.
        let kwarg_n = match u32::try_from(keywords.len()) {
            Ok(count) => count,
            Err(_) => {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many class keywords".to_string());
                }
                return None;
            }
        };
        let kwarg_base = self.next_temp;
        if kwarg_n > 0 {
            if self.next_temp.checked_add(Reg::from(kwarg_n)).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many class keyword registers".to_string());
                }
                return None;
            }
            self.next_temp += Reg::from(kwarg_n);
            if self.next_temp - 1 > self.max_reg {
                self.max_reg = self.next_temp - 1;
            }
            for (i, (_, val_expr)) in (0u32..).zip(keywords.iter()) {
                let saved = self.next_temp;
                let r = self.compile_expr(val_expr);
                if r != kwarg_base + i {
                    self.emit(Insn::Move(kwarg_base + i, r));
                }
                self.next_temp = saved;
            }
        }

        Some((bases_base, bases_n, kwarg_base, kwarg_n))
    }

    // AST-node compile entry: each arg is a distinct syntactic child of the
    // `class` statement; bundling them into a struct only relocates the field list.
    #[allow(clippy::too_many_arguments)]
    fn compile_class(
        &mut self,
        name: &str,
        bases: &[Expr],
        metaclass: Option<&Expr>,
        keywords: &[(String, Expr)],
        body: &[Stmt],
        decorators: &[Expr],
        type_params: &[TypeParam],
    ) {
        if let Some(message) = validate_type_parameter_bounds(type_params) {
            self.set_syntax_error(&message);
            return;
        }

        let proto_idx = match self.build_class_proto(name, keywords, body) {
            Some(idx) => idx,
            None => return,
        };

        // PEP 695: push a dedicated type-parameter environment and bind the type
        // parameters into it before the base-class expressions are evaluated (so
        // `class C[T](Base[T])` resolves `T`) and before the class body runs (so
        // a method annotation `def m(self, x: T)` resolves `T` at class-creation
        // time).  Binding them in a child env keeps the names from leaking into
        // the enclosing scope after the class statement, while the class object —
        // which captures this env — can still resolve them.  The block sits below
        // `dst`; the watermark resets below keep it live until
        // `finish_class_definition` builds the tuple and pops the env.
        if !type_params.is_empty() {
            self.emit(Insn::PushTypeParamEnv);
        }
        let (tp_base, tp_n) = self.emit_bind_type_params(type_params);

        let (bases_base, bases_n, kwarg_base, kwarg_n) =
            match self.emit_class_bases_and_keywords(bases, keywords) {
                Some(v) => v,
                None => return,
            };

        let name_idx = self.intern_name(name);

        // With an explicit `metaclass=`, route the whole creation through
        // `MakeClassMeta`: it calls `metaclass.__prepare__`, runs the body into
        // that namespace, and calls `metaclass(name, bases, ns, **kw)` so the
        // class-creation hooks fire once inside the metaclass (issues
        // #2128/#2130).  The metaclass value must live in a register kept alive
        // across the instruction; allocate it after the bases/kwargs region.
        if let Some(meta_expr) = metaclass {
            let meta_reg = self.alloc_temp();
            let saved = self.next_temp;
            self.compile_expr_into(meta_expr, meta_reg);
            self.next_temp = saved;
            let dst = self.alloc_temp();
            self.emit(Insn::MakeClassMeta(
                dst, proto_idx, bases_base, bases_n, name_idx, kwarg_base, kwarg_n, meta_reg,
            ));
            // bases/kwargs/meta_reg are dead after the instruction; keep only
            // `dst` (the class object) live for decorators / type-params / store.
            self.next_temp = dst + 1;
            return self.finish_class_definition(name, dst, decorators, tp_base, tp_n);
        }

        let dst = self.alloc_temp();
        self.emit(Insn::MakeClass(
            dst, proto_idx, bases_base, bases_n, name_idx, kwarg_base, kwarg_n,
        ));
        if bases_n > 0 {
            // The base registers are dead after MakeClass, but `dst` (the freshly
            // built class object) must stay live for the decorator / type-params
            // / store steps below.  `dst` was allocated immediately after the
            // bases, so `dst == bases_base + bases_n`; the correct watermark is
            // therefore `dst + 1`, which preserves the class object and releases
            // every slot above it.
            //
            // The previous formula (`bases_base + 1`) overwrote `dst` whenever a
            // base was present: with one base, `bases_base + 1 == dst`, so the
            // subsequent decorator base allocated the same register as `dst` and
            // the decorator value clobbered the class object (issue #1889). The
            // class decorator then received the decorator function itself.
            self.next_temp = dst + 1;
        }

        self.finish_class_definition(name, dst, decorators, tp_base, tp_n);
    }

    /// Shared tail of class compilation for both the plain `MakeClass` and the
    /// metaclass `MakeClassMeta` paths: apply PEP 695 `__type_params__`, run the
    /// class decorators, store the result, and free the class register.  On
    /// entry `dst` holds the class object and `next_temp == dst + 1`.
    /// `tp_base`/`tp_n` describe the contiguous block of bound TypeVar registers
    /// produced by `emit_bind_type_params` (`tp_n == 0` for a non-generic class).
    fn finish_class_definition(
        &mut self,
        name: &str,
        dst: Reg,
        decorators: &[Expr],
        tp_base: Reg,
        tp_n: Reg,
    ) {
        // PEP 695: if this is a generic class, build the __type_params__ tuple
        // and store it on the class object before decorators are applied.  The
        // tuple reuses the TypeVar registers bound before the class body ran, so
        // the objects in __type_params__ are identical to those the body saw.
        if tp_n > 0 {
            if self.next_temp <= dst {
                self.next_temp = dst + 1;
            }
            self.emit_type_params_attr_from_regs(dst, tp_base, tp_n);
            // PEP 695: pop the type-parameter environment now that the class
            // object exists and its `__type_params__` is set.  Decorators and the
            // class-name binding belong to the enclosing scope.
            self.emit(Insn::PopTypeParamEnv);
        }

        // Evaluate decorator expressions top-to-bottom, then apply bottom-to-top.
        let mut val_reg = dst;
        if !decorators.is_empty() {
            let n = decorators.len() as u32;
            let deco_base = self.next_temp;
            if deco_base.checked_add(n + 1).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg =
                        Some("too many registers for decorator application".to_string());
                }
                return;
            }
            self.next_temp = deco_base + n + 1;
            if deco_base + n > self.max_reg {
                self.max_reg = deco_base + n;
            }
            for (i, deco_expr) in decorators.iter().enumerate() {
                let saved = self.next_temp;
                self.compile_expr_into(deco_expr, deco_base + i as u32);
                self.next_temp = saved;
            }
            for i in (0..n).rev() {
                let frame = deco_base + i;
                self.emit(Insn::Move(frame + 1, val_reg));
                self.emit(Insn::Call(frame, 1));
                val_reg = frame;
            }
            self.next_temp = deco_base + 1;
        }

        self.compile_store_name(name, val_reg);
        if let Some(reg) = self.local_reg(name) {
            self.mark_def(reg);
        }
        self.free_temp(dst);
    }
}
