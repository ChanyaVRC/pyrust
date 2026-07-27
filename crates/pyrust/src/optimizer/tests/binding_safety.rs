use super::*;

#[test]
fn optimizer_keeps_dead_call_loaded_from_rebindable_builtin_name() {
    // A LoadGlobal name is not a canonical-builtin binding proof. The code
    // object may run with shared/explicit globals or a mutated __builtins__
    // provider where `abs` has an observable replacement.
    let mut code = compile_fn("pass\n");
    code.insns = vec![
        Insn::LoadGlobal(2, 0),
        Insn::LoadConst(3, 0),
        Insn::Call(2, 1),
        Insn::ReturnNone,
    ];
    code.names = vec!["abs".to_string()];
    code.consts = vec![Value::int(-5)];
    code.num_locals = 2;
    code.num_regs = 4;
    code.lineno_table = vec![1; code.insns.len()];
    code.col_table = vec![(0, 0, 0, 0); code.insns.len()];

    let optimized = optimize(code);
    assert!(
        optimized
            .insns
            .iter()
            .any(|insn| matches!(insn, Insn::Call(2, 1))),
        "optimizer must preserve a call whose runtime global binding is rebindable: {:?}",
        optimized.insns
    );
}
