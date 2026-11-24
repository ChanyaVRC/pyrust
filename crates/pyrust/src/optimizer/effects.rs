/// Whether executing `insn` can invalidate optimizer facts about named-local
/// registers without an explicit write to those registers in the bytecode.
///
/// Named locals can be reached through live module/explicit namespace aliases.
/// Any instruction that can execute Python protocol code, import code, class
/// hooks, or namespace-storage synchronization may therefore change them
/// re-entrantly.  This is deliberately a false whitelist: a new instruction is
/// a barrier until its implementation is audited and proven not to run user
/// code or storage-boundary callbacks.
///
/// This classifier does not describe ordinary destination writes.  Individual
/// passes must still invalidate the registers an instruction writes directly.
#[inline]
fn may_invalidate_named_locals(insn: &crate::bytecode::Insn) -> bool {
    use crate::bytecode::Insn::*;

    !matches!(
        insn,
        // Plain register/environment reads and copies. LoadGlobal,
        // LoadClassName, and LoadCell read concrete runtime-owned namespaces;
        // none invokes mapping protocols.
        LoadConst(..)
            | LoadGlobal(..)
            | LoadClassName(..)
            | LoadCell(..)
            | LoadNone(..)
            | LoadNoneRange { .. }
            | Move(..)
            | CheckLocal(..)
            // These builders only clone already-evaluated values into
            // runtime-owned containers. BuildString's operands are
            // compiler-guaranteed strings.
            | BuildList(..)
            | BuildListReserve(..)
            | BuildTuple(..)
            | BuildString(..)
            | BuildSlice(..)
            // Function creation captures runtime-owned values/environment
            // handles but does not execute the function or annotations.
            | MakeFunction(..)
    )
}

/// Instructions that only the late-stage guarded passes (`pass_int_loop_version`,
/// `pass_inline_leaf_binop`) may produce.
///
/// The earlier register-rewriting passes (copy propagation, constant-register
/// propagation, PRE, dead code, dead stores) intentionally do not model these
/// opcodes: today they run strictly before the producers, so the opcodes cannot
/// appear in their input.  Their kill-set helpers use wildcard arms, which
/// would fail *silently* (miscompile) rather than loudly if a future driver
/// reorder moved a producer earlier.  Each of those passes asserts this
/// predicate over its input in debug builds so a reorder fails fast in CI
/// instead.
fn is_late_stage_guard_insn(insn: &crate::bytecode::Insn) -> bool {
    use crate::bytecode::Insn::*;
    matches!(
        insn,
        JumpIfNotInt(..)
            | JumpIfIterNotIntRange(..)
            | JumpIfIterNotIndexedSeq(..)
            | JumpIfIterNotIntRangeExact(..)
            | GetItemSeqIntOrExit(..)
            | JumpIfNotBuiltinLen(..)
            | LenSeqOrExit(..)
            | CountCmpJumpTrue(..)
            | CountCmpJumpFalse(..)
            | CallInlineBinOp(..)
    )
}

/// Debug-build guard for passes that must run before the late-stage producers.
/// See [`is_late_stage_guard_insn`].
#[inline]
fn debug_assert_no_late_stage_insns(insns: &[crate::bytecode::Insn], pass: &str) {
    if cfg!(debug_assertions)
        && let Some(found) = insns.iter().find(|insn| is_late_stage_guard_insn(insn))
    {
        panic!(
            "{pass} ran after a late-stage guarded pass: {found:?} in its input; \
             this pass does not model late-stage opcodes and would miscompile"
        );
    }
}

/// Whether `insn` ends a basic block in the pre-late-stage bytecode CFG.
#[inline]
fn insn_ends_basic_block(insn: &Insn) -> bool {
    matches!(
        insn,
        Insn::Jump(_)
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
    )
}

/// Conservative false-whitelist for instructions that can raise an exception at
/// runtime (and therefore transfer control to the innermost active exception
/// handler when executed inside a `try` region).
#[inline]
fn insn_may_raise(insn: &Insn) -> bool {
    !matches!(
        insn,
        Insn::LoadConst(..)
            | Insn::LoadNone(..)
            | Insn::LoadNoneRange { .. }
            | Insn::Move(..)
            | Insn::Jump(..)
            | Insn::SetupExcept(..)
            | Insn::PopExcept
            | Insn::EndExcept
            | Insn::PushExcContext(..)
            | Insn::PopExcContext
            | Insn::Return(..)
            | Insn::ReturnNone
            | Insn::BuildList(..)
            | Insn::BuildListReserve(..)
            | Insn::BuildTuple(..)
            | Insn::BuildString(..)
            | Insn::BuildSlice(..)
            | Insn::MakeFunction(..)
            | Insn::PushTypeParamEnv
            | Insn::PopTypeParamEnv
    )
}

/// Returns `true` for instructions that unconditionally terminate the current
/// execution path with no fallthrough: `Return`, `ReturnNone`, and `Raise*`.
/// After checking `insn_reads_reg`, a terminator guarantees that no later
/// instruction in the linear sequence can read the candidate register.
fn is_terminator(insn: &Insn) -> bool {
    use Insn::*;
    matches!(
        insn,
        Return(..) | ReturnNone | RaiseAssert(..) | RaiseValue(..) | RaiseFrom(..) | RaiseReRaise
    )
}

/// Does `insn` change control flow non-trivially? Used by dead-store analysis
/// to bail before incorrectly concluding that a later write kills an earlier
/// store along all paths.
fn is_control_flow(insn: &Insn) -> bool {
    use Insn::*;
    matches!(
        insn,
        Jump(..)
            | JumpIfNotInt(..)
            | JumpIfIterNotIntRange(..)
            | JumpIfIterNotIndexedSeq(..)
            | JumpIfIterNotIntRangeExact(..)
            | GetItemSeqIntOrExit(..)
            | JumpIfNotBuiltinLen(..)
            | LenSeqOrExit(..)
            | CountCmpJumpTrue(..)
            | CountCmpJumpFalse(..)
            | CallInlineBinOp(..)
            | JumpIfFalse(..)
            | JumpIfTrue(..)
            | CmpJumpIfFalse(..)
            | CmpJumpIfTrue(..)
            | CmpJumpIfFalseConst(..)
            | CmpJumpIfTrueConst(..)
            | ForIter(..)
            | Return(..)
            | ReturnNone
            | RaiseAssert(..)
            | RaiseValue(..)
            | RaiseFrom(..)
            | RaiseReRaise
            | SetupExcept(..)
            | PopExcept
            | EndExcept
            | MatchExcept(..)
            | Yield { .. }
    )
}
