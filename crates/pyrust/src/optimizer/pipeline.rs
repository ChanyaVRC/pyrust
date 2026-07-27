use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::ast::BinaryOp;
use crate::bytecode::{
    AttrCacheEntry, BinOpCacheEntry, FnCode, FnProto, GlobalCacheEntry, Insn, KwCallCacheEntry,
    MAX_FRAME_REGS, Reg,
};
use crate::value::{Value, ValueKind};

/// Optimize a compiled `FnCode` and all nested function prototypes.
/// Applies a sequence of peephole passes over each instruction list.
pub fn optimize(code: FnCode) -> FnCode {
    optimize_fn_code(code)
}

// Pipeline implementation grouped by metadata mapping and pass orchestration.

include!("pipeline/source_mapping.rs");
include!("pipeline/driver.rs");
