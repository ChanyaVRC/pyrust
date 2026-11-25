// `dis` module — body for the `dis @inject` entry in
// `pyrust_builtin_modules!` (issue #3227).
//
// CPython 3.12's `dis` module provides bytecode disassembly, `Instruction`,
// `Positions`, `Bytecode`, `code_info`, `show_code`, `pretty_flags`,
// `findlabels`, `findlinestarts`, and re-exports `opcode.__all__`.
//
// Because PyRust compiles Python to its own register bytecode (`FnCode`)
// rather than CPython's stack bytecode, this file exposes a single private
// read-only introspection helper `_dis_fncode_info` (removed from the public
// module after injection) that reads `FnCode` instructions, line tables,
// PEP 657 column spans, exception tables, and nested `FnProto` code objects
// without touching the hot VM dispatch path.
//
// Reference: <https://docs.python.org/3/library/dis.html>

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use crate::ast::{BinaryOp, UnaryOp};
use crate::bytecode::{
    EXC_NO_HANDLER, FnCode, FnProto, Insn, NO_ARGS_SPLAT, NO_ASSERT_MSG, NO_CLASS_LOCAL, NO_KWARGS,
    NO_METACLASS,
};
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::Interpreter;
use crate::interpreter::builtin_args::PyValue;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_builtins::code as code_obj;
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `dis` module.
const DIS_PY_SOURCE: &str = include_str!("dis_py.py");

/// Public names exported from `DIS_PY_SOURCE` onto the `dis` module.
const DIS_PY_EXPORTS: &[&str] = &[
    "__doc__",
    "__all__",
    // Re-exported from opcode.__all__
    "cmp_op",
    "hasarg",
    "hasconst",
    "hasname",
    "hasjrel",
    "hasjabs",
    "haslocal",
    "hascompare",
    "hasfree",
    "hasexc",
    "opname",
    "opmap",
    "HAVE_ARGUMENT",
    "EXTENDED_ARG",
    "stack_effect",
    // dis public surface
    "COMPILER_FLAG_NAMES",
    "UNKNOWN",
    "Positions",
    "Instruction",
    "Bytecode",
    "code_info",
    "show_code",
    "pretty_flags",
    "dis",
    "distb",
    "disassemble",
    "disco",
    "get_instructions",
    "findlabels",
    "findlinestarts",
    "main",
];

fn binop_str(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::MatMul => "@",
        BinaryOp::Div => "/",
        BinaryOp::FloorDiv => "//",
        BinaryOp::Mod => "%",
        BinaryOp::Pow => "**",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::LShift => "<<",
        BinaryOp::RShift => ">>",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
        BinaryOp::In => "in",
        BinaryOp::NotIn => "not in",
        BinaryOp::Is => "is",
        BinaryOp::IsNot => "is not",
    }
}

fn unaryop_str(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "not",
        UnaryOp::BitNot => "~",
        UnaryOp::Pos => "+",
    }
}

fn jump_target_pc(pc: usize, insn: &Insn) -> Option<usize> {
    let off: i64 = match insn {
        Insn::Jump(k)
        | Insn::JumpIfFalse(_, k)
        | Insn::JumpIfTrue(_, k)
        | Insn::CmpJumpIfFalse(_, _, _, k)
        | Insn::CmpJumpIfTrue(_, _, _, k)
        | Insn::CmpJumpIfFalseConst(_, _, _, k)
        | Insn::CmpJumpIfTrueConst(_, _, _, k)
        | Insn::JumpIfNotInt(_, k)
        | Insn::CountCmpJumpTrue(_, _, _, _, k)
        | Insn::CountCmpJumpFalse(_, _, _, _, k)
        | Insn::JumpIfIterNotIntRange(_, k)
        | Insn::JumpIfIterNotIndexedSeq(_, k)
        | Insn::JumpIfIterNotIntRangeExact(_, k)
        | Insn::JumpIfNotBuiltinLen(_, k)
        | Insn::LenSeqOrExit(_, _, k)
        | Insn::CallInlineBinOp(_, k)
        | Insn::ForIter(_, _, k)
        | Insn::SetupExcept(k)
        | Insn::MatchExcept(_, k)
        | Insn::MatchExceptStar(_, k) => i64::from(*k),
        Insn::GetItemSeqIntOrExit(_, _, _, k) => i64::from(*k),
        _ => return None,
    };
    usize::try_from(pc as i64 + 1 + off).ok()
}

fn format_const_repr(v: &Value) -> String {
    match v.kind() {
        ValueKind::None => "None".to_string(),
        ValueKind::Bool(b) => if b { "True" } else { "False" }.to_string(),
        ValueKind::Int(i) => i.to_string(),
        ValueKind::Float(f) => format!("{f:?}"),
        ValueKind::Str(s) => format!("{s:?}"),
        _ => v.to_string(),
    }
}

fn lookup_name(fncode: &FnCode, idx: u16) -> String {
    fncode
        .names
        .get(idx as usize)
        .map(|s| s.as_str().to_string())
        .unwrap_or_else(|| format!("<name#{idx}>"))
}

fn lookup_cell(fncode: &FnCode, idx: u16) -> String {
    fncode
        .cell_slots
        .get(idx as usize)
        .map(|s| s.name.as_str().to_string())
        .unwrap_or_else(|| format!("<cell#{idx}>"))
}

struct DecodedInsn {
    opname: &'static str,
    opcode: i64,
    arg: Option<i64>,
    argval: Value,
    argrepr: String,
}

fn decode_insn(fncode: &FnCode, pc: usize, insn: &Insn) -> DecodedInsn {
    let target = |off: i64| -> (i64, String) {
        let t = (pc as i64 + 1 + off).max(0);
        (t, format!("to {t}"))
    };
    match insn {
        Insn::LoadConst(dst, idx) => {
            let val = fncode
                .consts
                .get(*idx as usize)
                .cloned()
                .unwrap_or_else(Value::none);
            let repr = format!("{} -> r{dst}", format_const_repr(&val));
            DecodedInsn {
                opname: "LOAD_CONST",
                opcode: 1,
                arg: Some(i64::from(*idx)),
                argval: val,
                argrepr: repr,
            }
        }
        Insn::LoadGlobal(dst, idx) => {
            let name = lookup_name(fncode, *idx);
            DecodedInsn {
                opname: "LOAD_GLOBAL",
                opcode: 2,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: format!("{name} -> r{dst}"),
            }
        }
        Insn::LoadClassName(dst, local, idx) => {
            let name = lookup_name(fncode, *idx);
            let repr = if *local == NO_CLASS_LOCAL {
                format!("{name} -> r{dst}")
            } else {
                format!("{name} (r{local}) -> r{dst}")
            };
            DecodedInsn {
                opname: "LOAD_CLASS_NAME",
                opcode: 3,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: repr,
            }
        }
        Insn::StoreGlobal(idx, src) => {
            let name = lookup_name(fncode, *idx);
            DecodedInsn {
                opname: "STORE_GLOBAL",
                opcode: 4,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: format!("r{src} -> {name}"),
            }
        }
        Insn::LoadCell(dst, idx) => {
            let name = lookup_cell(fncode, *idx);
            DecodedInsn {
                opname: "LOAD_CELL",
                opcode: 5,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: format!("{name} -> r{dst}"),
            }
        }
        Insn::StoreCell(idx, src) => {
            let name = lookup_cell(fncode, *idx);
            DecodedInsn {
                opname: "STORE_CELL",
                opcode: 6,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: format!("r{src} -> {name}"),
            }
        }
        Insn::DeleteCell(idx, _) => {
            let name = lookup_cell(fncode, *idx);
            DecodedInsn {
                opname: "DELETE_CELL",
                opcode: 7,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: name,
            }
        }
        Insn::LoadClassDeref(dst, idx) => {
            let name = lookup_cell(fncode, *idx);
            DecodedInsn {
                opname: "LOAD_CLASS_DEREF",
                opcode: 8,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: format!("{name} -> r{dst}"),
            }
        }
        Insn::LoadNone(dst) => DecodedInsn {
            opname: "LOAD_NONE",
            opcode: 9,
            arg: Some(i64::from(*dst)),
            argval: Value::none(),
            argrepr: format!("None -> r{dst}"),
        },
        Insn::Move(dst, src) => DecodedInsn {
            opname: "MOVE",
            opcode: 10,
            arg: Some(i64::from(*dst)),
            argval: Value::int(i64::from(*src)),
            argrepr: format!("r{src} -> r{dst}"),
        },
        Insn::BinOp(dst, lhs, op, rhs) => {
            let s = binop_str(*op);
            DecodedInsn {
                opname: "BINARY_OP",
                opcode: 11,
                arg: Some(i64::from(*dst)),
                argval: Value::string(s),
                argrepr: format!("{s} (r{lhs}, r{rhs} -> r{dst})"),
            }
        }
        Insn::BinOpInPlace(dst, lhs, op, rhs) => {
            let s = binop_str(*op);
            DecodedInsn {
                opname: "BINARY_OP_INPLACE",
                opcode: 12,
                arg: Some(i64::from(*dst)),
                argval: Value::string(s),
                argrepr: format!("{s}= (r{lhs}, r{rhs} -> r{dst})"),
            }
        }
        Insn::BinOpConst(dst, lhs, op, cidx, is_aug) => {
            let s = binop_str(*op);
            let cv = fncode
                .consts
                .get(*cidx as usize)
                .map(format_const_repr)
                .unwrap_or_else(|| format!("const#{cidx}"));
            let aug = if *is_aug { "=" } else { "" };
            DecodedInsn {
                opname: "BINARY_OP_CONST",
                opcode: 13,
                arg: Some(i64::from(*cidx)),
                argval: Value::string(s),
                argrepr: format!("{s}{aug} (r{lhs}, {cv} -> r{dst})"),
            }
        }
        Insn::BinOpImm(dst, lhs, op, imm, is_aug) => {
            let s = binop_str(*op);
            let aug = if *is_aug { "=" } else { "" };
            DecodedInsn {
                opname: "BINARY_OP_IMM",
                opcode: 14,
                arg: Some(i64::from(*imm)),
                argval: Value::int(i64::from(*imm)),
                argrepr: format!("{s}{aug} (r{lhs}, {imm} -> r{dst})"),
            }
        }
        Insn::UnaryOp(dst, op, src) => {
            let s = unaryop_str(*op);
            DecodedInsn {
                opname: "UNARY_OP",
                opcode: 15,
                arg: Some(i64::from(*dst)),
                argval: Value::string(s),
                argrepr: format!("{s} r{src} -> r{dst}"),
            }
        }
        Insn::MatchSeqExcluded(dst, subj) => DecodedInsn {
            opname: "MATCH_SEQ_EXCLUDED",
            opcode: 16,
            arg: Some(i64::from(*dst)),
            argval: Value::int(i64::from(*subj)),
            argrepr: format!("r{subj} -> r{dst}"),
        },
        Insn::MatchMapping(dst, subj) => DecodedInsn {
            opname: "MATCH_MAPPING",
            opcode: 17,
            arg: Some(i64::from(*dst)),
            argval: Value::int(i64::from(*subj)),
            argrepr: format!("r{subj} -> r{dst}"),
        },
        Insn::GetAttr(dst, obj, idx) => {
            let name = lookup_name(fncode, *idx);
            DecodedInsn {
                opname: "LOAD_ATTR",
                opcode: 18,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: format!("r{obj}.{name} -> r{dst}"),
            }
        }
        Insn::GetAttrForWith(dst, obj, idx, _) => {
            let name = lookup_name(fncode, *idx);
            DecodedInsn {
                opname: "LOAD_ATTR_WITH",
                opcode: 19,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: format!("r{obj}.{name} -> r{dst}"),
            }
        }
        Insn::SetAttr(obj, idx, val) => {
            let name = lookup_name(fncode, *idx);
            DecodedInsn {
                opname: "STORE_ATTR",
                opcode: 20,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: format!("r{obj}.{name} = r{val}"),
            }
        }
        Insn::DeleteAttr(obj, idx) => {
            let name = lookup_name(fncode, *idx);
            DecodedInsn {
                opname: "DELETE_ATTR",
                opcode: 21,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: format!("r{obj}.{name}"),
            }
        }
        Insn::GetItem(dst, obj, idx) => DecodedInsn {
            opname: "BINARY_SUBSCR",
            opcode: 22,
            arg: Some(i64::from(*dst)),
            argval: Value::int(i64::from(*dst)),
            argrepr: format!("r{obj}[r{idx}] -> r{dst}"),
        },
        Insn::GetItemSeqIntOrExit(dst, obj, idx, off) => {
            let (t, _) = target(i64::from(*off));
            DecodedInsn {
                opname: "GET_ITEM_SEQ_INT_OR_EXIT",
                opcode: 23,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("r{obj}[r{idx}] -> r{dst} else to {t}"),
            }
        }
        Insn::GetSlice(dst, obj, base) => DecodedInsn {
            opname: "BINARY_SLICE",
            opcode: 24,
            arg: Some(i64::from(*dst)),
            argval: Value::int(i64::from(*dst)),
            argrepr: format!("r{obj}[r{base}:..] -> r{dst}"),
        },
        Insn::SetItem(obj, idx, val) => DecodedInsn {
            opname: "STORE_SUBSCR",
            opcode: 25,
            arg: Some(i64::from(*obj)),
            argval: Value::int(i64::from(*obj)),
            argrepr: format!("r{obj}[r{idx}] = r{val}"),
        },
        Insn::DeleteItem(obj, idx) => DecodedInsn {
            opname: "DELETE_SUBSCR",
            opcode: 26,
            arg: Some(i64::from(*obj)),
            argval: Value::int(i64::from(*obj)),
            argrepr: format!("del r{obj}[r{idx}]"),
        },
        Insn::DeleteName(idx) => {
            let name = lookup_name(fncode, *idx);
            DecodedInsn {
                opname: "DELETE_NAME",
                opcode: 27,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: name,
            }
        }
        Insn::PushTypeParamEnv => DecodedInsn {
            opname: "PUSH_TYPE_PARAM_ENV",
            opcode: 28,
            arg: None,
            argval: Value::none(),
            argrepr: String::new(),
        },
        Insn::PopTypeParamEnv => DecodedInsn {
            opname: "POP_TYPE_PARAM_ENV",
            opcode: 29,
            arg: None,
            argval: Value::none(),
            argrepr: String::new(),
        },
        Insn::DeleteLocal(reg, idx, _) => {
            let name = lookup_name(fncode, *idx);
            DecodedInsn {
                opname: "DELETE_FAST",
                opcode: 30,
                arg: Some(i64::from(*reg)),
                argval: Value::string(&name),
                argrepr: format!("{name} (r{reg})"),
            }
        }
        Insn::Jump(off) => {
            let (t, repr) = target(i64::from(*off));
            DecodedInsn {
                opname: "JUMP",
                opcode: 31,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: repr,
            }
        }
        Insn::JumpIfFalse(reg, off) => {
            let (t, _) = target(i64::from(*off));
            DecodedInsn {
                opname: "JUMP_IF_FALSE",
                opcode: 32,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("r{reg} -> to {t}"),
            }
        }
        Insn::JumpIfTrue(reg, off) => {
            let (t, _) = target(i64::from(*off));
            DecodedInsn {
                opname: "JUMP_IF_TRUE",
                opcode: 33,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("r{reg} -> to {t}"),
            }
        }
        Insn::CmpJumpIfFalse(lhs, op, rhs, off) => {
            let (t, _) = target(i64::from(*off));
            DecodedInsn {
                opname: "CMP_JUMP_IF_FALSE",
                opcode: 34,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("r{lhs} {} r{rhs} -> to {t}", binop_str(*op)),
            }
        }
        Insn::CmpJumpIfTrue(lhs, op, rhs, off) => {
            let (t, _) = target(i64::from(*off));
            DecodedInsn {
                opname: "CMP_JUMP_IF_TRUE",
                opcode: 35,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("r{lhs} {} r{rhs} -> to {t}", binop_str(*op)),
            }
        }
        Insn::CmpJumpIfFalseConst(lhs, op, cidx, off) => {
            let (t, _) = target(i64::from(*off));
            let cv = fncode
                .consts
                .get(*cidx as usize)
                .map(format_const_repr)
                .unwrap_or_else(|| format!("const#{cidx}"));
            DecodedInsn {
                opname: "CMP_JUMP_IF_FALSE_CONST",
                opcode: 36,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("r{lhs} {} {cv} -> to {t}", binop_str(*op)),
            }
        }
        Insn::CmpJumpIfTrueConst(lhs, op, cidx, off) => {
            let (t, _) = target(i64::from(*off));
            let cv = fncode
                .consts
                .get(*cidx as usize)
                .map(format_const_repr)
                .unwrap_or_else(|| format!("const#{cidx}"));
            DecodedInsn {
                opname: "CMP_JUMP_IF_TRUE_CONST",
                opcode: 37,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("r{lhs} {} {cv} -> to {t}", binop_str(*op)),
            }
        }
        Insn::JumpIfNotInt(reg, off) => {
            let (t, _) = target(i64::from(*off));
            DecodedInsn {
                opname: "JUMP_IF_NOT_INT",
                opcode: 38,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("r{reg} -> to {t}"),
            }
        }
        Insn::CountCmpJumpTrue(var, op, stop, imm, off) => {
            let (t, _) = target(i64::from(*off));
            DecodedInsn {
                opname: "COUNT_CMP_JUMP_TRUE",
                opcode: 39,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("r{var}+={imm} {} r{stop} -> to {t}", binop_str(*op)),
            }
        }
        Insn::CountCmpJumpFalse(var, op, stop, imm, off) => {
            let (t, _) = target(i64::from(*off));
            DecodedInsn {
                opname: "COUNT_CMP_JUMP_FALSE",
                opcode: 40,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("r{var}+={imm} {} r{stop} -> to {t}", binop_str(*op)),
            }
        }
        Insn::JumpIfIterNotIntRange(slot, off) => {
            let (t, _) = target(i64::from(*off));
            DecodedInsn {
                opname: "JUMP_IF_ITER_NOT_INT_RANGE",
                opcode: 41,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("iter#{slot} -> to {t}"),
            }
        }
        Insn::JumpIfIterNotIndexedSeq(slot, off) => {
            let (t, _) = target(i64::from(*off));
            DecodedInsn {
                opname: "JUMP_IF_ITER_NOT_INDEXED_SEQ",
                opcode: 42,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("iter#{slot} -> to {t}"),
            }
        }
        Insn::JumpIfIterNotIntRangeExact(_, off) => {
            let (t, repr) = target(i64::from(*off));
            DecodedInsn {
                opname: "JUMP_IF_ITER_NOT_INT_RANGE_EXACT",
                opcode: 43,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: repr,
            }
        }
        Insn::JumpIfNotBuiltinLen(reg, off) => {
            let (t, _) = target(i64::from(*off));
            DecodedInsn {
                opname: "JUMP_IF_NOT_BUILTIN_LEN",
                opcode: 44,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("r{reg} -> to {t}"),
            }
        }
        Insn::LenSeqOrExit(dst, seq, off) => {
            let (t, _) = target(i64::from(*off));
            DecodedInsn {
                opname: "LEN_SEQ_OR_EXIT",
                opcode: 45,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("len(r{seq}) -> r{dst} else to {t}"),
            }
        }
        Insn::CallInlineBinOp(desc_idx, off) => {
            let (t, _) = target(i64::from(*off));
            let d = fncode.descs[*desc_idx as usize].as_call_inline_binop();
            DecodedInsn {
                opname: "CALL_INLINE_BINOP",
                opcode: 46,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!(
                    "r{}(r{}, r{}) {} -> r{} skip to {t}",
                    d.callee,
                    d.a,
                    d.b,
                    binop_str(d.op),
                    d.dst
                ),
            }
        }
        Insn::Call(func, argc) => DecodedInsn {
            opname: "CALL",
            opcode: 47,
            arg: Some(i64::from(*argc)),
            argval: Value::int(i64::from(*argc)),
            argrepr: format!("r{func}({argc} args)"),
        },
        Insn::CallMemo(func, argc) => DecodedInsn {
            opname: "CALL_MEMO",
            opcode: 48,
            arg: Some(i64::from(*argc)),
            argval: Value::int(i64::from(*argc)),
            argrepr: format!("r{func}({argc} args)"),
        },
        Insn::CallMethod {
            dst,
            obj,
            name_idx,
            args_base: _,
            nargs,
        } => {
            let name = lookup_name(fncode, *name_idx);
            DecodedInsn {
                opname: "CALL_METHOD",
                opcode: 49,
                arg: Some(i64::from(*nargs)),
                argval: Value::string(&name),
                argrepr: format!("r{obj}.{name}({nargs} args) -> r{dst}"),
            }
        }
        Insn::CallMethodExpanded(desc_idx) => {
            let d = fncode.descs[*desc_idx as usize].as_call_method_expanded();
            let name = lookup_name(fncode, d.name_idx);
            DecodedInsn {
                opname: "CALL_METHOD_EX",
                opcode: 50,
                arg: Some(i64::from(d.name_idx)),
                argval: Value::string(&name),
                argrepr: format!(
                    "r{}.{name}(*r{}, **r{}) -> r{}",
                    d.obj, d.pos_list, d.kw_dict, d.dst
                ),
            }
        }
        Insn::CallKw {
            func,
            total,
            nkw,
            kwnames_idx: _,
        } => DecodedInsn {
            opname: "CALL_KW",
            opcode: 51,
            arg: Some(i64::from(*total)),
            argval: Value::int(i64::from(*total)),
            argrepr: format!("r{func}({total} args, {nkw} kw)"),
        },
        Insn::CallExArgs(desc_idx) => {
            let d = fncode.descs[*desc_idx as usize].as_call_ex_args();
            let splat = if d.args_splat != NO_ARGS_SPLAT {
                ", *args"
            } else {
                ""
            };
            let kw = if d.kwargs != NO_KWARGS { ", **kw" } else { "" };
            DecodedInsn {
                opname: "CALL_FUNCTION_EX",
                opcode: 52,
                arg: Some(i64::from(d.npos)),
                argval: Value::int(i64::from(d.npos)),
                argrepr: format!("r{}({} pos, {} kw{splat}{kw})", d.func, d.npos, d.nkw),
            }
        }
        Insn::CallMethodKw(desc_idx) => {
            let d = fncode.descs[*desc_idx as usize].as_call_method_kw();
            let name = lookup_name(fncode, d.name_idx);
            DecodedInsn {
                opname: "CALL_METHOD_KW",
                opcode: 53,
                arg: Some(i64::from(d.total)),
                argval: Value::string(&name),
                argrepr: format!(
                    "r{}.{name}({} args, {} kw) -> r{}",
                    d.obj, d.total, d.nkw, d.dst
                ),
            }
        }
        Insn::Return(src) => DecodedInsn {
            opname: "RETURN_VALUE",
            opcode: 54,
            arg: Some(i64::from(*src)),
            argval: Value::int(i64::from(*src)),
            argrepr: format!("r{src}"),
        },
        Insn::ReturnNone => DecodedInsn {
            opname: "RETURN_NONE",
            opcode: 55,
            arg: None,
            argval: Value::none(),
            argrepr: String::new(),
        },
        Insn::BuildList(dst, base, n) => DecodedInsn {
            opname: "BUILD_LIST",
            opcode: 56,
            arg: Some(i64::from(*n)),
            argval: Value::int(i64::from(*n)),
            argrepr: format!("{n} from r{base} -> r{dst}"),
        },
        Insn::BuildListReserve(dst, src) => DecodedInsn {
            opname: "BUILD_LIST_RESERVE",
            opcode: 57,
            arg: Some(i64::from(*dst)),
            argval: Value::int(i64::from(*src)),
            argrepr: format!("hint r{src} -> r{dst}"),
        },
        Insn::BuildTuple(dst, base, n) => DecodedInsn {
            opname: "BUILD_TUPLE",
            opcode: 58,
            arg: Some(i64::from(*n)),
            argval: Value::int(i64::from(*n)),
            argrepr: format!("{n} from r{base} -> r{dst}"),
        },
        Insn::BuildString(dst, base, n) => DecodedInsn {
            opname: "BUILD_STRING",
            opcode: 59,
            arg: Some(i64::from(*n)),
            argval: Value::int(i64::from(*n)),
            argrepr: format!("{n} from r{base} -> r{dst}"),
        },
        Insn::FormatValue(dst, src) => DecodedInsn {
            opname: "FORMAT_VALUE",
            opcode: 60,
            arg: Some(i64::from(*dst)),
            argval: Value::int(i64::from(*src)),
            argrepr: format!("r{src} -> r{dst}"),
        },
        Insn::FormatValueSpec(dst, src, spec) => DecodedInsn {
            opname: "FORMAT_VALUE_SPEC",
            opcode: 61,
            arg: Some(i64::from(*dst)),
            argval: Value::int(i64::from(*src)),
            argrepr: format!("r{src}:r{spec} -> r{dst}"),
        },
        Insn::BuildSlice(dst, base) => DecodedInsn {
            opname: "BUILD_SLICE",
            opcode: 62,
            arg: Some(i64::from(*dst)),
            argval: Value::int(i64::from(*base)),
            argrepr: format!("r{base}..+3 -> r{dst}"),
        },
        Insn::BuildDict(dst, base, n, _) => DecodedInsn {
            opname: "BUILD_MAP",
            opcode: 63,
            arg: Some(i64::from(*n)),
            argval: Value::int(i64::from(*n)),
            argrepr: format!("{n} pairs from r{base} -> r{dst}"),
        },
        Insn::Unpack(src, base, n) => DecodedInsn {
            opname: "UNPACK_SEQUENCE",
            opcode: 64,
            arg: Some(i64::from(*n)),
            argval: Value::int(i64::from(*n)),
            argrepr: format!("r{src} -> r{base}..+{n}"),
        },
        Insn::UnpackEx {
            src,
            before,
            after,
            dst_base,
        } => DecodedInsn {
            opname: "UNPACK_EX",
            opcode: 65,
            arg: Some(i64::from(*before) | (i64::from(*after) << 8)),
            argval: Value::int(i64::from(*before)),
            argrepr: format!("r{src} ({before}, {after}) -> r{dst_base}"),
        },
        Insn::GetIter(slot, src) => DecodedInsn {
            opname: "GET_ITER",
            opcode: 66,
            arg: Some(i64::from(*slot)),
            argval: Value::int(i64::from(*slot)),
            argrepr: format!("r{src} -> iter#{slot}"),
        },
        Insn::ForIter(dst, slot, off) => {
            let (t, _) = target(i64::from(*off));
            DecodedInsn {
                opname: "FOR_ITER",
                opcode: 67,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("iter#{slot} -> r{dst}, end to {t}"),
            }
        }
        Insn::CheckLocal(reg, idx) => {
            let name = lookup_name(fncode, *idx);
            DecodedInsn {
                opname: "CHECK_LOCAL",
                opcode: 68,
                arg: Some(i64::from(*reg)),
                argval: Value::string(&name),
                argrepr: format!("{name} (r{reg})"),
            }
        }
        Insn::RaiseAssert(msg) => {
            let repr = if *msg == NO_ASSERT_MSG {
                String::new()
            } else {
                format!("r{msg}")
            };
            DecodedInsn {
                opname: "RAISE_ASSERT",
                opcode: 69,
                arg: if *msg == NO_ASSERT_MSG {
                    None
                } else {
                    Some(i64::from(*msg))
                },
                argval: Value::none(),
                argrepr: repr,
            }
        }
        Insn::RaiseValue(exc) => DecodedInsn {
            opname: "RAISE_VALUE",
            opcode: 70,
            arg: Some(i64::from(*exc)),
            argval: Value::int(i64::from(*exc)),
            argrepr: format!("r{exc}"),
        },
        Insn::RaiseFrom(exc, cause) => DecodedInsn {
            opname: "RAISE_FROM",
            opcode: 71,
            arg: Some(i64::from(*exc)),
            argval: Value::int(i64::from(*cause)),
            argrepr: format!("r{exc} from r{cause}"),
        },
        Insn::RaiseReRaise => DecodedInsn {
            opname: "RERAISE",
            opcode: 72,
            arg: None,
            argval: Value::none(),
            argrepr: String::new(),
        },
        Insn::RaiseExceptStarResidual(exc) => DecodedInsn {
            opname: "RAISE_EXCEPT_STAR_RESIDUAL",
            opcode: 73,
            arg: Some(i64::from(*exc)),
            argval: Value::int(i64::from(*exc)),
            argrepr: format!("r{exc}"),
        },
        Insn::MatchClassPositional(desc_idx) => {
            let d = fncode.descs[*desc_idx as usize].as_match_class_positional();
            DecodedInsn {
                opname: "MATCH_CLASS",
                opcode: 74,
                arg: Some(i64::from(d.n)),
                argval: Value::int(i64::from(d.n)),
                argrepr: format!("r{} as r{} ({} pos) -> r{}", d.subj, d.cls, d.n, d.dst_base),
            }
        }
        Insn::MakeFunction(desc_idx) => {
            let d = fncode.descs[*desc_idx as usize].as_make_function();
            let pname = fncode
                .fn_protos
                .get(d.proto_idx as usize)
                .map(|p| p.qualname.as_ref().to_string())
                .unwrap_or_else(|| format!("proto#{}", d.proto_idx));
            DecodedInsn {
                opname: "MAKE_FUNCTION",
                opcode: 75,
                arg: Some(i64::from(d.proto_idx)),
                argval: Value::string(&pname),
                argrepr: format!("{pname} -> r{}", d.dst),
            }
        }
        Insn::ImportModule(dst, idx) => {
            let name = lookup_name(fncode, *idx);
            DecodedInsn {
                opname: "IMPORT_NAME",
                opcode: 76,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: format!("{name} -> r{dst}"),
            }
        }
        Insn::ImportFromAttr(dst, mod_reg, idx) => {
            let name = lookup_name(fncode, *idx);
            DecodedInsn {
                opname: "IMPORT_FROM",
                opcode: 77,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: format!("r{mod_reg}.{name} -> r{dst}"),
            }
        }
        Insn::ImportStar(mod_reg) => DecodedInsn {
            opname: "IMPORT_STAR",
            opcode: 78,
            arg: Some(i64::from(*mod_reg)),
            argval: Value::int(i64::from(*mod_reg)),
            argrepr: format!("r{mod_reg}"),
        },
        Insn::SetupExcept(off) => {
            let (t, repr) = target(i64::from(*off));
            DecodedInsn {
                opname: "SETUP_EXCEPT",
                opcode: 79,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: repr,
            }
        }
        Insn::PopExcept => DecodedInsn {
            opname: "POP_EXCEPT",
            opcode: 80,
            arg: None,
            argval: Value::none(),
            argrepr: String::new(),
        },
        Insn::LoadExc(dst) => DecodedInsn {
            opname: "LOAD_EXC",
            opcode: 81,
            arg: Some(i64::from(*dst)),
            argval: Value::int(i64::from(*dst)),
            argrepr: format!("-> r{dst}"),
        },
        Insn::LoadExcTraceback(dst, exc) => DecodedInsn {
            opname: "LOAD_EXC_TRACEBACK",
            opcode: 82,
            arg: Some(i64::from(*dst)),
            argval: Value::int(i64::from(*exc)),
            argrepr: format!("r{exc}.__traceback__ -> r{dst}"),
        },
        Insn::MatchExcept(type_reg, off) => {
            let (t, _) = target(i64::from(*off));
            DecodedInsn {
                opname: "MATCH_EXCEPT",
                opcode: 83,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("r{type_reg} else to {t}"),
            }
        }
        Insn::MatchExceptStar(desc_idx, off) => {
            let (t, _) = target(i64::from(*off));
            let d = fncode.descs[*desc_idx as usize].as_match_except_star();
            DecodedInsn {
                opname: "MATCH_EXCEPT_STAR",
                opcode: 84,
                arg: Some(t),
                argval: Value::int(t),
                argrepr: format!("r{} -> r{} else to {t}", d.type_reg, d.matched_dst),
            }
        }
        Insn::EndExcept => DecodedInsn {
            opname: "END_EXCEPT",
            opcode: 85,
            arg: None,
            argval: Value::none(),
            argrepr: String::new(),
        },
        Insn::PushExcContext(src) => DecodedInsn {
            opname: "PUSH_EXC_CONTEXT",
            opcode: 86,
            arg: Some(i64::from(*src)),
            argval: Value::int(i64::from(*src)),
            argrepr: format!("r{src}"),
        },
        Insn::PopExcContext => DecodedInsn {
            opname: "POP_EXC_CONTEXT",
            opcode: 87,
            arg: None,
            argval: Value::none(),
            argrepr: String::new(),
        },
        Insn::MakeClass(desc_idx) => {
            let d = fncode.descs[*desc_idx as usize].as_make_class();
            let name = lookup_name(fncode, d.name_idx);
            let meta = if d.meta_reg == NO_METACLASS {
                String::new()
            } else {
                format!(", meta=r{}", d.meta_reg)
            };
            DecodedInsn {
                opname: "MAKE_CLASS",
                opcode: 88,
                arg: Some(i64::from(d.proto_idx)),
                argval: Value::string(&name),
                argrepr: format!("{name}({} bases{meta}) -> r{}", d.bases_n, d.dst),
            }
        }
        Insn::MakeTypeVar(dst, cidx, _) => {
            let cv = fncode
                .consts
                .get(*cidx as usize)
                .map(format_const_repr)
                .unwrap_or_else(|| format!("const#{cidx}"));
            DecodedInsn {
                opname: "MAKE_TYPEVAR",
                opcode: 89,
                arg: Some(i64::from(*cidx)),
                argval: fncode
                    .consts
                    .get(*cidx as usize)
                    .cloned()
                    .unwrap_or_else(Value::none),
                argrepr: format!("{cv} -> r{dst}"),
            }
        }
        Insn::SetTypeVarAttr(obj, idx, val) => {
            let name = lookup_name(fncode, *idx);
            DecodedInsn {
                opname: "SET_TYPEVAR_ATTR",
                opcode: 90,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: format!("r{obj}.{name} = r{val}"),
            }
        }
        Insn::MakeTypeAlias(dst, cidx, val_reg, params_reg) => {
            let cv = fncode
                .consts
                .get(*cidx as usize)
                .map(format_const_repr)
                .unwrap_or_else(|| format!("const#{cidx}"));
            DecodedInsn {
                opname: "MAKE_TYPE_ALIAS",
                opcode: 91,
                arg: Some(i64::from(*cidx)),
                argval: fncode
                    .consts
                    .get(*cidx as usize)
                    .cloned()
                    .unwrap_or_else(Value::none),
                argrepr: format!("{cv} (r{val_reg}, r{params_reg}) -> r{dst}"),
            }
        }
        Insn::PrintExpr(src) => DecodedInsn {
            opname: "PRINT_EXPR",
            opcode: 92,
            arg: Some(i64::from(*src)),
            argval: Value::int(i64::from(*src)),
            argrepr: format!("r{src}"),
        },
        Insn::SetAdd(set_reg, val) => DecodedInsn {
            opname: "SET_ADD",
            opcode: 93,
            arg: Some(i64::from(*set_reg)),
            argval: Value::int(i64::from(*val)),
            argrepr: format!("r{set_reg}.add(r{val})"),
        },
        Insn::ListAppend(list_reg, val) => DecodedInsn {
            opname: "LIST_APPEND",
            opcode: 94,
            arg: Some(i64::from(*list_reg)),
            argval: Value::int(i64::from(*val)),
            argrepr: format!("r{list_reg}.append(r{val})"),
        },
        Insn::ListExtend(list_reg, src) => DecodedInsn {
            opname: "LIST_EXTEND",
            opcode: 95,
            arg: Some(i64::from(*list_reg)),
            argval: Value::int(i64::from(*src)),
            argrepr: format!("r{list_reg}.extend(r{src})"),
        },
        Insn::ListExtendCall(desc_idx) => {
            let d = fncode.descs[*desc_idx as usize].as_list_extend_call();
            DecodedInsn {
                opname: "LIST_EXTEND_CALL",
                opcode: 96,
                arg: Some(i64::from(d.list)),
                argval: Value::int(i64::from(d.src)),
                argrepr: format!("r{}.extend(r{})", d.list, d.src),
            }
        }
        Insn::DictUpdate(dict_reg, src) => DecodedInsn {
            opname: "DICT_UPDATE",
            opcode: 97,
            arg: Some(i64::from(*dict_reg)),
            argval: Value::int(i64::from(*src)),
            argrepr: format!("r{dict_reg}.update(r{src})"),
        },
        Insn::DictMergeKwCall(desc_idx) => {
            let d = fncode.descs[*desc_idx as usize].as_dict_merge_kw_call();
            DecodedInsn {
                opname: "DICT_MERGE",
                opcode: 98,
                arg: Some(i64::from(d.dict)),
                argval: Value::int(i64::from(d.src)),
                argrepr: format!("r{}.merge(r{})", d.dict, d.src),
            }
        }
        Insn::SetItemKwCall(desc_idx) => {
            let d = fncode.descs[*desc_idx as usize].as_set_item_kw_call();
            DecodedInsn {
                opname: "SET_ITEM_KW_CALL",
                opcode: 99,
                arg: Some(i64::from(d.dict)),
                argval: Value::int(i64::from(d.key)),
                argrepr: format!("r{}[r{}] = r{}", d.dict, d.key, d.val),
            }
        }
        Insn::Yield { src, dst } => DecodedInsn {
            opname: "YIELD_VALUE",
            opcode: 100,
            arg: Some(i64::from(*src)),
            argval: Value::int(i64::from(*dst)),
            argrepr: format!("r{src} -> r{dst}"),
        },
        Insn::YieldFrom {
            iter_reg,
            sent_reg,
            result_reg,
        } => DecodedInsn {
            opname: "YIELD_FROM",
            opcode: 101,
            arg: Some(i64::from(*iter_reg)),
            argval: Value::int(i64::from(*result_reg)),
            argrepr: format!("r{iter_reg} (sent r{sent_reg}) -> r{result_reg}"),
        },
        Insn::GetAwaitable(dst, src) => DecodedInsn {
            opname: "GET_AWAITABLE",
            opcode: 102,
            arg: Some(i64::from(*dst)),
            argval: Value::int(i64::from(*src)),
            argrepr: format!("r{src} -> r{dst}"),
        },
        Insn::RecordClassStore(reg) => DecodedInsn {
            opname: "RECORD_CLASS_STORE",
            opcode: 103,
            arg: Some(i64::from(*reg)),
            argval: Value::int(i64::from(*reg)),
            argrepr: format!("r{reg}"),
        },
        Insn::RecordClassDel(reg) => DecodedInsn {
            opname: "RECORD_CLASS_DEL",
            opcode: 104,
            arg: Some(i64::from(*reg)),
            argval: Value::int(i64::from(*reg)),
            argrepr: format!("r{reg}"),
        },
        Insn::Concat { dst, base, count } => DecodedInsn {
            opname: "CONCAT",
            opcode: 105,
            arg: Some(i64::from(*count)),
            argval: Value::int(i64::from(*count)),
            argrepr: format!("{count} from r{base} -> r{dst}"),
        },
        Insn::SyncModuleGlobal(reg, idx) => {
            let name = lookup_name(fncode, *idx);
            DecodedInsn {
                opname: "SYNC_MODULE_GLOBAL",
                opcode: 106,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: format!("r{reg} -> {name}"),
            }
        }
        Insn::DeleteModuleGlobal(idx) => {
            let name = lookup_name(fncode, *idx);
            DecodedInsn {
                opname: "DELETE_MODULE_GLOBAL",
                opcode: 107,
                arg: Some(i64::from(*idx)),
                argval: Value::string(&name),
                argrepr: name,
            }
        }
    }
}

fn build_code_from_proto(proto: &FnProto) -> Value {
    let fncode = &proto.code;
    let spec = &proto.param_spec;
    let mut argcount = 0i64;
    let mut posonlyargcount = 0i64;
    let mut kwonlyargcount = 0i64;
    let mut flags = code_obj::CO_OPTIMIZED | code_obj::CO_NEWLOCALS;
    let mut varnames: Vec<Value> = Vec::new();
    let mut param_set: HashSet<&str> = HashSet::new();

    for i in 0..spec.names.len() {
        let is_args = spec.is_args.get(i).copied().unwrap_or(false);
        let is_kwargs = spec.is_kwargs.get(i).copied().unwrap_or(false);
        let is_kwonly = spec.is_keyword_only.get(i).copied().unwrap_or(false);
        let is_posonly = spec.is_positional_only.get(i).copied().unwrap_or(false);
        if is_args {
            flags |= code_obj::CO_VARARGS;
        }
        if is_kwargs {
            flags |= code_obj::CO_VARKEYWORDS;
        }
        if is_kwonly {
            kwonlyargcount += 1;
        } else if !is_args && !is_kwargs {
            argcount += 1;
            if is_posonly {
                posonlyargcount += 1;
            }
        }
    }
    for i in 0..spec.names.len() {
        let is_args = spec.is_args.get(i).copied().unwrap_or(false);
        let is_kwargs = spec.is_kwargs.get(i).copied().unwrap_or(false);
        let is_kwonly = spec.is_keyword_only.get(i).copied().unwrap_or(false);
        if !is_args && !is_kwargs && !is_kwonly {
            param_set.insert(spec.names[i].as_str());
            varnames.push(Value::string(spec.names[i].as_str()));
        }
    }
    for i in 0..spec.names.len() {
        if spec.is_keyword_only.get(i).copied().unwrap_or(false) {
            param_set.insert(spec.names[i].as_str());
            varnames.push(Value::string(spec.names[i].as_str()));
        }
    }
    for i in 0..spec.names.len() {
        if spec.is_args.get(i).copied().unwrap_or(false) {
            param_set.insert(spec.names[i].as_str());
            varnames.push(Value::string(spec.names[i].as_str()));
        }
    }
    for i in 0..spec.names.len() {
        if spec.is_kwargs.get(i).copied().unwrap_or(false) {
            param_set.insert(spec.names[i].as_str());
            varnames.push(Value::string(spec.names[i].as_str()));
        }
    }

    let cellvar_set: HashSet<&str> = fncode
        .own_cell_slots()
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    let mut body_locals: Vec<(u32, &str)> = proto
        .local_index
        .iter()
        .filter(|(n, _)| !param_set.contains(n.as_str()) && !cellvar_set.contains(n.as_str()))
        .map(|(n, &slot)| (slot, n.as_str()))
        .collect();
    body_locals.sort_by_key(|(slot, _)| *slot);
    for (_, name) in body_locals {
        varnames.push(Value::string(name));
    }

    match (fncode.is_coroutine, fncode.is_generator) {
        (true, true) => flags |= code_obj::CO_ASYNC_GENERATOR,
        (true, false) => flags |= code_obj::CO_COROUTINE,
        (false, true) => flags |= code_obj::CO_GENERATOR,
        (false, false) => {}
    }

    let mut consts = vec![Value::none()];
    consts.extend(fncode.consts.iter().filter(|c| !c.is_none()).cloned());
    let names = fncode
        .names
        .iter()
        .map(|s| Value::string(s.as_str()))
        .collect();
    let cellvars = fncode
        .own_cell_slots()
        .iter()
        .map(|s| Value::string(s.name.as_str()))
        .collect();
    let freevars = fncode
        .free_var_slots()
        .iter()
        .map(|s| Value::string(s.name.as_str()))
        .collect();

    let firstlineno = fncode.first_lineno as i64;
    let (lines, positions) =
        crate::interpreter::build_code_lines_and_positions(fncode, firstlineno);
    let linetable = code_obj::encode_linetable(&lines, firstlineno);
    code_obj::CodeBuild {
        name: proto.name.to_string(),
        qualname: proto.qualname.to_string(),
        argcount,
        posonlyargcount,
        kwonlyargcount,
        nlocals: varnames.len() as i64,
        stacksize: fncode.num_regs as i64,
        varnames,
        flags,
        filename: fncode.filename.to_string(),
        firstlineno,
        consts,
        names,
        freevars,
        cellvars,
        code_bytes: Vec::new(),
        linetable,
        exceptiontable: Vec::new(),
        lines,
        positions,
        raw_code: Some(Rc::clone(fncode) as Rc<dyn Any>),
        exec_meta: None,
    }
    .build()
}

fn extract_fncode(val: &Value) -> Option<Rc<FnCode>> {
    if let ValueKind::UserFunction(func) = val.kind() {
        return func
            .precompiled_code
            .as_ref()
            .and_then(|rc| Rc::clone(rc).downcast::<FnCode>().ok());
    }
    if let Some(fc) =
        code_obj::with_raw_code(val, |raw| Rc::clone(raw).downcast::<FnCode>().ok()).flatten()
    {
        return Some(fc);
    }
    crate::interpreter::with_code_state(val, |cs| Rc::clone(&cs.code))
}

pyrust_module! {
    /// Extract PyRust's real compiled `FnCode` instructions, line starts,
    /// nested `FnProto` code objects, and exception-table entries for `dis`.
    #[arity_style(takes_exactly_one)]
    fn _dis_fncode_info(#[positional_only] obj: PyValue) -> Result<Value> {
        let Some(fncode) = extract_fncode(&obj.0) else {
            return Ok(Value::none());
        };

        // Collect all jump targets (branch instructions + exception handlers).
        let mut jump_targets: HashSet<usize> = HashSet::new();
        for (pc, insn) in fncode.insns.iter().enumerate() {
            if let Some(target) = jump_target_pc(pc, insn) {
                jump_targets.insert(target);
            }
        }
        let mut exc_entries: Vec<Value> = Vec::new();
        let mut i = 0usize;
        while i < fncode.exc_table.len() {
            let target = fncode.exc_table[i];
            if target == EXC_NO_HANDLER {
                i += 1;
                continue;
            }
            let start = i;
            while i < fncode.exc_table.len() && fncode.exc_table[i] == target {
                i += 1;
            }
            let end = i;
            jump_targets.insert(target as usize);
            exc_entries.push(Value::tuple(vec![
                Value::int(start as i64),
                Value::int(end as i64),
                Value::int(i64::from(target)),
            ]));
        }

        // Decode instructions and line starts.
        let mut insn_rows: Vec<Value> = Vec::with_capacity(fncode.insns.len());
        let mut linestarts: Vec<Value> = Vec::new();
        let mut prev_line: u32 = 0;

        for (pc, insn) in fncode.insns.iter().enumerate() {
            let line = fncode.line_at(pc);
            let starts_line = if line != 0 && line != prev_line {
                prev_line = line;
                linestarts.push(Value::tuple(vec![
                    Value::int(pc as i64),
                    Value::int(i64::from(line)),
                ]));
                Value::int(i64::from(line))
            } else {
                Value::none()
            };
            let line_val = if line != 0 {
                Value::int(i64::from(line))
            } else {
                Value::none()
            };
            let positions = match fncode.col_at(pc) {
                Some((full_start, _, _, full_end)) => Value::tuple(vec![
                    line_val.clone(),
                    line_val,
                    Value::int(i64::from(full_start)),
                    Value::int(i64::from(full_end)),
                ]),
                None => Value::tuple(vec![
                    line_val.clone(),
                    line_val,
                    Value::none(),
                    Value::none(),
                ]),
            };
            let dec = decode_insn(&fncode, pc, insn);
            let arg_val = match dec.arg {
                Some(a) => Value::int(a),
                None => Value::none(),
            };
            insn_rows.push(Value::tuple(vec![
                Value::string(dec.opname),
                Value::int(dec.opcode),
                arg_val,
                dec.argval,
                Value::string(&dec.argrepr),
                Value::int(pc as i64),
                starts_line,
                Value::bool_(jump_targets.contains(&pc)),
                positions,
            ]));
        }

        let nested_codes: Vec<Value> = fncode
            .fn_protos
            .iter()
            .map(build_code_from_proto)
            .collect();

        Ok(Value::tuple(vec![
            Value::list(insn_rows),
            Value::list(linestarts),
            Value::list(nested_codes),
            Value::list(exc_entries),
        ]))
    }
}

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    if let Some(helper) = module.borrow().attrs.get("_dis_fncode_info").cloned() {
        ns.dict_insert(PyKey::str_from("_dis_fncode_info"), helper)?;
    }
    interp.exec_source(DIS_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("dis: exec namespace not a dict".into()))?;
    for name in DIS_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    module.borrow_mut().attrs.shift_remove("_dis_fncode_info");
    Ok(Some(ns.clone()))
}
