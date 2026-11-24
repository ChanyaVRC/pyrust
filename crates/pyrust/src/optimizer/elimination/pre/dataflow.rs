// ─── PRE CFG construction, type analysis, and dataflow solvers ────────────────

#[inline]
fn pre_jump_target(pc: usize, off: i32) -> usize {
    (pc as i64 + 1 + i64::from(off)) as usize
}

/// Steps 0 & 1: Derive exception-handler regions and build basic-block CFG.
fn pre_build_cfg(insns: &[Insn]) -> Option<PreCfg> {
    let n = insns.len();
    let has_except = insns.iter().any(|i| matches!(i, Insn::SetupExcept(_)));
    let (region_at, handler_pc_at): (Vec<u32>, Vec<usize>) = if has_except {
        let stack_in = analyze_active_handler_stacks(insns)?;
        let mut region_map: HashMap<Vec<usize>, u32> = HashMap::new();
        region_map.insert(Vec::new(), 0);
        let mut regions = vec![0u32; n];
        let mut handlers = vec![usize::MAX; n];
        for (pc, stack) in stack_in.into_iter().enumerate() {
            if let Some(s) = stack {
                if let Some(&h) = s.last() {
                    handlers[pc] = h;
                }
                let next_id = region_map.len() as u32;
                let id = *region_map.entry(s).or_insert(next_id);
                regions[pc] = id;
            }
        }
        (regions, handlers)
    } else {
        (vec![0u32; n], Vec::new())
    };

    let mut is_leader_buf = InlineBoolBuf::new(n, false);
    let is_leader = &mut is_leader_buf.as_mut_slice()[..n];
    is_leader[0] = true;
    for (pc, insn) in insns.iter().enumerate() {
        if let Some(off) = insn_jump_off(insn) {
            let target = pc as i64 + 1 + i64::from(off);
            if target >= 0 && (target as usize) < n {
                is_leader[target as usize] = true;
            }
        }
        if insn_ends_basic_block(insn) && pc + 1 < n {
            is_leader[pc + 1] = true;
        }
    }

    let mut blocks: Vec<(usize, usize)> = Vec::new();
    let mut pc_to_block = vec![0usize; n];
    let mut start = 0usize;
    for pc in 0..n {
        if pc > 0 && is_leader[pc] {
            blocks.push((start, pc));
            start = pc;
        }
        pc_to_block[pc] = blocks.len();
    }
    blocks.push((start, n));
    let num_blocks = blocks.len();

    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); num_blocks];
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); num_blocks];
    let mut is_exit = vec![false; num_blocks];
    let mut block_region = vec![0u32; num_blocks];

    for (b, &(b_start, b_end)) in blocks.iter().enumerate() {
        block_region[b] = region_at[b_start];
        let last_pc = b_end - 1;
        let mut add_succ = |target_pc: usize| {
            if target_pc < n {
                let tb = pc_to_block[target_pc];
                if !succs[b].contains(&tb) {
                    succs[b].push(tb);
                    preds[tb].push(b);
                }
            } else {
                is_exit[b] = true;
            }
        };
        match &insns[last_pc] {
            Insn::Jump(k) => add_succ(pre_jump_target(last_pc, *k)),
            Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, k) => {
                add_succ(last_pc + 1);
                add_succ(pre_jump_target(last_pc, *k));
            }
            Insn::Return(_)
            | Insn::ReturnNone
            | Insn::RaiseValue(_)
            | Insn::RaiseExceptStarResidual(_)
            | Insn::RaiseFrom(..)
            | Insn::RaiseReRaise
            | Insn::RaiseAssert(_) => {
                is_exit[b] = true;
            }
            _ => add_succ(b_end),
        }
    }

    Some(PreCfg {
        has_except,
        handler_pc_at,
        blocks,
        pc_to_block,
        succs,
        preds,
        is_exit,
        block_region,
    })
}

/// Step 2: Forward exact-type analysis (worklist fixpoint).
fn pre_analyze_types(
    insns: &[Insn],
    descs: &[InsnDesc],
    consts: &[Value],
    num_locals: u32,
    cfg: &PreCfg,
) -> (Vec<Option<PreTypeMap>>, Vec<Option<PreTypeMap>>) {
    let n = insns.len();
    let num_blocks = cfg.num_blocks();
    let mut type_in: Vec<Option<PreTypeMap>> = vec![None; num_blocks];
    let mut type_out_before_term: Vec<Option<PreTypeMap>> = vec![None; num_blocks];
    let mut in_wl = vec![false; num_blocks];
    let mut wl = std::collections::VecDeque::new();

    type_in[0] = Some(HashMap::new());
    in_wl[0] = true;
    wl.push_back(0);

    let propagate_types = |target_b: usize,
                           incoming: &PreTypeMap,
                           type_in: &mut [Option<PreTypeMap>],
                           in_wl: &mut [bool],
                           wl: &mut std::collections::VecDeque<usize>| {
        match &mut type_in[target_b] {
            None => {
                type_in[target_b] = Some(incoming.clone());
                if !in_wl[target_b] {
                    in_wl[target_b] = true;
                    wl.push_back(target_b);
                }
            }
            Some(existing) => {
                if !existing.is_empty() {
                    let before = existing.len();
                    existing.retain(|r, t| incoming.get(r) == Some(&*t));
                    if existing.len() < before && !in_wl[target_b] {
                        in_wl[target_b] = true;
                        wl.push_back(target_b);
                    }
                }
            }
        }
    };

    while let Some(b) = wl.pop_front() {
        in_wl[b] = false;
        let Some(mut state) = type_in[b].clone() else {
            continue;
        };
        let (b_start, b_end) = cfg.blocks[b];
        let has_term = insn_ends_basic_block(&insns[b_end - 1]);
        let body_end = if has_term { b_end - 1 } else { b_end };

        for pc in b_start..body_end {
            let insn = &insns[pc];
            let pre_state = if cfg.has_except && cfg.handler_pc_at[pc] < n {
                Some(state.clone())
            } else {
                None
            };
            let (pure, may_raise) = pre_step_types(insn, descs, &mut state, consts, num_locals);
            if may_raise && let Some(mut exc_state) = pre_state {
                let hb = cfg.pc_to_block[cfg.handler_pc_at[pc]];
                if !pure {
                    exc_state.retain(|&r, _| r >= num_locals);
                }
                for_each_write(insn, descs, |w| {
                    exc_state.remove(&w);
                });
                propagate_types(hb, &exc_state, &mut type_in, &mut in_wl, &mut wl);
            }
        }

        type_out_before_term[b] = Some(state.clone());

        if has_term {
            let last_pc = b_end - 1;
            let term = &insns[last_pc];
            let pre_state = if cfg.has_except && cfg.handler_pc_at[last_pc] < n {
                Some(state.clone())
            } else {
                None
            };
            let (pure, may_raise) = pre_step_types(term, descs, &mut state, consts, num_locals);
            if may_raise && let Some(mut exc_state) = pre_state {
                let hb = cfg.pc_to_block[cfg.handler_pc_at[last_pc]];
                if !pure {
                    exc_state.retain(|&r, _| r >= num_locals);
                }
                for_each_write(term, descs, |w| {
                    exc_state.remove(&w);
                });
                propagate_types(hb, &exc_state, &mut type_in, &mut in_wl, &mut wl);
            }
            match term {
                Insn::ForIter(dst, _, k) => {
                    // Exit edge does not write `dst`; fallthrough edge writes `dst` = Unknown.
                    let exit_pc = pre_jump_target(last_pc, *k);
                    if exit_pc < n {
                        propagate_types(
                            cfg.pc_to_block[exit_pc],
                            &state,
                            &mut type_in,
                            &mut in_wl,
                            &mut wl,
                        );
                    }
                    state.remove(dst);
                    if last_pc + 1 < n {
                        propagate_types(
                            cfg.pc_to_block[last_pc + 1],
                            &state,
                            &mut type_in,
                            &mut in_wl,
                            &mut wl,
                        );
                    }
                }
                _ => {
                    for &s in &cfg.succs[b] {
                        propagate_types(s, &state, &mut type_in, &mut in_wl, &mut wl);
                    }
                }
            }
        } else {
            for &s in &cfg.succs[b] {
                propagate_types(s, &state, &mut type_in, &mut in_wl, &mut wl);
            }
        }
    }

    (type_in, type_out_before_term)
}

/// Step 3: Discover candidate pure expressions and filter to those that can
/// possibly be redundant (appear >= 2 times, or appear in a function with loops).
fn pre_discover_exprs(
    insns: &[Insn],
    descs: &[InsnDesc],
    consts: &[Value],
    num_locals: u32,
    cfg: &PreCfg,
    type_in: &[Option<PreTypeMap>],
) -> PreDiscoveredExprs {
    const MAX_PRE_EXPRS: usize = 64;
    let mut expr_occurrences: HashMap<PreExpr, Vec<(usize, usize, u32)>> = HashMap::new();
    let mut pc_expr: Vec<Option<(u32, PreExpr)>> = vec![None; insns.len()];

    let has_back_edge_in_fn = (0..cfg.num_blocks()).any(|b| cfg.succs[b].iter().any(|&s| s <= b));

    for (b, &(b_start, b_end)) in cfg.blocks.iter().enumerate() {
        let Some(mut state) = type_in[b].clone() else {
            continue;
        };
        for pc in b_start..b_end {
            let insn = &insns[pc];
            if let Some((dst, expr)) = pre_extract_expr(insn, &state, consts) {
                pc_expr[pc] = Some((dst, expr));
                expr_occurrences.entry(expr).or_default().push((b, pc, dst));
            }
            pre_step_types(insn, descs, &mut state, consts, num_locals);
        }
    }

    let mut exprs: Vec<PreExpr> = Vec::new();
    let mut expr_to_bit: HashMap<PreExpr, usize> = HashMap::new();
    for (expr, occs) in &expr_occurrences {
        if occs.len() < 2 && !has_back_edge_in_fn {
            continue;
        }
        if exprs.len() >= MAX_PRE_EXPRS {
            break;
        }
        let bit = exprs.len();
        expr_to_bit.insert(*expr, bit);
        exprs.push(*expr);
    }

    PreDiscoveredExprs {
        exprs,
        expr_to_bit,
        pc_expr,
    }
}

/// Steps 4, 5, 6: Compute per-block local bitmasks, anticipated (down-safe),
/// real availability (forward MUST), and partial availability (forward MAY).
#[allow(clippy::too_many_arguments)]
fn pre_compute_dataflow(
    insns: &[Insn],
    descs: &[InsnDesc],
    consts: &[Value],
    num_locals: u32,
    cfg: &PreCfg,
    type_in: &[Option<PreTypeMap>],
    type_out_before_term: &[Option<PreTypeMap>],
    discovered: &PreDiscoveredExprs,
) -> PreDataflow {
    let exprs = &discovered.exprs;
    let expr_to_bit = &discovered.expr_to_bit;
    let pc_expr = &discovered.pc_expr;
    let num_blocks = cfg.num_blocks();
    let num_exprs = exprs.len();
    let all_mask: u64 = if num_exprs == 64 {
        u64::MAX
    } else {
        (1u64 << num_exprs) - 1
    };

    let mut named_local_expr_mask = 0u64;
    for (bit, &expr) in exprs.iter().enumerate() {
        if expr.reads_named_local(num_locals) {
            named_local_expr_mask |= 1u64 << bit;
        }
    }

    let mut gen_ant = vec![0u64; num_blocks];
    let mut comp = vec![0u64; num_blocks];
    let mut transp_body = vec![all_mask; num_blocks];
    let mut term_kills = vec![0u64; num_blocks];
    let mut type_safe_in = vec![0u64; num_blocks];
    let mut type_safe_out = vec![0u64; num_blocks];

    let mask_for_write = |w: u32| -> u64 {
        let mut m = 0u64;
        for (bit, &expr) in exprs.iter().enumerate() {
            if expr.reads_reg(w) {
                m |= 1u64 << bit;
            }
        }
        m
    };

    for (b, &(b_start, b_end)) in cfg.blocks.iter().enumerate() {
        let Some(mut state) = type_in[b].clone() else {
            transp_body[b] = 0;
            continue;
        };
        let in_try = cfg.block_region[b] != 0;
        for (bit, &expr) in exprs.iter().enumerate() {
            if pre_expr_is_safe(expr, &state, consts) {
                type_safe_in[b] |= 1u64 << bit;
            }
        }
        if let Some(out_state) = &type_out_before_term[b] {
            for (bit, &expr) in exprs.iter().enumerate() {
                if pre_expr_is_safe(expr, out_state, consts) {
                    type_safe_out[b] |= 1u64 << bit;
                }
            }
        }

        let has_term = insn_ends_basic_block(&insns[b_end - 1]);
        let body_end = if has_term { b_end - 1 } else { b_end };

        let mut killed_so_far = 0u64;
        let mut comp_cur = 0u64;
        for pc in b_start..body_end {
            let insn = &insns[pc];
            let expr_bit = pc_expr[pc].and_then(|(_, e)| expr_to_bit.get(&e).copied());

            if let Some(bit) = expr_bit {
                let bit_mask = 1u64 << bit;
                if (killed_so_far & bit_mask) == 0 && (type_safe_in[b] & bit_mask) != 0 {
                    gen_ant[b] |= bit_mask;
                }
            }

            let (pure, may_raise) = pre_step_types(insn, descs, &mut state, consts, num_locals);
            let mut insn_kill = 0u64;
            if in_try && may_raise {
                insn_kill = all_mask;
            } else {
                if !pure {
                    insn_kill |= named_local_expr_mask;
                }
                for_each_write(insn, descs, |w| {
                    insn_kill |= mask_for_write(w);
                });
            }

            // Record availability of `expr_bit` if this instruction's own write
            // did not clobber one of its operands (e.g. `a = a + 1`).
            if let Some(bit) = expr_bit {
                let bit_mask = 1u64 << bit;
                if (insn_kill & bit_mask) == 0 {
                    comp_cur |= bit_mask;
                }
            }
            comp_cur &= !insn_kill;
            killed_so_far |= insn_kill;
        }
        transp_body[b] = all_mask & !killed_so_far;

        if has_term {
            let last_pc = b_end - 1;
            let term = &insns[last_pc];
            let (pure, may_raise) = pre_step_types(term, descs, &mut state, consts, num_locals);
            let mut tk = 0u64;
            if in_try && may_raise {
                tk = all_mask;
            } else {
                if !pure {
                    tk |= named_local_expr_mask;
                }
                for_each_write(term, descs, |w| {
                    tk |= mask_for_write(w);
                });
            }
            term_kills[b] = tk;
        }
        comp[b] = comp_cur & !term_kills[b] & type_safe_out[b];
    }

    // Step 5: Anticipated (down-safe, backward MUST).
    let mut ant_in = vec![0u64; num_blocks];
    let mut ant_out = vec![0u64; num_blocks];
    for b in 0..num_blocks {
        if type_in[b].is_some() && !cfg.is_exit[b] {
            ant_in[b] = type_safe_in[b];
            ant_out[b] = type_safe_out[b];
        } else if type_in[b].is_some() {
            ant_in[b] = gen_ant[b];
        }
    }
    loop {
        let mut changed = false;
        for b in (0..num_blocks).rev() {
            if type_in[b].is_none() {
                continue;
            }
            let new_out = if cfg.is_exit[b] || cfg.succs[b].is_empty() {
                0
            } else {
                let mut acc = all_mask;
                for &s in &cfg.succs[b] {
                    if cfg.safe_edge(b, s) && type_in[s].is_some() {
                        acc &= ant_in[s];
                    } else {
                        acc = 0;
                        break;
                    }
                }
                acc & !term_kills[b] & type_safe_out[b]
            };
            let new_in = (gen_ant[b] | (new_out & transp_body[b])) & type_safe_in[b];
            if new_out != ant_out[b] || new_in != ant_in[b] {
                ant_out[b] = new_out;
                ant_in[b] = new_in;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Step 6: Real availability (forward MUST) and partial availability
    // (forward MAY) BEFORE any PRE insertions.
    let mut real_av_in = vec![0u64; num_blocks];
    let mut real_av_out = vec![0u64; num_blocks];
    for b in 1..num_blocks {
        if type_in[b].is_some() {
            real_av_in[b] = all_mask;
            real_av_out[b] = all_mask;
        }
    }
    real_av_out[0] = comp[0];
    loop {
        let mut changed = false;
        for b in 0..num_blocks {
            if type_in[b].is_none() {
                continue;
            }
            let new_in = if b == 0 {
                0
            } else {
                let mut acc = all_mask;
                let mut any_pred = false;
                for &p in &cfg.preds[b] {
                    if type_in[p].is_none() {
                        continue;
                    }
                    any_pred = true;
                    if cfg.safe_edge(p, b) {
                        acc &= real_av_out[p];
                    } else {
                        acc = 0;
                        break;
                    }
                }
                if any_pred { acc } else { 0 }
            };
            let new_out = (comp[b] | (new_in & transp_body[b])) & !term_kills[b] & type_safe_out[b];
            if new_in != real_av_in[b] || new_out != real_av_out[b] {
                real_av_in[b] = new_in;
                real_av_out[b] = new_out;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut pav_in = vec![0u64; num_blocks];
    let mut pav_out = vec![0u64; num_blocks];
    loop {
        let mut changed = false;
        for b in 0..num_blocks {
            if type_in[b].is_none() {
                continue;
            }
            let new_in = if b == 0 {
                0
            } else {
                let mut acc = 0u64;
                for &p in &cfg.preds[b] {
                    if type_in[p].is_some() && cfg.safe_edge(p, b) {
                        acc |= pav_out[p];
                    }
                }
                acc
            };
            let new_out = (comp[b] | (new_in & transp_body[b])) & !term_kills[b] & type_safe_out[b];
            if new_in != pav_in[b] || new_out != pav_out[b] {
                pav_in[b] = new_in;
                pav_out[b] = new_out;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    PreDataflow {
        comp,
        transp_body,
        term_kills,
        type_safe_out,
        ant_in,
        ant_out,
        real_av_out,
        pav_in,
    }
}
