// ─── Guarded leaf-call inlining ────────────────────────────────────────────────

/// Insert a `CallInlineBinOp` guard before eligible two-argument leaf calls.
///
/// Python function bindings are observable through live namespace aliases, so
/// no call may be inlined on compile-time evidence alone (ARCHITECTURE.md: a
/// guarded optimizer must carry binding-identity facts).  This pass therefore
/// never removes or rewrites the call sequence; it only prefixes it with a
/// single guard instruction whose success path computes the leaf's result and
/// skips the sequence, and whose failure path falls straight into it:
///
/// ```text
/// guard: CallInlineBinOp { callee: f, dst: c, a: x, op, b: y, proto, skip }
/// i:     Move(c,   f)      ; original call sequence — the deopt path,
/// i+1:   Move(c+1, x)      ; byte-for-byte unchanged
/// i+2:   Move(c+2, y)
/// i+3:   Call/CallMemo(c, 2)
/// skip →                    ; first instruction after the call
/// ```
///
/// ## Eligibility
///
/// A prototype qualifies when its body is exactly `BinOp(d, p0, op, p1);
/// Return(d)` over its two plain positional parameter registers with
/// `op ∈ {Add, Sub, Mul}` — on int arguments that body cannot raise, invoke
/// user code, read any namespace, or observe its own frame, so eliding the
/// frame is unobservable.  The call site must be the canonical
/// `Move ×3 + Call/CallMemo(c, 2)` shape.  The proto index wired into the
/// guard comes from scanning `MakeFunction` stores; a stale guess is safe —
/// the VM compares code-object identity at runtime and deopts.
///
/// Jumps that target the sequence head are redirected to the guard (a loop
/// whose body starts at the call keeps the guard on its hot path); jumps into
/// the middle of the sequence simply run the original tail unoptimized.
fn pass_inline_leaf_binop(insns: Vec<Insn>, fn_protos: &[FnProto]) -> Vec<Insn> {
    const MAX_INLINE_SITES: usize = 64;

    // `optimize` is normally single-shot, but callers and tests are allowed to
    // pass already-optimized code back through the pipeline.  The original
    // Move×3+Call deopt sequence intentionally remains after a guard, so a
    // second run would otherwise prefix it with another identical guard.
    if fn_protos.is_empty()
        || insns
            .iter()
            .any(|insn| matches!(insn, Insn::CallInlineBinOp { .. }))
    {
        return insns;
    }
    // proto idx → (op, params_swapped) for eligible leaf bodies.
    let leaf_op = |p: &FnProto| -> Option<(BinaryOp, bool)> {
        let spec = &p.param_spec;
        let plain_two_positional = spec.names.len() == 2
            && spec.has_default.iter().all(|&d| !d)
            && spec.is_args.iter().all(|&f| !f)
            && spec.is_kwargs.iter().all(|&f| !f)
            && spec.is_keyword_only.iter().all(|&f| !f);
        if !plain_two_positional {
            return None;
        }
        let code = &p.code;
        if code.is_generator
            || code.is_coroutine
            || !code.cell_vars.is_empty()
            || !p.global_names.is_empty()
            || !p.nonlocal_names.is_empty()
        {
            return None;
        }
        match code.insns.as_slice() {
            [Insn::BinOp(d, a, op, b), Insn::Return(r)]
                if d == r && matches!(op, BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul) =>
            {
                match (*a, *b) {
                    (0, 1) => Some((*op, false)),
                    (1, 0) => Some((*op, true)),
                    _ => None,
                }
            }
            _ => None,
        }
    };
    let eligible: Vec<Option<(BinaryOp, bool)>> = fn_protos.iter().map(leaf_op).collect();
    if eligible.iter().all(Option::is_none) {
        return insns;
    }

    let n = insns.len();
    // Best-effort binding of a register to the proto whose function the
    // compiler stored there: `MakeFunction(f, p, …)` or
    // `MakeFunction(r, p, …); Move(f, r)`.  Later bindings win; the runtime
    // identity check makes a wrong guess merely a missed optimization.
    let mut reg_proto: HashMap<Reg, u16> = HashMap::new();
    for (i, insn) in insns.iter().enumerate() {
        if let Insn::MakeFunction(r, p, ..) = insn {
            if eligible.get(*p as usize).copied().flatten().is_some() {
                reg_proto.insert(*r, *p);
                if let Some(Insn::Move(f, src)) = insns.get(i + 1)
                    && src == r
                {
                    reg_proto.insert(*f, *p);
                }
            } else {
                reg_proto.remove(r);
            }
        }
    }
    if reg_proto.is_empty() {
        return insns;
    }

    // Collect sites: (seq_start, guard insn without offsets resolved).
    struct Site {
        at: usize,
        callee: Reg,
        dst: Reg,
        a: Reg,
        op: BinaryOp,
        b: Reg,
        proto: u16,
        /// Absolute old index the success path resumes at (after the call).
        resume: usize,
    }
    let mut sites: Vec<Site> = Vec::new();
    let mut i = 0usize;
    while i + 3 < n {
        // Argument loader: `Move(c+k, src)` keeps its source readable before
        // the sequence; `LoadConst(c+k, _)` only materialises inside it.
        enum ArgSrc {
            Reg(Reg),
            Window,
        }
        let arg_src = |insn: &Insn, slot: Reg| -> Option<ArgSrc> {
            match insn {
                Insn::Move(d, s) if *d == slot => Some(ArgSrc::Reg(*s)),
                Insn::LoadConst(d, _) if *d == slot => Some(ArgSrc::Window),
                _ => None,
            }
        };
        if let (Insn::Move(c0, f), w1, w2, Insn::Call(cc, 2) | Insn::CallMemo(cc, 2)) =
            (&insns[i], &insns[i + 1], &insns[i + 2], &insns[i + 3])
            && cc == c0
            && let Some(arg1) = arg_src(w1, c0 + 1)
            && let Some(arg2) = arg_src(w2, c0 + 2)
            && let Some(&proto) = reg_proto.get(f)
            && let Some((op, swapped)) = eligible[proto as usize]
        {
            let site = match (arg1, arg2) {
                // Both argument values pre-exist in registers: guard before
                // the whole sequence, reading the original sources — the hot
                // shape for calls inside loops (2 dispatches per call).
                // Reject aliases whose effective value would be changed by an
                // earlier Move on the deopt path.
                (ArgSrc::Reg(x), ArgSrc::Reg(y)) if x != *c0 && y != *c0 && y != c0 + 1 => {
                    let (a, b) = if swapped { (y, x) } else { (x, y) };
                    Site {
                        at: i,
                        callee: *f,
                        dst: *c0,
                        a,
                        op,
                        b,
                        proto,
                        resume: i + 4,
                    }
                }
                // At least one argument is materialised inside the sequence:
                // guard immediately before the Call, when the callee sits in
                // the call-base register and both arguments are loaded.  The
                // loaders run on both paths; only the frame is elided.
                _ => Site {
                    at: i + 3,
                    callee: *c0,
                    dst: *c0,
                    a: if swapped { c0 + 2 } else { c0 + 1 },
                    op,
                    b: if swapped { c0 + 1 } else { c0 + 2 },
                    proto,
                    resume: i + 4,
                },
            };
            sites.push(site);
            if sites.len() == MAX_INLINE_SITES {
                break;
            }
            i += 4;
            continue;
        }
        i += 1;
    }
    if sites.is_empty() {
        return insns;
    }

    // Placement maps: one guard inserted before each site.  Jumps targeting a
    // sequence head land on its guard (`jump_target`); everything else keeps
    // its shifted position.
    let mut placement = vec![0usize; n + 1];
    let mut jump_target = vec![0usize; n + 1];
    {
        let mut shift = 0usize;
        let mut si = 0usize;
        for i in 0..=n {
            let mut guard_start = None;
            if si < sites.len() && sites[si].at == i {
                guard_start = Some(i + shift);
                shift += 1;
                si += 1;
            }
            placement[i] = i + shift;
            jump_target[i] = guard_start.unwrap_or(i + shift);
        }
    }

    let mut out: Vec<Insn> = Vec::with_capacity(n + sites.len());
    let mut si = 0usize;
    for (i, insn) in insns.iter().enumerate() {
        if si < sites.len() && sites[si].at == i {
            let site = &sites[si];
            let gpos = out.len();
            let skip = (placement[site.resume] as i64 - gpos as i64 - 1) as i32;
            out.push(Insn::CallInlineBinOp {
                callee: site.callee,
                dst: site.dst,
                a: site.a,
                op: site.op,
                b: site.b,
                proto: site.proto,
                skip,
            });
            si += 1;
        }
        out.push(rewrite_offsets_with(
            insn.clone(),
            i,
            &placement,
            &jump_target,
        ));
    }
    out
}
