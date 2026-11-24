// ─── Int-loop closed-form analysis & reaching-constant precomputation ─────────

/// The `(start, stop, step)` produced by a `range(...)` call with visible
/// int-constant arguments at the call site ahead of `head`.
///
/// This is a *proposal*, not a proof.  A rebound `range`, an aliased iterable,
/// or a computed argument can all make the runtime iterator disagree with the
/// triple returned here — which is precisely what the exact-bounds entry guard
/// is for.  The `LoadGlobal range` + argument setup + `Call` + `GetIter`
/// sequence must sit adjacent to the header, in the register layout the call
/// convention dictates.  An argument's `LoadConst` may sit earlier when LICM
/// hoisted it; `reaching_consts` admits that value only when its producer
/// reaches the read unambiguously and no loop-carried write can replace it.
///
/// The setup is *interpreted* over a small int-constant environment rather than
/// assumed to be one `LoadConst` per argument, because it is not: a negated
/// literal reaches this pass as `LoadConst` + `UnaryOp(Neg)` + `Move`.
fn traced_const_range_bounds(
    insns: &[Insn],
    consts: &[Value],
    names: &[pyrust_core::Symbol],
    reaching_consts: &HashMap<(usize, Reg), u16>,
    head: usize,
    slot: u8,
) -> Option<(i64, i64, i64)> {
    /// Instructions the compiler may spend materialising one argument:
    /// `LoadConst` + `UnaryOp(Neg)` + `Move` is the widest shape admitted here.
    const MAX_SETUP_PER_ARG: usize = 3;

    let Insn::GetIter(iter_slot, base) = *insns.get(head.checked_sub(1)?)? else {
        return None;
    };
    let call_at = head.checked_sub(2)?;
    let Insn::Call(callee, argc) = insns[call_at] else {
        return None;
    };
    if iter_slot != slot || callee != base || !(1..=3).contains(&argc) {
        return None;
    }
    let argc = usize::from(argc);
    // The setup is not a fixed length, so the producing `LoadGlobal` is not at a
    // fixed distance: take the nearest one that loads the call base, no further
    // back than the widest setup this trace admits.
    let window_start = call_at.saturating_sub(1 + argc * MAX_SETUP_PER_ARG);
    let load_global = (window_start..call_at).rev().find(|&i| {
        matches!(
            insns[i],
            Insn::LoadGlobal(dst, name_idx)
                if dst == base
                    && names.get(usize::from(name_idx)).map(|s| s.as_str()) == Some("range")
        )
    })?;

    let reaching_value = |at: usize, reg: Reg| -> Option<i64> {
        let cidx = *reaching_consts.get(&(at, reg))?;
        let value = consts.get(usize::from(cidx))?;
        if value.is_unset() || !matches!(value.kind(), ValueKind::Int(_)) {
            return None;
        }
        value.as_int()
    };

    // Interpret the local setup, consulting the precomputed reaching fact when
    // LICM placed a source `LoadConst` before `LoadGlobal range`. Anything
    // outside this whitelist — a computed bound or another call — abandons the
    // proposal rather than guessing.
    let mut env: Vec<(Reg, i64)> = Vec::new();
    for (relative_at, insn) in insns[load_global + 1..call_at].iter().enumerate() {
        let at = load_global + 1 + relative_at;
        let known_value = |reg: Reg| {
            env.iter()
                .find(|(known, _)| *known == reg)
                .map(|(_, value)| *value)
                .or_else(|| reaching_value(at, reg))
        };
        let (dst, value) = match *insn {
            Insn::LoadConst(dst, cidx) => {
                let value = consts.get(usize::from(cidx))?;
                if value.is_unset() || !matches!(value.kind(), ValueKind::Int(_)) {
                    return None;
                }
                (dst, value.as_int()?)
            }
            // `-i64::MIN` promotes to `BigInt`, which no machine-int cursor can
            // hold, so declining on the overflow is also the right answer.
            Insn::UnaryOp(dst, crate::ast::UnaryOp::Neg, src) => {
                (dst, known_value(src)?.checked_neg()?)
            }
            Insn::Move(dst, src) => (dst, known_value(src)?),
            _ => return None,
        };
        match env.iter_mut().find(|(reg, _)| *reg == dst) {
            Some((_, current)) => *current = value,
            None => env.push((dst, value)),
        }
    }

    let mut args = [0i64; 3];
    for (k, arg) in args.iter_mut().take(argc).enumerate() {
        let reg = base + 1 + k as Reg;
        *arg = env
            .iter()
            .find(|(known, _)| *known == reg)
            .map(|(_, value)| *value)
            .or_else(|| reaching_value(call_at, reg))?;
    }
    match argc {
        1 => Some((0, args[0], 1)),
        2 => Some((args[0], args[1], 1)),
        _ => Some((args[0], args[1], args[2])),
    }
}

/// Fold a straight-line accumulation body over a constant int range into its
/// closed form, or `None` when any part of the loop is not provably linear.
///
/// The admitted body is a sequence of `acc += <int constant>` and
/// `acc += <loop variable>` steps, in any mix, plus the module syncs the copy
/// defers to its exit stub.  Each such step contributes a delta that is a
/// closed-form function of the trip count, and nothing else in the body may
/// read or write an accumulator, so the whole loop collapses to one add per
/// accumulator plus the loop variable's final binding.
///
/// All the arithmetic runs in `i128` and every result must land back in `i64`.
/// That is not merely an overflow guard: it keeps the folded delta *exactly*
/// equal to the sum the iterated adds would have produced, so `acc + delta`
/// promotes to `BigInt` at the same value the original loop would have reached
/// by promoting mid-run.
fn closed_form_for(
    insns: &[Insn],
    consts: &[Value],
    names: &[pyrust_core::Symbol],
    reaching_consts: &HashMap<(usize, Reg), u16>,
    head: usize,
    back: usize,
    slot: u8,
) -> Option<ClosedForm> {
    let Insn::ForIter(var, _, _) = insns[head] else {
        return None;
    };
    let (start, stop, step) =
        traced_const_range_bounds(insns, consts, names, reaching_consts, head, slot)?;
    // A range outside the compact cursor's reach becomes `BigRange` at runtime
    // and could never match the guard, so the copy would be dead weight.
    if step == 0 || !i64_range_native_cursor_safe(start, stop, step) {
        return None;
    }
    let trips = range_len(start, stop, step);
    // A zero-trip range binds nothing and accumulates nothing.  Rather than
    // emit an empty copy whose exit stub would publish bindings the original
    // never made, leave the case to the ordinary per-iteration copy.
    if trips == 0 {
        return None;
    }
    let start_wide = i128::from(start);
    let step_wide = i128::from(step);
    let var_final = i64::try_from(start_wide + (trips - 1) * step_wide).ok()?;
    // Sum of the yielded values: `trips*start + step*trips*(trips-1)/2`.
    let sum = start_wide
        .checked_mul(trips)?
        .checked_add(step_wide.checked_mul(trips.checked_mul(trips - 1)? / 2)?)?;

    let mut acc_deltas: Vec<(Reg, i128)> = Vec::new();
    for insn in &insns[head + 1..back] {
        // `step_total` is what the step contributes over the whole run before
        // the operator's sign is applied.
        let (dst, src, op, step_total) = match insn {
            // Deferred to the exit stub, exactly as in the ordinary copy.
            Insn::SyncModuleGlobal(..) => continue,
            Insn::BinOpImm(dst, src, op, imm, _) => {
                (*dst, *src, *op, i128::from(*imm).checked_mul(trips)?)
            }
            Insn::BinOpConst(dst, src, op, cidx, _) => {
                let constant = consts
                    .get(usize::from(*cidx))
                    .filter(|value| !value.is_unset())
                    .and_then(Value::as_int)?;
                (*dst, *src, *op, i128::from(constant).checked_mul(trips)?)
            }
            Insn::BinOp(dst, src, op, rhs) | Insn::BinOpInPlace(dst, src, op, rhs)
                if *rhs == var =>
            {
                (*dst, *src, *op, sum)
            }
            _ => return None,
        };
        // Only the two operators that make repeated application a sum fold;
        // anything else (`*=`, bitwise, …) is not linear in the trip count.
        let delta = match op {
            BinaryOp::Add => step_total,
            BinaryOp::Sub => step_total.checked_neg()?,
            _ => return None,
        };
        // `acc = acc ± x` is the only linear shape: reading a different
        // register makes the step depend on values this fold does not track,
        // and writing the loop variable invalidates the traced sequence.
        if dst != src || dst == var {
            return None;
        }
        match acc_deltas.iter_mut().find(|(reg, _)| *reg == dst) {
            Some((_, total)) => *total = total.checked_add(delta)?,
            None => acc_deltas.push((dst, delta)),
        }
    }

    let acc_deltas = acc_deltas
        .into_iter()
        .map(|(reg, delta)| i64::try_from(delta).ok().map(|delta| (reg, delta)))
        .collect::<Option<Vec<_>>>()?;
    Some(ClosedForm {
        guard: IntRangeExactGuard {
            slot,
            start,
            stop,
            step,
        },
        var,
        var_final,
        acc_deltas,
        const_slots: Vec::new(),
    })
}

/// Constant producers visible at each register read.  LICM can move a `range`
/// argument's `LoadConst` before an enclosing loop header, so the call-site
/// trace cannot rely on finding that load in its short setup window.  Build the
/// fallback fact once in linear passes rather than rescanning a growing prefix
/// for every candidate.
///
/// A producer is omitted when a forward edge can skip it: the linear last
/// definition would then be ambiguous at a later join.  It is also omitted at a
/// read when a later write sits before the end of a back edge whose header is
/// at or before the read: that write could be carried into the next iteration
/// without re-running the producer.  Named locals are never facts because the
/// live namespace mirror can replace them outside this instruction stream.
fn compute_reaching_const_reads(
    insns: &[Insn],
    descs: &[InsnDesc],
    num_locals: u32,
) -> HashMap<(usize, Reg), u16> {
    let n = insns.len();
    let mut skipped_producer_delta = vec![0i32; n + 1];
    let mut backedge_end_at_target = vec![None; n];
    for (pc, insn) in insns.iter().enumerate() {
        let Some(target) = int_loop_target(pc, insn).filter(|&target| target < n) else {
            continue;
        };
        if target > pc + 1 {
            // The taken edge skips every producer in `[pc + 1, target)`; a
            // producer at `target` itself still executes.
            skipped_producer_delta[pc + 1] += 1;
            skipped_producer_delta[target] -= 1;
        } else if target <= pc {
            backedge_end_at_target[target] =
                Some(backedge_end_at_target[target].map_or(pc, |farthest: usize| farthest.max(pc)));
        }
    }
    let mut producer_can_be_skipped = vec![false; n];
    let mut skip_depth = 0i32;
    for pc in 0..n {
        skip_depth += skipped_producer_delta[pc];
        producer_can_be_skipped[pc] = skip_depth != 0;
    }

    let mut reaching_const_reads: HashMap<(usize, Reg), u16> = HashMap::new();
    let mut last_const: HashMap<Reg, (usize, u16)> = HashMap::new();
    let mut read_regs: HashSet<Reg> = HashSet::new();
    let mut written_regs: HashSet<Reg> = HashSet::new();
    for (pc, insn) in insns.iter().enumerate() {
        read_regs.clear();
        collect_reads(insn, descs, &mut read_regs);
        for &reg in &read_regs {
            if reg >= num_locals
                && let Some(&(producer, cidx)) = last_const.get(&reg)
                && !producer_can_be_skipped[producer]
            {
                reaching_const_reads.insert((pc, reg), cidx);
            }
        }

        written_regs.clear();
        collect_writes(insn, descs, &mut written_regs);
        for reg in &written_regs {
            last_const.remove(reg);
        }
        if let Insn::LoadConst(dst, cidx) = insn {
            last_const.insert(*dst, (pc, *cidx));
        }
    }

    // Merge overlapping back-edge intervals into transitive envelopes.  A
    // write can otherwise travel through chained backward jumps even when no
    // single edge spans both it and the later read.  Each instruction belongs
    // to at most one envelope, so discovering and filling them stays O(n).
    let mut backedge_extent = vec![None; n];
    let mut pc = 0usize;
    while pc < n {
        let Some(mut component_end) = backedge_end_at_target[pc] else {
            pc += 1;
            continue;
        };
        let component_start = pc;
        let mut scan = pc + 1;
        while scan <= component_end {
            if let Some(nested_end) = backedge_end_at_target[scan] {
                component_end = component_end.max(nested_end);
            }
            scan += 1;
        }
        backedge_extent[component_start..=component_end].fill(Some(component_end));
        pc = component_end + 1;
    }
    let mut next_write: HashMap<Reg, usize> = HashMap::new();
    for pc in (0..n).rev() {
        written_regs.clear();
        collect_writes(&insns[pc], descs, &mut written_regs);
        for &reg in &written_regs {
            // Include a same-instruction write: a read-then-write operation in
            // a loop would otherwise feed its previous iteration's result into
            // the next one instead of the proposed constant.
            next_write.insert(reg, pc);
        }
        if let Some(component_end) = backedge_extent[pc] {
            read_regs.clear();
            collect_reads(&insns[pc], descs, &mut read_regs);
            for &reg in &read_regs {
                if next_write
                    .get(&reg)
                    .is_some_and(|&write| write <= component_end)
                {
                    reaching_const_reads.remove(&(pc, reg));
                }
            }
        }
    }

    reaching_const_reads
}
