impl Compiler {
    fn compile_if(
        &mut self,
        branches: &[(Expr, Vec<Stmt>)],
        else_branch: Option<&[Stmt]>,
        branch_linenos: &[Vec<u32>],
        else_linenos: &[u32],
    ) {
        // `if COND: break` / `if COND: continue` with no elif/else lowers to a
        // single conditional jump straight at the loop's break/continue target.
        // The general lowering emits `JumpIfFalse(+1); Jump(target)`, whose
        // taken skip-jump sits on the per-iteration path of `while True:` loops;
        // the fused form removes it and lets loop inversion later convert the
        // back-edge into a conditional re-check (issue #282).  Truthiness is
        // still evaluated exactly once per test and the jump keeps the `if`
        // statement's source attribution, so protocol semantics and raise
        // attribution are unchanged for every operand type.  Bail when try/with
        // cleanups would have to run before the jump, or when the condition is
        // constant (the general path owns dead-branch validation).
        if let [(cond, body)] = branches
            && else_branch.is_none()
            && let [only] = body.as_slice()
            && (matches!(only, Stmt::Break)
                || (matches!(only, Stmt::Continue)
                    && self
                        .loops
                        .last()
                        .is_some_and(|l| l.continue_target.is_some())))
            && !self.loops.is_empty()
            && fold_constant(cond).is_none()
            && self.except_cleanups.len() <= self.loops[self.loops.len() - 1].cleanup_depth
        {
            let cond_reg = self.compile_expr(cond);
            let jmp = self.emit_cond_jump(cond_reg, true);
            self.free_temp(cond_reg);
            let last = self.loops.len() - 1;
            match only {
                Stmt::Break => self.loops[last].break_patches.push(jmp),
                Stmt::Continue => {
                    let target = self.loops[last]
                        .continue_target
                        .expect("guarded by peephole eligibility");
                    let offset = target as i32 - (jmp as i32 + 1);
                    if let Insn::JumpIfTrue(_, off) = &mut self.insns[jmp] {
                        *off = offset;
                    }
                }
                _ => unreachable!(),
            }
            // Definite-assignment: the taken branch binds nothing and leaves
            // the statement, so the fall-through def-set is unchanged.
            return;
        }
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
}
