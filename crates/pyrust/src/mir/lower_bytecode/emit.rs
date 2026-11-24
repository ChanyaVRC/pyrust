//! Bytecode emission driver, constant pool interner, and `lineno_table` /
//! `col_table` stream management.

use std::collections::HashMap;

use crate::ast::CaretSpan;
use crate::bytecode::{Insn, InsnDesc, Reg};
use crate::value::{Value, ValueKind};

use crate::mir::ir::{BlockId, MirFn, MirOp, Terminator, Val, mir_const_exact_eq};
use crate::mir::verify::DominatorTree;

use super::coalesce::can_coalesce_op_into_reg;
use super::de_ssa::{
    EdgeTrampoline, JumpPatch, PatchTarget, TrampolinePlan, edge_copies, emit_parallel_copies,
    patch_jumps, plan_trampolines,
};
use super::regalloc::RegAllocResult;

/// Hashable constant pool interner matching `optimizer/constant_folding`.
#[derive(PartialEq, Eq, Hash)]
enum ConstPoolKey {
    Int(i64),
    BigInt(Vec<u8>),
    FloatBits(u64),
    ComplexBits(u64, u64),
    Str(String),
    Bytes(Vec<u8>),
    Bool(bool),
    None,
}

fn pool_key(val: &Value) -> Option<ConstPoolKey> {
    Some(match val.kind() {
        ValueKind::Int(n) => ConstPoolKey::Int(n),
        ValueKind::BigInt(n) => ConstPoolKey::BigInt(n.to_signed_bytes_be()),
        ValueKind::Float(f) => ConstPoolKey::FloatBits(f.to_bits()),
        ValueKind::Complex(r, i) => ConstPoolKey::ComplexBits(r.to_bits(), i.to_bits()),
        ValueKind::Str(s) => ConstPoolKey::Str(s.to_string()),
        ValueKind::Bytes(b) => ConstPoolKey::Bytes(b.to_vec()),
        ValueKind::Bool(b) => ConstPoolKey::Bool(b),
        ValueKind::None => ConstPoolKey::None,
        _ => return None,
    })
}

pub(super) struct ConstPool {
    consts: Vec<Value>,
    index: HashMap<ConstPoolKey, u16>,
}

impl ConstPool {
    fn from_existing(existing: &[Value]) -> Self {
        let mut index = HashMap::with_capacity(existing.len());
        for (i, v) in existing.iter().enumerate() {
            if let Some(k) = pool_key(v) {
                index.entry(k).or_insert(i as u16);
            }
        }
        Self {
            consts: existing.to_vec(),
            index,
        }
    }

    pub(super) fn intern(&mut self, val: Value) -> u16 {
        if let Some(k) = pool_key(&val) {
            if let Some(&idx) = self.index.get(&k) {
                return idx;
            }
            let idx = self.consts.len() as u16;
            self.index.insert(k, idx);
            self.consts.push(val);
            return idx;
        }
        for (i, existing) in self.consts.iter().enumerate() {
            if mir_const_exact_eq(existing, &val) {
                return i as u16;
            }
        }
        let idx = self.consts.len() as u16;
        self.consts.push(val);
        idx
    }
}

/// Emitted bytecode streams and metadata ready for [`crate::bytecode::FnCode`].
pub(super) struct EmittedBytecode {
    pub(super) insns: Vec<Insn>,
    pub(super) descs: Vec<InsnDesc>,
    pub(super) lineno_table: Vec<u32>,
    pub(super) col_table: Vec<CaretSpan>,
    pub(super) consts: Vec<Value>,
    pub(super) max_reg_used: Reg,
}

/// Emit all basic blocks, parallel-copy trampolines, and patched jumps for `func`.
pub(super) fn emit_mir_bytecode(
    func: &MirFn,
    dom: &DominatorTree,
    alloc: RegAllocResult,
) -> EmittedBytecode {
    let nvals = func.next_val as usize;
    let mut const_val_of: Vec<Option<Value>> = vec![None; nvals];
    for &bb_id in &dom.rpo {
        for inst in &func.blocks[bb_id.0 as usize].instrs {
            if let (Some(def), MirOp::Const(v)) = (inst.def, &inst.op) {
                const_val_of[def.0 as usize] = Some(v.clone());
            }
        }
    }

    let mut elided_const: Vec<bool> = vec![false; nvals];
    for &bb_id in &dom.rpo {
        for inst in &func.blocks[bb_id.0 as usize].instrs {
            if let MirOp::BinOp { lhs, rhs, .. } = inst.op
                && lhs != rhs
                && alloc.use_count[rhs.0 as usize] == 1
                && const_val_of[rhs.0 as usize].is_some()
            {
                elided_const[rhs.0 as usize] = true;
            }
        }
    }

    let tramps = plan_trampolines(func, dom, &alloc.val_to_reg);
    let cycle_tmp = alloc.ssa_reg_end;
    let mut emitter = BytecodeEmitter {
        func,
        dom,
        val_to_reg: alloc.val_to_reg,
        end_pos: alloc.end_pos,
        use_count: alloc.use_count,
        const_val_of,
        elided_const,
        pool: ConstPool::from_existing(&func.consts),
        cycle_tmp,
        scratch_base: alloc.ssa_reg_end + 1,
        max_reg_used: alloc.ssa_reg_end,
        insns: Vec::new(),
        descs: Vec::new(),
        lineno_table: Vec::new(),
        col_table: Vec::new(),
        block_pc: vec![usize::MAX; func.blocks.len()],
        patches: Vec::new(),
        tramps,
        active_override: None,
    };

    emitter.emit_all_blocks();
    patch_jumps(
        &mut emitter.insns,
        emitter.patches,
        &emitter.block_pc,
        &emitter.tramps.tramp_pc,
    );

    EmittedBytecode {
        insns: emitter.insns,
        descs: emitter.descs,
        lineno_table: emitter.lineno_table,
        col_table: emitter.col_table,
        consts: emitter.pool.consts,
        max_reg_used: emitter.max_reg_used,
    }
}

pub(super) struct BytecodeEmitter<'a> {
    pub(super) func: &'a MirFn,
    dom: &'a DominatorTree,
    pub(super) val_to_reg: Vec<Reg>,
    pub(super) end_pos: Vec<usize>,
    pub(super) use_count: Vec<u32>,
    pub(super) const_val_of: Vec<Option<Value>>,
    elided_const: Vec<bool>,
    pub(super) pool: ConstPool,
    cycle_tmp: Reg,
    pub(super) scratch_base: Reg,
    pub(super) max_reg_used: Reg,
    pub(super) insns: Vec<Insn>,
    pub(super) descs: Vec<InsnDesc>,
    pub(super) lineno_table: Vec<u32>,
    pub(super) col_table: Vec<CaretSpan>,
    block_pc: Vec<usize>,
    pub(super) patches: Vec<JumpPatch>,
    tramps: TrampolinePlan,
    active_override: Option<(Val, Reg)>,
}

impl BytecodeEmitter<'_> {
    #[inline]
    pub(super) fn reg_of(&self, v: Val) -> Reg {
        if let Some((ov_val, ov_reg)) = self.active_override
            && ov_val == v
        {
            return ov_reg;
        }
        self.val_to_reg[v.0 as usize]
    }

    #[inline]
    pub(super) fn emit(&mut self, insn: Insn, ln: u32, sp: CaretSpan) -> usize {
        let pc = self.insns.len();
        self.insns.push(insn);
        self.lineno_table.push(ln);
        self.col_table.push(sp);
        pc
    }

    #[inline]
    fn emit_jump_to(&mut self, target: PatchTarget, ln: u32, sp: CaretSpan) {
        let pc = self.emit(Insn::Jump(0), ln, sp);
        self.patches.push(JumpPatch {
            insn_pc: pc,
            target,
        });
    }

    pub(super) fn emit_copies(&mut self, copies: &[(Reg, Reg)], ln: u32, sp: CaretSpan) {
        if !copies.is_empty() {
            self.max_reg_used = self.max_reg_used.max(self.cycle_tmp);
            emit_parallel_copies(
                copies,
                self.cycle_tmp,
                &mut self.insns,
                &mut self.lineno_table,
                &mut self.col_table,
                ln,
                sp,
            );
        }
    }

    pub(super) fn edge_copies(&self, target: BlockId, args: &[Val]) -> Vec<(Reg, Reg)> {
        edge_copies(self.func, &self.val_to_reg, target, args)
    }

    pub(super) fn target_or_trampoline(
        &mut self,
        target: BlockId,
        copies: Vec<(Reg, Reg)>,
        order_idx: usize,
        local_back_tramps: &mut Vec<(usize, EdgeTrampoline)>,
        ln: u32,
        sp: CaretSpan,
    ) -> PatchTarget {
        if copies.is_empty() {
            return PatchTarget::Block(target);
        }
        let tid = self.tramps.tramp_pc.len();
        self.tramps.tramp_pc.push(usize::MAX);
        let tr = EdgeTrampoline {
            copies,
            target,
            lineno: ln,
            col_span: sp,
        };
        if self.tramps.rpo_pos[target.0 as usize] > order_idx {
            self.tramps.fwd_tramps_before[target.0 as usize].push((tid, tr));
        } else {
            local_back_tramps.push((tid, tr));
        }
        PatchTarget::Trampoline(tid)
    }

    pub(super) fn emit_fallthrough_and_jump(
        &mut self,
        target: BlockId,
        copies: &[(Reg, Reg)],
        next_bb: Option<BlockId>,
        ln: u32,
        sp: CaretSpan,
    ) {
        self.emit_copies(copies, ln, sp);
        if next_bb != Some(target) {
            self.emit_jump_to(PatchTarget::Block(target), ln, sp);
        }
    }

    fn emit_all_blocks(&mut self) {
        let rpo_len = self.dom.rpo.len();
        let mut pos_cursor = 0usize;

        for (order_idx, &bb_id) in self.dom.rpo.iter().enumerate() {
            let bi = bb_id.0 as usize;
            let block = &self.func.blocks[bi];

            // Emit any forward-edge trampolines targeting `bb_id` directly before it
            // (the final trampoline falls straight through into `bb_id`).
            let incoming_fwd = std::mem::take(&mut self.tramps.fwd_tramps_before[bi]);
            let fwd_count = incoming_fwd.len();
            for (ti, (tid, tr)) in incoming_fwd.into_iter().enumerate() {
                self.tramps.tramp_pc[tid] = self.insns.len();
                self.emit_copies(&tr.copies, tr.lineno, tr.col_span);
                if ti + 1 < fwd_count {
                    self.emit_jump_to(PatchTarget::Block(tr.target), tr.lineno, tr.col_span);
                }
            }

            let next_bb = self.dom.rpo.get(order_idx + 1).copied().filter(|&nb_id| {
                !self.tramps.has_fwd_tramp_before[nb_id.0 as usize]
                    && !self.tramps.has_back_tramp_after[bi]
            });
            let mut local_back_tramps: Vec<(usize, EdgeTrampoline)> = Vec::new();
            self.block_pc[bi] = self.insns.len();
            pos_cursor += 1; // block entry position
            let mut pending_override: Option<(Val, Reg)> = None;
            self.active_override = None;

            let mut i = 0usize;
            while i < block.instrs.len() {
                let inst_pos = pos_cursor;
                let inst = &block.instrs[i];
                let is_noop_load_local = matches!(
                    (inst.def, &inst.op),
                    (Some(d), MirOp::LoadLocal(lr)) if self.val_to_reg[d.0 as usize] == *lr
                );
                if !is_noop_load_local {
                    self.active_override = pending_override.take();
                }

                // Single-use `StoreLocal` coalescing: write directly into `reg`
                // when the sole consumer is an immediately following `StoreLocal`.
                let coalesced_target: Option<Reg> = if let Some(def) = inst.def
                    && self.use_count[def.0 as usize] == 1
                    && let Some(next_inst) = block.instrs.get(i + 1)
                    && let MirOp::StoreLocal { reg, val } = next_inst.op
                    && val == def
                    && can_coalesce_op_into_reg(&inst.op)
                {
                    Some(reg)
                } else {
                    None
                };

                // Elide `v = Const(...)` when consumed exclusively by an immediate
                // `BinOpImm`/`BinOpConst` or a trailing `Return(None)`.
                if let (Some(def), MirOp::Const(val)) = (inst.def, &inst.op)
                    && coalesced_target.is_none()
                    && (self.elided_const[def.0 as usize]
                        || (val.is_none()
                            && self.use_count[def.0 as usize] == 1
                            && matches!(block.term, Terminator::Return(rv) if rv == def)
                            && i + 1 == block.instrs.len()))
                {
                    i += 1;
                    pos_cursor += 1;
                    continue;
                }

                self.emit_inst(
                    block,
                    i,
                    inst,
                    inst_pos,
                    coalesced_target,
                    &mut pending_override,
                );

                if coalesced_target.is_some() {
                    i += 2;
                    pos_cursor += 2;
                } else {
                    i += 1;
                    pos_cursor += 1;
                }
            }
            pos_cursor += 1; // terminator position

            self.emit_terminator(block, order_idx, rpo_len, next_bb, &mut local_back_tramps);

            // Emit back-edge trampolines originating from `bb_id` directly after it.
            for (tid, tr) in local_back_tramps {
                self.tramps.tramp_pc[tid] = self.insns.len();
                self.emit_copies(&tr.copies, tr.lineno, tr.col_span);
                self.emit_jump_to(PatchTarget::Block(tr.target), tr.lineno, tr.col_span);
            }
        }
    }
}
