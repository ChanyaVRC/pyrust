//! Raw bytecode basic-block discovery, CFG construction, reachability
//! compaction, exception-depth propagation, and compact register bitsets.

use std::collections::HashSet;

use smallvec::SmallVec;

use crate::bytecode::{Insn, Reg};

use crate::mir::insn_info::{ends_raw_block, jump_offset};

/// Raw basic block extracted directly from `FnCode::insns` before SSA renaming.
pub(super) struct RawBlock {
    pub(super) start_pc: usize,
    /// Non-terminator instructions (half-open range `[start_pc, body_end)`).
    pub(super) body_end: usize,
    /// Index of the terminator instruction in `insns`, if the block ends with one.
    pub(super) term_pc: Option<usize>,
    /// Outgoing block edges.
    pub(super) succs: SmallVec<[RawEdge; 2]>,
    pub(super) preds: Vec<usize>,
    pub(super) exc_depth: u16,
    pub(super) is_handler: bool,
}

#[derive(Clone, Copy)]
pub(super) struct RawEdge {
    pub(super) target: usize,
    pub(super) kind: RawEdgeKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RawEdgeKind {
    Normal,
    SetupExceptHandler,
    ForIterBody,
    ExceptStarMatched,
}

/// Compact bitset over register indices `0..num_regs`.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct RegBits {
    words: Vec<u64>,
    nbits: usize,
}

impl RegBits {
    pub(super) fn empty(nbits: usize) -> Self {
        Self {
            words: vec![0; nbits.div_ceil(64)],
            nbits,
        }
    }

    pub(super) fn all(nbits: usize) -> Self {
        let mut words = vec![u64::MAX; nbits.div_ceil(64)];
        if !nbits.is_multiple_of(64)
            && let Some(last) = words.last_mut()
        {
            *last = (1u64 << (nbits % 64)) - 1;
        }
        Self { words, nbits }
    }

    #[inline]
    pub(super) fn contains(&self, r: Reg) -> bool {
        let idx = r as usize;
        idx < self.nbits && ((self.words[idx / 64] >> (idx % 64)) & 1) != 0
    }

    #[inline]
    pub(super) fn insert(&mut self, r: Reg) {
        let idx = r as usize;
        if idx < self.nbits {
            self.words[idx / 64] |= 1u64 << (idx % 64);
        }
    }

    #[inline]
    pub(super) fn remove(&mut self, r: Reg) {
        let idx = r as usize;
        if idx < self.nbits {
            self.words[idx / 64] &= !(1u64 << (idx % 64));
        }
    }

    pub(super) fn clear_below(&mut self, limit: Reg) {
        let lim = (limit as usize).min(self.nbits);
        let full_words = lim / 64;
        for w in self.words.iter_mut().take(full_words) {
            *w = 0;
        }
        let rem = lim % 64;
        if rem != 0 && full_words < self.words.len() {
            self.words[full_words] &= !((1u64 << rem) - 1);
        }
    }

    pub(super) fn intersect_with(&mut self, other: &Self) {
        for (a, b) in self.words.iter_mut().zip(other.words.iter()) {
            *a &= *b;
        }
    }

    pub(super) fn union_with(&mut self, other: &Self) -> bool {
        let mut changed = false;
        for (a, b) in self.words.iter_mut().zip(other.words.iter()) {
            let next = *a | *b;
            if next != *a {
                *a = next;
                changed = true;
            }
        }
        changed
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = Reg> + '_ {
        self.words.iter().enumerate().flat_map(|(wi, &w)| {
            let mut rem = w;
            std::iter::from_fn(move || {
                if rem == 0 {
                    None
                } else {
                    let bit = rem.trailing_zeros() as usize;
                    rem &= rem - 1;
                    Some((wi * 64 + bit) as Reg)
                }
            })
        })
    }
}

/// Partition `insns` into reachable basic blocks with explicit CFG edges,
/// predecessor lists, handler flags, and propagated exception depths.
pub(super) fn build_raw_cfg(insns: &[Insn]) -> Vec<RawBlock> {
    let n = insns.len();
    let mut is_start = vec![false; n + 1];
    is_start[0] = true;
    let mut handler_pcs: HashSet<usize> = HashSet::new();

    for (pc, insn) in insns.iter().enumerate() {
        if let Some(off) = jump_offset(insn) {
            let target = (pc as i64 + 1 + off as i64) as usize;
            if target <= n {
                is_start[target] = true;
                if matches!(insn, Insn::SetupExcept(_)) {
                    handler_pcs.insert(target);
                }
            }
        }
        if ends_raw_block(insn) && pc < n {
            is_start[pc + 1] = true;
        }
    }

    let mut pc_to_raw: Vec<usize> = vec![usize::MAX; n + 1];
    let mut starts: Vec<usize> = Vec::new();
    for (pc, &flag) in is_start.iter().enumerate() {
        if flag && pc < n {
            pc_to_raw[pc] = starts.len();
            starts.push(pc);
        }
    }

    // Synthetic exit block index when an edge targets `pc == n` (fall-off-end).
    let synthetic_exit_raw = if is_start[n] || !ends_raw_block(&insns[n - 1]) {
        let idx = starts.len();
        pc_to_raw[n] = idx;
        Some(idx)
    } else {
        None
    };

    let mut raw_blocks: Vec<RawBlock> = Vec::with_capacity(starts.len() + 1);
    for (bi, &start_pc) in starts.iter().enumerate() {
        let end_pc = starts.get(bi + 1).copied().unwrap_or(n);
        let last_pc = end_pc - 1;
        let has_term = ends_raw_block(&insns[last_pc]);
        let (body_end, term_pc) = if has_term {
            (last_pc, Some(last_pc))
        } else {
            (end_pc, None)
        };
        let succs = collect_block_succs(insns, term_pc, end_pc, n, &pc_to_raw);
        raw_blocks.push(RawBlock {
            start_pc,
            body_end,
            term_pc,
            succs,
            preds: Vec::new(),
            exc_depth: 0,
            is_handler: handler_pcs.contains(&start_pc),
        });
    }

    if synthetic_exit_raw.is_some() {
        raw_blocks.push(RawBlock {
            start_pc: n,
            body_end: n,
            term_pc: None,
            succs: SmallVec::new(),
            preds: Vec::new(),
            exc_depth: 0,
            is_handler: false,
        });
    }

    compact_reachable_with_exc_depth(raw_blocks, insns)
}

fn collect_block_succs(
    insns: &[Insn],
    term_pc: Option<usize>,
    end_pc: usize,
    n: usize,
    pc_to_raw: &[usize],
) -> SmallVec<[RawEdge; 2]> {
    let mut succs: SmallVec<[RawEdge; 2]> = SmallVec::new();
    let Some(tpc) = term_pc else {
        succs.push(RawEdge {
            target: pc_to_raw[end_pc.min(n)],
            kind: RawEdgeKind::Normal,
        });
        return succs;
    };

    let target_of = |off: i32| -> usize {
        let dst_pc = (tpc as i64 + 1 + off as i64) as usize;
        pc_to_raw[dst_pc.min(n)]
    };
    let fallthrough = pc_to_raw[end_pc.min(n)];
    let edge = |target, kind| RawEdge { target, kind };

    match &insns[tpc] {
        Insn::Jump(off) => succs.push(edge(target_of(*off), RawEdgeKind::Normal)),
        Insn::JumpIfFalse(_, off)
        | Insn::CmpJumpIfFalse(_, _, _, off)
        | Insn::CmpJumpIfFalseConst(_, _, _, off)
        | Insn::MatchExcept(_, off) => {
            // Fallthrough first (then / matched), jump target second (else / nomatch).
            succs.push(edge(fallthrough, RawEdgeKind::Normal));
            succs.push(edge(target_of(*off), RawEdgeKind::Normal));
        }
        Insn::JumpIfTrue(_, off)
        | Insn::CmpJumpIfTrue(_, _, _, off)
        | Insn::CmpJumpIfTrueConst(_, _, _, off) => {
            // Jump target first (then), fallthrough second (else).
            succs.push(edge(target_of(*off), RawEdgeKind::Normal));
            succs.push(edge(fallthrough, RawEdgeKind::Normal));
        }
        Insn::ForIter(_, _, off) => {
            succs.push(edge(fallthrough, RawEdgeKind::ForIterBody));
            succs.push(edge(target_of(*off), RawEdgeKind::Normal));
        }
        Insn::SetupExcept(off) => {
            succs.push(edge(fallthrough, RawEdgeKind::Normal));
            succs.push(edge(target_of(*off), RawEdgeKind::SetupExceptHandler));
        }
        Insn::MatchExceptStar(_, off) => {
            succs.push(edge(fallthrough, RawEdgeKind::ExceptStarMatched));
            succs.push(edge(target_of(*off), RawEdgeKind::Normal));
        }
        Insn::Return(_)
        | Insn::ReturnNone
        | Insn::RaiseValue(_)
        | Insn::RaiseExceptStarResidual(_)
        | Insn::RaiseFrom(..)
        | Insn::RaiseReRaise
        | Insn::RaiseAssert(_) => {}
        _ => unreachable!("non-terminator in term_pc"),
    }
    succs
}

/// Propagate reachability and `exc_depth` from raw block 0, then compact to
/// only reachable blocks so every `MirBlock` is reachable from entry.
fn compact_reachable_with_exc_depth(raw_blocks: Vec<RawBlock>, insns: &[Insn]) -> Vec<RawBlock> {
    let mut reachable = vec![false; raw_blocks.len()];
    let mut exc_depth: Vec<Option<u16>> = vec![None; raw_blocks.len()];
    let mut work = vec![0usize];
    reachable[0] = true;
    exc_depth[0] = Some(0);

    while let Some(bi) = work.pop() {
        let mut cur_depth = exc_depth[bi].unwrap_or(0);
        for insn in &insns[raw_blocks[bi].start_pc..raw_blocks[bi].body_end] {
            if matches!(insn, Insn::PopExcept) {
                cur_depth = cur_depth.saturating_sub(1);
            }
        }
        let is_setup = raw_blocks[bi]
            .term_pc
            .is_some_and(|t| matches!(insns[t], Insn::SetupExcept(_)));
        for &edge in &raw_blocks[bi].succs {
            let next_d = if is_setup && edge.kind != RawEdgeKind::SetupExceptHandler {
                cur_depth.saturating_add(1)
            } else {
                cur_depth
            };
            if !reachable[edge.target] {
                reachable[edge.target] = true;
                exc_depth[edge.target] = Some(next_d);
                work.push(edge.target);
            }
        }
    }

    let mut raw_to_mir: Vec<Option<usize>> = vec![None; raw_blocks.len()];
    let mut compact: Vec<RawBlock> = Vec::new();
    for (old_idx, mut rb) in raw_blocks.into_iter().enumerate() {
        if reachable[old_idx] {
            let new_idx = compact.len();
            raw_to_mir[old_idx] = Some(new_idx);
            rb.exc_depth = exc_depth[old_idx].unwrap_or(0);
            compact.push(rb);
        }
    }
    for rb in &mut compact {
        for edge in &mut rb.succs {
            edge.target = raw_to_mir[edge.target].expect("reachable successor must be mapped");
        }
    }
    for bi in 0..compact.len() {
        let succs = compact[bi].succs.clone();
        for edge in succs {
            compact[edge.target].preds.push(bi);
        }
    }
    compact
}
