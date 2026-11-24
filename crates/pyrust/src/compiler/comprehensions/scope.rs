struct ComprehensionScopeBindings {
    locals: indexmap::IndexSet<String>,
    globals: HashSet<String>,
    nonlocals: HashSet<String>,
}

struct CompFnConfig<'a> {
    iter_reg: Reg,
    fn_body: &'a [Stmt],
    body_linenos: &'a [u32],
    display_name: &'a str,
    qualname: Option<&'a str>,
    is_async: bool,
    is_inlined_comp: bool,
    is_set_comp: bool,
    is_list_comp: bool,
    list_comp_presize: bool,
    first_lineno: u32,
    call_span: Option<crate::ast::CaretSpan>,
    comp_outer_for_span: Option<crate::ast::CaretSpan>,
    await_result: bool,
}

impl Compiler {
    /// Whether a comprehension is asynchronous due to an `await` in its body
    /// (#2304), independent of any `async for` clause. `elts` are the element
    /// expression(s) — one for list/set/gen, two (key, value) for dict.
    ///
    /// Scoping (matches CPython): the OUTERMOST iterable (`clauses[0].iter`) is
    /// evaluated in the ENCLOSING scope, so an `await` there belongs to the
    /// enclosing frame and does NOT make the comprehension async. We therefore
    /// inspect only the element(s), every clause `cond`, and the
    /// non-outermost clause iterables (`clauses[1..]`).
    fn comp_body_has_await(elts: &[&Expr], clauses: &[CompClause]) -> bool {
        // An `await` directly in the element/cond/non-outermost-iterable makes
        // this comprehension async; a nested async list/set/dict comprehension
        // in the same positions also does (CPython propagates async-ness from a
        // directly-nested COLLECTION comprehension outward — the outer body has
        // to await the inner comp's coroutine; issue #2312).  The outermost
        // iterable (`clauses[0].iter`) is excluded — it is evaluated in the
        // ENCLOSING scope, so an async comp there makes the *enclosing* function
        // async, not this one.
        elts.iter()
            .any(|e| expr_contains_await(e) || expr_has_async_collection_comp(e))
            || clauses.iter().any(|c| {
                c.cond.as_ref().is_some_and(|cond| {
                    expr_contains_await(cond) || expr_has_async_collection_comp(cond)
                })
            })
            || clauses[1..]
                .iter()
                .any(|c| expr_contains_await(&c.iter) || expr_has_async_collection_comp(&c.iter))
    }

    /// Shared validation for collection comprehensions and generator expressions.
    /// Returns `Some(is_async)` when valid, or `None` after recording the error.
    fn validate_comprehension_clauses(
        &mut self,
        elts: &[&Expr],
        clauses: &[CompClause],
        kind: &str,
        require_enclosing_async: bool,
    ) -> Option<bool> {
        if clauses.is_empty() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some(format!("{kind} requires at least one clause"));
            }
            return None;
        }
        let is_async =
            clauses.iter().any(|c| c.is_async) || Self::comp_body_has_await(elts, clauses);
        if let Some(msg) = check_comprehension(elts, clauses, kind) {
            self.set_syntax_error(&msg);
            return None;
        }
        if require_enclosing_async && is_async && !self.is_async_function {
            self.set_syntax_error("asynchronous comprehension outside of an asynchronous function");
            return None;
        }
        Some(is_async)
    }

    /// Build the loop-nest AST shared by list/set/dict comprehensions.
    ///
    /// Returns a `Vec<Stmt>` representing the `for`/`if` structure that wraps
    /// `innermost`.  The outermost `for` iterates over `IT_PARAM` (the implicit
    /// parameter that receives the outermost iterable from the enclosing scope).
    fn build_comp_loop_body(clauses: &[CompClause], innermost: Stmt) -> Vec<Stmt> {
        Self::build_comp_loop_body_with_linenos(clauses, innermost, 0).0
    }

    /// Build the loop-nest AST along with parallel 1-based statement line
    /// numbers so generator expressions carry accurate `tb_lineno` and PEP 657
    /// caret anchors across multi-line elements, clauses, and chained `if`
    /// filters (issue #2992).
    fn build_comp_loop_body_with_linenos(
        clauses: &[CompClause],
        innermost: Stmt,
        innermost_lineno: u32,
    ) -> (Vec<Stmt>, Vec<u32>) {
        const IT_PARAM: &str = ".0";

        let wrap_filters = |clause: &CompClause,
                            mut body: Vec<Stmt>,
                            mut body_linenos: Vec<u32>|
         -> (Vec<Stmt>, Vec<u32>) {
            if !clause.cond_filters.is_empty() {
                for (filter, filter_lineno) in clause.cond_filters.iter().rev() {
                    body = vec![Stmt::If {
                        branches: vec![(filter.clone(), body)],
                        else_branch: None,
                        branch_linenos: vec![body_linenos],
                        else_linenos: vec![],
                    }];
                    body_linenos = vec![*filter_lineno];
                }
            } else if let Some(cond) = &clause.cond {
                body = vec![Stmt::If {
                    branches: vec![(cond.clone(), body)],
                    else_branch: None,
                    branch_linenos: vec![body_linenos],
                    else_linenos: vec![],
                }];
                body_linenos = vec![clause.for_lineno];
            }
            (body, body_linenos)
        };

        let mut body = vec![innermost];
        let mut body_linenos = vec![innermost_lineno];
        for clause in clauses[1..].iter().rev() {
            (body, body_linenos) = wrap_filters(clause, body, body_linenos);
            body = vec![Stmt::For {
                target: clause.target.clone(),
                iter: clause.iter.clone(),
                body,
                else_branch: None,
                body_linenos,
                else_linenos: vec![],
                // `async for` clause (#2283): lowers to compile_async_for.
                is_async: clause.is_async,
            }];
            body_linenos = vec![clause.for_lineno];
        }
        (body, body_linenos) = wrap_filters(&clauses[0], body, body_linenos);
        body = vec![Stmt::For {
            target: clauses[0].target.clone(),
            iter: Expr::Var(Symbol::intern(IT_PARAM), None),
            body,
            else_branch: None,
            body_linenos,
            else_linenos: vec![],
            is_async: clauses[0].is_async,
        }];
        let outer_linenos = vec![clauses[0].for_lineno];
        (body, outer_linenos)
    }

    /// Resolve bindings for the implicit function shared by collection
    /// comprehensions and generator expressions.
    ///
    /// PEP 572 assignment-expression targets skip every comprehension scope:
    /// they become nonlocals when an enclosing function owns the name, and
    /// globals otherwise. Keeping this classification here prevents the two
    /// compilation paths from silently diverging.
    fn analyze_comprehension_scope(
        &mut self,
        params: &[FunctionParam],
        body: &[Stmt],
    ) -> Option<ComprehensionScopeBindings> {
        let mut walrus_targets = HashSet::new();
        Self::collect_walrus_targets_in_stmts(body, &mut walrus_targets);

        let walrus_nonlocals: HashSet<String> = walrus_targets
            .iter()
            .filter(|name| {
                self.outer_locals
                    .iter()
                    .any(|scope| scope.contains_key(*name))
                    || (self.is_function_scope && self.local_index.contains_key(*name))
            })
            .cloned()
            .collect();
        let walrus_globals: HashSet<String> = walrus_targets
            .difference(&walrus_nonlocals)
            .cloned()
            .collect();

        let mut globals = crate::interpreter::collect_global_names(body);
        globals.extend(walrus_globals);
        let mut nonlocals = crate::interpreter::collect_nonlocal_names(body);
        nonlocals.extend(walrus_nonlocals);

        let raw_locals =
            crate::interpreter::collect_local_names(params, body, &globals, &nonlocals);
        let locals = raw_locals
            .into_iter()
            .filter(|name| !walrus_targets.contains(name))
            .collect();

        let mut sorted_nonlocals: Vec<&String> = nonlocals.iter().collect();
        sorted_nonlocals.sort();
        for name in sorted_nonlocals {
            let found = self
                .outer_locals
                .iter()
                .any(|scope| scope.contains_key(name))
                || (self.is_function_scope && self.local_index.contains_key(name));
            if !found {
                self.set_syntax_error(&format!("no binding for nonlocal '{name}' found"));
                return None;
            }
        }

        Some(ComprehensionScopeBindings {
            locals,
            globals,
            nonlocals,
        })
    }

    /// Shared implicit-function lowering for both collection comprehensions
    /// (list/set/dict) and generator expressions.
    fn compile_comprehension_fn(&mut self, cfg: CompFnConfig<'_>) -> Reg {
        const IT_PARAM: &str = ".0";

        let params = vec![FunctionParam {
            name: Symbol::intern(IT_PARAM),
            default: None,
            annotation: None,
            is_args: false,
            is_kwargs: false,
            is_keyword_only: false,
            is_positional_only: false,
        }];

        let Some(scope) = self.analyze_comprehension_scope(&params, cfg.fn_body) else {
            return 0;
        };

        let mut inner_index: HashMap<String, Reg> = HashMap::new();
        let mut slot: Reg = 0;
        for param in &params {
            if scope.locals.contains(param.name.as_str()) {
                inner_index.insert(param.name.to_string(), slot);
                slot += 1;
            }
        }
        for loc in &scope.locals {
            if !inner_index.contains_key(loc) {
                inner_index.insert(loc.clone(), slot);
                slot += 1;
            }
        }
        let inner_index_rc: Rc<HashMap<String, Reg>> = Rc::new(inner_index);
        let def_bound = crate::interpreter::compute_def_bound_mask(&params, &inner_index_rc);
        let inner_cell_vars = collect_cell_vars(cfg.fn_body, &inner_index_rc);
        let inner_global_rc = Rc::new(scope.globals);
        let inner_nonlocal_rc = Rc::new(scope.nonlocals);

        let mut sub = Compiler::new(Rc::clone(&inner_index_rc), def_bound, inner_cell_vars);
        // Threaded source file (#2438): the implicit code object inherits the
        // enclosing scope's `co_filename`.
        sub.filename = self.filename.clone();
        if let Some(qn) = cfg.qualname {
            sub.qualname = std::sync::Arc::from(qn);
            sub.qualname_prefix = qn.to_string();
        }
        sub.outer_locals = self.outer_locals.clone();
        if self.is_function_scope {
            sub.outer_locals.push(Rc::clone(&self.local_index));
        }
        sub.is_function_scope = true;
        sub.future_annotations = self.future_annotations;
        sub.is_async_function = cfg.is_async;
        sub.is_set_comp = cfg.is_set_comp;
        sub.is_list_comp = cfg.is_list_comp;
        sub.list_comp_presize = cfg.list_comp_presize;
        sub.is_inlined_comp = cfg.is_inlined_comp;
        sub.first_lineno = cfg.first_lineno;
        sub.set_lineno(cfg.first_lineno);
        sub.comp_outer_for_span = cfg.comp_outer_for_span;
        if cfg.is_inlined_comp {
            // Record the locals of the comp's immediately-enclosing *real*
            // function (the PEP 709 inlining target) so the VM can tell an
            // unbound read of one of that frame's locals (`UnboundLocalError`)
            // apart from a free variable owned by a grandparent scope
            // (`NameError`) — issue #2457.
            sub.comp_enclosing_locals = if self.is_inlined_comp {
                self.comp_enclosing_locals.clone()
            } else if self.is_function_scope {
                Some(Rc::new(
                    self.local_index.keys().map(|k| Symbol::intern(k)).collect(),
                ))
            } else {
                None
            };
        }
        let enclosing_cells = self.child_enclosing_cell_names(&[]);
        sub.init_function_cells(enclosing_cells, &inner_global_rc, &inner_nonlocal_rc);
        sub.compile_block_with_linenos(cfg.fn_body, cfg.body_linenos);
        sub.ensure_lexical_free_cells(cfg.fn_body);
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
                return 0;
            }
        };
        merge_lexical_free_var_candidates(
            &mut inner_code,
            cfg.fn_body,
            &inner_index_rc,
            &inner_global_rc,
        );
        let closure_capture_map = self.build_capture_map(&inner_code);

        if self.fn_protos.len() >= u16::MAX as usize {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("too many functions in one scope (max 65535)".to_string());
            }
            return 0;
        }
        let proto_idx = self.fn_protos.len() as u16;
        let local_names = Rc::new(inner_index_rc.keys().cloned().collect::<HashSet<_>>());
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
        let self_bind = crate::bytecode::compute_self_bind(
            cfg.display_name,
            &inner_index_rc,
            inner_code.own_cell_slots(),
        );
        let proto_qualname = cfg.qualname.unwrap_or(cfg.display_name);
        self.fn_protos.push(FnProto {
            name: Rc::from(cfg.display_name),
            qualname: Rc::from(proto_qualname),
            param_spec,
            code: Rc::new(inner_code),
            local_index: inner_index_rc,
            param_binds,
            self_bind,
            local_names,
            global_names: inner_global_rc,
            nonlocal_names: inner_nonlocal_rc,
            // Lambdas / comprehensions are never pure (fresh-closure identity).
            is_memo_pure: false,
            annotation_keys: SmallVec::new(),
            docstring: None,
            class_kwarg_names: SmallVec::new(),
            closure_capture_map,
        });

        // Emit MakeFunction + Call: fn_reg = function, fn_reg+1 = iterable arg.
        let fn_reg = self.alloc_temp();
        self.emit(Insn::MakeFunction(fn_reg, proto_idx, 0, 0, 0, 0));

        let arg_reg = fn_reg + 1;
        if arg_reg > self.max_reg {
            self.max_reg = arg_reg;
        }
        if fn_reg + 2 > self.next_temp {
            self.next_temp = fn_reg + 2;
        }
        self.emit(Insn::Move(arg_reg, cfg.iter_reg));
        if cfg.call_span.is_some() {
            self.set_col_span_for_next(cfg.call_span);
        }
        self.emit(Insn::Call(fn_reg, 1));
        self.next_temp = fn_reg + 1;
        self.free_temp(cfg.iter_reg);

        if cfg.await_result {
            // The call produced a coroutine; the enclosing async frame awaits it
            // to materialise the collection (#2283).  A distinct result register
            // above `fn_reg` (the awaited source) receives the awaited value.
            let result_reg = self.alloc_temp();
            self.emit_await_drive_into_with_span(fn_reg, result_reg, cfg.call_span);
            self.next_temp = result_reg + 1;
            return result_reg;
        }

        fn_reg
    }
}
