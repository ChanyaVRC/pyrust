use super::*;

fn inline_guard_count(code: &FnCode) -> usize {
    code.insns
        .iter()
        .filter(|insn| matches!(insn, Insn::CallInlineBinOp { .. }))
        .count()
}

#[test]
fn leaf_binop_inlining_has_a_producer_and_is_idempotent() {
    let once = optimize(compile_fn(
        "def outer(count):\n    def leaf(a, b):\n        return a + b\n    increment = 1\n    total = 0\n    for value in range(count):\n        left = value & 255\n        total += leaf(left, increment)\n    return total\n",
    ));
    let outer = once
        .fn_protos
        .iter()
        .find(|proto| proto.name.as_ref() == "outer")
        .expect("outer prototype");
    assert_eq!(
        inline_guard_count(&outer.code),
        1,
        "eligible source must produce exactly one guarded inline site"
    );

    let twice = optimize(once);
    let outer = twice
        .fn_protos
        .iter()
        .find(|proto| proto.name.as_ref() == "outer")
        .expect("outer prototype");
    assert_eq!(
        inline_guard_count(&outer.code),
        1,
        "re-optimizing guarded code must not stack another guard"
    );
}

#[test]
fn leaf_binop_inlining_guards_a_one_shot_literal_call() {
    let code = optimize(compile_fn(
        "def leaf(a, b):\n    return a * b\nresult = leaf(1073741824, 1073741825)\n",
    ));

    assert_eq!(
        inline_guard_count(&code),
        1,
        "a literal call with no hot memo history should retain the inline guard: {:?}",
        code.insns
    );
}

#[test]
fn leaf_binop_inlining_rejects_explicit_environment_bindings() {
    let global = optimize(compile_fn(
        "def leaf(a, b):\n    global marker\n    return a + b\nresult = leaf(1, 2)\n",
    ));
    assert_eq!(
        inline_guard_count(&global),
        0,
        "a prototype with explicit global frame facts is not a frame-free leaf"
    );

    let nonlocal = optimize(compile_fn(
        "def outer():\n    marker = 0\n    def leaf(a, b):\n        nonlocal marker\n        return a + b\n    return leaf(1, 2)\nresult = outer()\n",
    ));
    let outer = &nonlocal.fn_protos[0].code;
    assert_eq!(
        inline_guard_count(outer),
        0,
        "a prototype with explicit nonlocal frame facts is not a frame-free leaf"
    );
}

#[test]
fn call_inline_binop_is_classified_late_stage() {
    // Completeness pin for this pass's single guarded opcode.  The sibling
    // assertion in `int_loop_versioning.rs` covers only the versioning pass's
    // opcodes, so nothing else would notice `CallInlineBinOp` dropping out of
    // `is_late_stage_guard_insn` — and an unclassified guarded opcode slips
    // past both the driver's re-entry skip and the early passes' debug assert,
    // reaching a register-rewriting pass whose wildcard kill-set arms cannot
    // model it.
    let insn = Insn::CallInlineBinOp {
        callee: 0,
        dst: 1,
        a: 2,
        op: BinaryOp::Add,
        b: 3,
        proto: 0,
        skip: 4,
    };
    assert!(
        is_late_stage_guard_insn(&insn),
        "{insn:?} is emitted by a late-stage guarded pass but is not classified as one"
    );
}

#[test]
fn optimize_is_idempotent_across_versioned_loops_inlined_calls_and_closed_forms() {
    // One module that reaches every late-stage guarded shape at once: a
    // closed-form counted loop, a versioned `for` over a canonical list, a
    // versioned `while` with a fused counted compare, and a guarded leaf call
    // both at module scope and inside a loop body.  Re-entering `optimize`
    // (exec of a cached code object, or a caller that optimizes twice) must
    // return the stream unchanged — stacking a second guard, or letting an
    // early pass rewrite registers around one, is a silent miscompile.
    let source = "\
def driver(n):
    def leaf(a, b):
        return a + b
    total = 0
    step = 1
    for value in range(n):
        total += leaf(value, step)
    return total

closed = 0
for closed_i in range(1000):
    closed += 3

listed = 0
for listed_value in [1, 2, 3, 4]:
    listed += listed_value

index = 0
counted = 0
while index < 100:
    counted += index
    index += 1

def local_leaf(a, b):
    return a * b

literal = local_leaf(1073741824, 1073741825)
result = driver(50)
";
    let once = optimize(compile_fn(source));

    // Guard against a vacuous pass: every producer must actually have fired.
    assert!(
        once.insns
            .iter()
            .any(|insn| matches!(insn, Insn::JumpIfIterNotIntRangeExact(..))),
        "closed-form guard missing: {:?}",
        once.insns
    );
    assert!(
        once.insns
            .iter()
            .any(|insn| matches!(insn, Insn::JumpIfIterNotIndexedSeq(..))),
        "indexed-sequence guard missing: {:?}",
        once.insns
    );
    assert!(
        once.insns.iter().any(|insn| matches!(
            insn,
            Insn::CountCmpJumpTrue(..) | Insn::CountCmpJumpFalse(..)
        )),
        "counted compare missing: {:?}",
        once.insns
    );
    assert_eq!(
        inline_guard_count(&once),
        1,
        "the module-scope leaf call should be guarded: {:?}",
        once.insns
    );
    let driver = once
        .fn_protos
        .iter()
        .find(|proto| proto.name.as_ref() == "driver")
        .expect("driver prototype");
    assert_eq!(
        inline_guard_count(&driver.code),
        1,
        "the leaf call inside the loop should be guarded: {:?}",
        driver.code.insns
    );

    let twice = optimize(once.clone());
    assert_eq!(
        format!("{:?}", twice.insns),
        format!("{:?}", once.insns),
        "re-optimizing already-guarded code must be a no-op"
    );
    assert_eq!(
        twice.fn_protos.len(),
        once.fn_protos.len(),
        "re-optimizing must not add or drop prototypes"
    );
    for (after, before) in twice.fn_protos.iter().zip(once.fn_protos.iter()) {
        assert_eq!(
            format!("{:?}", after.code.insns),
            format!("{:?}", before.code.insns),
            "re-optimizing a nested prototype must be a no-op"
        );
    }
    assert_eq!(
        format!("{:?}", twice.lineno_table),
        format!("{:?}", once.lineno_table),
        "re-optimizing must not disturb the line table"
    );

    // A third pass must be a no-op for the same reason.
    let thrice = optimize(twice.clone());
    assert_eq!(format!("{:?}", thrice.insns), format!("{:?}", twice.insns));
}
