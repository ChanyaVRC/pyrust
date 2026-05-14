use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use indexmap::IndexMap;
use smallvec::smallvec;

use crate::ast::{AssignTarget, BinaryOp, CallArg, CmpOp, Expr, Stmt, UnaryOp};
use crate::bytecode::FnCode;
use crate::error::{PyError, Result};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::value::{
    EnvRef, Environment, PyBigInt, PyClass, PyInstance, PyKey, PyModule, UserFunction,
    UserFunctionKind, UserFunctionParam, Value, ValueKind,
};

type ModuleCache = Rc<RefCell<HashMap<String, Value>>>;
type FnCache = HashMap<(u64, Vec<PyKey>), Value>;

const MAX_CALL_DEPTH: usize = 1000;
const ENV_POOL_MAX: usize = 64;
const SPEC_THRESHOLD: u8 = 8;

pub struct Interpreter {
    pub(crate) env: EnvRef,
    active_exception: Option<Value>,
    /// Stack of currently-handled exceptions (PEP 3134 `__context__`).
    /// An entry is pushed when an exception handler catches an exception
    /// (matching CPython's per-frame "exc_info" stack), and popped when
    /// the handler exits — either normally via `EndExcept`, or because
    /// a new exception propagates out of the handler body.
    ///
    /// The top of this stack is the exception whose handler we are
    /// currently inside, and it is the value attached as `__context__`
    /// to any *new* exception raised inside that handler body.
    ///
    /// Note: this differs from `active_exception`, which can be cleared
    /// transiently while the handler body is running (e.g. by an inner
    /// `EndExcept`).  The stack lets us restore the outer context.
    handled_exc_stack: Vec<Value>,
    script_dir: Option<PathBuf>,
    module_cache: ModuleCache,
    env_pool: Vec<EnvRef>,
    fn_cache: FnCache,
    spec_cache: HashMap<usize, SpecState>,
    /// Reusable argument buffer for VM Call instructions — avoids a per-call
    /// heap allocation in the common (non-recursive) case.
    call_arg_buf: Vec<ExpandedCallArg>,
    /// Reusable scratch buffer for building fn_cache probe keys — avoids a
    /// per-probe heap allocation in CallMemo's cache-hit path.
    key_scratch: Vec<crate::value::PyKey>,
    /// Stack of class-namespace store-order lists, pushed by `MakeClass` before
    /// running a class body and popped after.  Each entry tracks the slot numbers
    /// for class-body locals in the **order stores actually executed** at
    /// runtime — CPython's `__dict__` ordering rule.  A stack (not a single Vec)
    /// supports nested `class A: class B: ...` bodies cleanly.
    pub(crate) class_store_order: Vec<Vec<u32>>,
}

/// Thin wrapper around `iter_values` matching pyrust-core's `IterValuesFn`
/// signature (`&Value -> Result<Vec<Value>>`).  Installed at interpreter
/// startup so `pyrust-builtins` iterator helpers can drain arbitrary
/// sources without depending on this crate.
fn iter_values_for_registry(value: &Value) -> Result<Vec<Value>> {
    iter_values(value.clone())
}

impl Default for Interpreter {
    fn default() -> Self {
        pyrust_builtins::install();
        pyrust_core::install_iter_values(iter_values_for_registry);
        let env = Environment::new(None);
        install_exception_builtins(&env);
        install_singleton_builtins(&env);
        Self {
            env,
            active_exception: None,
            handled_exc_stack: Vec::new(),
            script_dir: None,
            module_cache: Rc::new(RefCell::new(HashMap::new())),
            env_pool: Vec::new(),
            fn_cache: HashMap::new(),
            spec_cache: HashMap::new(),
            call_arg_buf: Vec::new(),
            key_scratch: Vec::new(),
            class_store_order: Vec::new(),
        }
    }
}

include!("interpreter/runtime.rs");

include!("interpreter/helpers.rs");

pub(crate) mod builtin_args;

include!("interpreter/tests.rs");
