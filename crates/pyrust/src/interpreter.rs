use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use crate::ast::{AssignTarget, BinaryOp, CallArg, CmpOp, Expr, Stmt, UnaryOp};
use crate::bytecode::FnCode;
use crate::error::{PyError, Result};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::value::{
    EnvRef, Environment, PyBigInt, PyClass, PyInstance, PyKey, PyModule, UserFunction,
    UserFunctionKind, UserFunctionParam, Value, ValueKind, range_len,
};

type ModuleCache = Rc<RefCell<HashMap<String, Value>>>;
type FnCache = HashMap<(u64, Vec<PyKey>), Value>;

const MAX_CALL_DEPTH: usize = 1000;
const ENV_POOL_MAX: usize = 64;
const SPEC_THRESHOLD: u8 = 8;

pub struct Interpreter {
    env: EnvRef,
    active_exception: Option<Value>,
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
            script_dir: None,
            module_cache: Rc::new(RefCell::new(HashMap::new())),
            env_pool: Vec::new(),
            fn_cache: HashMap::new(),
            spec_cache: HashMap::new(),
            call_arg_buf: Vec::new(),
            key_scratch: Vec::new(),
        }
    }
}

include!("interpreter/runtime.rs");

include!("interpreter/helpers.rs");

include!("interpreter/tests.rs");
