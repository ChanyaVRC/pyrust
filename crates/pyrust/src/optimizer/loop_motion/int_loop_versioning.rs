// ─── Int-loop versioning ───────────────────────────────────────────────────────

/// Out-of-line int specialization for innermost counted loops, per
/// ARCHITECTURE.md rule 29.
///
/// A module-scope loop body executes `SyncModuleGlobal` after every fast-local
/// assignment so that live namespace aliases observe each iteration.  Rule 29
/// permits deferring those syncs to the loop exits only when every operation in
/// the loop is a proven non-reentrant primitive.  That proof cannot be purely
/// static — a namespace-mirror write (rule 27) can rebind any module fast-local
/// to an arbitrary object between loop entries — so this pass emits a
/// **runtime-guarded** version instead of rewriting the loop in place:
///
/// ```text
/// pre:
///   JumpIfNotInt(r1, → orig_head)      ; entry guards, one per source reg
///   …
///   Jump(→ fast_head)                  ; all guards passed
/// orig_head:                            ; original loop, byte-for-byte,
///   …                                   ; per-iteration syncs intact
/// post:
///   …
/// fast_head:                            ; appended out-of-line copy
///   <header exit jumps straight to its original target — a zero-trip
///    entry runs no body insn and no sync, exactly like the original>
///   <body with SyncModuleGlobal removed and the trailing
///    BinOpImm(v,v,Add,imm) + CmpJump back-edge fused into CountCmpJump*>
/// stub(t):                              ; one per body exit target
///   SyncModuleGlobal(…)                 ; deferred syncs, deduplicated
///   Jump(→ t)
/// ```
///
/// ## Mid-loop side exits
///
/// Entry guards can only establish loop-invariant facts.  Two operations the
/// pass admits produce values whose type is a *per-iteration* fact — a
/// `ForIter` step over a canonical list/tuple, and a canonical sequence
/// subscript — so the specialized copy carries **mid-loop side exits**:
///
/// ```text
/// fast_head:
///   ForIter(x, slot, → stub(exit))
///   JumpIfNotInt(x, → side(head+1))    ; element type is per-iteration
///   GetItemSeqOrExit(v, xs, i, → side(i_sub))
///   JumpIfNotInt(v, → side(i_sub))     ; element type is per-iteration
///   …
/// side(t):                              ; same shape as an exit stub
///   SyncModuleGlobal(…)                 ; every deferred sync, flushed
///   Jump(→ original instruction t)      ; resume the original loop mid-body
/// ```
///
/// Flushing *every* deferred sync is always correct: a synced register either
/// still holds the value the previous iteration published, or holds the value
/// the original would already have published at that program point.  The
/// iterator slot is shared by both copies, so resuming the original loop needs
/// no cursor fix-up, and any subsequent raise happens on the original path with
/// its own source line, PEP 657 caret span, and a synchronized namespace.
///
/// A side exit re-enters the original stream *at* the guarded operation (or,
/// for `ForIter`, at the instruction after it, because the cursor has already
/// advanced).  Re-running a canonical sequence read observes exactly the same
/// state, so the deopt target is only reached in states where the original
/// instruction reproduces the fast copy's effect — or raises, which is the
/// point.  Candidates whose subscript would clobber its own operand register,
/// or that could branch around a guarded definition, are rejected instead.
///
/// ## Soundness
///
/// - Eligible regions contain only `Move`/`CopyReg`, int-pool `LoadConst`,
///   `{Add,Sub,Mul}` binary forms, `{Eq,Ne,Lt,Le,Gt,Ge}` compare-jumps,
///   truthiness jumps on guarded registers, plain `Jump`, and
///   `SyncModuleGlobal`.  Every source register is guarded by `JumpIfNotInt`
///   at entry, and each allowed operation maps int-family inputs to int-family
///   outputs (`int` overflow promotes to `BigInt`, which stays primitive), so
///   no instruction in the fast copy can raise, invoke user code, or observe
///   the module namespace.  Deferring the syncs is therefore unobservable
///   until an exit stub flushes them.
/// - The original loop stays in place and untouched: any non-int entry state
///   (bool, BigInt beyond i64, user object, unset register) runs it with its
///   original per-iteration syncs, source lines, and caret spans.  The
///   appended copy cannot raise, so its lack of line-table anchors is inert.
/// - A sync-bearing candidate has a straight-line body.  This is required not
///   only to ensure every deferred binding is current at the exit, but also to
///   preserve the first-insertion order of the live globals dict: two
///   conditionally assigned names can first execute in a different order from
///   their lexical `SyncModuleGlobal` order.  Branching loops stay on the
///   original per-assignment sync path.
/// - The header copy's exit edge bypasses the sync stubs: on a zero-trip entry
///   no body instruction has executed, and the original would not have synced
///   either.  The back-edge fall-through goes through a stub.
/// - Regions containing another back-edge (nested loops), `Yield`, calls, or
///   any instruction outside the whitelist are rejected; outer loops simply
///   keep their original form while their inner loop is versioned.
struct IntLoopVersioningResult {
    insns: Vec<Insn>,
    /// Exclusive end of the source-derived main stream. Instructions after
    /// this boundary are out-of-line fast copies and must never participate in
    /// source-origin matching, even when they are structurally identical to an
    /// instruction emitted by the compiler.
    source_prefix_len: usize,
}

fn pass_int_loop_version(
    insns: Vec<Insn>,
    consts: &[Value],
    num_regs: &mut u32,
) -> IntLoopVersioningResult {
    const MAX_REGION: usize = 48;
    const MAX_GUARDS: usize = 8;

    enum CandidateKind {
        /// Inverted while loop: conditional header, conditional back-edge.
        Inverted,
        /// `for … in <iterable>`: `ForIter` header, unconditional back-edge;
        /// the guard block additionally checks the iterator slot's kind.
        ForIter { slot: u8 },
    }

    /// One out-of-line copy of a candidate region, reached through its own
    /// iterator-kind guard chain.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FastVariant {
        /// No iterator guard: the inverted-while copy.
        Plain,
        /// `for … in range(…)`: the canonical machine-int cursor always yields
        /// an `int`, so the loop variable needs no per-iteration guard.
        IntRange,
        /// `for … in <list|tuple>`: the element type is a per-iteration fact,
        /// so the copy opens with a `JumpIfNotInt` side exit on the loop
        /// variable.
        IndexedSeq,
    }

    struct Candidate {
        kind: CandidateKind,
        head: usize,
        back: usize,
        guards: Vec<Reg>,
        syncs: Vec<(Reg, u16)>,
        /// `(tmp_reg, const_idx)`: the back-edge compares against a constant
        /// pool slot; the guard block materialises it once into `tmp_reg` so
        /// the fused `CountCmpJump*` can use the register form.
        const_fuse: Option<(Reg, u16)>,
        /// One appended fast copy per entry, in guard-chain order.
        variants: Vec<FastVariant>,
    }

    impl Candidate {
        /// Instructions the guard block contributes ahead of the original head.
        fn guard_block_len(&self) -> usize {
            let per_variant = usize::from(matches!(self.kind, CandidateKind::ForIter { .. }))
                + self.guards.len()
                + usize::from(self.const_fuse.is_some())
                + 1;
            self.variants.len() * per_variant
        }
    }

    /// A fast-copy entry: either a rewritten copy of the original instruction
    /// at some old index, or a synthesized guard whose failure edge is the
    /// deferred-sync side-exit stub for the given old index.
    enum FastEntry {
        Copied(usize, Insn),
        SideExit(usize, Insn),
    }

    let n = insns.len();
    if n == 0 {
        return IntLoopVersioningResult {
            insns,
            source_prefix_len: 0,
        };
    }

    let const_is_int = |idx: u16| -> bool {
        consts
            .get(idx as usize)
            .is_some_and(|v| !v.is_unset() && matches!(v.kind(), ValueKind::Int(_)))
    };
    let cmp_op = |op: &BinaryOp| {
        matches!(
            op,
            BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
        )
    };
    // Int-family-closed operations that cannot raise on int operands.
    let arith_op = |op: &BinaryOp| {
        matches!(
            op,
            BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::BitAnd
                | BinaryOp::BitOr
                | BinaryOp::BitXor
        )
    };
    // Division-family ops additionally require a known non-zero divisor, so
    // they are admitted only in immediate/constant form.
    let imm_arith_op = |op: &BinaryOp, imm: i16| {
        arith_op(op) || (matches!(op, BinaryOp::FloorDiv | BinaryOp::Mod) && imm != 0)
    };
    let const_arith_op = |op: &BinaryOp, idx: u16| {
        arith_op(op)
            || (matches!(op, BinaryOp::FloorDiv | BinaryOp::Mod)
                && consts
                    .get(idx as usize)
                    .and_then(|v| if v.is_unset() { None } else { v.as_int() })
                    .is_some_and(|value| value != 0))
    };

    // Jump targets of an instruction, as absolute indices.
    let targets = |i: usize, insn: &Insn| -> Option<usize> {
        let off = match insn {
            Insn::Jump(k)
            | Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::JumpIfNotInt(_, k)
            | Insn::JumpIfIterNotIntRange(_, k)
            | Insn::JumpIfIterNotIndexedSeq(_, k)
            | Insn::GetItemSeqOrExit(_, _, _, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::CountCmpJumpTrue(_, _, _, _, k)
            | Insn::CountCmpJumpFalse(_, _, _, _, k)
            | Insn::CallInlineBinOp { skip: k, .. }
            | Insn::ForIter(_, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => *k,
            _ => return None,
        };
        Some((i as i64 + 1 + off as i64) as usize)
    };

    // ── Find candidates ────────────────────────────────────────────────────
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut h = 0usize;
    while h < n {
        // Header: forward conditional jump (inverted while) or ForIter
        // (for-range) over the whole region.
        let (kind, k) = match &insns[h] {
            Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
                if *k > 1 =>
            {
                (CandidateKind::Inverted, *k as usize)
            }
            Insn::ForIter(_, slot, k) if *k > 1 => {
                (CandidateKind::ForIter { slot: *slot }, *k as usize)
            }
            _ => {
                h += 1;
                continue;
            }
        };
        let back = h + k;
        if back >= n || k > MAX_REGION {
            h += 1;
            continue;
        }
        // Back-edge: an inverted while re-checks its condition at the body
        // end (the shape produced by `pass_loop_inversion`); a for-range
        // returns to its ForIter with an unconditional Jump.
        let back_targets_body = match (&kind, &insns[back]) {
            (
                CandidateKind::Inverted,
                Insn::CmpJumpIfFalse(_, _, _, kb)
                | Insn::CmpJumpIfTrue(_, _, _, kb)
                | Insn::CmpJumpIfFalseConst(_, _, _, kb)
                | Insn::CmpJumpIfTrueConst(_, _, _, kb)
                | Insn::JumpIfFalse(_, kb)
                | Insn::JumpIfTrue(_, kb),
            ) => *kb == -(k as i32),
            (CandidateKind::ForIter { .. }, Insn::Jump(kb)) => *kb == -(k as i32 + 1),
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
        // Old indices of `GetItem`s the fast copy may run as the deopting
        // `GetItemSeqOrExit`.
        let mut subscripts: Vec<usize> = Vec::new();
        let walk_start = match kind {
            // The ForIter header is not in the body whitelist; the runtime
            // iterator-slot guard owns its semantics.
            CandidateKind::ForIter { .. } => h + 1,
            CandidateKind::Inverted => h,
        };
        for i in walk_start..=back {
            match &insns[i] {
                Insn::Move(_, s) | Insn::CopyReg(_, s) => guard(*s, &mut guards),
                Insn::LoadConst(_, idx) => {
                    if !const_is_int(*idx) {
                        eligible = false;
                        break;
                    }
                }
                Insn::BinOp(_, a, op, b) | Insn::BinOpInPlace(_, a, op, b) => {
                    if !arith_op(op) {
                        eligible = false;
                        break;
                    }
                    guard(*a, &mut guards);
                    guard(*b, &mut guards);
                }
                Insn::BinOpImm(_, s, op, imm, _) => {
                    if !imm_arith_op(op, *imm) {
                        eligible = false;
                        break;
                    }
                    guard(*s, &mut guards);
                }
                Insn::BinOpConst(_, s, op, idx, _) => {
                    if !const_arith_op(op, *idx) || !const_is_int(*idx) {
                        eligible = false;
                        break;
                    }
                    guard(*s, &mut guards);
                }
                Insn::CmpJumpIfFalse(a, op, b, off) | Insn::CmpJumpIfTrue(a, op, b, off) => {
                    let continue_edge = *off < 0 && (i as i64 + 1 + *off as i64) == h as i64;
                    if !cmp_op(op) || (i != h && i != back && *off < 0 && !continue_edge) {
                        eligible = false;
                        break;
                    }
                    has_interior_control_flow |= i != h && i != back;
                    guard(*a, &mut guards);
                    guard(*b, &mut guards);
                }
                Insn::CmpJumpIfFalseConst(a, op, idx, off)
                | Insn::CmpJumpIfTrueConst(a, op, idx, off) => {
                    let continue_edge = *off < 0 && (i as i64 + 1 + *off as i64) == h as i64;
                    if !cmp_op(op)
                        || !const_is_int(*idx)
                        || (i != h && i != back && *off < 0 && !continue_edge)
                    {
                        eligible = false;
                        break;
                    }
                    has_interior_control_flow |= i != h && i != back;
                    guard(*a, &mut guards);
                }
                Insn::JumpIfFalse(r, off) | Insn::JumpIfTrue(r, off) => {
                    let continue_edge = *off < 0 && (i as i64 + 1 + *off as i64) == h as i64;
                    if i != h && i != back && *off < 0 && !continue_edge {
                        eligible = false;
                        break;
                    }
                    has_interior_control_flow |= i != h && i != back;
                    guard(*r, &mut guards);
                }
                Insn::Jump(off) => {
                    let is_forrange_backedge =
                        i == back && matches!(kind, CandidateKind::ForIter { .. });
                    // A backward jump to the region head is a `continue`; any
                    // other backward jump is a nested loop's back-edge and only
                    // innermost regions are versioned.
                    let continue_edge = *off < 0 && (i as i64 + 1 + *off as i64) == h as i64;
                    if *off < 0 && !is_forrange_backedge && !continue_edge {
                        eligible = false;
                        break;
                    }
                    has_interior_control_flow |= i != h && i != back;
                }
                Insn::GetItem(dst, obj, idx) => {
                    // Admitted through a mid-loop side exit: the fast copy runs
                    // the deopting `GetItemSeqOrExit` and guards its result.
                    // Neither operand may be entry-guarded (the sequence is not
                    // an int, and the index may legitimately be the loop
                    // variable), and the deopt only reproduces the original
                    // read when the destination is not one of them.
                    if dst == obj || dst == idx {
                        eligible = false;
                        break;
                    }
                    subscripts.push(i);
                }
                Insn::SyncModuleGlobal(r, name_idx) => {
                    if !syncs.contains(&(*r, *name_idx)) {
                        syncs.push((*r, *name_idx));
                    }
                }
                _ => {
                    eligible = false;
                    break;
                }
            }
        }
        if let CandidateKind::ForIter { .. } = kind {
            // The ForIter target register is (re)written by the guarded cursor
            // before every body execution, so its entry state is irrelevant —
            // and on a fresh loop it is legitimately unset.  Over a canonical
            // sequence the element type is a per-iteration fact instead, which
            // the copy's opening side exit covers.
            if let Insn::ForIter(dst, _, _) = &insns[h] {
                guards.retain(|g| g != dst);
            }
        }
        // A subscript destination is likewise rewritten every iteration, and
        // its side exit dominates every later read only while the body is
        // straight-line and nothing reads it ahead of its definition.
        for &si in &subscripts {
            let Insn::GetItem(dst, _, _) = &insns[si] else {
                unreachable!("only GetItem sites are recorded as subscripts");
            };
            if insns[h..si].iter().any(|insn| insn_reads_reg(insn, *dst)) {
                eligible = false;
                break;
            }
            guards.retain(|g| g != dst);
        }
        if has_interior_control_flow && !subscripts.is_empty() {
            eligible = false;
        }
        // Interior control flow is compatible with sync deferral only when no
        // synced name can be first-inserted into the live globals dict by this
        // loop: conditionally-executed first insertions could otherwise change
        // the dict's insertion order.  A name is proven pre-bound when the
        // same (reg, name) sync already ran on the straight-line path before
        // the loop head, and the function never deletes bindings.
        let function_deletes_bindings = || {
            insns.iter().any(|insn| {
                matches!(
                    insn,
                    Insn::DeleteModuleGlobal(_) | Insn::DeleteName(_) | Insn::DeleteLocal(..)
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
        if !eligible
            || guards.len() > MAX_GUARDS
            || (!syncs.is_empty()
                && has_interior_control_flow
                && (function_deletes_bindings() || !syncs_pre_bound()))
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
        // A branch landing after the add but before/on the latch would enter a
        // fused instruction through its compare half in the original stream,
        // while the fused opcode would execute the add too. Reject that fusion
        // before allocating any constant-stop temporary or accepting a
        // fusion-only candidate.
        let has_fusion_interior_landing = prev_non_sync.is_some_and(|p| {
            insns[h..=back]
                .iter()
                .enumerate()
                .filter_map(|(rel, region_insn)| targets(h + rel, region_insn))
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
        // A constant-stop back-edge fuses too: the guard block materialises
        // the (already int-checked) constant into a fresh register once per
        // loop entry so the register-form `CountCmpJump*` applies.
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
        let has_fusion = has_reg_fusion || const_fuse.is_some();
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
            if let Some(t) = targets(i, insn)
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
        // A `for` header admits two iterator kinds, each with its own guard
        // chain and copy: the machine-int range cursor, and — when the body is
        // straight-line, so the opening side exit dominates every use of the
        // loop variable — the canonical list/tuple index cursor.
        let variants = match kind {
            CandidateKind::Inverted => vec![FastVariant::Plain],
            CandidateKind::ForIter { .. } if has_interior_control_flow => {
                vec![FastVariant::IntRange]
            }
            CandidateKind::ForIter { .. } => {
                vec![FastVariant::IntRange, FastVariant::IndexedSeq]
            }
        };
        candidates.push(Candidate {
            kind,
            head: h,
            back,
            guards,
            syncs,
            const_fuse,
            variants,
        });
        h = back + 1;
    }

    if candidates.is_empty() {
        return IntLoopVersioningResult {
            insns,
            source_prefix_len: n,
        };
    }

    // ── Rebuild ────────────────────────────────────────────────────────────
    // Placement map: each candidate's guard block (guards + 1 trailing Jump)
    // is inserted immediately before its head.
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
    // `placement[n] == main_len` is only the end of the rebuilt *main*
    // stream. Once fast copies are appended it is no longer the bytecode
    // past-the-end sentinel, so old edges to `n` are patched after the final
    // output length is known. Otherwise a final break/loop exit jumps into the
    // first appended copy and can replay the function tail.
    let mut past_end_patches: Vec<usize> = Vec::new();
    // Main stream: guard blocks (offsets patched later) + remapped originals.
    // Per candidate, the out index of each variant's trailing dispatch jump.
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
                    if let CandidateKind::ForIter { slot } = cand.kind {
                        // A rejected iterator kind falls through to the next
                        // guard chain; the last one gives up on the fast copies
                        // entirely and runs the original loop.
                        let fail = if vi + 1 < cand.variants.len() {
                            block_end
                        } else {
                            placement[i]
                        };
                        let fail_off = (fail as i64 - out.len() as i64 - 1) as i32;
                        out.push(match variant {
                            FastVariant::IntRange => Insn::JumpIfIterNotIntRange(slot, fail_off),
                            FastVariant::IndexedSeq => {
                                Insn::JumpIfIterNotIndexedSeq(slot, fail_off)
                            }
                            FastVariant::Plain => {
                                unreachable!("a for header always guards its iterator kind")
                            }
                        });
                    }
                    for g in &cand.guards {
                        let gpos = out.len();
                        let fail_off = placement[i] as i64 - gpos as i64 - 1;
                        out.push(Insn::JumpIfNotInt(*g, fail_off as i32));
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
            if targets(i, insn) == Some(n) {
                past_end_patches.push(out.len());
            }
            out.push(remapped);
        }
    }
    debug_assert_eq!(out.len(), main_len);

    // The main stream may end by falling off the end (module frames have no
    // trailing Return).  Insert a barrier jump past everything that will be
    // appended so execution can never fall into the first fast copy; like the
    // explicit past-the-end edges above, it is patched to the final length.
    past_end_patches.push(out.len());
    out.push(Insn::Jump(0));
    let source_prefix_len = out.len();

    // Appended fast copies + stubs, one per candidate variant.
    let variant_copies: Vec<(usize, usize, FastVariant)> = candidates
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
        let fast_base = out.len();
        // Patch this variant's guard-block trailing jump.
        let jpos = guard_jump_patches[ci][vi];
        out[jpos] = Insn::Jump((fast_base as i64 - jpos as i64 - 1) as i32);

        // fast_index[i - head] = index within the fast copy (before offsets),
        // usize::MAX for skipped syncs / fused-away insns.
        let mut fast_index = vec![usize::MAX; cand.back - cand.head + 1];
        let mut fast: Vec<FastEntry> = Vec::new();
        {
            // Side exits are emitted immediately *before* the next original
            // instruction rather than after their producer, so an internal edge
            // landing there re-runs the guard instead of skipping it.
            let mut pending: Vec<(Reg, usize)> = Vec::new(); // (guarded reg, old target)
            let mut i = cand.head;
            while i <= cand.back {
                let entry_pos = fast.len();
                for (reg, target) in pending.drain(..) {
                    fast.push(FastEntry::SideExit(target, Insn::JumpIfNotInt(reg, 0)));
                }
                fast_index[i - cand.head] = entry_pos;
                match &insns[i] {
                    Insn::SyncModuleGlobal(..) => {}
                    Insn::ForIter(dst, _, _)
                        if i == cand.head && variant == FastVariant::IndexedSeq =>
                    {
                        // The element type is a per-iteration fact: guard it
                        // before the body, side-exiting to the instruction
                        // after the original ForIter because the shared cursor
                        // has already advanced.
                        fast.push(FastEntry::Copied(i, insns[i].clone()));
                        pending.push((*dst, cand.head + 1));
                    }
                    Insn::GetItem(dst, obj, idx) => {
                        fast.push(FastEntry::SideExit(
                            i,
                            Insn::GetItemSeqOrExit(*dst, *obj, *idx, 0),
                        ));
                        // Re-running the original subscript after a deopt reads
                        // the same element, so both guards resume at it.
                        pending.push((*dst, i));
                    }
                    Insn::BinOpImm(d, s, BinaryOp::Add, imm, _) if d == s && i < cand.back => {
                        // Try to fuse with the back-edge compare-jump when the
                        // only instructions between them are removed syncs and
                        // no jump lands between the add and the compare (a jump
                        // to the fused instruction would otherwise execute an
                        // add the original landing point did not).
                        let mut j = i + 1;
                        while j < cand.back && matches!(insns[j], Insn::SyncModuleGlobal(..)) {
                            j += 1;
                        }
                        let interior_landing = insns[cand.head..=cand.back]
                            .iter()
                            .enumerate()
                            .filter_map(|(rel, region_insn)| targets(cand.head + rel, region_insn))
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
                            // Attribute the fused insn to the back-edge index so
                            // its jump offset is rewritten relative to it.
                            fast.push(FastEntry::Copied(cand.back, fused_insn));
                            i = cand.back + 1;
                            continue;
                        }
                        fast.push(FastEntry::Copied(i, insns[i].clone()));
                    }
                    _ => {
                        fast.push(FastEntry::Copied(i, insns[i].clone()));
                    }
                }
                i += 1;
            }
            debug_assert!(
                pending.is_empty(),
                "a side exit must precede a later instruction in the region"
            );
        }
        // Resolve, per external target, a stub slot.  The back-edge's
        // fall-through (the normal loop exit, `back + 1`) always needs one and
        // is emitted first so execution falls straight into it.
        let mut stub_targets: Vec<usize> = vec![cand.back + 1]; // old absolute targets
        for (fpos, entry) in fast.iter().enumerate() {
            let t = match entry {
                // A side exit always resumes the original stream, even though
                // its target is inside the region.
                FastEntry::SideExit(target, _) => *target,
                FastEntry::Copied(old_i, insn) => {
                    let Some(t) = targets(*old_i, insn) else {
                        continue;
                    };
                    let internal = t >= cand.head && t <= cand.back;
                    if internal {
                        continue;
                    }
                    // An inverted-while header runs once, on entry: its exit
                    // edge is the zero-trip path and must not flush syncs the
                    // original never executed.  A `for` header is the recurring
                    // exhaustion test, so its exit flushes like any body exit
                    // (a zero-trip pass is safe there: never-assigned registers
                    // are unset and the sync skips them, and already-synced
                    // names rewrite identical values).
                    if fpos == 0 && matches!(cand.kind, CandidateKind::Inverted) {
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
                FastEntry::Copied(old_i, insn) | FastEntry::SideExit(old_i, insn) => (old_i, insn),
            };
            if let FastEntry::SideExit(target, _) = entry {
                let dest_abs = stub_abs(*target, &stub_targets);
                let new_off = (dest_abs as i64 - abs as i64 - 1) as i32;
                out.push(replace_jump_offset(insn.clone(), new_off));
                continue;
            }
            // Only an inverted-while header keeps a direct edge to old
            // past-the-end. Every `for` exhaustion and every body exit is
            // deliberately routed through a deferred-sync stub; registering
            // those source edges here would overwrite the stub destination
            // during the final-length patch and silently skip the syncs.
            let directly_targets_past_end = targets(*old_i, insn) == Some(n)
                && fpos == 0
                && matches!(cand.kind, CandidateKind::Inverted);
            let new_insn = match targets(*old_i, insn) {
                Some(t) => {
                    let internal = t >= cand.head && t <= cand.back;
                    let dest_abs = if internal {
                        // Map to the first fast insn at or after `t`.
                        let mut rel = t - cand.head;
                        while fast_index[rel] == usize::MAX {
                            rel += 1;
                        }
                        fast_base + fast_index[rel]
                    } else if fpos == 0 && matches!(cand.kind, CandidateKind::Inverted) {
                        jump_target[t]
                    } else {
                        stub_abs(t, &stub_targets)
                    };
                    let new_off = (dest_abs as i64 - abs as i64 - 1) as i32;
                    replace_jump_offset(insn.clone(), new_off)
                }
                None => insn.clone(),
            };
            if directly_targets_past_end {
                past_end_patches.push(out.len());
            }
            out.push(new_insn);
        }
        // Emit stubs.  The first (the back-edge fall-through, `back + 1`) is
        // entered by falling off the fast copy; the rest only by body jumps.
        for &t in &stub_targets {
            for &(r, name_idx) in &cand.syncs {
                out.push(Insn::SyncModuleGlobal(r, name_idx));
            }
            let abs = out.len();
            if t == n {
                past_end_patches.push(abs);
            }
            out.push(Insn::Jump((jump_target[t] as i64 - abs as i64 - 1) as i32));
        }
    }

    let final_len = out.len();
    for pc in past_end_patches {
        let offset = (final_len as i64 - pc as i64 - 1) as i32;
        out[pc] = replace_jump_offset(out[pc].clone(), offset);
    }

    IntLoopVersioningResult {
        insns: out,
        source_prefix_len,
    }
}

/// Replace the single jump offset carried by `insn` with `off`.
fn replace_jump_offset(insn: Insn, off: i32) -> Insn {
    use Insn::*;
    match insn {
        Jump(_) => Jump(off),
        JumpIfFalse(r, _) => JumpIfFalse(r, off),
        JumpIfTrue(r, _) => JumpIfTrue(r, off),
        JumpIfNotInt(r, _) => JumpIfNotInt(r, off),
        JumpIfIterNotIntRange(s2, _) => JumpIfIterNotIntRange(s2, off),
        JumpIfIterNotIndexedSeq(s2, _) => JumpIfIterNotIndexedSeq(s2, off),
        GetItemSeqOrExit(dst, obj, idx, _) => GetItemSeqOrExit(dst, obj, idx, off),
        CmpJumpIfFalse(a, op, b, _) => CmpJumpIfFalse(a, op, b, off),
        CmpJumpIfTrue(a, op, b, _) => CmpJumpIfTrue(a, op, b, off),
        CmpJumpIfFalseConst(a, op, c, _) => CmpJumpIfFalseConst(a, op, c, off),
        CmpJumpIfTrueConst(a, op, c, _) => CmpJumpIfTrueConst(a, op, c, off),
        CountCmpJumpTrue(v, op, s, imm, _) => CountCmpJumpTrue(v, op, s, imm, off),
        CountCmpJumpFalse(v, op, s, imm, _) => CountCmpJumpFalse(v, op, s, imm, off),
        CallInlineBinOp {
            callee,
            dst,
            a,
            op,
            b,
            proto,
            ..
        } => CallInlineBinOp {
            callee,
            dst,
            a,
            op,
            b,
            proto,
            skip: off,
        },
        ForIter(dst, slot, _) => ForIter(dst, slot, off),
        SetupExcept(_) => SetupExcept(off),
        MatchExcept(r, _) => MatchExcept(r, off),
        MatchExceptStar(r, src, dst, _) => MatchExceptStar(r, src, dst, off),
        other => other,
    }
}
