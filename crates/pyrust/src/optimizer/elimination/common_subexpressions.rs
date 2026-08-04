// ─── Common subexpression elimination ─────────────────────────────────────────

/// Eliminate redundant computations within each basic block.
///
/// Within a straight-line sequence of instructions (a *basic block* — no jumps
/// in or out), if two instructions compute exactly the same value from the same
/// inputs, the second one is redundant.  This pass replaces the second with
/// `CopyReg(dst2, dst1)`, pointing `dst2` at the already-computed result.
///
/// ## Tracked expressions
///
/// Only `LoadConst(dst, idx)` is tracked. Unary and binary instructions can
/// invoke user protocols even when one operand is encoded in the instruction,
/// so repeated bytecode does not imply a repeated, side-effect-free result.
///
/// A result is retained as a reusable source only when `dst` is a temporary
/// register (`dst >= num_locals`). Script named-local registers can be mutated
/// through a live globals alias without an intervening register-write opcode;
/// using one as the source of a later `CopyReg` would therefore be unsound.
///
/// ## CSE key and invalidation
///
/// The *CSE key* for a tracked instruction is `(discriminant, src_regs..., const_idx)`.
/// The map is cleared at every basic-block boundary (any branch, jump, or
/// exception instruction, as well as any instruction that is a jump *target*).
///
/// Whenever a retained temporary is overwritten, its table entry is removed.
///
/// ## Interaction with later passes
///
/// The emitted `CopyReg` instructions are subsequently cleaned up by
/// `pass_dead_store_elim` (if the original dst is never read) and
/// `pass_trivial_nop` (if dst == src, which cannot happen here but is guarded
/// for safety).  `pass_copy_prop` does *not* chase through `CopyReg` — that
/// keeps the pass order simple and avoids invalidating other CSE entries.
///
/// Reference: Aho, Lam, Sethi, Ullman *Compilers* §9.1 (available expressions);
/// Kennedy *A Survey of Data-Flow Analysis Techniques* §3 (CSE).
fn pass_cse(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    debug_assert_no_late_stage_insns(&insns, "pass_cse");
    use std::collections::HashMap;

    #[derive(Eq, PartialEq, Hash, Clone)]
    enum CseKey {
        /// `LoadConst(_, idx)` — two loads of the same pool entry.
        LoadConst(u16),
    }

    /// CSE table with a reverse output index so per-register eviction touches
    /// only the affected entries instead of scanning the whole table. A plain
    /// `HashMap::retain` per written register is O(table) and degenerates to
    /// O(n²) inside a long single basic block (e.g. a large literal whose
    /// elements each emit a fresh `LoadConst` into a new temp — issue #2004).
    ///
    /// `by_output[w]` lists keys whose retained result register is `w`. It may
    /// contain stale keys already removed from `map`; the current-output guard
    /// makes re-processing those entries a no-op.
    struct CseTable {
        map: HashMap<CseKey, u32>,
        by_output: HashMap<u32, Vec<CseKey>>,
    }

    impl CseTable {
        fn new() -> Self {
            CseTable {
                map: HashMap::new(),
                by_output: HashMap::new(),
            }
        }
        fn clear(&mut self) {
            self.map.clear();
            self.by_output.clear();
        }
        fn get(&self, k: &CseKey) -> Option<u32> {
            self.map.get(k).copied()
        }
        fn insert(&mut self, k: CseKey, dst: u32) {
            self.by_output.entry(dst).or_default().push(k.clone());
            self.map.insert(k, dst);
        }
        /// Evict every entry whose retained result register is `w`.
        fn evict_reg(&mut self, w: u32) {
            if let Some(keys) = self.by_output.remove(&w) {
                for k in keys {
                    // Only remove if this key's *current* result register is `w`.
                    // A stale index entry can survive a remove-then-reinsert that
                    // assigned the key a different output register; removing it
                    // here would wrongly evict an entry whose output is not `w`.
                    if self.map.get(&k) == Some(&w) {
                        self.map.remove(&k);
                    }
                }
            }
        }
        /// Evict every entry whose result register falls in `lo..hi`.
        fn evict_range(&mut self, lo: u32, hi: u32) {
            for w in lo..hi {
                self.evict_reg(w);
            }
        }
    }

    let n = insns.len();
    if n == 0 {
        return insns;
    }

    // Pre-pass: mark every instruction that is a jump target so we can clear
    // the CSE table at basic-block boundaries.
    let mut is_bb_start = vec![false; n + 1];
    is_bb_start[0] = true;
    for (i, insn) in insns.iter().enumerate() {
        let k: Option<i32> = match insn {
            Insn::Jump(k)
            | Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => Some(*k),
            _ => None,
        };
        if let Some(k) = k {
            let target = (i as i64 + 1 + k as i64) as usize;
            if target <= n {
                is_bb_start[target] = true;
            }
        }
    }

    // `table`: CSE key → (original dst register that holds the result).
    let mut table = CseTable::new();
    let mut result: Vec<Insn> = Vec::with_capacity(n);

    for (i, insn) in insns.into_iter().enumerate() {
        // Clear CSE state at basic-block boundaries.
        if is_bb_start[i] {
            table.clear();
        }

        // Build the CSE key for this instruction, if it is the tracked pure form.
        let key: Option<(CseKey, u32)> = match &insn {
            Insn::LoadConst(dst, idx) => Some((CseKey::LoadConst(*idx), *dst)),
            _ => None,
        };

        // Determine which register (if any) this instruction writes to, so we
        // can evict stale CSE table entries BEFORE the match check.  Eviction
        // must happen regardless of whether the instruction is later replaced
        // by a CopyReg, because the CopyReg itself still writes to `dst`.
        let written_reg: Option<u32> = match &insn {
            Insn::LoadConst(r, _)
            | Insn::LoadNone(r)
            | Insn::LoadGlobal(r, _)
            | Insn::LoadClassName(r, _, _)
            | Insn::LoadCell(r, _) => Some(*r),
            // Move writes its destination register; must evict stale CSE entries
            // that recorded `prev_dst == dst` from an earlier computation.
            Insn::Move(dst, _) => Some(*dst),
            Insn::Unpack(base, _, n) => {
                // Handled separately below; use sentinel None here.
                let _ = (base, n);
                None
            }
            _ => writable_dst(&insn),
        };

        // Evict stale entries whose retained output register is overwritten.
        // Do this before the match check so a new entry is not immediately
        // invalidated by its own write.
        if let Insn::Unpack(base, _, n) = &insn {
            table.evict_range(*base, base + n);
        } else if let Some(w) = written_reg {
            table.evict_reg(w);
        }
        // UnpackEx writes dst_base..dst_base+before+1+after.  writable_dst
        // returns None for it (multi-register write), so evict the full range
        // explicitly — mirrors the Unpack handling above.
        // Without this, a LoadConst that was emitted for a list element earlier
        // in the same basic block can survive into the CSE table, causing a
        // later LoadConst for the same constant value to be replaced by a
        // CopyReg pointing at the now-overwritten temp register (issue #1358).
        if let Insn::UnpackEx {
            dst_base,
            before,
            after,
            ..
        } = &insn
        {
            let lo = *dst_base;
            let hi = dst_base + *before as u32 + 1 + *after;
            table.evict_range(lo, hi);
        }
        // YieldFrom writes both result_reg and sent_reg on resume.  Neither
        // register is in writable_dst (which is single-register), so evict
        // both explicitly here, mirroring the Unpack pattern above.
        if let Insn::YieldFrom {
            result_reg,
            sent_reg,
            ..
        } = &insn
        {
            table.evict_reg(*result_reg);
            table.evict_reg(*sent_reg);
        }

        // Check for a previous matching computation.
        let replaced = if let Some((ref k, dst)) = key {
            if let Some(prev_dst) = table.get(k) {
                if prev_dst != dst {
                    // Replace this instruction with a register copy from the
                    // earlier result.  The original instruction is discarded.
                    result.push(Insn::CopyReg(dst, prev_dst));
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        if !replaced {
            // Record the expression in the CSE table.  Eviction already happened
            // above before the match check, so the new entry will not be removed.
            if let Some((k, dst)) = key
                && dst >= num_locals
            {
                table.insert(k, dst);
            }

            result.push(insn);
        }

        // After a basic-block-terminating instruction, clear the table so the
        // next block starts fresh.  (We also clear at the *start* of targets via
        // is_bb_start, but this handles the fall-through path of conditionals.)
        let is_terminator = matches!(
            result.last().unwrap(),
            Insn::Jump(_)
                | Insn::JumpIfFalse(..)
                | Insn::JumpIfTrue(..)
                | Insn::CmpJumpIfFalse(..)
                | Insn::CmpJumpIfTrue(..)
                | Insn::CmpJumpIfFalseConst(..)
                | Insn::CmpJumpIfTrueConst(..)
                | Insn::ForIter(..)
                | Insn::SetupExcept(_)
                | Insn::MatchExcept(..)
                | Insn::MatchExceptStar(..)
                | Insn::Return(_)
                | Insn::ReturnNone
                | Insn::RaiseValue(_)
                | Insn::RaiseExceptStarResidual(_)
                | Insn::RaiseFrom(..)
                | Insn::RaiseReRaise
                | Insn::RaiseAssert(_)
                | Insn::RaiseAssertNoMsg
        );
        if is_terminator {
            table.clear();
        }
    }

    result
}
