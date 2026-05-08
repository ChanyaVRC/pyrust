use crate::ast::{BinaryOp, UnaryOp};
use crate::value::Value;

pub type Reg = u8;

#[derive(Debug, Clone)]
pub enum Insn {
    /// R[dst] = consts[idx]
    LoadConst(Reg, u16),
    /// R[dst] = lookup name through env chain
    LoadGlobal(Reg, u16),
    /// R[dst] = None
    LoadNone(Reg),
    /// R[dst] = R[src]
    Move(Reg, Reg),
    /// R[dst] = R[lhs] op R[rhs]
    BinOp(Reg, Reg, BinaryOp, Reg),
    /// R[dst] = R[lhs] op consts[const_idx]  (fuses LoadConst + BinOp)
    BinOpConst(Reg, Reg, BinaryOp, u16),
    /// R[dst] = unary_op(R[src])
    UnaryOp(Reg, UnaryOp, Reg),
    /// R[dst] = R[obj].names[name_idx]
    GetAttr(Reg, Reg, u16),
    /// R[obj].names[name_idx] = R[val]
    SetAttr(Reg, u16, Reg),
    /// R[dst] = R[obj][R[idx]]
    GetItem(Reg, Reg, Reg),
    /// R[obj][R[idx]] = R[val]
    SetItem(Reg, Reg, Reg),
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
}
