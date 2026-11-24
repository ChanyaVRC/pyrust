//! Instruction-level bytecode emission (`MirInst` -> register `Insn`s).

use smallvec::SmallVec;

use crate::ast::CaretSpan;
use crate::bytecode::{Insn, Reg};
use crate::value::ValueKind;

use crate::mir::insn_info::materialize_side_effect_insn;
use crate::mir::ir::{MirBlock, MirInst, MirOp, Val};

use super::emit::BytecodeEmitter;

impl BytecodeEmitter<'_> {
    pub(super) fn emit_inst(
        &mut self,
        block: &MirBlock,
        i: usize,
        inst: &MirInst,
        inst_pos: usize,
        coalesced_target: Option<Reg>,
        pending_override: &mut Option<(Val, Reg)>,
    ) {
        let ln = inst.lineno;
        let sp = inst.col_span;
        let dst_reg = coalesced_target
            .or_else(|| inst.def.map(|d| self.reg_of(d)))
            .unwrap_or(0);

        match &inst.op {
            MirOp::Const(val) => {
                if val.is_none() {
                    self.emit(Insn::LoadNone(dst_reg), ln, sp);
                } else {
                    let c_idx = self.pool.intern(val.clone());
                    self.emit(Insn::LoadConst(dst_reg, c_idx), ln, sp);
                }
            }
            MirOp::Copy(src) => {
                let sr = self.reg_of(*src);
                if dst_reg != sr {
                    self.emit(Insn::Move(dst_reg, sr), ln, sp);
                }
            }
            MirOp::BinOp {
                op,
                lhs,
                rhs,
                is_inplace,
            } => {
                let lr = self.reg_of(*lhs);
                let op_insn = if *lhs != *rhs
                    && let Some(rv) = self.const_val_of[rhs.0 as usize].clone()
                {
                    if *is_inplace
                        && let ValueKind::Int(n) = rv.kind()
                        && (i16::MIN as i64..=i16::MAX as i64).contains(&n)
                    {
                        Insn::BinOpImm(dst_reg, lr, *op, n as i16, true)
                    } else {
                        let c_idx = self.pool.intern(rv);
                        Insn::BinOpConst(dst_reg, lr, *op, c_idx, *is_inplace)
                    }
                } else {
                    let rr = self.reg_of(*rhs);
                    if *is_inplace {
                        Insn::BinOpInPlace(dst_reg, lr, *op, rr)
                    } else {
                        Insn::BinOp(dst_reg, lr, *op, rr)
                    }
                };
                self.emit(op_insn, ln, sp);
            }
            MirOp::UnaryOp { op, arg } => {
                self.emit(Insn::UnaryOp(dst_reg, *op, self.reg_of(*arg)), ln, sp);
            }
            MirOp::LoadLocal(local_reg) => {
                if dst_reg != *local_reg {
                    self.emit(Insn::Move(dst_reg, *local_reg), ln, sp);
                }
            }
            MirOp::StoreLocal { reg, val } => {
                let sr = self.reg_of(*val);
                if *reg != sr {
                    self.emit(Insn::Move(*reg, sr), ln, sp);
                }
            }
            MirOp::LoadGlobal { name_idx } => {
                self.emit(Insn::LoadGlobal(dst_reg, *name_idx), ln, sp);
            }
            MirOp::StoreGlobal { name_idx, val } => {
                self.emit(Insn::StoreGlobal(*name_idx, self.reg_of(*val)), ln, sp);
            }
            MirOp::LoadCell { cell_idx } => {
                self.emit(Insn::LoadCell(dst_reg, *cell_idx), ln, sp);
            }
            MirOp::StoreCell { cell_idx, val } => {
                self.emit(Insn::StoreCell(*cell_idx, self.reg_of(*val)), ln, sp);
            }
            MirOp::GetAttr { obj, name_idx } => {
                self.emit(Insn::GetAttr(dst_reg, self.reg_of(*obj), *name_idx), ln, sp);
            }
            MirOp::SetAttr { obj, name_idx, val } => {
                self.emit(
                    Insn::SetAttr(self.reg_of(*obj), *name_idx, self.reg_of(*val)),
                    ln,
                    sp,
                );
            }
            MirOp::GetItem { obj, key } => {
                self.emit(
                    Insn::GetItem(dst_reg, self.reg_of(*obj), self.reg_of(*key)),
                    ln,
                    sp,
                );
            }
            MirOp::SetItem { obj, key, val } => {
                self.emit(
                    Insn::SetItem(self.reg_of(*obj), self.reg_of(*key), self.reg_of(*val)),
                    ln,
                    sp,
                );
            }
            MirOp::Call {
                callee,
                args,
                is_memo,
            } => {
                self.emit_call(
                    block,
                    i,
                    inst,
                    inst_pos,
                    *callee,
                    args,
                    *is_memo,
                    dst_reg,
                    coalesced_target,
                    pending_override,
                );
            }
            MirOp::BuildTuple(elems) => self.emit_seq_build(elems, dst_reg, true, ln, sp),
            MirOp::BuildList(elems) => self.emit_seq_build(elems, dst_reg, false, ln, sp),
            MirOp::SideEffectInsn { insn, uses, defs } => {
                let effective_defs: SmallVec<[Reg; 2]> = if let Some(cr) = coalesced_target {
                    smallvec::smallvec![cr]
                } else {
                    defs.iter().map(|&d| self.reg_of(d)).collect()
                };
                let use_regs: SmallVec<[Reg; 4]> = uses.iter().map(|&u| self.reg_of(u)).collect();
                materialize_side_effect_insn(
                    insn,
                    &self.func.descs,
                    &mut self.descs,
                    &use_regs,
                    &effective_defs,
                    self.scratch_base,
                    &mut self.max_reg_used,
                    &mut self.insns,
                    &mut self.lineno_table,
                    &mut self.col_table,
                    ln,
                    sp,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_call(
        &mut self,
        block: &MirBlock,
        i: usize,
        inst: &MirInst,
        inst_pos: usize,
        callee: Val,
        args: &[Val],
        is_memo: bool,
        dst_reg: Reg,
        coalesced_target: Option<Reg>,
        pending_override: &mut Option<(Val, Reg)>,
    ) {
        let ln = inst.lineno;
        let sp = inst.col_span;
        let callee_r = self.reg_of(callee);
        let argc = args.len() as u8;
        let can_reuse_in_place = callee_r >= self.func.num_locals
            && self.end_pos[callee.0 as usize] <= inst_pos
            && args
                .iter()
                .enumerate()
                .all(|(ai, &a)| self.reg_of(a) == callee_r + 1 + ai as u32);

        let base = if can_reuse_in_place {
            callee_r
        } else {
            let b = self.scratch_base;
            self.max_reg_used = self.max_reg_used.max(b + argc as u32);
            // If callee was loaded by an immediately preceding single-use
            // `LoadGlobal(callee_r, name_idx)`, retarget it directly into `b`.
            let retargeted = self.use_count[callee.0 as usize] == 1
                && matches!(self.insns.last(), Some(Insn::LoadGlobal(d, _)) if *d == callee_r);
            if retargeted {
                if let Some(Insn::LoadGlobal(d, _)) = self.insns.last_mut() {
                    *d = b;
                }
            } else {
                self.emit(Insn::Move(b, callee_r), ln, sp);
            }
            for (ai, &arg) in args.iter().enumerate() {
                self.emit(Insn::Move(b + 1 + ai as u32, self.reg_of(arg)), ln, sp);
            }
            b
        };

        let call_insn = if is_memo {
            Insn::CallMemo(base, argc)
        } else {
            Insn::Call(base, argc)
        };
        self.emit(call_insn, ln, sp);

        if let Some(call_def) = inst.def
            && self.use_count[call_def.0 as usize] > 0
            && dst_reg != base
        {
            let next_emitting = block.instrs[i + 1..].iter().find(|ni| {
                !matches!(
                    (ni.def, &ni.op),
                    (Some(d), MirOp::LoadLocal(lr)) if self.val_to_reg[d.0 as usize] == *lr
                )
            });
            let can_forward_base = coalesced_target.is_none()
                && self.use_count[call_def.0 as usize] == 1
                && next_emitting.is_some_and(|ni| {
                    let mut uses_call = false;
                    ni.for_each_use(|u| {
                        if u == call_def {
                            uses_call = true;
                        }
                    });
                    uses_call
                        && matches!(
                            ni.op,
                            MirOp::BinOp { .. }
                                | MirOp::UnaryOp { .. }
                                | MirOp::StoreLocal { .. }
                                | MirOp::StoreGlobal { .. }
                                | MirOp::StoreCell { .. }
                                | MirOp::GetAttr { .. }
                                | MirOp::SetAttr { .. }
                                | MirOp::GetItem { .. }
                                | MirOp::SetItem { .. }
                        )
                });
            if can_forward_base {
                *pending_override = Some((call_def, base));
            } else {
                self.emit(Insn::Move(dst_reg, base), ln, sp);
            }
        }
    }

    fn emit_seq_build(
        &mut self,
        elems: &[Val],
        dst_reg: Reg,
        is_tuple: bool,
        ln: u32,
        sp: CaretSpan,
    ) {
        let n = elems.len() as u32;
        let base = self.scratch_base;
        if n > 0 {
            self.max_reg_used = self.max_reg_used.max(base + n - 1);
        }
        for (ei, &elem) in elems.iter().enumerate() {
            self.emit(Insn::Move(base + ei as u32, self.reg_of(elem)), ln, sp);
        }
        let insn = if is_tuple {
            Insn::BuildTuple(dst_reg, base, n)
        } else {
            Insn::BuildList(dst_reg, base, n)
        };
        self.emit(insn, ln, sp);
    }
}
