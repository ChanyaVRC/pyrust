impl Compiler {
    // ── Expression compilation ────────────────────────────────────────────────

    /// Retarget the last emitted instruction's dst from `from` to `to`.
    /// Returns true if the instruction was retargeted (had dst == from).
    fn retarget_last(&mut self, from: Reg, to: Reg) -> bool {
        let Some(insn) = self.insns.last_mut() else {
            return false;
        };
        let dst = match insn {
            Insn::BinOp(d, ..)
            | Insn::BinOpInPlace(d, ..)
            | Insn::BinOpConst(d, ..)
            | Insn::BinOpImm(d, ..)
            | Insn::UnaryOp(d, ..)
            | Insn::LoadConst(d, ..)
            | Insn::LoadNone(d)
            | Insn::LoadGlobal(d, ..)
            | Insn::LoadClassName(d, ..)
            | Insn::Move(d, ..)
            | Insn::GetAttr(d, ..)
            | Insn::GetAttrForWith(d, ..)
            | Insn::ImportFromAttr(d, ..)
            | Insn::GetItem(d, ..)
            // Call is NOT retargetable: Call(func_reg, argc) uses func_reg as both
            // the function source and the result destination. Retargeting it to a
            // different register would point the call at the wrong function.
            | Insn::MakeFunction(d, ..)
            | Insn::MakeClass(d, ..)
            | Insn::BuildList(d, ..)
            | Insn::BuildTuple(d, ..)
            | Insn::BuildDict(d, ..)
            | Insn::ForIter(d, ..)
            | Insn::LoadExc(d)
            | Insn::ImportModule(d, ..) => d,
            _ => return false,
        };
        if *dst == from {
            *dst = to;
            true
        } else {
            false
        }
    }

    fn compile_expr_into(&mut self, expr: &Expr, dst: Reg) {
        if self.failed {
            return;
        }
        let saved_next = self.next_temp;
        let insn_before = self.insns.len();
        let r = self.compile_expr(expr);
        if r != dst {
            // Safe to retarget only when the expression compiled to EXACTLY one
            // instruction and the result is a fresh temp: guarantees no control
            // flow or multi-instruction sequences where other branches still write
            // to `r` and would be missed by retargeting only the last instruction.
            let single = self.insns.len() == insn_before + 1;
            if single && r >= self.base_temp && self.retarget_last(r, dst) {
                self.next_temp = saved_next;
            } else {
                self.emit(Insn::Move(dst, r));
                if r >= self.base_temp {
                    self.next_temp = saved_next;
                }
            }
        }
    }

    /// Try to extract a small i16 integer immediate from an expression.
    /// Returns `Some(imm)` when `expr` is an integer literal in `i16` range.
    fn try_imm_i16(expr: &Expr) -> Option<i16> {
        if let Expr::Int(v) = expr
            && *v >= i16::MIN as i64
            && *v <= i16::MAX as i64
        {
            return Some(*v as i16);
        }
        None
    }

    fn emit_aug_binop(&mut self, reg: Reg, op: BinaryOp, expr: &Expr) {
        if let Some(imm) = Self::try_imm_i16(expr) {
            self.emit(Insn::BinOpImm(reg, reg, op, imm, true));
        } else if let Some(val) = fold_constant(expr) {
            // BinOpConst is safe for augmented assignment: the VM's BinOpConst
            // handler calls try_inplace_op before eval_binary, so mutable
            // containers (list *= / list += / set |= etc.) still get the
            // in-place fast path even when the RHS is a folded constant.  The
            // `is_aug = true` flag tells the VM this fused op carries in-place
            // semantics (issue #1874).
            let idx = self.intern_const(val);
            self.emit(Insn::BinOpConst(reg, reg, op, idx, true));
        } else {
            let rhs = self.compile_expr(expr);
            self.emit(Insn::BinOpInPlace(reg, reg, op, rhs));
            self.free_temp(rhs);
        }
    }

    fn compile_short_circuit(&mut self, left: &Expr, right: &Expr, jump_if_true: bool) -> Reg {
        let lhs = self.compile_expr(left);
        // Always copy to a fresh temp so the JumpIf tests the copy, not `lhs`
        // itself. This prevents the optimizer from fusing BinOp(lhs,lhs,...)+
        // JumpIfFalse(lhs) → CmpJumpIfFalse, which would leave `lhs` holding
        // the original (pre-BinOp) value after the jump instead of False/True.
        let dst = self.alloc_temp();
        self.emit(Insn::Move(dst, lhs));
        self.free_temp(lhs);
        let jmp = if jump_if_true {
            self.emit(Insn::JumpIfTrue(dst, 0))
        } else {
            self.emit(Insn::JumpIfFalse(dst, 0))
        };
        let saved = self.next_temp;
        self.compile_expr_into(right, dst);
        self.next_temp = saved;
        self.patch_jump(jmp);
        dst
    }

    fn compile_literal(&mut self, v: Value) -> Reg {
        let idx = self.intern_const(v);
        let dst = self.alloc_temp();
        self.emit(Insn::LoadConst(dst, idx));
        dst
    }

    /// Compile an expression whose result is discarded by its enclosing
    /// statement.  A definitely-bound local normally compiles to its register
    /// with no instruction. If this compile unit can expose its live module
    /// mapping, however, that binding can be removed without changing the
    /// compiler's `def_set`; force the existing checked `LoadGlobal` path so
    /// the discarded read still performs Python's runtime lookup (#3026).
    fn compile_discarded_expr(&mut self, expr: &Expr) -> Reg {
        let Expr::Var(name, _) = expr else {
            return self.compile_expr(expr);
        };
        let Some(reg) = self.local_reg(name) else {
            return self.compile_expr(expr);
        };
        let definitely_bound = (reg as usize) < 64 && (self.def_set >> reg) & 1 != 0;
        let needs_checked_load =
            self.is_module_scope && self.module_namespace_may_be_exposed && definitely_bound;
        if !needs_checked_load {
            return self.compile_expr(expr);
        }

        let saved_def_set = self.def_set;
        self.def_set &= !(1u64 << reg);
        let result = self.compile_expr(expr);
        self.def_set = saved_def_set;
        result
    }

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
                // PEP 657 caret anchor (#2426): `set_col_span_for_next` arms the
                // name's column span so the very next `emit` stamps it onto the
                // load instruction that may raise NameError; `emit` then clears
                // it.  We arm *immediately before* each load emit (not for the
                // definitely-bound-local path, which emits nothing) so a stale
                // span never leaks onto an unrelated instruction.  A bare name's
                // anchor is whole-span (`^`), so widen the `(start, end)` form to
                // the `(full, prim) = (start, start, end, end)` shape (#2411).
                //
                // Multi-line line stamping (#2632): the parser also records the
                // name's own 1-based line.  When it differs from the statement's
                // `current_lineno` (the name sits on a continuation line of a
                // multi-line expression), stamp the load instruction with the
                // name's line so a NameError it raises reports that line and its
                // source text — matching CPython 3.12, which gives each name node
                // its own lineno.  We restore `current_lineno` afterwards so the
                // override never leaks onto sibling instructions.
                let name_lineno = span.and_then(|(_, _, ln)| (ln != 0).then_some(ln));
                let span: Option<crate::ast::CaretSpan> = span.map(|(s, e, _)| (s, s, e, e));
                let saved_lineno = self.current_lineno;
                if let Some(ln) = name_lineno {
                    self.set_lineno(ln);
                }
                let result = if let Some(reg) = self.local_reg(name) {
                    if self.is_class_body {
                        let name_idx = self.intern_name(name);
                        let dst = self.alloc_temp();
                        self.set_col_span_for_next(span);
                        self.emit(Insn::LoadClassName(dst, reg, name_idx));
                        self.set_lineno(saved_lineno);
                        return dst;
                    }
                    let definitely_bound = (reg as usize) < 64 && (self.def_set >> reg) & 1 != 0;
                    if !definitely_bound {
                        // Issue #1411: at module scope, a name that is not yet
                        // definitely bound must resolve through the global →
                        // builtins chain rather than raising NameError.  Module
                        // scope is sequential (like a REPL), so a later
                        // assignment does NOT shadow earlier reads.  LoadGlobal
                        // already has a fastlocal-register fallback (via
                        // vm_frame_views) for names that have been assigned,
                        // so already-written names are still found efficiently.
                        if self.is_module_scope {
                            let name_idx = self.intern_name(name);
                            let dst = self.alloc_temp();
                            self.set_col_span_for_next(span);
                            self.emit(Insn::LoadGlobal(dst, name_idx));
                            self.set_lineno(saved_lineno);
                            return dst;
                        }
                        let name_idx = self.intern_name(name);
                        self.set_col_span_for_next(span);
                        self.emit(Insn::CheckLocal(reg, name_idx));
                    }
                    reg
                } else {
                    // global / nonlocal / cell / free variable
                    let name_idx = self.intern_name(name);
                    let dst = self.alloc_temp();
                    // A function-scope cell / nonlocal resolves in the env chain;
                    // emit LoadCell to skip the LoadGlobal inline-cache + module
                    // -dict path (issue #2339).  Everything else (true globals,
                    // builtins, module/class-scope free vars) keeps LoadGlobal.
                    self.set_col_span_for_next(span);
                    if self.is_class_body && !self.class_direct_env_names.contains(name) {
                        self.emit(Insn::LoadClassName(
                            dst,
                            crate::bytecode::NO_CLASS_LOCAL,
                            name_idx,
                        ));
                    } else if self.is_function_cell(name) {
                        self.emit(Insn::LoadCell(dst, name_idx));
                    } else {
                        self.emit(Insn::LoadGlobal(dst, name_idx));
                    }
                    dst
                };
                self.set_lineno(saved_lineno);
                result
            }
            Expr::Unary { op, expr, span } => {
                let src = self.compile_expr(expr);
                let dst = self.ensure_dst(src);
                // PEP 657 caret anchor (#2582): underline the whole `OP operand`
                // span with `^` for the arithmetic unary forms.  Arm immediately
                // before the UnaryOp that may raise (e.g. TypeError on `-"s"`);
                // `emit` consumes and clears it.  `span` is `None` for `not`.
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
                    // PEP 657 caret anchor (#2411): the operator underlines `^`,
                    // operands `~`.  Arm immediately before the BinOp that may
                    // raise (e.g. ZeroDivisionError / TypeError); `emit` clears it.
                    self.set_col_span_for_next(*span);
                    self.emit(Insn::BinOp(dst, lhs, *op, rhs));
                    self.free_temp(rhs);
                    dst
                }
            },
            Expr::Compare { left, ops } => {
                if ops.len() == 1 {
                    let (cmp_op, right) = &ops[0];
                    let lhs = self.compile_expr(left);
                    let bin_op = BinaryOp::from(*cmp_op);
                    let dst = self.ensure_dst(lhs);
                    let rhs = self.compile_expr(right);
                    self.emit(Insn::BinOp(dst, lhs, bin_op, rhs));
                    self.free_temp(rhs);
                    dst
                } else {
                    // Chained comparison: a < b < c  →  (a < b) and (b < c)
                    // Evaluate left once, then chain.
                    let first_lhs = self.compile_expr(left);
                    let result_dst = self.alloc_temp();
                    let mut and_patches: Vec<usize> = Vec::new();
                    let mut prev_rhs = first_lhs;
                    for (i, (cmp_op, rhs_expr)) in ops.iter().enumerate() {
                        let bin_op = BinaryOp::from(*cmp_op);
                        let rhs = self.compile_expr(rhs_expr);
                        let last = i == ops.len() - 1;
                        // For the last comparison write directly into result_dst to
                        // avoid a trailing Move(result_dst, cmp_dst).
                        let cmp_dst = if last { result_dst } else { self.alloc_temp() };
                        self.emit(Insn::BinOp(cmp_dst, prev_rhs, bin_op, rhs));
                        if i > 0 {
                            self.free_temp(prev_rhs);
                        }
                        if !last {
                            self.emit(Insn::Move(result_dst, cmp_dst));
                            self.free_temp(cmp_dst);
                            let p = self.emit(Insn::JumpIfFalse(result_dst, 0));
                            and_patches.push(p);
                        }
                        prev_rhs = rhs;
                    }
                    self.free_temp(prev_rhs);
                    for p in and_patches {
                        self.patch_jump(p);
                    }
                    self.free_temp(first_lhs);
                    result_dst
                }
            }
            Expr::Call { func, args, span } => self.compile_call(func, args, *span),
            Expr::Attr { target, name, span } => {
                let obj = self.compile_expr(target);
                let name_idx = self.intern_name(name);
                let dst = self.ensure_dst(obj);
                // PEP 657 caret anchor (#2442): underline the whole `obj.attr`
                // span.  Arm immediately before the GetAttr that may raise
                // AttributeError; `emit` clears it.
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
                // PEP 657 caret anchor (#2411): object underlined `~`, `[...]`
                // underlined `^`.  Arm before the GetItem that may raise
                // KeyError / IndexError / TypeError; `emit` clears it.
                self.set_col_span_for_next(*span);
                self.emit(Insn::GetItem(dst, obj, idx));
                self.free_temp(idx);
                dst
            }
            Expr::Slice {
                target,
                lower,
                upper,
                step,
            } => {
                // Rvalue slice read `obj[lo:hi:step]`: emit GetSlice, which reads
                // the three contiguous bound registers directly and slices `obj`
                // without materialising a `slice` object on the built-in-sequence
                // fast path (#1964, CPython BINARY_SLICE analogue).
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
                // a call-site argument, or an assign target.  Encountering it
                // here means the parser produced one in an unexpected position.
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
            Expr::ListComp { elt, clauses } => self.compile_list_comp(elt, clauses),
            Expr::DictComp { key, val, clauses } => self.compile_dict_comp(key, val, clauses),
            Expr::SetComp { elt, clauses } => self.compile_set_comp(elt, clauses),
            Expr::GenExp { elt, clauses } => self.compile_gen_exp(elt, clauses),
            Expr::Named { target, value } => {
                let val_reg = self.compile_expr(value);
                if let Some(reg) = self.local_reg(target) {
                    if val_reg != reg {
                        self.emit(Insn::Move(reg, val_reg));
                    }
                    self.maybe_record_class_store(reg);
                    self.mark_def(reg);
                } else {
                    let name_idx = self.intern_name(target);
                    self.emit(Insn::StoreGlobal(name_idx, val_reg));
                }
                val_reg
            }
            Expr::FString(parts) => self.compile_fstring(parts),

            Expr::Yield(val_expr) => {
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
                self.emit(Insn::Yield { src, dst });
                self.free_temp(src);
                dst
            }

            Expr::YieldFrom(iter_expr) => {
                if !self.is_function_scope {
                    self.set_syntax_error("'yield' outside function");
                    return 0;
                }
                // `yield from` is not allowed inside an `async def` body
                // (#2280): CPython raises SyntaxError.  (A bare `yield` is fine
                // — it makes the function an async generator.)  `await` lowers
                // to the same `YieldFrom` *instruction* internally, but that
                // path goes through `Expr::Await`, not this user-facing
                // `Expr::YieldFrom` compilation, so it is unaffected.
                if self.is_async_function {
                    self.set_syntax_error("'yield from' inside async function");
                    return 0;
                }
                // PEP 380 `yield from` delegation via the single YieldFrom instruction.
                //
                // The VM handles the send/yield/StopIteration loop internally:
                // - Calls sub_iter.send(sent_reg) on each execution.
                // - Yields the produced value to the outer caller, suspending at this
                //   instruction; on resume, writes the received sent value into sent_reg.
                // - On StopIteration, writes the sub-iterator's return value into
                //   result_reg and falls through to the next instruction.
                //
                // Unlike the old ForIter/Yield/Jump loop, YieldFrom forwards the outer
                // caller's sent value (and throw) into the sub-iterator (PEP 380).

                // Evaluate the iterable and call iter() on it to get the iterator
                // object.  For generators, iter(gen) == gen; for lists, tuples, etc.,
                // iter() returns the appropriate iterator.
                // Call convention: Call(func_reg, argc) reads args from
                // func_reg+1 .. func_reg+argc; alloc_temp() is sequential so
                // iter_arg_reg == iter_fn_reg + 1.
                let iter_src = self.compile_expr(iter_expr);
                let iter_fn_reg = self.alloc_temp();
                let iter_name_idx = self.intern_name("iter");
                self.emit(Insn::LoadGlobal(iter_fn_reg, iter_name_idx));
                let iter_arg_reg = self.alloc_temp(); // == iter_fn_reg + 1
                self.emit(Insn::Move(iter_arg_reg, iter_src));
                self.free_temp(iter_src);
                self.emit(Insn::Call(iter_fn_reg, 1)); // result lands in iter_fn_reg
                self.free_temp(iter_arg_reg);
                let iter_reg = iter_fn_reg; // iter_reg holds the iterator object

                // sent_reg: value to send on each iteration.  Initialized to None
                // (first call is always next()-equivalent); on resumption the VM
                // writes the caller's sent value here (like Yield.dst).
                let sent_reg = self.alloc_temp();
                self.emit(Insn::LoadNone(sent_reg));

                // result_reg: receives StopIteration.value when sub-iterator exhausts.
                // This is the value of the `yield from` expression in the outer generator.
                let result_reg = self.alloc_temp();
                self.emit(Insn::LoadNone(result_reg));

                self.emit(Insn::YieldFrom {
                    iter_reg,
                    sent_reg,
                    result_reg,
                });

                // iter_reg and sent_reg are only live during YieldFrom.
                self.free_temp(sent_reg);
                self.free_temp(iter_reg);

                // result_reg is the value of the `yield from` expression.
                result_reg
            }

            Expr::Await(awaited_expr) => {
                if !self.is_function_scope {
                    self.set_syntax_error("'await' outside function");
                    return 0;
                }
                if !self.is_async_function {
                    self.set_syntax_error("'await' outside async function");
                    return 0;
                }
                // `await expr` lowers to roughly `yield from GET_AWAITABLE(expr)`
                // (issue #1039).  `GetAwaitable` resolves the awaitable to its
                // driving iterator (a coroutine drives itself; an object with
                // `__await__` yields its `__await__()` result); `YieldFrom` then
                // reuses the PEP 380 suspend/resume machinery to drive it to
                // completion, surfacing its return value (StopIteration.value).
                //
                // The result register is allocated FIRST so it sits below the
                // scratch temps (`awaited_src`/`iter_reg`/`sent_reg`).  The temp
                // allocator is strictly LIFO — `free_temp` only reclaims the top
                // of the stack — so the scratch temps are freed in reverse
                // allocation order while `result_reg` (the expression's value,
                // which outlives them) stays below.  An earlier version freed
                // `awaited_src` before the temps allocated above it, making those
                // frees silent no-ops; the leaked slots then corrupted register
                // allocation for a *subsequent* await when this await was nested
                // in a larger expression (e.g. `print(await x)`), surfacing as a
                // spurious "object is not iterable".
                let result_reg = self.alloc_temp();
                let awaited_src = self.compile_expr(awaited_expr);
                self.emit_await_drive_into(awaited_src, result_reg);
                self.free_temp(awaited_src);
                result_reg
            }
        }
    }

    /// Emit the `await` drive for an awaitable already living in `awaited_src`,
    /// placing the awaited result into `result_reg` (which the caller has
    /// allocated *below* `awaited_src` so the LIFO temp allocator can reclaim
    /// the scratch temps without clobbering it).
    ///
    /// This is the same `GetAwaitable` + `YieldFrom` sequence the `Expr::Await`
    /// lowering uses (issue #1039); `async for` / `async with` reuse it to drive
    /// `__anext__` / `__aenter__` / `__aexit__` coroutines to completion
    /// (issue #2279).  Both `awaited_src` and `result_reg` must outlive this
    /// call; only the internal scratch temps are freed here.
    fn emit_await_drive_into(&mut self, awaited_src: Reg, result_reg: Reg) {
        let iter_reg = self.alloc_temp();
        self.emit(Insn::GetAwaitable(iter_reg, awaited_src));

        let sent_reg = self.alloc_temp();
        self.emit(Insn::LoadNone(sent_reg));
        self.emit(Insn::LoadNone(result_reg));

        self.emit(Insn::YieldFrom {
            iter_reg,
            sent_reg,
            result_reg,
        });

        self.free_temp(sent_reg);
        self.free_temp(iter_reg);
    }

    /// Compile an f-string into a series of str-conversions concatenated with `+`.
    fn compile_fstring(&mut self, parts: &[FStringPart]) -> Reg {
        if parts.is_empty() {
            return self.compile_literal(Value::string(String::new()));
        }

        // Compile each part into a string register.
        let mut part_regs: Vec<Reg> = Vec::new();
        for part in parts {
            let r = match part {
                FStringPart::Literal(s) => self.compile_literal(Value::string(s.clone())),
                FStringPart::Expr {
                    expr,
                    conversion,
                    format_spec,
                    debug_text,
                    span,
                    line,
                } => {
                    // Stamp this field's instructions with its own source line
                    // so a field on a continuation line of a multi-line
                    // f-string (or in a later implicitly-joined fragment)
                    // anchors the traceback on the right line (issue #2587).
                    // `line == 0` means line info is unavailable; leave the
                    // statement's current line in place.
                    let saved_lineno = self.current_lineno;
                    if *line != 0 {
                        self.set_lineno(*line);
                    }
                    // Python 3.8 debug form `f"{x=}"`: emit the verbatim
                    // source text (with trailing `=`) as a literal prefix
                    // BEFORE the formatted value.  When no explicit
                    // conversion flag and no format spec are given, the
                    // default conversion becomes `repr` instead of the
                    // implicit `str`/`format(val, "")` path.
                    if let Some(label) = debug_text {
                        let lit_r = self.compile_literal(Value::string(label.clone()));
                        part_regs.push(lit_r);
                    }
                    let val_r = self.compile_expr(expr);
                    // Determine the effective conversion: explicit !r/!s/!a
                    // wins; otherwise, in debug form with no format spec, the
                    // implicit conversion is `repr`.
                    let effective_conversion: Option<char> = match conversion {
                        Some(c) => Some(*c),
                        None if debug_text.is_some() && format_spec.is_none() => Some('r'),
                        None => None,
                    };
                    // Apply conversion flag first.
                    let val_r = match effective_conversion {
                        Some('r') => {
                            // repr(val)
                            let frame = self.next_temp;
                            if frame + 1 > self.max_reg {
                                self.max_reg = frame + 1;
                            }
                            self.next_temp = frame + 2;
                            let repr_idx = self.intern_name("repr");
                            self.emit(Insn::LoadGlobal(frame, repr_idx));
                            if val_r != frame + 1 {
                                self.emit(Insn::Move(frame + 1, val_r));
                            }
                            self.free_temp(val_r);
                            // PEP 657 (#2582): the `{...}` field caret covers a
                            // conversion `__repr__` that raises.
                            self.set_col_span_for_next(*span);
                            self.emit(Insn::Call(frame, 1));
                            self.next_temp = frame + 1;
                            frame
                        }
                        Some('s') => {
                            // str(val) — calls __str__ on user instances
                            let frame = self.next_temp;
                            if frame + 1 > self.max_reg {
                                self.max_reg = frame + 1;
                            }
                            self.next_temp = frame + 2;
                            let str_idx = self.intern_name("str");
                            self.emit(Insn::LoadGlobal(frame, str_idx));
                            if val_r != frame + 1 {
                                self.emit(Insn::Move(frame + 1, val_r));
                            }
                            self.free_temp(val_r);
                            // PEP 657 (#2582): the `{...}` field caret covers a
                            // conversion `__str__` that raises.
                            self.set_col_span_for_next(*span);
                            self.emit(Insn::Call(frame, 1));
                            self.next_temp = frame + 1;
                            frame
                        }
                        Some('a') => {
                            // ascii(val) — repr with non-ASCII chars escaped
                            let frame = self.next_temp;
                            if frame + 1 > self.max_reg {
                                self.max_reg = frame + 1;
                            }
                            self.next_temp = frame + 2;
                            let ascii_idx = self.intern_name("ascii");
                            self.emit(Insn::LoadGlobal(frame, ascii_idx));
                            if val_r != frame + 1 {
                                self.emit(Insn::Move(frame + 1, val_r));
                            }
                            self.free_temp(val_r);
                            // PEP 657 (#2582): the `{...}` field caret covers a
                            // conversion `ascii()`/`__repr__` that raises.
                            self.set_col_span_for_next(*span);
                            self.emit(Insn::Call(frame, 1));
                            self.next_temp = frame + 1;
                            frame
                        }
                        _ => val_r,
                    };
                    // Apply format spec if present.  The spec is itself a
                    // mini f-string (literals plus nested `{expr}` parts), so
                    // we compile it via the same fstring helper to obtain a
                    // single string register, then format the value with it via
                    // the dedicated `FormatValueSpec` opcode.  This mirrors
                    // CPython's `FORMAT_VALUE` (which calls `PyObject_Format`
                    // directly): it skips the `format` global lookup, the
                    // two-register call window, and the call-arg expansion that
                    // the previous `Call(format, 2)` lowering paid on every
                    // interpolation.  User `__format__` dispatch for PyInstance
                    // values is preserved by the VM via `dispatch_dunder_format`.
                    let field_r = if let Some(spec_parts) = format_spec {
                        // Nested spec fields carry their own absolute `line`.
                        let spec_r = self.compile_fstring(spec_parts);
                        let dst = self.alloc_temp();
                        // PEP 657 (#2582): the `{...}` field caret covers a
                        // `__format__` that raises.  Arm immediately before the
                        // op; `emit` consumes and clears it.
                        self.set_col_span_for_next(*span);
                        self.emit(Insn::FormatValueSpec(dst, val_r, spec_r));
                        self.free_temp(val_r);
                        self.free_temp(spec_r);
                        dst
                    } else {
                        // format(val, "") — dispatch __format__("") per Python
                        // semantics, but via the dedicated FormatValue opcode so
                        // we skip the `format` global lookup and the generic call
                        // frame (issue #1926). The VM preserves user
                        // `__format__`/`__str__` dispatch for PyInstance values.
                        let dst = self.alloc_temp();
                        // PEP 657 (#2582): the `{...}` field caret covers a
                        // `__format__`/`__str__` that raises.
                        self.set_col_span_for_next(*span);
                        self.emit(Insn::FormatValue(dst, val_r));
                        self.free_temp(val_r);
                        dst
                    };
                    // Restore the statement's line for the next part / literal.
                    self.set_lineno(saved_lineno);
                    field_r
                }
            };
            part_regs.push(r);
        }

        // Single part: nothing to join.
        if part_regs.len() == 1 {
            return part_regs[0];
        }

        // BuildString consumes `n` CONSECUTIVE str registers and joins them in a
        // single preallocated pass (mirrors CPython's BUILD_STRING). The count is
        // encoded as u8, so for the rare >255-part f-string fall back to the
        // chained `BinOp(Add)` fold below.
        if part_regs.len() <= u8::MAX as usize {
            let n = part_regs.len() as u8;
            // Lay the parts out in a consecutive window starting at `base`, then
            // build into `base` (same shape as BuildList lowering).
            let base = self.next_temp;
            self.next_temp = base + Reg::from(n);
            let max_used = base + Reg::from(n) - 1;
            if max_used > self.max_reg {
                self.max_reg = max_used;
            }
            for (i, &r) in part_regs.iter().enumerate() {
                let slot = base + i as Reg;
                if r != slot {
                    self.emit(Insn::Move(slot, r));
                }
            }
            self.emit(Insn::BuildString(base, base, n));
            // Collapse the temp window: every part register lives below `base`
            // and is dead once the join is done.
            self.next_temp = base + 1;
            return base;
        }

        // Fallback for >255 parts: concatenate with BinOp(Add).
        let mut acc = part_regs[0];
        for &r in &part_regs[1..] {
            let dst = self.ensure_dst(acc);
            self.emit(Insn::BinOp(dst, acc, BinaryOp::Add, r));
            self.free_temp(r);
            acc = dst;
        }
        acc
    }
}
