impl Compiler {
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

    /// Emit code that tests whether register `subj` matches `pattern`.
    /// On mismatch, jumps via newly-pushed entries in `fail_patches`
    /// (caller will patch them all to the next arm).
    /// On match, binds any capture variables and falls through.
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
