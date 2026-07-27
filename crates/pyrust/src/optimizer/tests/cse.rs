use super::*;

#[test]
fn cse_duplicate_loadconst_becomes_copyreg() {
    // Two loads of the same constant index → second becomes CopyReg.
    // LoadConst(r2, 0)  LoadConst(r3, 0)  Return(r2)
    // Expected: LoadConst(r2, 0)  CopyReg(r3, r2)  Return(r2)
    let insns = vec![
        Insn::LoadConst(2, 0),
        Insn::LoadConst(3, 0), // duplicate
        Insn::Return(2),
    ];
    let out = pass_cse(insns, 0);
    assert_eq!(out.len(), 3, "instruction count unchanged");
    assert!(
        matches!(out[0], Insn::LoadConst(2, 0)),
        "first LoadConst must be kept"
    );
    assert!(
        matches!(out[1], Insn::CopyReg(3, 2)),
        "second LoadConst should become CopyReg(r3, r2)"
    );
}

#[test]
fn cse_does_not_deduplicate_user_binary_protocols() {
    use crate::ast::BinaryOp;
    // The encoded constant does not make the operation pure: r0 can dispatch
    // a stateful __add__/__radd__ protocol on every evaluation.
    let insns = vec![
        Insn::BinOpConst(4, 0, BinaryOp::Add, 1, false),
        Insn::BinOpConst(5, 0, BinaryOp::Add, 1, false),
        Insn::Return(4),
    ];
    let out = pass_cse(insns, 0);
    assert_eq!(out.len(), 3);
    assert!(
        matches!(out[0], Insn::BinOpConst(4, 0, BinaryOp::Add, 1, ..)),
        "first BinOpConst must be kept"
    );
    assert!(
        matches!(out[1], Insn::BinOpConst(5, 0, BinaryOp::Add, 1, ..)),
        "a repeated user-dispatching BinOpConst must still execute"
    );
}

#[test]
fn cse_intervening_write_invalidates_entry() {
    use crate::ast::BinaryOp;
    // BinOpConst(r4, r0, Add, 1)
    // LoadConst(r0, 2)        ← writes r0, the input of the BinOpConst
    // BinOpConst(r5, r0, Add, 1)   ← r0 is now a different value; NOT a duplicate
    let insns = vec![
        Insn::BinOpConst(4, 0, BinaryOp::Add, 1, false),
        Insn::LoadConst(0, 2), // clobbers r0
        Insn::BinOpConst(5, 0, BinaryOp::Add, 1, false),
        Insn::Return(4),
    ];
    let out = pass_cse(insns, 0);
    assert_eq!(out.len(), 4, "no elimination when input clobbered");
    assert!(
        matches!(out[2], Insn::BinOpConst(5, 0, BinaryOp::Add, 1, ..)),
        "second BinOpConst must not be replaced after input clobber"
    );
}

#[test]
fn cse_does_not_cross_basic_block_boundary() {
    // LoadConst(r2, 0)
    // JumpIfFalse(r1, 0)   ← ends basic block; target is next instruction
    // LoadConst(r3, 0)     ← same const, but different basic block → NOT replaced
    // Return(r2)
    let insns = vec![
        Insn::LoadConst(2, 0),
        Insn::JumpIfFalse(1, 0), // offset 0 → target = idx 2
        Insn::LoadConst(3, 0),   // idx 2 is a BB start (jump target)
        Insn::Return(2),
    ];
    let out = pass_cse(insns, 0);
    // The third instruction (idx 2) is a BB start, so the CSE table is cleared
    // before it is processed; the second LoadConst(r3, 0) must NOT be replaced.
    assert!(
        matches!(out[2], Insn::LoadConst(3, 0)),
        "CSE must not cross basic-block boundary"
    );
}

#[test]
fn cse_does_not_deduplicate_user_unary_protocols() {
    use crate::ast::UnaryOp;
    let insns = vec![
        Insn::UnaryOp(4, UnaryOp::Neg, 0),
        Insn::UnaryOp(5, UnaryOp::Neg, 0),
        Insn::Return(4),
    ];
    let out = pass_cse(insns, 0);
    assert_eq!(out.len(), 3);
    assert!(
        matches!(out[0], Insn::UnaryOp(4, UnaryOp::Neg, 0)),
        "first UnaryOp must be kept"
    );
    assert!(
        matches!(out[1], Insn::UnaryOp(5, UnaryOp::Neg, 0)),
        "a repeated user-dispatching UnaryOp must still execute"
    );
}

#[test]
fn cse_does_not_publish_a_named_local_loadconst() {
    // A live globals alias can overwrite r0 without an explicit bytecode write.
    // The later equal constant must not copy from that externally reachable
    // register.
    let insns = vec![
        Insn::LoadConst(0, 0),
        Insn::Call(5, 0),
        Insn::LoadConst(1, 0),
        Insn::Return(1),
    ];
    let out = pass_cse(insns, 2);
    assert!(
        matches!(out[2], Insn::LoadConst(1, 0)),
        "named-local producers must not enter the CSE table"
    );
}

#[test]
fn cse_output_clobber_invalidates_entry() {
    // If the output register of a CSE candidate is overwritten, subsequent
    // identical computations cannot be replaced by a CopyReg pointing to it.
    //
    // LoadConst(r2, 0)        ← r2 holds consts[0]
    // LoadNone(r2)            ← clobbers r2; CSE entry for consts[0] removed
    // LoadConst(r3, 0)        ← must NOT be replaced by CopyReg(r3, r2)
    // Return(r3)
    let insns = vec![
        Insn::LoadConst(2, 0),
        Insn::LoadNone(2), // clobbers the output register r2
        Insn::LoadConst(3, 0),
        Insn::Return(3),
    ];
    let out = pass_cse(insns, 0);
    assert_eq!(out.len(), 4, "no elimination when output clobbered");
    assert!(
        matches!(out[2], Insn::LoadConst(3, 0)),
        "LoadConst must not be replaced after its output register is clobbered"
    );
}

#[test]
fn cse_does_not_match_binopconst_across_call_boundary() {
    use crate::ast::BinaryOp;
    // Simulates the pattern where a named-local register (r0) is used
    // as a source in BinOpConst before and after a Call instruction that
    // may update r0 via assign_name write-through.
    //
    // A Call writes to r9 (a temp, not r0 or r4).  The CSE table entry
    // {BinOpConst(r0, Add, 1) -> r4} must NOT survive across the Call,
    // because at runtime r0 may have been updated by the callee.
    //
    //   [0] BinOpConst(r4, r0, Add, 1)   <- first occurrence; r4 = r0 + c1
    //   [1] Call(r9, r2)                  <- may update r0 via write-through
    //   [2] BinOpConst(r5, r0, Add, 1)   <- second occurrence; must NOT be CopyReg(r5, r4)
    //   [3] Return(r5)
    // num_locals=1: r0 is a named local; r4, r5, r9 are temps.
    let insns = vec![
        Insn::BinOpConst(4, 0, BinaryOp::Add, 1, false), // r4 = r0 + c1
        Insn::Call(9, 2),                                // call; writes r9 (temp), may clobber r0
        Insn::BinOpConst(5, 0, BinaryOp::Add, 1, false), // r5 = r0 + c1 (same expr)
        Insn::Return(5),
    ];
    let out = pass_cse(insns, 1); // r0 < num_locals: named local, must not be CSE'd across call
    // The second BinOpConst MUST NOT be replaced by CopyReg(r5, r4)
    // because r0 may have been mutated by the Call.
    assert!(
        matches!(out[2], Insn::BinOpConst(5, 0, BinaryOp::Add, 1, ..)),
        "BinOpConst after Call must not be CSE-replaced when src is a named local: {:?}",
        out[2]
    );
}

#[test]
fn cse_yield_from_evicts_result_reg_as_output() {
    // If result_reg was previously the dst of a retained constant, that entry
    // must be evicted after YieldFrom writes a new value into result_reg.
    //
    //   [0] LoadConst(r3, 0)        -- CSE records {const[0] -> r3}
    //   [1] YieldFrom { iter_reg=r0, sent_reg=r2, result_reg=r3 }
    //                               -- r3 is overwritten; entry must be removed
    //   [2] LoadConst(r4, 0)        -- CopyReg(r4, r3) would be wrong
    //   [3] Return(r4)
    let insns = vec![
        Insn::LoadConst(3, 0),
        Insn::YieldFrom {
            iter_reg: 0,
            sent_reg: 2,
            result_reg: 3,
        },
        Insn::LoadConst(4, 0),
        Insn::Return(4),
    ];
    let out = pass_cse(insns, 0);
    assert_eq!(out.len(), 4, "no instruction removed");
    assert!(
        matches!(out[2], Insn::LoadConst(4, 0)),
        "LoadConst after YieldFrom must not use stale result_reg as CopyReg source: {:?}",
        out[2]
    );
}

// ── linearized-pass equivalence (issue #2004) ──────────────────────────────

/// Reference dead-store elimination that uses the original O(n) tail-scan
/// (`reg_is_read_before_next_write`).  Used to confirm the linearized
/// `pass_dead_store_elim` produces byte-identical output.
fn dead_store_elim_reference(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    let n = insns.len();
    let mut keep = vec![true; n];
    let mut global_read_count: HashMap<u32, usize> = HashMap::new();
    let mut reads_buf: HashSet<u32> = HashSet::new();
    for insn in &insns {
        reads_buf.clear();
        collect_reads(insn, &mut reads_buf);
        for &r in &reads_buf {
            if r >= num_locals {
                *global_read_count.entry(r).or_insert(0) += 1;
            }
        }
    }
    for i in 0..n {
        let dst = match &insns[i] {
            Insn::LoadConst(r, _) | Insn::LoadNone(r) | Insn::Move(r, _) | Insn::CopyReg(r, _)
                if *r >= num_locals =>
            {
                *r
            }
            // `CallMemo` remains observable even when its result is dead, so
            // dead-store elimination never drops it.
            _ => continue,
        };
        if global_read_count.get(&dst).copied().unwrap_or(0) == 0 {
            keep[i] = false;
            continue;
        }
        if slice_has_back_edge(&insns[i + 1..]) {
            continue;
        }
        if !reg_is_read_before_next_write(&insns[i + 1..], dst) {
            keep[i] = false;
        }
    }
    compact(insns, &keep)
}

/// A small library of random-ish instruction streams that exercise the
/// dead-store / CSE eviction logic: loads, moves, copies, fused ops, reads,
/// and control flow (terminators and back-edges).
fn sample_streams() -> Vec<Vec<Insn>> {
    use crate::ast::{BinaryOp, UnaryOp};
    vec![
        // Long single block of distinct LoadConsts feeding one BuildList
        // (the #2004 literal shape).
        {
            let mut v: Vec<Insn> = (0..40u16)
                .map(|i| Insn::LoadConst(2 + i as u32, i))
                .collect();
            v.push(Insn::BuildList(2, 2, 40));
            v.push(Insn::Return(2));
            v
        },
        // Move chain with a read in the middle.
        vec![
            Insn::LoadConst(2, 0),
            Insn::Move(3, 2),
            Insn::UnaryOp(4, UnaryOp::Neg, 3),
            Insn::Move(3, 4),
            Insn::Return(3),
        ],
        // Read-then-write of the same reg in one instruction (BinOp(r,r,..)).
        vec![
            Insn::LoadConst(2, 0),
            Insn::BinOpConst(2, 2, BinaryOp::Add, 1, false),
            Insn::Return(2),
        ],
        // Store with a terminator before any read.
        vec![Insn::LoadConst(2, 0), Insn::ReturnNone],
        // Store, control-flow (forward jump), then read.
        vec![Insn::LoadConst(2, 0), Insn::Jump(0), Insn::Return(2)],
        // Back-edge after a store (loop-carried).
        vec![
            Insn::LoadConst(2, 0),
            Insn::BinOpConst(2, 2, BinaryOp::Add, 1, false),
            Insn::Jump(-2),
        ],
        // CopyReg dead store (never read).
        vec![Insn::LoadConst(2, 0), Insn::CopyReg(3, 2), Insn::Return(2)],
        // Duplicate LoadConst (CSE target) with intervening write.
        vec![
            Insn::LoadConst(2, 0),
            Insn::Move(2, 3),
            Insn::LoadConst(4, 0),
            Insn::Return(4),
        ],
        // Duplicate fused op separated by an unrelated write.
        vec![
            Insn::BinOpConst(2, 0, BinaryOp::Add, 1, false),
            Insn::LoadConst(5, 7),
            Insn::BinOpConst(3, 0, BinaryOp::Add, 1, false),
            Insn::Return(3),
        ],
        // Fused op whose source is overwritten between two occurrences.
        vec![
            Insn::BinOpConst(2, 0, BinaryOp::Add, 1, false),
            Insn::Move(0, 9),
            Insn::BinOpConst(3, 0, BinaryOp::Add, 1, false),
            Insn::Return(3),
        ],
    ]
}

#[test]
fn dead_store_elim_matches_reference() {
    for stream in sample_streams() {
        for num_locals in [0u32, 2, 3] {
            let a = pass_dead_store_elim(stream.clone(), num_locals);
            let b = dead_store_elim_reference(stream.clone(), num_locals);
            assert_eq!(
                a, b,
                "dead_store_elim diverged from reference (num_locals={num_locals}) on {stream:?}"
            );
        }
    }
}

/// Reference CSE using a plain `HashMap` with full-table `retain` eviction
/// (the pre-#2004 algorithm).  Confirms the reverse-indexed `CseTable`
/// produces byte-identical output.
fn cse_reference(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    #[derive(Eq, PartialEq, Hash, Clone)]
    enum K {
        LoadConst(u16),
    }
    let n = insns.len();
    if n == 0 {
        return insns;
    }
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
    let mut table: HashMap<K, u32> = HashMap::new();
    let mut result: Vec<Insn> = Vec::with_capacity(n);
    for (i, insn) in insns.into_iter().enumerate() {
        if is_bb_start[i] {
            table.clear();
        }
        let key: Option<(K, u32)> = match &insn {
            Insn::LoadConst(dst, idx) => Some((K::LoadConst(*idx), *dst)),
            _ => None,
        };
        let written_reg: Option<u32> = match &insn {
            Insn::LoadConst(r, _) | Insn::LoadNone(r) | Insn::LoadGlobal(r, _) => Some(*r),
            Insn::Move(dst, _) => Some(*dst),
            Insn::Unpack(..) => None,
            _ => writable_dst(&insn),
        };
        let evict_range = |table: &mut HashMap<K, u32>, lo: u32, hi: u32| {
            table.retain(|_, prev_dst| *prev_dst < lo || *prev_dst >= hi);
        };
        if let Insn::Unpack(base, _, m) = &insn {
            evict_range(&mut table, *base, base + m);
        } else if let Some(w) = written_reg {
            table.retain(|_, prev_dst| *prev_dst != w);
        }
        if let Insn::UnpackEx {
            dst_base,
            before,
            after,
            ..
        } = &insn
        {
            let lo = *dst_base;
            let hi = dst_base + *before as u32 + 1 + *after;
            evict_range(&mut table, lo, hi);
        }
        if let Insn::YieldFrom {
            result_reg,
            sent_reg,
            ..
        } = &insn
        {
            let rr = *result_reg;
            let sr = *sent_reg;
            table.retain(|_, prev_dst| *prev_dst != rr && *prev_dst != sr);
        }
        let replaced = if let Some((ref k, dst)) = key {
            if let Some(&prev_dst) = table.get(k) {
                if prev_dst != dst {
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
            if let Some((k, dst)) = key
                && dst >= num_locals
            {
                table.insert(k, dst);
            }
            result.push(insn);
        }
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

#[test]
fn cse_linearized_matches_reference() {
    for stream in sample_streams() {
        for num_locals in [0u32, 1, 2, 3] {
            let a = pass_cse(stream.clone(), num_locals);
            let b = cse_reference(stream.clone(), num_locals);
            assert_eq!(
                a, b,
                "cse diverged from reference (num_locals={num_locals}) on {stream:?}"
            );
        }
    }
}

#[test]
fn cse_linearized_matches_small_cases() {
    // The dedicated cse_* tests above lock in the expected output for each
    // eviction path; re-run them through the linearized table on the literal
    // shape to confirm the indexed eviction still dedups within one block.
    let mut v: Vec<Insn> = vec![Insn::LoadConst(2, 0), Insn::LoadConst(3, 0)];
    v.push(Insn::BuildList(2, 2, 2));
    v.push(Insn::Return(2));
    let out = pass_cse(v, 0);
    // The second LoadConst(_, 0) must become CopyReg(3, 2).
    assert!(
        matches!(out[1], Insn::CopyReg(3, 2)),
        "duplicate LoadConst within a block should CSE: {:?}",
        out[1]
    );
}
