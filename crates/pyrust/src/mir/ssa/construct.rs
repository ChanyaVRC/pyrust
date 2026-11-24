//! Dominator-tree SSA renaming pass and pinned edge-bridge insertion.

use smallvec::SmallVec;

use crate::ast::CaretSpan;
use crate::bytecode::{FnCode, Insn, InsnDesc, Reg};

use crate::mir::insn_info::insn_may_invalidate_named_locals;
use crate::mir::ir::{BlockId, MirBlock, MirFn, MirInst, MirOp, Terminator, Val};

use super::analysis::SsaAnalysis;
use super::cfg::{RawBlock, RegBits};

/// Stateful dominator-tree SSA renamer.
pub(super) struct SsaRenamer<'a> {
    pub(super) compact_raw: &'a [RawBlock],
    pub(super) code: &'a FnCode,
    pub(super) num_locals: Reg,
    pub(super) aliased_locals: bool,
    pub(super) analysis: &'a SsaAnalysis,
    pub(super) mir: &'a mut MirFn,
    pub(super) stacks: Vec<Vec<Option<Val>>>,
}

/// Per-block instruction/stack builder active while lifting a single block.
pub(super) struct BlockBuilder<'a, 'b> {
    pub(super) renamer: &'b mut SsaRenamer<'a>,
    pushed_regs: Vec<Reg>,
    instrs_out: Vec<MirInst>,
}

impl BlockBuilder<'_, '_> {
    #[inline]
    pub(super) fn push_def(&mut self, r: Reg, val: Option<Val>) {
        if (r as usize) < self.renamer.stacks.len() {
            self.renamer.stacks[r as usize].push(val);
            self.pushed_regs.push(r);
        }
    }

    #[inline]
    pub(super) fn emit_raw(
        &mut self,
        def: Option<Val>,
        op: MirOp,
        lineno: u32,
        col_span: CaretSpan,
    ) {
        self.instrs_out.push(MirInst {
            def,
            op,
            lineno,
            col_span,
        });
    }

    #[inline]
    pub(super) fn emit_def(&mut self, op: MirOp, lineno: u32, col_span: CaretSpan) -> Val {
        let v = self.renamer.mir.fresh_val();
        self.emit_raw(Some(v), op, lineno, col_span);
        v
    }

    #[inline]
    pub(super) fn emit_void(&mut self, op: MirOp, lineno: u32, col_span: CaretSpan) {
        self.emit_raw(None, op, lineno, col_span);
    }

    /// Return the active SSA [`Val`] for `r`, or emit `LoadLocal(r)`.
    pub(super) fn read_reg(&mut self, r: Reg, lineno: u32, col_span: CaretSpan) -> Val {
        if let Some(Some(v)) = self
            .renamer
            .stacks
            .get(r as usize)
            .and_then(|s| s.last())
            .copied()
        {
            return v;
        }
        let v = self.emit_def(MirOp::LoadLocal(r), lineno, col_span);
        // Once LoadLocal(r) succeeds without raising UnboundLocalError, `r` is
        // bound to `v` in this block (until deleted or invalidated).
        self.push_def(r, Some(v));
        v
    }

    /// Bind `r` to `v` in SSA and emit write-through `StoreLocal` whenever `r`
    /// is pinned (`0..num_locals` or exception-escaping temp).
    pub(super) fn write_reg(&mut self, r: Reg, v: Val, lineno: u32, col_span: CaretSpan) {
        self.push_def(r, Some(v));
        if self.renamer.analysis.pinned.contains(r) {
            self.emit_void(MirOp::StoreLocal { reg: r, val: v }, lineno, col_span);
        }
    }
}

impl<'a> SsaRenamer<'a> {
    pub(super) fn new(
        compact_raw: &'a [RawBlock],
        code: &'a FnCode,
        num_locals: Reg,
        num_regs: usize,
        aliased_locals: bool,
        analysis: &'a SsaAnalysis,
        mir: &'a mut MirFn,
    ) -> Self {
        Self {
            compact_raw,
            code,
            num_locals,
            aliased_locals,
            analysis,
            mir,
            stacks: vec![Vec::new(); num_regs],
        }
    }

    #[inline]
    pub(super) fn lineno_of(&self, pc: usize) -> u32 {
        self.code.line_at(pc)
    }

    #[inline]
    pub(super) fn span_of(&self, pc: usize) -> CaretSpan {
        self.code.col_at(pc).unwrap_or((0, 0, 0, 0))
    }

    pub(super) fn collect_edge_args(
        &self,
        target_bi: usize,
        override_regs: &[(Reg, Val)],
    ) -> Vec<Val> {
        self.analysis.phi_regs[target_bi]
            .iter()
            .map(|&r| {
                if let Some(&(_, ov)) = override_regs.iter().find(|&&(or, _)| or == r) {
                    return ov;
                }
                self.stacks
                    .get(r as usize)
                    .and_then(|s| s.last())
                    .copied()
                    .flatten()
                    .expect("phi register must be definitely bound on every incoming edge")
            })
            .collect()
    }

    pub(super) fn rename_block(&mut self, bi: usize) {
        let mut builder = BlockBuilder {
            renamer: self,
            pushed_regs: Vec::new(),
            instrs_out: Vec::new(),
        };

        // At an exception handler block, clear named locals so reads reload from R[r].
        if builder.renamer.compact_raw[bi].is_handler {
            for r in 0..builder.renamer.num_locals {
                builder.push_def(r, None);
            }
        }

        // Soundness guard for non-phi merges: if `bi` is not the entry block and
        // register `r` is assigned somewhere in the function yet is NOT in
        // `def_in[bi]`, `bi` did not receive a phi for `r`, so it must NOT
        // inherit a stale `LoadLocal(r)` from a dominating block across a
        // modifying sibling branch.
        if bi != 0 {
            for r_usize in 0..builder.renamer.analysis.def_blocks.len() {
                let r = r_usize as Reg;
                if !builder.renamer.analysis.def_blocks[r_usize].is_empty()
                    && !builder.renamer.analysis.def_in[bi].contains(r)
                {
                    builder.push_def(r, None);
                }
            }
        }

        // Push block-parameter phis onto the rename stacks.
        for pi in 0..builder.renamer.analysis.phi_regs[bi].len() {
            let r = builder.renamer.analysis.phi_regs[bi][pi];
            let v = builder.renamer.analysis.phi_vals[bi][pi];
            builder.push_def(r, Some(v));
        }

        let start_pc = builder.renamer.compact_raw[bi].start_pc;
        let body_end = builder.renamer.compact_raw[bi].body_end;
        for pc in start_pc..body_end {
            let insn = &builder.renamer.code.insns[pc];
            let ln = builder.renamer.lineno_of(pc);
            let sp = builder.renamer.span_of(pc);
            builder.lift_body_insn(insn, ln, sp);

            // At module / class scope, external namespace aliases may mutate
            // named locals during protocol-dispatching instructions.
            if builder.renamer.aliased_locals && insn_may_invalidate_named_locals(insn) {
                for r in 0..builder.renamer.num_locals {
                    builder.push_def(r, None);
                }
            }
        }

        let rb = &builder.renamer.compact_raw[bi];
        let (term, term_lineno, term_col_span) = builder.lift_terminator(rb);
        let BlockBuilder {
            pushed_regs,
            instrs_out,
            ..
        } = builder;

        self.mir.blocks[bi].instrs = instrs_out;
        self.mir.blocks[bi].term = term;
        self.mir.blocks[bi].term_lineno = term_lineno;
        self.mir.blocks[bi].term_col_span = term_col_span;

        // Check if this block ended with `ForIter` or `MatchExceptStar`, which
        // defines values on its first (fallthrough) successor ONLY.
        type FallthroughBindings = SmallVec<[(Reg, Val); 2]>;
        let special_fallthrough: Option<(usize, FallthroughBindings)> =
            self.compact_raw[bi].term_pc.and_then(|tpc| {
                match (&self.code.insns[tpc], &self.mir.blocks[bi].term) {
                    (Insn::ForIter(dst_reg, _, _), Terminator::ForIter { dst, body_bb, .. }) => {
                        Some((body_bb.0 as usize, smallvec::smallvec![(*dst_reg, *dst)]))
                    }
                    (
                        Insn::MatchExceptStar(desc_idx, _),
                        Terminator::MatchExceptStar {
                            matched_def,
                            residual_def,
                            matched_bb,
                            ..
                        },
                    ) => {
                        let desc = self.code.descs[*desc_idx as usize].as_match_except_star();
                        Some((
                            matched_bb.0 as usize,
                            smallvec::smallvec![
                                (desc.matched_dst, *matched_def),
                                (desc.src_group, *residual_def),
                            ],
                        ))
                    }
                    _ => None,
                }
            });

        // Recurse on dominator-tree children.
        for &child in &self.analysis.dom.children[bi] {
            let child_idx = child.0 as usize;
            let mut extra_pushes = 0usize;
            if let Some((target_idx, ref bindings)) = special_fallthrough
                && child_idx == target_idx
            {
                for &(r, v) in bindings {
                    if (r as usize) < self.stacks.len() {
                        self.stacks[r as usize].push(Some(v));
                        extra_pushes += 1;
                    }
                }
            }

            self.rename_block(child_idx);

            // If `child_idx` is the exclusive fallthrough of `ForIter` or
            // `MatchExceptStar` and a bound reg is pinned, prepend write-through
            // `StoreLocal`s at the top of `child_idx` so canonical `R[r]` is updated!
            if extra_pushes > 0
                && let Some((_, ref bindings)) = special_fallthrough
            {
                let mut prepend = Vec::new();
                for &(r, v) in bindings {
                    if self.analysis.pinned.contains(r) {
                        prepend.push(MirInst {
                            def: None,
                            op: MirOp::StoreLocal { reg: r, val: v },
                            lineno: term_lineno,
                            col_span: term_col_span,
                        });
                    }
                }
                if !prepend.is_empty() {
                    let old = std::mem::take(&mut self.mir.blocks[child_idx].instrs);
                    prepend.extend(old);
                    self.mir.blocks[child_idx].instrs = prepend;
                }
                for &(r, _) in bindings.iter().rev() {
                    if (r as usize) < self.stacks.len() {
                        self.stacks[r as usize].pop();
                    }
                }
            }
        }

        // Unwind stack pushes from this block.
        for r in pushed_regs.into_iter().rev() {
            self.stacks[r as usize].pop();
        }
    }
}

/// Handle the rare case where a `ForIter` body or `MatchExceptStar` matched
/// block has >1 predecessor (so `dom.children[bi]` may not have directly owned
/// it for prepending `StoreLocal`): insert an edge bridge block carrying the
/// pinned `StoreLocal`s.
pub(super) fn insert_pinned_edge_bridges(
    compact_raw: &[RawBlock],
    insns: &[Insn],
    descs: &[InsnDesc],
    pinned: &RegBits,
    mir: &mut MirFn,
) {
    let nb = compact_raw.len();
    let mut bridges: Vec<MirBlock> = Vec::new();

    for bi in 0..nb {
        let Some(tpc) = compact_raw[bi].term_pc else {
            continue;
        };
        let needs_bridge = match (&insns[tpc], &mir.blocks[bi].term) {
            (Insn::ForIter(dst_reg, _, _), Terminator::ForIter { body_bb, .. }) => {
                pinned.contains(*dst_reg) && compact_raw[body_bb.0 as usize].preds.len() > 1
            }
            (
                Insn::MatchExceptStar(desc_idx, _),
                Terminator::MatchExceptStar { matched_bb, .. },
            ) => {
                let desc = descs[*desc_idx as usize].as_match_except_star();
                (pinned.contains(desc.src_group) || pinned.contains(desc.matched_dst))
                    && compact_raw[matched_bb.0 as usize].preds.len() > 1
            }
            _ => false,
        };
        if !needs_bridge {
            continue;
        }

        let bridge_id = BlockId((nb + bridges.len()) as u32);
        let ln = mir.blocks[bi].term_lineno;
        let sp = mir.blocks[bi].term_col_span;
        let mut bridge_instrs = Vec::new();
        let mut push_store = |reg: Reg, val: Val| {
            if pinned.contains(reg) {
                bridge_instrs.push(MirInst {
                    def: None,
                    op: MirOp::StoreLocal { reg, val },
                    lineno: ln,
                    col_span: sp,
                });
            }
        };

        let (orig_target, orig_args) = match (&insns[tpc], &mut mir.blocks[bi].term) {
            (
                Insn::ForIter(dst_reg, _, _),
                Terminator::ForIter {
                    dst,
                    body_bb,
                    body_args,
                    ..
                },
            ) => {
                push_store(*dst_reg, *dst);
                let t = *body_bb;
                let a = std::mem::take(body_args);
                *body_bb = bridge_id;
                (t, a)
            }
            (
                Insn::MatchExceptStar(desc_idx, _),
                Terminator::MatchExceptStar {
                    matched_def,
                    residual_def,
                    matched_bb,
                    matched_args,
                    ..
                },
            ) => {
                let desc = descs[*desc_idx as usize].as_match_except_star();
                push_store(desc.matched_dst, *matched_def);
                push_store(desc.src_group, *residual_def);
                let t = *matched_bb;
                let a = std::mem::take(matched_args);
                *matched_bb = bridge_id;
                (t, a)
            }
            _ => continue,
        };

        bridges.push(MirBlock {
            id: bridge_id,
            params: Vec::new(),
            instrs: bridge_instrs,
            term: Terminator::Jump {
                target: orig_target,
                args: orig_args,
            },
            term_lineno: ln,
            term_col_span: sp,
            preds: Vec::new(),
            exc_depth: mir.blocks[bi].exc_depth,
        });
    }

    if !bridges.is_empty() {
        mir.blocks.extend(bridges);
    }
}
