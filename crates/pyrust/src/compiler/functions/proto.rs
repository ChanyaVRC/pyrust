impl Compiler {
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
        type_param_names: &[Symbol],
    ) -> Option<(u16, bool, bool)> {
        // Build inner function's scope metadata.
        let inner_global = crate::interpreter::collect_global_names(body);
        let inner_nonlocal = crate::interpreter::collect_nonlocal_names(body);

        // A parameter may not also be declared `global`/`nonlocal` in the body.
        // CPython 3.12 raises `SyntaxError: name 'x' is parameter and global`
        // (resp. `... and nonlocal`).  This conflict wins over the later
        // ordering / annotation / no-binding diagnostics, so check it first.
        for p in params {
            if inner_global.contains(p.name.as_str()) {
                self.set_syntax_error(&format!("name '{}' is parameter and global", p.name));
                return None;
            }
            if inner_nonlocal.contains(p.name.as_str()) {
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
            if inner_local.contains(param.name.as_str()) {
                inner_index.insert(param.name.to_string(), slot);
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

        let mut sub = Compiler::new(Rc::clone(&inner_index_rc), def_bound, inner_cell_vars);
        // Threaded source file (#2438): the nested function's code object shares
        // its enclosing scope's `co_filename`.
        sub.filename = self.filename.clone();
        sub.outer_locals = self.outer_locals.clone();
        if self.is_function_scope {
            sub.outer_locals.push(Rc::clone(&self.local_index));
        }
        sub.is_function_scope = true;
        // Initialize compile-time indexed cell slots (issues #452 / #2339 / #2348).
        let enclosing_cells = self.child_enclosing_cell_names(type_param_names);
        sub.init_function_cells(enclosing_cells, &inner_global_rc, &inner_nonlocal_rc);
        sub.is_async_function = is_async;
        // An `async def` whose body contains a bare `yield` is an async
        // generator (#2280); `return <value>` inside it is a SyntaxError.
        sub.is_async_generator_fn = is_async && stmts_contain_yield(body);
        sub.future_annotations = self.future_annotations;
        // A function compiled directly inside a class body is a class method
        // and gets access to zero-arg super().
        sub.is_class_method = self.is_class_body;
        let fn_qualname = if self.qualname_prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}.{}", self.qualname_prefix, name)
        };
        sub.qualname = std::sync::Arc::from(fn_qualname.as_str());
        sub.qualname_prefix = format!("{fn_qualname}.<locals>");
        let has_kwonly_params = params.iter().any(|p| p.is_keyword_only);
        if is_memo_pure && !has_kwonly_params {
            sub.pure_locals.insert(name.to_string());
        }
        // `co_firstlineno`: the `def`/`lambda` line (issue #2185).
        sub.first_lineno = def_lineno;
        sub.compile_block_with_linenos(body, body_linenos);
        sub.ensure_lexical_free_cells(body);
        let mut inner_code = match sub.finish() {
            Ok(c) => c,
            Err(e) => {
                self.failed = true;
                if matches!(e.kind(), crate::error::PyErrorKind::Named(cls, _) if cls.as_ref() == "SyntaxError")
                {
                    self.is_syntax_error = true;
                }
                if self.error_msg.is_none() {
                    self.error_msg = Some(match e.into_kind() {
                        crate::error::PyErrorKind::Named(_, msg)
                        | crate::error::PyErrorKind::Runtime(msg) => msg,
                        other => other.to_string(),
                    });
                }
                return None;
            }
        };
        merge_lexical_free_var_candidates(&mut inner_code, body, &inner_index_rc, &inner_global_rc);
        let closure_capture_map = self.build_capture_map(&inner_code);

        if self.fn_protos.len() >= u16::MAX as usize {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("too many functions in one scope (max 65535)".to_string());
            }
            return None;
        }
        let proto_idx = self.fn_protos.len() as u16;
        let local_names = Rc::new(inner_index_rc.keys().cloned().collect::<HashSet<_>>());
        // Collect annotation keys: annotated param names (in declaration order)
        // then "return" if there is a return annotation.
        let annotation_keys: SmallVec<[Symbol; 4]> = params
            .iter()
            .filter(|p| p.annotation.is_some())
            .map(|p| p.name)
            .chain(return_annotation.map(|_| Symbol::intern("return")))
            .collect();
        // Extract docstring: if the first statement in the body is a bare
        // string literal, capture it as the function's __doc__ (CPython parity).
        let fn_docstring = match body {
            [Stmt::Expr(Expr::Str(s)), ..] => Some(s.clone()),
            _ => None,
        };
        let param_spec = Rc::new(FnParamSpec {
            names: params.iter().map(|p| p.name).collect(),
            has_default: params.iter().map(|p| p.default.is_some()).collect(),
            is_args: params.iter().map(|p| p.is_args).collect(),
            is_kwargs: params.iter().map(|p| p.is_kwargs).collect(),
            is_keyword_only: params.iter().map(|p| p.is_keyword_only).collect(),
            is_positional_only: params.iter().map(|p| p.is_positional_only).collect(),
        });
        let param_binds = Rc::new(crate::bytecode::compute_param_binds(
            &param_spec,
            &inner_index_rc,
            inner_code.own_cell_slots(),
        ));
        // Do not overwrite a parameter that happens to share the function's own
        // name (e.g. `def repeat(self, repeat=5): ...` in `timeit.Timer`).
        let self_bind = if param_spec.names.iter().any(|p| p.as_str() == name) {
            None
        } else {
            crate::bytecode::compute_self_bind(name, &inner_index_rc, inner_code.own_cell_slots())
        };
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
            closure_capture_map,
        });

        Some((proto_idx, is_memo_pure, has_kwonly_params))
    }
}
