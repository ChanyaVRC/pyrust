// ─── Constant-pool compaction ─────────────────────────────────────────────────

/// Compact the descriptor side-table to only entries still referenced by
/// `insns`, rewriting every instruction's [`DescIdx`] accordingly.
fn pass_compact_descs(mut insns: Vec<Insn>, descs: Vec<InsnDesc>) -> (Vec<Insn>, Vec<InsnDesc>) {
    if descs.is_empty() {
        return (insns, descs);
    }
    let old_len = descs.len();
    let mut used = vec![false; old_len];
    for insn in &insns {
        if let Some(idx) = crate::mir::insn_info::insn_desc_idx(insn)
            && (idx as usize) < old_len
        {
            used[idx as usize] = true;
        }
    }
    if used.iter().all(|&u| u) {
        return (insns, descs);
    }
    let mut old_to_new: Vec<Option<DescIdx>> = vec![None; old_len];
    let mut new_descs: Vec<InsnDesc> = Vec::with_capacity(used.iter().filter(|&&u| u).count());
    for (old_idx, desc) in descs.into_iter().enumerate() {
        if used[old_idx] {
            old_to_new[old_idx] = Some(new_descs.len() as DescIdx);
            new_descs.push(desc);
        }
    }
    for insn in &mut insns {
        if let Some(old_idx) = crate::mir::insn_info::insn_desc_idx(insn) {
            let new_idx =
                old_to_new[old_idx as usize].expect("referenced desc must have new index");
            crate::mir::insn_info::set_insn_desc_idx(insn, new_idx);
        }
    }
    (insns, new_descs)
}

/// Remove unreferenced entries from the constant pool and rewrite every
/// instruction-side and descriptor-side index through the resulting old-to-new map.
///
/// Assumes `pass_compact_descs` has already run so every entry in `descs` is
/// live and visited exactly once.
///
/// Reference: CPython `flowgraph.c` `remove_unused_consts()`.
fn pass_compact_consts(
    insns: Vec<Insn>,
    descs: &mut [InsnDesc],
    consts: Vec<Value>,
) -> (Vec<Insn>, Vec<Value>) {
    let old_len = consts.len();
    if old_len == 0 {
        return (insns, consts);
    }

    // Step 1: collect referenced indices.
    let mut used = vec![false; old_len];
    let mark = |used: &mut Vec<bool>, idx: u16| {
        if (idx as usize) < used.len() {
            used[idx as usize] = true;
        }
    };
    for insn in &insns {
        match insn {
            Insn::LoadConst(_, c) => mark(&mut used, *c),
            Insn::BinOpConst(_, _, _, c, _) => mark(&mut used, *c),
            Insn::CmpJumpIfFalseConst(_, _, c, _) => mark(&mut used, *c),
            Insn::CmpJumpIfTrueConst(_, _, c, _) => mark(&mut used, *c),
            Insn::MakeTypeAlias(_, name_idx, _, _) => mark(&mut used, *name_idx),
            Insn::MakeTypeVar(_, name_idx, _) => mark(&mut used, *name_idx),
            Insn::CallKw { kwnames_idx, .. } => mark(&mut used, *kwnames_idx),
            _ => {}
        }
    }
    for desc in descs.iter() {
        match desc {
            InsnDesc::CallMethodKw(d) => mark(&mut used, d.kwnames_idx),
            InsnDesc::CallExArgs(d) if d.nkw > 0 => mark(&mut used, d.kwnames_idx),
            _ => {}
        }
    }

    // Early exit if every entry is still referenced.
    if used.iter().all(|&u| u) {
        return (insns, consts);
    }

    // Step 2: build remap table.
    let mut old_to_new: Vec<Option<u16>> = vec![None; old_len];
    let mut new_consts: Vec<Value> = Vec::with_capacity(used.iter().filter(|&&u| u).count());
    for (old_idx, val) in consts.into_iter().enumerate() {
        if used[old_idx] {
            old_to_new[old_idx] = Some(
                u16::try_from(new_consts.len())
                    .expect("u16-indexed instructions reference at most 65536 constants"),
            );
            new_consts.push(val);
        }
    }

    // Step 3: rewrite constant indices in instructions and descriptors.
    let remap = |c: u16| old_to_new[c as usize].expect("referenced const must have new index");
    let new_insns = insns
        .into_iter()
        .map(|insn| match insn {
            Insn::LoadConst(r, c) => Insn::LoadConst(r, remap(c)),
            Insn::BinOpConst(d, l, op, c, ia) => Insn::BinOpConst(d, l, op, remap(c), ia),
            Insn::CmpJumpIfFalseConst(r, op, c, k) => Insn::CmpJumpIfFalseConst(r, op, remap(c), k),
            Insn::CmpJumpIfTrueConst(r, op, c, k) => Insn::CmpJumpIfTrueConst(r, op, remap(c), k),
            Insn::MakeTypeAlias(dst, name_idx, value_reg, params_reg) => {
                Insn::MakeTypeAlias(dst, remap(name_idx), value_reg, params_reg)
            }
            Insn::MakeTypeVar(dst, name_idx, kind) => Insn::MakeTypeVar(dst, remap(name_idx), kind),
            Insn::CallKw {
                func,
                total,
                nkw,
                kwnames_idx,
            } => Insn::CallKw {
                func,
                total,
                nkw,
                kwnames_idx: remap(kwnames_idx),
            },
            other => other,
        })
        .collect();
    for desc in descs.iter_mut() {
        match desc {
            InsnDesc::CallMethodKw(d) => d.kwnames_idx = remap(d.kwnames_idx),
            InsnDesc::CallExArgs(d) if d.nkw > 0 => d.kwnames_idx = remap(d.kwnames_idx),
            _ => {}
        }
    }

    (new_insns, new_consts)
}
