impl Compiler {
    fn compile_expr(&mut self, expr: &Expr) -> Reg {
        if self.failed {
            return 0;
        }
        match expr {
            Expr::None => {
                let dst = self.alloc_temp();
                self.emit(Insn::LoadNone(dst));
                dst
            }
            Expr::Ellipsis => self.compile_literal(Value::ellipsis()),
            Expr::Int(v) => self.compile_literal(Value::int(*v)),
            Expr::BigInt(s) => {
                // The decimal string was validated at lex time; parse cannot fail.
                let n = s
                    .parse::<PyBigInt>()
                    .expect("BigInt decimal string is valid");
                self.compile_literal(Value::bigint(n))
            }
            Expr::Float(v) => self.compile_literal(Value::float(*v)),
            Expr::Str(s) => self.compile_literal(Value::string(s.clone())),
            Expr::Bytes(b) => self.compile_literal(Value::bytes(b.clone())),
            Expr::Complex(re, im) => self.compile_literal(Value::complex(*re, *im)),
            Expr::Bool(b) => self.compile_literal(Value::bool_(*b)),
            Expr::Var(name, span) => {
                if self.optimize >= 1 && name.as_str() == "__debug__" {
                    self.compile_literal(Value::bool_(false))
                } else {
                    self.compile_var_expr(name, *span)
                }
            }
            Expr::Unary { op, expr, span } => {
                let src = self.compile_expr(expr);
                let dst = self.ensure_dst(src);
                // PEP 657 caret anchor (#2582): underline the whole `OP operand`
                // span with `^` for the arithmetic unary forms.  `span` is `None` for `not`.
                self.set_col_span_for_next(*span);
                self.emit(Insn::UnaryOp(dst, *op, src));
                dst
            }
            Expr::Binary {
                left,
                op,
                right,
                span,
            } => match op {
                BinaryOp::And => self.compile_short_circuit(left, right, false),
                BinaryOp::Or => self.compile_short_circuit(left, right, true),
                _ => {
                    let lhs = self.compile_expr(left);
                    let dst = self.ensure_dst(lhs);
                    let rhs = self.compile_expr(right);
                    // PEP 657 caret anchor (#2411): operator `^`, operands `~`.
                    self.set_col_span_for_next(*span);
                    self.emit(Insn::BinOp(dst, lhs, *op, rhs));
                    self.free_temp(rhs);
                    dst
                }
            },
            Expr::Compare { left, ops } => self.compile_compare_expr(left, ops),
            Expr::Call {
                func,
                args,
                span,
                lineno,
            } => self.compile_call(func, args, *span, *lineno),
            Expr::Attr { target, name, span } => {
                let obj = self.compile_expr(target);
                let name_idx = self.intern_name(name);
                let dst = self.ensure_dst(obj);
                // PEP 657 caret anchor (#2442): underline the whole `obj.attr` span.
                self.set_col_span_for_next(*span);
                self.emit(Insn::GetAttr(dst, obj, name_idx));
                dst
            }
            Expr::Index {
                target,
                index,
                span,
            } => {
                let obj = self.compile_expr(target);
                let idx = self.compile_expr(index);
                let dst = self.ensure_dst(obj);
                // PEP 657 caret anchor (#2411): object `~`, `[...]` `^`.
                self.set_col_span_for_next(*span);
                self.emit(Insn::GetItem(dst, obj, idx));
                self.free_temp(idx);
                dst
            }
            Expr::SliceLit { lower, upper, step } => {
                // Standalone slice element of a tuple subscript (#3316).
                self.compile_slice_key(lower.as_deref(), upper.as_deref(), step.as_deref())
            }
            Expr::Slice {
                target,
                lower,
                upper,
                step,
            } => {
                // Rvalue slice read `obj[lo:hi:step]` via GetSlice (#1964).
                let obj = self.compile_expr(target);
                let dst = self.ensure_dst(obj);
                let saved_next = self.next_temp;
                let base =
                    self.compile_slice_bounds(lower.as_deref(), upper.as_deref(), step.as_deref());
                self.emit(Insn::GetSlice(dst, obj, base));
                // The three bound slots [base, base+3) are consumed by GetSlice.
                self.next_temp = saved_next;
                dst
            }
            Expr::List(items) => {
                if items.iter().any(|e| matches!(e, Expr::Starred(_))) {
                    self.compile_unpack_list_or_tuple(items, false)
                } else {
                    self.compile_collection(items, false)
                }
            }
            Expr::Tuple(items) => {
                if items.iter().any(|e| matches!(e, Expr::Starred(_))) {
                    self.compile_unpack_list_or_tuple(items, true)
                } else {
                    self.compile_collection(items, true)
                }
            }
            Expr::Starred(_) => {
                // `*expr` is only valid as a child of a list/tuple/set literal,
                // a call-site argument, or an assign target.
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("can't use starred expression here".to_string());
                }
                0
            }
            Expr::Set(items) => self.compile_set_literal(items),
            Expr::Dict(items) => self.compile_dict_literal(items),
            Expr::Ternary { cond, then, else_ } => {
                let cond_reg = self.compile_expr(cond);
                let jmp_false = self.emit(Insn::JumpIfFalse(cond_reg, 0));
                self.free_temp(cond_reg);
                let dst = self.alloc_temp();
                let saved = self.next_temp;
                self.compile_expr_into(then, dst);
                self.next_temp = saved;
                let jmp_end = self.emit(Insn::Jump(0));
                self.patch_jump(jmp_false);
                let saved = self.next_temp;
                self.compile_expr_into(else_, dst);
                self.next_temp = saved;
                self.patch_jump(jmp_end);
                dst
            }
            Expr::Lambda { params, body } => self.compile_lambda(params, body),
            Expr::ListComp { elt, clauses, span } => self.compile_list_comp(elt, clauses, *span),
            Expr::DictComp {
                key,
                val,
                clauses,
                span,
            } => self.compile_dict_comp(key, val, clauses, *span),
            Expr::SetComp { elt, clauses, span } => self.compile_set_comp(elt, clauses, *span),
            Expr::GenExp {
                elt,
                elt_lineno,
                first_lineno,
                clauses,
            } => self.compile_gen_exp(elt, *elt_lineno, *first_lineno, clauses),
            Expr::Named { target, value } => self.compile_named_expr(target, value),
            Expr::FString(parts) => self.compile_fstring(parts),
            Expr::Yield {
                value,
                span,
                lineno,
            } => self.compile_yield_expr(value.as_deref(), *span, *lineno),
            Expr::YieldFrom { expr, span, lineno } => {
                self.compile_yield_from_expr(expr, *span, *lineno)
            }
            Expr::Await { expr, span, lineno } => self.compile_await_expr(expr, *span, *lineno),
        }
    }

    fn compile_yield_expr(
        &mut self,
        val_expr: Option<&Expr>,
        span: Option<crate::ast::CaretSpan>,
        lineno: u32,
    ) -> Reg {
        if !self.is_function_scope {
            self.set_syntax_error("'yield' outside function");
            return 0;
        }
        // Compile the yielded value (or None if bare `yield`).
        let src = if let Some(e) = val_expr {
            self.compile_expr(e)
        } else {
            let r = self.alloc_temp();
            self.emit(Insn::LoadNone(r));
            r
        };
        let dst = self.alloc_temp();
        let saved_lineno = self.current_lineno;
        if lineno != 0 {
            self.set_lineno(lineno);
        }
        self.set_col_span_for_next(span);
        self.emit(Insn::Yield { src, dst });
        self.set_lineno(saved_lineno);
        self.free_temp(src);
        dst
    }

    fn compile_yield_from_expr(
        &mut self,
        iter_expr: &Expr,
        span: Option<crate::ast::CaretSpan>,
        lineno: u32,
    ) -> Reg {
        if !self.is_function_scope {
            self.set_syntax_error("'yield' outside function");
            return 0;
        }
        // `yield from` is not allowed inside an `async def` body (#2280).
        if self.is_async_function {
            self.set_syntax_error("'yield from' inside async function");
            return 0;
        }
        // PEP 380 `yield from` delegation via the single YieldFrom instruction.
        let iter_src = self.compile_expr(iter_expr);
        let saved_lineno = self.current_lineno;
        if lineno != 0 {
            self.set_lineno(lineno);
        }
        let iter_fn_reg = self.alloc_temp();
        let iter_name_idx = self.intern_name("iter");
        self.emit(Insn::LoadGlobal(iter_fn_reg, iter_name_idx));
        let iter_arg_reg = self.alloc_temp(); // == iter_fn_reg + 1
        self.emit(Insn::Move(iter_arg_reg, iter_src));
        self.free_temp(iter_src);
        // PEP 657 (#2992): both `iter(...)` and `YieldFrom` underline `yield from <expr>`.
        self.set_col_span_for_next(span);
        self.emit(Insn::Call(iter_fn_reg, 1));
        self.free_temp(iter_arg_reg);
        let iter_reg = iter_fn_reg;

        let sent_reg = self.alloc_temp();
        self.emit(Insn::LoadNone(sent_reg));

        let result_reg = self.alloc_temp();
        self.emit(Insn::LoadNone(result_reg));

        self.set_col_span_for_next(span);
        self.emit(Insn::YieldFrom {
            iter_reg,
            sent_reg,
            result_reg,
        });
        self.set_lineno(saved_lineno);

        self.free_temp(sent_reg);
        self.free_temp(iter_reg);
        result_reg
    }

    fn compile_await_expr(
        &mut self,
        awaited_expr: &Expr,
        span: Option<crate::ast::CaretSpan>,
        lineno: u32,
    ) -> Reg {
        if !self.is_function_scope {
            if self.allow_top_level_await {
                self.saw_top_level_await = true;
            } else {
                self.set_syntax_error("'await' outside function");
                return 0;
            }
        } else if !self.is_async_function {
            self.set_syntax_error("'await' outside async function");
            return 0;
        }
        // Allocate `result_reg` FIRST so it sits below the scratch temps
        // (`awaited_src`/`iter_reg`/`sent_reg`) in the strictly LIFO temp stack.
        let result_reg = self.alloc_temp();
        let awaited_src = self.compile_expr(awaited_expr);
        let saved_lineno = self.current_lineno;
        if lineno != 0 {
            self.set_lineno(lineno);
        }
        self.emit_await_drive_into_with_span(awaited_src, result_reg, span);
        self.set_lineno(saved_lineno);
        self.free_temp(awaited_src);
        result_reg
    }

    /// Emit the `await` drive for an awaitable already living in `awaited_src`,
    /// placing the awaited result into `result_reg` (which the caller has
    /// allocated *below* `awaited_src` so the LIFO temp allocator can reclaim
    /// the scratch temps without clobbering it).
    ///
    /// Reused by `async for` / `async with` (#2279) and async comprehensions.
    fn emit_await_drive_into(&mut self, awaited_src: Reg, result_reg: Reg) {
        self.emit_await_drive_into_with_span(awaited_src, result_reg, None);
    }

    fn emit_await_drive_into_with_span(
        &mut self,
        awaited_src: Reg,
        result_reg: Reg,
        span: Option<crate::ast::CaretSpan>,
    ) {
        let iter_reg = self.alloc_temp();
        self.set_col_span_for_next(span);
        self.emit(Insn::GetAwaitable(iter_reg, awaited_src));

        let sent_reg = self.alloc_temp();
        self.emit(Insn::LoadNone(sent_reg));
        self.emit(Insn::LoadNone(result_reg));

        self.set_col_span_for_next(span);
        self.emit(Insn::YieldFrom {
            iter_reg,
            sent_reg,
            result_reg,
        });

        self.free_temp(sent_reg);
        self.free_temp(iter_reg);
    }
}
