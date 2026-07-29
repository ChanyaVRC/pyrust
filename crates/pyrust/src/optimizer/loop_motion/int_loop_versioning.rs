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
/// Entry guards can only establish loop-invariant facts.  Three operations the
/// pass admits produce values that are a *per-iteration* fact — a `ForIter`
/// step over a canonical list/tuple, a canonical sequence subscript, and a
/// canonical sequence length — so the specialized copy carries **mid-loop side
/// exits**:
///
/// ```text
/// fast_head:
///   ForIter(x, slot, → stub(exit))
///   JumpIfNotInt(x, → side(head+1))    ; element type is per-iteration
///   GetItemSeqIntOrExit(v, xs, i, → side(i_sub))   ; read *and* element type
///   LenSeqOrExit(c, xs, → side(i_len)) ; the length is per-iteration too
///   …
/// side(t):                              ; same shape as an exit stub
///   SyncModuleGlobal(…)                 ; every deferred sync, flushed
///   Jump(→ original instruction t)      ; resume the original loop mid-body
/// ```
///
/// Flushing *every* deferred sync is correct for an admitted candidate: a
/// synced register either still holds the value the previous iteration
/// published, or holds the value the original would already have published at
/// that program point.  That is a property of the region, not a given — a
/// module-scope `name = <expr>` publishes from the scratch register the next
/// expression reuses, so a candidate is admitted only when every sync source
/// still holds its published value at the exits (see
/// `sync_sources_republish_exactly`).  The
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
/// A subscript's *element type* is folded into `GetItemSeqIntOrExit` rather
/// than following it as a separate `JumpIfNotInt`: both facts deopt to the same
/// instruction, and a region carrying a subscript admits no interior branch
/// that could land between a read and its check.
///
/// ## `while i < len(seq):` headers
///
/// A `len` call in a while header is re-evaluated on every iteration, so
/// `pass_loop_inversion` cannot collapse the back-edge and the region opens
/// with the call triple instead of a comparison:
///
/// ```text
/// head:   LoadGlobal(c, "len") + Move(c+1, seq) + Call(c, 1)
/// hdr:    CmpJumpIfFalse(i, <, c, → exit)
///         …body…
///         BinOpImm(i, i, Add, 1)
/// back:   Jump(→ head)
/// ```
///
/// The copy replaces the triple with a native length read and rotates the
/// header down to the latch, where the counter increment fuses into it:
///
/// ```text
/// pre:
///   LoadGlobal(c, "len")
///   JumpIfNotBuiltinLen(c, → orig_head)  ; a rebound `len` runs the real call
///   JumpIfNotInt(…)                       ; the usual register chain
///   Jump(→ fast_head)
/// fast_head:
///   LenSeqOrExit(c, seq, → side(head))
///   CmpJumpIfFalse(i, <, c, → stub(exit)) ; zero-trip test, runs once
/// body:
///   …
///   LenSeqOrExit(c, seq, → side(the increment))
///   CountCmpJumpTrue(i, <, c, 1, → body)  ; increment + re-derived bound
/// ```
///
/// The length read stays **inside** the loop.  That is the correctness line the
/// historical AST rewrite (#289) crossed by hoisting it: a per-iteration read
/// keeps `len` observable exactly where CPython observes it, so a body that
/// moves the bound moves it here too, and nothing about the sequence's identity
/// or size is assumed across iterations.
///
/// Two guards make the substitution legitimate.  `JumpIfNotBuiltinLen` checks
/// the *value* the header just loaded, so a `def len` shadow, an assignment, a
/// `globals()` write, or a `builtins.len` patch all run the original call.
/// `LenSeqOrExit` checks its argument on every read, so a user `__len__`, a
/// `dict`, or an oversized `range` deopts to the original `LoadGlobal` — which
/// owns the protocol dispatch, the raise, and the diagnostics.
///
/// Rotating the latch reads the length *above* the increment.  The two touch
/// disjoint registers and neither can raise or re-enter, so the reorder is
/// unobservable — provided the rotated read's side exit resumes the original at
/// the increment it has not yet run, which is what its stub does.
///
/// ## Closed-form copies
///
/// A `for … in range(<int constants>)` whose body is nothing but
/// `acc += <constant>` / `acc += <loop variable>` steps has an effect that is a
/// closed-form function of the trip count, so its copy need not iterate at all:
///
/// ```text
/// pre:
///   JumpIfIterNotIntRangeExact(slot, {start, stop, step}, → next guard chain)
///   JumpIfNotInt(acc, → orig_head)
///   Jump(→ closed_head)
/// …
/// closed_head:
///   LoadConst(v, <last value the range yields>)
///   BinOpConst(acc, acc, Add, <total delta>)
///   ; falls through into the exit stub — no back-edge, no exit jump
/// ```
///
/// The entry guard is the strong one: `JumpIfIterNotIntRangeExact` pins the
/// cursor's exact `(start, stop, step)`, because a copy that folded a specific
/// trip count is wrong for any other. Reading the live cursor is what makes the
/// fold sound where the old compile-time `pass_linear_loop_fold` was not — the
/// optimizer's trace of `LoadGlobal range` + `LoadConst`s + `Call` + `GetIter`
/// only *proposes* a triple, and a rebound `range`, an aliased iterable, or a
/// partly consumed cursor simply fails the guard and runs the original loop.
/// The region's own back-edge routes through the guard chain too, and a stepped
/// cursor no longer sits at `start`, so a deopted loop cannot re-enter the
/// closed form mid-run.
///
/// The fold runs in `i128` and every result must land back in `i64`, which
/// keeps the folded delta exactly equal to the sum the iterated adds would have
/// produced — so a single `acc + delta` promotes to `BigInt` at the same value
/// the original loop reached by promoting mid-run. Zero-trip ranges are
/// declined outright rather than folded to an empty copy, leaving the ordinary
/// copy to bind nothing exactly as the original does.
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
/// - Every deferred sync is replayed by a stub whose source register still
///   holds the value the original published there.  A region that publishes two
///   names from one register, or overwrites a sync source before the exit, is
///   declined rather than deferred.
/// - A once-entered header copy's exit edge bypasses the sync stubs: on a
///   zero-trip entry no body instruction has executed, and the original would
///   not have synced either.  The back-edge fall-through goes through a stub —
///   and so does the header exit of a region whose `continue` edge re-enters
///   the header, because there the header is also the exhaustion test for every
///   iteration that took that edge.
/// - Regions containing another back-edge (nested loops), `Yield`, calls, or
///   any instruction outside the whitelist are rejected; outer loops simply
///   keep their original form while their inner loop is versioned.  A `len`
///   header's own call triple is the single exception, and only because the
///   copy replaces it with a guarded native read rather than executing it.
/// - A `len` header's call scratch (`callee` and the argument slot at
///   `callee + 1`) must be register-file scratch, not a module fast-local, and
///   nothing past the header comparison may touch either: the copy rewrites
///   `callee` from the sequence each pass and never materialises the argument
///   slot at all.
struct IntLoopVersioningResult {
    insns: Vec<Insn>,
    /// Exclusive end of the source-derived main stream. Instructions after
    /// this boundary are out-of-line fast copies and must never participate in
    /// source-origin matching, even when they are structurally identical to an
    /// instruction emitted by the compiler.
    source_prefix_len: usize,
}

/// The compile-time closed form of a counted loop whose body is a proven
/// linear accumulation over a constant `range`.
///
/// Every field is derived from the `(start, stop, step)` triple in `guard`, so
/// the copy this describes is correct exactly when that guard matches at
/// runtime.  Nothing here depends on the *provenance* of the range: the
/// argument trace only proposes a triple, and the runtime guard confirms the
/// iterator really is it.
struct ClosedForm {
    /// Exact iterator state the copy is specialized for.
    guard: IntRangeExactGuard,
    /// The loop variable, and the last value the range yields into it.  The
    /// original loop leaves that value bound after a non-empty run.
    var: Reg,
    var_final: i64,
    /// `(accumulator, total delta)` in first-write order, matching the order
    /// the exit stub's deferred syncs publish them.
    acc_deltas: Vec<(Reg, i64)>,
    /// Constant-pool slots allocated once the candidate search releases the
    /// pool: `var_final` first, then one per accumulator delta.
    const_slots: Vec<u16>,
}

/// The `(start, stop, step)` produced by a `range(...)` call with visible
/// int-constant arguments in the call sequence immediately ahead of `head`.
///
/// This is a *proposal*, not a proof.  A rebound `range`, an aliased iterable,
/// or a computed argument can all make the runtime iterator disagree with the
/// triple returned here — which is precisely what the exact-bounds entry guard
/// is for.  Requiring the whole `LoadGlobal range` + argument setup + `Call` +
/// `GetIter` sequence to sit adjacent to the header, in the register layout the
/// call convention dictates, just keeps the pass from proposing triples that
/// could never match.
///
/// The setup is *interpreted* over a small int-constant environment rather than
/// assumed to be one `LoadConst` per argument, because it is not: a negated
/// literal reaches this pass as `LoadConst` + `UnaryOp(Neg)` + `Move`.
/// `pass_unary_fold` would normally collapse that to a single `LoadConst`, but
/// it declines whenever a back edge follows the pair — and a back edge always
/// does here, since the whole point of the sequence is to feed a loop header.
/// Reading only the rigid one-instruction-per-argument shape would therefore
/// leave every negative bound and every negative step permanently unfoldable.
fn traced_const_range_bounds(
    insns: &[Insn],
    consts: &[Value],
    names: &[String],
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
                    && names.get(usize::from(name_idx)).map(String::as_str) == Some("range")
        )
    })?;

    // Interpret the setup.  Anything outside this whitelist — a computed bound,
    // a call, a register defined before the window — abandons the proposal
    // rather than guessing, so the pass never emits a copy whose guard could
    // not match.
    let mut env: Vec<(Reg, i64)> = Vec::new();
    for insn in &insns[load_global + 1..call_at] {
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
            Insn::UnaryOp(dst, crate::ast::UnaryOp::Neg, src) => (
                dst,
                env.iter().find(|(reg, _)| *reg == src)?.1.checked_neg()?,
            ),
            Insn::Move(dst, src) | Insn::CopyReg(dst, src) => {
                (dst, env.iter().find(|(reg, _)| *reg == src)?.1)
            }
            _ => return None,
        };
        match env.iter_mut().find(|(reg, _)| *reg == dst) {
            Some((_, current)) => *current = value,
            None => env.push((dst, value)),
        }
    }

    let mut args = [0i64; 3];
    for (k, arg) in args.iter_mut().take(argc).enumerate() {
        *arg = env.iter().find(|(reg, _)| *reg == base + 1 + k as Reg)?.1;
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
    names: &[String],
    head: usize,
    back: usize,
    slot: u8,
) -> Option<ClosedForm> {
    let Insn::ForIter(var, _, _) = insns[head] else {
        return None;
    };
    let (start, stop, step) = traced_const_range_bounds(insns, consts, names, head, slot)?;
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

fn pass_int_loop_version(
    insns: Vec<Insn>,
    consts: &mut Vec<Value>,
    names: &[String],
    num_locals: u32,
    num_regs: &mut u32,
) -> IntLoopVersioningResult {
    const MAX_REGION: usize = 48;
    const MAX_GUARDS: usize = 8;
    /// `LoadGlobal len` + `Move` + `Call` — the instructions a `len(seq)` loop
    /// bound occupies ahead of the comparison that tests it.
    const LEN_HEADER_TRIPLE: usize = 3;

    enum CandidateKind {
        /// Inverted while loop: conditional header, conditional back-edge.
        Inverted,
        /// `for … in <iterable>`: `ForIter` header, unconditional back-edge;
        /// the guard block additionally checks the iterator slot's kind.
        ForIter { slot: u8 },
        /// `while <i> < len(<seq>):` — a header the loop re-evaluates in full
        /// on every iteration, so `pass_loop_inversion` cannot collapse its
        /// back-edge and the region opens with the call triple rather than
        /// with a comparison.
        ///
        /// The copy replaces the triple with one native length read, still
        /// executed per iteration: `len` on a canonical sequence is a field
        /// read that can neither raise nor re-enter, but its *result* is not
        /// loop-invariant — a body that appends to or pops from the sequence
        /// moves the bound, and CPython observes exactly that.  Eliding the
        /// re-read is what made the historical AST rewrite (#289) unsound.
        LenHeaderWhile {
            /// Call base the header's `LoadGlobal` writes; also the register
            /// the comparison reads the length from.
            callee: Reg,
            /// The register holding the sequence — re-read every iteration, so
            /// rebinding it mid-loop is observed, not assumed away.
            seq: Reg,
            /// Name-table slot of `"len"`, for the guard block's re-load.
            name_idx: u16,
        },
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
        /// `for … in range(<int constants>)` with a provably linear body: the
        /// copy is the loop's closed form, so it runs in constant time no
        /// matter how many iterations the range describes.  Its guard pins the
        /// exact cursor state, not just the kind.
        ClosedForm,
    }

    /// The rotated latch of a `len`-header copy: the counter increment the
    /// copy folds into the header comparison at the region end.
    struct LenLatch {
        /// Old index of the `BinOpImm(v, v, Add, imm)` the fusion consumes.
        /// A deopt out of the rotated length read resumes the original here,
        /// because the copy re-reads the length *before* incrementing.
        add: usize,
        var: Reg,
        op: BinaryOp,
        imm: i16,
        /// Whether the fused latch jumps back on a true comparison.
        jump_when_true: bool,
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
        /// Present when the body folds to a closed form; drives the
        /// `FastVariant::ClosedForm` copy at the head of the guard chain.
        closed_form: Option<ClosedForm>,
        /// Present when a `len` header's copy rotates its length read and
        /// comparison into a fused latch.
        len_latch: Option<LenLatch>,
    }

    impl Candidate {
        /// Instructions the guard block contributes ahead of the original head.
        fn guard_block_len(&self) -> usize {
            let per_variant = self.shape_guard_len()
                + self.guards.len()
                + usize::from(self.const_fuse.is_some())
                + 1;
            self.variants.len() * per_variant
        }

        /// Guard-block instructions the candidate's *shape* contributes, ahead
        /// of the per-register `JumpIfNotInt` chain.
        fn shape_guard_len(&self) -> usize {
            match self.kind {
                CandidateKind::Inverted => 0,
                CandidateKind::ForIter { .. } => 1,
                // `LoadGlobal len` + the value guard on what it produced.
                CandidateKind::LenHeaderWhile { .. } => 2,
            }
        }

        /// Index of the comparison that decides whether the loop body runs.
        /// It is the region's first instruction except for a `len` header,
        /// whose call triple precedes it.
        fn header(&self) -> usize {
            match self.kind {
                CandidateKind::LenHeaderWhile { .. } => self.head + LEN_HEADER_TRIPLE,
                _ => self.head,
            }
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
            | Insn::JumpIfIterNotIntRangeExact(_, k)
            | Insn::GetItemSeqIntOrExit(_, _, _, k)
            | Insn::JumpIfNotBuiltinLen(_, k)
            | Insn::LenSeqOrExit(_, _, k)
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

    // Whether `insn` writes register `r`.  Whitelisted region instructions
    // report their real destination; anything else is assumed to clobber.
    let writes_reg = |insn: &Insn, r: Reg| match insn {
        Insn::Jump(..)
        | Insn::JumpIfFalse(..)
        | Insn::JumpIfTrue(..)
        | Insn::CmpJumpIfFalse(..)
        | Insn::CmpJumpIfTrue(..)
        | Insn::CmpJumpIfFalseConst(..)
        | Insn::CmpJumpIfTrueConst(..)
        | Insn::SyncModuleGlobal(..) => false,
        Insn::LoadConst(dst, _)
        | Insn::Move(dst, _)
        | Insn::CopyReg(dst, _)
        | Insn::BinOp(dst, ..)
        | Insn::BinOpInPlace(dst, ..)
        | Insn::BinOpImm(dst, ..)
        | Insn::BinOpConst(dst, ..)
        | Insn::GetItem(dst, ..)
        | Insn::ForIter(dst, ..) => *dst == r,
        // Outside the region whitelist: assume it clobbers.
        _ => true,
    };

    // The `LoadGlobal len` + `Move` + `Call` triple a `while … len(seq)` header
    // opens with, as `(callee, seq, name_idx)`.
    //
    // This only *proposes* that the call computes a built-in length: the name is
    // matched textually and the value it resolves to is checked at runtime by
    // the `JumpIfNotBuiltinLen` entry guard, exactly as the closed-form copy
    // trusts `JumpIfIterNotIntRangeExact` rather than the traced `range(...)`
    // arguments.
    let len_header = |h: usize| -> Option<(Reg, Reg, u16)> {
        let Insn::LoadGlobal(callee, name_idx) = insns[h] else {
            return None;
        };
        if names.get(usize::from(name_idx)).map(String::as_str) != Some("len") {
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
            Insn::Move(dst, src) | Insn::CopyReg(dst, src) if *dst == callee + 1 => *src,
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
    };

    // ── Find candidates ────────────────────────────────────────────────────
    let mut candidates: Vec<Candidate> = Vec::new();
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
                (CandidateKind::Inverted, h, *k as usize)
            }
            Insn::ForIter(_, slot, k) if *k > 1 => {
                (CandidateKind::ForIter { slot: *slot }, h, *k as usize)
            }
            Insn::LoadGlobal(..) => {
                let hdr = h + LEN_HEADER_TRIPLE;
                // The comparison must read the length the triple produced;
                // otherwise the call's result is used for something the copy
                // does not model and eliding the call would be wrong.
                match (len_header(h), insns.get(hdr)) {
                    (
                        Some((callee, seq, name_idx)),
                        Some(Insn::CmpJumpIfFalse(a, _, b, k) | Insn::CmpJumpIfTrue(a, _, b, k)),
                    ) if *k > 1 && (*a == callee || *b == callee) => (
                        CandidateKind::LenHeaderWhile {
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
        if back >= n || back - h > MAX_REGION {
            h += 1;
            continue;
        }
        // Back-edge: an inverted while re-checks its condition at the body
        // end (the shape produced by `pass_loop_inversion`); a for-range
        // returns to its ForIter, and a `len` header to its `LoadGlobal`, with
        // an unconditional Jump.
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
            (CandidateKind::LenHeaderWhile { .. }, Insn::Jump(kb)) => {
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
        // Old indices of `GetItem`s the fast copy may run as the deopting
        // `GetItemSeqIntOrExit`.
        let mut subscripts: Vec<usize> = Vec::new();
        // `hdr` doubles as the start of everything the whitelist walk and the
        // sync analysis reason about: only a `len` header has instructions
        // ahead of it, and the copy replaces that call triple wholesale.
        let walk_start = match kind {
            // The ForIter header is not in the body whitelist; the runtime
            // iterator-slot guard owns its semantics.
            CandidateKind::ForIter { .. } => h + 1,
            CandidateKind::Inverted | CandidateKind::LenHeaderWhile { .. } => hdr,
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
                    if !cmp_op(op) || (i != hdr && i != back && *off < 0 && !continue_edge) {
                        eligible = false;
                        break;
                    }
                    has_interior_control_flow |= i != hdr && i != back;
                    guard(*a, &mut guards);
                    guard(*b, &mut guards);
                }
                Insn::CmpJumpIfFalseConst(a, op, idx, off)
                | Insn::CmpJumpIfTrueConst(a, op, idx, off) => {
                    let continue_edge = *off < 0 && (i as i64 + 1 + *off as i64) == h as i64;
                    if !cmp_op(op)
                        || !const_is_int(*idx)
                        || (i != hdr && i != back && *off < 0 && !continue_edge)
                    {
                        eligible = false;
                        break;
                    }
                    has_interior_control_flow |= i != hdr && i != back;
                    guard(*a, &mut guards);
                }
                Insn::JumpIfFalse(r, off) | Insn::JumpIfTrue(r, off) => {
                    let continue_edge = *off < 0 && (i as i64 + 1 + *off as i64) == h as i64;
                    if i != hdr && i != back && *off < 0 && !continue_edge {
                        eligible = false;
                        break;
                    }
                    has_interior_control_flow |= i != hdr && i != back;
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
                    has_interior_control_flow |= i != hdr && i != back;
                }
                Insn::GetItem(dst, obj, idx) => {
                    // Admitted through a mid-loop side exit: the fast copy runs
                    // the deopting `GetItemSeqIntOrExit` and guards its result.
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
        if let CandidateKind::LenHeaderWhile { callee, .. } = kind {
            // `callee` is the length, rewritten by the copy's opening
            // `LenSeqOrExit` before the comparison reads it — so, like a
            // `ForIter` target, its entry state is irrelevant and guarding it
            // would divert every first entry (it is unset until the original
            // `LoadGlobal` runs).  Beyond the comparison nothing in the region
            // may touch it, and nothing at all may touch the argument slot the
            // copy never materialises.
            let touches =
                |i: usize, r: Reg| insn_reads_reg(&insns[i], r) || writes_reg(&insns[i], r);
            if (hdr + 1..=back).any(|i| touches(i, callee) || touches(i, callee + 1))
                || insn_reads_reg(&insns[hdr], callee + 1)
                || syncs.iter().any(|&(r, _)| r == callee || r == callee + 1)
            {
                eligible = false;
            }
            guards.retain(|g| *g != callee);
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
        // The same reasoning generalises to any body temporary the region
        // defines before it reads: `t = i % 2` gives `t` an int-family value
        // from already-guarded inputs on every pass, so `t`'s entry state is
        // irrelevant.  Guarding it anyway is not merely redundant — a body
        // temporary is legitimately *unset* the first time the loop is
        // entered, so `JumpIfNotInt` always diverts to the original stream and
        // the fast copy is never executed at all.
        //
        // Only the whitelisted int-family definitions count.  `GetItem` and
        // the `ForIter` target produce a value of unproven type and keep the
        // per-iteration side exits installed above.
        let defines_int_family = |insn: &Insn, g: Reg| {
            matches!(
                insn,
                Insn::LoadConst(dst, _)
                    | Insn::Move(dst, _)
                    | Insn::CopyReg(dst, _)
                    | Insn::BinOp(dst, ..)
                    | Insn::BinOpInPlace(dst, ..)
                    | Insn::BinOpImm(dst, ..)
                    | Insn::BinOpConst(dst, ..)
                if *dst == g
            )
        };
        let entry_state_is_dead = |g: Reg| {
            for d in walk_start..=back {
                if insn_reads_reg(&insns[d], g) {
                    return false;
                }
                if defines_int_family(&insns[d], g) {
                    // Every branch ahead of the definition must land at or
                    // before it, or leave the region entirely; otherwise some
                    // path reaches a later read without running the definition.
                    // An edge out of the region is harmless: the copy performs
                    // no further operation on `g` after taking it.
                    return (walk_start..d)
                        .all(|b| targets(b, &insns[b]).is_none_or(|t| t <= d || t > back));
                }
            }
            false
        };
        guards.retain(|g| !entry_state_is_dead(*g));
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
        // Deferring a `SyncModuleGlobal` to an exit stub republishes what the
        // original published only while the source register still *holds* that
        // value when the stub runs.  A module-scope `name = <expr>` reaches
        // this pass as `BinOpConst(t, …)` + `Move(local, t)` +
        // `SyncModuleGlobal(t, …)` over a scratch register `t` that the next
        // expression immediately reuses, so a single register is routinely the
        // source of several names and is overwritten between its sync and the
        // loop exit.  Flushing that at the stub publishes the register's last
        // value under every name it ever synced — and on a first entry binds
        // names the original loop never bound at all.
        //
        // A source is safe to defer exactly when its value at every exit is
        // what its own last executed sync published: it feeds one name, and
        // every write to it reaches that sync before any other write to it and
        // without passing a branch that could skip it.  A source the region
        // never writes is trivially safe — the stub rewrites the entry value
        // the original republished every iteration.
        let write_reaches_its_sync = |r: Reg, k: usize| {
            for s in k + 1..=back {
                if matches!(insns[s], Insn::SyncModuleGlobal(r2, _) if r2 == r) {
                    return true;
                }
                if writes_reg(&insns[s], r) || targets(s, &insns[s]).is_some() {
                    return false;
                }
            }
            false
        };
        // Scanned from `hdr`: a `len` header's call triple writes only its own
        // call scratch, which the eligibility walk already forbids as a sync
        // source, and `writes_reg` would otherwise read the `Call` through its
        // conservative "assume it clobbers" arm and decline every candidate.
        let sync_sources_republish_exactly = || {
            syncs.iter().all(|&(r, name_idx)| {
                (hdr..=back).all(|s| {
                    !matches!(insns[s], Insn::SyncModuleGlobal(r2, n2) if r2 == r && n2 != name_idx)
                }) && (hdr..=back)
                    .all(|k| !writes_reg(&insns[k], r) || write_reaches_its_sync(r, k))
            })
        };
        if !eligible
            || guards.len() > MAX_GUARDS
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
        // A `len` header's back-edge is an unconditional `Jump`, so there is no
        // compare at the region end to fuse the counter increment into.  The
        // copy makes one: it rotates the length read and the header comparison
        // down to the latch, leaving the entry pair as the zero-trip test.  Per
        // iteration that replaces `add` + `jump` + `length` + `compare` with a
        // `length` + one fused `CountCmpJump*`.
        //
        // Reading the length *above* the increment is unobservable — the two
        // touch disjoint registers and neither can raise or re-enter — provided
        // the deopt out of the rotated read resumes the original *at* the
        // increment, which has not run yet.  That is the side-exit target below.
        let len_latch = if let CandidateKind::LenHeaderWhile { callee, seq, .. } = kind
            && let Some(add) = prev_non_sync
            && let Insn::BinOpImm(dst, src, BinaryOp::Add, imm, _) = insns[add]
            && dst == src
            && dst != seq
            && let Insn::CmpJumpIfFalse(a, op, b, _) | Insn::CmpJumpIfTrue(a, op, b, _) = insns[hdr]
            && a == dst
            && b == callee
            && !has_fusion_interior_landing
        {
            Some(LenLatch {
                add,
                var: dst,
                op,
                imm,
                // The header jumps *out* when its test fails, so the latch
                // jumps *back* on the opposite polarity.
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
        let mut variants = match kind {
            CandidateKind::Inverted | CandidateKind::LenHeaderWhile { .. } => {
                vec![FastVariant::Plain]
            }
            CandidateKind::ForIter { .. } if has_interior_control_flow => {
                vec![FastVariant::IntRange]
            }
            CandidateKind::ForIter { .. } => {
                vec![FastVariant::IntRange, FastVariant::IndexedSeq]
            }
        };
        // A closed form subsumes the per-iteration copies, so its guard leads
        // the chain; the others stay behind it as the fallback for every
        // iterator state it does not pin.
        let closed_form = match kind {
            CandidateKind::ForIter { slot } => {
                closed_form_for(&insns, consts, names, h, back, slot)
            }
            CandidateKind::Inverted | CandidateKind::LenHeaderWhile { .. } => None,
        };
        if closed_form.is_some() {
            variants.insert(0, FastVariant::ClosedForm);
        }
        candidates.push(Candidate {
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

    if candidates.is_empty() {
        return IntLoopVersioningResult {
            insns,
            source_prefix_len: n,
        };
    }

    // Materialise the closed forms' constants now that the candidate search has
    // released the pool.  A pool that cannot address another entry simply loses
    // the closed form; its candidate keeps the per-iteration copies behind it.
    for cand in &mut candidates {
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
            cand.variants.retain(|v| *v != FastVariant::ClosedForm);
        }
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
                            FastVariant::ClosedForm => {
                                let closed_form = cand
                                    .closed_form
                                    .as_ref()
                                    .expect("a ClosedForm variant carries its plan");
                                Insn::JumpIfIterNotIntRangeExact(
                                    Box::new(closed_form.guard.clone()),
                                    fail_off,
                                )
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
                    if let CandidateKind::LenHeaderWhile {
                        callee, name_idx, ..
                    } = cand.kind
                    {
                        // Resolve the name once per entry and guard the value it
                        // produced.  Loading it here is exactly the header's own
                        // first instruction — which the deopt edge re-runs — so
                        // it observes the same binding the original would, and a
                        // `def len` shadow or a `builtins.len` patch fails the
                        // guard and runs the real call.
                        //
                        // Emitted *after* the register chain: the back-edge of a
                        // loop that keeps failing a `JumpIfNotInt` re-enters this
                        // block every iteration, and resolving the name first
                        // made each of those passes pay a global lookup whose
                        // result the very next guard discarded.  Nothing in the
                        // chain reads `callee` — the eligibility walk excludes it
                        // — so the order is free, and the failure edge now leaves
                        // `callee` for the original `LoadGlobal` to set, exactly
                        // as it does when the value guard itself fails.
                        out.push(Insn::LoadGlobal(callee, name_idx));
                        let gpos = out.len();
                        let fail_off = placement[i] as i64 - gpos as i64 - 1;
                        out.push(Insn::JumpIfNotBuiltinLen(callee, fail_off as i32));
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
        if variant == FastVariant::ClosedForm {
            // The whole loop, evaluated at compile time: bind the loop variable
            // to the range's last value and settle each accumulator with a
            // single add.  There is no back-edge and no exit jump — the copy
            // falls straight into the deferred-sync stub for `back + 1`, which
            // is always emitted first.
            let closed_form = cand
                .closed_form
                .as_ref()
                .expect("a ClosedForm variant carries its plan");
            let (&var_slot, delta_slots) = closed_form
                .const_slots
                .split_first()
                .expect("a retained closed form allocated its constant slots");
            fast_index[0] = 0;
            fast.push(FastEntry::Copied(
                cand.head,
                Insn::LoadConst(closed_form.var, var_slot),
            ));
            for (&(acc, _), &delta_slot) in closed_form.acc_deltas.iter().zip(delta_slots) {
                fast.push(FastEntry::Copied(
                    cand.head,
                    Insn::BinOpConst(acc, acc, BinaryOp::Add, delta_slot, true),
                ));
            }
        } else {
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
                    _ if i < cand.header() => {
                        // The `LoadGlobal len` + `Move` + `Call` triple becomes
                        // a single native length read.  It stays *inside* the
                        // loop — the back-edge returns here — so a body that
                        // appends to or pops from the sequence moves the bound
                        // on the next iteration exactly as the call did.  Any
                        // non-canonical receiver side-exits to the original
                        // `LoadGlobal`, which owns the protocol dispatch, the
                        // raise, and the diagnostics.
                        let CandidateKind::LenHeaderWhile { callee, seq, .. } = cand.kind else {
                            unreachable!(
                                "only a len header has instructions before its comparison"
                            );
                        };
                        if i == cand.head {
                            fast.push(FastEntry::SideExit(
                                cand.head,
                                Insn::LenSeqOrExit(callee, seq, 0),
                            ));
                        }
                    }
                    Insn::BinOpImm(..)
                        if cand.len_latch.as_ref().is_some_and(|latch| latch.add == i) =>
                    {
                        // Rotated latch: re-read the length, then run the
                        // counter increment and the header comparison as one
                        // fused back-edge.  Only syncs separate this add from
                        // the region's unconditional back-edge, so the copy
                        // ends here and the fall-through is the loop exit —
                        // the same stub the copied header exits to.
                        let latch = cand
                            .len_latch
                            .as_ref()
                            .expect("the arm guard proved the latch is present");
                        let CandidateKind::LenHeaderWhile { callee, seq, .. } = cand.kind else {
                            unreachable!("only a len header carries a rotated latch");
                        };
                        fast.push(FastEntry::SideExit(i, Insn::LenSeqOrExit(callee, seq, 0)));
                        // The fused latch is attributed to the back-edge index,
                        // and its offset names the body top so the generic
                        // rewrite maps it inside the copy rather than to the
                        // region head the original `Jump` targets.
                        let to_body_top = (cand.header() as i64 - cand.back as i64) as i32;
                        fast_index[cand.back - cand.head] = fast.len();
                        fast.push(FastEntry::Copied(
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
                        // One instruction covers both per-iteration facts: the
                        // operands are a canonical sequence read, and the
                        // element that comes out is an int.  They share this
                        // subscript as their deopt target — re-running the
                        // original reads the same element — and a region
                        // carrying a subscript admits no interior branch, so
                        // nothing can land between the read and its check.
                        fast.push(FastEntry::SideExit(
                            i,
                            Insn::GetItemSeqIntOrExit(*dst, *obj, *idx, 0),
                        ));
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
        // An inverted-while header normally runs once, on entry — but a
        // `continue` edge jumps back to it, which makes it the recurring
        // exhaustion test for every iteration that takes that edge.  Its exit
        // is then a real loop exit and has to flush the deferred syncs like any
        // other, or a loop whose last iteration `continue`d leaves the live
        // namespace at its pre-loop values.
        let header_is_reentered =
            (cand.head + 1..=cand.back).any(|k| targets(k, &insns[k]) == Some(cand.head));
        let header_exit_skips_syncs =
            matches!(cand.kind, CandidateKind::Inverted) && !header_is_reentered;
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
                    // A once-only inverted-while header's exit edge is the
                    // zero-trip path and must not flush syncs the original never
                    // executed.  A `for` header — and a header a `continue`
                    // re-enters — is the recurring exhaustion test, so its exit
                    // flushes like any body exit (a zero-trip pass is safe
                    // there: never-assigned registers are unset and the sync
                    // skips them, and already-synced names rewrite identical
                    // values).
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
            let directly_targets_past_end =
                targets(*old_i, insn) == Some(n) && fpos == 0 && header_exit_skips_syncs;
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
                    } else if fpos == 0 && header_exit_skips_syncs {
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
            // A side exit resumes the *original* instruction it deopted from,
            // so an in-region target lands on `placement`, not `jump_target`.
            // Only the region head differs between the two, and there the
            // difference matters: `jump_target` re-enters the guard block,
            // which would pass its entry guards again and jump straight back
            // into the copy that just deopted.
            let dest = if (cand.head..=cand.back).contains(&t) {
                placement[t]
            } else {
                jump_target[t]
            };
            out.push(Insn::Jump((dest as i64 - abs as i64 - 1) as i32));
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
        JumpIfIterNotIntRangeExact(guard, _) => JumpIfIterNotIntRangeExact(guard, off),
        GetItemSeqIntOrExit(dst, obj, idx, _) => GetItemSeqIntOrExit(dst, obj, idx, off),
        JumpIfNotBuiltinLen(r, _) => JumpIfNotBuiltinLen(r, off),
        LenSeqOrExit(dst, seq, _) => LenSeqOrExit(dst, seq, off),
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
