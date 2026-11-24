// ─── Sparse Conditional Constant Propagation (SCCP) ────────────────────────────

/// Whether `val` is an immutable, primitive compile-time constant whose
/// truthiness and pure operations never invoke user Python code.
#[inline]
fn is_sccp_pure_const(val: &Value) -> bool {
    matches!(
        val.kind(),
        ValueKind::Int(_)
            | ValueKind::BigInt(_)
            | ValueKind::Float(_)
            | ValueKind::Complex(_, _)
            | ValueKind::Str(_)
            | ValueKind::Bytes(_)
            | ValueKind::Bool(_)
            | ValueKind::None
            | ValueKind::Tuple(_)
    )
}

/// Evaluate a pure binary operation between two compile-time constant `Value`s.
///
/// Delegates integer/float/string arithmetic and core comparisons to the
/// canonical [`crate::compiler::fold_binop`] helper (which shares runtime
/// numeric semantics and returns `None` on any runtime error such as division
/// by zero or oversized shifts/powers), and extends it with pure constant
/// comparisons (`None`/`bool` identity, `bool != bool`, IEEE-754 `float`
/// comparisons, and lexicographic `str` ordering).
fn eval_sccp_binop(lv: &Value, op: crate::ast::BinaryOp, rv: &Value) -> Option<Value> {
    if let Some(folded) = crate::compiler::fold_binop(lv, op, rv) {
        return Some(folded);
    }
    use crate::ast::BinaryOp::*;
    match (lv.kind(), op, rv.kind()) {
        // Bool inequality (fold_binop already covers Bool Eq).
        (ValueKind::Bool(a), Ne, ValueKind::Bool(b)) => Some(Value::bool_(a != b)),
        // Singleton identity comparisons (`None` and `bool`).  Never fold `is`
        // across ints/floats/strings because their object identity is not
        // guaranteed by the language specification.
        (ValueKind::None, Is, ValueKind::None) => Some(Value::bool_(true)),
        (ValueKind::None, IsNot, ValueKind::None) => Some(Value::bool_(false)),
        (ValueKind::None, Is, _) if is_sccp_pure_const(rv) => Some(Value::bool_(false)),
        (ValueKind::None, IsNot, _) if is_sccp_pure_const(rv) => Some(Value::bool_(true)),
        (_, Is, ValueKind::None) if is_sccp_pure_const(lv) => Some(Value::bool_(false)),
        (_, IsNot, ValueKind::None) if is_sccp_pure_const(lv) => Some(Value::bool_(true)),
        (ValueKind::None, Eq, ValueKind::None) => Some(Value::bool_(true)),
        (ValueKind::None, Ne, ValueKind::None) => Some(Value::bool_(false)),
        (ValueKind::Bool(a), Is, ValueKind::Bool(b)) => Some(Value::bool_(a == b)),
        (ValueKind::Bool(a), IsNot, ValueKind::Bool(b)) => Some(Value::bool_(a != b)),
        (ValueKind::Bool(_), Is, _) if is_sccp_pure_const(rv) => Some(Value::bool_(false)),
        (ValueKind::Bool(_), IsNot, _) if is_sccp_pure_const(rv) => Some(Value::bool_(true)),
        (_, Is, ValueKind::Bool(_)) if is_sccp_pure_const(lv) => Some(Value::bool_(false)),
        (_, IsNot, ValueKind::Bool(_)) if is_sccp_pure_const(lv) => Some(Value::bool_(true)),
        // IEEE-754 float comparisons (NaN == NaN is false, -0.0 == 0.0 is true).
        (ValueKind::Float(a), Eq, ValueKind::Float(b)) => Some(Value::bool_(a == b)),
        (ValueKind::Float(a), Ne, ValueKind::Float(b)) => Some(Value::bool_(a != b)),
        (ValueKind::Float(a), Lt, ValueKind::Float(b)) => Some(Value::bool_(a < b)),
        (ValueKind::Float(a), Le, ValueKind::Float(b)) => Some(Value::bool_(a <= b)),
        (ValueKind::Float(a), Gt, ValueKind::Float(b)) => Some(Value::bool_(a > b)),
        (ValueKind::Float(a), Ge, ValueKind::Float(b)) => Some(Value::bool_(a >= b)),
        // Lexicographic Unicode codepoint ordering on strings.
        (ValueKind::Str(a), Lt, ValueKind::Str(b)) => Some(Value::bool_(a < b)),
        (ValueKind::Str(a), Le, ValueKind::Str(b)) => Some(Value::bool_(a <= b)),
        (ValueKind::Str(a), Gt, ValueKind::Str(b)) => Some(Value::bool_(a > b)),
        (ValueKind::Str(a), Ge, ValueKind::Str(b)) => Some(Value::bool_(a >= b)),
        _ => None,
    }
}

/// Evaluate a pure unary operation on a compile-time constant using the
/// canonical runtime unary implementation (issue #458).  Returns `None` on any
/// error (e.g. `~1.5`, `-"a"`) so the instruction remains in bytecode and
/// raises at runtime.
#[inline]
fn eval_sccp_unary(op: crate::ast::UnaryOp, v: &Value) -> Option<Value> {
    if !is_sccp_pure_const(v) {
        return None;
    }
    crate::interpreter::eval_builtin_unary(op, v.clone()).ok()
}

/// Whether `insn` ends a basic block in the pre-late-stage bytecode CFG.
#[inline]
fn sccp_ends_basic_block(insn: &Insn) -> bool {
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
            | Insn::RaiseAssertNoMsg
    )
}

/// Conservative false-whitelist for instructions that can raise an exception at
/// runtime (and therefore transfer control to the innermost active exception
/// handler when executed inside a `try` region).
#[inline]
fn sccp_insn_may_raise(insn: &Insn) -> bool {
    !matches!(
        insn,
        Insn::LoadConst(..)
            | Insn::LoadNone(..)
            | Insn::LoadNoneRange { .. }
            | Insn::Move(..)
            | Insn::CopyReg(..)
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

/// Map a constant-pool index to its canonical deduplicated slot in
/// `const_index` so two `LoadConst`s of the same constant (including distinct
/// `Int` vs `Bool` and `-0.0` vs `+0.0` bit patterns) always compare equal in
/// the lattice meet.
#[inline]
fn sccp_canonical_const_idx(const_index: &ConstIndex, consts: &[Value], idx: u16) -> Option<u16> {
    let val = consts.get(idx as usize)?;
    if !is_sccp_pure_const(val) {
        return None;
    }
    match const_key(val) {
        Some(key) => const_index.map.get(&key).copied().or(Some(idx)),
        None => Some(idx),
    }
}

/// Lazily intern `Value::none()` into `consts` for tracking `LoadNone` /
/// `LoadNoneRange` through the SCCP lattice.
#[inline]
fn sccp_none_const_idx(
    const_index: &mut ConstIndex,
    consts: &mut Vec<Value>,
    cached: &mut Option<u16>,
) -> Option<u16> {
    if let Some(idx) = *cached {
        return Some(idx);
    }
    let idx = const_index.intern(consts, Value::none())?;
    *cached = Some(idx);
    Some(idx)
}

/// Emit `LoadNone(dst)` for a folded `None` constant and `LoadConst(dst, idx)`
/// otherwise.
#[inline]
fn sccp_load_folded_insn(dst: u32, idx: u16, consts: &[Value]) -> Insn {
    if consts.get(idx as usize).is_some_and(Value::is_none) {
        Insn::LoadNone(dst)
    } else {
        Insn::LoadConst(dst, idx)
    }
}

/// Sparse Conditional Constant Propagation (Wegman-Zadeck, issue #351).
///
/// Builds a basic-block CFG over `insns` (respecting `SetupExcept`/`PopExcept`
/// handler regions via [`analyze_active_handler_stacks`]) and computes a
/// sparse worklist fixpoint over the three-level constant lattice:
/// - **Unvisited (`None`)**: block has no executable incoming edge yet (`Top`).
/// - **Constant (`Some(map)` containing `reg -> canonical_const_idx`)**: along
///   every executable path reaching the block, `reg` holds the exact same
///   primitive constant (distinguishing `Int` vs `Bool`, `-0.0` vs `+0.0`,
///   and `BigInt` via [`ConstKey`]).
/// - **Overdefined (absent from `Some(map)`)**: `Bottom` — register may hold
///   multiple values, a non-constant value, a function parameter, or an
///   uninitialized local (`UnboundLocalError`).
///
/// Soundness invariants:
/// 1. Block 0 entry starts with an empty map (all registers `Bottom`), so
///    parameters and unassigned locals are never confused with `Top`.
/// 2. Named locals (`reg < num_locals`) are dropped to `Bottom` after any
///    surviving instruction where [`may_invalidate_named_locals`] is true,
///    preserving live namespace-alias re-entrancy.
/// 3. Inside a protected `try` region, `SetupExcept` seeds its handler block as
///    executable, and every potentially-raising instruction propagates its
///    conservative pre-exception state (with named locals and written targets
///    invalidated as needed) to its active handler.
/// 4. Conditional branches whose condition is provably constant across all
///    executable paths only mark the taken edge executable during fixpoint,
///    then rewrite to unconditional `Jump` and compact unreachable blocks via
///    [`pass_dead_code`].
fn pass_sccp(insns: Vec<Insn>, consts: &mut Vec<Value>, num_locals: u32) -> Vec<Insn> {
    debug_assert_no_late_stage_insns(&insns, "pass_sccp");
    let n = insns.len();
    if n == 0 {
        return insns;
    }

    // Derive active innermost exception handler per PC when any SetupExcept is
    // present.  Bail conservatively if the handler stack is inconsistent.
    let has_except = insns.iter().any(|i| matches!(i, Insn::SetupExcept(_)));
    let handler_pc_at: Vec<usize> = if has_except {
        let Some(stack_in) = analyze_active_handler_stacks(&insns) else {
            return insns;
        };
        stack_in
            .into_iter()
            .map(|stack| stack.and_then(|s| s.last().copied()).unwrap_or(usize::MAX))
            .collect()
    } else {
        Vec::new()
    };

    // Step 1: Partition `insns` into basic blocks.
    let mut is_leader_buf = InlineBoolBuf::new(n, false);
    let is_leader = &mut is_leader_buf.as_mut_slice()[..n];
    is_leader[0] = true;
    for (pc, insn) in insns.iter().enumerate() {
        if let Some(off) = insn_jump_off(insn) {
            let target = pc as i64 + 1 + i64::from(off);
            if target >= 0 && (target as usize) < n {
                is_leader[target as usize] = true;
            }
        }
        if sccp_ends_basic_block(insn) && pc + 1 < n {
            is_leader[pc + 1] = true;
        }
    }

    let mut blocks: Vec<(usize, usize)> = Vec::new();
    let mut pc_to_block = vec![0usize; n];
    let mut start = 0usize;
    for pc in 0..n {
        if pc > 0 && is_leader[pc] {
            blocks.push((start, pc));
            start = pc;
        }
        pc_to_block[pc] = blocks.len();
    }
    blocks.push((start, n));
    let num_blocks = blocks.len();

    // Step 2: Sparse worklist fixpoint over basic blocks.
    let mut const_index = ConstIndex::build(consts);
    let mut none_idx_cache: Option<u16> = None;

    type RegConstMap = HashMap<u32, u16>;
    let mut block_in: Vec<Option<RegConstMap>> = vec![None; num_blocks];
    let mut in_worklist = vec![false; num_blocks];
    let mut worklist = std::collections::VecDeque::new();

    // Entry block starts executable with every register at Bottom (empty map),
    // covering function parameters and uninitialized locals conservatively.
    block_in[0] = Some(HashMap::new());
    in_worklist[0] = true;
    worklist.push_back(0);

    let propagate = |target_pc: usize,
                     incoming: &RegConstMap,
                     pc_to_block: &[usize],
                     block_in: &mut [Option<RegConstMap>],
                     in_worklist: &mut [bool],
                     worklist: &mut std::collections::VecDeque<usize>| {
        if target_pc >= n {
            return;
        }
        let target_block = pc_to_block[target_pc];
        match &mut block_in[target_block] {
            None => {
                block_in[target_block] = Some(incoming.clone());
                if !in_worklist[target_block] {
                    in_worklist[target_block] = true;
                    worklist.push_back(target_block);
                }
            }
            Some(existing) => {
                if !existing.is_empty() {
                    let before = existing.len();
                    existing.retain(|reg, c| incoming.get(reg) == Some(&*c));
                    if existing.len() < before && !in_worklist[target_block] {
                        in_worklist[target_block] = true;
                        worklist.push_back(target_block);
                    }
                }
            }
        }
    };

    let propagate_exception =
        |pc: usize,
         insn: &Insn,
         state: &RegConstMap,
         handler_pc_at: &[usize],
         pc_to_block: &[usize],
         block_in: &mut [Option<RegConstMap>],
         in_worklist: &mut [bool],
         worklist: &mut std::collections::VecDeque<usize>| {
            if handler_pc_at.is_empty() {
                return;
            }
            let handler_pc = handler_pc_at[pc];
            if handler_pc >= n || !sccp_insn_may_raise(insn) {
                return;
            }
            let target_block = pc_to_block[handler_pc];
            // Fast out: handler already reached with all registers at Bottom.
            if let Some(existing) = &block_in[target_block]
                && existing.is_empty()
            {
                return;
            }
            let mut exc_state = state.clone();
            if may_invalidate_named_locals(insn) {
                exc_state.retain(|&reg, _| reg >= num_locals);
            }
            for_each_write(insn, |w| {
                exc_state.remove(&w);
            });
            propagate(
                handler_pc,
                &exc_state,
                pc_to_block,
                block_in,
                in_worklist,
                worklist,
            );
        };

    let jump_target = |pc: usize, off: i32| -> usize { (pc as i64 + 1 + i64::from(off)) as usize };

    while let Some(b) = worklist.pop_front() {
        in_worklist[b] = false;
        let Some(mut state) = block_in[b].clone() else {
            continue;
        };
        let (b_start, b_end) = blocks[b];
        let mut terminated = false;

        for pc in b_start..b_end {
            let insn = &insns[pc];
            match insn {
                Insn::LoadConst(dst, c) => {
                    if let Some(canon) = sccp_canonical_const_idx(&const_index, consts, *c) {
                        state.insert(*dst, canon);
                    } else {
                        state.remove(dst);
                    }
                }
                Insn::LoadNone(dst) => {
                    if let Some(nc) =
                        sccp_none_const_idx(&mut const_index, consts, &mut none_idx_cache)
                    {
                        state.insert(*dst, nc);
                    } else {
                        state.remove(dst);
                    }
                }
                Insn::LoadNoneRange { start, count } => {
                    if let Some(nc) =
                        sccp_none_const_idx(&mut const_index, consts, &mut none_idx_cache)
                    {
                        for offset in 0..*count as u32 {
                            state.insert(start + offset, nc);
                        }
                    } else {
                        for offset in 0..*count as u32 {
                            state.remove(&(start + offset));
                        }
                    }
                }
                Insn::Move(dst, src) | Insn::CopyReg(dst, src) => match state.get(src).copied() {
                    Some(c) => {
                        state.insert(*dst, c);
                    }
                    None => {
                        state.remove(dst);
                    }
                },
                Insn::BinOp(dst, lhs, op, rhs) | Insn::BinOpInPlace(dst, lhs, op, rhs) => {
                    let folded = state.get(lhs).copied().and_then(|lc| {
                        state.get(rhs).copied().and_then(|rc| {
                            eval_sccp_binop(&consts[lc as usize], *op, &consts[rc as usize])
                                .and_then(|v| const_index.intern(consts, v))
                        })
                    });
                    if let Some(nc) = folded {
                        state.insert(*dst, nc);
                    } else {
                        propagate_exception(
                            pc,
                            insn,
                            &state,
                            &handler_pc_at,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                        state.remove(dst);
                        state.retain(|&reg, _| reg >= num_locals);
                    }
                }
                Insn::BinOpConst(dst, lhs, op, c, _) => {
                    let folded = state.get(lhs).copied().and_then(|lc| {
                        eval_sccp_binop(&consts[lc as usize], *op, &consts[*c as usize])
                            .and_then(|v| const_index.intern(consts, v))
                    });
                    if let Some(nc) = folded {
                        state.insert(*dst, nc);
                    } else {
                        propagate_exception(
                            pc,
                            insn,
                            &state,
                            &handler_pc_at,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                        state.remove(dst);
                        state.retain(|&reg, _| reg >= num_locals);
                    }
                }
                Insn::BinOpImm(dst, lhs, op, imm, _) => {
                    let folded = state.get(lhs).copied().and_then(|lc| {
                        let rhs_val = Value::int(*imm as i64);
                        eval_sccp_binop(&consts[lc as usize], *op, &rhs_val)
                            .and_then(|v| const_index.intern(consts, v))
                    });
                    if let Some(nc) = folded {
                        state.insert(*dst, nc);
                    } else {
                        propagate_exception(
                            pc,
                            insn,
                            &state,
                            &handler_pc_at,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                        state.remove(dst);
                        state.retain(|&reg, _| reg >= num_locals);
                    }
                }
                Insn::UnaryOp(dst, op, src) => {
                    let folded = state.get(src).copied().and_then(|sc| {
                        eval_sccp_unary(*op, &consts[sc as usize])
                            .and_then(|v| const_index.intern(consts, v))
                    });
                    if let Some(nc) = folded {
                        state.insert(*dst, nc);
                    } else {
                        propagate_exception(
                            pc,
                            insn,
                            &state,
                            &handler_pc_at,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                        state.remove(dst);
                        state.retain(|&reg, _| reg >= num_locals);
                    }
                }
                Insn::Jump(k) => {
                    propagate(
                        jump_target(pc, *k),
                        &state,
                        &pc_to_block,
                        &mut block_in,
                        &mut in_worklist,
                        &mut worklist,
                    );
                    terminated = true;
                }
                Insn::JumpIfFalse(cond, k) => {
                    if let Some(&c) = state.get(cond)
                        && is_sccp_pure_const(&consts[c as usize])
                    {
                        let next_pc = if consts[c as usize].truthy_raw() {
                            pc + 1
                        } else {
                            jump_target(pc, *k)
                        };
                        propagate(
                            next_pc,
                            &state,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                    } else {
                        propagate_exception(
                            pc,
                            insn,
                            &state,
                            &handler_pc_at,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                        state.retain(|&reg, _| reg >= num_locals);
                        propagate(
                            pc + 1,
                            &state,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                        propagate(
                            jump_target(pc, *k),
                            &state,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                    }
                    terminated = true;
                }
                Insn::JumpIfTrue(cond, k) => {
                    if let Some(&c) = state.get(cond)
                        && is_sccp_pure_const(&consts[c as usize])
                    {
                        let next_pc = if consts[c as usize].truthy_raw() {
                            jump_target(pc, *k)
                        } else {
                            pc + 1
                        };
                        propagate(
                            next_pc,
                            &state,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                    } else {
                        propagate_exception(
                            pc,
                            insn,
                            &state,
                            &handler_pc_at,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                        state.retain(|&reg, _| reg >= num_locals);
                        propagate(
                            pc + 1,
                            &state,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                        propagate(
                            jump_target(pc, *k),
                            &state,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                    }
                    terminated = true;
                }
                Insn::CmpJumpIfFalse(lhs, op, rhs, k) | Insn::CmpJumpIfTrue(lhs, op, rhs, k) => {
                    let is_if_true = matches!(insn, Insn::CmpJumpIfTrue(..));
                    let known_cond = state.get(lhs).copied().and_then(|lc| {
                        state.get(rhs).copied().and_then(|rc| {
                            eval_sccp_binop(&consts[lc as usize], *op, &consts[rc as usize])
                                .filter(is_sccp_pure_const)
                                .map(|v| v.truthy_raw())
                        })
                    });
                    if let Some(cond) = known_cond {
                        let takes_jump = if is_if_true { cond } else { !cond };
                        let next_pc = if takes_jump {
                            jump_target(pc, *k)
                        } else {
                            pc + 1
                        };
                        propagate(
                            next_pc,
                            &state,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                    } else {
                        propagate_exception(
                            pc,
                            insn,
                            &state,
                            &handler_pc_at,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                        state.retain(|&reg, _| reg >= num_locals);
                        propagate(
                            pc + 1,
                            &state,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                        propagate(
                            jump_target(pc, *k),
                            &state,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                    }
                    terminated = true;
                }
                Insn::CmpJumpIfFalseConst(lhs, op, rhs_c, k)
                | Insn::CmpJumpIfTrueConst(lhs, op, rhs_c, k) => {
                    let is_if_true = matches!(insn, Insn::CmpJumpIfTrueConst(..));
                    let known_cond = state.get(lhs).copied().and_then(|lc| {
                        eval_sccp_binop(&consts[lc as usize], *op, &consts[*rhs_c as usize])
                            .filter(is_sccp_pure_const)
                            .map(|v| v.truthy_raw())
                    });
                    if let Some(cond) = known_cond {
                        let takes_jump = if is_if_true { cond } else { !cond };
                        let next_pc = if takes_jump {
                            jump_target(pc, *k)
                        } else {
                            pc + 1
                        };
                        propagate(
                            next_pc,
                            &state,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                    } else {
                        propagate_exception(
                            pc,
                            insn,
                            &state,
                            &handler_pc_at,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                        state.retain(|&reg, _| reg >= num_locals);
                        propagate(
                            pc + 1,
                            &state,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                        propagate(
                            jump_target(pc, *k),
                            &state,
                            &pc_to_block,
                            &mut block_in,
                            &mut in_worklist,
                            &mut worklist,
                        );
                    }
                    terminated = true;
                }
                Insn::SetupExcept(k) => {
                    // Entering a `try` region keeps both the protected body
                    // and its handler block reachable so `SetupExcept`/`PopExcept`
                    // remain balanced for `build_exc_table`.
                    propagate(
                        pc + 1,
                        &state,
                        &pc_to_block,
                        &mut block_in,
                        &mut in_worklist,
                        &mut worklist,
                    );
                    propagate(
                        jump_target(pc, *k),
                        &state,
                        &pc_to_block,
                        &mut block_in,
                        &mut in_worklist,
                        &mut worklist,
                    );
                    terminated = true;
                }
                Insn::ForIter(dst, _, k) => {
                    propagate_exception(
                        pc,
                        insn,
                        &state,
                        &handler_pc_at,
                        &pc_to_block,
                        &mut block_in,
                        &mut in_worklist,
                        &mut worklist,
                    );
                    state.retain(|&reg, _| reg >= num_locals);
                    propagate(
                        jump_target(pc, *k),
                        &state,
                        &pc_to_block,
                        &mut block_in,
                        &mut in_worklist,
                        &mut worklist,
                    );
                    state.remove(dst);
                    propagate(
                        pc + 1,
                        &state,
                        &pc_to_block,
                        &mut block_in,
                        &mut in_worklist,
                        &mut worklist,
                    );
                    terminated = true;
                }
                Insn::MatchExcept(_, k) => {
                    propagate_exception(
                        pc,
                        insn,
                        &state,
                        &handler_pc_at,
                        &pc_to_block,
                        &mut block_in,
                        &mut in_worklist,
                        &mut worklist,
                    );
                    state.retain(|&reg, _| reg >= num_locals);
                    propagate(
                        pc + 1,
                        &state,
                        &pc_to_block,
                        &mut block_in,
                        &mut in_worklist,
                        &mut worklist,
                    );
                    propagate(
                        jump_target(pc, *k),
                        &state,
                        &pc_to_block,
                        &mut block_in,
                        &mut in_worklist,
                        &mut worklist,
                    );
                    terminated = true;
                }
                Insn::MatchExceptStar(_, src, dst, k) => {
                    propagate_exception(
                        pc,
                        insn,
                        &state,
                        &handler_pc_at,
                        &pc_to_block,
                        &mut block_in,
                        &mut in_worklist,
                        &mut worklist,
                    );
                    state.retain(|&reg, _| reg >= num_locals);
                    state.remove(src);
                    state.remove(dst);
                    propagate(
                        pc + 1,
                        &state,
                        &pc_to_block,
                        &mut block_in,
                        &mut in_worklist,
                        &mut worklist,
                    );
                    propagate(
                        jump_target(pc, *k),
                        &state,
                        &pc_to_block,
                        &mut block_in,
                        &mut in_worklist,
                        &mut worklist,
                    );
                    terminated = true;
                }
                Insn::Return(_) | Insn::ReturnNone => {
                    terminated = true;
                }
                Insn::RaiseValue(_)
                | Insn::RaiseExceptStarResidual(_)
                | Insn::RaiseFrom(..)
                | Insn::RaiseReRaise
                | Insn::RaiseAssert(_)
                | Insn::RaiseAssertNoMsg => {
                    propagate_exception(
                        pc,
                        insn,
                        &state,
                        &handler_pc_at,
                        &pc_to_block,
                        &mut block_in,
                        &mut in_worklist,
                        &mut worklist,
                    );
                    terminated = true;
                }
                _ => {
                    propagate_exception(
                        pc,
                        insn,
                        &state,
                        &handler_pc_at,
                        &pc_to_block,
                        &mut block_in,
                        &mut in_worklist,
                        &mut worklist,
                    );
                    for_each_write(insn, |w| {
                        state.remove(&w);
                    });
                    if may_invalidate_named_locals(insn) {
                        state.retain(|&reg, _| reg >= num_locals);
                    }
                }
            }
        }

        if !terminated && b_end < n {
            propagate(
                b_end,
                &state,
                &pc_to_block,
                &mut block_in,
                &mut in_worklist,
                &mut worklist,
            );
        }
    }

    // Step 3: Rewrite provably constant expressions and branches across all
    // reachable blocks using the converged entry states.
    let mut out = insns;
    let mut branch_folded = false;
    let mut has_unreachable = false;

    for (b, &(b_start, b_end)) in blocks.iter().enumerate() {
        let Some(mut state) = block_in[b].clone() else {
            has_unreachable = true;
            continue;
        };
        for pc in b_start..b_end {
            match &out[pc] {
                Insn::LoadConst(dst, c) => {
                    if let Some(canon) = sccp_canonical_const_idx(&const_index, consts, *c) {
                        state.insert(*dst, canon);
                    } else {
                        state.remove(dst);
                    }
                }
                Insn::LoadNone(dst) => {
                    if let Some(nc) =
                        sccp_none_const_idx(&mut const_index, consts, &mut none_idx_cache)
                    {
                        state.insert(*dst, nc);
                    } else {
                        state.remove(dst);
                    }
                }
                Insn::LoadNoneRange { start, count } => {
                    if let Some(nc) =
                        sccp_none_const_idx(&mut const_index, consts, &mut none_idx_cache)
                    {
                        for offset in 0..*count as u32 {
                            state.insert(start + offset, nc);
                        }
                    } else {
                        for offset in 0..*count as u32 {
                            state.remove(&(start + offset));
                        }
                    }
                }
                Insn::Move(dst, src) | Insn::CopyReg(dst, src) => {
                    let (dst, src) = (*dst, *src);
                    if let Some(c) = state.get(&src).copied() {
                        state.insert(dst, c);
                        out[pc] = sccp_load_folded_insn(dst, c, consts);
                    } else {
                        state.remove(&dst);
                    }
                }
                Insn::BinOp(dst, lhs, op, rhs) | Insn::BinOpInPlace(dst, lhs, op, rhs) => {
                    let (dst, lhs, op, rhs) = (*dst, *lhs, *op, *rhs);
                    let folded = state.get(&lhs).copied().and_then(|lc| {
                        state.get(&rhs).copied().and_then(|rc| {
                            eval_sccp_binop(&consts[lc as usize], op, &consts[rc as usize])
                                .and_then(|v| const_index.intern(consts, v))
                        })
                    });
                    if let Some(nc) = folded {
                        state.insert(dst, nc);
                        out[pc] = sccp_load_folded_insn(dst, nc, consts);
                    } else {
                        state.remove(&dst);
                        state.retain(|&reg, _| reg >= num_locals);
                    }
                }
                Insn::BinOpConst(dst, lhs, op, c, _) => {
                    let (dst, lhs, op, c) = (*dst, *lhs, *op, *c);
                    let folded = state.get(&lhs).copied().and_then(|lc| {
                        eval_sccp_binop(&consts[lc as usize], op, &consts[c as usize])
                            .and_then(|v| const_index.intern(consts, v))
                    });
                    if let Some(nc) = folded {
                        state.insert(dst, nc);
                        out[pc] = sccp_load_folded_insn(dst, nc, consts);
                    } else {
                        state.remove(&dst);
                        state.retain(|&reg, _| reg >= num_locals);
                    }
                }
                Insn::BinOpImm(dst, lhs, op, imm, _) => {
                    let (dst, lhs, op, imm) = (*dst, *lhs, *op, *imm);
                    let folded = state.get(&lhs).copied().and_then(|lc| {
                        let rhs_val = Value::int(imm as i64);
                        eval_sccp_binop(&consts[lc as usize], op, &rhs_val)
                            .and_then(|v| const_index.intern(consts, v))
                    });
                    if let Some(nc) = folded {
                        state.insert(dst, nc);
                        out[pc] = sccp_load_folded_insn(dst, nc, consts);
                    } else {
                        state.remove(&dst);
                        state.retain(|&reg, _| reg >= num_locals);
                    }
                }
                Insn::UnaryOp(dst, op, src) => {
                    let (dst, op, src) = (*dst, *op, *src);
                    let folded = state.get(&src).copied().and_then(|sc| {
                        eval_sccp_unary(op, &consts[sc as usize])
                            .and_then(|v| const_index.intern(consts, v))
                    });
                    if let Some(nc) = folded {
                        state.insert(dst, nc);
                        out[pc] = sccp_load_folded_insn(dst, nc, consts);
                    } else {
                        state.remove(&dst);
                        state.retain(|&reg, _| reg >= num_locals);
                    }
                }
                Insn::JumpIfFalse(cond, k) => {
                    let (cond, k) = (*cond, *k);
                    if let Some(&c) = state.get(&cond)
                        && is_sccp_pure_const(&consts[c as usize])
                    {
                        out[pc] = if consts[c as usize].truthy_raw() {
                            Insn::Jump(0)
                        } else {
                            Insn::Jump(k)
                        };
                        branch_folded = true;
                    } else {
                        state.retain(|&reg, _| reg >= num_locals);
                    }
                }
                Insn::JumpIfTrue(cond, k) => {
                    let (cond, k) = (*cond, *k);
                    if let Some(&c) = state.get(&cond)
                        && is_sccp_pure_const(&consts[c as usize])
                    {
                        out[pc] = if consts[c as usize].truthy_raw() {
                            Insn::Jump(k)
                        } else {
                            Insn::Jump(0)
                        };
                        branch_folded = true;
                    } else {
                        state.retain(|&reg, _| reg >= num_locals);
                    }
                }
                Insn::CmpJumpIfFalse(lhs, op, rhs, k) | Insn::CmpJumpIfTrue(lhs, op, rhs, k) => {
                    let is_if_true = matches!(out[pc], Insn::CmpJumpIfTrue(..));
                    let (lhs, op, rhs, k) = (*lhs, *op, *rhs, *k);
                    let known_cond = state.get(&lhs).copied().and_then(|lc| {
                        state.get(&rhs).copied().and_then(|rc| {
                            eval_sccp_binop(&consts[lc as usize], op, &consts[rc as usize])
                                .filter(is_sccp_pure_const)
                                .map(|v| v.truthy_raw())
                        })
                    });
                    if let Some(cond) = known_cond {
                        let takes_jump = if is_if_true { cond } else { !cond };
                        out[pc] = if takes_jump {
                            Insn::Jump(k)
                        } else {
                            Insn::Jump(0)
                        };
                        branch_folded = true;
                    } else {
                        state.retain(|&reg, _| reg >= num_locals);
                    }
                }
                Insn::CmpJumpIfFalseConst(lhs, op, rhs_c, k)
                | Insn::CmpJumpIfTrueConst(lhs, op, rhs_c, k) => {
                    let is_if_true = matches!(out[pc], Insn::CmpJumpIfTrueConst(..));
                    let (lhs, op, rhs_c, k) = (*lhs, *op, *rhs_c, *k);
                    let known_cond = state.get(&lhs).copied().and_then(|lc| {
                        eval_sccp_binop(&consts[lc as usize], op, &consts[rhs_c as usize])
                            .filter(is_sccp_pure_const)
                            .map(|v| v.truthy_raw())
                    });
                    if let Some(cond) = known_cond {
                        let takes_jump = if is_if_true { cond } else { !cond };
                        out[pc] = if takes_jump {
                            Insn::Jump(k)
                        } else {
                            Insn::Jump(0)
                        };
                        branch_folded = true;
                    } else {
                        state.retain(|&reg, _| reg >= num_locals);
                    }
                }
                insn => {
                    for_each_write(insn, |w| {
                        state.remove(&w);
                    });
                    if may_invalidate_named_locals(insn) {
                        state.retain(|&reg, _| reg >= num_locals);
                    }
                }
            }
        }
    }

    if branch_folded || has_unreachable {
        pass_dead_code(out)
    } else {
        out
    }
}
