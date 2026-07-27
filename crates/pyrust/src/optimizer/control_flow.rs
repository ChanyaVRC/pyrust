// Control-flow analysis and rewriting helpers.

/// Extract the PC-relative jump field from an instruction, when present.
///
/// Several pass families need one shared definition when collecting branch
/// targets. Keeping it with the control-flow utilities avoids coupling those
/// passes to any particular loop transformation.
fn insn_jump_off(insn: &Insn) -> Option<i32> {
    match insn {
        Insn::Jump(offset) => Some(*offset),
        Insn::JumpIfFalse(_, offset) | Insn::JumpIfTrue(_, offset) => Some(*offset),
        Insn::CmpJumpIfFalse(_, _, _, offset)
        | Insn::CmpJumpIfTrue(_, _, _, offset)
        | Insn::CmpJumpIfFalseConst(_, _, _, offset)
        | Insn::CmpJumpIfTrueConst(_, _, _, offset)
        | Insn::ForIter(_, _, offset) => Some(*offset),
        Insn::SetupExcept(offset)
        | Insn::MatchExcept(_, offset)
        | Insn::MatchExceptStar(_, _, _, offset) => Some(*offset),
        _ => None,
    }
}

include!("control_flow/exception_regions.rs");
include!("control_flow/exception_table.rs");
include!("control_flow/compaction.rs");
include!("control_flow/string_concat.rs");
include!("control_flow/cross_jumping.rs");
