// ─── LoadNone merging ────────────────────────────────────────────────────────

/// Merge runs of consecutive `LoadNone(r), LoadNone(r+1), ..., LoadNone(r+N-1)` into
/// a single `LoadNoneRange { start: r, count: N }` instruction.
///
/// Only contiguous ascending register sequences are merged, AND the run must be
/// free of any jump targets in its interior — a jump to the middle of a run
/// cannot be redirected to the first instruction without changing semantics.
/// A run of 1 is left as a bare `LoadNone` (no gain from wrapping).  If a run
/// exceeds 255 registers, it is split into multiple `LoadNoneRange` instructions.
///
/// ## Why this helps
///
/// Function entry emits one `LoadNone` per local variable.  A function with N
/// locals emits N separate VM dispatches through the giant `match insn`.
/// `LoadNoneRange` collapses those N dispatches into one tight `for` loop that
/// Rust/LLVM can lower to a memset-style fill.
///
/// ## Jump-offset correctness
///
/// When the pass merges a run of K instructions into one, it uses the `compact`
/// helper to rewrite all subsequent jump offsets automatically.  The `keep`
/// vector marks which instructions to retain: the first instruction in a run is
/// replaced with a `LoadNoneRange`; the rest are marked `keep = false`.
fn pass_loadnone_merge(insns: Vec<Insn>) -> Vec<Insn> {
    let n = insns.len();
    if n == 0 {
        return insns;
    }

    // Pre-pass: collect all instruction indices that are jump targets.
    // An instruction that is a jump target cannot be swallowed into the middle of
    // a LoadNoneRange (the range would be misinterpreted as starting earlier).
    let mut jump_targets: HashSet<usize> = HashSet::new();
    for (i, insn) in insns.iter().enumerate() {
        let k: Option<i32> = match insn {
            Insn::Jump(k) => Some(*k),
            Insn::JumpIfFalse(_, k)
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
            let target_signed = i as i64 + 1 + k as i64;
            if target_signed >= 0 && (target_signed as usize) < n {
                jump_targets.insert(target_signed as usize);
            }
        }
    }

    let mut transformed = insns;
    let mut keep = vec![true; n];

    let mut i = 0;
    while i < n {
        if let Insn::LoadNone(start) = transformed[i] {
            // Extend the run as long as:
            // 1. The next instruction is LoadNone(run_end + 1) (contiguous ascending).
            // 2. The next instruction is NOT a jump target (cannot be subsumed).
            let mut run_end = start;
            let mut j = i + 1;
            while j < n && !jump_targets.contains(&j) {
                if let Insn::LoadNone(r) = transformed[j]
                    && r == run_end + 1
                {
                    run_end = r;
                    j += 1;
                    continue;
                }
                break;
            }

            let count = (run_end - start + 1) as usize; // >= 1
            if count == 1 {
                // Single LoadNone — no merge beneficial; leave as-is.
                i += 1;
                continue;
            }

            // Replace the first instruction with LoadNoneRange(s) and mark the
            // rest of the run for removal via `compact`.
            // If count > 255, split into batches; each batch occupies one slot
            // from the run (the first, then second, …).
            let mut base = start;
            let mut remaining = count;
            let mut slot = i; // which position in the run gets the next instruction
            while remaining > 0 {
                let batch = remaining.min(u8::MAX as usize) as u8;
                transformed[slot] = Insn::LoadNoneRange {
                    start: base,
                    count: batch,
                };
                base += batch as u32;
                remaining -= batch as usize;
                slot += 1;
            }
            // Mark the tail of the run (slot..j) as dead.
            for dead in slot..j {
                keep[dead] = false;
            }
            i = j;
        } else {
            i += 1;
        }
    }

    compact(transformed, &keep)
}
