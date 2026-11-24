// ─── Int-loop candidate discovery, eligibility, and soundness checks ──────────

/// Detect the `LoadGlobal len` + `Move` + `Call` triple a `while … len(seq)`
/// header opens with, returning `(callee, seq, name_idx)`.
///
/// This only *proposes* that the call computes a built-in length: the name is
/// matched textually and the value it resolves to is checked at runtime by
/// `JumpIfNotBuiltinLen`.
fn detect_len_header(
    insns: &[Insn],
    names: &[pyrust_core::Symbol],
    num_locals: u32,
    h: usize,
) -> Option<(Reg, Reg, u16)> {
    let Insn::LoadGlobal(callee, name_idx) = insns[h] else {
        return None;
    };
    if names.get(usize::from(name_idx)).map(|s| s.as_str()) != Some("len") {
        return None;
    }
    // The call convention places the sole argument at `callee + 1`.  Both
    // registers must be call scratch: the copy never materialises the
    // argument slot and leaves `callee` holding the native length, which
    // would be visible through the namespace mirror if either aliased a
    // module fast-local.
    if callee < num_locals {
        return None;
    }
    let seq = match insns.get(h + 1)? {
        Insn::Move(dst, src) if *dst == callee + 1 => *src,
        _ => return None,
    };
    if seq == callee || seq == callee + 1 {
        return None;
    }
    match insns.get(h + 2)? {
        Insn::Call(base, 1) | Insn::CallMemo(base, 1) if *base == callee => {
            Some((callee, seq, name_idx))
        }
        _ => None,
    }
}

/// Scan `insns` for eligible innermost int-loop versioning candidates.
fn find_int_loop_candidates(
    insns: &[Insn],
    descs: &[InsnDesc],
    consts: &[Value],
    names: &[pyrust_core::Symbol],
    num_locals: u32,
    num_regs: &mut u32,
    reaching_const_reads: &HashMap<(usize, Reg), u16>,
) -> Vec<IntLoopCandidate> {
    let n = insns.len();
    let mut candidates: Vec<IntLoopCandidate> = Vec::new();
    let mut h = 0usize;

    while h < n {
        // Header: forward conditional jump (inverted while), ForIter
        // (for-range), or the `len(seq)` call triple that precedes a
        // never-inverted `while i < len(seq)` comparison.
        let (kind, hdr, k) = match &insns[h] {
            Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
                if *k > 1 =>
            {
                (IntLoopCandidateKind::Inverted, h, *k as usize)
            }
            Insn::ForIter(_, slot, k) if *k > 1 => (
                IntLoopCandidateKind::ForIter { slot: *slot },
                h,
                *k as usize,
            ),
            Insn::LoadGlobal(..) => {
                let hdr = h + INT_LOOP_LEN_HEADER_TRIPLE;
                // The comparison must read the length the triple produced;
                // otherwise the call's result is used for something the copy
                // does not model and eliding the call would be wrong.
                match (
                    detect_len_header(insns, names, num_locals, h),
                    insns.get(hdr),
                ) {
                    (
                        Some((callee, seq, name_idx)),
                        Some(Insn::CmpJumpIfFalse(a, _, b, k) | Insn::CmpJumpIfTrue(a, _, b, k)),
                    ) if *k > 1 && (*a == callee || *b == callee) => (
                        IntLoopCandidateKind::LenHeaderWhile {
                            callee,
                            seq,
                            name_idx,
                        },
                        hdr,
                        *k as usize,
                    ),
                    _ => {
                        h += 1;
                        continue;
                    }
                }
            }
            _ => {
                h += 1;
                continue;
            }
        };
        let back = hdr + k;
        if back >= n || back - h > INT_LOOP_MAX_REGION {
            h += 1;
            continue;
        }
        // Back-edge: an inverted while re-checks its condition at the body
        // end (the shape produced by `pass_loop_inversion`); a for-range
        // returns to its ForIter, and a `len` header to its `LoadGlobal`, with
        // an unconditional Jump.
        let back_targets_body = match (&kind, &insns[back]) {
            (
                IntLoopCandidateKind::Inverted,
                Insn::CmpJumpIfFalse(_, _, _, kb)
                | Insn::CmpJumpIfTrue(_, _, _, kb)
                | Insn::CmpJumpIfFalseConst(_, _, _, kb)
                | Insn::CmpJumpIfTrueConst(_, _, _, kb)
                | Insn::JumpIfFalse(_, kb)
                | Insn::JumpIfTrue(_, kb),
            ) => *kb == -(k as i32),
            (IntLoopCandidateKind::ForIter { .. }, Insn::Jump(kb)) => *kb == -(k as i32 + 1),
            (IntLoopCandidateKind::LenHeaderWhile { .. }, Insn::Jump(kb)) => {
                (back as i64 + 1 + i64::from(*kb)) == h as i64
            }
            _ => false,
        };
        if !back_targets_body {
            h += 1;
            continue;
        }

        // Eligibility walk.
        let mut guards: Vec<Reg> = Vec::new();
        let mut syncs: Vec<(Reg, u16)> = Vec::new();
        let mut has_interior_control_flow = false;
        let guard = |r: Reg, guards: &mut Vec<Reg>| {
            if !guards.contains(&r) {
                guards.push(r);
            }
        };
        let mut eligible = true;
        let len_seq = match kind {
            IntLoopCandidateKind::LenHeaderWhile { seq, .. } => Some(seq),
            IntLoopCandidateKind::Inverted | IntLoopCandidateKind::ForIter { .. } => None,
        };
        let mut len_header_is_non_mutating = true;
        // Old indices of `GetItem`s the fast copy may run as the deopting
        // `GetItemSeqIntOrExit`.
        let mut subscripts: Vec<usize> = Vec::new();
        let walk_start = match kind {
            // The ForIter header is not in the body whitelist; the runtime
            // iterator-slot guard owns its semantics.
            IntLoopCandidateKind::ForIter { .. } => h + 1,
            IntLoopCandidateKind::Inverted | IntLoopCandidateKind::LenHeaderWhile { .. } => hdr,
        };
        for i in walk_start..=back {
            let Some(policy) = classify_int_loop_region_insn(&insns[i]) else {
                eligible = false;
                break;
            };
            if let Some(seq) = len_seq
                && policy.len_hoist_safety(seq) != LenHoistSafety::ProvenNonMutating
            {
                len_header_is_non_mutating = false;
            }
            match policy {
                IntLoopRegionInsn::Copy { src, .. } => guard(src, &mut guards),
                IntLoopRegionInsn::LoadConst { idx, .. } => {
                    if !int_loop_const_is_int(consts, idx) {
                        eligible = false;
                        break;
                    }
                }
                IntLoopRegionInsn::Binary { lhs, op, rhs, .. } => {
                    if !int_loop_arith_op(&op) {
                        eligible = false;
                        break;
                    }
                    guard(lhs, &mut guards);
                    guard(rhs, &mut guards);
                }
                IntLoopRegionInsn::BinaryImm { src, op, imm, .. } => {
                    if !int_loop_imm_arith_op(&op, imm) {
                        eligible = false;
                        break;
                    }
                    guard(src, &mut guards);
                }
                IntLoopRegionInsn::BinaryConst { src, op, idx, .. } => {
                    if !int_loop_const_arith_op(consts, &op, idx)
                        || !int_loop_const_is_int(consts, idx)
                    {
                        eligible = false;
                        break;
                    }
                    guard(src, &mut guards);
                }
                IntLoopRegionInsn::Compare {
                    lhs,
                    op,
                    rhs,
                    offset,
                } => {
                    let continue_edge =
                        offset < 0 && (i as i64 + 1 + i64::from(offset)) == h as i64;
                    if !int_loop_cmp_op(&op)
                        || (i != hdr && i != back && offset < 0 && !continue_edge)
                    {
                        eligible = false;
                        break;
                    }
                    has_interior_control_flow |= i != hdr && i != back;
                    guard(lhs, &mut guards);
                    guard(rhs, &mut guards);
                }
                IntLoopRegionInsn::CompareConst {
                    lhs,
                    op,
                    idx,
                    offset,
                } => {
                    let continue_edge =
                        offset < 0 && (i as i64 + 1 + i64::from(offset)) == h as i64;
                    if !int_loop_cmp_op(&op)
                        || !int_loop_const_is_int(consts, idx)
                        || (i != hdr && i != back && offset < 0 && !continue_edge)
                    {
                        eligible = false;
                        break;
                    }
                    has_interior_control_flow |= i != hdr && i != back;
                    guard(lhs, &mut guards);
                }
                IntLoopRegionInsn::Truthiness { reg, offset } => {
                    let continue_edge =
                        offset < 0 && (i as i64 + 1 + i64::from(offset)) == h as i64;
                    if i != hdr && i != back && offset < 0 && !continue_edge {
                        eligible = false;
                        break;
                    }
                    has_interior_control_flow |= i != hdr && i != back;
                    guard(reg, &mut guards);
                }
                IntLoopRegionInsn::Jump { offset } => {
                    let is_forrange_backedge =
                        i == back && matches!(kind, IntLoopCandidateKind::ForIter { .. });
                    // A backward jump to the region head is a `continue`; any
                    // other backward jump is a nested loop's back-edge and only
                    // innermost regions are versioned.
                    let continue_edge =
                        offset < 0 && (i as i64 + 1 + i64::from(offset)) == h as i64;
                    if offset < 0 && !is_forrange_backedge && !continue_edge {
                        eligible = false;
                        break;
                    }
                    has_interior_control_flow |= i != hdr && i != back;
                }
                IntLoopRegionInsn::Subscript { dst, obj, idx } => {
                    // Admitted through a mid-loop side exit: the fast copy runs
                    // the deopting `GetItemSeqIntOrExit` and guards its result.
                    if dst == obj || dst == idx {
                        eligible = false;
                        break;
                    }
                    subscripts.push(i);
                }
                IntLoopRegionInsn::Sync { reg, name_idx } => {
                    if !syncs.contains(&(reg, name_idx)) {
                        syncs.push((reg, name_idx));
                    }
                }
            }
        }
        if let IntLoopCandidateKind::ForIter { .. } = kind
            && let Insn::ForIter(dst, _, _) = &insns[h]
        {
            guards.retain(|g| g != dst);
        }
        if let IntLoopCandidateKind::LenHeaderWhile { callee, .. } = kind {
            let touches = |i: usize, r: Reg| {
                insn_reads_reg(&insns[i], descs, r) || int_loop_writes_reg(&insns[i], r)
            };
            if (hdr + 1..=back).any(|i| touches(i, callee) || touches(i, callee + 1))
                || insn_reads_reg(&insns[hdr], descs, callee + 1)
                || syncs.iter().any(|&(r, _)| r == callee || r == callee + 1)
            {
                eligible = false;
            }
            guards.retain(|g| *g != callee);
        }
        for &si in &subscripts {
            let Insn::GetItem(dst, _, _) = &insns[si] else {
                unreachable!("only GetItem sites are recorded as subscripts");
            };
            if insns[h..si]
                .iter()
                .any(|insn| insn_reads_reg(insn, descs, *dst))
            {
                eligible = false;
                break;
            }
            guards.retain(|g| g != dst);
        }
        if has_interior_control_flow && !subscripts.is_empty() {
            eligible = false;
        }
        // Any body temporary defined before it is read via a whitelisted
        // int-family operation needs no entry guard.
        let defines_int_family = |insn: &Insn, g: Reg| {
            matches!(
                insn,
                Insn::LoadConst(dst, _)
                    | Insn::Move(dst, _)
                    | Insn::BinOp(dst, ..)
                    | Insn::BinOpInPlace(dst, ..)
                    | Insn::BinOpImm(dst, ..)
                    | Insn::BinOpConst(dst, ..)
                if *dst == g
            )
        };
        let entry_state_is_dead = |g: Reg| {
            for d in walk_start..=back {
                if insn_reads_reg(&insns[d], descs, g) {
                    return false;
                }
                if defines_int_family(&insns[d], g) {
                    return (walk_start..d)
                        .all(|b| int_loop_target(b, &insns[b]).is_none_or(|t| t <= d || t > back));
                }
            }
            false
        };
        guards.retain(|g| !entry_state_is_dead(*g));

        // Sync-deferral soundness checks: preserve insertion order and ensure
        // every deferred sync source still holds its published value at exit.
        let function_deletes_bindings = || {
            insns.iter().any(|insn| {
                matches!(
                    insn,
                    Insn::DeleteModuleGlobal(_)
                        | Insn::DeleteName(_)
                        | Insn::DeleteCell(..)
                        | Insn::DeleteLocal(..)
                )
            })
        };
        let syncs_pre_bound = || {
            syncs.iter().all(|&(r, name_idx)| {
                insns[..h]
                    .iter()
                    .any(|insn| matches!(insn, Insn::SyncModuleGlobal(r2, n2) if *r2 == r && *n2 == name_idx))
            })
        };
        let write_reaches_its_sync = |r: Reg, k: usize| {
            for s in k + 1..=back {
                if matches!(insns[s], Insn::SyncModuleGlobal(r2, _) if r2 == r) {
                    return true;
                }
                if int_loop_writes_reg(&insns[s], r) || int_loop_target(s, &insns[s]).is_some() {
                    return false;
                }
            }
            false
        };
        let sync_sources_republish_exactly = || {
            syncs.iter().all(|&(r, name_idx)| {
                (hdr..=back).all(|s| {
                    !matches!(insns[s], Insn::SyncModuleGlobal(r2, n2) if r2 == r && n2 != name_idx)
                }) && (hdr..=back)
                    .all(|k| !int_loop_writes_reg(&insns[k], r) || write_reaches_its_sync(r, k))
            })
        };
        if !eligible
            || !len_header_is_non_mutating
            || guards.len() > INT_LOOP_MAX_GUARDS
            || (!syncs.is_empty()
                && (!sync_sources_republish_exactly()
                    || (has_interior_control_flow
                        && (function_deletes_bindings() || !syncs_pre_bound()))))
        {
            h += 1;
            continue;
        }

        // Fusion opportunity: the last two non-sync insns forming a
        // BinOpImm(v,v,Add,imm) + CmpJump(v, …) back-edge pair.
        let mut prev_non_sync = None;
        for i in (h..back).rev() {
            if !matches!(insns[i], Insn::SyncModuleGlobal(..)) {
                prev_non_sync = Some(i);
                break;
            }
        }
        let has_fusion_interior_landing = prev_non_sync.is_some_and(|p| {
            insns[h..=back]
                .iter()
                .enumerate()
                .filter_map(|(rel, region_insn)| int_loop_target(h + rel, region_insn))
                .any(|t| t > p && t <= back)
        });
        let has_reg_fusion = prev_non_sync.is_some_and(|p| {
            matches!(
                (&insns[p], &insns[back]),
                (
                    Insn::BinOpImm(d, s, BinaryOp::Add, _, _),
                    Insn::CmpJumpIfTrue(a, _, _, _) | Insn::CmpJumpIfFalse(a, _, _, _),
                ) if d == s && a == d
            )
        }) && !has_fusion_interior_landing;
        let const_fuse = if !has_reg_fusion
            && let Some(p) = prev_non_sync
            && let Insn::BinOpImm(d, s, BinaryOp::Add, _, _) = &insns[p]
            && d == s
            && let Insn::CmpJumpIfTrueConst(a, _, cidx, _)
            | Insn::CmpJumpIfFalseConst(a, _, cidx, _) = &insns[back]
            && a == d
            && !has_fusion_interior_landing
            && *num_regs < MAX_FRAME_REGS
        {
            Some((*num_regs as Reg, *cidx))
        } else {
            None
        };
        let len_latch = if let IntLoopCandidateKind::LenHeaderWhile { callee, seq, .. } = kind
            && let Some(add) = prev_non_sync
            && let Insn::BinOpImm(dst, src, BinaryOp::Add, imm, _) = insns[add]
            && dst == src
            && dst != seq
            && let Insn::CmpJumpIfFalse(a, op, b, _) | Insn::CmpJumpIfTrue(a, op, b, _) = insns[hdr]
            && a == dst
            && b == callee
            && !has_fusion_interior_landing
        {
            Some(IntLoopLenLatch {
                add,
                var: dst,
                op,
                imm,
                jump_when_true: matches!(insns[hdr], Insn::CmpJumpIfFalse(..)),
            })
        } else {
            None
        };
        let has_fusion = has_reg_fusion || const_fuse.is_some() || len_latch.is_some();
        if syncs.is_empty() && !has_fusion {
            h += 1;
            continue;
        }
        // External jumps into the region interior disqualify it.
        let mut externally_entered = false;
        for (i, insn) in insns.iter().enumerate() {
            if (h..=back).contains(&i) {
                continue;
            }
            if let Some(t) = int_loop_target(i, insn)
                && t > h
                && t <= back
            {
                externally_entered = true;
                break;
            }
        }
        if externally_entered {
            h += 1;
            continue;
        }

        if const_fuse.is_some() {
            *num_regs += 1;
        }
        let mut variants = match kind {
            IntLoopCandidateKind::Inverted | IntLoopCandidateKind::LenHeaderWhile { .. } => {
                vec![IntLoopFastVariant::Plain]
            }
            IntLoopCandidateKind::ForIter { .. } if has_interior_control_flow => {
                vec![IntLoopFastVariant::IntRange]
            }
            IntLoopCandidateKind::ForIter { .. } => {
                vec![IntLoopFastVariant::IntRange, IntLoopFastVariant::IndexedSeq]
            }
        };
        let closed_form = match kind {
            IntLoopCandidateKind::ForIter { slot } => {
                closed_form_for(insns, consts, names, reaching_const_reads, h, back, slot)
            }
            IntLoopCandidateKind::Inverted | IntLoopCandidateKind::LenHeaderWhile { .. } => None,
        };
        if closed_form.is_some() {
            variants.insert(0, IntLoopFastVariant::ClosedForm);
        }
        candidates.push(IntLoopCandidate {
            kind,
            head: h,
            back,
            guards,
            syncs,
            const_fuse,
            variants,
            closed_form,
            len_latch,
        });
        h = back + 1;
    }

    candidates
}
