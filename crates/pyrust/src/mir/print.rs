//! Human-readable pretty-printer for SSA MIR (`--emit mir`).

use std::fmt::Write;

use crate::ast::{BinaryOp, UnaryOp};
use crate::value::{Value, ValueKind};

use super::ir::{BlockId, MirFn, MirOp, MirType, Terminator, Val};

fn fmt_val(v: Val) -> String {
    format!("v{}", v.0)
}

fn fmt_vals(vals: &[Val]) -> String {
    vals.iter()
        .map(|&v| fmt_val(v))
        .collect::<Vec<_>>()
        .join(", ")
}

fn fmt_edge(target: BlockId, args: &[Val]) -> String {
    if args.is_empty() {
        format!("bb{}", target.0)
    } else {
        format!("bb{}({})", target.0, fmt_vals(args))
    }
}

fn fmt_const(val: &Value) -> String {
    match val.kind() {
        ValueKind::Int(n) => format!("{n}"),
        ValueKind::BigInt(n) => format!("{n}"),
        ValueKind::Float(f) => format!("{f:?}"),
        ValueKind::Complex(r, i) => format!("({r:?}+{i:?}j)"),
        ValueKind::Bool(b) => if b { "True" } else { "False" }.to_string(),
        ValueKind::None => "None".to_string(),
        ValueKind::Str(s) => format!("{s:?}"),
        ValueKind::Bytes(b) => format!("b{:?}", String::from_utf8_lossy(b)),
        ValueKind::Tuple(items) => {
            let inner: Vec<String> = items.iter().map(fmt_const).collect();
            format!("({})", inner.join(", "))
        }
        _ => format!("{val:?}"),
    }
}

fn fmt_binop(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "add",
        BinaryOp::Sub => "sub",
        BinaryOp::Mul => "mul",
        BinaryOp::MatMul => "matmul",
        BinaryOp::Div => "div",
        BinaryOp::FloorDiv => "floordiv",
        BinaryOp::Mod => "mod",
        BinaryOp::Pow => "pow",
        BinaryOp::BitAnd => "bitand",
        BinaryOp::BitOr => "bitor",
        BinaryOp::BitXor => "bitxor",
        BinaryOp::LShift => "lshift",
        BinaryOp::RShift => "rshift",
        BinaryOp::Eq => "eq",
        BinaryOp::Ne => "ne",
        BinaryOp::Lt => "lt",
        BinaryOp::Le => "le",
        BinaryOp::Gt => "gt",
        BinaryOp::Ge => "ge",
        BinaryOp::In => "in",
        BinaryOp::NotIn => "not_in",
        BinaryOp::Is => "is",
        BinaryOp::IsNot => "is_not",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
    }
}

fn fmt_unary(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "neg",
        UnaryOp::Pos => "pos",
        UnaryOp::Not => "not",
        UnaryOp::BitNot => "bitnot",
    }
}

fn fmt_type_tag(ty: &MirType) -> &'static str {
    match ty {
        MirType::Unknown => "",
        MirType::Int => " : int",
        MirType::Float => " : float",
        MirType::Bool => " : bool",
        MirType::Str => " : str",
        MirType::None => " : None",
        MirType::Tuple => " : tuple",
        MirType::ExactConst(v) => match v.kind() {
            ValueKind::Int(_) | ValueKind::BigInt(_) => " : const int",
            ValueKind::Float(_) => " : const float",
            ValueKind::Bool(_) => " : const bool",
            ValueKind::Str(_) => " : const str",
            ValueKind::None => " : const None",
            ValueKind::Tuple(_) => " : const tuple",
            _ => " : const",
        },
    }
}

/// Render a single [`MirFn`] in clean Cranelift-style textual SSA MIR.
pub fn format_mir_fn(func: &MirFn) -> String {
    let mut out = String::new();
    let name = if func.qualname.is_empty() {
        "<module>"
    } else {
        func.qualname.as_ref()
    };
    let _ = writeln!(out, "fn {name}:");

    for block in &func.blocks {
        if block.params.is_empty() {
            let _ = write!(out, "  bb{}:", block.id.0);
        } else {
            let _ = write!(out, "  bb{}({}):", block.id.0, fmt_vals(&block.params));
        }
        if block.exc_depth > 0 {
            let _ = write!(out, "  ; exc_depth={}", block.exc_depth);
        }
        out.push('\n');

        for inst in &block.instrs {
            out.push_str("    ");
            if let Some(def) = inst.def {
                let _ = write!(out, "{} = ", fmt_val(def));
            }
            match &inst.op {
                MirOp::Const(v) => {
                    let _ = write!(out, "const {}", fmt_const(v));
                }
                MirOp::Copy(v) => {
                    let _ = write!(out, "copy {}", fmt_val(*v));
                }
                MirOp::BinOp {
                    op,
                    lhs,
                    rhs,
                    is_inplace,
                } => {
                    let suffix = if *is_inplace { "_inplace" } else { "" };
                    let _ = write!(
                        out,
                        "{}{suffix} {}, {}",
                        fmt_binop(*op),
                        fmt_val(*lhs),
                        fmt_val(*rhs)
                    );
                }
                MirOp::UnaryOp { op, arg } => {
                    let _ = write!(out, "{} {}", fmt_unary(*op), fmt_val(*arg));
                }
                MirOp::LoadLocal(r) => {
                    let _ = write!(out, "load_local r{r}");
                }
                MirOp::StoreLocal { reg, val } => {
                    let _ = write!(out, "store_local r{reg}, {}", fmt_val(*val));
                }
                MirOp::LoadGlobal { name_idx } => {
                    let n = func
                        .names
                        .get(*name_idx as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("?");
                    let _ = write!(out, "load_global {n}");
                }
                MirOp::StoreGlobal { name_idx, val } => {
                    let n = func
                        .names
                        .get(*name_idx as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("?");
                    let _ = write!(out, "store_global {n}, {}", fmt_val(*val));
                }
                MirOp::LoadCell { cell_idx } => {
                    let n = func
                        .cell_slots
                        .get(*cell_idx as usize)
                        .map(|m| m.name.as_str())
                        .unwrap_or("?");
                    let _ = write!(out, "load_cell c{cell_idx}({n})");
                }
                MirOp::StoreCell { cell_idx, val } => {
                    let n = func
                        .cell_slots
                        .get(*cell_idx as usize)
                        .map(|m| m.name.as_str())
                        .unwrap_or("?");
                    let _ = write!(out, "store_cell c{cell_idx}({n}), {}", fmt_val(*val));
                }
                MirOp::GetAttr { obj, name_idx } => {
                    let n = func
                        .names
                        .get(*name_idx as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("?");
                    let _ = write!(out, "getattr {}, .{n}", fmt_val(*obj));
                }
                MirOp::SetAttr { obj, name_idx, val } => {
                    let n = func
                        .names
                        .get(*name_idx as usize)
                        .map(|s| s.as_str())
                        .unwrap_or("?");
                    let _ = write!(out, "setattr {}, .{n}, {}", fmt_val(*obj), fmt_val(*val));
                }
                MirOp::GetItem { obj, key } => {
                    let _ = write!(out, "getitem {}, {}", fmt_val(*obj), fmt_val(*key));
                }
                MirOp::SetItem { obj, key, val } => {
                    let _ = write!(
                        out,
                        "setitem {}, {}, {}",
                        fmt_val(*obj),
                        fmt_val(*key),
                        fmt_val(*val)
                    );
                }
                MirOp::Call {
                    callee,
                    args,
                    is_memo,
                } => {
                    let tag = if *is_memo { "call_memo" } else { "call" };
                    let _ = write!(out, "{tag} {}({})", fmt_val(*callee), fmt_vals(args));
                }
                MirOp::BuildTuple(elems) => {
                    let _ = write!(out, "build_tuple ({})", fmt_vals(elems));
                }
                MirOp::BuildList(elems) => {
                    let _ = write!(out, "build_list [{}]", fmt_vals(elems));
                }
                MirOp::SideEffectInsn { insn, uses, defs } => {
                    let _ = write!(out, "vm {insn:?}");
                    if !uses.is_empty() {
                        let _ = write!(out, " uses=[{}]", fmt_vals(uses));
                    }
                    if !defs.is_empty() && !(defs.len() == 1 && inst.def == Some(defs[0])) {
                        let _ = write!(out, " defs=[{}]", fmt_vals(defs));
                    }
                }
            }
            if let Some(def) = inst.def {
                let tag = fmt_type_tag(func.val_type(def));
                if !tag.is_empty() {
                    let _ = write!(out, "  ;{tag}");
                }
            }
            out.push('\n');
        }

        out.push_str("    ");
        match &block.term {
            Terminator::Jump { target, args } => {
                let _ = writeln!(out, "jump {}", fmt_edge(*target, args));
            }
            Terminator::Branch {
                cond,
                then_bb,
                then_args,
                else_bb,
                else_args,
            } => {
                let _ = writeln!(
                    out,
                    "branch {}, {}, {}",
                    fmt_val(*cond),
                    fmt_edge(*then_bb, then_args),
                    fmt_edge(*else_bb, else_args)
                );
            }
            Terminator::Return(v) => {
                let _ = writeln!(out, "return {}", fmt_val(*v));
            }
            Terminator::Raise(Some(v)) => {
                let _ = writeln!(out, "raise {}", fmt_val(*v));
            }
            Terminator::Raise(None) => {
                let _ = writeln!(out, "reraise");
            }
            Terminator::RaiseSpecial { insn, uses } => {
                let _ = writeln!(out, "raise_special {insn:?} [{}]", fmt_vals(uses));
            }
            Terminator::ForIter {
                slot,
                dst,
                body_bb,
                body_args,
                exit_bb,
                exit_args,
            } => {
                let _ = writeln!(
                    out,
                    "{} = for_iter slot={slot}, body={}, exit={}",
                    fmt_val(*dst),
                    fmt_edge(*body_bb, body_args),
                    fmt_edge(*exit_bb, exit_args)
                );
            }
            Terminator::SetupExcept {
                try_bb,
                try_args,
                handler_bb,
                handler_args,
            } => {
                let _ = writeln!(
                    out,
                    "setup_except try={}, handler={}",
                    fmt_edge(*try_bb, try_args),
                    fmt_edge(*handler_bb, handler_args)
                );
            }
            Terminator::MatchExcept {
                type_val,
                matched_bb,
                matched_args,
                nomatch_bb,
                nomatch_args,
            } => {
                let _ = writeln!(
                    out,
                    "match_except {}, matched={}, nomatch={}",
                    fmt_val(*type_val),
                    fmt_edge(*matched_bb, matched_args),
                    fmt_edge(*nomatch_bb, nomatch_args)
                );
            }
            Terminator::MatchExceptStar {
                type_val,
                src_group,
                matched_def,
                residual_def,
                matched_bb,
                matched_args,
                nomatch_bb,
                nomatch_args,
            } => {
                let _ = writeln!(
                    out,
                    "({}, {}) = match_except_star {}, {}, matched={}, nomatch={}",
                    fmt_val(*matched_def),
                    fmt_val(*residual_def),
                    fmt_val(*type_val),
                    fmt_val(*src_group),
                    fmt_edge(*matched_bb, matched_args),
                    fmt_edge(*nomatch_bb, nomatch_args)
                );
            }
            Terminator::Unreachable => {
                let _ = writeln!(out, "unreachable");
            }
        }
    }
    out
}
