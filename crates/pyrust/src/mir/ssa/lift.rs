//! Bytecode-to-MIR instruction and terminator lifting during SSA renaming.

use smallvec::SmallVec;

use crate::ast::CaretSpan;
use crate::bytecode::{Insn, Reg};
use crate::value::Value;

use crate::mir::insn_info::{visit_ssa_reads, visit_ssa_writes};
use crate::mir::ir::{BlockId, MirOp, Terminator, Val};

use super::cfg::RawBlock;
use super::construct::BlockBuilder;

impl BlockBuilder<'_, '_> {
    /// Lift one non-terminator bytecode instruction into SSA MIR instructions.
    pub(super) fn lift_body_insn(&mut self, insn: &Insn, ln: u32, sp: CaretSpan) {
        match insn {
            Insn::LoadConst(dst, c_idx) => {
                let val = self.renamer.code.consts[*c_idx as usize].clone();
                let v = self.emit_def(MirOp::Const(val), ln, sp);
                self.write_reg(*dst, v, ln, sp);
            }
            Insn::LoadNone(dst) => {
                let v = self.emit_def(MirOp::Const(Value::none()), ln, sp);
                self.write_reg(*dst, v, ln, sp);
            }
            Insn::LoadNoneRange { start, count } => {
                for offset in 0..(*count as u32) {
                    let v = self.emit_def(MirOp::Const(Value::none()), ln, sp);
                    self.write_reg(*start + offset, v, ln, sp);
                }
            }
            Insn::Move(dst, src) => {
                let sv = self.read_reg(*src, ln, sp);
                let v = self.emit_def(MirOp::Copy(sv), ln, sp);
                self.write_reg(*dst, v, ln, sp);
            }
            Insn::BinOp(dst, lhs, op, rhs) | Insn::BinOpInPlace(dst, lhs, op, rhs) => {
                let is_inplace = matches!(insn, Insn::BinOpInPlace(..));
                let lv = self.read_reg(*lhs, ln, sp);
                let rv = self.read_reg(*rhs, ln, sp);
                let v = self.emit_def(
                    MirOp::BinOp {
                        op: *op,
                        lhs: lv,
                        rhs: rv,
                        is_inplace,
                    },
                    ln,
                    sp,
                );
                self.write_reg(*dst, v, ln, sp);
            }
            Insn::BinOpConst(dst, lhs, op, c_idx, is_aug) => {
                let lv = self.read_reg(*lhs, ln, sp);
                let val = self.renamer.code.consts[*c_idx as usize].clone();
                let rv = self.emit_def(MirOp::Const(val), ln, sp);
                let v = self.emit_def(
                    MirOp::BinOp {
                        op: *op,
                        lhs: lv,
                        rhs: rv,
                        is_inplace: *is_aug,
                    },
                    ln,
                    sp,
                );
                self.write_reg(*dst, v, ln, sp);
            }
            Insn::BinOpImm(dst, lhs, op, imm, is_aug) => {
                let lv = self.read_reg(*lhs, ln, sp);
                let rv = self.emit_def(MirOp::Const(Value::int(*imm as i64)), ln, sp);
                let v = self.emit_def(
                    MirOp::BinOp {
                        op: *op,
                        lhs: lv,
                        rhs: rv,
                        is_inplace: *is_aug,
                    },
                    ln,
                    sp,
                );
                self.write_reg(*dst, v, ln, sp);
            }
            Insn::UnaryOp(dst, op, src) => {
                let sv = self.read_reg(*src, ln, sp);
                let v = self.emit_def(MirOp::UnaryOp { op: *op, arg: sv }, ln, sp);
                self.write_reg(*dst, v, ln, sp);
            }
            Insn::LoadGlobal(dst, name_idx) => {
                let v = self.emit_def(
                    MirOp::LoadGlobal {
                        name_idx: *name_idx,
                    },
                    ln,
                    sp,
                );
                self.write_reg(*dst, v, ln, sp);
            }
            Insn::StoreGlobal(name_idx, src) => {
                let sv = self.read_reg(*src, ln, sp);
                self.emit_void(
                    MirOp::StoreGlobal {
                        name_idx: *name_idx,
                        val: sv,
                    },
                    ln,
                    sp,
                );
            }
            Insn::LoadCell(dst, cell_idx) => {
                let v = self.emit_def(
                    MirOp::LoadCell {
                        cell_idx: *cell_idx,
                    },
                    ln,
                    sp,
                );
                self.write_reg(*dst, v, ln, sp);
            }
            Insn::StoreCell(cell_idx, src) => {
                let sv = self.read_reg(*src, ln, sp);
                self.emit_void(
                    MirOp::StoreCell {
                        cell_idx: *cell_idx,
                        val: sv,
                    },
                    ln,
                    sp,
                );
            }
            Insn::GetAttr(dst, obj, name_idx) => {
                let ov = self.read_reg(*obj, ln, sp);
                let v = self.emit_def(
                    MirOp::GetAttr {
                        obj: ov,
                        name_idx: *name_idx,
                    },
                    ln,
                    sp,
                );
                self.write_reg(*dst, v, ln, sp);
            }
            Insn::SetAttr(obj, name_idx, val) => {
                let ov = self.read_reg(*obj, ln, sp);
                let vv = self.read_reg(*val, ln, sp);
                self.emit_void(
                    MirOp::SetAttr {
                        obj: ov,
                        name_idx: *name_idx,
                        val: vv,
                    },
                    ln,
                    sp,
                );
            }
            Insn::GetItem(dst, obj, key) => {
                let ov = self.read_reg(*obj, ln, sp);
                let kv = self.read_reg(*key, ln, sp);
                let v = self.emit_def(MirOp::GetItem { obj: ov, key: kv }, ln, sp);
                self.write_reg(*dst, v, ln, sp);
            }
            Insn::SetItem(obj, key, val) => {
                let ov = self.read_reg(*obj, ln, sp);
                let kv = self.read_reg(*key, ln, sp);
                let vv = self.read_reg(*val, ln, sp);
                self.emit_void(
                    MirOp::SetItem {
                        obj: ov,
                        key: kv,
                        val: vv,
                    },
                    ln,
                    sp,
                );
            }
            Insn::Call(base, argc) | Insn::CallMemo(base, argc) => {
                let is_memo = matches!(insn, Insn::CallMemo(..));
                let callee = self.read_reg(*base, ln, sp);
                let mut args = Vec::with_capacity(*argc as usize);
                for i in 0..(*argc as u32) {
                    args.push(self.read_reg(*base + 1 + i, ln, sp));
                }
                let v = self.emit_def(
                    MirOp::Call {
                        callee,
                        args,
                        is_memo,
                    },
                    ln,
                    sp,
                );
                self.write_reg(*base, v, ln, sp);
            }
            Insn::BuildTuple(dst, base, n_elems) | Insn::BuildList(dst, base, n_elems) => {
                let mut elems = Vec::with_capacity(*n_elems as usize);
                for i in 0..*n_elems {
                    elems.push(self.read_reg(*base + i, ln, sp));
                }
                let op = if matches!(insn, Insn::BuildTuple(..)) {
                    MirOp::BuildTuple(elems)
                } else {
                    MirOp::BuildList(elems)
                };
                let v = self.emit_def(op, ln, sp);
                self.write_reg(*dst, v, ln, sp);
            }
            Insn::DeleteLocal(reg, _, _) => {
                self.emit_void(
                    MirOp::SideEffectInsn {
                        insn: insn.clone(),
                        uses: SmallVec::new(),
                        defs: SmallVec::new(),
                    },
                    ln,
                    sp,
                );
                self.push_def(*reg, None);
            }
            other => {
                let mut read_regs: SmallVec<[Reg; 4]> = SmallVec::new();
                visit_ssa_reads(other, &self.renamer.code.descs, |r| read_regs.push(r));
                let mut uses: SmallVec<[Val; 4]> = SmallVec::with_capacity(read_regs.len());
                for r in read_regs {
                    uses.push(self.read_reg(r, ln, sp));
                }
                let mut write_regs: SmallVec<[Reg; 2]> = SmallVec::new();
                visit_ssa_writes(other, &self.renamer.code.descs, |w| write_regs.push(w));
                let mut defs: SmallVec<[Val; 2]> = SmallVec::with_capacity(write_regs.len());
                for _ in &write_regs {
                    defs.push(self.renamer.mir.fresh_val());
                }
                let single_def = if defs.len() == 1 { Some(defs[0]) } else { None };
                self.emit_raw(
                    single_def,
                    MirOp::SideEffectInsn {
                        insn: other.clone(),
                        uses,
                        defs: defs.clone(),
                    },
                    ln,
                    sp,
                );
                for (&wr, &dv) in write_regs.iter().zip(defs.iter()) {
                    self.write_reg(wr, dv, ln, sp);
                }
            }
        }
    }

    /// Lift the block terminator (or implicit fallthrough / synthetic return).
    pub(super) fn lift_terminator(&mut self, rb: &RawBlock) -> (Terminator, u32, CaretSpan) {
        let Some(tpc) = rb.term_pc else {
            if let Some(edge) = rb.succs.first() {
                let target = edge.target;
                let args = self.renamer.collect_edge_args(target, &[]);
                return (
                    Terminator::Jump {
                        target: BlockId(target as u32),
                        args,
                    },
                    0,
                    (0, 0, 0, 0),
                );
            }
            // Synthetic fall-off-end return None.
            let v = self.emit_def(MirOp::Const(Value::none()), 0, (0, 0, 0, 0));
            return (Terminator::Return(v), 0, (0, 0, 0, 0));
        };

        let t_insn = &self.renamer.code.insns[tpc];
        let ln = self.renamer.lineno_of(tpc);
        let sp = self.renamer.span_of(tpc);

        let term = match t_insn {
            Insn::Jump(_) => {
                let target = rb.succs[0].target;
                Terminator::Jump {
                    target: BlockId(target as u32),
                    args: self.renamer.collect_edge_args(target, &[]),
                }
            }
            Insn::JumpIfFalse(cond_r, _) | Insn::JumpIfTrue(cond_r, _) => {
                let cond = self.read_reg(*cond_r, ln, sp);
                self.make_branch(cond, rb)
            }
            Insn::CmpJumpIfFalse(a, op, b, _) | Insn::CmpJumpIfTrue(a, op, b, _) => {
                let lv = self.read_reg(*a, ln, sp);
                let rv = self.read_reg(*b, ln, sp);
                let cond = self.emit_def(
                    MirOp::BinOp {
                        op: *op,
                        lhs: lv,
                        rhs: rv,
                        is_inplace: false,
                    },
                    ln,
                    sp,
                );
                self.make_branch(cond, rb)
            }
            Insn::CmpJumpIfFalseConst(a, op, c_idx, _)
            | Insn::CmpJumpIfTrueConst(a, op, c_idx, _) => {
                let lv = self.read_reg(*a, ln, sp);
                let val = self.renamer.code.consts[*c_idx as usize].clone();
                let rv = self.emit_def(MirOp::Const(val), ln, sp);
                let cond = self.emit_def(
                    MirOp::BinOp {
                        op: *op,
                        lhs: lv,
                        rhs: rv,
                        is_inplace: false,
                    },
                    ln,
                    sp,
                );
                self.make_branch(cond, rb)
            }
            Insn::Return(r) => Terminator::Return(self.read_reg(*r, ln, sp)),
            Insn::ReturnNone => {
                let v = self.emit_def(MirOp::Const(Value::none()), ln, sp);
                Terminator::Return(v)
            }
            Insn::RaiseValue(r) => Terminator::Raise(Some(self.read_reg(*r, ln, sp))),
            Insn::RaiseReRaise => Terminator::Raise(None),
            Insn::RaiseFrom(a, b) => {
                let av = self.read_reg(*a, ln, sp);
                let bv = self.read_reg(*b, ln, sp);
                Terminator::RaiseSpecial {
                    insn: t_insn.clone(),
                    uses: smallvec::smallvec![av, bv],
                }
            }
            Insn::RaiseAssert(r) if *r == crate::bytecode::NO_ASSERT_MSG => {
                Terminator::RaiseSpecial {
                    insn: t_insn.clone(),
                    uses: SmallVec::new(),
                }
            }
            Insn::RaiseAssert(r) | Insn::RaiseExceptStarResidual(r) => {
                let rv = self.read_reg(*r, ln, sp);
                Terminator::RaiseSpecial {
                    insn: t_insn.clone(),
                    uses: smallvec::smallvec![rv],
                }
            }
            Insn::ForIter(dst_reg, slot, _) => {
                let dst_val = self.renamer.mir.fresh_val();
                let body_t = rb.succs[0].target;
                let exit_t = rb.succs[1].target;
                Terminator::ForIter {
                    slot: *slot,
                    dst: dst_val,
                    body_bb: BlockId(body_t as u32),
                    body_args: self
                        .renamer
                        .collect_edge_args(body_t, &[(*dst_reg, dst_val)]),
                    exit_bb: BlockId(exit_t as u32),
                    exit_args: self.renamer.collect_edge_args(exit_t, &[]),
                }
            }
            Insn::SetupExcept(_) => {
                let try_t = rb.succs[0].target;
                let handler_t = rb.succs[1].target;
                Terminator::SetupExcept {
                    try_bb: BlockId(try_t as u32),
                    try_args: self.renamer.collect_edge_args(try_t, &[]),
                    handler_bb: BlockId(handler_t as u32),
                    handler_args: self.renamer.collect_edge_args(handler_t, &[]),
                }
            }
            Insn::MatchExcept(type_r, _) => {
                let tv = self.read_reg(*type_r, ln, sp);
                let matched_t = rb.succs[0].target;
                let nomatch_t = rb.succs[1].target;
                Terminator::MatchExcept {
                    type_val: tv,
                    matched_bb: BlockId(matched_t as u32),
                    matched_args: self.renamer.collect_edge_args(matched_t, &[]),
                    nomatch_bb: BlockId(nomatch_t as u32),
                    nomatch_args: self.renamer.collect_edge_args(nomatch_t, &[]),
                }
            }
            Insn::MatchExceptStar(desc_idx, _) => {
                let desc = *self.renamer.code.descs[*desc_idx as usize].as_match_except_star();
                let tv = self.read_reg(desc.type_reg, ln, sp);
                let sv = self.read_reg(desc.src_group, ln, sp);
                let matched_def = self.renamer.mir.fresh_val();
                let residual_def = self.renamer.mir.fresh_val();
                let matched_t = rb.succs[0].target;
                let nomatch_t = rb.succs[1].target;
                Terminator::MatchExceptStar {
                    type_val: tv,
                    src_group: sv,
                    matched_def,
                    residual_def,
                    matched_bb: BlockId(matched_t as u32),
                    matched_args: self.renamer.collect_edge_args(
                        matched_t,
                        &[
                            (desc.matched_dst, matched_def),
                            (desc.src_group, residual_def),
                        ],
                    ),
                    nomatch_bb: BlockId(nomatch_t as u32),
                    nomatch_args: self.renamer.collect_edge_args(nomatch_t, &[]),
                }
            }
            _ => unreachable!("unexpected terminator"),
        };

        (term, ln, sp)
    }

    fn make_branch(&self, cond: Val, rb: &RawBlock) -> Terminator {
        let then_t = rb.succs[0].target;
        let else_t = rb.succs[1].target;
        Terminator::Branch {
            cond,
            then_bb: BlockId(then_t as u32),
            then_args: self.renamer.collect_edge_args(then_t, &[]),
            else_bb: BlockId(else_t as u32),
            else_args: self.renamer.collect_edge_args(else_t, &[]),
        }
    }
}
