use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::ast::{BinaryOp, UnaryOp};
use crate::value::Value;

/// Identifies a named variable that is captured by a nested function via `nonlocal`.
/// These live in the env (not registers) so nested closures can share them.
pub type CellVar = String;

pub type Reg = u8;

/// Prototype for a nested function or class body.  Created at compile time,
/// instantiated into a `UserFunction` / class value at runtime via `MakeFunction`
/// / `MakeClass`.
#[derive(Debug)]
pub struct FnProto {
    pub name: String,
    /// Parameter names in declaration order (no defaults — defaults are in registers).
    pub param_names: Vec<String>,
    /// Which params carry defaults (filled right-to-left, like Python).
    pub param_has_default: Vec<bool>,
    pub param_is_args: Vec<bool>,
    pub param_is_kwargs: Vec<bool>,
    pub code: Rc<FnCode>,
    pub local_index: Rc<HashMap<String, usize>>,
    pub global_names: Rc<HashSet<String>>,
    pub nonlocal_names: Rc<HashSet<String>>,
    pub is_pure: bool,
    pub def_bound_mask: u64,
    /// True when this proto describes a class body (MakeClass uses this).
    pub is_class_body: bool,
}

#[derive(Debug, Clone)]
pub enum Insn {
    /// R[dst] = consts[idx]
    LoadConst(Reg, u16),
    /// R[dst] = lookup name through env chain
    LoadGlobal(Reg, u16),
    /// names[name_idx] = R[src]  (write to module / enclosing env)
    StoreGlobal(u16, Reg),
    /// R[dst] = None
    LoadNone(Reg),
    /// R[dst] = R[src]
    Move(Reg, Reg),
    /// R[dst] = R[lhs] op R[rhs]
    BinOp(Reg, Reg, BinaryOp, Reg),
    /// R[dst] = R[lhs] op= R[rhs]  (tries __i<op>__ before __<op>__)
    BinOpInPlace(Reg, Reg, BinaryOp, Reg),
    /// R[dst] = R[lhs] op consts[const_idx]  (fuses LoadConst + BinOp)
    BinOpConst(Reg, Reg, BinaryOp, u16),
    /// R[dst] = unary_op(R[src])
    UnaryOp(Reg, UnaryOp, Reg),
    /// R[dst] = R[obj].names[name_idx]
    GetAttr(Reg, Reg, u16),
    /// R[obj].names[name_idx] = R[val]
    SetAttr(Reg, u16, Reg),
    /// del R[obj].names[name_idx]
    DeleteAttr(Reg, u16),
    /// R[dst] = R[obj][R[idx]]
    GetItem(Reg, Reg, Reg),
    /// R[obj][R[idx]] = R[val]
    SetItem(Reg, Reg, Reg),
    /// del R[obj][R[idx]]
    DeleteItem(Reg, Reg),
    /// del names[name_idx] from current env
    DeleteName(u16),
    /// pc += offset  (offset 0 = next instruction)
    Jump(i32),
    /// if !R[cond].truthy(): pc += offset
    JumpIfFalse(Reg, i32),
    /// if R[cond].truthy(): pc += offset
    JumpIfTrue(Reg, i32),
    /// R[func_reg] = call(R[func_reg], R[func_reg+1..func_reg+1+argc]); result in R[func_reg]
    Call(Reg, u8),
    /// return R[src]
    Return(Reg),
    /// return None
    ReturnNone,
    /// R[dst] = [R[base], R[base+1], ..., R[base+n-1]]
    BuildList(Reg, Reg, u8),
    /// R[dst] = (R[base], R[base+1], ..., R[base+n-1])
    BuildTuple(Reg, Reg, u8),
    /// R[dst] = {R[base]: R[base+1], R[base+2]: R[base+3], ...}  (n key-value pairs)
    BuildDict(Reg, Reg, u8),
    /// R[base..base+n] = iter_values(R[src])
    Unpack(Reg, Reg, u8),
    /// iters[slot] = iter_values(R[src])
    GetIter(u8, Reg),
    /// if iters[slot] exhausted: pc += offset; else R[dst] = next(iters[slot])
    ForIter(Reg, u8, i32),
    /// error if R[reg] is uninitialised: "cannot access local variable '<name>' ..."
    CheckLocal(Reg, u16),
    /// raise AssertionError(R[msg])  (condition already tested by JumpIfTrue)
    RaiseAssert(Reg),
    /// raise R[exc]  (coerces class to instance)
    RaiseValue(Reg),
    /// raise R[exc] from R[cause]  (sets __cause__ on the coerced instance)
    RaiseFrom(Reg, Reg),
    /// re-raise active exception (bare `raise`)
    RaiseReRaise,
    /// R[dst] = new UserFunction(fn_protos[proto_idx], defaults R[defs_base..+defs_n], env=current)
    MakeFunction(Reg, u8, Reg, u8),
    /// R[dst] = load_module(names[name_idx])
    ImportModule(Reg, u16),
    /// Push an exception handler; if an exception is raised before PopExcept,
    /// the active_exception is set and pc jumps to (pc_after_this_insn + offset).
    SetupExcept(i32),
    /// Pop the innermost exception handler (normal exit from try block).
    PopExcept,
    /// R[dst] = current active exception value.
    LoadExc(Reg),
    /// if active_exception is NOT an instance of R[type_reg]: pc += offset.
    MatchExcept(Reg, i32),
    /// Clear active_exception (end of except handler).
    EndExcept,
    /// R[dst] = create class(fn_protos[proto_idx], bases R[bases_base..+bases_n], name=names[name_idx])
    MakeClass(Reg, u8, Reg, u8, u16),
    /// Print R[src] if not None (REPL expression output).
    PrintExpr(Reg),
    /// R[list].push(R[val])  — in-place append for variadic call construction
    ListAppend(Reg, Reg),
    /// R[list].extend(iter(R[src]))  — in-place extend
    ListExtend(Reg, Reg),
    /// R[dict].update(R[src])  — in-place dict merge
    DictUpdate(Reg, Reg),
}

#[derive(Debug)]
pub struct FnCode {
    pub(crate) insns: Vec<Insn>,
    /// Constant pool (literals used in the function body)
    pub(crate) consts: Vec<Value>,
    /// Name pool (global variable names and attribute names)
    pub(crate) names: Vec<String>,
    /// Number of registers needed (locals + max temporaries)
    pub(crate) num_regs: u8,
    /// Number of iterator slots needed
    pub(crate) num_iters: u8,
    /// Number of local variable slots (registers 0..num_locals are locals; the rest are temps)
    pub(crate) num_locals: u8,
    /// Nested function / class body prototypes
    pub(crate) fn_protos: Vec<FnProto>,
    /// Variables captured by nested functions (stored in env, not registers).
    pub(crate) cell_vars: Vec<CellVar>,
}
