struct ComprehensionScopeBindings {
    locals: indexmap::IndexSet<String>,
    globals: HashSet<String>,
    nonlocals: HashSet<String>,
}

impl Compiler {
    // ── Comprehension compilation ──────────────────────────────────────────────

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

    /// Build the loop-nest AST shared by list/set/dict comprehensions.
    ///
    /// Returns a `Vec<Stmt>` representing the `for`/`if` structure that wraps
    /// `innermost`.  The outermost `for` iterates over `IT_PARAM` (the implicit
    /// parameter that receives the outermost iterable from the enclosing scope).
    fn build_comp_loop_body(clauses: &[CompClause], innermost: Stmt) -> Vec<Stmt> {
        const IT_PARAM: &str = ".0";

        let mut body = vec![innermost];
        for clause in clauses[1..].iter().rev() {
            if let Some(cond) = &clause.cond {
                body = vec![Stmt::If {
                    branches: vec![(cond.clone(), body)],
                    else_branch: None,
                    branch_linenos: vec![],
                    else_linenos: vec![],
                }];
            }
            body = vec![Stmt::For {
                target: clause.target.clone(),
                iter: clause.iter.clone(),
                body,
                else_branch: None,
                body_linenos: vec![],
                else_linenos: vec![],
                // `async for` clause (#2283): lowers to compile_async_for.
                is_async: clause.is_async,
            }];
        }
        if let Some(cond) = &clauses[0].cond {
            body = vec![Stmt::If {
                branches: vec![(cond.clone(), body)],
                else_branch: None,
                branch_linenos: vec![],
                else_linenos: vec![],
            }];
        }
        body = vec![Stmt::For {
            target: clauses[0].target.clone(),
            iter: Expr::Var(IT_PARAM.to_string(), None),
            body,
            else_branch: None,
            body_linenos: vec![],
            else_linenos: vec![],
            is_async: clauses[0].is_async,
        }];
        body
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

    /// Compile a comprehension (list/set/dict) as a nested implicit function,
    /// mirroring CPython's scope-isolation behaviour.
    ///
    /// `iter_reg`   — register holding the outermost iterable (already compiled
    ///                in the enclosing scope by the caller).
    /// `fn_body`    — complete body of the inner function (init + loops + return).
    /// `comp_name`  — display name used in the `FnProto` ("listcomp", etc.).
    ///
    /// Returns the result register (holds the constructed collection after
    /// the implicit call returns).
    fn compile_collection_comp_impl(
        &mut self,
        iter_reg: Reg,
        fn_body: Vec<Stmt>,
        comp_name: &str,
        is_async: bool,
        presize_acc: bool,
    ) -> Reg {
        const IT_PARAM: &str = ".0";

        let params = vec![FunctionParam {
            name: IT_PARAM.to_string(),
            default: None,
            annotation: None,
            is_args: false,
            is_kwargs: false,
            is_keyword_only: false,
            is_positional_only: false,
        }];

        let Some(scope) = self.analyze_comprehension_scope(&params, &fn_body) else {
            return 0;
        };

        let mut inner_index: HashMap<String, Reg> = HashMap::new();
        let mut slot: Reg = 0;
        for param in &params {
            if scope.locals.contains(&param.name) {
                inner_index.insert(param.name.clone(), slot);
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
        let inner_cell_vars = collect_cell_vars(&fn_body, &inner_index_rc);
        let inner_global_rc = Rc::new(scope.globals);
        let inner_nonlocal_rc = Rc::new(scope.nonlocals);

        let mut sub = Compiler::new(Rc::clone(&inner_index_rc), def_bound, inner_cell_vars);
        // Threaded source file (#2438): a lambda's code object inherits the
        // enclosing scope's `co_filename`.
        sub.filename = self.filename.clone();
        sub.outer_locals = self.outer_locals.clone();
        if self.is_function_scope {
            sub.outer_locals.push(Rc::clone(&self.local_index));
        }
        sub.is_function_scope = true;
        sub.future_annotations = self.future_annotations;
        // An async comprehension (`[x async for x in ait]`, #2283) compiles its
        // implicit function as a coroutine: the `async for` clauses lower to the
        // `__aiter__`/`await __anext__()` drive, and the enclosing async frame
        // awaits the returned coroutine (below).  No bare `yield` is emitted, so
        // `is_generator` stays false and this is a plain coroutine, not an async
        // generator (only `compile_gen_exp` produces async generators).
        sub.is_async_function = is_async;
        // Set comprehensions lower `.acc.add(elt)` to `Insn::SetAdd` (issue #1861).
        sub.is_set_comp = comp_name == "setcomp";
        // List comprehensions lower `.acc.append(elt)` to `Insn::ListAppend` (issue #1862).
        sub.is_list_comp = comp_name == "listcomp";
        // Only a single-clause, unconditional, non-async list comp reaches here
        // with `presize_acc` — see `compile_list_comp`.
        sub.list_comp_presize = presize_acc;
        // list / set / dict comprehensions are CPython-3.12-inlined scopes; an
        // unbound enclosing-local read inside them is `UnboundLocalError`, not the
        // free-variable `NameError` (issue #2340).  Generator expressions
        // (`compile_gen_exp`) are a real separate frame and stay free.
        sub.is_inlined_comp = true;
        // Record the locals of the comp's immediately-enclosing *real* function
        // (the PEP 709 inlining target) so the VM can tell an unbound read of one
        // of that frame's locals (`UnboundLocalError`) apart from a free variable
        // owned by a grandparent scope (`NameError`) — issue #2457.  When this
        // compiler is itself an inlined comp, the inlining target is the real
        // function further up, whose locals we already captured; inherit it.
        sub.comp_enclosing_locals = if self.is_inlined_comp {
            self.comp_enclosing_locals.clone()
        } else if self.is_function_scope {
            Some(Rc::new(self.local_index.keys().cloned().collect()))
        } else {
            None
        };
        sub.compile_block(&fn_body);
        let inner_code = match sub.finish() {
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

        if self.fn_protos.len() >= u16::MAX as usize {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("too many functions in one scope (max 65535)".to_string());
            }
            return 0;
        }
        let proto_idx = self.fn_protos.len() as u16;
        let local_names = Rc::new(inner_index_rc.keys().cloned().collect::<HashSet<_>>());
        let display = format!("<{}>", comp_name);
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
            crate::bytecode::compute_self_bind(&display, &inner_index_rc, &inner_code.cell_vars);
        self.fn_protos.push(FnProto {
            name: Rc::from(display.as_str()),
            qualname: Rc::from(display.as_str()),
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
        });

        // Emit MakeFunction + Call, same layout as compile_gen_exp.
        let fn_reg = self.alloc_temp();
        self.emit(Insn::MakeFunction(fn_reg, proto_idx, 0, 0, 0, 0));

        let arg_reg = fn_reg + 1;
        if arg_reg > self.max_reg {
            self.max_reg = arg_reg;
        }
        if fn_reg + 2 > self.next_temp {
            self.next_temp = fn_reg + 2;
        }
        self.emit(Insn::Move(arg_reg, iter_reg));
        self.emit(Insn::Call(fn_reg, 1));
        self.next_temp = fn_reg + 1;
        self.free_temp(iter_reg);

        if is_async {
            // The call produced a coroutine; the enclosing async frame awaits it
            // to materialise the collection (#2283).  A distinct result register
            // above `fn_reg` (the awaited source) receives the awaited value —
            // same register discipline as `compile_async_with`'s `__aenter__`
            // drive; the `GetAwaitable`/`YieldFrom` scratch temps sit above it.
            let result_reg = self.alloc_temp();
            self.emit_await_drive_into(fn_reg, result_reg);
            self.next_temp = result_reg + 1;
            return result_reg;
        }

        fn_reg
    }

    fn compile_list_comp(&mut self, elt: &Expr, clauses: &[CompClause]) -> Reg {
        if clauses.is_empty() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg =
                    Some("list comprehension requires at least one clause".to_string());
            }
            return 0;
        }
        let is_async =
            clauses.iter().any(|c| c.is_async) || Self::comp_body_has_await(&[elt], clauses);
        if let Some(msg) = check_comprehension(&[elt], clauses, "list comprehension") {
            self.set_syntax_error(&msg);
            return 0;
        }
        if is_async && !self.is_async_function {
            self.set_syntax_error("asynchronous comprehension outside of an asynchronous function");
            return 0;
        }

        const ACC_NAME: &str = ".acc";

        // Evaluate the outermost iterable in the enclosing scope.
        let iter_reg = self.compile_expr(&clauses[0].iter);

        // Build the innermost statement: .acc.append(elt)
        let innermost = Stmt::Expr(Expr::Call {
            span: None,
            func: Box::new(Expr::Attr {
                target: Box::new(Expr::Var(ACC_NAME.to_string(), None)),
                name: "append".to_string(),
                span: None,
            }),
            args: vec![CallArg {
                name: None,
                value: elt.clone(),
                splat: false,
                double_splat: false,
            }],
        });

        // Build loop nest around the innermost statement.
        let mut fn_body = vec![Stmt::Assign(
            AssignTarget::Name(ACC_NAME.to_string()),
            Expr::List(vec![]),
        )];
        fn_body.extend(Self::build_comp_loop_body(clauses, innermost));
        fn_body.push(Stmt::Return(Some(Expr::Var(ACC_NAME.to_string(), None))));

        // Pre-size the accumulator when the produced element count is exactly the
        // source length: a single clause with no `if` condition and not async.
        // With a condition or a second `for`, the count is unknown, so we cannot
        // reserve.  The reserve reads the source length from `.0` without running
        // user code (see `list_reserve_hint`), so it never changes semantics.
        let presize = clauses.len() == 1 && clauses[0].cond.is_none() && !is_async;
        self.compile_collection_comp_impl(iter_reg, fn_body, "listcomp", is_async, presize)
    }

    fn compile_dict_comp(&mut self, key: &Expr, val: &Expr, clauses: &[CompClause]) -> Reg {
        if clauses.is_empty() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg =
                    Some("dict comprehension requires at least one clause".to_string());
            }
            return 0;
        }
        let is_async =
            clauses.iter().any(|c| c.is_async) || Self::comp_body_has_await(&[key, val], clauses);
        if let Some(msg) = check_comprehension(&[key, val], clauses, "dict comprehension") {
            self.set_syntax_error(&msg);
            return 0;
        }
        if is_async && !self.is_async_function {
            self.set_syntax_error("asynchronous comprehension outside of an asynchronous function");
            return 0;
        }

        const ACC_NAME: &str = ".acc";

        let iter_reg = self.compile_expr(&clauses[0].iter);

        // Build the innermost statement: .acc[key] = val
        let innermost = Stmt::IndexAssign {
            target: Box::new(Expr::Var(ACC_NAME.to_string(), None)),
            index: Box::new(key.clone()),
            expr: val.clone(),
        };

        let mut fn_body = vec![Stmt::Assign(
            AssignTarget::Name(ACC_NAME.to_string()),
            Expr::Dict(vec![]),
        )];
        fn_body.extend(Self::build_comp_loop_body(clauses, innermost));
        fn_body.push(Stmt::Return(Some(Expr::Var(ACC_NAME.to_string(), None))));

        self.compile_collection_comp_impl(iter_reg, fn_body, "dictcomp", is_async, false)
    }

    /// When compiling the implicit function body of a set comprehension
    /// (`self.is_set_comp`), recognize the synthesized accumulator add
    /// `.acc.add(elt)` and lower it directly to `Insn::SetAdd(acc, elt)`,
    /// skipping attribute resolution + method-call dispatch (issue #1861).
    ///
    /// Returns `true` when it handled `expr` (matching the exact synthesized
    /// shape: a positional-only one-arg call to `.add` on the reserved `.acc`
    /// accumulator name). The `.acc` name is compiler-internal and cannot
    /// appear in user source, so this never intercepts a user `x.add(y)`.
    fn try_emit_set_comp_add(&mut self, expr: &Expr) -> bool {
        if !self.is_set_comp {
            return false;
        }
        let Expr::Call { func, args, .. } = expr else {
            return false;
        };
        let Expr::Attr { target, name, .. } = func.as_ref() else {
            return false;
        };
        if name != "add" || !matches!(target.as_ref(), Expr::Var(v, _) if v == ".acc") {
            return false;
        }
        if args.len() != 1 {
            return false;
        }
        let arg = &args[0];
        if arg.name.is_some() || arg.splat || arg.double_splat {
            return false;
        }
        let acc_reg = self.compile_expr(target);
        let elt_reg = self.compile_expr(&arg.value);
        self.emit(Insn::SetAdd(acc_reg, elt_reg));
        self.free_temp(elt_reg);
        self.free_temp(acc_reg);
        true
    }

    /// When compiling the implicit function body of a list comprehension
    /// (`self.is_list_comp`), recognize the synthesized accumulator append
    /// `.acc.append(elt)` and lower it directly to `Insn::ListAppend(acc, elt)`,
    /// skipping attribute resolution + method-call dispatch (issue #1862).
    ///
    /// Returns `true` when it handled `expr` (matching the exact synthesized
    /// shape: a positional-only one-arg call to `.append` on the reserved
    /// `.acc` accumulator name). The `.acc` name is compiler-internal and
    /// cannot appear in user source, so this never intercepts a user
    /// `x.append(y)`.
    fn try_emit_list_comp_append(&mut self, expr: &Expr) -> bool {
        if !self.is_list_comp {
            return false;
        }
        let Expr::Call { func, args, .. } = expr else {
            return false;
        };
        let Expr::Attr { target, name, .. } = func.as_ref() else {
            return false;
        };
        if name != "append" || !matches!(target.as_ref(), Expr::Var(v, _) if v == ".acc") {
            return false;
        }
        if args.len() != 1 {
            return false;
        }
        let arg = &args[0];
        if arg.name.is_some() || arg.splat || arg.double_splat {
            return false;
        }
        let acc_reg = self.compile_expr(target);
        let elt_reg = self.compile_expr(&arg.value);
        self.emit(Insn::ListAppend(acc_reg, elt_reg));
        self.free_temp(elt_reg);
        self.free_temp(acc_reg);
        true
    }

    fn compile_set_comp(&mut self, elt: &Expr, clauses: &[CompClause]) -> Reg {
        if clauses.is_empty() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("set comprehension requires at least one clause".to_string());
            }
            return 0;
        }
        let is_async =
            clauses.iter().any(|c| c.is_async) || Self::comp_body_has_await(&[elt], clauses);
        if let Some(msg) = check_comprehension(&[elt], clauses, "set comprehension") {
            self.set_syntax_error(&msg);
            return 0;
        }
        if is_async && !self.is_async_function {
            self.set_syntax_error("asynchronous comprehension outside of an asynchronous function");
            return 0;
        }

        const ACC_NAME: &str = ".acc";

        let iter_reg = self.compile_expr(&clauses[0].iter);

        // Build the innermost statement: .acc.add(elt)
        let innermost = Stmt::Expr(Expr::Call {
            span: None,
            func: Box::new(Expr::Attr {
                target: Box::new(Expr::Var(ACC_NAME.to_string(), None)),
                name: "add".to_string(),
                span: None,
            }),
            args: vec![CallArg {
                name: None,
                value: elt.clone(),
                splat: false,
                double_splat: false,
            }],
        });

        // Build the accumulator: .acc = set()
        let acc_init = Expr::Call {
            span: None,
            func: Box::new(Expr::Var("set".to_string(), None)),
            args: vec![],
        };
        let mut fn_body = vec![Stmt::Assign(
            AssignTarget::Name(ACC_NAME.to_string()),
            acc_init,
        )];
        fn_body.extend(Self::build_comp_loop_body(clauses, innermost));
        fn_body.push(Stmt::Return(Some(Expr::Var(ACC_NAME.to_string(), None))));

        self.compile_collection_comp_impl(iter_reg, fn_body, "setcomp", is_async, false)
    }

    /// Compile a generator expression `(elt for target in iter ...)`.
    ///
    /// Strategy (mirrors CPython):
    ///   1. Evaluate the outermost iterable in the enclosing scope.
    ///   2. Compile an anonymous generator function whose parameter receives
    ///      that iterable and whose body iterates over it, handling nested
    ///      clauses, and yields `elt` on each matching element.
    ///   3. Emit `MakeFunction` directly into a temp register — no global
    ///      store/reload — then call it immediately with the iterable.
    fn compile_gen_exp(&mut self, elt: &Expr, clauses: &[CompClause]) -> Reg {
        if clauses.is_empty() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg =
                    Some("generator expression requires at least one clause".to_string());
            }
            return 0;
        }
        // An `async for` clause makes this an *async generator* expression
        // (#2283): `(x async for x in ait)`.  Unlike a collection comprehension
        // it does NOT require an enclosing async function — it constructs an
        // `async_generator` object anywhere, just like an `async def` with a
        // bare `yield` (#2280).
        // An `await` in the element/cond/non-outermost iter (without an
        // `async for`) also makes the genexp an async generator (#2304):
        // `(await f(x) for x in xs)` — matches CPython, which produces an
        // `async_generator` object. Unlike collection comprehensions this does
        // not require an enclosing async function.
        let is_async =
            clauses.iter().any(|c| c.is_async) || Self::comp_body_has_await(&[elt], clauses);
        if let Some(msg) = check_comprehension(&[elt], clauses, "generator expression") {
            self.set_syntax_error(&msg);
            return 0;
        }

        // Evaluate the outermost iterable in the enclosing (current) scope
        // before creating the nested function.
        let iter_reg = self.compile_expr(&clauses[0].iter);

        // Use ".0" as the implicit parameter name — matches CPython's internal
        // convention and is not a valid Python identifier (cannot be lexed), so
        // user code inside the genexp body cannot accidentally reference or
        // shadow it.
        const IT_PARAM: &str = ".0";

        // Build the inner body working from the innermost clause outward:
        // `yield elt` wrapped in the clause loop nest (shared with the
        // collection comprehensions, so `async for` clauses thread through to
        // `compile_async_for` identically — #2283).
        let yield_stmt = Stmt::Expr(Expr::Yield(Some(Box::new(elt.clone()))));
        let body = Self::build_comp_loop_body(clauses, yield_stmt);

        // Parameter spec for the anonymous generator function.
        let params = vec![FunctionParam {
            name: IT_PARAM.to_string(),
            default: None,
            annotation: None,
            is_args: false,
            is_kwargs: false,
            is_keyword_only: false,
            is_positional_only: false,
        }];

        // Inline the function-object construction (mirrors compile_def) but
        // emit MakeFunction directly into a temp register instead of binding
        // the function into the global/local environment.  This avoids the
        // observable StoreGlobal("<genexp>") / LoadGlobal("<genexp>") pair.
        let Some(scope) = self.analyze_comprehension_scope(&params, &body) else {
            return 0;
        };

        let mut inner_index: HashMap<String, Reg> = HashMap::new();
        let mut slot: Reg = 0;
        for param in &params {
            if scope.locals.contains(&param.name) {
                inner_index.insert(param.name.clone(), slot);
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
        // Genexp bodies are never pure (they produce a generator object with
        // side-effectful iteration).
        let inner_cell_vars = collect_cell_vars(&body, &inner_index_rc);
        let inner_global_rc = Rc::new(scope.globals);
        let inner_nonlocal_rc = Rc::new(scope.nonlocals);

        let mut sub = Compiler::new(Rc::clone(&inner_index_rc), def_bound, inner_cell_vars);
        // Threaded source file (#2438): the comprehension's implicit code object
        // inherits the enclosing scope's `co_filename`.
        sub.filename = self.filename.clone();
        // Comprehensions create an implicit function scope; thread outer_locals.
        sub.outer_locals = self.outer_locals.clone();
        if self.is_function_scope {
            sub.outer_locals.push(Rc::clone(&self.local_index));
        }
        sub.is_function_scope = true;
        sub.future_annotations = self.future_annotations;
        // An async generator expression (`(x async for x in ait)`): the bare
        // `yield elt` plus `is_async_function` makes the synthesized function an
        // async generator (#2280/#2283).  The result is the async-gen object —
        // NOT awaited here, unlike a collection comprehension.
        sub.is_async_function = is_async;
        sub.compile_block(&body);
        let inner_code = match sub.finish() {
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
            crate::bytecode::compute_self_bind("<genexpr>", &inner_index_rc, &inner_code.cell_vars);
        self.fn_protos.push(FnProto {
            name: Rc::from("<genexpr>"),
            qualname: Rc::from("<genexpr>"),
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
        });

        // Allocate a temp for the function value, emit MakeFunction (no
        // defaults or annotations — the single parameter has no default and
        // genexp params carry no annotations), then call it.
        // Layout: fn_reg = function, fn_reg+1 = iterable arg.
        let fn_reg = self.alloc_temp();
        self.emit(Insn::MakeFunction(fn_reg, proto_idx, 0, 0, 0, 0));

        let arg_reg = fn_reg + 1;
        if arg_reg > self.max_reg {
            self.max_reg = arg_reg;
        }
        if fn_reg + 2 > self.next_temp {
            self.next_temp = fn_reg + 2;
        }
        self.emit(Insn::Move(arg_reg, iter_reg));
        self.emit(Insn::Call(fn_reg, 1));
        // Release the arg register slot; fn_reg stays live as the result.
        self.next_temp = fn_reg + 1;
        self.free_temp(iter_reg);

        fn_reg
    }
}
