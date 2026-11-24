impl Compiler {
    /// Compile a sequence pattern (`[a, b, *rest]`): exclude non-sequence types,
    /// length-check the subject, then destructure each element (and the star).
    fn compile_sequence_pattern(
        &mut self,
        subj: Reg,
        elements: &[(Pattern, bool)],
        fail_patches: &mut Vec<usize>,
    ) {
        // PEP 634 §3: str, bytes, dict, set, and frozenset are excluded
        // from sequence pattern matching. str/bytes are text sequences;
        // dict/set/frozenset support len() but not integer indexing.
        // A single `MatchSeqExcluded` instruction computes
        // `isinstance(subj, (str, bytes, dict, set, frozenset))` directly
        // — no per-arm `LoadGlobal`/`BuildTuple`/`Call` to rebuild the
        // exclusion tuple on every match execution (issue #1789).
        {
            let excluded = self.alloc_temp();
            self.emit(Insn::MatchSeqExcluded(excluded, subj));
            let jmp = self.emit(Insn::JumpIfTrue(excluded, 0));
            fail_patches.push(jmp);
            self.free_temp(excluded);
        }

        // Check that subject has exactly `fixed_count` elements
        // (unless there's a star element, then >= fixed_count).
        let has_star = elements.iter().any(|(_, is_star)| *is_star);
        let fixed_count = elements.iter().filter(|(_, s)| !s).count();

        // R_len = len(subj).  Wrap the call in try/except so that a
        // TypeError (subject has no __len__) is treated as a sequence
        // mismatch rather than a propagated error — matching CPython's
        // behaviour for non-sequence types inside OR patterns.
        let len_name_idx = self.intern_name("len");
        let setup_idx = self.emit(Insn::SetupExcept(0));
        let len_fn = self.alloc_temp();
        self.emit(Insn::LoadGlobal(len_fn, len_name_idx));
        let len_arg = self.alloc_temp();
        self.emit(Insn::Move(len_arg, subj));
        self.emit(Insn::Call(len_fn, 1));
        let r_len = len_fn; // result in len_fn after call
        self.free_temp(len_arg);
        // Success path: remove the exception handler.
        self.emit(Insn::PopExcept);
        let jmp_over_handler = self.emit(Insn::Jump(0));
        // Exception handler: any error from len() means the subject is
        // not a sequence — treat as match failure.
        self.patch_jump(setup_idx);
        self.emit(Insn::EndExcept);
        let len_err_jmp = self.emit(Insn::Jump(0));
        fail_patches.push(len_err_jmp);
        self.patch_jump(jmp_over_handler);

        // Check length
        let count_val = self.intern_const(Value::int(fixed_count as i64));
        let len_jmp = if has_star {
            self.emit(Insn::CmpJumpIfFalseConst(r_len, BinaryOp::Ge, count_val, 0))
        } else {
            self.emit(Insn::CmpJumpIfFalseConst(r_len, BinaryOp::Eq, count_val, 0))
        };
        fail_patches.push(len_jmp);
        self.free_temp(r_len);

        // Destructure each element.
        let mut fixed_idx: i64 = 0;
        let mut star_seen = false;
        let total = elements.len();
        for (elem_i, (elem_pat, is_star)) in elements.iter().enumerate() {
            if *is_star {
                star_seen = true;
                // Star element captures subj[fixed_idx : len - (total - elem_i - 1)]
                let trailing = (total - elem_i - 1) as i64;
                if let Pattern::Capture(name) = elem_pat {
                    // Compute start index
                    let start_c = self.intern_const(Value::int(fixed_idx));
                    let start_r = self.alloc_temp();
                    self.emit(Insn::LoadConst(start_r, start_c));
                    // Compute stop index: re-compute len
                    let len2_fn = self.alloc_temp();
                    self.emit(Insn::LoadGlobal(len2_fn, len_name_idx));
                    let arg2 = self.alloc_temp();
                    self.emit(Insn::Move(arg2, subj));
                    self.emit(Insn::Call(len2_fn, 1));
                    self.free_temp(arg2);
                    let r_len2 = len2_fn;
                    let stop_r = if trailing > 0 {
                        let trail_c = self.intern_const(Value::int(trailing));
                        let trail_r = self.alloc_temp();
                        self.emit(Insn::LoadConst(trail_r, trail_c));
                        let stop = self.alloc_temp();
                        self.emit(Insn::BinOp(stop, r_len2, BinaryOp::Sub, trail_r));
                        self.free_temp(trail_r);
                        self.free_temp(r_len2);
                        stop
                    } else {
                        r_len2
                    };
                    // Build slice subj[start:stop] via BuildSlice (issue #931).
                    let base = self.alloc_temp();
                    self.emit(Insn::Move(base, start_r));
                    let base1 = self.alloc_temp();
                    self.emit(Insn::Move(base1, stop_r));
                    let base2 = self.alloc_temp();
                    self.emit(Insn::LoadNone(base2));
                    let slice_key = self.alloc_temp();
                    self.emit(Insn::BuildSlice(slice_key, base));
                    self.free_temp(base2);
                    self.free_temp(base1);
                    self.free_temp(base);
                    self.free_temp(stop_r);
                    self.free_temp(start_r);
                    // CPython guarantees *rest is always a list regardless
                    // of the subject's type, so convert via BuildList + ListExtend.
                    let saved_next = self.next_temp;
                    let slice_r = self.alloc_temp();
                    self.emit(Insn::GetItem(slice_r, subj, slice_key));
                    self.free_temp(slice_key);
                    let list_r = self.alloc_temp();
                    let empty_base = self.next_temp;
                    self.next_temp = empty_base + 1;
                    if empty_base > self.max_reg {
                        self.max_reg = empty_base;
                    }
                    self.emit(Insn::BuildList(list_r, empty_base, 0));
                    self.emit(Insn::ListExtend(list_r, slice_r));
                    self.compile_store_name(name, list_r);
                    if let Some(reg) = self.local_reg(name) {
                        self.mark_def(reg);
                    }
                    self.next_temp = saved_next;
                }
                continue;
            }
            // Compute index: before star use fixed_idx from left; after star index from right.
            let idx_val = if !star_seen {
                fixed_idx
            } else {
                let after_star = elements[elem_i..].iter().filter(|(_, s)| !s).count() as i64;
                -(after_star)
            };
            if !star_seen {
                fixed_idx += 1;
            }

            let idx_c = self.intern_const(Value::int(idx_val));
            let idx_r = self.alloc_temp();
            self.emit(Insn::LoadConst(idx_r, idx_c));
            let elem_r = self.alloc_temp();
            self.emit(Insn::GetItem(elem_r, subj, idx_r));
            self.free_temp(idx_r);
            self.compile_pattern_match(elem_r, elem_pat, fail_patches);
            self.free_temp(elem_r);
            if self.failed {
                return;
            }
        }
    }

    /// Compile a mapping pattern (`{k: p, **rest}`): for each key check
    /// membership and match the value sub-pattern, then bind any `**rest`.
    fn compile_mapping_pattern(
        &mut self,
        subj: Reg,
        pairs: &[(Expr, Pattern)],
        rest_name: Option<&str>,
        fail_patches: &mut Vec<usize>,
    ) {
        // PEP 634 §3: a mapping pattern matches only if the subject is a
        // mapping (`isinstance(subject, collections.abc.Mapping)`).
        {
            let is_map = self.alloc_temp();
            self.emit(Insn::MatchMapping(is_map, subj));
            let jmp = self.emit(Insn::JumpIfFalse(is_map, 0));
            fail_patches.push(jmp);
            self.free_temp(is_map);
        }

        // For each key-pattern pair: check key in subject, then match pattern.
        let in_name_idx = self.intern_name("__contains__");
        let _ = in_name_idx; // used indirectly via BinaryOp::In

        for (key_expr, val_pat) in pairs {
            let key_r = self.compile_expr(key_expr);
            let check_r = self.alloc_temp();
            self.emit(Insn::BinOp(check_r, key_r, BinaryOp::In, subj));
            let jmp = self.emit(Insn::JumpIfFalse(check_r, 0));
            self.free_temp(check_r);
            fail_patches.push(jmp);
            let val_r = self.alloc_temp();
            self.emit(Insn::GetItem(val_r, subj, key_r));
            self.free_temp(key_r);
            self.compile_pattern_match(val_r, val_pat, fail_patches);
            self.free_temp(val_r);
            if self.failed {
                return;
            }
        }
        // If there's a **rest, bind it to subj minus matched keys.
        if let Some(rest) = rest_name {
            let dict_name_idx = self.intern_name("dict");
            let dict_fn = self.alloc_temp();
            self.emit(Insn::LoadGlobal(dict_fn, dict_name_idx));
            let arg = self.alloc_temp();
            self.emit(Insn::Move(arg, subj));
            self.emit(Insn::Call(dict_fn, 1));
            self.free_temp(arg);
            let rest_r = dict_fn;
            for (key_expr, _) in pairs {
                let k = self.compile_expr(key_expr);
                self.emit(Insn::DeleteItem(rest_r, k));
                self.free_temp(k);
            }
            self.compile_store_name(rest, rest_r);
            if let Some(reg) = self.local_reg(rest) {
                self.mark_def(reg);
            }
            self.free_temp(rest_r);
        }
    }

    /// Compile a class pattern (`C(p, ..., attr=p)`): isinstance-check the
    /// subject, then match positional (via `__match_args__`) and keyword attrs.
    fn compile_class_pattern(
        &mut self,
        subj: Reg,
        cls: &Expr,
        positional: &[Pattern],
        kwargs: &[(Symbol, Pattern)],
        fail_patches: &mut Vec<usize>,
    ) {
        // isinstance(subj, cls) check must come FIRST so that attribute
        // access is never attempted on a subject of the wrong type.
        let isinstance_name_idx = self.intern_name("isinstance");
        let isinstance_fn = self.alloc_temp();
        self.emit(Insn::LoadGlobal(isinstance_fn, isinstance_name_idx));
        let arg0 = self.alloc_temp();
        self.emit(Insn::Move(arg0, subj));
        let cls_r = self.compile_expr(cls);
        let arg1 = self.alloc_temp();
        self.emit(Insn::Move(arg1, cls_r));
        // Keep cls_r alive when we have positional sub-patterns: the
        // MatchClassPositional instruction needs the class to load __match_args__.
        let cls_for_pos = if !positional.is_empty() {
            let saved = self.alloc_temp();
            self.emit(Insn::Move(saved, cls_r));
            self.free_temp(cls_r);
            Some(saved)
        } else {
            self.free_temp(cls_r);
            None
        };
        self.emit(Insn::Call(isinstance_fn, 2));
        self.free_temp(arg1);
        self.free_temp(arg0);
        let jmp = self.emit(Insn::JumpIfFalse(isinstance_fn, 0));
        fail_patches.push(jmp);
        self.free_temp(isinstance_fn);
        // Positional sub-patterns: resolved via __match_args__.
        if !positional.is_empty() {
            let cls_reg = cls_for_pos.expect("set above when positional non-empty");
            let n = match u32::try_from(positional.len()) {
                Ok(count) => count,
                Err(_) => {
                    self.failed = true;
                    if self.error_msg.is_none() {
                        self.error_msg = Some("too many positional class sub-patterns".to_string());
                    }
                    return;
                }
            };
            let dst_base = self.alloc_temp();
            for _ in 1..positional.len() {
                self.alloc_temp();
            }
            let desc_idx =
                self.push_desc(InsnDesc::MatchClassPositional(MatchClassPositionalDesc {
                    dst_base,
                    subj,
                    cls: cls_reg,
                    n,
                }));
            self.emit(Insn::MatchClassPositional(desc_idx));
            self.free_temp(cls_reg);
            for (i, pat) in positional.iter().enumerate() {
                let attr_r = dst_base + i as u32;
                self.compile_pattern_match(attr_r, pat, fail_patches);
                if self.failed {
                    for j in i..positional.len() {
                        self.free_temp(dst_base + j as u32);
                    }
                    return;
                }
                self.free_temp(attr_r);
            }
        } else if let Some(cls_reg) = cls_for_pos {
            self.free_temp(cls_reg);
        }
        // Keyword sub-patterns: matched directly against named attributes.
        for (attr_name, attr_pat) in kwargs {
            let name_idx = self.intern_name(attr_name);
            let attr_r = self.alloc_temp();
            self.emit(Insn::GetAttr(attr_r, subj, name_idx));
            self.compile_pattern_match(attr_r, attr_pat, fail_patches);
            self.free_temp(attr_r);
            if self.failed {
                return;
            }
        }
    }
}
