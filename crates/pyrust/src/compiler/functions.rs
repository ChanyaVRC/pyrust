impl Compiler {
    // ── Def / Class ───────────────────────────────────────────────────────────

    /// Build the inner-function scope metadata, validate global/nonlocal/
    /// annotation rules, compile the body into a child compiler, and push the
    /// resulting `FnProto`.  Returns
    /// `(proto_idx, is_memo_pure, has_kwonly_params)`,
    /// or `None` when a (syntax/limit) error was recorded and the caller must
    /// bail out.
    #[allow(clippy::too_many_arguments)]
    fn build_def_proto(
        &mut self,
        name: &str,
        params: &[FunctionParam],
        body: &[Stmt],
        body_linenos: &[u32],
        def_lineno: u32,
        return_annotation: Option<&Expr>,
        is_async: bool,
    ) -> Option<(u16, bool, bool)> {
        // Build inner function's scope metadata.
        let inner_global = crate::interpreter::collect_global_names(body);
        let inner_nonlocal = crate::interpreter::collect_nonlocal_names(body);

        // A parameter may not also be declared `global`/`nonlocal` in the body.
        // CPython 3.12 raises `SyntaxError: name 'x' is parameter and global`
        // (resp. `... and nonlocal`).  This conflict wins over the later
        // ordering / annotation / no-binding diagnostics, so check it first.
        for p in params {
            if inner_global.contains(&p.name) {
                self.set_syntax_error(&format!("name '{}' is parameter and global", p.name));
                return None;
            }
            if inner_nonlocal.contains(&p.name) {
                self.set_syntax_error(&format!("name '{}' is parameter and nonlocal", p.name));
                return None;
            }
        }

        let inner_local =
            crate::interpreter::collect_local_names(params, body, &inner_global, &inner_nonlocal);

        // Build a compact local_index for the inner function.
        // Parameters come first (preserving declaration order), then body locals.
        let mut inner_index: HashMap<String, Reg> = HashMap::new();
        let mut slot: Reg = 0;
        for param in params {
            if inner_local.contains(&param.name) {
                inner_index.insert(param.name.clone(), slot);
                slot += 1;
            }
        }
        for loc in &inner_local {
            if !inner_index.contains_key(loc) {
                inner_index.insert(loc.clone(), slot);
                slot += 1;
            }
        }
        let inner_index_rc: Rc<HashMap<String, Reg>> = Rc::new(inner_index);

        let def_bound = crate::interpreter::compute_def_bound_mask(params, &inner_index_rc);
        // Include only `name` so direct self-recursion can use the fixpoint
        // assumption.  Sibling/global function names are mutable bindings:
        // carrying `self.pure_locals` into this body would let a later rebind
        // make the outer function's memo cache serve stale results.
        let mut pure_fns_with_self = std::collections::HashSet::new();
        pure_fns_with_self.insert(name.to_string());
        // A coroutine function (`async def`, issue #1039) is never memo-pure:
        // calling it must build a fresh coroutine object.
        // Memo purity gates `CallMemo` emission and the VM result cache while
        // keeping comparison/unary self-recursive functions (`fib`) memoized.
        let is_memo_pure = !is_async
            && crate::interpreter::is_memo_pure_function_body(
                body,
                &pure_fns_with_self,
                &inner_index_rc,
                name,
                params,
            );

        // Detect cell vars for the inner function.
        let inner_cell_vars = collect_cell_vars(body, &inner_index_rc);

        // Validate ordering: global/nonlocal declarations must appear before
        // any assignment or use of the same name in the function body.
        // CPython 3.12 raises SyntaxError for `def f(): x = 1; global x`.
        if let Some(msg) = crate::interpreter::check_global_nonlocal_order(body) {
            self.failed = true;
            self.is_syntax_error = true;
            if self.error_msg.is_none() {
                self.error_msg = Some(msg);
            }
            return None;
        }

        // Validate annotation targets against global/nonlocal declarations.
        // CPython 3.12 raises SyntaxError for `def f(): global x; x: int` and
        // `def f(): nonlocal x; x: int` (issue #748 / companion to #770).
        let def_ann_targets = crate::interpreter::collect_annotation_target_names(body);
        for ann_name in &def_ann_targets {
            if inner_global.contains(ann_name) {
                self.set_syntax_error(&format!("annotated name '{}' can't be global", ann_name));
                return None;
            }
            if inner_nonlocal.contains(ann_name) {
                self.set_syntax_error(&format!("annotated name '{}' can't be nonlocal", ann_name));
                return None;
            }
        }

        let inner_global_rc = Rc::new(inner_global);
        let inner_nonlocal_rc = Rc::new(inner_nonlocal);

        // Compile-time validation: every name in `inner_nonlocal` must have a
        // binding in some enclosing *function* scope.  Module scope and class
        // scope do not count (`nonlocal` is only valid inside a function).
        //
        // `self.outer_locals` is the chain of enclosing function scope
        // `local_index` maps (outermost first).  If `self.is_function_scope`,
        // `self.local_index` is also an enclosing function scope.
        let mut sorted_nonlocals: Vec<&String> = inner_nonlocal_rc.iter().collect();
        sorted_nonlocals.sort();
        for nonlocal_name in sorted_nonlocals {
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

        // Compile-time validation: an annotated name (`x: T` or `x: T = v`)
        // cannot also be declared `global` or `nonlocal` in the same function
        // scope.  CPython 3.12 raises `SyntaxError: annotated name 'x' can't
        // be global` / `can't be nonlocal`.
        let ann_targets = crate::interpreter::collect_annotation_target_names(body);
        let mut sorted_ann: Vec<&String> = ann_targets.iter().collect();
        sorted_ann.sort();
        for ann_name in sorted_ann {
            if inner_global_rc.contains(ann_name) {
                self.set_syntax_error(&format!("annotated name '{}' can't be global", ann_name));
                return None;
            }
            if inner_nonlocal_rc.contains(ann_name) {
                self.set_syntax_error(&format!("annotated name '{}' can't be nonlocal", ann_name));
                return None;
            }
        }

        let mut sub = Compiler::new(Rc::clone(&inner_index_rc), def_bound, inner_cell_vars);
        // Threaded source file (#2438): the nested function's code object shares
        // its enclosing scope's `co_filename` so an imported module's functions
        // report their own file in tracebacks.
        sub.filename = self.filename.clone();
        // Thread the enclosing function scope chain into the child compiler.
        // Since compile_def always produces a function scope, add self.local_index
        // (if self is a function scope) and mark the child as a function scope.
        sub.outer_locals = self.outer_locals.clone();
        if self.is_function_scope {
            sub.outer_locals.push(Rc::clone(&self.local_index));
        }
        sub.is_function_scope = true;
        // Names declared `nonlocal` in this body resolve to an enclosing cell;
        // record them so reads/writes emit LoadCell/StoreCell (issue #2339).
        sub.nonlocal_names = (*inner_nonlocal_rc).clone();
        sub.is_async_function = is_async;
        // An `async def` whose body contains a bare `yield` is an async
        // generator (#2280); `return <value>` inside it is a SyntaxError.
        // Detect it from the body AST here (CPython derives the analogous
        // `ste_generator && ste_coroutine` flag the same way).
        sub.is_async_generator_fn = is_async && stmts_contain_yield(body);
        // Propagate PEP 563 lazy-annotation flag to the inner compiler.
        sub.future_annotations = self.future_annotations;
        // A function compiled directly inside a class body is a class method and
        // gets access to zero-arg super().  Functions compiled inside other
        // functions (nested) do not — they get is_class_method = false (the
        // default from Compiler::new) because self.is_class_body is false there.
        sub.is_class_method = self.is_class_body;
        // Compute the qualname for this function and its `<locals>` prefix.
        // Classes defined inside this function inherit `"fn_name.<locals>"` as
        // their qualname prefix — matching CPython's `"fn_name.<locals>.ClassName"`.
        let fn_qualname = if self.qualname_prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}.{}", self.qualname_prefix, name)
        };
        sub.qualname_prefix = format!("{fn_qualname}.<locals>");
        let has_kwonly_params = params.iter().any(|p| p.is_keyword_only);
        if is_memo_pure && !has_kwonly_params {
            // Seed the inner compiler with the function's own name so that
            // direct self-recursive calls are compiled as CallMemo rather than
            // Call.  This lets the VM return from the fn_cache on repeated
            // invocations without re-entering call_function_expanded at all,
            // making recursive memoizable functions (e.g. fib) substantially
            // faster.
            // Exclude kwonly-param functions: CallMemo keys by raw positional
            // registers and would bypass keyword-only enforcement on self-calls.
            sub.pure_locals.insert(name.to_string());
        }
        // `co_firstlineno`: the `def`/`lambda` line, recorded on the body's
        // FnCode (issue #2185).
        sub.first_lineno = def_lineno;
        sub.compile_block_with_linenos(body, body_linenos);
        let mut inner_code = match sub.finish() {
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
        merge_lexical_free_var_candidates(&mut inner_code, body, &inner_index_rc, &inner_global_rc);

        if self.fn_protos.len() >= u16::MAX as usize {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("too many functions in one scope (max 65535)".to_string());
            }
            return None;
        }
        let proto_idx = self.fn_protos.len() as u16;
        let local_names = Rc::new(inner_index_rc.keys().cloned().collect::<HashSet<_>>());
        // Collect annotation keys: annotated param names (in declaration order) then
        // "return" if there is a return annotation.  These are parallel to the
        // annotation register window emitted just before MakeFunction.
        let annotation_keys: SmallVec<[String; 4]> = params
            .iter()
            .filter(|p| p.annotation.is_some())
            .map(|p| p.name.clone())
            .chain(return_annotation.map(|_| "return".to_string()))
            .collect();
        // Extract docstring: if the first statement in the body is a bare
        // string literal, capture it as the function's __doc__ (CPython parity).
        let fn_docstring = match body {
            [Stmt::Expr(Expr::Str(s)), ..] => Some(s.clone()),
            _ => None,
        };
        let param_spec = Rc::new(FnParamSpec {
            names: params.iter().map(|p| p.name.clone()).collect(),
            has_default: params.iter().map(|p| p.default.is_some()).collect(),
            is_args: params.iter().map(|p| p.is_args).collect(),
            is_kwargs: params.iter().map(|p| p.is_kwargs).collect(),
            is_keyword_only: params.iter().map(|p| p.is_keyword_only).collect(),
            is_positional_only: params.iter().map(|p| p.is_positional_only).collect(),
        });
        let param_binds = Rc::new(crate::bytecode::compute_param_binds(
            &param_spec,
            &inner_index_rc,
            &inner_code.cell_vars,
        ));
        let self_bind =
            crate::bytecode::compute_self_bind(name, &inner_index_rc, &inner_code.cell_vars);
        self.fn_protos.push(FnProto {
            name: Rc::from(name),
            qualname: Rc::from(fn_qualname.as_str()),
            param_spec,
            code: Rc::new(inner_code),
            local_index: inner_index_rc,
            param_binds,
            self_bind,
            local_names,
            global_names: inner_global_rc,
            nonlocal_names: inner_nonlocal_rc,
            is_memo_pure,
            annotation_keys,
            docstring: fn_docstring,
            class_kwarg_names: SmallVec::new(),
        });

        Some((proto_idx, is_memo_pure, has_kwonly_params))
    }

    /// Compile a function's default-value expressions into a contiguous
    /// register window.  Returns `(base, count)`, or `None` on register
    /// overflow (error already recorded).
    fn emit_def_default_values(&mut self, params: &[FunctionParam]) -> Option<(Reg, u32)> {
        // Compile default values (right-to-left in declaration, left-to-right in slots).
        let defaults: Vec<usize> = params
            .iter()
            .enumerate()
            .filter(|(_, p)| p.default.is_some())
            .map(|(i, _)| i)
            .collect();
        let defs_n = match u32::try_from(defaults.len()) {
            Ok(count) => count,
            Err(_) => {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many default values".to_string());
                }
                return None;
            }
        };
        let defs_base = self.next_temp;
        if defs_n > 0 {
            // Reserve slots
            if self.next_temp.checked_add(Reg::from(defs_n)).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many default-value registers".to_string());
                }
                return None;
            }
            self.next_temp += Reg::from(defs_n);
            if self.next_temp - 1 > self.max_reg {
                self.max_reg = self.next_temp - 1;
            }
            for (slot_i, param_i) in (0u32..).zip(defaults.iter()) {
                let def_expr = params[*param_i].default.as_ref().unwrap();
                let saved = self.next_temp;
                let r = self.compile_expr(def_expr);
                if r != defs_base + slot_i {
                    self.emit(Insn::Move(defs_base + slot_i, r));
                }
                self.next_temp = saved;
            }
        }
        Some((defs_base, defs_n))
    }

    /// Compile a function's parameter/return annotation expressions into a
    /// contiguous register window (param annotations in declaration order, then
    /// the return annotation).  Returns `(base, count)`, or `None` on register
    /// overflow (error already recorded).
    fn emit_def_annotation_values(
        &mut self,
        params: &[FunctionParam],
        return_annotation: Option<&Expr>,
    ) -> Option<(Reg, u32)> {
        // Compile annotation expressions (evaluated in enclosing scope, like defaults).
        // Under PEP 563 (`from __future__ import annotations`), emit the annotation
        // source text as a string literal instead of evaluating the expression.
        // Order: annotated params in declaration order, then return annotation.
        let annotated_params: Vec<(usize, &Expr)> = params
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.annotation.as_ref().map(|a| (i, a)))
            .collect();
        let annots_n = match u32::try_from(annotated_params.len())
            .ok()
            .and_then(|count| count.checked_add(u32::from(return_annotation.is_some())))
        {
            Some(count) => count,
            None => {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many annotations".to_string());
                }
                return None;
            }
        };
        let annots_base = self.next_temp;
        if annots_n > 0 {
            if self.next_temp.checked_add(Reg::from(annots_n)).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many annotation registers".to_string());
                }
                return None;
            }
            self.next_temp += Reg::from(annots_n);
            if self.next_temp - 1 > self.max_reg {
                self.max_reg = self.next_temp - 1;
            }
            for (slot_i, (_, annot_expr)) in (0u32..).zip(annotated_params.iter()) {
                let saved = self.next_temp;
                let r = if self.future_annotations {
                    self.compile_literal(Value::string(stringify_annotation(annot_expr)))
                } else {
                    self.compile_expr(annot_expr)
                };
                if r != annots_base + slot_i {
                    self.emit(Insn::Move(annots_base + slot_i, r));
                }
                self.next_temp = saved;
            }
            if let Some(ret_annot) = return_annotation {
                let slot_i = annots_n - 1;
                let saved = self.next_temp;
                let r = if self.future_annotations {
                    self.compile_literal(Value::string(stringify_annotation(ret_annot)))
                } else {
                    self.compile_expr(ret_annot)
                };
                if r != annots_base + slot_i {
                    self.emit(Insn::Move(annots_base + slot_i, r));
                }
                self.next_temp = saved;
            }
        }
        Some((annots_base, annots_n))
    }

    /// Apply a chain of decorators to the value in `dst`: evaluate each
    /// decorator expression top-to-bottom, then apply innermost-first
    /// (`fn = d1(d2(d3(fn)))`).  Returns the register holding the final
    /// decorated value (`dst` when there are no decorators), or `None` on
    /// register overflow (error already recorded).  Shared by `compile_def`
    /// and `compile_class`.
    fn emit_decorator_application(&mut self, decorators: &[Expr], dst: Reg) -> Option<Reg> {
        // Evaluate decorator expressions top-to-bottom, then apply bottom-to-top.
        // CPython evaluates decorators in declaration order (top first) but applies
        // them innermost-first (bottom first): fn = d1(d2(d3(fn))).
        let mut val_reg = dst;
        if !decorators.is_empty() {
            let n = decorators.len() as u32;
            let deco_base = self.next_temp;
            // Need n slots for the callables plus 1 extra arg slot for the first call.
            if deco_base.checked_add(n + 1).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg =
                        Some("too many registers for decorator application".to_string());
                }
                return None;
            }
            // Reserve n + 1 registers (n callables + 1 arg slot for the first application).
            self.next_temp = deco_base + n + 1;
            if deco_base + n > self.max_reg {
                self.max_reg = deco_base + n;
            }
            // Evaluate each decorator expression top-to-bottom into consecutive registers.
            for (i, deco_expr) in decorators.iter().enumerate() {
                let saved = self.next_temp;
                self.compile_expr_into(deco_expr, deco_base + i as u32);
                self.next_temp = saved;
            }
            // Apply decorators bottom-to-top (innermost first).
            for i in (0..n).rev() {
                let frame = deco_base + i;
                // frame+1 is the argument slot; for i == n-1 this is deco_base+n
                // (the extra slot reserved above); for smaller i it reuses the
                // register freed by the previous application result.
                self.emit(Insn::Move(frame + 1, val_reg));
                self.emit(Insn::Call(frame, 1));
                val_reg = frame;
            }
            self.next_temp = deco_base + 1;
        }
        Some(val_reg)
    }

    #[allow(clippy::too_many_arguments)]
    fn compile_def(
        &mut self,
        name: &str,
        params: &[FunctionParam],
        body: &[Stmt],
        body_linenos: &[u32],
        def_lineno: u32,
        decorators: &[Expr],
        return_annotation: Option<&Expr>,
        is_async: bool,
        type_params: &[TypeParam],
    ) {
        if let Some(message) = validate_type_parameter_bounds(type_params) {
            self.set_syntax_error(&message);
            return;
        }

        let (proto_idx, is_memo_pure, has_kwonly_params) = match self.build_def_proto(
            name,
            params,
            body,
            body_linenos,
            def_lineno,
            return_annotation,
            is_async,
        ) {
            Some(v) => v,
            None => return,
        };

        // PEP 695 default values are evaluated in the *enclosing* scope, not the
        // type-parameter scope: a default that references a type parameter
        // (`def g[T](x=T)`) sees the enclosing `T` (or raises NameError if none
        // exists), matching CPython.  Evaluate defaults *before* pushing the
        // type-param environment so they resolve against the enclosing scope.
        let (defs_base, defs_n) = match self.emit_def_default_values(params) {
            Some(v) => v,
            None => return,
        };

        // PEP 695: push a dedicated type-parameter environment, then bind the
        // type parameters (as TypeVar objects) into it *before* the annotations
        // are evaluated, so a parameter or return annotation that references `T`
        // (e.g. `def f[T](x: T) -> T`) resolves.  Binding them in a child env
        // (rather than the enclosing namespace) keeps the parameter names from
        // leaking after the def while the generic function — which captures this
        // env via `MakeFunction` — can still resolve them lazily in its body.
        // The returned register block holds the same TypeVar objects reused for
        // `__type_params__` below to preserve object identity.  The block sits
        // below `dst`, so the `next_temp = dst + 1` watermark reset after
        // `MakeFunction` keeps it live until the tuple is built.
        if !type_params.is_empty() {
            self.emit(Insn::PushTypeParamEnv);
        }
        let (tp_base, tp_n) = self.emit_bind_type_params(type_params);

        let (annots_base, annots_n) =
            match self.emit_def_annotation_values(params, return_annotation) {
                Some(v) => v,
                None => return,
            };

        let dst = self.alloc_temp();
        self.emit(Insn::MakeFunction(
            dst,
            proto_idx,
            defs_base,
            defs_n,
            annots_base,
            annots_n,
        ));
        if defs_n > 0 || annots_n > 0 {
            // Free the temp registers used by defaults and annotations; keep
            // only the function value register (dst) alive from this point.
            // `dst` was allocated after all defaults/annotations, so `dst + 1`
            // is the correct watermark: it preserves the function and releases
            // every slot below it (defaults, annotations).
            //
            // The previous formula (`defs_base + 1` or `annots_base + 1`)
            // was wrong when exactly one default or annotation was present:
            // defs_base + 1 == dst, so the subsequent decorator-base
            // allocation used the same register as dst, overwriting the
            // freshly created function with the decorator value (issue #1362).
            self.next_temp = dst + 1;
        }

        // PEP 695: if this is a generic function, build the __type_params__ tuple
        // and store it on the function object before decorators are applied.
        // CPython sets __type_params__ on the raw function, before wrapping it
        // with decorators (verified: the decorator receives a function that already
        // has __type_params__).  Reuse the TypeVar registers bound above so the
        // objects in __type_params__ are identical to those seen in annotations.
        if tp_n > 0 {
            self.emit_type_params_attr_from_regs(dst, tp_base, tp_n);
        }

        // PEP 695: pop the type-parameter environment before decorators run and
        // before the def name is bound — decorators and the binding belong to the
        // enclosing scope (a decorator referencing `T` must see the enclosing
        // `T`, not the type parameter).
        if !type_params.is_empty() {
            self.emit(Insn::PopTypeParamEnv);
        }

        let val_reg = match self.emit_decorator_application(decorators, dst) {
            Some(r) => r,
            None => return,
        };

        self.compile_store_name(name, val_reg);
        if let Some(reg) = self.local_reg(name) {
            self.mark_def(reg);
        }
        // Exclude kwonly-param functions from CallMemo optimisation.
        // CallMemo keys by raw positional arg values; a kwarg-based call stores
        // a cache entry that an invalid positional-only call could match, bypassing
        // keyword-only enforcement in call_user_function_expanded.
        // Memo-purity gates `CallMemo` emission so the VM result cache stays
        // active. Record memo-pure names so later call sites in this enclosing
        // compiler can emit `CallMemo` for the stable local binding. Sibling
        // purity analysis deliberately does not consume this set.
        if decorators.is_empty() && !has_kwonly_params && is_memo_pure {
            self.pure_locals.insert(name.to_string());
        }
        self.free_temp(dst);
    }
}
