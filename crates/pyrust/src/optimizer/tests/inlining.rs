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
fn leaf_binop_inlining_binds_each_site_to_the_proto_live_before_it() {
    // A name re-`def`ed later in the module must not drag its earlier call
    // sites onto the final proto: the guard's runtime code-object identity
    // check would then fail on every call, making the earlier region a
    // permanent deopt (one wasted dispatch per call, never an inline hit).
    let code = optimize(compile_fn(
        "def leaf(a, b):\n    return a + b\nfirst_left = 3\nfirst_right = 4\nfirst = leaf(first_left, first_right)\ndef leaf(a, b):\n    return a * b\nsecond = leaf(first_left, first_right)\n",
    ));

    let guards: Vec<(u16, BinaryOp)> = code
        .insns
        .iter()
        .filter_map(|insn| match insn {
            Insn::CallInlineBinOp { proto, op, .. } => Some((*proto, *op)),
            _ => None,
        })
        .collect();
    assert_eq!(
        guards.len(),
        2,
        "both call regions should be guarded: {:?}",
        code.insns
    );
    assert_ne!(
        guards[0].0, guards[1].0,
        "each site must wire the proto bound at that point in the stream, not the last one in the function: {guards:?}"
    );
    // The op is read out of the same proto slot, so it is a second, independent
    // witness that the earlier site kept the earlier `def`.
    assert_eq!(
        (guards[0].1, guards[1].1),
        (BinaryOp::Add, BinaryOp::Mul),
        "guards should carry their own region's leaf body: {guards:?}"
    );

    // The pass's `CallInlineBinOp` early return is a whole-stream `any`, so a
    // second run must leave *every* region alone, not just the first one it
    // meets.  The sibling idempotence test only ever has one guard in flight,
    // which cannot tell a working bail apart from one that stops after the
    // region it happens to hit first.
    let twice = optimize(code);
    let again: Vec<(u16, BinaryOp)> = twice
        .insns
        .iter()
        .filter_map(|insn| match insn {
            Insn::CallInlineBinOp { proto, op, .. } => Some((*proto, *op)),
            _ => None,
        })
        .collect();
    assert_eq!(
        again, guards,
        "re-optimizing multi-region guarded code must not stack or re-point guards: {:?}",
        twice.insns
    );
}

#[test]
fn leaf_binop_inlining_drops_a_binding_rebound_to_an_ineligible_proto() {
    // The kill side of the same rule: once the name holds a proto this pass
    // cannot inline, later sites must not keep guarding against the old one.
    let code = optimize(compile_fn(
        "def leaf(a, b):\n    return a + b\nleft = 3\nright = 4\nfirst = leaf(left, right)\ndef leaf(a, b):\n    return a / b\nsecond = leaf(left, right)\n",
    ));

    assert_eq!(
        inline_guard_count(&code),
        1,
        "only the region whose binding is an eligible leaf should be guarded: {:?}",
        code.insns
    );
}

#[test]
fn leaf_binop_inlining_retargets_a_plain_move_rebinding() {
    let code = optimize(compile_fn(
        "def driver(count):\n    def leaf(a, b):\n        return a + b\n    def other(a, b):\n        return a - b\n    leaf = other\n    total = 0\n    one = 1\n    for value in range(count):\n        total += leaf(value, one)\n    return total\n",
    ));
    let driver = code
        .fn_protos
        .iter()
        .find(|proto| proto.name.as_ref() == "driver")
        .expect("driver prototype");
    let guards: Vec<(u16, BinaryOp)> = driver
        .code
        .insns
        .iter()
        .filter_map(|insn| match insn {
            Insn::CallInlineBinOp { proto, op, .. } => Some((*proto, *op)),
            _ => None,
        })
        .collect();
    let other_proto = driver
        .code
        .fn_protos
        .iter()
        .position(|proto| proto.name.as_ref() == "other")
        .expect("other prototype") as u16;

    assert_eq!(
        guards.len(),
        1,
        "the rebound eligible leaf should retain one useful guard: {:?}",
        driver.code.insns
    );
    assert_eq!(
        guards[0],
        (other_proto, BinaryOp::Sub),
        "leaf = other must retarget the binding to other's proto instead of permanently deopting on leaf's old proto: {:?}",
        driver.code.insns
    );
}

#[test]
fn leaf_binop_inlining_kills_a_plain_move_from_an_untracked_source() {
    let code = optimize(compile_fn(
        "def driver(count):\n    def leaf(a, b):\n        return a + b\n    def other(a, b):\n        return a / b\n    leaf = other\n    total = 0\n    one = 1\n    for value in range(count):\n        total += leaf(value, one)\n    return total\n",
    ));
    let driver = code
        .fn_protos
        .iter()
        .find(|proto| proto.name.as_ref() == "driver")
        .expect("driver prototype");

    assert_eq!(
        inline_guard_count(&driver.code),
        0,
        "leaf = other must kill leaf's old binding when other is not an eligible tracked proto: {:?}",
        driver.code.insns
    );
}

#[test]
fn leaf_binop_inlining_retargets_copyreg_from_a_tracked_source() {
    let code = compile_fn(
        "def driver(left, right):\n    def leaf(a, b):\n        return a + b\n    def other(a, b):\n        return a - b\n    leaf = other\n    return leaf(left, right)\n",
    );
    let driver = code
        .fn_protos
        .iter()
        .find(|proto| proto.name.as_ref() == "driver")
        .expect("driver prototype");
    let proto_index = |name: &str| {
        driver
            .code
            .fn_protos
            .iter()
            .position(|proto| proto.name.as_ref() == name)
            .expect("nested leaf prototype") as u16
    };
    let leaf_proto = proto_index("leaf");
    let other_proto = proto_index("other");
    let bound_reg = |proto_index| {
        driver
            .code
            .insns
            .windows(2)
            .find_map(|window| match (&window[0], &window[1]) {
                (Insn::MakeFunction(tmp, proto, ..), Insn::Move(dst, src))
                    if *proto == proto_index && src == tmp =>
                {
                    Some(*dst)
                }
                _ => None,
            })
            .expect("compiler-shaped function binding")
    };
    let leaf_reg = bound_reg(leaf_proto);
    let other_reg = bound_reg(other_proto);
    let mut insns = driver.code.insns.clone();
    let rebind_at = insns
        .iter()
        .position(
            |insn| matches!(insn, Insn::Move(dst, src) if *dst == leaf_reg && *src == other_reg),
        )
        .expect("plain Move rebinding");
    insns[rebind_at] = Insn::CopyReg(leaf_reg, other_reg);

    let out = pass_inline_leaf_binop(insns, &driver.code.fn_protos);
    let guards: Vec<(u16, BinaryOp)> = out
        .iter()
        .filter_map(|insn| match insn {
            Insn::CallInlineBinOp { proto, op, .. } => Some((*proto, *op)),
            _ => None,
        })
        .collect();
    assert_eq!(
        guards,
        [(other_proto, BinaryOp::Sub)],
        "CopyReg preserves the function value and should retarget the binding just like Move: {out:?}"
    );
}

#[test]
fn leaf_binop_inlining_invalidates_non_move_register_writes() {
    let code = compile_fn(
        "def leaf(a, b):\n    return a + b\nleft = 3\nright = 4\nresult = leaf(left, right)\n",
    );
    let leaf_reg = code
        .insns
        .windows(2)
        .find_map(|window| match (&window[0], &window[1]) {
            (Insn::MakeFunction(tmp, ..), Insn::Move(dst, src)) if src == tmp => Some(*dst),
            _ => None,
        })
        .expect("compiler-shaped MakeFunction + Move binding");
    let call_at = code
        .insns
        .windows(4)
        .position(|window| {
            matches!(
                (&window[0], &window[1], &window[2], &window[3]),
                (
                    Insn::Move(_, callee),
                    Insn::Move(..) | Insn::LoadConst(..),
                    Insn::Move(..) | Insn::LoadConst(..),
                    Insn::Call(_, 2) | Insn::CallMemo(_, 2)
                ) if *callee == leaf_reg
            )
        })
        .expect("compiler-shaped two-argument leaf call");
    let clobbers = [
        ("LoadConst", Insn::LoadConst(leaf_reg, 0)),
        ("Call", Insn::Call(leaf_reg, 0)),
        (
            "LoadNoneRange",
            Insn::LoadNoneRange {
                start: leaf_reg,
                count: 1,
            },
        ),
    ];

    for (name, clobber) in clobbers {
        let mut insns = code.insns.clone();
        insns.insert(call_at, clobber);
        let out = pass_inline_leaf_binop(insns, &code.fn_protos);
        assert!(
            out.iter()
                .all(|insn| !matches!(insn, Insn::CallInlineBinOp { .. })),
            "{name} overwrites the tracked register and must kill its stale proto binding: {out:?}"
        );
    }
}

#[test]
fn leaf_binop_inlining_processes_call_window_writes_before_later_sites() {
    let code = optimize(compile_fn(
        "def driver(a, b, count):\n    def leaf(x, y):\n        return x + y\n    result = None\n    for _ in range(count):\n        result = leaf(a, b)\n    return result(a, b)\n",
    ));
    let driver = code
        .fn_protos
        .iter()
        .find(|proto| proto.name.as_ref() == "driver")
        .expect("driver prototype");

    assert_eq!(
        inline_guard_count(&driver.code),
        1,
        "the first Call overwrites its propagated call-base binding, so using that result as a later callee must not emit a stale second guard: {:?}",
        driver.code.insns
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
