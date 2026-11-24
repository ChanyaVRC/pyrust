impl Compiler {
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
        comp_span: Option<crate::ast::CaretSpan>,
    ) -> Reg {
        let display = format!("<{comp_name}>");
        self.compile_comprehension_fn(CompFnConfig {
            iter_reg,
            fn_body: &fn_body,
            body_linenos: &[],
            display_name: &display,
            qualname: None,
            is_async,
            // list / set / dict comprehensions are CPython-3.12-inlined scopes;
            // an unbound enclosing-local read inside them is UnboundLocalError,
            // not the free-variable NameError (issue #2340).
            is_inlined_comp: true,
            // Set comprehensions lower `.acc.add(elt)` to Insn::SetAdd (#1861).
            is_set_comp: comp_name == "setcomp",
            // List comprehensions lower `.acc.append(elt)` to Insn::ListAppend (#1862).
            is_list_comp: comp_name == "listcomp",
            // Only a single-clause, unconditional, non-async list comp reaches
            // here with presize_acc — see compile_list_comp.
            list_comp_presize: presize_acc,
            // PEP 709 (#2993): carry the enclosing statement's source line and
            // caret span into the implicit helper.
            first_lineno: self.current_lineno,
            call_span: comp_span,
            comp_outer_for_span: comp_span,
            await_result: is_async,
        })
    }

    fn build_collection_comp_body(
        acc_sym: Symbol,
        acc_init: Expr,
        clauses: &[CompClause],
        innermost: Stmt,
    ) -> Vec<Stmt> {
        let mut fn_body = vec![Stmt::Assign(AssignTarget::Name(acc_sym), acc_init)];
        fn_body.extend(Self::build_comp_loop_body(clauses, innermost));
        fn_body.push(Stmt::Return(Some(Expr::Var(acc_sym, None))));
        fn_body
    }

    fn build_acc_method_call(acc_sym: Symbol, method: &str, elt: &Expr) -> Stmt {
        Stmt::Expr(Expr::Call {
            span: None,
            lineno: 0,
            func: Box::new(Expr::Attr {
                target: Box::new(Expr::Var(acc_sym, None)),
                name: Symbol::intern(method),
                span: None,
            }),
            args: vec![CallArg {
                name: None,
                value: elt.clone(),
                splat: false,
                double_splat: false,
            }],
        })
    }

    fn compile_list_comp(
        &mut self,
        elt: &Expr,
        clauses: &[CompClause],
        comp_span: Option<crate::ast::CaretSpan>,
    ) -> Reg {
        let Some(is_async) =
            self.validate_comprehension_clauses(&[elt], clauses, "list comprehension", true)
        else {
            return 0;
        };

        // Evaluate the outermost iterable in the enclosing scope.
        let iter_reg = self.compile_expr(&clauses[0].iter);
        let acc_sym = Symbol::intern(".acc");
        let innermost = Self::build_acc_method_call(acc_sym, "append", elt);
        let fn_body =
            Self::build_collection_comp_body(acc_sym, Expr::List(vec![]), clauses, innermost);

        // Pre-size the accumulator when the produced element count is exactly the
        // source length: a single clause with no `if` condition and not async.
        // With a condition or a second `for`, the count is unknown, so we cannot
        // reserve.  The reserve reads the source length from `.0` without running
        // user code (see `list_reserve_hint`), so it never changes semantics.
        let presize = clauses.len() == 1 && clauses[0].cond.is_none() && !is_async;
        self.compile_collection_comp_impl(
            iter_reg, fn_body, "listcomp", is_async, presize, comp_span,
        )
    }

    fn compile_dict_comp(
        &mut self,
        key: &Expr,
        val: &Expr,
        clauses: &[CompClause],
        comp_span: Option<crate::ast::CaretSpan>,
    ) -> Reg {
        let Some(is_async) =
            self.validate_comprehension_clauses(&[key, val], clauses, "dict comprehension", true)
        else {
            return 0;
        };

        let iter_reg = self.compile_expr(&clauses[0].iter);
        let acc_sym = Symbol::intern(".acc");
        let innermost = Stmt::IndexAssign {
            target: Box::new(Expr::Var(acc_sym, None)),
            index: Box::new(key.clone()),
            expr: val.clone(),
        };
        let fn_body =
            Self::build_collection_comp_body(acc_sym, Expr::Dict(vec![]), clauses, innermost);

        self.compile_collection_comp_impl(iter_reg, fn_body, "dictcomp", is_async, false, comp_span)
    }

    /// Recognize a synthesized comprehension accumulator method call
    /// (`.acc.add(elt)` / `.acc.append(elt)`) and emit the direct VM opcode,
    /// skipping attribute resolution and method-call dispatch (#1861, #1862).
    /// `.acc` is compiler-internal and cannot appear in user source.
    fn try_emit_comp_acc_method(
        &mut self,
        enabled: bool,
        expected_method: &str,
        expr: &Expr,
        make_insn: impl FnOnce(Reg, Reg) -> Insn,
    ) -> bool {
        if !enabled {
            return false;
        }
        let Expr::Call { func, args, .. } = expr else {
            return false;
        };
        let Expr::Attr { target, name, .. } = func.as_ref() else {
            return false;
        };
        if name != expected_method || !matches!(target.as_ref(), Expr::Var(v, _) if v == ".acc") {
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
        self.emit(make_insn(acc_reg, elt_reg));
        self.free_temp(elt_reg);
        self.free_temp(acc_reg);
        true
    }

    fn try_emit_set_comp_add(&mut self, expr: &Expr) -> bool {
        self.try_emit_comp_acc_method(self.is_set_comp, "add", expr, Insn::SetAdd)
    }

    fn try_emit_list_comp_append(&mut self, expr: &Expr) -> bool {
        self.try_emit_comp_acc_method(self.is_list_comp, "append", expr, Insn::ListAppend)
    }

    fn compile_set_comp(
        &mut self,
        elt: &Expr,
        clauses: &[CompClause],
        comp_span: Option<crate::ast::CaretSpan>,
    ) -> Reg {
        let Some(is_async) =
            self.validate_comprehension_clauses(&[elt], clauses, "set comprehension", true)
        else {
            return 0;
        };

        let iter_reg = self.compile_expr(&clauses[0].iter);
        let acc_sym = Symbol::intern(".acc");
        let innermost = Self::build_acc_method_call(acc_sym, "add", elt);
        let acc_init = Expr::Call {
            span: None,
            lineno: 0,
            func: Box::new(Expr::Var(Symbol::intern("set"), None)),
            args: vec![],
        };
        let fn_body = Self::build_collection_comp_body(acc_sym, acc_init, clauses, innermost);

        self.compile_collection_comp_impl(iter_reg, fn_body, "setcomp", is_async, false, comp_span)
    }
}
