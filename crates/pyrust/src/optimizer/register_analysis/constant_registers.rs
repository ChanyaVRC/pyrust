// ─── Constant-register propagation into CmpJump/BinOp ────────────────────────

/// Convert `BinOp`, `BinOpInPlace`, and `CmpJump*` instructions that use a
/// write-once `LoadConst` register on the RHS to their `*Const`/`*Imm` variants.
///
/// ## Pattern
///
/// ```text
/// LoadConst(r_n, idx)          ← written exactly once
/// ...
/// BinOp(dst, lhs, op, r_n)       → BinOpConst(dst, lhs, op, idx)
/// BinOpInPlace(dst, lhs, op, r_n) → BinOpImm(dst, lhs, op, imm)   (if const fits i16)
///                                 → BinOpConst(dst, lhs, op, idx)  (if lhs is temp)
/// CmpJumpIfFalse(lhs, op, r_n, k) → CmpJumpIfFalseConst(lhs, op, idx, k)
/// CmpJumpIfTrue(lhs, op, r_n, k)  → CmpJumpIfTrueConst(lhs, op, idx, k)
/// ```
///
/// ## Why this helps
///
/// `BinOp` looks up the RHS from the register file; `BinOpConst` indexes the
/// const pool directly.  The `*Const` VM dispatch paths also avoid one register
/// read.  Likewise, the constant comparison branches carry the pool index in
/// the instruction and avoid materializing an RHS register at dispatch time.
///
/// For `BinOpInPlace`, when the constant fits in `i16`, `BinOpImm` embeds the
/// value directly in the instruction word — eliminating both the register read
/// and the const-pool indirection.  `BinOpImm`'s slow path still calls
/// `try_inplace_op`, so `__iadd__` / `__imul__` semantics are preserved for
/// user-defined types on the LHS.
///
/// When the constant does not fit in `i16` but `lhs` is a temp register
/// (compiler-generated, never a user object with `__iadd__`), we emit
/// `BinOpConst` instead, still saving the register-file read.
///
/// ## Guard
///
/// Only **temporary** registers that are written exactly once by a `LoadConst`
/// across the entire function body are considered.  A source-level named local
/// is not immutable merely because the bytecode writes it once: an explicit or
/// shared namespace dictionary can replace that register through the runtime's
/// namespace mirror while user code is re-entered.  Temporaries are not exposed
/// through that mirror, so write-once is a valid immutability proof for them.
fn pass_const_reg_prop(insns: Vec<Insn>, num_locals: u32, consts: &[Value]) -> Vec<Insn> {
    debug_assert_no_late_stage_insns(&insns, "pass_const_reg_prop");
    // Pre-scan: count writes and record LoadConst targets.
    //
    // Use collect_writes (not writable_dst) to capture ALL write destinations,
    // including Move(r, _) and Unpack which writable_dst omits.
    //
    // IMPORTANT: reuse a single HashSet across iterations (clear instead of
    // new) to avoid N heap allocations — one per instruction.  For files with
    // hundreds of assertions this cut ~600 allocations to 1, removing the
    // compile-time overhead that caused an 88% regression on assertion-heavy
    // benchmarks.
    let mut write_count: HashMap<u32, u32> = HashMap::new();
    let mut load_const_idx: HashMap<u32, u16> = HashMap::new();
    let mut written: HashSet<u32> = HashSet::new();
    for insn in &insns {
        written.clear();
        collect_writes(insn, &mut written);
        for &dst in &written {
            *write_count.entry(dst).or_insert(0) += 1;
        }
        if let Insn::LoadConst(dst, idx) = insn {
            load_const_idx.insert(*dst, *idx);
        }
    }
    // Registers written exactly once, and that single write was a LoadConst.
    let immutable_const: HashMap<u32, u16> = load_const_idx
        .into_iter()
        .filter(|(r, _)| *r >= num_locals && write_count.get(r).copied() == Some(1))
        .collect();

    if immutable_const.is_empty() {
        return insns;
    }

    // Convert BinOp/CmpJump that use immutable_const registers.  Track which
    // temp registers were actually substituted — their LoadConst becomes a
    // dead store and must be pruned below.  `immutable_const` contains no named
    // locals because namespace mirrors can rewrite them outside the bytecode.
    let mut converted_regs: HashSet<u32> = HashSet::new();
    let mut out: Vec<Insn> = insns
        .into_iter()
        .map(|insn| match insn {
            Insn::BinOp(dst, lhs, op, rhs) => {
                if let Some(&idx) = immutable_const.get(&rhs) {
                    if rhs >= num_locals {
                        converted_regs.insert(rhs);
                    }
                    // Plain BinOp is never augmented → is_aug = false.
                    Insn::BinOpConst(dst, lhs, op, idx, false)
                } else {
                    Insn::BinOp(dst, lhs, op, rhs)
                }
            }
            Insn::CmpJumpIfFalse(lhs, op, rhs, k) => {
                if let Some(&idx) = immutable_const.get(&rhs) {
                    if rhs >= num_locals {
                        converted_regs.insert(rhs);
                    }
                    Insn::CmpJumpIfFalseConst(lhs, op, idx, k)
                } else {
                    Insn::CmpJumpIfFalse(lhs, op, rhs, k)
                }
            }
            Insn::CmpJumpIfTrue(lhs, op, rhs, k) => {
                if let Some(&idx) = immutable_const.get(&rhs) {
                    if rhs >= num_locals {
                        converted_regs.insert(rhs);
                    }
                    Insn::CmpJumpIfTrueConst(lhs, op, idx, k)
                } else {
                    Insn::CmpJumpIfTrue(lhs, op, rhs, k)
                }
            }
            Insn::BinOpInPlace(dst, lhs, op, rhs) => {
                if let Some(&idx) = immutable_const.get(&rhs) {
                    // The constant value fits in i16: embed it directly as BinOpImm.
                    // BinOpImm's slow path still invokes try_inplace_op, so __iadd__
                    // and friends are preserved for user-defined types on the LHS.
                    // BinOpInPlace is always augmented → is_aug = true.
                    if let Some(int_val) = consts[idx as usize].as_int()
                        && let Ok(imm) = i16::try_from(int_val)
                    {
                        if rhs >= num_locals {
                            converted_regs.insert(rhs);
                        }
                        return Insn::BinOpImm(dst, lhs, op, imm, true);
                    }
                    // Constant does not fit in i16.  Only safe to use BinOpConst
                    // when lhs is a temp register — temps are compiler-generated and
                    // never hold user objects that could have custom __iadd__.
                    if lhs >= num_locals {
                        if rhs >= num_locals {
                            converted_regs.insert(rhs);
                        }
                        return Insn::BinOpConst(dst, lhs, op, idx, true);
                    }
                }
                Insn::BinOpInPlace(dst, lhs, op, rhs)
            }
            other => other,
        })
        .collect();

    // Prune the LoadConst instructions that are now dead.
    //
    // `pass_dead_store_elim` cannot remove them because it conservatively bails
    // when it hits a CmpJumpConst (which is control flow) before seeing a
    // subsequent write.  Since we know exactly which registers were substituted,
    // we collect the set of ALL registers still read in `out` in a single O(n)
    // pass, then subtract to find the dead ones.
    //
    // IMPORTANT: use `compact` (not `retain`) so that all jump offsets are
    // rewritten to account for the removed instructions.  A raw `retain` would
    // leave stale offsets and produce wrong loop targets.
    if !converted_regs.is_empty() {
        // True O(n) pass: collect every register read by any instruction, then
        // filter converted_regs.  Avoids the O(n × k) loop from calling
        // insn_reads_reg once per converted_reg per instruction.
        let mut all_reads: HashSet<u32> = HashSet::new();
        for insn in &out {
            collect_reads(insn, &mut all_reads);
        }
        let dead_regs: HashSet<u32> = converted_regs
            .into_iter()
            .filter(|r| !all_reads.contains(r))
            .collect();
        if !dead_regs.is_empty() {
            let keep: Vec<bool> = out
                .iter()
                .map(|insn| !matches!(insn, Insn::LoadConst(r, _) if dead_regs.contains(r)))
                .collect();
            out = compact(out, &keep);
        }
    }

    out
}
