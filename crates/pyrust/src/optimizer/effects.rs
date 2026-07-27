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
        // Plain register/environment reads and copies. LoadGlobal/LoadCell read
        // concrete runtime-owned namespaces; neither invokes mapping protocols.
        LoadConst(..)
            | LoadGlobal(..)
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
