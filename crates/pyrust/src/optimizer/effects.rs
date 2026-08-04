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
            | CopyReg(..)
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
/// The earlier register-rewriting passes (copy propagation, constant folding,
/// constant-register propagation, dead code, dead stores, CSE) intentionally do
/// not model these opcodes: today they run strictly before the producers, so
/// the opcodes cannot appear in their input.  Their kill-set helpers use
/// wildcard arms, which would fail *silently* (miscompile) rather than loudly
/// if a future driver reorder moved a producer earlier.  Each of those passes
/// asserts this predicate over its input in debug builds so a reorder fails
/// fast in CI instead.
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
            | CallInlineBinOp { .. }
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
