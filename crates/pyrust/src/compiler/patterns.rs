impl Compiler {
    fn compile_if(
        &mut self,
        branches: &[(Expr, Vec<Stmt>)],
        else_branch: Option<&[Stmt]>,
        branch_linenos: &[Vec<u32>],
        else_linenos: &[u32],
    ) {
        let has_else = else_branch.is_some();
        let n = branches.len();
        let mut end_patches: Vec<usize> = Vec::new();
        let pre_def_set = self.def_set;
        // Collect def_set after each branch body for definite-assignment analysis.
        let mut branch_def_sets: Vec<u64> = Vec::with_capacity(n + 1);

        for (bi, (cond, body)) in branches.iter().enumerate() {
            self.def_set = pre_def_set;
            let body_lns: &[u32] = branch_linenos.get(bi).map(|v| v.as_slice()).unwrap_or(&[]);
            // Constant-condition optimisation: fold at compile time.
            if let Some(val) = fold_constant(cond) {
                if val.truthy_raw() {
                    // Always-true branch: compile body unconditionally; skip rest.
                    self.compile_block_with_linenos(body, body_lns);
                    if self.failed {
                        return;
                    }
                    branch_def_sets.push(self.def_set);
                    // Treat as if there were an else so intersection analysis kicks in.
                    for _ in bi + 1..n {
                        branch_def_sets.push(pre_def_set);
                    }
                    if has_else {
                        branch_def_sets.push(pre_def_set);
                    }
                    // Skipped elif/else bodies are dead code but CPython still
                    // validates their context-sensitive syntax.
                    let in_loop = !self.loops.is_empty();
                    for (_, skipped_body) in &branches[bi + 1..] {
                        self.check_dead_block(skipped_body, in_loop);
                        if self.failed {
                            return;
                        }
                        // A `yield`/`yield from` in a skipped branch still makes
                        // the enclosing function a generator (CPython parity,
                        // issue #1758).
                        if self.is_function_scope && stmts_contain_yield(skipped_body) {
                            self.has_dead_yield = true;
                        }
                    }
                    if let Some(else_stmts) = else_branch {
                        self.check_dead_block(else_stmts, in_loop);
                        if self.failed {
                            return;
                        }
                        if self.is_function_scope && stmts_contain_yield(else_stmts) {
                            self.has_dead_yield = true;
                        }
                    }
                    for idx in end_patches {
                        self.patch_jump(idx);
                    }
                    if has_else && !branch_def_sets.is_empty() {
                        let all_define = branch_def_sets.iter().fold(!0u64, |acc, &s| acc & s);
                        self.def_set = pre_def_set | all_define;
                    } else {
                        self.def_set = pre_def_set | branch_def_sets[0];
                    }
                    // Validate skipped elif/else bodies as dead code so that
                    // context-sensitive syntax errors are not silently swallowed.
                    let in_loop = !self.loops.is_empty();
                    for (_, dead_body) in &branches[bi + 1..] {
                        self.check_dead_block(dead_body, in_loop);
                        if self.failed {
                            return;
                        }
                    }
                    if let Some(else_stmts) = else_branch {
                        self.check_dead_block(else_stmts, in_loop);
                    }
                    return;
                } else {
                    // Always-false branch: skip emitting code, but still
                    // validate context-sensitive syntax (CPython does this).
                    self.check_dead_block(body, !self.loops.is_empty());
                    if self.failed {
                        return;
                    }
                    // A `yield` / `yield from` in a dead branch still makes
                    // the enclosing function a generator (CPython parity,
                    // issue #1758).  No `Insn::Yield` is emitted for this
                    // branch, so flag it explicitly for `finish()`.
                    if self.is_function_scope && stmts_contain_yield(body) {
                        self.has_dead_yield = true;
                    }
                    continue;
                }
            }
            let cond_reg = self.compile_expr(cond);
            let jmp_false = self.emit_cond_jump(cond_reg, false);
            self.free_temp(cond_reg);
            self.compile_block_with_linenos(body, body_lns);
            if self.failed {
                return;
            }
            branch_def_sets.push(self.def_set);
            self.def_set = pre_def_set;
            if bi < n - 1 || has_else {
                let jmp_end = self.emit(Insn::Jump(0));
                end_patches.push(jmp_end);
            }
            self.patch_jump(jmp_false);
        }
        if let Some(else_stmts) = else_branch {
            self.compile_block_with_linenos(else_stmts, else_linenos);
            if self.failed {
                return;
            }
            branch_def_sets.push(self.def_set);
        }
        for idx in end_patches {
            self.patch_jump(idx);
        }
        // A variable is definitely bound after the if/elif/else iff it is bound
        // on every possible exit path.
        // With an else: exactly one branch executes, so intersect all branches.
        // Without an else: control may skip all branches (pre_def_set path) or
        // take one branch (branch_def_sets[i] path).  Intersect everything.
        if has_else && !branch_def_sets.is_empty() {
            self.def_set = branch_def_sets.iter().fold(!0u64, |acc, &s| acc & s);
        } else {
            // Include the "no branch taken" path (pre_def_set) in the intersection.
            self.def_set = branch_def_sets.iter().fold(pre_def_set, |acc, &s| acc & s);
        }
    }

    // ── Match/case ────────────────────────────────────────────────────────────

    fn compile_match(&mut self, subject: &Expr, arms: &[MatchArm]) {
        // Evaluate the subject once into a temp register.
        let subj = self.compile_expr(subject);
        let pre_def_set = self.def_set;
        let mut end_patches: Vec<usize> = Vec::new();
        let mut all_arm_def_sets: Vec<u64> = Vec::new();

        for arm in arms {
            self.def_set = pre_def_set;
            // Emit pattern-matching code; collect jump-to-next-arm patches.
            let mut next_arm_patches: Vec<usize> = Vec::new();
            self.compile_pattern_match(subj, &arm.pattern, &mut next_arm_patches);
            if self.failed {
                return;
            }
            // If there's a guard, test it.
            if let Some(guard_expr) = &arm.guard {
                let g = self.compile_expr(guard_expr);
                let jmp = self.emit(Insn::JumpIfFalse(g, 0));
                self.free_temp(g);
                next_arm_patches.push(jmp);
            }
            // Arm body
            self.compile_block_with_linenos(&arm.body, &arm.body_linenos);
            if self.failed {
                return;
            }
            all_arm_def_sets.push(self.def_set);
            // Jump past remaining arms after successful execution.
            let jmp_end = self.emit(Insn::Jump(0));
            end_patches.push(jmp_end);
            // Patch all "no match" jumps to land here (start of next arm).
            for idx in next_arm_patches {
                self.patch_jump(idx);
            }
        }

        // Patch all end-of-arm jumps to land after the whole match.
        for idx in end_patches {
            self.patch_jump(idx);
        }
        self.free_temp(subj);
        // Variables defined in every arm are definitely bound after the match.
        let all_define = if all_arm_def_sets.is_empty() {
            0
        } else {
            all_arm_def_sets.iter().fold(!0u64, |acc, &s| acc & s)
        };
        self.def_set = pre_def_set | all_define;
    }

    /// Emit code that tests whether register `subj` matches `pattern`.
    /// On mismatch, jumps via newly-pushed entries in `fail_patches`
    /// (caller will patch them all to the next arm).
    /// On match, binds any capture variables and falls through.
    /// Compile an OR pattern (`a | b | c`): validate alternatives bind the same
    /// names, then try each in turn, jumping to success on the first match.
    fn compile_or_pattern(
        &mut self,
        subj: Reg,
        alternatives: &[Pattern],
        fail_patches: &mut Vec<usize>,
    ) {
        // Validate that every alternative binds the same set of names
        // (PEP 634; CPython 3.12 raises SyntaxError if they differ).
        //
        // Check first: a bare name capture or wildcard in a non-last
        // position makes every subsequent alternative unreachable —
        // CPython 3.12 emits a dedicated message for each case,
        // distinct from the generic "bind different names" error.
        let non_last = alternatives.len().saturating_sub(1);
        for alt in alternatives.iter().take(non_last) {
            // Recurse into the leading edge of nested OR patterns so that
            // `case (x | 1) | z:` is caught the same way as `case x | z:`.
            if let Some(name) = or_leading_capture(alt) {
                self.set_syntax_error(&format!(
                    "name capture '{}' makes remaining patterns unreachable",
                    name
                ));
                return;
            }
            if or_leading_is_wildcard(alt) {
                self.set_syntax_error("wildcard makes remaining patterns unreachable");
                return;
            }
        }
        if let Some(first) = alternatives.first() {
            let first_names = pattern_bound_names(first);
            for alt in alternatives.iter().skip(1) {
                if pattern_bound_names(alt) != first_names {
                    self.set_syntax_error("alternative patterns bind different names");
                    return;
                }
            }
        }
        // Try each alternative; if one matches, jump to success.
        // If all fail, fall through to after (which will be patched to next arm).
        let mut success_patches: Vec<usize> = Vec::new();
        let n = alternatives.len();
        for (i, alt) in alternatives.iter().enumerate() {
            let mut alt_fail: Vec<usize> = Vec::new();
            self.compile_pattern_match(subj, alt, &mut alt_fail);
            if self.failed {
                return;
            }
            if i < n - 1 {
                // This alternative matched — jump to success.
                let jmp_ok = self.emit(Insn::Jump(0));
                success_patches.push(jmp_ok);
                // Patch the fail of this alternative to try the next one.
                for idx in alt_fail {
                    self.patch_jump(idx);
                }
            } else {
                // Last alternative: its failures propagate to caller.
                fail_patches.extend(alt_fail);
            }
        }
        for idx in success_patches {
            self.patch_jump(idx);
        }
    }

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
        // exclusion tuple on every match execution (issue #1789).  If
        // subj IS one of the excluded types, jump to the fail label.
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
                // Star element captures subj[fixed_idx:]
                // i.e., subj[fixed_idx : len - (total - elem_i - 1)]
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
                    // Arrange the three bounds in consecutive registers and emit
                    // BuildSlice so the VM unambiguously identifies it as a slice
                    // (not a user 3-tuple).
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
                    // Get the slice: subj[start:stop] via GetItem with a slice key.
                    // The slice result preserves the subject's type (e.g. tuple →
                    // tuple). CPython guarantees *rest is always a list regardless
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
                    // Store into capture name
                    self.compile_store_name(name, list_r);
                    if let Some(reg) = self.local_reg(name) {
                        self.mark_def(reg);
                    }
                    // slice_r / list_r / empty_base cannot be freed in LIFO
                    // order because the phantom empty_base slot sits above
                    // list_r. All three are dead after the store, so restore
                    // next_temp explicitly; max_reg already reflects peak
                    // usage from the empty_base bump above.
                    self.next_temp = saved_next;
                }
                // Don't increment fixed_idx for the star element itself.
                continue;
            }
            // Compute index: if we haven't seen the star yet, use fixed_idx from left.
            // After the star, index from the right.
            let idx_val = if !star_seen {
                fixed_idx
            } else {
                // Negative index (from end): -(fixed_count after star) + offset
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
        // mapping (`isinstance(subject, collections.abc.Mapping)`).  Guard on
        // that first so a non-mapping subject (int, str, list, set, None, …)
        // fails the match rather than raising on the per-key `in` test below
        // (issue #1879).  Mirrors the `MatchSeqExcluded` gate in
        // `compile_sequence_pattern`; in pyrust the only built-in mapping is
        // `dict` (and its subclasses).
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
            // Check: key in subj
            let check_r = self.alloc_temp();
            self.emit(Insn::BinOp(check_r, key_r, BinaryOp::In, subj));
            let jmp = self.emit(Insn::JumpIfFalse(check_r, 0));
            self.free_temp(check_r);
            fail_patches.push(jmp);
            // Get the value: subj[key]
            let val_r = self.alloc_temp();
            self.emit(Insn::GetItem(val_r, subj, key_r));
            self.free_temp(key_r);
            // Match sub-pattern against the value
            self.compile_pattern_match(val_r, val_pat, fail_patches);
            self.free_temp(val_r);
            if self.failed {
                return;
            }
        }
        // If there's a **rest, bind it to subj minus matched keys.
        if let Some(rest) = rest_name {
            // Build a copy of subj and remove matched keys.
            // Simplest: call dict(subj) then del keys.
            let dict_name_idx = self.intern_name("dict");
            let dict_fn = self.alloc_temp();
            self.emit(Insn::LoadGlobal(dict_fn, dict_name_idx));
            let arg = self.alloc_temp();
            self.emit(Insn::Move(arg, subj));
            self.emit(Insn::Call(dict_fn, 1));
            self.free_temp(arg);
            let rest_r = dict_fn; // result in dict_fn
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
        kwargs: &[(String, Pattern)],
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
        // MatchClassPositional instruction needs the class to load
        // __match_args__ from it.
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
            // Allocate a contiguous block of n temporaries for the
            // attribute values loaded by MatchClassPositional.
            let dst_base = self.alloc_temp();
            for _ in 1..positional.len() {
                self.alloc_temp();
            }
            self.emit(Insn::MatchClassPositional {
                dst_base,
                subj,
                cls: cls_reg,
                n,
            });
            self.free_temp(cls_reg);
            // Match each positional attribute value against its sub-pattern.
            for (i, pat) in positional.iter().enumerate() {
                let attr_r = dst_base + i as u32;
                self.compile_pattern_match(attr_r, pat, fail_patches);
                if self.failed {
                    // Free remaining allocated registers before returning.
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

    fn compile_pattern_match(
        &mut self,
        subj: Reg,
        pattern: &Pattern,
        fail_patches: &mut Vec<usize>,
    ) {
        if self.failed {
            return;
        }
        match pattern {
            Pattern::Wildcard => {
                // Always matches, nothing to do.
            }
            Pattern::Capture(name) => {
                // Bind subj to name, always succeeds.
                self.compile_store_name(name, subj);
                if let Some(reg) = self.local_reg(name) {
                    self.mark_def(reg);
                }
            }
            Pattern::Literal(expr) => {
                // Emit: if subj != literal → fail
                let lit = self.compile_expr(expr);
                let jmp = self.emit(Insn::CmpJumpIfFalse(subj, BinaryOp::Eq, lit, 0));
                self.free_temp(lit);
                fail_patches.push(jmp);
            }
            Pattern::Value(expr) => {
                // Value pattern: evaluate the dotted attribute expression and
                // compare with == (same as a literal match, no binding).
                let val = self.compile_expr(expr);
                let jmp = self.emit(Insn::CmpJumpIfFalse(subj, BinaryOp::Eq, val, 0));
                self.free_temp(val);
                fail_patches.push(jmp);
            }
            Pattern::Or(alternatives) => {
                self.compile_or_pattern(subj, alternatives, fail_patches);
            }
            Pattern::Sequence(elements) => {
                self.compile_sequence_pattern(subj, elements, fail_patches);
            }
            Pattern::Mapping(pairs, rest_name) => {
                self.compile_mapping_pattern(subj, pairs, rest_name.as_deref(), fail_patches);
            }
            Pattern::Class {
                cls,
                positional,
                kwargs,
            } => {
                self.compile_class_pattern(subj, cls, positional, kwargs, fail_patches);
            }
            Pattern::As { pattern, name } => {
                // Compile the inner pattern first (may add to fail_patches).
                self.compile_pattern_match(subj, pattern, fail_patches);
                if self.failed {
                    return;
                }
                // If we reach here the inner pattern matched; bind the entire
                // subject (not just the matched portion) to `name`.
                self.compile_store_name(name, subj);
                if let Some(reg) = self.local_reg(name) {
                    self.mark_def(reg);
                }
            }
        }
    }
}
