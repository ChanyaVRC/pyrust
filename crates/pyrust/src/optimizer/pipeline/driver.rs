fn optimize_fn_code(code: FnCode) -> FnCode {
    // Recursively optimize nested function / class bodies first.
    let fn_protos: Vec<FnProto> = code
        .fn_protos
        .into_iter()
        .map(|mut proto| {
            let inner = Rc::try_unwrap(proto.code).unwrap_or_else(|rc| (*rc).clone());
            proto.code = Rc::new(optimize_fn_code(inner));
            proto
        })
        .collect();

    // Already-optimized code (re-entering `optimize`, e.g. through exec of a
    // cached code object or the idempotence tests) contains late-stage guarded
    // opcodes that the early register-rewriting passes do not model — their
    // wildcard kill-set arms would silently miscompile around them (a
    // CallInlineBinOp writes its dst; copy propagation would keep stale copy
    // facts alive across it).  The pipeline is single-shot by design: return
    // the code unchanged instead of re-running any pass over it.
    if code.insns.iter().any(is_late_stage_guard_insn) {
        return FnCode { fn_protos, ..code };
    }
    let num_locals = code.num_locals;
    let mut num_regs = code.num_regs;
    let mut consts = code.consts;
    let names = code.names;
    let original_insns = code.insns.clone();
    let original_linenos = code.lineno_table.clone();
    let original_cols = code.col_table.clone();
    // Python function bindings and frames are observable through live namespace
    // aliases and re-entrant protocols. Keep calls explicit unless a future
    // guarded optimizer carries binding identity and frame-observability facts.
    let insns = code.insns;
    let insns = pass_thread_jumps(insns);
    let insns = pass_binop_const_fusion(insns, num_locals);
    let insns = pass_fold_const_tuple(insns, num_locals, &mut consts);
    let insns = pass_const_fold(insns, &mut consts, num_locals);
    let insns = pass_str_method_const_fold(insns, &mut consts, &names, num_locals);

    let insns = pass_unary_fold(insns, num_locals, &mut consts);
    let insns = pass_const_branch_elim(insns, &consts);
    let insns = pass_cmpjump_fusion(insns, num_locals);
    let insns = pass_not_invert(insns, num_locals);
    // Run cmpjump fusion again: `pass_not_invert` can expose new
    // `BinOp + Cond-Jump` pairs (e.g. when an outer `not` was stripped from
    // `not (a == b)`, leaving `BinOp(Eq) + JumpIfTrue` ready to fuse into
    // `CmpJumpIfTrue`).
    let insns = pass_cmpjump_fusion(insns, num_locals);
    let insns = pass_const_reg_prop(insns, num_locals, &consts);
    let insns = pass_concat_merge(insns, num_locals, &mut num_regs, &consts);
    let insns = pass_exit_inline(insns);
    let insns = pass_licm(insns, num_locals);
    let insns = pass_cse(insns, num_locals);
    let insns = pass_dead_code(insns);
    let insns = pass_dead_store_elim(insns, num_locals);
    // Second run catches argument-prep moves that became dead after the first
    // pass removed their consuming dead store (store-chain cascade).
    let insns = pass_dead_store_elim(insns, num_locals);
    // Keep every module-global synchronization at its original program point.
    // An opcode does not need to be an explicit Call to re-enter Python:
    // BinOp/Cmp/GetAttr/SetItem and protocol dispatch can invoke user dunders
    // that expose or read globals while the loop is still running. Deferring a
    // SyncModuleGlobal across any such opcode makes the live namespace stale.
    // A future replacement must be driven by shared, proven-non-reentrant
    // opcode/type facts rather than a call-opcode blacklist.
    // Tail-merging must preserve each raising site's exception region, source
    // line, and PEP 657 caret span.  Compute the current source tables before
    // cross-jump; the pass separately derives active handler stacks from this
    // same instruction stream and rejects any mismatch across candidate tails.
    let (pre_cj_linenos, pre_cj_cols) =
        remap_lineno_and_col_tables(&original_insns, &original_linenos, &original_cols, &insns);
    let insns = pass_cross_jump(insns, &pre_cj_linenos, &pre_cj_cols);
    let insns = pass_copy_prop(insns, num_locals);
    let insns = pass_loop_inversion(insns);
    let insns = pass_trivial_nop(insns);
    let insns = pass_loadnone_merge(insns);
    // Runs last among the shape passes: the guarded out-of-line loop copies it
    // appends introduce `JumpIfNotInt` / `CountCmpJump*`, which downstream
    // shape passes do not model.  Only `build_exc_table`, the source-table
    // remap (the appended copies cannot raise), and `pass_compact_consts`
    // (which they carry no const slots through) run after it.
    // Guarded leaf-call inlining precedes loop versioning: its guard opcode
    // is not in the versioning whitelist, so a loop containing a call site
    // simply keeps its original form.
    let insns = pass_inline_leaf_binop(insns, &fn_protos);
    let versioned = pass_int_loop_version(insns, &consts, &mut num_regs);
    let insns = versioned.insns;
    let source_prefix_len = versioned.source_prefix_len;

    // Remap line numbers BEFORE compacting constants.  `pass_compact_consts`
    // reindexes constant-pool slots, which mutates the `idx` field of every
    // `LoadConst`/`BinOpConst`/etc.  `remap_linenos` matches new instructions to
    // the (un-reindexed) original stream by structural equality, so running it on
    // the post-compaction stream lets a reindexed constant spuriously match an
    // unrelated original instruction that happened to use the same raw index.
    // That false match advances the greedy scan cursor past the correct
    // occurrence, so a later raising instruction (e.g. a division that overflows)
    // inherits the wrong line number — attributing an exception to a later
    // statement than the one that actually raised (issue #1962).
    //
    // `pass_compact_consts` is a 1:1 instruction-count- and order-preserving
    // transformation, so the line numbers computed against the pre-compaction
    // stream apply unchanged to the post-compaction stream.
    // Zero-cost exception handling (CPython 3.11): build the per-pc handler
    // table from the balanced SetupExcept/PopExcept structure and strip those
    // two block-setup instructions from the stream.  Runs before the lineno
    // remap so the (post-strip) instruction stream is the one line numbers are
    // computed against.  On the (never-observed) bail path the stream is handed
    // back unchanged with an empty table, and the VM falls back to the dynamic
    // SetupExcept/PopExcept handler stack — always correct, never wrong.
    let (insns, exc_table, source_prefix_len) =
        build_exc_table_with_source_prefix(insns, source_prefix_len);
    let has_exc_handlers = has_exception_handlers(&insns, &exc_table);

    // Remap line numbers and PEP 657 caret anchors (#2426) in one shared scan.
    // `pass_compact_consts` below is 1:1 order-preserving, so both tables
    // computed against the pre-compaction stream apply unchanged after it.
    let (lineno_table, col_table) = remap_lineno_and_col_tables_with_source_prefix(
        &original_insns,
        &original_linenos,
        &original_cols,
        &insns,
        source_prefix_len,
    );
    let (insns, consts) = pass_compact_consts(insns, consts);

    let insns_len = insns.len();
    let names_len = names.len();
    let global_cache_interest_masks = names
        .iter()
        .map(|name| crate::bytecode::global_cache_interest_mask(name))
        .collect();
    FnCode {
        insns,
        filename: code.filename,
        lineno_table,
        col_table,
        first_lineno: code.first_lineno,
        consts,
        names,
        num_regs,
        num_iters: code.num_iters,
        num_locals,
        fn_protos,
        cell_vars: code.cell_vars,
        is_generator: code.is_generator,
        is_coroutine: code.is_coroutine,
        is_class_method: code.is_class_method,
        is_inlined_comp: code.is_inlined_comp,
        comp_enclosing_locals: code.comp_enclosing_locals,
        attr_cache: std::cell::RefCell::new(vec![AttrCacheEntry::Empty; insns_len]),
        global_cache: RefCell::new(vec![GlobalCacheEntry::Empty; names_len]),
        global_cache_interest_masks,
        binop_cache: RefCell::new(vec![BinOpCacheEntry::Empty; insns_len]),
        kwcall_cache: RefCell::new(vec![KwCallCacheEntry::Empty; insns_len]),
        fmt_spec_cache: RefCell::new(vec![
            crate::interpreter::FmtSpecCacheEntry::Empty;
            insns_len
        ]),
        call_builtin_cache: RefCell::new(vec![
            crate::interpreter::CallBuiltinCacheEntry::Empty;
            insns_len
        ]),
        exc_table,
        has_exc_handlers,
    }
}
