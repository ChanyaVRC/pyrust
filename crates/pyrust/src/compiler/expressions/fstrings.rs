impl Compiler {
    /// Compile an f-string into a series of str-conversions joined via
    /// `BuildString` (or chained `+` when exceeding 255 parts).
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
                    // Apply conversion flag first (!r → repr, !s → str, !a → ascii).
                    let val_r = match effective_conversion {
                        Some(conv @ ('r' | 's' | 'a')) => {
                            let builtin_name = match conv {
                                'r' => "repr",
                                's' => "str",
                                'a' => "ascii",
                                _ => unreachable!(),
                            };
                            let frame = self.next_temp;
                            if frame + 1 > self.max_reg {
                                self.max_reg = frame + 1;
                            }
                            self.next_temp = frame + 2;
                            let fn_idx = self.intern_name(builtin_name);
                            self.emit(Insn::LoadGlobal(frame, fn_idx));
                            if val_r != frame + 1 {
                                self.emit(Insn::Move(frame + 1, val_r));
                            }
                            self.free_temp(val_r);
                            // PEP 657 (#2582): the `{...}` field caret covers a
                            // conversion (`__repr__`/`__str__`/`ascii`) that raises.
                            self.set_col_span_for_next(*span);
                            self.emit(Insn::Call(frame, 1));
                            self.next_temp = frame + 1;
                            frame
                        }
                        _ => val_r,
                    };
                    // Apply format spec if present via the dedicated
                    // `FormatValueSpec` / `FormatValue` opcodes (#1926).
                    let field_r = if let Some(spec_parts) = format_spec {
                        // Nested spec fields carry their own absolute `line`.
                        let spec_r = self.compile_fstring(spec_parts);
                        let dst = self.alloc_temp();
                        // PEP 657 (#2582): the `{...}` field caret covers a
                        // `__format__` that raises.
                        self.set_col_span_for_next(*span);
                        self.emit(Insn::FormatValueSpec(dst, val_r, spec_r));
                        self.free_temp(val_r);
                        self.free_temp(spec_r);
                        dst
                    } else {
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
