use super::*;

#[test]
fn loadnone_merge_fuses_consecutive_run() {
    let insns = vec![
        Insn::LoadNone(0),
        Insn::LoadNone(1),
        Insn::LoadNone(2),
        Insn::ReturnNone,
    ];
    let out = pass_loadnone_merge(insns);
    assert_eq!(
        out.len(),
        2,
        "three consecutive LoadNone instructions should become one range"
    );
    assert!(
        matches!(out[0], Insn::LoadNoneRange { start: 0, count: 3 }),
        "expected LoadNoneRange {{ start: 0, count: 3 }}, got {:?}",
        out[0]
    );
}

#[test]
fn loadnone_merge_single_unchanged() {
    let insns = vec![Insn::LoadNone(5), Insn::ReturnNone];
    let out = pass_loadnone_merge(insns);
    assert_eq!(out.len(), 2);
    assert!(matches!(out[0], Insn::LoadNone(5)));
}

#[test]
fn loadnone_merge_non_consecutive_not_merged() {
    let insns = vec![Insn::LoadNone(0), Insn::LoadNone(2), Insn::ReturnNone];
    let out = pass_loadnone_merge(insns);
    assert_eq!(out.len(), 3, "non-consecutive registers must not merge");
    assert!(matches!(out[0], Insn::LoadNone(0)));
    assert!(matches!(out[1], Insn::LoadNone(2)));
}

#[test]
fn loadnone_merge_interrupted_by_other_insn() {
    let insns = vec![
        Insn::LoadNone(0),
        Insn::Move(1, 0),
        Insn::LoadNone(1),
        Insn::ReturnNone,
    ];
    let out = pass_loadnone_merge(insns);
    assert_eq!(out.len(), 4);
    assert!(matches!(out[0], Insn::LoadNone(0)));
    assert!(matches!(out[1], Insn::Move(1, 0)));
    assert!(matches!(out[2], Insn::LoadNone(1)));
}

#[test]
fn loadnone_merge_rewrites_jump_offsets() {
    // The four loads collapse by three slots. The branch and its target both
    // shift by three, so the encoded relative offset remains 3.
    let insns = vec![
        Insn::LoadNone(1),
        Insn::LoadNone(2),
        Insn::LoadNone(3),
        Insn::LoadNone(4),
        Insn::JumpIfFalse(0, 3),
        Insn::LoadConst(1, 0),
        Insn::LoadConst(2, 0),
        Insn::LoadConst(3, 0),
        Insn::ReturnNone,
    ];
    let out = pass_loadnone_merge(insns);
    assert_eq!(out.len(), 6);
    assert!(matches!(out[0], Insn::LoadNoneRange { start: 1, count: 4 }));
    assert!(matches!(out[1], Insn::JumpIfFalse(0, 3)));
}
