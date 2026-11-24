// ─── PRE planning, materialization, and driver ────────────────────────────────

/// Step 7: Plan compensation insertions & verify strict safety + profitability
/// independently for each candidate expression.
#[allow(clippy::too_many_arguments)]
fn pre_plan_exprs(
    insns: &[Insn],
    descs: &[InsnDesc],
    consts: &[Value],
    num_locals: u32,
    num_regs: u32,
    cfg: &PreCfg,
    type_in: &[Option<PreTypeMap>],
    exprs: &[PreExpr],
    pc_expr: &[Option<(u32, PreExpr)>],
    df: &PreDataflow,
) -> (Vec<(usize, PreExprPlan)>, u32) {
    let num_blocks = cfg.num_blocks();

    // Ensure fresh temp registers never collide with existing registers.
    let mut max_reg_used = num_regs.max(num_locals);
    for insn in insns {
        for_each_read(insn, descs, |r| {
            if r + 1 > max_reg_used {
                max_reg_used = r + 1;
            }
        });
        for_each_write(insn, descs, |w| {
            if w + 1 > max_reg_used {
                max_reg_used = w + 1;
            }
        });
    }

    let mut plans: Vec<(usize, PreExprPlan)> = Vec::new();
    let mut next_temp = max_reg_used;

    for (bit, &expr) in exprs.iter().enumerate() {
        let mask = 1u64 << bit;

        // Seed blocks where `e` is both anticipated (down-safe) AND partially
        // available (redundant on at least one incoming path).
        let mut need_in = vec![false; num_blocks];
        let mut any_seed = false;
        for b in 0..num_blocks {
            if type_in[b].is_some() && ((df.ant_in[b] & df.pav_in[b]) & mask) != 0 {
                need_in[b] = true;
                any_seed = true;
            }
        }
        if !any_seed {
            continue;
        }

        // Determine compensation insertions for predecessors of `need_in` blocks
        // that do not already make `e` available (`real_av_out[p] == 0`).
        let mut exit_insert = vec![false; num_blocks];
        let mut ft_insert = vec![false; num_blocks];
        let mut jump_insert: Vec<(usize, usize)> = Vec::new();
        let mut aborted = false;

        'plan_loop: for b in 0..num_blocks {
            if !need_in[b] {
                continue;
            }
            for &p in &cfg.preds[b] {
                if type_in[p].is_none() {
                    continue;
                }
                if !cfg.safe_edge(p, b) {
                    aborted = true;
                    break 'plan_loop;
                }
                if (df.real_av_out[p] & mask) != 0 {
                    // Already available at exit of `p`.
                    continue;
                }
                if (df.ant_out[p] & mask) != 0 {
                    // Down-safe at exit of `p` (covers both single-successor
                    // blocks and diamond headers where ALL successors anticipate
                    // `e`).  Insert once at the exit of `p`.
                    exit_insert[p] = true;
                } else {
                    // Critical edge `p -> b`: `p` has another successor that
                    // does NOT anticipate `e`.
                    let p_last = cfg.blocks[p].1 - 1;
                    let b_start = cfg.blocks[b].0;
                    if p_last + 1 == b_start && cfg.succs[p].len() > 1 {
                        // Fallthrough edge from conditional branch in `p`:
                        // safe to insert inline right after `p_last` and before
                        // `b_start` (only executed on `p -> b` fallthrough!).
                        if (df.type_safe_out[p] & mask) != 0 && (df.term_kills[p] & mask) == 0 {
                            ft_insert[p] = true;
                        } else {
                            aborted = true;
                            break 'plan_loop;
                        }
                    } else if cfg.block_region[p] == 0
                        && cfg.block_region[b] == 0
                        && (df.type_safe_out[p] & mask) != 0
                        && (df.term_kills[p] & mask) == 0
                    {
                        if !jump_insert.contains(&(p, b)) {
                            jump_insert.push((p, b));
                        }
                    } else {
                        aborted = true;
                        break 'plan_loop;
                    }
                }
            }
        }
        if aborted {
            continue;
        }

        // Forward MUST verification: verify `temp_reg` availability across the
        // entire CFG given the planned compensation insertions and original
        // evaluations of `e`.  Any original evaluation where `temp_reg` is
        // already available becomes a `Move(dst, temp_reg)`; any other becomes
        // a capture (`Move(temp_reg, dst)` right after the original insn).
        let mut t_in = vec![false; num_blocks];
        let mut t_out = vec![false; num_blocks];
        for b in 1..num_blocks {
            if type_in[b].is_some() {
                t_in[b] = true;
                t_out[b] = true;
            }
        }
        let edge_has_insert = |p: usize, b: usize| -> bool {
            let p_last = cfg.blocks[p].1 - 1;
            let b_start = cfg.blocks[b].0;
            (ft_insert[p] && p_last + 1 == b_start) || jump_insert.contains(&(p, b))
        };

        t_out[0] = (df.comp[0] & mask) != 0 || exit_insert[0];
        loop {
            let mut changed = false;
            for b in 0..num_blocks {
                if type_in[b].is_none() {
                    continue;
                }
                let new_in = if b == 0 {
                    false
                } else {
                    let mut all_ok = true;
                    let mut any_p = false;
                    for &p in &cfg.preds[b] {
                        if type_in[p].is_none() {
                            continue;
                        }
                        any_p = true;
                        let edge_avail = cfg.safe_edge(p, b) && (t_out[p] || edge_has_insert(p, b));
                        if !edge_avail {
                            all_ok = false;
                            break;
                        }
                    }
                    any_p && all_ok
                };
                let body_transp = (df.transp_body[b] & mask) != 0;
                let term_ok = (df.term_kills[b] & mask) == 0;
                let new_out = ((df.comp[b] & mask) != 0
                    || (new_in && body_transp && term_ok)
                    || (exit_insert[b] && term_ok))
                    && (df.type_safe_out[b] & mask) != 0;
                if new_in != t_in[b] || new_out != t_out[b] {
                    t_in[b] = new_in;
                    t_out[b] = new_out;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        // Classify every reachable evaluation of `e` as either a replacement
        // (strictly redundant!) or a capture.
        let mut replace_pcs = Vec::new();
        let mut capture_pcs = Vec::new();
        for (b, &(b_start, b_end)) in cfg.blocks.iter().enumerate() {
            let Some(mut state) = type_in[b].clone() else {
                continue;
            };
            let in_try = cfg.block_region[b] != 0;
            let has_term = insn_ends_basic_block(&insns[b_end - 1]);
            let body_end = if has_term { b_end - 1 } else { b_end };
            let mut avail = t_in[b];

            for pc in b_start..body_end {
                let insn = &insns[pc];
                let is_this_expr = pc_expr[pc].is_some_and(|(_, e)| e == expr);
                if is_this_expr {
                    if avail {
                        replace_pcs.push(pc);
                    } else {
                        capture_pcs.push(pc);
                    }
                }
                let (pure, may_raise) = pre_step_types(insn, descs, &mut state, consts, num_locals);
                let mut killed = false;
                if in_try && may_raise {
                    killed = true;
                } else {
                    if !pure && expr.reads_named_local(num_locals) {
                        killed = true;
                    }
                    for_each_write(insn, descs, |w| {
                        if expr.reads_reg(w) {
                            killed = true;
                        }
                    });
                }
                if is_this_expr && !killed {
                    avail = true;
                } else if killed {
                    avail = false;
                }
            }
        }

        // Profitability & soundness guard: commit only when at least one
        // original evaluation is genuinely replaced by `Move(dst, temp_reg)`.
        if replace_pcs.is_empty() {
            continue;
        }

        let temp_reg = next_temp;
        next_temp += 1;
        let exit_blocks: Vec<usize> = exit_insert
            .into_iter()
            .enumerate()
            .filter_map(|(b, flag)| flag.then_some(b))
            .collect();
        let fallthrough_edges: Vec<usize> = ft_insert
            .into_iter()
            .enumerate()
            .filter_map(|(b, flag)| flag.then_some(b))
            .collect();

        plans.push((
            bit,
            PreExprPlan {
                temp_reg,
                exit_blocks,
                fallthrough_edges,
                jump_edges: jump_insert,
                replace_pcs,
                capture_pcs,
            },
        ));
    }

    (plans, next_temp)
}

/// Step 8: Materialize the transformed instruction stream with accurate jump
/// offset rewriting and critical-edge trampolines (if any).
fn pre_materialize(
    insns: &[Insn],
    cfg: &PreCfg,
    exprs: &[PreExpr],
    pc_expr: &[Option<(u32, PreExpr)>],
    plans: &[(usize, PreExprPlan)],
) -> Vec<Insn> {
    let n = insns.len();
    let num_blocks = cfg.num_blocks();

    let mut before_pc: Vec<Vec<Insn>> = vec![Vec::new(); n];
    let mut after_pc: Vec<Vec<Insn>> = vec![Vec::new(); n];
    let mut replace_map: HashMap<usize, Insn> = HashMap::new();
    let mut after_block_fallthrough: Vec<Vec<Insn>> = vec![Vec::new(); num_blocks];
    let mut trampolines: HashMap<(usize, usize), Vec<Insn>> = HashMap::new();

    for (bit, plan) in plans {
        let expr = exprs[*bit];
        for &b in &plan.exit_blocks {
            let last_pc = cfg.blocks[b].1 - 1;
            if insn_ends_basic_block(&insns[last_pc]) {
                before_pc[last_pc].push(expr.to_insn(plan.temp_reg));
            } else {
                after_pc[last_pc].push(expr.to_insn(plan.temp_reg));
            }
        }
        for &p in &plan.fallthrough_edges {
            after_block_fallthrough[p].push(expr.to_insn(plan.temp_reg));
        }
        for &(p, b) in &plan.jump_edges {
            trampolines
                .entry((p, b))
                .or_default()
                .push(expr.to_insn(plan.temp_reg));
        }
        for &pc in &plan.replace_pcs {
            let (dst, _) = pc_expr[pc].expect("replacement pc must hold candidate expr");
            replace_map.insert(pc, Insn::Move(dst, plan.temp_reg));
        }
        for &pc in &plan.capture_pcs {
            let (dst, _) = pc_expr[pc].expect("capture pc must hold candidate expr");
            // Skip copying back if `dst` is already `plan.temp_reg` (cannot
            // happen for freshly allocated temps, guarded for clarity).
            if dst != plan.temp_reg {
                after_pc[pc].push(Insn::Move(plan.temp_reg, dst));
            }
        }
    }

    let mut out: Vec<Insn> = Vec::with_capacity(n + plans.len() * 4);
    let mut old_to_new = vec![0usize; n + 1];
    let mut normal_jumps: Vec<(usize, usize)> = Vec::new();
    let mut tramp_jumps: Vec<(usize, (usize, usize))> = Vec::new();

    for (b, &(b_start, b_end)) in cfg.blocks.iter().enumerate() {
        for pc in b_start..b_end {
            old_to_new[pc] = out.len();
            for extra in &before_pc[pc] {
                out.push(extra.clone());
            }
            let emitted = replace_map
                .get(&pc)
                .cloned()
                .unwrap_or_else(|| insns[pc].clone());
            let emitted_idx = out.len();
            if let Some(off) = insn_jump_off(&emitted) {
                let old_target = pre_jump_target(pc, off);
                if old_target < n
                    && pc == b_end - 1
                    && trampolines.contains_key(&(b, cfg.pc_to_block[old_target]))
                {
                    tramp_jumps.push((emitted_idx, (b, cfg.pc_to_block[old_target])));
                } else {
                    normal_jumps.push((emitted_idx, old_target));
                }
            }
            out.push(emitted);
            for extra in &after_pc[pc] {
                out.push(extra.clone());
            }
        }
        // Fallthrough-edge compensation copies sit AFTER `b`'s conditional
        // terminator and BEFORE the next block's `old_to_new[b_end]`, so only
        // fallthrough from `b` executes them.
        for extra in &after_block_fallthrough[b] {
            out.push(extra.clone());
        }
    }
    old_to_new[n] = out.len();

    // If any critical-edge trampolines exist, ensure normal control cannot fall
    // off the end of the original function into the trampolines.
    let mut tramp_pc_map: HashMap<(usize, usize), usize> = HashMap::new();
    if !trampolines.is_empty() {
        let last_terminates = matches!(
            out.last(),
            Some(
                Insn::Jump(_)
                    | Insn::Return(_)
                    | Insn::ReturnNone
                    | Insn::RaiseValue(_)
                    | Insn::RaiseExceptStarResidual(_)
                    | Insn::RaiseFrom(..)
                    | Insn::RaiseReRaise
                    | Insn::RaiseAssert(_)
            )
        );
        let guard_jump_idx = if !last_terminates {
            let idx = out.len();
            out.push(Insn::Jump(0));
            Some(idx)
        } else {
            None
        };

        // Emit trampolines in deterministic order sorted by `(p, b)`.
        let mut keys: Vec<(usize, usize)> = trampolines.keys().copied().collect();
        keys.sort_unstable();
        for key @ (_p, target_b) in keys {
            let tramp_start = out.len();
            tramp_pc_map.insert(key, tramp_start);
            for extra in &trampolines[&key] {
                out.push(extra.clone());
            }
            let jmp_idx = out.len();
            out.push(Insn::Jump(0));
            normal_jumps.push((jmp_idx, cfg.blocks[target_b].0));
        }

        let past_end = out.len();
        if let Some(guard_idx) = guard_jump_idx {
            set_insn_jump_off(
                &mut out[guard_idx],
                (past_end as i64 - guard_idx as i64 - 1) as i32,
            );
        }
        old_to_new[n] = past_end;
    }

    // Rewrite all jump offsets to their new targets.
    for (new_pc, old_target) in normal_jumps {
        let new_target = old_to_new[old_target.min(n)];
        let new_off = (new_target as i64 - new_pc as i64 - 1) as i32;
        set_insn_jump_off(&mut out[new_pc], new_off);
    }
    for (new_pc, key) in tramp_jumps {
        let new_target = tramp_pc_map[&key];
        let new_off = (new_target as i64 - new_pc as i64 - 1) as i32;
        set_insn_jump_off(&mut out[new_pc], new_off);
    }

    out
}

/// Partial Redundancy Elimination (PRE / Lazy Code Motion, issue #350).
///
/// Eliminates partially redundant pure primitive computations across branches
/// and loops by inserting compensation copies at the latest safe points and
/// reusing the computed value via `Move(dst, temp)`.
///
/// ## Soundness guarantees
/// 1. **Exact primitive type proof**: A forward type analysis proves exact
///    built-in primitive operand types (`Int`, `Float`, `Bool`, `Str`,
///    `NoneType`), starting with all registers `Unknown` at entry (covering
///    function parameters and uninitialized locals).  Only pure, exception-free
///    operations on proven exact primitives are admitted — user objects with
///    custom `__mul__`/`__add__`, division by unknown/zero divisors, and
///    potentially-raising ops are never hoisted or deduplicated.
/// 2. **Down-safety (Anticipatability)**: Expressions are only hoisted to
///    blocks/edges where EVERY forward path evaluates the expression before
///    any operand redefinition, named-local re-entrancy barrier, or exit.
/// 3. **Exception-region isolation**: Expressions never cross `SetupExcept` /
///    `PopExcept` / handler region boundaries, and potentially-raising
///    instructions inside a `try` region act as barriers to anticipation and
///    availability.
/// 4. **Forward MUST self-verification**: Before committing any candidate
///    expression, a forward MUST check verifies that the dedicated temp
///    register is initialized and fresh along every executable path reaching
///    each `Move(dst, temp)` replacement and that at least one original
///    computation is strictly eliminated.
fn pass_pre(
    insns: Vec<Insn>,
    descs: &[InsnDesc],
    consts: &[Value],
    num_locals: u32,
    num_regs: &mut u32,
) -> Vec<Insn> {
    debug_assert_no_late_stage_insns(&insns, "pass_pre");
    if insns.len() < 3 {
        return insns;
    }

    let Some(cfg) = pre_build_cfg(&insns) else {
        return insns;
    };
    let (type_in, type_out_before_term) =
        pre_analyze_types(&insns, descs, consts, num_locals, &cfg);
    let discovered = pre_discover_exprs(&insns, descs, consts, num_locals, &cfg, &type_in);
    if discovered.exprs.is_empty() {
        return insns;
    }

    let df = pre_compute_dataflow(
        &insns,
        descs,
        consts,
        num_locals,
        &cfg,
        &type_in,
        &type_out_before_term,
        &discovered,
    );
    let (plans, next_temp) = pre_plan_exprs(
        &insns,
        descs,
        consts,
        num_locals,
        *num_regs,
        &cfg,
        &type_in,
        &discovered.exprs,
        &discovered.pc_expr,
        &df,
    );
    if plans.is_empty() {
        return insns;
    }

    *num_regs = (*num_regs).max(next_temp);
    pre_materialize(&insns, &cfg, &discovered.exprs, &discovered.pc_expr, &plans)
}

/// Update the relative jump offset stored in a branching instruction.
fn set_insn_jump_off(insn: &mut Insn, new_off: i32) {
    use Insn::*;
    match insn {
        Jump(k)
        | JumpIfFalse(_, k)
        | JumpIfTrue(_, k)
        | CmpJumpIfFalse(_, _, _, k)
        | CmpJumpIfTrue(_, _, _, k)
        | CmpJumpIfFalseConst(_, _, _, k)
        | CmpJumpIfTrueConst(_, _, _, k)
        | ForIter(_, _, k)
        | SetupExcept(k)
        | MatchExcept(_, k)
        | MatchExceptStar(_, k) => *k = new_off,
        _ => {}
    }
}
