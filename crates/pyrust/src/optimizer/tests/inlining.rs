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
        "def leaf(a, b):\n    return a + b\nresult = leaf(20, 22)\n",
    ));
    assert_eq!(
        inline_guard_count(&once),
        1,
        "eligible source must produce exactly one guarded inline site"
    );

    let twice = optimize(once);
    assert_eq!(
        inline_guard_count(&twice),
        1,
        "re-optimizing guarded code must not stack another guard"
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
