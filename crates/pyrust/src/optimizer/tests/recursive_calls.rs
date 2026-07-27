use super::*;

#[test]
fn recursive_call_keeps_observable_frame() {
    let code = compile_fn(
        "def factorial(n, acc=1):\n    if n <= 1:\n        return acc\n    return factorial(n - 1, acc * n)\n",
    );
    let optimized = optimize(code);
    let inner = &optimized.fn_protos[0].code;
    assert!(
        inner
            .insns
            .iter()
            .any(|i| matches!(i, Insn::Call(..) | Insn::CallMemo(..))),
        "recursive call must remain explicit so traceback and f_back are correct"
    );
}
