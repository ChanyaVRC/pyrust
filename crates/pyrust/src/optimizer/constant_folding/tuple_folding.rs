// ─── Constant tuple folding ────────────────────────────────────────────────────

/// Fold a sequence of `LoadConst` instructions feeding a `BuildTuple` into a
/// single `LoadConst` pointing to a pre-built tuple constant.
///
/// ## Pattern
///
/// ```text
/// LoadConst(base+0, c0)
/// LoadConst(base+1, c1)
/// ...
/// LoadConst(base+n-1, c_{n-1})
/// BuildTuple(dst, base, n)
/// ```
/// → replaced with a single `LoadConst(dst, tuple_pool_idx)` where
///   `consts[tuple_pool_idx]` is `Value::tuple([consts[c0], ..., consts[c_{n-1}]])`.
///
/// ## Guards
///
/// - Only `BuildTuple`, not `BuildList` (lists are mutable, tuples are immutable
///   constants and safe to deduplicate).
/// - `n >= 1 && n <= 16` — avoids unbounded look-back.
/// - All `base+j >= num_locals` — the element registers must be temporaries, not
///   named locals that could have been written by non-`LoadConst` instructions.
/// - `insns[i-n .. i]` are exactly `LoadConst(base+j, c_j)` for j in 0..n — the
///   look-back must be a perfect, contiguous, in-order match.
fn pass_fold_const_tuple(insns: Vec<Insn>, num_locals: u32, consts: &mut Vec<Value>) -> Vec<Insn> {
    let n = insns.len();
    let mut transformed = insns;
    let mut keep = vec![true; n];
    // None of the instructions removed/replaced by a fold may be an incoming
    // jump target.  A one-element tuple whose element is a ternary is the
    // minimal counterexample: the then arm jumps directly over the else-arm's
    // LoadConst to the shared BuildTuple.  Folding the adjacent else constant
    // there would force `(else_value,)` onto both paths.
    let mut jump_targets: HashSet<usize> = HashSet::new();
    for (idx, insn) in transformed.iter().enumerate() {
        if let Some(offset) = insn_jump_off(insn) {
            let target = idx as i64 + 1 + offset as i64;
            if target >= 0 && (target as usize) < n {
                jump_targets.insert(target as usize);
            }
        }
    }

    let mut i = 0;
    while i < n {
        if let Insn::BuildTuple(dst, base, argc) = transformed[i] {
            let argc = argc as usize;
            if (1..=16).contains(&argc)
                && i >= argc
                && !(i - argc..=i).any(|idx| jump_targets.contains(&idx))
            {
                // Check that insns[i-argc .. i] are LoadConst(base+j, c_j) for j in 0..argc
                // and that all base+j >= num_locals.
                let mut all_match = true;
                let mut c_indices: Vec<u16> = Vec::with_capacity(argc);
                for j in 0..argc {
                    let slot = i - argc + j;
                    match transformed[slot] {
                        Insn::LoadConst(reg, c_idx)
                            if reg == base + j as u32 && reg >= num_locals =>
                        {
                            c_indices.push(c_idx);
                        }
                        _ => {
                            all_match = false;
                            break;
                        }
                    }
                }

                if all_match {
                    // Build the tuple value from the pooled constants.
                    let elems: Vec<Value> = c_indices
                        .iter()
                        .map(|&ci| consts[ci as usize].clone())
                        .collect();
                    let tuple_val = Value::tuple(elems);

                    // Intern the tuple in the const pool (always append — tuples are
                    // not deduplicated by intern_const_in_pool which only handles
                    // scalars, and identity equality on tuples is object-level).
                    if consts.len() < u16::MAX as usize {
                        let new_idx =
                            u16::try_from(consts.len()).expect("constant-pool limit checked above");
                        consts.push(tuple_val);

                        // Mark the n LoadConst predecessors as removed.
                        for j in 0..argc {
                            keep[i - argc + j] = false;
                        }
                        // Replace BuildTuple with LoadConst(dst, new_idx).
                        transformed[i] = Insn::LoadConst(dst, new_idx);
                    }
                }
            }
        }
        i += 1;
    }
    compact(transformed, &keep)
}
