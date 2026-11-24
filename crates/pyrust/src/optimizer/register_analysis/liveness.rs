// ─── Register liveness helpers ────────────────────────────────────────────────

/// Returns `true` if `insn` is a backward branch (offset `< 0`), i.e. a loop
/// back-edge.
fn insn_is_back_edge(insn: &Insn) -> bool {
    matches!(insn,
        Insn::Jump(k)
        | Insn::JumpIfFalse(_, k)
        | Insn::JumpIfTrue(_, k)
        | Insn::ForIter(_, _, k)
        | Insn::CmpJumpIfFalse(_, _, _, k)
        | Insn::CmpJumpIfTrue(_, _, _, k)
        | Insn::CmpJumpIfFalseConst(_, _, _, k)
        | Insn::CmpJumpIfTrueConst(_, _, _, k)
        | Insn::JumpIfNotInt(_, k)
        | Insn::JumpIfIterNotIntRange(_, k)
        | Insn::JumpIfIterNotIndexedSeq(_, k)
        | Insn::JumpIfIterNotIntRangeExact(_, k)
        | Insn::GetItemSeqIntOrExit(_, _, _, k)
        | Insn::JumpIfNotBuiltinLen(_, k)
        | Insn::LenSeqOrExit(_, _, k)
        | Insn::CountCmpJumpTrue(_, _, _, _, k)
        | Insn::CountCmpJumpFalse(_, _, _, _, k)
        | Insn::CallInlineBinOp { skip: k, .. }
        if *k < 0
    )
}

#[cfg(test)]
fn slice_has_back_edge(insns: &[Insn]) -> bool {
    insns.iter().any(insn_is_back_edge)
}

/// Used as a forward liveness guard before removing a `LoadConst` that produced `r`.
fn reg_is_read_in(insns: &[Insn], r: u32) -> bool {
    insns.iter().any(|insn| insn_reads_reg(insn, r))
}

/// Returns `true` when the value held in register `r` is provably dead starting
/// at `insns[start]` — the first instruction at or after `start` that touches
/// `r` *writes* it without first reading it.  Lets `pass_binop_const_fusion`
/// fuse the "reused scratch register" shape inside loop bodies, which the coarse
/// global `last_read[r]` and `back_edge_after` guards otherwise veto.
///
/// Conservatively returns `false` (treat as live) at the first control-flow
/// instruction: a jump-over could reach a later read of the stale value, so we
/// only reason within a straight-line run.  Scans a tiny fixed window — a
/// scratch temp is always overwritten within a couple of instructions, and an
/// unbounded scan would reintroduce the O(n²) blowup this pass avoids (#2002).
fn scratch_dead_after(insns: &[Insn], start: usize, r: u32) -> bool {
    const WINDOW: usize = 6;
    let end = (start + WINDOW).min(insns.len());
    for insn in &insns[start..end] {
        // Any branch / suspend / return ends the straight-line region.
        if matches!(
            insn,
            Insn::Jump(_)
                | Insn::JumpIfFalse(..)
                | Insn::JumpIfTrue(..)
                | Insn::CmpJumpIfFalse(..)
                | Insn::CmpJumpIfTrue(..)
                | Insn::CmpJumpIfFalseConst(..)
                | Insn::CmpJumpIfTrueConst(..)
                | Insn::ForIter(..)
                | Insn::SetupExcept(..)
                | Insn::Yield { .. }
                | Insn::YieldFrom { .. }
                | Insn::Return(..)
                | Insn::ReturnNone
        ) {
            return false;
        }
        if insn_reads_reg(insn, r) {
            return false; // read before any overwrite → value is live
        }
        let mut overwrote = false;
        for_each_write(insn, |w| {
            if w == r {
                overwrote = true;
            }
        });
        if overwrote {
            return true; // overwritten before any read → old value is dead
        }
    }
    // Hit the true end of the stream without a read → dead; otherwise (window
    // edge reached mid-stream) be conservative.
    end == insns.len()
}

/// Stack-backed bitset for dense temp-register offsets (`r - num_locals`).
/// Covers the first 256 temps with zero heap allocations and spills to a
/// `Vec<u64>` only for unusually wide frames (issue #2905).
struct TempBitset {
    inline: [u64; 4],
    overflow: Vec<u64>,
}

impl TempBitset {
    #[inline]
    fn new() -> Self {
        Self {
            inline: [0; 4],
            overflow: Vec::new(),
        }
    }

    #[inline]
    fn clear(&mut self) {
        self.inline = [0; 4];
        if !self.overflow.is_empty() {
            self.overflow.fill(0);
        }
    }

    #[inline]
    fn insert(&mut self, bit: usize) {
        if bit < 256 {
            self.inline[bit >> 6] |= 1u64 << (bit & 63);
        } else {
            let word = (bit - 256) >> 6;
            if word >= self.overflow.len() {
                self.overflow.resize(word + 1, 0);
            }
            self.overflow[word] |= 1u64 << (bit & 63);
        }
    }

    #[inline]
    fn contains(&self, bit: usize) -> bool {
        if bit < 256 {
            (self.inline[bit >> 6] & (1u64 << (bit & 63))) != 0
        } else {
            let word = (bit - 256) >> 6;
            self.overflow
                .get(word)
                .is_some_and(|&w| (w & (1u64 << (bit & 63))) != 0)
        }
    }
}

/// Small stack-backed `bool` buffer (zero heap allocation for functions with
/// `<= 128` instructions, issue #2905).
enum InlineBoolBuf {
    Inline([bool; 128]),
    Heap(Vec<bool>),
}

impl InlineBoolBuf {
    #[inline]
    fn new(len: usize, init: bool) -> Self {
        if len <= 128 {
            Self::Inline([init; 128])
        } else {
            Self::Heap(vec![init; len])
        }
    }

    #[inline]
    fn as_slice(&self) -> &[bool] {
        match self {
            Self::Inline(arr) => arr.as_slice(),
            Self::Heap(vec) => vec.as_slice(),
        }
    }

    #[inline]
    fn as_mut_slice(&mut self) -> &mut [bool] {
        match self {
            Self::Inline(arr) => arr.as_mut_slice(),
            Self::Heap(vec) => vec.as_mut_slice(),
        }
    }
}

/// Small stack-backed `usize` array for per-temp liveness slots (zero heap
/// allocation for functions with `<= 64` temporaries, issue #2905).
struct TempSlotBuf {
    inline: [usize; 64],
    heap: Vec<usize>,
}

impl TempSlotBuf {
    #[inline]
    fn new(len: usize, init: usize) -> Self {
        if len <= 64 {
            Self {
                inline: [init; 64],
                heap: Vec::new(),
            }
        } else {
            Self {
                inline: [0; 64],
                heap: vec![init; len],
            }
        }
    }

    #[inline]
    fn as_mut_slice(&mut self) -> &mut [usize] {
        if self.heap.is_empty() {
            self.inline.as_mut_slice()
        } else {
            self.heap.as_mut_slice()
        }
    }
}

/// For every index `i`, whether a `LoadConst` at `i` writes a scratch temp
/// whose loaded value is provably unread after `i + 1` on *every* path.
///
/// `pass_binop_const_fusion` folds `LoadConst(r, c); BinOp(dst, lhs, op, r)`
/// into one `BinOpConst` and drops the load, which needs exactly that fact.
/// Its other guards answer the question with whole-function approximations —
/// "is `r` read anywhere later" plus "does a back edge appear later" — and the
/// shape every `while COND: … EXPR == K …` loop compiles to always fails them:
/// the compiler reloads one scratch temp with a different constant per operand,
/// so a later reload counts as a later read, and the loop's own back edge is
/// always "later".  The surviving `LoadConst` of the loop bound then sits on
/// the back-edge target, which is what stops LICM (the temp is no longer
/// single-writer), loop inversion and int-loop versioning from recognising the
/// loop at all (issue #2889).
///
/// Two linear scans compute the precise fact:
///
/// 1. **Block-local temps.**  A temp is *block-local* when every read of it is
///    reached from its defining write by pure fall-through — no jump target
///    lies between the write and the read.  `insn_jump_off` reports
///    exception-handler entries (`SetupExcept` / `MatchExcept` /
///    `MatchExceptStar`) as well as ordinary branches, so a handler counts as
///    an entry edge too.  A block-local temp never carries a value across a
///    control-flow edge, so reaching definitions for it can be decided by a
///    straight-line walk without following jumps.
/// 2. **Reaching scan.**  For a block-local `r` defined at `i`, the loaded
///    value is observable only at indices reached from `i + 2` by fall-through,
///    up to the first jump target (past which any read is fed by its own,
///    later, definition) or the next write of `r`.  If no read of `r` comes
///    first, the definition is dead after `i + 1`.
///
/// Registers below `num_locals` are never reported: named locals are visible
/// through `globals()` / `locals()` frame views without a bytecode read.
fn load_const_dead_after_use(
    insns: &[Insn],
    num_locals: u32,
    is_jump_target: &[bool],
) -> InlineBoolBuf {
    let n = insns.len();
    let mut dead_buf = InlineBoolBuf::new(n, false);
    if n == 0 {
        return dead_buf;
    }

    // Pass 1 (forward): which temps are block-local, and how many temp-register
    // slots the reverse pass needs.  Uses zero-allocation `for_each_read` /
    // `for_each_write` visitors and stack-backed bitsets (issue #2905).
    let mut has_temp = false;
    let mut max_temp: u32 = num_locals;
    let mut defined = TempBitset::new();
    let mut escaping = TempBitset::new();
    for (idx, insn) in insns.iter().enumerate() {
        // Control can enter here from elsewhere, so nothing carries over.
        if is_jump_target[idx] {
            defined.clear();
        }
        for_each_read(insn, |r| {
            if r >= num_locals {
                has_temp = true;
                max_temp = max_temp.max(r);
                let bit = (r - num_locals) as usize;
                if !defined.contains(bit) {
                    escaping.insert(bit);
                }
            }
        });
        // Reads are folded in before writes so an instruction that both reads
        // and writes `r` is judged against the *previous* definition.
        for_each_write(insn, |w| {
            if w >= num_locals {
                has_temp = true;
                max_temp = max_temp.max(w);
                defined.insert((w - num_locals) as usize);
            }
        });
    }
    if !has_temp {
        return dead_buf;
    }

    // Pass 2 (reverse): nearest upcoming read / write per temp register, and
    // the nearest upcoming jump target.
    let dead = dead_buf.as_mut_slice();
    let temp_slots = (max_temp - num_locals) as usize + 1;
    let mut next_read_buf = TempSlotBuf::new(temp_slots, n);
    let mut next_write_buf = TempSlotBuf::new(temp_slots, n);
    let next_read = next_read_buf.as_mut_slice();
    let next_write = next_write_buf.as_mut_slice();
    let mut next_target = n;
    for idx in (0..n).rev() {
        // The running state describes positions `> idx`.  At `idx` that is
        // exactly the region a `LoadConst` at `idx - 1`, consumed by
        // `insns[idx]`, must stay dead in.
        if idx > 0
            && let Insn::LoadConst(r, _) = insns[idx - 1]
            && r >= num_locals
        {
            let slot = (r - num_locals) as usize;
            if !escaping.contains(slot) {
                let (read_at, write_at) = (next_read[slot], next_write[slot]);
                // Live only if a read comes strictly before the first jump target
                // (a read at the target is reached by an incoming edge, so it sees
                // a different definition) and at or before the next write (a
                // same-instruction read+write reads the old value).
                dead[idx - 1] = !(read_at < next_target && read_at <= write_at);
            }
        }
        if is_jump_target[idx] {
            next_target = idx;
        }
        for_each_read(&insns[idx], |r| {
            if r >= num_locals {
                next_read[(r - num_locals) as usize] = idx;
            }
        });
        for_each_write(&insns[idx], |w| {
            if w >= num_locals {
                next_write[(w - num_locals) as usize] = idx;
            }
        });
    }
    dead_buf
}

/// Is register `r` read by any instruction in `insns` before the first
/// instruction that writes `r`? Retained for the dead-store-elimination
/// reference equivalence test.
#[cfg(test)]
fn reg_is_read_before_next_write(insns: &[Insn], r: u32) -> bool {
    for insn in insns {
        if insn_reads_reg(insn, r) {
            return true;
        }
        if is_terminator(insn) {
            return false;
        }
        if is_control_flow(insn) {
            return true;
        }
        if writable_dst(insn) == Some(r) {
            return false;
        }
        if matches!(
            insn,
            Insn::LoadConst(dst, _)
                | Insn::LoadNone(dst)
                | Insn::LoadGlobal(dst, _)
                | Insn::LoadClassName(dst, _, _)
                | Insn::LoadCell(dst, _)
                | Insn::LoadClassDeref(dst, _)
                | Insn::Move(dst, _)
                | Insn::CopyReg(dst, _)
                if *dst == r
        ) {
            return false;
        }
    }
    false
}
