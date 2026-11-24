//! Basic-block terminator bytecode emission with fallthrough and trampoline
//! lowering.

use crate::bytecode::{Insn, InsnDesc, MatchExceptStarDesc, NO_ASSERT_MSG, push_desc};
use crate::value::Value;

use crate::mir::ir::{BlockId, MirBlock, Terminator, Val};

use super::de_ssa::{EdgeTrampoline, JumpPatch};
use super::emit::BytecodeEmitter;

impl BytecodeEmitter<'_> {
    /// Emit a conditional jump instruction targeting `taken_bb` (via a
    /// trampoline if `taken_args` require copies), followed by inline
    /// `fallthrough_args` copies and an optional `Jump(fallthrough_bb)`.
    #[allow(clippy::too_many_arguments)]
    fn emit_two_way_term(
        &mut self,
        branch_insn: Insn,
        taken_bb: BlockId,
        taken_args: &[Val],
        fallthrough_bb: BlockId,
        fallthrough_args: &[Val],
        order_idx: usize,
        next_bb: Option<BlockId>,
        local_back_tramps: &mut Vec<(usize, EdgeTrampoline)>,
        ln: u32,
        sp: crate::ast::CaretSpan,
    ) {
        let fallthrough_copies = self.edge_copies(fallthrough_bb, fallthrough_args);
        let taken_copies = self.edge_copies(taken_bb, taken_args);
        let taken_target =
            self.target_or_trampoline(taken_bb, taken_copies, order_idx, local_back_tramps, ln, sp);
        let pc = self.emit(branch_insn, ln, sp);
        self.patches.push(JumpPatch {
            insn_pc: pc,
            target: taken_target,
        });
        self.emit_fallthrough_and_jump(fallthrough_bb, &fallthrough_copies, next_bb, ln, sp);
    }

    pub(super) fn emit_terminator(
        &mut self,
        block: &MirBlock,
        order_idx: usize,
        rpo_len: usize,
        next_bb: Option<BlockId>,
        local_back_tramps: &mut Vec<(usize, EdgeTrampoline)>,
    ) {
        let tln = block.term_lineno;
        let tsp = block.term_col_span;

        match &block.term {
            Terminator::Jump { target, args } => {
                let copies = self.edge_copies(*target, args);
                self.emit_fallthrough_and_jump(*target, &copies, next_bb, tln, tsp);
            }
            Terminator::Branch {
                cond,
                then_bb,
                then_args,
                else_bb,
                else_args,
            } => {
                let cr = self.reg_of(*cond);
                if next_bb == Some(*else_bb) {
                    self.emit_two_way_term(
                        Insn::JumpIfTrue(cr, 0),
                        *then_bb,
                        then_args,
                        *else_bb,
                        else_args,
                        order_idx,
                        next_bb,
                        local_back_tramps,
                        tln,
                        tsp,
                    );
                } else {
                    self.emit_two_way_term(
                        Insn::JumpIfFalse(cr, 0),
                        *else_bb,
                        else_args,
                        *then_bb,
                        then_args,
                        order_idx,
                        next_bb,
                        local_back_tramps,
                        tln,
                        tsp,
                    );
                }
            }
            Terminator::Return(v) => {
                if self.const_val_of[v.0 as usize]
                    .as_ref()
                    .is_some_and(Value::is_none)
                {
                    self.emit(Insn::ReturnNone, tln, tsp);
                } else {
                    self.emit(Insn::Return(self.reg_of(*v)), tln, tsp);
                }
            }
            Terminator::Raise(Some(v)) => {
                self.emit(Insn::RaiseValue(self.reg_of(*v)), tln, tsp);
            }
            Terminator::Raise(None) => {
                self.emit(Insn::RaiseReRaise, tln, tsp);
            }
            Terminator::RaiseSpecial { insn, uses } => {
                let lowered = match insn {
                    Insn::RaiseFrom(_, _) => {
                        Insn::RaiseFrom(self.reg_of(uses[0]), self.reg_of(uses[1]))
                    }
                    Insn::RaiseAssert(r) => {
                        if *r == NO_ASSERT_MSG {
                            Insn::RaiseAssert(NO_ASSERT_MSG)
                        } else {
                            Insn::RaiseAssert(self.reg_of(uses[0]))
                        }
                    }
                    Insn::RaiseExceptStarResidual(_) => {
                        Insn::RaiseExceptStarResidual(self.reg_of(uses[0]))
                    }
                    other => other.clone(),
                };
                self.emit(lowered, tln, tsp);
            }
            Terminator::ForIter {
                slot,
                dst,
                body_bb,
                body_args,
                exit_bb,
                exit_args,
            } => {
                let dst_r = self.reg_of(*dst);
                self.emit_two_way_term(
                    Insn::ForIter(dst_r, *slot, 0),
                    *exit_bb,
                    exit_args,
                    *body_bb,
                    body_args,
                    order_idx,
                    next_bb,
                    local_back_tramps,
                    tln,
                    tsp,
                );
            }
            Terminator::SetupExcept {
                try_bb,
                try_args,
                handler_bb,
                handler_args,
            } => {
                self.emit_two_way_term(
                    Insn::SetupExcept(0),
                    *handler_bb,
                    handler_args,
                    *try_bb,
                    try_args,
                    order_idx,
                    next_bb,
                    local_back_tramps,
                    tln,
                    tsp,
                );
            }
            Terminator::MatchExcept {
                type_val,
                matched_bb,
                matched_args,
                nomatch_bb,
                nomatch_args,
            } => {
                let type_r = self.reg_of(*type_val);
                self.emit_two_way_term(
                    Insn::MatchExcept(type_r, 0),
                    *nomatch_bb,
                    nomatch_args,
                    *matched_bb,
                    matched_args,
                    order_idx,
                    next_bb,
                    local_back_tramps,
                    tln,
                    tsp,
                );
            }
            Terminator::MatchExceptStar {
                type_val,
                src_group,
                matched_def,
                residual_def,
                matched_bb,
                matched_args,
                nomatch_bb,
                nomatch_args,
            } => {
                let matched_copies = self.edge_copies(*matched_bb, matched_args);
                let nomatch_copies = self.edge_copies(*nomatch_bb, nomatch_args);
                let nomatch_target = self.target_or_trampoline(
                    *nomatch_bb,
                    nomatch_copies,
                    order_idx,
                    local_back_tramps,
                    tln,
                    tsp,
                );
                // MatchExceptStar overwrites `src_group` in-place on match; isolate
                // in a scratch slot so `src_group` is not clobbered on no-match.
                let sg_scratch = self.scratch_base;
                self.max_reg_used = self.max_reg_used.max(sg_scratch);
                self.emit(Insn::Move(sg_scratch, self.reg_of(*src_group)), tln, tsp);
                let type_reg = self.reg_of(*type_val);
                let matched_dst = self.reg_of(*matched_def);
                let desc_idx = push_desc(
                    &mut self.descs,
                    InsnDesc::MatchExceptStar(MatchExceptStarDesc {
                        type_reg,
                        src_group: sg_scratch,
                        matched_dst,
                    }),
                );
                let pc = self.emit(Insn::MatchExceptStar(desc_idx, 0), tln, tsp);
                self.patches.push(JumpPatch {
                    insn_pc: pc,
                    target: nomatch_target,
                });
                let res_reg = self.reg_of(*residual_def);
                if res_reg != sg_scratch {
                    self.emit(Insn::Move(res_reg, sg_scratch), tln, tsp);
                }
                self.emit_fallthrough_and_jump(*matched_bb, &matched_copies, next_bb, tln, tsp);
            }
            Terminator::Unreachable => {
                if order_idx + 1 == rpo_len {
                    self.emit(Insn::ReturnNone, tln, tsp);
                }
            }
        }
    }
}
