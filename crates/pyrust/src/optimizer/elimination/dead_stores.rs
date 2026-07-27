/// Remove writes to temp registers whose stored value is never read before
/// the next write to the same register.
///
/// ## Safety restrictions
///
/// - Only temp registers (`>= num_locals`) are considered; named locals may
///   escape via closures.
/// - Only *unconditionally pure* instructions are removed: `LoadConst`,
///   `LoadNone`, `Move`, and `CopyReg`.  Instructions that can raise exceptions
///   (`LoadGlobal` → NameError; `BinOp`/`BinOpConst` → ValueError /
///   ZeroDivisionError / etc.; `UnaryOp` → TypeError) are always preserved so
///   that expression statements like `a << b` or `undefined_name` still
///   propagate their errors instead of being silently dropped.
/// - Calls are never removed. Even a compiler-classified pure name can be
///   rebound through an explicit/shared namespace mirror without a bytecode
///   write to the named register. A dead-result call must therefore remain
///   observable unless a future optimizer carries a runtime binding-identity
///   guard.
/// - A back-edge guard (`slice_has_back_edge`) prevents removing a store that
///   is the initial value consumed by a later loop iteration.
fn pass_dead_store_elim(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    let n = insns.len();
    let mut keep = vec![true; n];

    // Pre-scan: record every temp register (>= num_locals) that is read at least
    // once anywhere in the function, and the highest register index seen (used to
    // size the per-register liveness arrays below).  A temp that is read nowhere
    // is provably dead everywhere — it cannot be live on a loop back-edge because
    // temps are single-assignment (by the compiler) and are never shared across
    // iterations or external frames. This lets us remove dead temp stores even
    // when a later, unrelated loop leaves a back-edge visible after them.
    let mut read_anywhere: HashSet<u32> = HashSet::new();
    let mut max_reg: u32 = num_locals;
    {
        let mut reads_buf: HashSet<u32> = HashSet::new();
        for insn in &insns {
            reads_buf.clear();
            collect_reads(insn, &mut reads_buf);
            for &r in &reads_buf {
                max_reg = max_reg.max(r);
                if r >= num_locals {
                    read_anywhere.insert(r);
                }
            }
            if let Some(w) = writable_dst(insn) {
                max_reg = max_reg.max(w);
            }
            match insn {
                Insn::LoadConst(d, _) | Insn::Move(d, _) => max_reg = max_reg.max(*d),
                _ => {}
            }
        }
    }

    // Suffix back-edge map: `back_edge_after[i]` is true iff any instruction at
    // index `> i` is a backward branch.  Computing this once turns the per-store
    // `slice_has_back_edge(&insns[i + 1..])` lookup (an O(n) tail scan) into an
    // O(1) array read, so this pass stays linear instead of O(n²) on long
    // single-block instruction streams (issue #2002).
    let mut back_edge_after = vec![false; n + 1];
    for i in (0..n).rev() {
        back_edge_after[i] = back_edge_after[i + 1] || insn_is_back_edge(&insns[i]);
    }

    // The dead-store decision for a store at index `i` to register `r` mirrors
    // `reg_is_read_before_next_write(&insns[i + 1..], r)`, which returns at the
    // first instruction `j > i` matching, in priority order: (1) reads r → true,
    // (2) terminator → false, (3) control-flow → true, (4) kills r → false; with
    // a read taking priority over a kill/control-flow at the same instruction.
    //
    // A single reverse pass computes everything needed in O(1) per store: the
    // nearest control-flow/terminator at-or-after each index (register-
    // independent) plus, per register, the nearest upcoming read and kill.  This
    // replaces the original per-store O(n) tail scan that made the pass O(n²) on
    // long single blocks (a large literal whose elements are all read once by a
    // single trailing `BuildList`/`BuildDict` — issue #2004).
    // `n` sentinel = "no such position".
    let reg_slots = (max_reg as usize) + 1;
    let mut next_read = vec![n; reg_slots];
    let mut next_kill = vec![n; reg_slots];
    // Nearest control-flow/terminator strictly after the current scan position,
    // updated as we walk backwards; `cf_pos == n` means none.
    let mut cf_pos = n;
    let mut cf_is_cf = false;

    let mut reads_buf: HashSet<u32> = HashSet::new();
    for i in (0..n).rev() {
        // At index `i`, `next_read`/`next_kill`/`cf_*` describe positions `> i`,
        // i.e. exactly the slice `&insns[i + 1..]` that the original scan walked.
        let dst = match &insns[i] {
            Insn::LoadConst(r, _) | Insn::LoadNone(r) | Insn::Move(r, _) | Insn::CopyReg(r, _)
                if *r >= num_locals =>
            {
                Some(*r)
            }
            _ => None,
        };
        if let Some(r) = dst {
            // Fast path: register read nowhere in the function ⇒ provably dead.
            if !read_anywhere.contains(&r) {
                keep[i] = false;
            } else if back_edge_after[i + 1] {
                // A back-edge could carry the value into the next iteration.
            } else {
                let read = next_read[r as usize];
                let kill = next_kill[r as usize];
                let dead = if read < cf_pos && read <= kill {
                    false // read first (ties to read) ⇒ value is live
                } else if kill < cf_pos && kill < read {
                    true // killed before any read / control-flow ⇒ dead
                } else if read == cf_pos && cf_pos < n {
                    false // a read on the control-flow/terminator wins ⇒ live
                } else if cf_pos == n {
                    true // fell off the end with no read ⇒ dead
                } else {
                    // Decided by the control-flow instruction: control-flow ⇒
                    // conservatively live; terminator ⇒ dead.
                    !cf_is_cf
                };
                if dead {
                    keep[i] = false;
                }
            }
        }

        // Fold instruction `i` into the running state for the next (lower) index.
        // Reads first so a same-instruction read+kill records the read position
        // (matching the original scan's read-before-write priority).
        reads_buf.clear();
        collect_reads(&insns[i], &mut reads_buf);
        for &r in &reads_buf {
            next_read[r as usize] = i;
        }
        if let Some(w) = writable_dst(&insns[i]) {
            next_kill[w as usize] = i;
        }
        match &insns[i] {
            Insn::LoadConst(d, _) | Insn::Move(d, _) => next_kill[*d as usize] = i,
            _ => {}
        }
        if is_terminator(&insns[i]) {
            cf_pos = i;
            cf_is_cf = false;
        } else if is_control_flow(&insns[i]) {
            cf_pos = i;
            cf_is_cf = true;
        }
    }

    compact(insns, &keep)
}
