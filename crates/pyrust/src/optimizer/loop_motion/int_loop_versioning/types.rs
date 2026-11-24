// ─── Int-loop versioning types, policies, and small opcode helpers ────────────
//
// Out-of-line int specialization for innermost counted loops, per
// ARCHITECTURE.md rule 29.
//
// A module-scope loop body executes `SyncModuleGlobal` after every fast-local
// assignment so that live namespace aliases observe each iteration.  Rule 29
// permits deferring those syncs to the loop exits only when every operation in
// the loop is a proven non-reentrant primitive.  That proof cannot be purely
// static — a namespace-mirror write (rule 27) can rebind any module fast-local
// to an arbitrary object between loop entries — so this pass emits a
// runtime-guarded out-of-line copy instead of rewriting the loop in place.
//
// Supported shapes:
// - Inverted while loops (`CmpJump*` / `JumpIf*` header + matching back-edge).
// - `for … in range(…)` (int-range cursor) and `for … in <list|tuple>`
//   (indexed-seq cursor with per-iteration `JumpIfNotInt` side exits).
// - Straight-line linear accumulations over constant `range(…)` folded into a
//   closed-form copy guarded by `JumpIfIterNotIntRangeExact`.
// - `while i < len(seq):` headers guarded by `JumpIfNotBuiltinLen` +
//   `LenSeqOrExit` with `CountCmpJump*` latch fusion.
// - Canonical sequence subscripts lowered to `GetItemSeqIntOrExit` side exits.

const INT_LOOP_MAX_REGION: usize = 48;
const INT_LOOP_MAX_GUARDS: usize = 8;
/// `LoadGlobal len` + `Move` + `Call` — the instructions a `len(seq)` loop
/// bound occupies ahead of the comparison that tests it.
const INT_LOOP_LEN_HEADER_TRIPLE: usize = 3;

struct IntLoopVersioningResult {
    insns: Vec<Insn>,
    /// Exclusive end of the source-derived main stream. Instructions after
    /// this boundary are out-of-line fast copies and must never participate in
    /// source-origin matching, even when they are structurally identical to an
    /// instruction emitted by the compiler.
    source_prefix_len: usize,
}

/// The compile-time closed form of a counted loop whose body is a proven
/// linear accumulation over a constant `range`.
///
/// Every field is derived from the `(start, stop, step)` triple in `guard`, so
/// the copy this describes is correct exactly when that guard matches at
/// runtime.  Nothing here depends on the *provenance* of the range: the
/// argument trace only proposes a triple, and the runtime guard confirms the
/// iterator really is it.
struct ClosedForm {
    /// Exact iterator state the copy is specialized for.
    guard: IntRangeExactGuard,
    /// The loop variable, and the last value the range yields into it.  The
    /// original loop leaves that value bound after a non-empty run.
    var: Reg,
    var_final: i64,
    /// `(accumulator, total delta)` in first-write order, matching the order
    /// the exit stub's deferred syncs publish them.
    acc_deltas: Vec<(Reg, i64)>,
    /// Constant-pool slots allocated once the candidate search releases the
    /// pool: `var_final` first, then one per accumulator delta.
    const_slots: Vec<u16>,
}

/// The single typed policy for instructions admitted by the int-loop region
/// walk. Admission consumes this classification instead of matching `Insn`
/// independently, and the len-hoist proof is an exhaustive method on the same
/// enum. Adding a new eligible opcode therefore requires an explicit safety
/// decision before the code compiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IntLoopRegionInsn {
    Copy {
        dst: Reg,
        src: Reg,
    },
    LoadConst {
        dst: Reg,
        idx: u16,
    },
    Binary {
        dst: Reg,
        lhs: Reg,
        op: BinaryOp,
        rhs: Reg,
    },
    BinaryImm {
        dst: Reg,
        src: Reg,
        op: BinaryOp,
        imm: i16,
    },
    BinaryConst {
        dst: Reg,
        src: Reg,
        op: BinaryOp,
        idx: u16,
    },
    Compare {
        lhs: Reg,
        op: BinaryOp,
        rhs: Reg,
        offset: i32,
    },
    CompareConst {
        lhs: Reg,
        op: BinaryOp,
        idx: u16,
        offset: i32,
    },
    Truthiness {
        reg: Reg,
        offset: i32,
    },
    Jump {
        offset: i32,
    },
    Subscript {
        dst: Reg,
        obj: Reg,
        idx: Reg,
    },
    Sync {
        reg: Reg,
        name_idx: u16,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LenHoistSafety {
    ProvenNonMutating,
    SequenceRegisterWrite,
}

impl IntLoopRegionInsn {
    /// Classify the additional invariant required to cache `len(seq)`. The
    /// region walk separately checks the int operators/constants and rewrites
    /// `Subscript` to `GetItemSeqIntOrExit`; every represented operation is
    /// consequently non-reentrant and non-mutating except that a destination
    /// write may replace the sequence register itself.
    fn len_hoist_safety(self, seq: Reg) -> LenHoistSafety {
        let destination = match self {
            Self::Copy { dst, .. }
            | Self::LoadConst { dst, .. }
            | Self::Binary { dst, .. }
            | Self::BinaryImm { dst, .. }
            | Self::BinaryConst { dst, .. }
            | Self::Subscript { dst, .. } => Some(dst),
            Self::Compare { .. }
            | Self::CompareConst { .. }
            | Self::Truthiness { .. }
            | Self::Jump { .. }
            | Self::Sync { .. } => None,
        };
        if destination == Some(seq) {
            LenHoistSafety::SequenceRegisterWrite
        } else {
            LenHoistSafety::ProvenNonMutating
        }
    }
}

fn classify_int_loop_region_insn(insn: &Insn) -> Option<IntLoopRegionInsn> {
    Some(match *insn {
        Insn::Move(dst, src) => IntLoopRegionInsn::Copy { dst, src },
        Insn::LoadConst(dst, idx) => IntLoopRegionInsn::LoadConst { dst, idx },
        Insn::BinOp(dst, lhs, op, rhs) | Insn::BinOpInPlace(dst, lhs, op, rhs) => {
            IntLoopRegionInsn::Binary { dst, lhs, op, rhs }
        }
        Insn::BinOpImm(dst, src, op, imm, _) => IntLoopRegionInsn::BinaryImm { dst, src, op, imm },
        Insn::BinOpConst(dst, src, op, idx, _) => {
            IntLoopRegionInsn::BinaryConst { dst, src, op, idx }
        }
        Insn::CmpJumpIfFalse(lhs, op, rhs, offset) | Insn::CmpJumpIfTrue(lhs, op, rhs, offset) => {
            IntLoopRegionInsn::Compare {
                lhs,
                op,
                rhs,
                offset,
            }
        }
        Insn::CmpJumpIfFalseConst(lhs, op, idx, offset)
        | Insn::CmpJumpIfTrueConst(lhs, op, idx, offset) => IntLoopRegionInsn::CompareConst {
            lhs,
            op,
            idx,
            offset,
        },
        Insn::JumpIfFalse(reg, offset) | Insn::JumpIfTrue(reg, offset) => {
            IntLoopRegionInsn::Truthiness { reg, offset }
        }
        Insn::Jump(offset) => IntLoopRegionInsn::Jump { offset },
        Insn::GetItem(dst, obj, idx) => IntLoopRegionInsn::Subscript { dst, obj, idx },
        Insn::SyncModuleGlobal(reg, name_idx) => IntLoopRegionInsn::Sync { reg, name_idx },
        _ => return None,
    })
}

enum IntLoopCandidateKind {
    /// Inverted while loop: conditional header, conditional back-edge.
    Inverted,
    /// `for … in <iterable>`: `ForIter` header, unconditional back-edge;
    /// the guard block additionally checks the iterator slot's kind.
    ForIter { slot: u8 },
    /// `while <i> < len(<seq>):` — a header the loop re-evaluates in full
    /// on every iteration, so `pass_loop_inversion` cannot collapse its
    /// back-edge and the region opens with the call triple rather than
    /// with a comparison.
    LenHeaderWhile {
        /// Call base the header's `LoadGlobal` writes; also the register
        /// the comparison reads the length from.
        callee: Reg,
        /// The register holding the sequence. Hoisted copies reject any
        /// region instruction that writes it.
        seq: Reg,
        /// Name-table slot of `"len"`, for the guard block's re-load.
        name_idx: u16,
    },
}

/// One out-of-line copy of a candidate region, reached through its own
/// iterator-kind guard chain.
#[derive(Clone, Copy, PartialEq, Eq)]
enum IntLoopFastVariant {
    /// No iterator guard: the inverted-while copy.
    Plain,
    /// `for … in range(…)`: the canonical machine-int cursor always yields
    /// an `int`, so the loop variable needs no per-iteration guard.
    IntRange,
    /// `for … in <list|tuple>`: the element type is a per-iteration fact,
    /// so the copy opens with a `JumpIfNotInt` side exit on the loop
    /// variable.
    IndexedSeq,
    /// `for … in range(<int constants>)` with a provably linear body: the
    /// copy is the loop's closed form, so it runs in constant time no
    /// matter how many iterations the range describes.  Its guard pins the
    /// exact cursor state, not just the kind.
    ClosedForm,
}

/// The latch of a `len`-header copy: the counter increment the copy folds
/// into the comparison against the guarded entry length.
struct IntLoopLenLatch {
    /// Old index of the `BinOpImm(v, v, Add, imm)` the fusion consumes.
    add: usize,
    var: Reg,
    op: BinaryOp,
    imm: i16,
    /// Whether the fused latch jumps back on a true comparison.
    jump_when_true: bool,
}

struct IntLoopCandidate {
    kind: IntLoopCandidateKind,
    head: usize,
    back: usize,
    guards: Vec<Reg>,
    syncs: Vec<(Reg, u16)>,
    /// `(tmp_reg, const_idx)`: the back-edge compares against a constant
    /// pool slot; the guard block materialises it once into `tmp_reg` so
    /// the fused `CountCmpJump*` can use the register form.
    const_fuse: Option<(Reg, u16)>,
    /// One appended fast copy per entry, in guard-chain order.
    variants: Vec<IntLoopFastVariant>,
    /// Present when the body folds to a closed form; drives the
    /// `IntLoopFastVariant::ClosedForm` copy at the head of the guard chain.
    closed_form: Option<ClosedForm>,
    /// Present when a `len` header's copy fuses its increment and comparison.
    len_latch: Option<IntLoopLenLatch>,
}

impl IntLoopCandidate {
    /// Instructions the guard block contributes ahead of the original head.
    fn guard_block_len(&self) -> usize {
        let per_variant =
            self.shape_guard_len() + self.guards.len() + usize::from(self.const_fuse.is_some()) + 1;
        self.variants.len() * per_variant
    }

    /// Guard-block instructions the candidate's *shape* contributes, ahead
    /// of the per-register `JumpIfNotInt` chain.
    fn shape_guard_len(&self) -> usize {
        match self.kind {
            IntLoopCandidateKind::Inverted => 0,
            IntLoopCandidateKind::ForIter { .. } => 1,
            // `LoadGlobal len` + value guard + native length read.
            IntLoopCandidateKind::LenHeaderWhile { .. } => 3,
        }
    }

    /// Index of the comparison that decides whether the loop body runs.
    /// It is the region's first instruction except for a `len` header,
    /// whose call triple precedes it.
    fn header(&self) -> usize {
        match self.kind {
            IntLoopCandidateKind::LenHeaderWhile { .. } => self.head + INT_LOOP_LEN_HEADER_TRIPLE,
            _ => self.head,
        }
    }
}

/// A fast-copy entry: either a rewritten copy of the original instruction
/// at some old index, or a synthesized guard whose failure edge is the
/// deferred-sync side-exit stub for the given old index.
enum IntLoopFastEntry {
    Copied(usize, Insn),
    SideExit(usize, Insn),
}

/// Jump target of an instruction, as an absolute index.
#[inline]
fn int_loop_target(i: usize, insn: &Insn) -> Option<usize> {
    let off = insn_jump_off(insn)?;
    Some((i as i64 + 1 + off as i64) as usize)
}

/// Whether `insn` writes register `r`.  Whitelisted region instructions report
/// their real destination; anything else is assumed to clobber.
#[inline]
fn int_loop_writes_reg(insn: &Insn, r: Reg) -> bool {
    match insn {
        Insn::Jump(..)
        | Insn::JumpIfFalse(..)
        | Insn::JumpIfTrue(..)
        | Insn::CmpJumpIfFalse(..)
        | Insn::CmpJumpIfTrue(..)
        | Insn::CmpJumpIfFalseConst(..)
        | Insn::CmpJumpIfTrueConst(..)
        | Insn::SyncModuleGlobal(..) => false,
        Insn::LoadConst(dst, _)
        | Insn::Move(dst, _)
        | Insn::BinOp(dst, ..)
        | Insn::BinOpInPlace(dst, ..)
        | Insn::BinOpImm(dst, ..)
        | Insn::BinOpConst(dst, ..)
        | Insn::GetItem(dst, ..)
        | Insn::ForIter(dst, ..) => *dst == r,
        // Outside the region whitelist: assume it clobbers.
        _ => true,
    }
}

#[inline]
fn int_loop_const_is_int(consts: &[Value], idx: u16) -> bool {
    consts
        .get(idx as usize)
        .is_some_and(|v| !v.is_unset() && matches!(v.kind(), ValueKind::Int(_)))
}

#[inline]
fn int_loop_cmp_op(op: &BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
    )
}

/// Int-family-closed operations that cannot raise on int operands.
#[inline]
fn int_loop_arith_op(op: &BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::BitAnd
            | BinaryOp::BitOr
            | BinaryOp::BitXor
    )
}

/// Division-family ops additionally require a known non-zero divisor, so they
/// are admitted only in immediate/constant form.
#[inline]
fn int_loop_imm_arith_op(op: &BinaryOp, imm: i16) -> bool {
    int_loop_arith_op(op) || (matches!(op, BinaryOp::FloorDiv | BinaryOp::Mod) && imm != 0)
}

#[inline]
fn int_loop_const_arith_op(consts: &[Value], op: &BinaryOp, idx: u16) -> bool {
    int_loop_arith_op(op)
        || (matches!(op, BinaryOp::FloorDiv | BinaryOp::Mod)
            && consts
                .get(idx as usize)
                .and_then(|v| if v.is_unset() { None } else { v.as_int() })
                .is_some_and(|value| value != 0))
}

/// Replace the single jump offset carried by `insn` with `off`.
fn replace_jump_offset(insn: Insn, off: i32) -> Insn {
    use Insn::*;
    match insn {
        Jump(_) => Jump(off),
        JumpIfFalse(r, _) => JumpIfFalse(r, off),
        JumpIfTrue(r, _) => JumpIfTrue(r, off),
        JumpIfNotInt(r, _) => JumpIfNotInt(r, off),
        JumpIfIterNotIntRange(s2, _) => JumpIfIterNotIntRange(s2, off),
        JumpIfIterNotIndexedSeq(s2, _) => JumpIfIterNotIndexedSeq(s2, off),
        JumpIfIterNotIntRangeExact(guard, _) => JumpIfIterNotIntRangeExact(guard, off),
        GetItemSeqIntOrExit(dst, obj, idx, _) => GetItemSeqIntOrExit(
            dst,
            obj,
            idx,
            i16::try_from(off).expect("GetItemSeqIntOrExit side-exit offset fits i16"),
        ),
        JumpIfNotBuiltinLen(r, _) => JumpIfNotBuiltinLen(r, off),
        LenSeqOrExit(dst, seq, _) => LenSeqOrExit(dst, seq, off),
        CmpJumpIfFalse(a, op, b, _) => CmpJumpIfFalse(a, op, b, off),
        CmpJumpIfTrue(a, op, b, _) => CmpJumpIfTrue(a, op, b, off),
        CmpJumpIfFalseConst(a, op, c, _) => CmpJumpIfFalseConst(a, op, c, off),
        CmpJumpIfTrueConst(a, op, c, _) => CmpJumpIfTrueConst(a, op, c, off),
        CountCmpJumpTrue(v, op, s, imm, _) => CountCmpJumpTrue(v, op, s, imm, off),
        CountCmpJumpFalse(v, op, s, imm, _) => CountCmpJumpFalse(v, op, s, imm, off),
        CallInlineBinOp(desc, _) => CallInlineBinOp(desc, off),
        ForIter(dst, slot, _) => ForIter(dst, slot, off),
        SetupExcept(_) => SetupExcept(off),
        MatchExcept(r, _) => MatchExcept(r, off),
        MatchExceptStar(desc, _) => MatchExceptStar(desc, off),
        other => other,
    }
}
