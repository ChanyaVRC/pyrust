// ─── Copy propagation ─────────────────────────────────────────────────────────

/// Eliminate `Move(dst, src)` instructions by substituting `src` for all reads
/// of `dst` within the same basic block.
///
/// Algorithm (forward dataflow within basic blocks):
/// 1. Maintain a `copies` map: `dst → canonical_src`.
/// 2. At each jump target (instruction reachable from >1 predecessor), clear
///    `copies` — we cannot guarantee what was in `src` on all incoming paths.
/// 3. For each instruction: substitute reads of any key in `copies` with the
///    canonical source, kill entries whose key or value is overwritten, and
///    record new `Move(dst, src)` pairs.
///
/// After substitution, `Move(r, r)` becomes trivial and is removed by the
/// subsequent `pass_trivial_nop`.
///
/// Reference: GCC `-ftree-copy-prop`; Shi/Gregg/Beatty/Ertl *VEE'05*.
fn pass_copy_prop(insns: Vec<Insn>, num_locals: u32) -> Vec<Insn> {
    debug_assert_no_late_stage_insns(&insns, "pass_copy_prop");
    use std::collections::HashMap;

    let n = insns.len();
    if n == 0 {
        return insns;
    }

    // Step 1: mark all jump target indices so we can reset copies there.
    let mut is_target = vec![false; n + 1];
    is_target[0] = true; // entry point is always a target
    for (i, insn) in insns.iter().enumerate() {
        let offset: Option<i32> = match insn {
            Insn::Jump(k)
            | Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::ForIter(_, _, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => Some(*k),
            _ => None,
        };
        if let Some(k) = offset {
            let target = (i as i64 + 1 + k as i64) as usize;
            if target <= n {
                is_target[target] = true;
            }
        }
    }

    // Step 2: forward pass.
    let s = |copies: &HashMap<u32, u32>, r: u32| -> u32 { *copies.get(&r).unwrap_or(&r) };

    let mut copies: HashMap<u32, u32> = HashMap::new();
    let mut result: Vec<Insn> = Vec::with_capacity(n);

    for (i, insn) in insns.into_iter().enumerate() {
        if is_target[i] {
            copies.clear();
        }

        // Substitute source registers and collect the (possibly modified) instruction.
        let insn = match insn {
            Insn::Move(dst, src) => Insn::Move(dst, s(&copies, src)),
            // CopyReg: substitute the source register (may itself be an alias) but do
            // NOT record a new copy-propagation alias — downstream passes should see
            // CopyReg as an opaque assignment, not a transparent rename.
            Insn::CopyReg(dst, src) => Insn::CopyReg(dst, s(&copies, src)),
            Insn::Return(src) => Insn::Return(s(&copies, src)),
            Insn::PrintExpr(v) => Insn::PrintExpr(s(&copies, v)),
            Insn::RaiseValue(v) => Insn::RaiseValue(s(&copies, v)),
            Insn::RaiseExceptStarResidual(v) => Insn::RaiseExceptStarResidual(s(&copies, v)),
            Insn::RaiseAssert(v) => Insn::RaiseAssert(s(&copies, v)),
            Insn::RaiseFrom(exc, cause) => Insn::RaiseFrom(s(&copies, exc), s(&copies, cause)),
            Insn::JumpIfFalse(cond, k) => Insn::JumpIfFalse(s(&copies, cond), k),
            Insn::JumpIfTrue(cond, k) => Insn::JumpIfTrue(s(&copies, cond), k),
            Insn::UnaryOp(dst, op, src) => Insn::UnaryOp(dst, op, s(&copies, src)),
            Insn::BinOp(dst, lhs, op, rhs) => {
                Insn::BinOp(dst, s(&copies, lhs), op, s(&copies, rhs))
            }
            Insn::BinOpInPlace(dst, lhs, op, rhs) => {
                Insn::BinOpInPlace(dst, s(&copies, lhs), op, s(&copies, rhs))
            }
            // For an augmented BinOpConst/BinOpImm (is_aug == true) the `lhs`
            // register is the in-place target (`x op= c`); do NOT copy-propagate
            // it, since the mutation must apply to that exact register.  Plain
            // (is_aug == false) fused ops are pure and may have lhs substituted.
            // The `is_aug` flag is carried through unchanged (issue #1874).
            Insn::BinOpConst(dst, lhs, op, c, is_aug) => {
                let new_lhs = if is_aug { lhs } else { s(&copies, lhs) };
                Insn::BinOpConst(dst, new_lhs, op, c, is_aug)
            }
            Insn::BinOpImm(dst, lhs, op, imm, is_aug) => {
                let new_lhs = if is_aug { lhs } else { s(&copies, lhs) };
                Insn::BinOpImm(dst, new_lhs, op, imm, is_aug)
            }
            Insn::CmpJumpIfFalse(lhs, op, rhs, k) => {
                Insn::CmpJumpIfFalse(s(&copies, lhs), op, s(&copies, rhs), k)
            }
            Insn::CmpJumpIfTrue(lhs, op, rhs, k) => {
                Insn::CmpJumpIfTrue(s(&copies, lhs), op, s(&copies, rhs), k)
            }
            Insn::CmpJumpIfFalseConst(lhs, op, c, k) => {
                Insn::CmpJumpIfFalseConst(s(&copies, lhs), op, c, k)
            }
            Insn::CmpJumpIfTrueConst(lhs, op, c, k) => {
                Insn::CmpJumpIfTrueConst(s(&copies, lhs), op, c, k)
            }
            // In-place mutation instructions: substitute only the VALUE arg, not the
            // container/receiver — substituting the receiver would redirect the
            // mutation to the original allocation (copy propagation is only valid for
            // reads; deep-copied containers are independent allocations).
            Insn::SetAdd(st, val) => Insn::SetAdd(st, s(&copies, val)),
            Insn::ListAppend(lst, val) => Insn::ListAppend(lst, s(&copies, val)),
            Insn::ListExtend(lst, src) => Insn::ListExtend(lst, s(&copies, src)),
            Insn::DictUpdate(dct, other) => Insn::DictUpdate(dct, s(&copies, other)),
            Insn::DictMergeKwCall { dict, src, name } => Insn::DictMergeKwCall {
                dict,
                src: s(&copies, src),
                name: kwcall_name_subst(name, |r| s(&copies, r)),
            },
            Insn::SetItemKwCall {
                dict,
                key,
                val,
                name,
            } => Insn::SetItemKwCall {
                dict,
                key: s(&copies, key),
                val: s(&copies, val),
                name: kwcall_name_subst(name, |r| s(&copies, r)),
            },
            Insn::SetAttr(obj, n, val) => Insn::SetAttr(obj, n, s(&copies, val)),
            Insn::SetTypeVarAttr(obj, n, val) => Insn::SetTypeVarAttr(obj, n, s(&copies, val)),
            Insn::DeleteAttr(obj, n) => Insn::DeleteAttr(obj, n),
            Insn::SetItem(obj, idx, val) => Insn::SetItem(obj, s(&copies, idx), s(&copies, val)),
            Insn::DeleteItem(obj, idx) => Insn::DeleteItem(obj, s(&copies, idx)),
            Insn::GetAttr(dst, obj, n) => Insn::GetAttr(dst, s(&copies, obj), n),
            Insn::GetAttrForWith(dst, obj, n, me) => {
                Insn::GetAttrForWith(dst, s(&copies, obj), n, me)
            }
            Insn::ImportFromAttr(dst, obj, n) => Insn::ImportFromAttr(dst, s(&copies, obj), n),
            Insn::GetItem(dst, obj, idx) => Insn::GetItem(dst, s(&copies, obj), s(&copies, idx)),
            Insn::GetIter(slot, src) => Insn::GetIter(slot, s(&copies, src)),
            Insn::BuildListReserve(dst, src) => Insn::BuildListReserve(dst, s(&copies, src)),
            Insn::GetAwaitable(dst, src) => Insn::GetAwaitable(dst, s(&copies, src)),
            Insn::Unpack(dst, src, n) => Insn::Unpack(dst, s(&copies, src), n),
            Insn::UnpackEx {
                src,
                before,
                after,
                dst_base,
            } => Insn::UnpackEx {
                src: s(&copies, src),
                before,
                after,
                dst_base,
            },
            Insn::CheckLocal(r, n) => Insn::CheckLocal(s(&copies, r), n),
            Insn::MatchExcept(r, k) => Insn::MatchExcept(s(&copies, r), k),
            Insn::MatchExceptStar(r, src, dst, k) => {
                Insn::MatchExceptStar(s(&copies, r), s(&copies, src), s(&copies, dst), k)
            }
            Insn::StoreGlobal(n, src) => Insn::StoreGlobal(n, s(&copies, src)),
            Insn::StoreCell(n, src) => Insn::StoreCell(n, s(&copies, src)),
            Insn::SyncModuleGlobal(reg, name_idx) => {
                Insn::SyncModuleGlobal(s(&copies, reg), name_idx)
            }
            Insn::YieldFrom {
                iter_reg,
                sent_reg,
                result_reg,
            } => Insn::YieldFrom {
                iter_reg: s(&copies, iter_reg),
                sent_reg: s(&copies, sent_reg),
                result_reg,
            },
            // Call/BuildList/BuildTuple/etc. use a base register for a range of args;
            // do not substitute the base register as that would misalign the arg block.
            other => other,
        };

        // Python-reentry invalidation: a user callback can write a new value
        // directly into a module/exec fastlocal register (r < num_locals)
        // through a live namespace mirror.  This is not limited to explicit
        // Call instructions: arithmetic, comparison, attribute/item access,
        // iteration, formatting, hashing, and other protocols can all execute
        // Python code.  Any copy-propagation alias whose *key* is a named-local
        // register is therefore stale after such an instruction.
        //
        // We also evict entries whose *value* is a named local, because the
        // value register is the "canonical source" used in substitution; if
        // it was mutated by the callee, downstream reads that copy-prop
        // redirected to it would see the wrong (pre-call) value.
        //
        // Temporaries (r >= num_locals) are safe to retain — namespace aliases
        // cannot reach them.  Keep the effect classification shared with the
        // other named-local fact trackers so newly-added protocol opcodes are
        // barriers by default.
        if may_invalidate_named_locals(&insn) {
            copies.retain(|k, v| *k >= num_locals && *v >= num_locals);
        }

        // Kill map entries: any key or value that == dst is stale after a write.
        if let Some(dst) = writable_dst(&insn) {
            copies.retain(|k, v| *k != dst && *v != dst);
        }
        // YieldFrom writes both result_reg and sent_reg; writable_dst cannot
        // express two destinations, so evict them manually.
        if let Insn::YieldFrom {
            result_reg,
            sent_reg,
            ..
        } = &insn
        {
            copies.retain(|k, v| {
                *k != *result_reg && *v != *result_reg && *k != *sent_reg && *v != *sent_reg
            });
        }
        // LoadConst writes dst (not in writable_dst so handled here).
        if let Insn::LoadConst(dst, _) = &insn {
            copies.retain(|k, v| *k != *dst && *v != *dst);
        }
        // Unpack writes dst..dst+n; kill the entire range.
        if let Insn::Unpack(dst, _, n) = &insn {
            let lo = *dst;
            let hi = dst + n;
            copies.retain(|k, v| (*k < lo || *k >= hi) && (*v < lo || *v >= hi));
        }
        // UnpackEx writes dst_base..dst_base+before+1+after; kill the entire range.
        if let Insn::UnpackEx {
            before,
            after,
            dst_base,
            ..
        } = &insn
        {
            let lo = *dst_base;
            let hi = dst_base + *before as u32 + 1 + *after;
            copies.retain(|k, v| (*k < lo || *k >= hi) && (*v < lo || *v >= hi));
        }
        // Move(dst, src): kill stale aliases THEN record the new copy.
        // Killing is necessary because overwriting `dst` invalidates any
        // existing alias that names `dst` as its source (e.g. `x → dst`).
        if let Insn::Move(dst, src) = &insn {
            copies.retain(|k, v| *k != *dst && *v != *dst);
            let canonical = *copies.get(src).unwrap_or(src);
            if dst != &canonical {
                copies.insert(*dst, canonical);
            }
        }

        result.push(insn);
    }
    result
}
