//! SSA MIR -> Register Bytecode (`FnCode`) backend.
//!
//! Lowers an optimized SSA [`MirFn`] back into executable [`FnCode`]:
//! 1. Computes exact SSA liveness (`val_live_out`) and live intervals over RPO
//!    block order, extending intervals across loop back-edges so no live SSA
//!    value can ever be clobbered.
//! 2. Allocates temporary registers (`num_locals..`) via linear scan, plus
//!    single-use `StoreLocal` coalescing directly into canonical `0..num_locals`.
//! 3. De-SSAs Cranelift-style block parameters via cycle-safe parallel-copy
//!    sequentialization (breaking swap cycles like `(a, b) <- (b, a)` with a
//!    dedicated scratch register) and critical-edge trampolines when needed.
//! 4. Emits exact `lineno_table` and `col_table` entries directly from every
//!    `MirInst` and block terminator.

use std::collections::{HashMap, HashSet};

use smallvec::SmallVec;

use crate::ast::CaretSpan;
use crate::bytecode::{
    AttrCacheEntry, BinOpCacheEntry, FnCode, GlobalCacheEntry, Insn, KwCallCacheEntry, KwCallName,
    MAX_FRAME_REGS, NO_KWARGS, Reg,
};
use crate::value::{Value, ValueKind};

use super::ir::{BlockId, MirFn, MirOp, Terminator, Val, empty_cache_vec, mir_const_exact_eq};
use super::verify::DominatorTree;

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

struct ConstPool {
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

    fn intern(&mut self, val: Value) -> u16 {
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

/// Allocate physical registers (`num_locals..`) for all SSA [`Val`]s via
/// exact liveness + linear scan, with zero-copy coalescing for:
/// - `LoadLocal(r)` when `r` is unmodified during the value's live interval,
/// - `ForIter::dst` whose sole use is a leading `StoreLocal` in `body_bb`.
///
/// Returns `(val_to_reg, end_pos, max_ssa_reg_plus_one)`.
fn allocate_ssa_registers(func: &MirFn, dom: &DominatorTree) -> (Vec<Reg>, Vec<usize>, Reg) {
    let nvals = func.next_val as usize;
    let nb = func.blocks.len();

    // 1. Exact backward SSA liveness per block (live-in / live-out sets).
    let mut live_in: Vec<HashSet<Val>> = vec![HashSet::new(); nb];
    let mut live_out: Vec<HashSet<Val>> = vec![HashSet::new(); nb];

    for _ in 0..(nb * 4 + 16) {
        let mut changed = false;
        for &bb_id in dom.rpo.iter().rev() {
            let bi = bb_id.0 as usize;
            let block = &func.blocks[bi];

            // Live-out = union over successors of (live_in[succ] \ params[succ])
            // plus all edge args passed from `bi` to `succ`.
            let mut out_set: HashSet<Val> = HashSet::new();
            block.term.for_each_edge(|succ, args| {
                let si = succ.0 as usize;
                let succ_params: HashSet<Val> = func.blocks[si].params.iter().copied().collect();
                for &v in &live_in[si] {
                    if !succ_params.contains(&v) {
                        out_set.insert(v);
                    }
                }
                for &a in args {
                    out_set.insert(a);
                }
            });
            live_out[bi] = out_set.clone();

            // Walk backward through terminator and instructions.
            let mut cur = out_set;
            block.term.for_each_def(|d| {
                cur.remove(&d);
            });
            block.term.for_each_use(|u| {
                cur.insert(u);
            });
            for inst in block.instrs.iter().rev() {
                inst.for_each_def(|d| {
                    cur.remove(&d);
                });
                inst.for_each_use(|u| {
                    cur.insert(u);
                });
            }
            // Note: block params ARE live-in to `bi` (needed for incoming edge
            // parallel copies), so keep them in `live_in[bi]`!
            for &p in &block.params {
                cur.insert(p);
            }

            if cur != live_in[bi] {
                live_in[bi] = cur;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // 2. Assign linear positions in RPO order, compute `[start, end]` intervals,
    //    and record every write position to canonical local registers `0..num_locals`.
    let mut start_pos: Vec<usize> = vec![usize::MAX; nvals];
    let mut end_pos: Vec<usize> = vec![0; nvals];
    let mut block_start: Vec<usize> = vec![0; nb];
    let mut block_end: Vec<usize> = vec![0; nb];
    let mut use_count: Vec<u32> = vec![0; nvals];
    let mut local_write_positions: Vec<Vec<usize>> = vec![Vec::new(); func.num_locals as usize];
    let mut barrier_positions: Vec<usize> = Vec::new();
    let aliased_locals = !func.is_function_scope;

    let mut cursor = 0usize;
    for &bb_id in &dom.rpo {
        let bi = bb_id.0 as usize;
        let block = &func.blocks[bi];
        block_start[bi] = cursor;
        for &p in &block.params {
            start_pos[p.0 as usize] = start_pos[p.0 as usize].min(cursor);
            end_pos[p.0 as usize] = end_pos[p.0 as usize].max(cursor);
        }
        cursor += 1;

        for inst in &block.instrs {
            let pos = cursor;
            inst.for_each_use(|u| {
                use_count[u.0 as usize] += 1;
                end_pos[u.0 as usize] = end_pos[u.0 as usize].max(pos);
            });
            inst.for_each_def(|d| {
                start_pos[d.0 as usize] = start_pos[d.0 as usize].min(pos);
                end_pos[d.0 as usize] = end_pos[d.0 as usize].max(pos);
            });
            match &inst.op {
                MirOp::LoadLocal(_) => {}
                MirOp::StoreLocal { reg, .. } => {
                    if (*reg as usize) < local_write_positions.len() {
                        local_write_positions[*reg as usize].push(pos);
                    }
                }
                MirOp::SideEffectInsn {
                    insn: Insn::DeleteLocal(reg, _, _),
                    ..
                } => {
                    if (*reg as usize) < local_write_positions.len() {
                        local_write_positions[*reg as usize].push(pos);
                    }
                }
                op => {
                    if aliased_locals && mir_op_may_invalidate_locals(op) {
                        barrier_positions.push(pos);
                    }
                }
            }
            cursor += 1;
        }

        let tpos = cursor;
        block.term.for_each_use(|u| {
            use_count[u.0 as usize] += 1;
            end_pos[u.0 as usize] = end_pos[u.0 as usize].max(tpos);
        });
        block.term.for_each_def(|d| {
            start_pos[d.0 as usize] = start_pos[d.0 as usize].min(tpos);
            end_pos[d.0 as usize] = end_pos[d.0 as usize].max(tpos);
        });
        block_end[bi] = tpos;
        cursor += 1;
    }

    // Extend intervals for every value live across block boundaries (handles
    // loops, back-edges, and arbitrary merges cleanly).
    for &bb_id in &dom.rpo {
        let bi = bb_id.0 as usize;
        for &v in &live_out[bi] {
            let vi = v.0 as usize;
            if start_pos[vi] != usize::MAX {
                end_pos[vi] = end_pos[vi].max(block_end[bi]);
            }
        }
        for &v in &live_in[bi] {
            let vi = v.0 as usize;
            if start_pos[vi] != usize::MAX {
                start_pos[vi] = start_pos[vi].min(block_start[bi]);
                end_pos[vi] = end_pos[vi].max(block_start[bi]);
            }
        }
    }

    // Pre-assign `val_to_reg` for zero-copy `LoadLocal(r)` values whenever `R[r]`
    // cannot be clobbered on any CFG path between the `LoadLocal(r)` and a use
    // of the value (and for `ForIter::dst` whose sole use is a leading
    // `StoreLocal(r, dst)`).
    let mut val_to_reg: Vec<Reg> = vec![func.num_locals; nvals];
    let mut preassigned: Vec<bool> = vec![false; nvals];

    // Record `(def_bb, def_inst_idx)` for every `LoadLocal(r)`.
    let mut load_local_def_loc: Vec<Option<(usize, usize, Reg)>> = vec![None; nvals];
    for &bb_id in &dom.rpo {
        let bi = bb_id.0 as usize;
        for (ii, inst) in func.blocks[bi].instrs.iter().enumerate() {
            if let (Some(def), MirOp::LoadLocal(r)) = (inst.def, &inst.op) {
                load_local_def_loc[def.0 as usize] = Some((bi, ii, *r));
            }
        }
    }

    let inst_clobbers_local = |op: &MirOp, r: Reg| -> bool {
        match op {
            MirOp::StoreLocal { reg, .. } => *reg == r,
            MirOp::SideEffectInsn {
                insn: Insn::DeleteLocal(reg, _, _),
                ..
            } => *reg == r,
            other => aliased_locals && mir_op_may_invalidate_locals(other),
        }
    };

    for vi in 0..nvals {
        let Some((def_bi, def_ii, r)) = load_local_def_loc[vi] else {
            continue;
        };
        if (r as usize) >= local_write_positions.len() {
            continue;
        }
        let v = Val(vi as u32);
        // Quick exit when `r` is never written anywhere in the function and no
        // aliased-local barrier exists.
        if local_write_positions[r as usize].is_empty() && barrier_positions.is_empty() {
            val_to_reg[vi] = r;
            preassigned[vi] = true;
            continue;
        }
        // Exact per-block check along every block where `v` is live:
        // In `def_bi`, scan after `def_ii`; in any other block where `v` is in
        // `live_in`, scan from instruction 0.  If `r` is clobbered BEFORE a
        // subsequent use of `v` in that block (or while `v` is in `live_out` of
        // that block), `v` cannot alias `R[r]`.
        let mut safe = true;
        for &bb_id in &dom.rpo {
            let bi = bb_id.0 as usize;
            let start_ii = if bi == def_bi {
                def_ii + 1
            } else if live_in[bi].contains(&v) {
                0
            } else {
                continue;
            };
            let block = &func.blocks[bi];
            let mut clobbered = false;
            for inst in &block.instrs[start_ii..] {
                let mut used_here = false;
                inst.for_each_use(|u| {
                    if u == v {
                        used_here = true;
                    }
                });
                if used_here && clobbered {
                    safe = false;
                    break;
                }
                if inst_clobbers_local(&inst.op, r) {
                    clobbered = true;
                }
            }
            if !safe {
                break;
            }
            if clobbered {
                let mut used_in_term = false;
                block.term.for_each_use(|u| {
                    if u == v {
                        used_in_term = true;
                    }
                });
                if used_in_term || live_out[bi].contains(&v) {
                    safe = false;
                    break;
                }
            }
        }
        if safe {
            val_to_reg[vi] = r;
            preassigned[vi] = true;
        }
    }

    // Coalesce `ForIter::dst` directly into the loop variable when `body_bb`
    // immediately stores it via `StoreLocal { reg, val: dst }` as its sole use.
    for &bb_id in &dom.rpo {
        let bi = bb_id.0 as usize;
        if let Terminator::ForIter { dst, body_bb, .. } = func.blocks[bi].term
            && use_count[dst.0 as usize] == 1
            && let Some(first_inst) = func.blocks[body_bb.0 as usize].instrs.first()
            && let MirOp::StoreLocal { reg, val } = first_inst.op
            && val == dst
        {
            val_to_reg[dst.0 as usize] = reg;
            preassigned[dst.0 as usize] = true;
        }
    }

    // Coalesce named-local block parameters (and their single-block defining
    // edge args) directly into canonical `R[r]` when every incoming edge
    // already writes `StoreLocal { reg: r, val: arg }` and `r` is never
    // clobbered while the param is live!
    let mut stored_to_local: Vec<Option<Reg>> = vec![None; nvals];
    for &bb_id in &dom.rpo {
        for inst in &func.blocks[bb_id.0 as usize].instrs {
            if let MirOp::StoreLocal { reg, val } = inst.op {
                stored_to_local[val.0 as usize] = Some(reg);
            }
        }
    }
    for &bb_id in &dom.rpo {
        let bi = bb_id.0 as usize;
        for (pi, &param) in func.blocks[bi].params.iter().enumerate() {
            let mut common_reg: Option<Reg> = None;
            let mut all_match = true;
            let mut edge_args: SmallVec<[Val; 4]> = SmallVec::new();
            for &pred in &func.blocks[bi].preds {
                func.blocks[pred.0 as usize]
                    .term
                    .for_each_edge(|target, args| {
                        if target == bb_id
                            && let Some(&arg) = args.get(pi)
                        {
                            if arg == param {
                                return;
                            }
                            edge_args.push(arg);
                            match (common_reg, stored_to_local[arg.0 as usize]) {
                                (None, Some(r)) => common_reg = Some(r),
                                (Some(prev), Some(r)) if prev == r => {}
                                _ => all_match = false,
                            }
                        }
                    });
            }
            if !all_match {
                continue;
            }
            let Some(r) = common_reg else {
                continue;
            };
            if (r as usize) >= local_write_positions.len() {
                continue;
            }
            // Verify `r` is not clobbered in `bi` (or any block where `param` is
            // live-in) before `param`'s last use!
            let mut safe = true;
            for &b_id in &dom.rpo {
                let b_idx = b_id.0 as usize;
                if b_idx != bi && !live_in[b_idx].contains(&param) {
                    continue;
                }
                let block = &func.blocks[b_idx];
                let mut clobbered = false;
                for inst in &block.instrs {
                    let mut used_here = false;
                    inst.for_each_use(|u| {
                        if u == param {
                            used_here = true;
                        }
                    });
                    if used_here && clobbered {
                        safe = false;
                        break;
                    }
                    if inst_clobbers_local(&inst.op, r) {
                        clobbered = true;
                    }
                }
                if !safe {
                    break;
                }
                if clobbered {
                    let mut used_in_term = false;
                    block.term.for_each_use(|u| {
                        if u == param {
                            used_in_term = true;
                        }
                    });
                    if used_in_term || live_out[b_idx].contains(&param) {
                        safe = false;
                        break;
                    }
                }
            }
            if safe {
                val_to_reg[param.0 as usize] = r;
                preassigned[param.0 as usize] = true;
                // Also coalesce each edge arg into `r` if its only uses are
                // `StoreLocal(r, arg)` and this phi edge (and `r` isn't clobbered
                // between its def and the end of its block).
                for arg in edge_args {
                    if !preassigned[arg.0 as usize] && use_count[arg.0 as usize] == 2 {
                        val_to_reg[arg.0 as usize] = r;
                        preassigned[arg.0 as usize] = true;
                    }
                }
            }
        }
    }

    // Coalesce `Call` result into its dying temp `callee` register when the
    // callee and args already form a contiguous window, OR when `Call` is
    // followed by a single consumer (`GetIter`, `StoreLocal`, etc.) so
    // `range(N)` emits `LoadGlobal; LoadConst; Call; GetIter` with ZERO extra Moves!
    let mut call_reused_base: Vec<Option<Val>> = vec![None; nvals];
    for &bb_id in &dom.rpo {
        let bi = bb_id.0 as usize;
        let block = &func.blocks[bi];
        for (ii, inst) in block.instrs.iter().enumerate() {
            if let (Some(def), MirOp::Call { callee, .. }) = (inst.def, &inst.op) {
                let pos = block_start[bi] + 1 + ii;
                if end_pos[callee.0 as usize] <= pos && !preassigned[callee.0 as usize] {
                    call_reused_base[def.0 as usize] = Some(*callee);
                }
            }
        }
    }

    // 3. Linear-scan allocation over remaining `[start, end]` intervals.
    let mut intervals: Vec<(usize, usize, Val)> = Vec::new();
    for vi in 0..nvals {
        if start_pos[vi] != usize::MAX && !preassigned[vi] {
            intervals.push((start_pos[vi], end_pos[vi], Val(vi as u32)));
        }
    }
    intervals.sort_unstable_by_key(|&(s, e, v)| (s, e, v.0));

    let mut active: Vec<(usize, Reg)> = Vec::new(); // (end, reg)
    let mut free_regs: Vec<Reg> = Vec::new();
    let mut next_reg: Reg = func.num_locals;

    for (s, e, v) in intervals {
        // Expire intervals that ended strictly before `s`.
        let mut i = 0;
        while i < active.len() {
            if active[i].0 < s {
                let (_, r) = active.swap_remove(i);
                free_regs.push(r);
            } else {
                i += 1;
            }
        }
        // If `v` is the result of a `Call` whose `callee` died right at `s`,
        // prefer `callee`'s register (`ae == s` is `callee` itself, which is
        // read before `Call` writes its return value into `cr`!).
        let preferred = call_reused_base[v.0 as usize].and_then(|callee| {
            let cr = val_to_reg[callee.0 as usize];
            if cr >= func.num_locals && !active.iter().any(|&(ae, ar)| ar == cr && ae > s) {
                Some(cr)
            } else {
                None
            }
        });
        let reg = if let Some(pref) = preferred {
            // Remove any expiring `(s, pref)` entry so `pref` is not double-freed.
            active.retain(|&(ae, ar)| !(ar == pref && ae <= s));
            if let Some(pos) = free_regs.iter().position(|&r| r == pref) {
                free_regs.swap_remove(pos);
            }
            pref
        } else if let Some((best_idx, &best_r)) =
            free_regs.iter().enumerate().min_by_key(|&(_, &r)| r)
        {
            free_regs.swap_remove(best_idx);
            best_r
        } else {
            let r = next_reg;
            next_reg += 1;
            r
        };
        val_to_reg[v.0 as usize] = reg;
        active.push((e, reg));
    }

    (val_to_reg, end_pos, next_reg)
}

#[inline]
fn mir_op_may_invalidate_locals(op: &MirOp) -> bool {
    !matches!(
        op,
        MirOp::Const(_)
            | MirOp::Copy(_)
            | MirOp::LoadLocal(_)
            | MirOp::LoadGlobal { .. }
            | MirOp::LoadCell { .. }
            | MirOp::BuildTuple(_)
            | MirOp::BuildList(_)
    )
}

/// Resolve a set of parallel register copies `(dst, src)` into a cycle-safe
/// sequence of `Insn::Move` instructions using `cycle_tmp` to break swap cycles
/// (e.g. `a, b = b, a`).
pub(crate) fn emit_parallel_copies(
    copies: &[(Reg, Reg)],
    cycle_tmp: Reg,
    out: &mut Vec<Insn>,
    linenos: &mut Vec<u32>,
    cols: &mut Vec<CaretSpan>,
    lineno: u32,
    col_span: CaretSpan,
) {
    let mut pending: SmallVec<[(Reg, Reg); 8]> =
        copies.iter().copied().filter(|&(d, s)| d != s).collect();
    let mut push_move = |dst: Reg, src: Reg| {
        if dst != src {
            out.push(Insn::Move(dst, src));
            linenos.push(lineno);
            cols.push(col_span);
        }
    };

    while !pending.is_empty() {
        // Find a copy whose `dst` is NOT read as `src` by any other pending copy.
        if let Some(idx) = pending
            .iter()
            .position(|&(d, _)| !pending.iter().any(|&(_, s)| s == d))
        {
            let (d, s) = pending.swap_remove(idx);
            push_move(d, s);
            continue;
        }
        // Remaining copies form one or more cycles: break one using `cycle_tmp`.
        let (_d, s) = pending[0];
        push_move(cycle_tmp, s);
        for &mut (_, ref mut src) in &mut pending {
            if *src == s {
                *src = cycle_tmp;
            }
        }
    }
}

struct JumpPatch {
    insn_pc: usize,
    target: PatchTarget,
}

enum PatchTarget {
    Block(BlockId),
    Trampoline(usize),
}

struct EdgeTrampoline {
    copies: Vec<(Reg, Reg)>,
    target: BlockId,
    lineno: u32,
    col_span: CaretSpan,
}

/// Lower an SSA [`MirFn`] back into executable register bytecode [`FnCode`].
pub fn lower_mir_to_fncode(func: &MirFn) -> FnCode {
    let dom = DominatorTree::compute(func);
    let (val_to_reg, end_pos, ssa_reg_end) = allocate_ssa_registers(func, &dom);
    let mut pool = ConstPool::from_existing(&func.consts);

    // Reserve a dedicated cycle-breaking temp for parallel copies, and start
    // contiguous scratch windows strictly above all live SSA registers.
    let cycle_tmp = ssa_reg_end;
    let scratch_base = ssa_reg_end + 1;
    let mut max_reg_used = ssa_reg_end;

    // Count uses of each Val so we can coalesce single-use defs directly into
    // an immediately following `StoreLocal { reg, val }` (eliminating redundant
    // `Move(local, tmp)` pairs!).
    let mut use_count: Vec<u32> = vec![0; func.next_val as usize];
    for &bb_id in &dom.rpo {
        let block = &func.blocks[bb_id.0 as usize];
        for inst in &block.instrs {
            inst.for_each_use(|u| use_count[u.0 as usize] += 1);
        }
        block.term.for_each_use(|u| use_count[u.0 as usize] += 1);
    }

    // Track `MirOp::Const(v)` definitions so:
    // - `Return(v)` on `Const(None)` lowers directly to `ReturnNone`,
    // - `BinOp { lhs, rhs, is_inplace }` with constant RHS lowers directly to
    //   `BinOpImm` (for `i16`) or `BinOpConst` (matching compiler immediate
    //   fusion so `int_loop_versioning` and closed-form folding fire cleanly!),
    // - single-use `Const`s consumed exclusively by `BinOpImm`/`BinOpConst`
    //   elide their dead `LoadConst`.
    let mut is_const_none: Vec<bool> = vec![false; func.next_val as usize];
    let mut const_val_of: Vec<Option<Value>> = vec![None; func.next_val as usize];
    for &bb_id in &dom.rpo {
        for inst in &func.blocks[bb_id.0 as usize].instrs {
            if let (Some(def), MirOp::Const(v)) = (inst.def, &inst.op) {
                if v.is_none() {
                    is_const_none[def.0 as usize] = true;
                }
                const_val_of[def.0 as usize] = Some(v.clone());
            }
        }
    }
    let mut elided_const: Vec<bool> = vec![false; func.next_val as usize];
    for &bb_id in &dom.rpo {
        for inst in &func.blocks[bb_id.0 as usize].instrs {
            if let MirOp::BinOp { lhs, rhs, .. } = inst.op
                && lhs != rhs
                && use_count[rhs.0 as usize] == 1
                && const_val_of[rhs.0 as usize].is_some()
            {
                elided_const[rhs.0 as usize] = true;
            }
        }
    }

    let mut insns: Vec<Insn> = Vec::new();
    let mut lineno_table: Vec<u32> = Vec::new();
    let mut col_table: Vec<CaretSpan> = Vec::new();
    let mut block_pc: Vec<usize> = vec![usize::MAX; func.blocks.len()];
    let mut patches: Vec<JumpPatch> = Vec::new();

    let active_override: std::cell::Cell<Option<(Val, Reg)>> = std::cell::Cell::new(None);
    let reg_of = |v: Val| -> Reg {
        if let Some((ov_val, ov_reg)) = active_override.get()
            && ov_val == v
        {
            return ov_reg;
        }
        val_to_reg[v.0 as usize]
    };

    let edge_copies = |target: BlockId, args: &[Val]| -> Vec<(Reg, Reg)> {
        func.blocks[target.0 as usize]
            .params
            .iter()
            .zip(args.iter())
            .map(|(&p, &a)| (val_to_reg[p.0 as usize], val_to_reg[a.0 as usize]))
            .filter(|&(d, s)| d != s)
            .collect()
    };

    let emit = |insns: &mut Vec<Insn>,
                linenos: &mut Vec<u32>,
                cols: &mut Vec<CaretSpan>,
                insn: Insn,
                ln: u32,
                sp: CaretSpan|
     -> usize {
        let pc = insns.len();
        insns.push(insn);
        linenos.push(ln);
        cols.push(sp);
        pc
    };

    let nb = func.blocks.len();
    let mut rpo_pos: Vec<usize> = vec![usize::MAX; nb];
    for (idx, &bb_id) in dom.rpo.iter().enumerate() {
        rpo_pos[bb_id.0 as usize] = idx;
    }

    // Pre-mark blocks that may receive a forward trampoline right before them
    // or emit a back-edge trampoline right after them.  Placing forward
    // trampolines directly before their target block keeps every forward MIR
    // edge a strictly forward bytecode jump (preventing spurious backward jumps
    // that downstream passes like `pass_licm` could mistake for loops!), and
    // placing back-edge trampolines right after their source block keeps them
    // strictly inside their loop and exception region.
    let mut has_fwd_tramp_before: Vec<bool> = vec![false; nb];
    let mut has_back_tramp_after: Vec<bool> = vec![false; nb];
    for (order_idx, &bb_id) in dom.rpo.iter().enumerate() {
        let bi = bb_id.0 as usize;
        let mut note_tramp = |target: BlockId, args: &[Val]| {
            if !edge_copies(target, args).is_empty() {
                if rpo_pos[target.0 as usize] > order_idx {
                    has_fwd_tramp_before[target.0 as usize] = true;
                } else {
                    has_back_tramp_after[bi] = true;
                }
            }
        };
        match &func.blocks[bi].term {
            Terminator::Branch {
                then_bb,
                then_args,
                else_bb,
                else_args,
                ..
            } => {
                note_tramp(*else_bb, else_args);
                if dom.rpo.get(order_idx + 1) == Some(else_bb) {
                    note_tramp(*then_bb, then_args);
                }
            }
            Terminator::ForIter {
                exit_bb, exit_args, ..
            } => note_tramp(*exit_bb, exit_args),
            Terminator::SetupExcept {
                handler_bb,
                handler_args,
                ..
            } => note_tramp(*handler_bb, handler_args),
            Terminator::MatchExcept {
                nomatch_bb,
                nomatch_args,
                ..
            }
            | Terminator::MatchExceptStar {
                nomatch_bb,
                nomatch_args,
                ..
            } => note_tramp(*nomatch_bb, nomatch_args),
            _ => {}
        }
    }

    let mut fwd_tramps_before: Vec<Vec<(usize, EdgeTrampoline)>> =
        (0..nb).map(|_| Vec::new()).collect();
    let mut tramp_pc: Vec<usize> = Vec::new();

    let rpo_len = dom.rpo.len();
    let mut pos_cursor = 0usize;
    for (order_idx, &bb_id) in dom.rpo.iter().enumerate() {
        let bi = bb_id.0 as usize;
        let block = &func.blocks[bi];

        // Emit any forward-edge trampolines targeting `bb_id` directly before it
        // (the final trampoline falls straight through into `bb_id`!).
        let incoming_fwd = std::mem::take(&mut fwd_tramps_before[bi]);
        let fwd_count = incoming_fwd.len();
        for (ti, (tid, tr)) in incoming_fwd.into_iter().enumerate() {
            tramp_pc[tid] = insns.len();
            max_reg_used = max_reg_used.max(cycle_tmp);
            emit_parallel_copies(
                &tr.copies,
                cycle_tmp,
                &mut insns,
                &mut lineno_table,
                &mut col_table,
                tr.lineno,
                tr.col_span,
            );
            if ti + 1 < fwd_count {
                let jpc = emit(
                    &mut insns,
                    &mut lineno_table,
                    &mut col_table,
                    Insn::Jump(0),
                    tr.lineno,
                    tr.col_span,
                );
                patches.push(JumpPatch {
                    insn_pc: jpc,
                    target: PatchTarget::Block(tr.target),
                });
            }
        }

        let next_bb =
            dom.rpo.get(order_idx + 1).copied().filter(|&nb_id| {
                !has_fwd_tramp_before[nb_id.0 as usize] && !has_back_tramp_after[bi]
            });
        let mut local_back_tramps: Vec<(usize, EdgeTrampoline)> = Vec::new();
        block_pc[bi] = insns.len();
        pos_cursor += 1; // block entry position
        let mut pending_override: Option<(Val, Reg)> = None;
        active_override.set(None);

        let mut i = 0usize;
        while i < block.instrs.len() {
            let inst_pos = pos_cursor;
            let inst = &block.instrs[i];
            let ln = inst.lineno;
            let sp = inst.col_span;
            // Activate any single-use scratch-base forwarding from an
            // immediately preceding `Call` on the next emitting instruction.
            let is_noop_load_local = matches!(
                (inst.def, &inst.op),
                (Some(d), MirOp::LoadLocal(lr)) if val_to_reg[d.0 as usize] == *lr
            );
            if !is_noop_load_local {
                active_override.set(pending_override.take());
            }

            // Single-use `StoreLocal` coalescing: if this instruction produces
            // a single `def` whose ONLY use is an immediately following
            // `StoreLocal { reg, val: def }`, write directly into `reg`!
            let coalesced_target: Option<Reg> = if let Some(def) = inst.def
                && use_count[def.0 as usize] == 1
                && let Some(next_inst) = block.instrs.get(i + 1)
                && let MirOp::StoreLocal { reg, val } = next_inst.op
                && val == def
                && can_coalesce_op_into_reg(&inst.op, reg, &val_to_reg)
            {
                Some(reg)
            } else {
                None
            };

            let dst_reg = coalesced_target
                .or_else(|| inst.def.map(reg_of))
                .unwrap_or(0);

            // Elide `v = Const(...)` when consumed exclusively by an immediate
            // `BinOpImm` / `BinOpConst` or a trailing `Return(None)`.
            if let (Some(def), MirOp::Const(val)) = (inst.def, &inst.op)
                && coalesced_target.is_none()
                && (elided_const[def.0 as usize]
                    || (val.is_none()
                        && use_count[def.0 as usize] == 1
                        && matches!(block.term, Terminator::Return(rv) if rv == def)
                        && i + 1 == block.instrs.len()))
            {
                i += 1;
                pos_cursor += 1;
                continue;
            }

            match &inst.op {
                MirOp::Const(val) => {
                    if val.is_none() {
                        emit(
                            &mut insns,
                            &mut lineno_table,
                            &mut col_table,
                            Insn::LoadNone(dst_reg),
                            ln,
                            sp,
                        );
                    } else {
                        let c_idx = pool.intern(val.clone());
                        emit(
                            &mut insns,
                            &mut lineno_table,
                            &mut col_table,
                            Insn::LoadConst(dst_reg, c_idx),
                            ln,
                            sp,
                        );
                    }
                }
                MirOp::Copy(src) => {
                    let sr = reg_of(*src);
                    if dst_reg != sr {
                        emit(
                            &mut insns,
                            &mut lineno_table,
                            &mut col_table,
                            Insn::Move(dst_reg, sr),
                            ln,
                            sp,
                        );
                    }
                }
                MirOp::BinOp {
                    op,
                    lhs,
                    rhs,
                    is_inplace,
                } => {
                    let lr = reg_of(*lhs);
                    let op_insn = if *lhs != *rhs
                        && let Some(rv) = &const_val_of[rhs.0 as usize]
                    {
                        if *is_inplace
                            && let ValueKind::Int(n) = rv.kind()
                            && (i16::MIN as i64..=i16::MAX as i64).contains(&n)
                        {
                            Insn::BinOpImm(dst_reg, lr, *op, n as i16, true)
                        } else {
                            let c_idx = pool.intern(rv.clone());
                            Insn::BinOpConst(dst_reg, lr, *op, c_idx, *is_inplace)
                        }
                    } else {
                        let rr = reg_of(*rhs);
                        if *is_inplace {
                            Insn::BinOpInPlace(dst_reg, lr, *op, rr)
                        } else {
                            Insn::BinOp(dst_reg, lr, *op, rr)
                        }
                    };
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        op_insn,
                        ln,
                        sp,
                    );
                }
                MirOp::UnaryOp { op, arg } => {
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::UnaryOp(dst_reg, *op, reg_of(*arg)),
                        ln,
                        sp,
                    );
                }
                MirOp::LoadLocal(local_reg) => {
                    if dst_reg != *local_reg {
                        emit(
                            &mut insns,
                            &mut lineno_table,
                            &mut col_table,
                            Insn::Move(dst_reg, *local_reg),
                            ln,
                            sp,
                        );
                    }
                }
                MirOp::StoreLocal { reg, val } => {
                    let sr = reg_of(*val);
                    if *reg != sr {
                        emit(
                            &mut insns,
                            &mut lineno_table,
                            &mut col_table,
                            Insn::Move(*reg, sr),
                            ln,
                            sp,
                        );
                    }
                }
                MirOp::LoadGlobal { name_idx } => {
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::LoadGlobal(dst_reg, *name_idx),
                        ln,
                        sp,
                    );
                }
                MirOp::StoreGlobal { name_idx, val } => {
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::StoreGlobal(*name_idx, reg_of(*val)),
                        ln,
                        sp,
                    );
                }
                MirOp::LoadCell { cell_idx } => {
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::LoadCell(dst_reg, *cell_idx),
                        ln,
                        sp,
                    );
                }
                MirOp::StoreCell { cell_idx, val } => {
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::StoreCell(*cell_idx, reg_of(*val)),
                        ln,
                        sp,
                    );
                }
                MirOp::GetAttr { obj, name_idx } => {
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::GetAttr(dst_reg, reg_of(*obj), *name_idx),
                        ln,
                        sp,
                    );
                }
                MirOp::SetAttr { obj, name_idx, val } => {
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::SetAttr(reg_of(*obj), *name_idx, reg_of(*val)),
                        ln,
                        sp,
                    );
                }
                MirOp::GetItem { obj, key } => {
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::GetItem(dst_reg, reg_of(*obj), reg_of(*key)),
                        ln,
                        sp,
                    );
                }
                MirOp::SetItem { obj, key, val } => {
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::SetItem(reg_of(*obj), reg_of(*key), reg_of(*val)),
                        ln,
                        sp,
                    );
                }
                MirOp::Call {
                    callee,
                    args,
                    is_memo,
                } => {
                    let callee_r = reg_of(*callee);
                    let argc = args.len() as u8;
                    // Reuse existing contiguous temp window `[callee_r..=callee_r+argc]`
                    // directly when `callee` is a dying temp and all args already occupy
                    // consecutive slots (avoids redundant Moves and keeps Call sites tight!).
                    let can_reuse_in_place = callee_r >= func.num_locals
                        && end_pos[callee.0 as usize] <= inst_pos
                        && args
                            .iter()
                            .enumerate()
                            .all(|(ai, &a)| reg_of(a) == callee_r + 1 + ai as u32);
                    let base = if can_reuse_in_place {
                        callee_r
                    } else {
                        let b = scratch_base;
                        max_reg_used = max_reg_used.max(b + argc as u32);
                        // If the callee was just loaded by an immediately
                        // preceding single-use `LoadGlobal(callee_r, name_idx)`,
                        // retarget that `LoadGlobal` directly into `b` so
                        // `len(xs)` / `foo(x)` emits `LoadGlobal(b, name);
                        // Move(b+1, arg); Call(b, 1)` with zero extra Move!
                        let retargeted = use_count[callee.0 as usize] == 1
                            && matches!(insns.last(), Some(Insn::LoadGlobal(d, _)) if *d == callee_r);
                        if retargeted {
                            if let Some(Insn::LoadGlobal(d, _)) = insns.last_mut() {
                                *d = b;
                            }
                        } else {
                            emit(
                                &mut insns,
                                &mut lineno_table,
                                &mut col_table,
                                Insn::Move(b, callee_r),
                                ln,
                                sp,
                            );
                        }
                        for (ai, &arg) in args.iter().enumerate() {
                            emit(
                                &mut insns,
                                &mut lineno_table,
                                &mut col_table,
                                Insn::Move(b + 1 + ai as u32, reg_of(arg)),
                                ln,
                                sp,
                            );
                        }
                        b
                    };
                    let call_insn = if *is_memo {
                        Insn::CallMemo(base, argc)
                    } else {
                        Insn::Call(base, argc)
                    };
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        call_insn,
                        ln,
                        sp,
                    );
                    if let Some(call_def) = inst.def
                        && use_count[call_def.0 as usize] > 0
                        && dst_reg != base
                    {
                        // If `call_def` is single-use and consumed immediately
                        // by the next emitting non-scratch instruction (e.g.
                        // `index < len(values)`), let that consumer read `base`
                        // directly and skip `Move(dst_reg, base)`!
                        let next_emitting = block.instrs[i + 1..].iter().find(|ni| {
                            !matches!(
                                (ni.def, &ni.op),
                                (Some(d), MirOp::LoadLocal(lr)) if val_to_reg[d.0 as usize] == *lr
                            )
                        });
                        let can_forward_base = coalesced_target.is_none()
                            && use_count[call_def.0 as usize] == 1
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
                            pending_override = Some((call_def, base));
                        } else {
                            emit(
                                &mut insns,
                                &mut lineno_table,
                                &mut col_table,
                                Insn::Move(dst_reg, base),
                                ln,
                                sp,
                            );
                        }
                    }
                }
                MirOp::BuildTuple(elems) => {
                    let n = elems.len() as u32;
                    let base = scratch_base;
                    if n > 0 {
                        max_reg_used = max_reg_used.max(base + n - 1);
                    }
                    for (ei, &elem) in elems.iter().enumerate() {
                        emit(
                            &mut insns,
                            &mut lineno_table,
                            &mut col_table,
                            Insn::Move(base + ei as u32, reg_of(elem)),
                            ln,
                            sp,
                        );
                    }
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::BuildTuple(dst_reg, base, n),
                        ln,
                        sp,
                    );
                }
                MirOp::BuildList(elems) => {
                    let n = elems.len() as u32;
                    let base = scratch_base;
                    if n > 0 {
                        max_reg_used = max_reg_used.max(base + n - 1);
                    }
                    for (ei, &elem) in elems.iter().enumerate() {
                        emit(
                            &mut insns,
                            &mut lineno_table,
                            &mut col_table,
                            Insn::Move(base + ei as u32, reg_of(elem)),
                            ln,
                            sp,
                        );
                    }
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::BuildList(dst_reg, base, n),
                        ln,
                        sp,
                    );
                }
                MirOp::SideEffectInsn { insn, uses, defs } => {
                    let effective_defs: SmallVec<[Reg; 2]> = if let Some(cr) = coalesced_target {
                        smallvec::smallvec![cr]
                    } else {
                        defs.iter().map(|&d| reg_of(d)).collect()
                    };
                    let use_regs: SmallVec<[Reg; 4]> = uses.iter().map(|&u| reg_of(u)).collect();
                    materialize_side_effect_insn(
                        insn,
                        &use_regs,
                        &effective_defs,
                        scratch_base,
                        &mut max_reg_used,
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        ln,
                        sp,
                    );
                }
            }

            if coalesced_target.is_some() {
                // Skip the consumed `StoreLocal` right after this instruction.
                i += 2;
                pos_cursor += 2;
            } else {
                i += 1;
                pos_cursor += 1;
            }
        }
        pos_cursor += 1; // terminator position

        let mut queue_trampoline = |copies: Vec<(Reg, Reg)>,
                                    target: BlockId,
                                    lineno: u32,
                                    col_span: CaretSpan|
         -> PatchTarget {
            let tid = tramp_pc.len();
            tramp_pc.push(usize::MAX);
            let tr = EdgeTrampoline {
                copies,
                target,
                lineno,
                col_span,
            };
            if rpo_pos[target.0 as usize] > order_idx {
                fwd_tramps_before[target.0 as usize].push((tid, tr));
            } else {
                local_back_tramps.push((tid, tr));
            }
            PatchTarget::Trampoline(tid)
        };

        // Emit terminator.
        let tln = block.term_lineno;
        let tsp = block.term_col_span;
        match &block.term {
            Terminator::Jump { target, args } => {
                let copies = edge_copies(*target, args);
                if !copies.is_empty() {
                    max_reg_used = max_reg_used.max(cycle_tmp);
                    emit_parallel_copies(
                        &copies,
                        cycle_tmp,
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        tln,
                        tsp,
                    );
                }
                if next_bb != Some(*target) {
                    let pc = emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::Jump(0),
                        tln,
                        tsp,
                    );
                    patches.push(JumpPatch {
                        insn_pc: pc,
                        target: PatchTarget::Block(*target),
                    });
                }
            }
            Terminator::Branch {
                cond,
                then_bb,
                then_args,
                else_bb,
                else_args,
            } => {
                let cr = reg_of(*cond);
                let then_copies = edge_copies(*then_bb, then_args);
                let else_copies = edge_copies(*else_bb, else_args);

                if next_bb == Some(*then_bb) {
                    // JumpIfFalse to else (or trampoline if else has copies),
                    // fall through to then (emitting then_copies inline).
                    let else_target = if else_copies.is_empty() {
                        PatchTarget::Block(*else_bb)
                    } else {
                        queue_trampoline(else_copies, *else_bb, tln, tsp)
                    };
                    let pc = emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::JumpIfFalse(cr, 0),
                        tln,
                        tsp,
                    );
                    patches.push(JumpPatch {
                        insn_pc: pc,
                        target: else_target,
                    });
                    if !then_copies.is_empty() {
                        max_reg_used = max_reg_used.max(cycle_tmp);
                        emit_parallel_copies(
                            &then_copies,
                            cycle_tmp,
                            &mut insns,
                            &mut lineno_table,
                            &mut col_table,
                            tln,
                            tsp,
                        );
                    }
                } else if next_bb == Some(*else_bb) {
                    // JumpIfTrue to then (or trampoline), fall through to else.
                    let then_target = if then_copies.is_empty() {
                        PatchTarget::Block(*then_bb)
                    } else {
                        queue_trampoline(then_copies, *then_bb, tln, tsp)
                    };
                    let pc = emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::JumpIfTrue(cr, 0),
                        tln,
                        tsp,
                    );
                    patches.push(JumpPatch {
                        insn_pc: pc,
                        target: then_target,
                    });
                    if !else_copies.is_empty() {
                        max_reg_used = max_reg_used.max(cycle_tmp);
                        emit_parallel_copies(
                            &else_copies,
                            cycle_tmp,
                            &mut insns,
                            &mut lineno_table,
                            &mut col_table,
                            tln,
                            tsp,
                        );
                    }
                } else {
                    // Neither target is next_bb.
                    let else_target = if else_copies.is_empty() {
                        PatchTarget::Block(*else_bb)
                    } else {
                        queue_trampoline(else_copies, *else_bb, tln, tsp)
                    };
                    let pc = emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::JumpIfFalse(cr, 0),
                        tln,
                        tsp,
                    );
                    patches.push(JumpPatch {
                        insn_pc: pc,
                        target: else_target,
                    });
                    if !then_copies.is_empty() {
                        max_reg_used = max_reg_used.max(cycle_tmp);
                        emit_parallel_copies(
                            &then_copies,
                            cycle_tmp,
                            &mut insns,
                            &mut lineno_table,
                            &mut col_table,
                            tln,
                            tsp,
                        );
                    }
                    let jpc = emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::Jump(0),
                        tln,
                        tsp,
                    );
                    patches.push(JumpPatch {
                        insn_pc: jpc,
                        target: PatchTarget::Block(*then_bb),
                    });
                }
            }
            Terminator::Return(v) => {
                if is_const_none[v.0 as usize] {
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::ReturnNone,
                        tln,
                        tsp,
                    );
                } else {
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::Return(reg_of(*v)),
                        tln,
                        tsp,
                    );
                }
            }
            Terminator::Raise(Some(v)) => {
                emit(
                    &mut insns,
                    &mut lineno_table,
                    &mut col_table,
                    Insn::RaiseValue(reg_of(*v)),
                    tln,
                    tsp,
                );
            }
            Terminator::Raise(None) => {
                emit(
                    &mut insns,
                    &mut lineno_table,
                    &mut col_table,
                    Insn::RaiseReRaise,
                    tln,
                    tsp,
                );
            }
            Terminator::RaiseSpecial { insn, uses } => {
                let lowered = match insn {
                    Insn::RaiseFrom(_, _) => Insn::RaiseFrom(reg_of(uses[0]), reg_of(uses[1])),
                    Insn::RaiseAssert(_) => Insn::RaiseAssert(reg_of(uses[0])),
                    Insn::RaiseExceptStarResidual(_) => {
                        Insn::RaiseExceptStarResidual(reg_of(uses[0]))
                    }
                    Insn::RaiseAssertNoMsg => Insn::RaiseAssertNoMsg,
                    other => other.clone(),
                };
                emit(
                    &mut insns,
                    &mut lineno_table,
                    &mut col_table,
                    lowered,
                    tln,
                    tsp,
                );
            }
            Terminator::ForIter {
                slot,
                dst,
                body_bb,
                body_args,
                exit_bb,
                exit_args,
            } => {
                let body_copies = edge_copies(*body_bb, body_args);
                let exit_copies = edge_copies(*exit_bb, exit_args);
                let exit_target = if exit_copies.is_empty() {
                    PatchTarget::Block(*exit_bb)
                } else {
                    queue_trampoline(exit_copies, *exit_bb, tln, tsp)
                };
                let pc = emit(
                    &mut insns,
                    &mut lineno_table,
                    &mut col_table,
                    Insn::ForIter(reg_of(*dst), *slot, 0),
                    tln,
                    tsp,
                );
                patches.push(JumpPatch {
                    insn_pc: pc,
                    target: exit_target,
                });
                if !body_copies.is_empty() {
                    max_reg_used = max_reg_used.max(cycle_tmp);
                    emit_parallel_copies(
                        &body_copies,
                        cycle_tmp,
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        tln,
                        tsp,
                    );
                }
                if next_bb != Some(*body_bb) {
                    let jpc = emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::Jump(0),
                        tln,
                        tsp,
                    );
                    patches.push(JumpPatch {
                        insn_pc: jpc,
                        target: PatchTarget::Block(*body_bb),
                    });
                }
            }
            Terminator::SetupExcept {
                try_bb,
                try_args,
                handler_bb,
                handler_args,
            } => {
                let try_copies = edge_copies(*try_bb, try_args);
                let handler_copies = edge_copies(*handler_bb, handler_args);
                let handler_target = if handler_copies.is_empty() {
                    PatchTarget::Block(*handler_bb)
                } else {
                    queue_trampoline(handler_copies, *handler_bb, tln, tsp)
                };
                let pc = emit(
                    &mut insns,
                    &mut lineno_table,
                    &mut col_table,
                    Insn::SetupExcept(0),
                    tln,
                    tsp,
                );
                patches.push(JumpPatch {
                    insn_pc: pc,
                    target: handler_target,
                });
                if !try_copies.is_empty() {
                    max_reg_used = max_reg_used.max(cycle_tmp);
                    emit_parallel_copies(
                        &try_copies,
                        cycle_tmp,
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        tln,
                        tsp,
                    );
                }
                if next_bb != Some(*try_bb) {
                    let jpc = emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::Jump(0),
                        tln,
                        tsp,
                    );
                    patches.push(JumpPatch {
                        insn_pc: jpc,
                        target: PatchTarget::Block(*try_bb),
                    });
                }
            }
            Terminator::MatchExcept {
                type_val,
                matched_bb,
                matched_args,
                nomatch_bb,
                nomatch_args,
            } => {
                let matched_copies = edge_copies(*matched_bb, matched_args);
                let nomatch_copies = edge_copies(*nomatch_bb, nomatch_args);
                let nomatch_target = if nomatch_copies.is_empty() {
                    PatchTarget::Block(*nomatch_bb)
                } else {
                    queue_trampoline(nomatch_copies, *nomatch_bb, tln, tsp)
                };
                let pc = emit(
                    &mut insns,
                    &mut lineno_table,
                    &mut col_table,
                    Insn::MatchExcept(reg_of(*type_val), 0),
                    tln,
                    tsp,
                );
                patches.push(JumpPatch {
                    insn_pc: pc,
                    target: nomatch_target,
                });
                if !matched_copies.is_empty() {
                    max_reg_used = max_reg_used.max(cycle_tmp);
                    emit_parallel_copies(
                        &matched_copies,
                        cycle_tmp,
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        tln,
                        tsp,
                    );
                }
                if next_bb != Some(*matched_bb) {
                    let jpc = emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::Jump(0),
                        tln,
                        tsp,
                    );
                    patches.push(JumpPatch {
                        insn_pc: jpc,
                        target: PatchTarget::Block(*matched_bb),
                    });
                }
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
                let matched_copies = edge_copies(*matched_bb, matched_args);
                let nomatch_copies = edge_copies(*nomatch_bb, nomatch_args);
                let nomatch_target = if nomatch_copies.is_empty() {
                    PatchTarget::Block(*nomatch_bb)
                } else {
                    queue_trampoline(nomatch_copies, *nomatch_bb, tln, tsp)
                };
                // MatchExceptStar overwrites `src_group` in-place on match; use
                // a dedicated scratch register so `src_group` is not clobbered
                // on the no-match path!
                let sg_scratch = scratch_base;
                max_reg_used = max_reg_used.max(sg_scratch);
                emit(
                    &mut insns,
                    &mut lineno_table,
                    &mut col_table,
                    Insn::Move(sg_scratch, reg_of(*src_group)),
                    tln,
                    tsp,
                );
                let pc = emit(
                    &mut insns,
                    &mut lineno_table,
                    &mut col_table,
                    Insn::MatchExceptStar(reg_of(*type_val), sg_scratch, reg_of(*matched_def), 0),
                    tln,
                    tsp,
                );
                patches.push(JumpPatch {
                    insn_pc: pc,
                    target: nomatch_target,
                });
                if reg_of(*residual_def) != sg_scratch {
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::Move(reg_of(*residual_def), sg_scratch),
                        tln,
                        tsp,
                    );
                }
                if !matched_copies.is_empty() {
                    max_reg_used = max_reg_used.max(cycle_tmp);
                    emit_parallel_copies(
                        &matched_copies,
                        cycle_tmp,
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        tln,
                        tsp,
                    );
                }
                if next_bb != Some(*matched_bb) {
                    let jpc = emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::Jump(0),
                        tln,
                        tsp,
                    );
                    patches.push(JumpPatch {
                        insn_pc: jpc,
                        target: PatchTarget::Block(*matched_bb),
                    });
                }
            }
            Terminator::Unreachable => {
                if order_idx + 1 == rpo_len {
                    emit(
                        &mut insns,
                        &mut lineno_table,
                        &mut col_table,
                        Insn::ReturnNone,
                        tln,
                        tsp,
                    );
                }
            }
        }

        // Emit any back-edge trampolines originating from `bb_id` directly
        // after it so they remain strictly inside their loop / exc region.
        for (tid, tr) in local_back_tramps {
            tramp_pc[tid] = insns.len();
            max_reg_used = max_reg_used.max(cycle_tmp);
            emit_parallel_copies(
                &tr.copies,
                cycle_tmp,
                &mut insns,
                &mut lineno_table,
                &mut col_table,
                tr.lineno,
                tr.col_span,
            );
            let jpc = emit(
                &mut insns,
                &mut lineno_table,
                &mut col_table,
                Insn::Jump(0),
                tr.lineno,
                tr.col_span,
            );
            patches.push(JumpPatch {
                insn_pc: jpc,
                target: PatchTarget::Block(tr.target),
            });
        }
    }

    // Patch all relative jump offsets.
    for patch in patches {
        let target_pc = match patch.target {
            PatchTarget::Block(bb) => block_pc[bb.0 as usize],
            PatchTarget::Trampoline(tid) => tramp_pc[tid],
        };
        let off = target_pc as i32 - (patch.insn_pc as i32 + 1);
        match &mut insns[patch.insn_pc] {
            Insn::Jump(o)
            | Insn::JumpIfFalse(_, o)
            | Insn::JumpIfTrue(_, o)
            | Insn::ForIter(_, _, o)
            | Insn::SetupExcept(o)
            | Insn::MatchExcept(_, o)
            | Insn::MatchExceptStar(_, _, _, o) => *o = off,
            other => unreachable!("non-jump in JumpPatch: {other:?}"),
        }
    }

    let num_regs = (max_reg_used + 1).max(func.num_locals).min(MAX_FRAME_REGS);
    let insns_len = insns.len();
    let global_cache_interest_masks = func
        .names
        .iter()
        .map(|name| crate::bytecode::global_cache_interest_mask(name))
        .collect();

    FnCode {
        insns,
        filename: func.filename.clone(),
        qualname: func.qualname.clone(),
        lineno_table,
        col_table,
        first_lineno: func.first_lineno,
        consts: pool.consts,
        names: func.names.clone(),
        num_regs,
        num_iters: func.num_iters,
        num_locals: func.num_locals,
        fn_protos: func.fn_protos.clone(),
        cell_slots: func.cell_slots.clone(),
        own_cell_count: func.own_cell_count,
        free_var_candidates: func.free_var_candidates.clone(),
        is_generator: func.is_generator,
        is_coroutine: func.is_coroutine,
        is_class_method: func.is_class_method,
        is_function_scope: func.is_function_scope,
        is_inlined_comp: func.is_inlined_comp,
        comp_enclosing_locals: func.comp_enclosing_locals.clone(),
        attr_cache: empty_cache_vec(insns_len, AttrCacheEntry::Empty),
        global_cache: empty_cache_vec(func.names.len(), GlobalCacheEntry::Empty),
        global_cache_interest_masks,
        binop_cache: empty_cache_vec(insns_len, BinOpCacheEntry::Empty),
        kwcall_cache: empty_cache_vec(insns_len, KwCallCacheEntry::Empty),
        fmt_spec_cache: empty_cache_vec(insns_len, crate::interpreter::FmtSpecCacheEntry::Empty),
        call_builtin_cache: empty_cache_vec(
            insns_len,
            crate::interpreter::CallBuiltinCacheEntry::Empty,
        ),
        cached_code_object: std::cell::RefCell::new(None),
        exc_table: Vec::new(),
        has_exc_handlers: true,
    }
}

/// True when `op` can write directly into `target_reg` (eliminating a trailing
/// single-use `StoreLocal { reg: target_reg, val }`).
fn can_coalesce_op_into_reg(op: &MirOp, _target_reg: Reg, _val_to_reg: &[Reg]) -> bool {
    match op {
        MirOp::Const(_)
        | MirOp::Copy(_)
        | MirOp::BinOp { .. }
        | MirOp::UnaryOp { .. }
        | MirOp::LoadLocal(_)
        | MirOp::LoadGlobal { .. }
        | MirOp::LoadCell { .. }
        | MirOp::GetAttr { .. }
        | MirOp::GetItem { .. }
        | MirOp::Call { .. }
        | MirOp::BuildTuple(_)
        | MirOp::BuildList(_) => true,
        MirOp::SideEffectInsn { defs, .. } => defs.len() == 1,
        _ => false,
    }
}

#[inline]
fn subst_kwcall_name(name: &KwCallName, uses: &[Reg], cursor: &mut usize) -> KwCallName {
    match name {
        KwCallName::Callee(_) => {
            let r = uses[*cursor];
            *cursor += 1;
            KwCallName::Callee(r)
        }
        KwCallName::Method { name_idx, .. } => {
            let r = uses[*cursor];
            *cursor += 1;
            KwCallName::Method {
                obj: r,
                name_idx: *name_idx,
            }
        }
    }
}

/// Lower a `MirOp::SideEffectInsn` back to one or more register `Insn`s using
/// `scratch_base` whenever a contiguous register window is required.
#[allow(clippy::too_many_arguments)]
fn materialize_side_effect_insn(
    insn: &Insn,
    uses: &[Reg],
    defs: &[Reg],
    scratch_base: Reg,
    max_reg_used: &mut Reg,
    insns: &mut Vec<Insn>,
    linenos: &mut Vec<u32>,
    cols: &mut Vec<CaretSpan>,
    ln: u32,
    sp: CaretSpan,
) {
    macro_rules! emit_i {
        ($i:expr) => {{
            insns.push($i);
            linenos.push(ln);
            cols.push(sp);
        }};
    }

    macro_rules! copy_window {
        ($base:expr, $src_regs:expr) => {{
            let base = $base;
            let src_regs: &[Reg] = $src_regs;
            if !src_regs.is_empty() {
                *max_reg_used = (*max_reg_used).max(base + src_regs.len() as u32 - 1);
            }
            for (idx, &sr) in src_regs.iter().enumerate() {
                let dr = base + idx as u32;
                if dr != sr {
                    emit_i!(Insn::Move(dr, sr));
                }
            }
        }};
    }

    use Insn::*;
    match insn {
        // Zero-use / canonical-local-slot instructions: preserve verbatim (or
        // only update `dst`).
        DeleteName(_)
        | DeleteCell(..)
        | DeleteLocal(..)
        | DeleteModuleGlobal(_)
        | PushTypeParamEnv
        | PopTypeParamEnv
        | PopExcept
        | EndExcept
        | PopExcContext
        | CheckLocal(..)
        | RecordClassStore(_)
        | RecordClassDel(_)
        | SyncModuleGlobal(..) => emit_i!(insn.clone()),

        LoadClassName(_, local, name_idx) => emit_i!(LoadClassName(defs[0], *local, *name_idx)),
        LoadClassDeref(_, cell_idx) => emit_i!(LoadClassDeref(defs[0], *cell_idx)),
        LoadExc(_) => emit_i!(LoadExc(defs[0])),
        ImportModule(_, name_idx) => emit_i!(ImportModule(defs[0], *name_idx)),
        MakeTypeVar(_, name_idx) => emit_i!(MakeTypeVar(defs[0], *name_idx)),

        // Single-use, no-def.
        ImportStar(_) => emit_i!(ImportStar(uses[0])),
        PrintExpr(_) => emit_i!(PrintExpr(uses[0])),
        GetIter(slot, _) => emit_i!(GetIter(*slot, uses[0])),
        DeleteAttr(_, name_idx) => emit_i!(DeleteAttr(uses[0], *name_idx)),
        PushExcContext(_) => emit_i!(PushExcContext(uses[0])),

        // Single-use, single-def.
        FormatValue(_, _) => emit_i!(FormatValue(defs[0], uses[0])),
        MatchSeqExcluded(_, _) => emit_i!(MatchSeqExcluded(defs[0], uses[0])),
        MatchMapping(_, _) => emit_i!(MatchMapping(defs[0], uses[0])),
        GetAwaitable(_, _) => emit_i!(GetAwaitable(defs[0], uses[0])),
        BuildListReserve(_, _) => emit_i!(BuildListReserve(defs[0], uses[0])),
        GetAttrForWith(_, _, name_idx, kind) => {
            emit_i!(GetAttrForWith(defs[0], uses[0], *name_idx, *kind))
        }
        LoadExcTraceback(_, _) => emit_i!(LoadExcTraceback(defs[0], uses[0])),
        ImportFromAttr(_, _, name_idx) => emit_i!(ImportFromAttr(defs[0], uses[0], *name_idx)),
        Yield { .. } => emit_i!(Yield {
            src: uses[0],
            dst: defs[0],
        }),

        // Two-use, no-def.
        SetAdd(_, _) => emit_i!(SetAdd(uses[0], uses[1])),
        ListAppend(_, _) => emit_i!(ListAppend(uses[0], uses[1])),
        ListExtend(_, _) => emit_i!(ListExtend(uses[0], uses[1])),
        DictUpdate(_, _) => emit_i!(DictUpdate(uses[0], uses[1])),
        DeleteItem(_, _) => emit_i!(DeleteItem(uses[0], uses[1])),
        SetTypeVarAttr(_, name_idx, _) => emit_i!(SetTypeVarAttr(uses[0], *name_idx, uses[1])),

        // Two-use, single-def.
        FormatValueSpec(_, _, _) => emit_i!(FormatValueSpec(defs[0], uses[0], uses[1])),
        MakeTypeAlias(_, name_idx, _, _) => {
            emit_i!(MakeTypeAlias(defs[0], *name_idx, uses[0], uses[1]))
        }
        YieldFrom { .. } => {
            // Allocate dedicated scratch slots for iter_reg & sent_reg so
            // `YieldFrom` can overwrite `sent_reg` on resume without clobbering
            // any other live SSA register!
            let w_iter = scratch_base;
            let w_sent = scratch_base + 1;
            *max_reg_used = (*max_reg_used).max(w_sent);
            emit_i!(Move(w_iter, uses[0]));
            emit_i!(Move(w_sent, uses[1]));
            emit_i!(YieldFrom {
                iter_reg: w_iter,
                sent_reg: w_sent,
                result_reg: defs[0],
            });
        }

        // KwCallName helpers.
        ListExtendCall { name, .. } => {
            let mut c = 2;
            let new_name = subst_kwcall_name(name, uses, &mut c);
            emit_i!(ListExtendCall {
                list: uses[0],
                src: uses[1],
                name: new_name,
            });
        }
        DictMergeKwCall { name, .. } => {
            let mut c = 2;
            let new_name = subst_kwcall_name(name, uses, &mut c);
            emit_i!(DictMergeKwCall {
                dict: uses[0],
                src: uses[1],
                name: new_name,
            });
        }
        SetItemKwCall { name, .. } => {
            let mut c = 3;
            let new_name = subst_kwcall_name(name, uses, &mut c);
            emit_i!(SetItemKwCall {
                dict: uses[0],
                key: uses[1],
                val: uses[2],
                name: new_name,
            });
        }

        // Contiguous window calls.
        CallKw {
            total,
            nkw,
            kwnames_idx,
            ..
        } => {
            let base = scratch_base;
            copy_window!(base, uses);
            emit_i!(CallKw {
                func: base,
                total: *total,
                nkw: *nkw,
                kwnames_idx: *kwnames_idx,
            });
            if defs[0] != base {
                emit_i!(Move(defs[0], base));
            }
        }
        CallEx { npos, .. } => {
            let base = scratch_base;
            let pos_count = *npos as usize + 1;
            copy_window!(base, &uses[..pos_count]);
            let kw_reg = uses[pos_count];
            emit_i!(CallEx {
                func: base,
                npos: *npos,
                kwargs: kw_reg,
            });
            if defs[0] != base {
                emit_i!(Move(defs[0], base));
            }
        }
        CallExArgs {
            npos,
            nkw,
            kwnames_idx,
            kwargs,
            ..
        } => {
            let base = scratch_base;
            let fixed = 1 + *npos as usize + *nkw as usize;
            copy_window!(base, &uses[..fixed]);
            let args_splat = uses[fixed];
            let new_kwargs = if *kwargs == NO_KWARGS {
                NO_KWARGS
            } else {
                uses[fixed + 1]
            };
            emit_i!(CallExArgs {
                func: base,
                npos: *npos,
                nkw: *nkw,
                kwnames_idx: *kwnames_idx,
                args_splat,
                kwargs: new_kwargs,
            });
            if defs[0] != base {
                emit_i!(Move(defs[0], base));
            }
        }
        CallMethod {
            name_idx, nargs, ..
        } => {
            let obj = uses[0];
            let base = scratch_base;
            copy_window!(base, &uses[1..]);
            emit_i!(CallMethod {
                dst: defs[0],
                obj,
                name_idx: *name_idx,
                args_base: base,
                nargs: *nargs,
            });
        }
        CallMethodKw {
            name_idx,
            total,
            nkw,
            kwnames_idx,
            ..
        } => {
            let obj = uses[0];
            let base = scratch_base;
            copy_window!(base, &uses[1..]);
            emit_i!(CallMethodKw {
                dst: defs[0],
                obj,
                name_idx: *name_idx,
                args_base: base,
                total: *total,
                nkw: *nkw,
                kwnames_idx: *kwnames_idx,
            });
        }
        CallMethodExpanded { name_idx, .. } => {
            emit_i!(CallMethodExpanded {
                dst: defs[0],
                obj: uses[0],
                name_idx: *name_idx,
                pos_list: uses[1],
                kw_dict: uses[2],
            });
        }

        // Contiguous window builders & slicers.
        BuildString(_, _, n) => {
            let base = scratch_base;
            copy_window!(base, uses);
            emit_i!(BuildString(defs[0], base, *n));
        }
        BuildSlice(_, _) => {
            let base = scratch_base;
            copy_window!(base, uses);
            emit_i!(BuildSlice(defs[0], base));
        }
        GetSlice(_, _, _) => {
            let obj = uses[0];
            let base = scratch_base;
            copy_window!(base, &uses[1..4]);
            emit_i!(GetSlice(defs[0], obj, base));
        }
        BuildDict(_, _, n, hint) => {
            let base = scratch_base;
            copy_window!(base, uses);
            emit_i!(BuildDict(defs[0], base, *n, *hint));
        }
        Concat { count, .. } => {
            let base = scratch_base;
            copy_window!(base, uses);
            emit_i!(Concat {
                dst: defs[0],
                base,
                count: *count,
            });
        }
        MakeFunction(_, proto_idx, _, defs_n, _, annots_n) => {
            let dn = *defs_n as usize;
            let an = *annots_n as usize;
            let defs_base = scratch_base;
            copy_window!(defs_base, &uses[..dn]);
            let annots_base = defs_base + *defs_n;
            if an > 0 {
                copy_window!(annots_base, &uses[dn..dn + an]);
            }
            emit_i!(MakeFunction(
                defs[0],
                *proto_idx,
                defs_base,
                *defs_n,
                annots_base,
                *annots_n,
            ));
        }
        MakeClass(_, proto_idx, _, bases_n, name_idx, _, kwarg_n) => {
            let bn = *bases_n as usize;
            let kn = *kwarg_n as usize;
            let bases_base = scratch_base;
            copy_window!(bases_base, &uses[..bn]);
            let kwarg_base = bases_base + *bases_n;
            if kn > 0 {
                copy_window!(kwarg_base, &uses[bn..bn + kn]);
            }
            emit_i!(MakeClass(
                defs[0], *proto_idx, bases_base, *bases_n, *name_idx, kwarg_base, *kwarg_n,
            ));
        }
        MakeClassMeta(_, proto_idx, _, bases_n, name_idx, _, kwarg_n, _) => {
            let meta_reg = uses[0];
            let bn = *bases_n as usize;
            let kn = *kwarg_n as usize;
            let bases_base = scratch_base;
            copy_window!(bases_base, &uses[1..1 + bn]);
            let kwarg_base = bases_base + *bases_n;
            if kn > 0 {
                copy_window!(kwarg_base, &uses[1 + bn..1 + bn + kn]);
            }
            emit_i!(MakeClassMeta(
                defs[0], *proto_idx, bases_base, *bases_n, *name_idx, kwarg_base, *kwarg_n,
                meta_reg,
            ));
        }

        // Multi-def contiguous windows.
        Unpack(_, _, n) => {
            let base = scratch_base;
            if *n > 0 {
                *max_reg_used = (*max_reg_used).max(base + *n - 1);
            }
            emit_i!(Unpack(base, uses[0], *n));
            for (idx, &dr) in defs.iter().enumerate() {
                let sr = base + idx as u32;
                if dr != sr {
                    emit_i!(Move(dr, sr));
                }
            }
        }
        UnpackEx { before, after, .. } => {
            let total = *before as u32 + 1 + *after;
            let base = scratch_base;
            if total > 0 {
                *max_reg_used = (*max_reg_used).max(base + total - 1);
            }
            emit_i!(UnpackEx {
                src: uses[0],
                before: *before,
                after: *after,
                dst_base: base,
            });
            for (idx, &dr) in defs.iter().enumerate() {
                let sr = base + idx as u32;
                if dr != sr {
                    emit_i!(Move(dr, sr));
                }
            }
        }
        MatchClassPositional { n, .. } => {
            let base = scratch_base;
            if *n > 0 {
                *max_reg_used = (*max_reg_used).max(base + *n - 1);
            }
            emit_i!(MatchClassPositional {
                dst_base: base,
                subj: uses[0],
                cls: uses[1],
                n: *n,
            });
            for (idx, &dr) in defs.iter().enumerate() {
                let sr = base + idx as u32;
                if dr != sr {
                    emit_i!(Move(dr, sr));
                }
            }
        }

        other => {
            unreachable!("unhandled SideEffectInsn in MIR backend: {other:?}")
        }
    }
}
