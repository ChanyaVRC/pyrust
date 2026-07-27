// ─── Pure string method constant folding ──────────────────────────────────────

/// Fold `CallMethod` instructions whose receiver and all arguments are
/// compile-time constants (loaded via `LoadConst`) and whose method name is in
/// the known-pure list into a single `LoadConst` of the pre-computed result.
///
/// Only methods that return immutable value types (str, bool, int, float,
/// tuple) are eligible; methods returning mutable containers (list, bytes, …)
/// are excluded to prevent aliasing the const pool through a mutable reference.
///
/// The const-register map is cleared at every basic-block boundary (any
/// instruction index that is a jump target) to avoid propagating stale values
/// across control-flow merge points.
fn pass_str_method_const_fold(
    insns: Vec<Insn>,
    consts: &mut Vec<Value>,
    names: &[String],
    num_locals: u32,
) -> Vec<Insn> {
    fn is_foldable_str_method(method: &str) -> bool {
        matches!(
            method,
            "casefold"
                | "lower"
                | "upper"
                | "swapcase"
                | "title"
                | "capitalize"
                | "center"
                | "ljust"
                | "rjust"
                | "zfill"
                | "expandtabs"
                | "strip"
                | "lstrip"
                | "rstrip"
                | "removeprefix"
                | "removesuffix"
                | "replace"
                | "partition"
                | "rpartition"
                | "islower"
                | "isupper"
                | "istitle"
                | "isascii"
                | "isdecimal"
                | "isnumeric"
                | "isidentifier"
                | "isprintable"
                | "isdigit"
                | "isalpha"
                | "isalnum"
                | "isspace"
                | "startswith"
                | "endswith"
                | "find"
                | "rfind"
                | "count"
        )
    }

    // Build the set of basic-block entry points (branch targets).
    let mut bb_starts: HashSet<usize> = HashSet::new();
    for (i, insn) in insns.iter().enumerate() {
        let k: Option<i32> = match insn {
            Insn::Jump(k) => Some(*k),
            Insn::JumpIfFalse(_, k)
            | Insn::JumpIfTrue(_, k)
            | Insn::CmpJumpIfFalse(_, _, _, k)
            | Insn::CmpJumpIfTrue(_, _, _, k)
            | Insn::CmpJumpIfFalseConst(_, _, _, k)
            | Insn::CmpJumpIfTrueConst(_, _, _, k)
            | Insn::ForIter(_, _, k)
            | Insn::SetupExcept(k)
            | Insn::MatchExcept(_, k)
            | Insn::MatchExceptStar(_, _, _, k) => Some(*k),
            _ => None,
        };
        if let Some(k) = k {
            let target = (i as i64 + 1 + k as i64) as usize;
            if target < insns.len() {
                bb_starts.insert(target);
            }
        }
    }

    // reg → const-pool index for registers known to hold a constant.
    let mut const_regs: HashMap<u32, u16> = HashMap::new();
    let mut written_buf: HashSet<u32> = HashSet::new();
    let mut out = Vec::with_capacity(insns.len());

    for (i, insn) in insns.into_iter().enumerate() {
        if bb_starts.contains(&i) {
            const_regs.clear();
        }

        match insn {
            Insn::LoadConst(dst, idx) => {
                if dst >= num_locals {
                    const_regs.insert(dst, idx);
                }
                out.push(Insn::LoadConst(dst, idx));
            }
            Insn::CallMethod {
                dst,
                obj,
                name_idx,
                args_base,
                nargs,
            } => {
                let folded = (|| -> Option<Value> {
                    let obj_idx = *const_regs.get(&obj)?;
                    let obj_val = &consts[obj_idx as usize];
                    obj_val.as_str()?; // Only fold string receivers.
                    let method = names.get(name_idx as usize)?.as_str();
                    if !is_foldable_str_method(method) {
                        return None;
                    }
                    let mut arg_vals = Vec::with_capacity(nargs as usize);
                    for j in 0..nargs as u32 {
                        let arg_idx = *const_regs.get(&(args_base + j))?;
                        arg_vals.push(consts[arg_idx as usize].clone());
                    }
                    let result = pyrust_builtins::string::call(method, obj_val, &arg_vals).ok()?;
                    // Exclude mutable containers to avoid aliasing the pool.
                    let immutable = matches!(
                        result.kind(),
                        ValueKind::Str(_)
                            | ValueKind::Bool(_)
                            | ValueKind::Int(_)
                            | ValueKind::Float(_)
                            | ValueKind::Tuple(_)
                    );
                    if immutable { Some(result) } else { None }
                })();

                if let Some(result) = folded
                    && let Ok(new_idx) = u16::try_from(consts.len())
                {
                    consts.push(result);
                    if dst >= num_locals {
                        const_regs.insert(dst, new_idx);
                    } else {
                        const_regs.remove(&dst);
                    }
                    out.push(Insn::LoadConst(dst, new_idx));
                } else {
                    const_regs.remove(&dst);
                    out.push(Insn::CallMethod {
                        dst,
                        obj,
                        name_idx,
                        args_base,
                        nargs,
                    });
                }
            }
            insn => {
                written_buf.clear();
                collect_writes(&insn, &mut written_buf);
                for r in &written_buf {
                    const_regs.remove(r);
                }
                out.push(insn);
            }
        }
    }

    out
}
