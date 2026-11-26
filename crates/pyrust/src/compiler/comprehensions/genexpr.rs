impl Compiler {
    /// Compile a generator expression `(elt for target in iter ...)`.
    ///
    /// Strategy (mirrors CPython):
    ///   1. Evaluate the outermost iterable in the enclosing scope.
    ///   2. Compile an anonymous generator function whose parameter receives
    ///      that iterable and whose body iterates over it, handling nested
    ///      clauses, and yields `elt` on each matching element.
    ///   3. Emit `MakeFunction` directly into a temp register — no global
    ///      store/reload — then call it immediately with the iterable.
    fn compile_gen_exp(
        &mut self,
        elt: &Expr,
        elt_lineno: u32,
        first_lineno: u32,
        clauses: &[CompClause],
    ) -> Reg {
        // An `async for` clause or `await` in the body makes this an *async
        // generator* expression (#2283, #2304). Unlike a collection
        // comprehension it does NOT require an enclosing async function — it
        // constructs an `async_generator` object anywhere.
        let Some(is_async) =
            self.validate_comprehension_clauses(&[elt], clauses, "generator expression", false)
        else {
            return 0;
        };

        // Evaluate the outermost iterable in the enclosing (current) scope
        // before creating the nested function.
        let iter_reg = self.compile_expr(&clauses[0].iter);
        self.compile_gen_exp_over(iter_reg, elt, elt_lineno, first_lineno, clauses, is_async)
    }

    /// Qualified name of a generator expression created in this scope.
    fn genexpr_qualname(&self) -> String {
        if self.qualname_prefix.is_empty() {
            "<genexpr>".to_string()
        } else {
            format!("{}.<genexpr>", self.qualname_prefix)
        }
    }

    /// Steps 2 and 3 of [`Self::compile_gen_exp`] for an outermost iterable
    /// already evaluated into `iter_reg` (also the fallback path of the
    /// reduction lowering in `reduction.rs`, which shares that register with
    /// its fast path).
    fn compile_gen_exp_over(
        &mut self,
        iter_reg: Reg,
        elt: &Expr,
        elt_lineno: u32,
        first_lineno: u32,
        clauses: &[CompClause],
        is_async: bool,
    ) -> Reg {
        // Thread per-part line numbers so `<genexpr>` raises report accurate
        // `tb_lineno` and PEP 657 carets (#2992).
        let effective_first_lineno = if first_lineno != 0 {
            first_lineno
        } else {
            self.current_lineno
        };
        let effective_elt_lineno = if elt_lineno != 0 {
            elt_lineno
        } else {
            effective_first_lineno
        };
        let yield_stmt = Stmt::Expr(Expr::Yield {
            value: Some(Box::new(elt.clone())),
            span: None,
            lineno: effective_elt_lineno,
        });
        let (body, body_linenos) = Self::build_comp_loop_body_with_linenos(
            clauses,
            vec![yield_stmt],
            effective_elt_lineno,
        );

        let genexpr_qualname = self.genexpr_qualname();

        self.compile_comprehension_fn(CompFnConfig {
            iter_reg,
            extra_arg: None,
            fn_body: &body,
            body_linenos: &body_linenos,
            display_name: "<genexpr>",
            qualname: Some(&genexpr_qualname),
            is_async,
            is_inlined_comp: false,
            is_set_comp: false,
            is_list_comp: false,
            is_sum_reduction: false,
            list_comp_presize: false,
            first_lineno: effective_first_lineno,
            call_span: None,
            comp_outer_for_span: None,
            // An async generator expression returns the async-gen object —
            // NOT awaited here, unlike a collection comprehension.
            await_result: false,
        })
    }
}
