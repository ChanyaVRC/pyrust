//! Dominator-tree computation and SSA soundness verifier for [`MirFn`].
//!
//! [`verify_mir`] enforces the structural invariants of SSA MIR:
//! 1. Single static assignment: every [`Val`] is defined at most once across
//!    all block parameters, instructions, and value-producing terminators.
//! 2. Dominance: every use of a [`Val`] is dominated by its definition (or
//!    preceded by it within the same basic block).
//! 3. CFG edge arity: every terminator target [`BlockId`] is in range and its
//!    argument count matches `target.params.len()`.
//! 4. Predecessor consistency: every block's `preds` matches the exact incoming
//!    CFG edges of all terminators.

use std::collections::HashSet;

use smallvec::SmallVec;

use super::ir::{BlockId, MirFn, Val};

/// Dominator tree over reachable blocks of a [`MirFn`] (Cooper-Harvey-Kennedy).
#[derive(Debug, Clone)]
pub struct DominatorTree {
    /// Postorder index for each block (`usize::MAX` if unreachable).
    pub rpo_index: Vec<usize>,
    /// Reachable blocks in Reverse Postorder (entry first).
    pub rpo: Vec<BlockId>,
    /// Immediate dominator of each block (`None` for entry or unreachable).
    pub idom: Vec<Option<BlockId>>,
    /// Children of each block in the dominator tree.
    pub children: Vec<Vec<BlockId>>,
}

impl DominatorTree {
    /// Compute the dominator tree of `func` starting at `func.entry`.
    pub fn compute(func: &MirFn) -> Self {
        let n = func.blocks.len();
        let mut visited = vec![false; n];
        let mut postorder = Vec::with_capacity(n);

        // Iterative DFS to compute postorder without risking stack overflow on
        // huge functions.  Visit successors in reverse order so `succs[0]`
        // (`then_bb` / `body_bb` / `try_bb` / `matched_bb`) finishes LAST in
        // postorder and therefore appears IMMEDIATELY after `bb` in RPO
        // (keeping fallthrough edges jump-free!).
        if (func.entry.0 as usize) < n {
            let mut stack: Vec<(BlockId, SmallVec<[BlockId; 2]>)> = vec![(
                func.entry,
                func.blocks[func.entry.0 as usize].term.successors(),
            )];
            visited[func.entry.0 as usize] = true;
            while let Some((bb, succs)) = stack.last_mut() {
                if let Some(s) = succs.pop() {
                    if (s.0 as usize) < n && !visited[s.0 as usize] {
                        visited[s.0 as usize] = true;
                        let child_succs = func.blocks[s.0 as usize].term.successors();
                        stack.push((s, child_succs));
                    }
                } else {
                    let finished = *bb;
                    stack.pop();
                    postorder.push(finished);
                }
            }
        }

        let mut rpo = postorder;
        rpo.reverse();
        let mut rpo_index = vec![usize::MAX; n];
        for (idx, &bb) in rpo.iter().enumerate() {
            rpo_index[bb.0 as usize] = idx;
        }

        let mut idom: Vec<Option<BlockId>> = vec![None; n];
        if (func.entry.0 as usize) < n {
            idom[func.entry.0 as usize] = Some(func.entry);
        }

        let intersect = |mut b1: usize, mut b2: usize, idom: &[Option<BlockId>]| -> BlockId {
            while b1 != b2 {
                while b1 > b2 {
                    let bb = rpo[b1];
                    b1 = rpo_index[idom[bb.0 as usize].expect("idom must exist").0 as usize];
                }
                while b2 > b1 {
                    let bb = rpo[b2];
                    b2 = rpo_index[idom[bb.0 as usize].expect("idom must exist").0 as usize];
                }
            }
            rpo[b1]
        };

        let mut changed = true;
        while changed {
            changed = false;
            for &bb in rpo.iter().skip(1) {
                let mut new_idom: Option<BlockId> = None;
                for &p in &func.blocks[bb.0 as usize].preds {
                    let p_idx = p.0 as usize;
                    if p_idx < n && idom[p_idx].is_some() {
                        new_idom = match new_idom {
                            None => Some(p),
                            Some(cur) => Some(intersect(
                                rpo_index[p_idx],
                                rpo_index[cur.0 as usize],
                                &idom,
                            )),
                        };
                    }
                }
                if idom[bb.0 as usize] != new_idom {
                    idom[bb.0 as usize] = new_idom;
                    changed = true;
                }
            }
        }

        if (func.entry.0 as usize) < n {
            idom[func.entry.0 as usize] = None;
        }

        let mut children = vec![Vec::new(); n];
        for &bb in &rpo {
            if let Some(parent) = idom[bb.0 as usize] {
                children[parent.0 as usize].push(bb);
            }
        }

        Self {
            rpo_index,
            rpo,
            idom,
            children,
        }
    }

    /// True if block `a` dominates block `b` (reflexive: `a` dominates `a`).
    #[cfg_attr(not(any(debug_assertions, test)), allow(dead_code))]
    #[inline]
    pub fn dominates(&self, a: BlockId, b: BlockId) -> bool {
        if a == b {
            return true;
        }
        let mut cur = b;
        while let Some(parent) = self.idom.get(cur.0 as usize).copied().flatten() {
            if parent == a {
                return true;
            }
            cur = parent;
        }
        false
    }

    /// Compute dominance frontiers for all reachable blocks.
    pub fn dominance_frontiers(&self, func: &MirFn) -> Vec<Vec<BlockId>> {
        let n = func.blocks.len();
        let mut df: Vec<Vec<BlockId>> = vec![Vec::new(); n];
        for &b in &self.rpo {
            let preds = &func.blocks[b.0 as usize].preds;
            if preds.len() < 2 {
                continue;
            }
            let b_idom = self.idom[b.0 as usize];
            for &p in preds {
                if self.rpo_index[p.0 as usize] == usize::MAX {
                    continue;
                }
                let mut runner = Some(p);
                while runner.is_some() && runner != b_idom {
                    let r = runner.unwrap();
                    if !df[r.0 as usize].contains(&b) {
                        df[r.0 as usize].push(b);
                    }
                    runner = self.idom[r.0 as usize];
                }
            }
        }
        df
    }
}

/// Location where an SSA [`Val`] is defined.
#[cfg_attr(not(any(debug_assertions, test)), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefLoc {
    BlockParam(BlockId),
    Inst(BlockId, usize),
    Term(BlockId),
}

/// Verify all SSA, dominance, and CFG invariants of `func`.
#[cfg_attr(not(any(debug_assertions, test)), allow(dead_code))]
pub fn verify_mir(func: &MirFn) -> Result<(), String> {
    let n = func.blocks.len();
    if n == 0 {
        return Err("MIR function has zero basic blocks".to_string());
    }
    if (func.entry.0 as usize) >= n {
        return Err(format!(
            "MIR entry block bb{} out of range (blocks={n})",
            func.entry.0
        ));
    }

    // 1. Check block IDs, terminator targets, edge arg arities, and preds consistency.
    let mut expected_preds: Vec<Vec<BlockId>> = vec![Vec::new(); n];
    for (idx, block) in func.blocks.iter().enumerate() {
        if block.id.0 as usize != idx {
            return Err(format!(
                "block at index {idx} has mismatched id bb{}",
                block.id.0
            ));
        }
        let mut edge_err: Option<String> = None;
        block.term.for_each_edge(|target, args| {
            let t = target.0 as usize;
            if t >= n {
                edge_err = Some(format!(
                    "bb{} terminator targets out-of-range bb{t} (blocks={n})",
                    block.id.0
                ));
                return;
            }
            let expected_params = func.blocks[t].params.len();
            if args.len() != expected_params {
                edge_err = Some(format!(
                    "edge bb{} -> bb{t} passes {} args, but bb{t} expects {expected_params} params",
                    block.id.0,
                    args.len()
                ));
                return;
            }
            expected_preds[t].push(block.id);
        });
        if let Some(err) = edge_err {
            return Err(err);
        }
    }
    for (idx, block) in func.blocks.iter().enumerate() {
        let mut actual = block.preds.clone();
        let mut expected = expected_preds[idx].clone();
        actual.sort_unstable();
        expected.sort_unstable();
        if actual != expected {
            return Err(format!(
                "bb{idx} preds mismatch: stored={actual:?}, actual_from_terminators={expected:?}"
            ));
        }
    }

    // 2. Single-assignment check: every Val defined at most once and < next_val.
    let mut def_loc: Vec<Option<DefLoc>> = vec![None; func.next_val as usize];
    let mut record_def = |v: Val, loc: DefLoc| -> Result<(), String> {
        let idx = v.0 as usize;
        if idx >= def_loc.len() {
            return Err(format!(
                "Val v{} >= next_val ({}) at {loc:?}",
                v.0, func.next_val
            ));
        }
        if let Some(prev) = def_loc[idx] {
            return Err(format!(
                "SSA violation: v{} defined twice (first at {prev:?}, again at {loc:?})",
                v.0
            ));
        }
        def_loc[idx] = Some(loc);
        Ok(())
    };

    for block in &func.blocks {
        for &p in &block.params {
            record_def(p, DefLoc::BlockParam(block.id))?;
        }
        for (i_idx, inst) in block.instrs.iter().enumerate() {
            let mut err = None;
            inst.for_each_def(|d| {
                if err.is_none()
                    && let Err(e) = record_def(d, DefLoc::Inst(block.id, i_idx))
                {
                    err = Some(e);
                }
            });
            if let Some(e) = err {
                return Err(e);
            }
        }
        let mut term_def_err = None;
        block.term.for_each_def(|d| {
            if term_def_err.is_none()
                && let Err(e) = record_def(d, DefLoc::Term(block.id))
            {
                term_def_err = Some(e);
            }
        });
        if let Some(e) = term_def_err {
            return Err(e);
        }
    }

    // 3. Dominance check for every Val use in reachable blocks.
    let dom = DominatorTree::compute(func);
    let reachable: HashSet<BlockId> = dom.rpo.iter().copied().collect();

    let check_use = |v: Val, use_bb: BlockId, use_inst_idx: Option<usize>| -> Result<(), String> {
        let idx = v.0 as usize;
        let Some(Some(loc)) = def_loc.get(idx) else {
            return Err(format!(
                "SSA violation: use of undefined v{} in bb{}",
                v.0, use_bb.0
            ));
        };
        let dominated = match *loc {
            DefLoc::BlockParam(def_bb) => dom.dominates(def_bb, use_bb),
            DefLoc::Inst(def_bb, def_idx) => {
                if def_bb == use_bb {
                    match use_inst_idx {
                        Some(ui) => def_idx < ui,
                        None => true, // terminator comes after all instructions
                    }
                } else {
                    dom.dominates(def_bb, use_bb)
                }
            }
            DefLoc::Term(def_bb) => {
                // A terminator def (e.g. ForIter::dst) dominates strictly
                // dominated successor blocks, and is usable in the terminator's
                // own outgoing edge args.
                if def_bb == use_bb {
                    use_inst_idx.is_none()
                } else {
                    dom.dominates(def_bb, use_bb)
                }
            }
        };
        if !dominated {
            return Err(format!(
                "SSA dominance violation: v{} (defined at {loc:?}) does not dominate use in bb{} (inst={use_inst_idx:?})",
                v.0, use_bb.0
            ));
        }
        Ok(())
    };

    for &bb in &dom.rpo {
        let block = &func.blocks[bb.0 as usize];
        for (i_idx, inst) in block.instrs.iter().enumerate() {
            let mut err = None;
            inst.for_each_use(|u| {
                if err.is_none()
                    && let Err(e) = check_use(u, bb, Some(i_idx))
                {
                    err = Some(e);
                }
            });
            if let Some(e) = err {
                return Err(e);
            }
        }
        let mut term_err = None;
        block.term.for_each_use(|u| {
            if term_err.is_none()
                && let Err(e) = check_use(u, bb, None)
            {
                term_err = Some(e);
            }
        });
        if let Some(e) = term_err {
            return Err(e);
        }
    }

    // Ensure entry block has no block parameters (entry parameters come from
    // LoadLocal).
    if !func.blocks[func.entry.0 as usize].params.is_empty() && reachable.contains(&func.entry) {
        // Entry block params are permitted only if entry has incoming edges (e.g.
        // a loop back-edge to entry) with matching arity (already checked above).
        if func.blocks[func.entry.0 as usize].preds.is_empty() {
            return Err(format!(
                "entry block bb{} has {} params with no predecessors",
                func.entry.0,
                func.blocks[func.entry.0 as usize].params.len()
            ));
        }
    }

    Ok(())
}
