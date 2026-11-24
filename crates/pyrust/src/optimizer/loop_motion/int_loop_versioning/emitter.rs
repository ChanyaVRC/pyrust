// ─── Int-loop guard emission, fast-copy emission, and pass driver ─────────────

/// Materialise closed-form constants once candidate discovery has released the
/// constant pool.  A pool that cannot address another entry simply drops the
/// closed-form variant while retaining its per-iteration copies.
fn materialize_closed_form_consts(candidates: &mut [IntLoopCandidate], consts: &mut Vec<Value>) {
    for cand in candidates {
        let Some(closed_form) = &mut cand.closed_form else {
            continue;
        };
        let values = std::iter::once(closed_form.var_final)
            .chain(closed_form.acc_deltas.iter().map(|&(_, delta)| delta));
        closed_form.const_slots = values
            .map(|value| {
                let slot = u16::try_from(consts.len()).ok()?;
                consts.push(Value::int(value));
                Some(slot)
            })
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default();
        if closed_form.const_slots.is_empty() {
            cand.closed_form = None;
            cand.variants
                .retain(|v| *v != IntLoopFastVariant::ClosedForm);
        }
    }
}

/// Emit the rebuilt main stream with per-candidate entry guard blocks and a
/// trailing barrier jump ahead of the out-of-line fast copies.
struct IntLoopMainLayout {
    out: Vec<Insn>,
    placement: Vec<usize>,
    jump_target: Vec<usize>,
    past_end_patches: Vec<usize>,
    guard_jump_patches: Vec<Vec<usize>>,
    source_prefix_len: usize,
}

fn emit_int_loop_guards_and_main(
    insns: &[Insn],
    descs: &mut Vec<InsnDesc>,
    candidates: &[IntLoopCandidate],
) -> IntLoopMainLayout {
    let n = insns.len();
    let mut placement = vec![0usize; n + 1];
    let mut jump_target = vec![0usize; n + 1];
    {
        let mut shift = 0usize;
        let mut ci = 0usize;
        for i in 0..=n {
            let mut guard_start = None;
            if ci < candidates.len() && candidates[ci].head == i {
                guard_start = Some(i + shift);
                shift += candidates[ci].guard_block_len();
                ci += 1;
            }
            placement[i] = i + shift;
            // Jumps to a versioned head divert through its guard block; the
            // guard-fail edges below jump to `placement[head]` directly.
            jump_target[i] = guard_start.unwrap_or(i + shift);
        }
    }

    let main_len = placement[n];
    let mut out: Vec<Insn> = Vec::with_capacity(main_len + 32);
    let mut past_end_patches: Vec<usize> = Vec::new();
    let mut guard_jump_patches: Vec<Vec<usize>> = Vec::new();
    {
        let mut ci = 0usize;
        for (i, insn) in insns.iter().enumerate() {
            if ci < candidates.len() && candidates[ci].head == i {
                let cand = &candidates[ci];
                let per_variant = cand.guard_block_len() / cand.variants.len();
                let mut patches = Vec::with_capacity(cand.variants.len());
                for (vi, variant) in cand.variants.iter().enumerate() {
                    let block_end = out.len() + per_variant;
                    if let IntLoopCandidateKind::ForIter { slot } = cand.kind {
                        let fail = if vi + 1 < cand.variants.len() {
                            block_end
                        } else {
                            placement[i]
                        };
                        let fail_off = (fail as i64 - out.len() as i64 - 1) as i32;
                        out.push(match variant {
                            IntLoopFastVariant::IntRange => {
                                Insn::JumpIfIterNotIntRange(slot, fail_off)
                            }
                            IntLoopFastVariant::IndexedSeq => {
                                Insn::JumpIfIterNotIndexedSeq(slot, fail_off)
                            }
                            IntLoopFastVariant::ClosedForm => {
                                let closed_form = cand
                                    .closed_form
                                    .as_ref()
                                    .expect("a ClosedForm variant carries its plan");
                                let desc_idx = push_desc(
                                    descs,
                                    InsnDesc::IntRangeExactGuard(closed_form.guard.clone()),
                                );
                                Insn::JumpIfIterNotIntRangeExact(desc_idx, fail_off)
                            }
                            IntLoopFastVariant::Plain => {
                                unreachable!("a for header always guards its iterator kind")
                            }
                        });
                    }
                    for g in &cand.guards {
                        let gpos = out.len();
                        let fail_off = placement[i] as i64 - gpos as i64 - 1;
                        out.push(Insn::JumpIfNotInt(*g, fail_off as i32));
                    }
                    if let IntLoopCandidateKind::LenHeaderWhile {
                        callee,
                        seq,
                        name_idx,
                    } = cand.kind
                    {
                        out.push(Insn::LoadGlobal(callee, name_idx));
                        let gpos = out.len();
                        let fail_off = placement[i] as i64 - gpos as i64 - 1;
                        out.push(Insn::JumpIfNotBuiltinLen(callee, fail_off as i32));
                        let gpos = out.len();
                        let fail_off = placement[i] as i64 - gpos as i64 - 1;
                        out.push(Insn::LenSeqOrExit(callee, seq, fail_off as i32));
                    }
                    if let Some((tmp, cidx)) = cand.const_fuse {
                        out.push(Insn::LoadConst(tmp, cidx));
                    }
                    patches.push(out.len());
                    out.push(Insn::Jump(0)); // → fast head, patched below
                    debug_assert_eq!(out.len(), block_end);
                }
                guard_jump_patches.push(patches);
                ci += 1;
            }
            let remapped = rewrite_offsets_with(insn.clone(), i, &placement, &jump_target);
            if int_loop_target(i, insn) == Some(n) {
                past_end_patches.push(out.len());
            }
            out.push(remapped);
        }
    }
    debug_assert_eq!(out.len(), main_len);

    // Barrier jump past all appended fast copies so normal fallthrough cannot
    // enter the first out-of-line copy.
    past_end_patches.push(out.len());
    out.push(Insn::Jump(0));
    let source_prefix_len = out.len();

    IntLoopMainLayout {
        out,
        placement,
        jump_target,
        past_end_patches,
        guard_jump_patches,
        source_prefix_len,
    }
}

/// Append the specialized out-of-line loop copies and deferred-sync stubs, then
/// patch all past-the-end edges to the final bytecode length.
fn emit_int_loop_fast_copies(
    insns: &[Insn],
    candidates: &[IntLoopCandidate],
    mut layout: IntLoopMainLayout,
) -> IntLoopVersioningResult {
    let n = insns.len();
    let variant_copies: Vec<(usize, usize, IntLoopFastVariant)> = candidates
        .iter()
        .enumerate()
        .flat_map(|(ci, cand)| {
            cand.variants
                .iter()
                .enumerate()
                .map(move |(vi, variant)| (ci, vi, *variant))
        })
        .collect();

    for (ci, vi, variant) in variant_copies {
        let cand = &candidates[ci];
        let fast_base = layout.out.len();
        let jpos = layout.guard_jump_patches[ci][vi];
        layout.out[jpos] = Insn::Jump((fast_base as i64 - jpos as i64 - 1) as i32);

        let mut fast_index = vec![usize::MAX; cand.back - cand.head + 1];
        let mut fast: Vec<IntLoopFastEntry> = Vec::new();
        if variant == IntLoopFastVariant::ClosedForm {
            let closed_form = cand
                .closed_form
                .as_ref()
                .expect("a ClosedForm variant carries its plan");
            let (&var_slot, delta_slots) = closed_form
                .const_slots
                .split_first()
                .expect("a retained closed form allocated its constant slots");
            fast_index[0] = 0;
            fast.push(IntLoopFastEntry::Copied(
                cand.head,
                Insn::LoadConst(closed_form.var, var_slot),
            ));
            for (&(acc, _), &delta_slot) in closed_form.acc_deltas.iter().zip(delta_slots) {
                fast.push(IntLoopFastEntry::Copied(
                    cand.head,
                    Insn::BinOpConst(acc, acc, BinaryOp::Add, delta_slot, true),
                ));
            }
        } else {
            let mut pending: Vec<(Reg, usize)> = Vec::new();
            let mut i = cand.head;
            while i <= cand.back {
                let entry_pos = fast.len();
                for (reg, target) in pending.drain(..) {
                    fast.push(IntLoopFastEntry::SideExit(
                        target,
                        Insn::JumpIfNotInt(reg, 0),
                    ));
                }
                fast_index[i - cand.head] = entry_pos;
                match &insns[i] {
                    _ if i < cand.header() => {}
                    Insn::BinOpImm(..)
                        if cand.len_latch.as_ref().is_some_and(|latch| latch.add == i) =>
                    {
                        let latch = cand
                            .len_latch
                            .as_ref()
                            .expect("the arm guard proved the latch is present");
                        let IntLoopCandidateKind::LenHeaderWhile { callee, .. } = cand.kind else {
                            unreachable!("only a len header carries this latch");
                        };
                        let to_body_top = (cand.header() as i64 - cand.back as i64) as i32;
                        fast_index[cand.back - cand.head] = fast.len();
                        fast.push(IntLoopFastEntry::Copied(
                            cand.back,
                            if latch.jump_when_true {
                                Insn::CountCmpJumpTrue(
                                    latch.var,
                                    latch.op,
                                    callee,
                                    latch.imm,
                                    to_body_top,
                                )
                            } else {
                                Insn::CountCmpJumpFalse(
                                    latch.var,
                                    latch.op,
                                    callee,
                                    latch.imm,
                                    to_body_top,
                                )
                            },
                        ));
                        i = cand.back + 1;
                        continue;
                    }
                    Insn::SyncModuleGlobal(..) => {}
                    Insn::ForIter(dst, _, _)
                        if i == cand.head && variant == IntLoopFastVariant::IndexedSeq =>
                    {
                        fast.push(IntLoopFastEntry::Copied(i, insns[i].clone()));
                        pending.push((*dst, cand.head + 1));
                    }
                    Insn::GetItem(dst, obj, idx) => {
                        fast.push(IntLoopFastEntry::SideExit(
                            i,
                            Insn::GetItemSeqIntOrExit(*dst, *obj, *idx, 0),
                        ));
                    }
                    Insn::BinOpImm(d, s, BinaryOp::Add, imm, _) if d == s && i < cand.back => {
                        let mut j = i + 1;
                        while j < cand.back && matches!(insns[j], Insn::SyncModuleGlobal(..)) {
                            j += 1;
                        }
                        let interior_landing = insns[cand.head..=cand.back]
                            .iter()
                            .enumerate()
                            .filter_map(|(rel, region_insn)| {
                                int_loop_target(cand.head + rel, region_insn)
                            })
                            .any(|t| t > i && t <= cand.back);
                        let fused = if j == cand.back && !interior_landing {
                            match &insns[cand.back] {
                                Insn::CmpJumpIfTrue(a, op, b, off) if a == d => {
                                    Some(Insn::CountCmpJumpTrue(*d, *op, *b, *imm, *off))
                                }
                                Insn::CmpJumpIfFalse(a, op, b, off) if a == d => {
                                    Some(Insn::CountCmpJumpFalse(*d, *op, *b, *imm, *off))
                                }
                                Insn::CmpJumpIfTrueConst(a, op, _, off)
                                    if a == d && cand.const_fuse.is_some() =>
                                {
                                    let (tmp, _) = cand.const_fuse.unwrap();
                                    Some(Insn::CountCmpJumpTrue(*d, *op, tmp, *imm, *off))
                                }
                                Insn::CmpJumpIfFalseConst(a, op, _, off)
                                    if a == d && cand.const_fuse.is_some() =>
                                {
                                    let (tmp, _) = cand.const_fuse.unwrap();
                                    Some(Insn::CountCmpJumpFalse(*d, *op, tmp, *imm, *off))
                                }
                                _ => None,
                            }
                        } else {
                            None
                        };
                        if let Some(fused_insn) = fused {
                            fast_index[cand.back - cand.head] = fast.len();
                            fast.push(IntLoopFastEntry::Copied(cand.back, fused_insn));
                            i = cand.back + 1;
                            continue;
                        }
                        fast.push(IntLoopFastEntry::Copied(i, insns[i].clone()));
                    }
                    _ => {
                        fast.push(IntLoopFastEntry::Copied(i, insns[i].clone()));
                    }
                }
                i += 1;
            }
            debug_assert!(
                pending.is_empty(),
                "a side exit must precede a later instruction in the region"
            );
        }

        let header_is_reentered =
            (cand.head + 1..=cand.back).any(|k| int_loop_target(k, &insns[k]) == Some(cand.head));
        let header_exit_skips_syncs =
            matches!(cand.kind, IntLoopCandidateKind::Inverted) && !header_is_reentered;

        let mut stub_targets: Vec<usize> = vec![cand.back + 1];
        for (fpos, entry) in fast.iter().enumerate() {
            let t = match entry {
                IntLoopFastEntry::SideExit(target, _) => *target,
                IntLoopFastEntry::Copied(old_i, insn) => {
                    let Some(t) = int_loop_target(*old_i, insn) else {
                        continue;
                    };
                    if t >= cand.head && t <= cand.back {
                        continue;
                    }
                    if fpos == 0 && header_exit_skips_syncs {
                        continue;
                    }
                    t
                }
            };
            if !stub_targets.contains(&t) {
                stub_targets.push(t);
            }
        }

        let fast_len = fast.len();
        let stubs_base = fast_base + fast_len;
        let stub_len = cand.syncs.len() + 1;
        let stub_abs = |t: usize, stub_targets: &[usize]| -> usize {
            let si = stub_targets.iter().position(|&x| x == t).unwrap();
            stubs_base + si * stub_len
        };

        // Emit fast insns with rewritten offsets.
        for (fpos, entry) in fast.iter().enumerate() {
            let abs = fast_base + fpos;
            let (old_i, insn) = match entry {
                IntLoopFastEntry::Copied(old_i, insn) | IntLoopFastEntry::SideExit(old_i, insn) => {
                    (old_i, insn)
                }
            };
            if let IntLoopFastEntry::SideExit(target, _) = entry {
                let dest_abs = stub_abs(*target, &stub_targets);
                let new_off = (dest_abs as i64 - abs as i64 - 1) as i32;
                layout.out.push(replace_jump_offset(insn.clone(), new_off));
                continue;
            }
            let directly_targets_past_end =
                int_loop_target(*old_i, insn) == Some(n) && fpos == 0 && header_exit_skips_syncs;
            let new_insn = match int_loop_target(*old_i, insn) {
                Some(t) => {
                    let internal = t >= cand.head && t <= cand.back;
                    let dest_abs = if internal {
                        let mut rel = t - cand.head;
                        while fast_index[rel] == usize::MAX {
                            rel += 1;
                        }
                        fast_base + fast_index[rel]
                    } else if fpos == 0 && header_exit_skips_syncs {
                        layout.jump_target[t]
                    } else {
                        stub_abs(t, &stub_targets)
                    };
                    let new_off = (dest_abs as i64 - abs as i64 - 1) as i32;
                    replace_jump_offset(insn.clone(), new_off)
                }
                None => insn.clone(),
            };
            if directly_targets_past_end {
                layout.past_end_patches.push(layout.out.len());
            }
            layout.out.push(new_insn);
        }

        // Emit deferred-sync stubs.
        for &t in &stub_targets {
            for &(r, name_idx) in &cand.syncs {
                layout.out.push(Insn::SyncModuleGlobal(r, name_idx));
            }
            let abs = layout.out.len();
            if t == n {
                layout.past_end_patches.push(abs);
            }
            let dest = if (cand.head..=cand.back).contains(&t) {
                layout.placement[t]
            } else {
                layout.jump_target[t]
            };
            layout
                .out
                .push(Insn::Jump((dest as i64 - abs as i64 - 1) as i32));
        }
    }

    let final_len = layout.out.len();
    for pc in layout.past_end_patches {
        let offset = (final_len as i64 - pc as i64 - 1) as i32;
        layout.out[pc] = replace_jump_offset(layout.out[pc].clone(), offset);
    }

    IntLoopVersioningResult {
        insns: layout.out,
        source_prefix_len: layout.source_prefix_len,
    }
}

fn pass_int_loop_version(
    insns: Vec<Insn>,
    descs: &mut Vec<InsnDesc>,
    consts: &mut Vec<Value>,
    names: &[pyrust_core::Symbol],
    num_locals: u32,
    num_regs: &mut u32,
) -> IntLoopVersioningResult {
    let n = insns.len();
    if n == 0 {
        return IntLoopVersioningResult {
            insns,
            source_prefix_len: 0,
        };
    }

    let reaching_const_reads = compute_reaching_const_reads(&insns, descs, num_locals);
    let mut candidates = find_int_loop_candidates(
        &insns,
        descs,
        consts,
        names,
        num_locals,
        num_regs,
        &reaching_const_reads,
    );
    if candidates.is_empty() {
        return IntLoopVersioningResult {
            insns,
            source_prefix_len: n,
        };
    }

    materialize_closed_form_consts(&mut candidates, consts);
    let layout = emit_int_loop_guards_and_main(&insns, descs, &candidates);
    emit_int_loop_fast_copies(&insns, &candidates, layout)
}
