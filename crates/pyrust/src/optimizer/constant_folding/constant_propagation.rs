// ─── Constant folding ──────────────────────────────────────────────────────────

/// Forward dataflow constant folding.
///
/// Tracks registers whose values are statically known (`known: reg → const_idx`).
/// When both operands of a `BinOp` or `BinOpConst` are known, replace the
/// instruction with a `LoadConst` of the folded result.  Also propagates known
/// values through `Move(dst, src)`.
///
/// The map is cleared at branch/loop instructions where we cannot guarantee
/// which path was taken at runtime, and also at loop headers (targets of
/// backward jumps) to avoid incorrectly folding loop conditions.  Named-local
/// facts are additionally invalidated after every instruction that can execute
/// Python code or cross a namespace-storage boundary: live namespace aliases
/// can update those registers without an explicit bytecode write.
fn pass_const_fold(insns: Vec<Insn>, consts: &mut Vec<Value>, num_locals: u32) -> Vec<Insn> {
    debug_assert_no_late_stage_insns(&insns, "pass_const_fold");
    // Pre-pass: collect every instruction index that is the target of *any*
    // jump (forward or backward).  At every such basic-block boundary the
    // known-constant map must be cleared, otherwise a value that was assigned
    // along one incoming path can be incorrectly propagated to the merge
    // instruction — e.g. the `then`-arm of a ternary unconditionally jumps
    // over the `else`-arm; at the merge point the destination register's true
    // value depends on which arm ran, but a linear forward scan would see the
    // `else`-arm's write as the most recent and fold the wrong constant
    // downstream.  Loop headers (backward-jump targets) are a special case of
    // the same problem.
    let mut bb_starts: HashSet<usize> = HashSet::new();
    for (i, insn) in insns.iter().enumerate() {
        let k: Option<i32> = match insn {
            Insn::Jump(k) => Some(*k),
            Insn::JumpIfFalse(_, k)
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
            if target < insns.len() {
                bb_starts.insert(target);
            }
        }
    }

    let mut known: HashMap<u32, u16> = HashMap::new();
    let mut out = Vec::with_capacity(insns.len());
    // Hash index over the const pool so interning a folded constant is
    // amortized O(1) instead of an O(pool) linear scan.  A long foldable chain
    // (`x = x + i` × N) interns ~N fresh constants; the linear scan made the
    // whole pass O(n²) (issue #2002).
    let mut const_index = ConstIndex::build(consts);

    for (i, insn) in insns.into_iter().enumerate() {
        if bb_starts.contains(&i) {
            known.clear();
        }
        match insn {
            Insn::LoadConst(dst, c) => {
                known.insert(dst, c);
                out.push(Insn::LoadConst(dst, c));
            }
            Insn::Move(dst, src) => {
                match known.get(&src).copied() {
                    Some(c) => {
                        known.insert(dst, c);
                    }
                    None => {
                        known.remove(&dst);
                    }
                }
                out.push(Insn::Move(dst, src));
            }
            insn @ Insn::MatchClassPositional { .. } => {
                // This protocol-dispatching instruction writes a dynamic
                // register range.  Clearing all facts handles both its direct
                // destinations and re-entrant namespace writes.
                known.clear();
                out.push(insn);
            }
            Insn::BinOp(dst, lhs, op, rhs) => {
                let folded = known.get(&lhs).copied().and_then(|cl| {
                    known.get(&rhs).copied().and_then(|cr| {
                        crate::compiler::fold_binop(&consts[cl as usize], op, &consts[cr as usize])
                            .and_then(|v| const_index.intern(consts, v))
                    })
                });
                apply_const_fold(
                    &mut known,
                    &mut out,
                    dst,
                    folded,
                    Insn::BinOp(dst, lhs, op, rhs),
                );
            }
            Insn::BinOpConst(dst, lhs, op, c, is_aug) => {
                let folded = known.get(&lhs).copied().and_then(|cl| {
                    crate::compiler::fold_binop(&consts[cl as usize], op, &consts[c as usize])
                        .and_then(|v| const_index.intern(consts, v))
                });
                apply_const_fold(
                    &mut known,
                    &mut out,
                    dst,
                    folded,
                    Insn::BinOpConst(dst, lhs, op, c, is_aug),
                );
            }
            Insn::BinOpImm(dst, lhs, op, imm, is_aug) => {
                let folded = known.get(&lhs).copied().and_then(|cl| {
                    let rhs_val = Value::int(imm as i64);
                    crate::compiler::fold_binop(&consts[cl as usize], op, &rhs_val)
                        .and_then(|v| const_index.intern(consts, v))
                });
                apply_const_fold(
                    &mut known,
                    &mut out,
                    dst,
                    folded,
                    Insn::BinOpImm(dst, lhs, op, imm, is_aug),
                );
            }
            // Branch/loop/raise/suspend: clear the map — values may differ per
            // path or may be written by external resume machinery.
            insn @ (Insn::Jump(_)
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
            | Insn::Unpack(..)
            | Insn::UnpackEx { .. }
            | Insn::Yield { .. }
            | Insn::YieldFrom { .. }) => {
                known.clear();
                out.push(insn);
            }
            // Any other instruction: invalidate dst if we can identify it.
            insn => {
                if let Some(dst) = writable_dst(&insn) {
                    known.remove(&dst);
                }
                out.push(insn);
            }
        }

        // Live namespace aliases can write named-local registers while Python
        // protocol code or namespace storage runs, even though the bytecode
        // contains no write to those registers.  Apply the shared conservative
        // effect classifier to the instruction that actually remains after
        // folding: a successfully folded BinOp is now a pure LoadConst and
        // therefore keeps the propagation chain, while an unfurled runtime
        // BinOp is a barrier.
        //
        // Temps are frame-private scratch registers and cannot be reached by a
        // namespace alias, so retain their facts.  Direct destination writes
        // were invalidated by the transfer arms above.
        if may_invalidate_named_locals(
            out.last()
                .expect("constant propagation emits one instruction per input"),
        ) {
            known.retain(|&reg, _| reg >= num_locals);
        }
    }
    out
}

/// Emit a folded constant or the original instruction into `out`, updating `known`.
///
/// Called by the three BinOp folding arms in `pass_const_fold`. When `folded`
/// is `Some(nc)`, emits `LoadConst(dst, nc)` and records the known value;
/// otherwise emits `fallback` and removes `dst` from `known`.
#[inline]
fn apply_const_fold(
    known: &mut std::collections::HashMap<u32, u16>,
    out: &mut Vec<Insn>,
    dst: u32,
    folded: Option<u16>,
    fallback: Insn,
) {
    if let Some(nc) = folded {
        known.insert(dst, nc);
        out.push(Insn::LoadConst(dst, nc));
    } else {
        known.remove(&dst);
        out.push(fallback);
    }
}

/// Return the single destination register of `insn`, if any.
/// Used to precisely invalidate the `known` map without clearing it entirely.
fn writable_dst(insn: &Insn) -> Option<u32> {
    use Insn::*;
    match insn {
        LoadGlobal(r, _)
        | LoadClassName(r, _, _)
        | LoadCell(r, _)
        | LoadNone(r)
        | DeleteLocal(r, _)
        | BinOp(r, _, _, _)
        | BinOpConst(r, _, _, _, _)
        | BinOpImm(r, _, _, _, _)
        | BinOpInPlace(r, _, _, _)
        | UnaryOp(r, _, _)
        | FormatValue(r, _)
        | FormatValueSpec(r, _, _)
        | MatchSeqExcluded(r, _)
        | MatchMapping(r, _)
        | GetAttr(r, _, _)
        | GetAttrForWith(r, _, _, _)
        | ImportFromAttr(r, _, _)
        | GetItem(r, _, _)
        | GetSlice(r, _, _)
        // GetAwaitable writes the driving iterator into `r`; without this arm
        // copy-prop fails to kill a `Move(r, src)` alias on `r`, mis-substituting
        // a later read of the iterator (e.g. `YieldFrom.iter_reg`) back to `src`
        // — surfaced by `await f(…, kw=v)`, whose variadic-call lowering emits an
        // arg `Move` into the slot that becomes the await iterator (issue #2298).
        | GetAwaitable(r, _)
        | Call(r, _)
        | CallMemo(r, _)
        | CallKw { func: r, .. }
        | CallEx { func: r, .. }
        | CallExArgs { func: r, .. }
        | BuildList(r, _, _)
        | BuildListReserve(r, _)
        | BuildTuple(r, _, _)
        | BuildString(r, _, _)
        | BuildSlice(r, _)
        | BuildDict(r, _, _, _)
        | MakeFunction(r, _, _, _, _, _)
        | ImportModule(r, _)
        | LoadExc(r)
        | LoadExcTraceback(r, _)
        | MakeClass(r, _, _, _, _, _, _)
        | MakeClassMeta(r, _, _, _, _, _, _, _)
        | MakeTypeAlias(r, _, _, _)
        | MakeTypeVar(r, _) => Some(*r),
        CallMethod { dst, .. }
        | CallMethodKw { dst, .. }
        | CallMethodExpanded { dst, .. }
        | Concat { dst, .. }
        // Yield writes the caller's sent value into `dst` on resume; aliases
        // through `dst` are stale after this instruction.
        | Yield { dst, .. } => Some(*dst),
        // ForIter writes its destination on each successful iteration.
        ForIter(dst, _, _) => Some(*dst),
        // CopyReg is emitted by the CSE pass; it writes to dst just like Move.
        CopyReg(r, _) => Some(*r),
        _ => None,
    }
}

/// A hashable, type-exact dedup key for a constant pool `Value`.
///
/// Mirrors the equality semantics of `intern_const_in_pool`'s linear scan:
/// `Bool` and `Int` never collide (distinct variants), floats/complex compare
/// by raw bits so NaN-keyed constants share a slot, and every other type
/// returns `None` (no key → never deduplicated, exactly like the `_ => false`
/// arm of the linear scan).
#[derive(PartialEq, Eq, Hash)]
enum ConstKey {
    Int(i64),
    BigInt(Vec<u8>),
    FloatBits(u64),
    ComplexBits(u64, u64),
    Str(String),
    Bytes(Vec<u8>),
    Bool(bool),
    None,
}

fn const_key(val: &Value) -> Option<ConstKey> {
    Some(match val.kind() {
        ValueKind::Int(a) => ConstKey::Int(a),
        // BigInt has no `Hash`; key on its big-endian byte serialization, which
        // is a 1:1 representation for the `==` used by the linear scan.
        ValueKind::BigInt(a) => ConstKey::BigInt(a.to_signed_bytes_be()),
        ValueKind::Float(a) => ConstKey::FloatBits(a.to_bits()),
        ValueKind::Complex(ar, ai) => ConstKey::ComplexBits(ar.to_bits(), ai.to_bits()),
        ValueKind::Str(a) => ConstKey::Str(a.to_owned()),
        ValueKind::Bytes(a) => ConstKey::Bytes(a.as_ref().clone()),
        ValueKind::Bool(a) => ConstKey::Bool(a),
        ValueKind::None => ConstKey::None,
        _ => return None,
    })
}

/// Hash index over a constant pool for amortized-O(1) interning.
///
/// `intern_const_in_pool` is otherwise an O(pool) linear scan per call; a pass
/// that folds a long def-use chain (`x = x + i` × N) interns ~N fresh constants,
/// driving the whole pass to O(n²) (issue #2002).  This index maps each
/// dedup-able `ConstKey` to the *first* (lowest) pool slot holding it, matching
/// the linear scan's "first match wins" behaviour exactly, so the resulting pool
/// indices are identical.
struct ConstIndex {
    map: HashMap<ConstKey, u16>,
}

impl ConstIndex {
    /// Build the index from the existing pool contents.
    fn build(consts: &[Value]) -> Self {
        let mut map = HashMap::with_capacity(consts.len());
        for (i, v) in consts.iter().enumerate() {
            if let Some(k) = const_key(v)
                && let Ok(idx) = u16::try_from(i)
            {
                // First occurrence wins (matches linear-scan ordering).
                map.entry(k).or_insert(idx);
            }
        }
        ConstIndex { map }
    }

    /// Look up or insert `val`; returns its pool index (or `None` if the pool is
    /// full or `val` is not a dedup-able type and the pool is full).
    fn intern(&mut self, consts: &mut Vec<Value>, val: Value) -> Option<u16> {
        match const_key(&val) {
            Some(k) => {
                if let Some(&idx) = self.map.get(&k) {
                    return Some(idx);
                }
                if consts.len() >= u16::MAX as usize {
                    return None;
                }
                let idx = u16::try_from(consts.len()).expect("constant-pool limit checked above");
                consts.push(val);
                self.map.insert(k, idx);
                Some(idx)
            }
            // Non-dedup-able type: never shares a slot (matches `_ => false`).
            None => {
                if consts.len() >= u16::MAX as usize {
                    return None;
                }
                let idx = u16::try_from(consts.len()).expect("constant-pool limit checked above");
                consts.push(val);
                Some(idx)
            }
        }
    }
}

/// Look up or insert `val` in the const pool; return its index.
/// Returns `None` if the pool is full (>= u16::MAX entries).
fn intern_const_in_pool(consts: &mut Vec<Value>, val: Value) -> Option<u16> {
    // Type-exact linear scan to avoid Bool/Int key collisions and to handle
    // non-hashable types (Complex, Bytes) that cannot use a HashMap fast path.
    for (i, existing) in consts.iter().enumerate() {
        let same = match (existing.kind(), val.kind()) {
            (ValueKind::Int(a), ValueKind::Int(b)) => a == b,
            (ValueKind::BigInt(a), ValueKind::BigInt(b)) => a == b,
            (ValueKind::Float(a), ValueKind::Float(b)) => a.to_bits() == b.to_bits(),
            // Bit-level comparison so that NaN-keyed constants share a slot.
            (ValueKind::Complex(ar, ai), ValueKind::Complex(br, bi)) => {
                ar.to_bits() == br.to_bits() && ai.to_bits() == bi.to_bits()
            }
            (ValueKind::Str(a), ValueKind::Str(b)) => a == b,
            (ValueKind::Bytes(a), ValueKind::Bytes(b)) => a.as_ref() == b.as_ref(),
            (ValueKind::Bool(a), ValueKind::Bool(b)) => a == b,
            (ValueKind::None, ValueKind::None) => true,
            _ => false,
        };
        if same {
            return u16::try_from(i).ok();
        }
    }
    if consts.len() >= u16::MAX as usize {
        return None;
    }
    let idx = u16::try_from(consts.len()).expect("constant-pool limit checked above");
    consts.push(val);
    Some(idx)
}
