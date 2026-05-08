use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use crate::ast::{
    AssignTarget, BinaryOp, CallArg, CmpOp, ExceptHandler, Expr, FunctionParam, Stmt, UnaryOp,
};
use crate::bytecode::FnCode;
use crate::error::{PyError, Result};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::value::{
    EnvRef, Environment, FunctionLocals, PyClass, PyInstance, PyKey, PyModule, UserFunction,
    UserFunctionParam, Value, range_len,
};

#[derive(Debug, Clone, PartialEq)]
enum ExecSignal {
    None,
    Break,
    Continue,
    Return(Box<Value>),
}

type ModuleCache = Rc<RefCell<HashMap<String, Value>>>;
type FnCache = HashMap<(usize, Vec<PyKey>), Value>;

const MAX_CALL_DEPTH: usize = 1000;
const ENV_POOL_MAX: usize = 64;
const SPEC_THRESHOLD: u8 = 8;
const HOT_THRESHOLD: u32 = 50;

pub struct Interpreter {
    env: EnvRef,
    class_closure_env: Option<EnvRef>,
    active_exception: Option<Value>,
    script_dir: Option<PathBuf>,
    module_cache: ModuleCache,
    call_depth: usize,
    env_pool: Vec<EnvRef>,
    fn_cache: FnCache,
    spec_cache: HashMap<usize, SpecState>,
    /// Tier-1 warmup counters: fn_ptr → call count.
    call_counts: HashMap<usize, u32>,
    /// Tier-2 hot frames: fn_ptr → dedicated pre-warmed EnvRef.
    hot_frames: HashMap<usize, EnvRef>,
    /// Set of fn_ptrs whose hot frame is currently executing (recursion guard).
    hot_frames_active: HashSet<usize>,
    /// Bytecode cache: fn_ptr → compiled FnCode (None = compilation failed, use tree-walker).
    bytecode_cache: HashMap<usize, (std::rc::Weak<UserFunction>, Option<Rc<FnCode>>)>,
}

impl Default for Interpreter {
    fn default() -> Self {
        let env = Environment::new(None);
        install_exception_builtins(&env);
        Self {
            env,
            class_closure_env: None,
            active_exception: None,
            script_dir: None,
            module_cache: Rc::new(RefCell::new(HashMap::new())),
            call_depth: 0,
            env_pool: Vec::new(),
            fn_cache: HashMap::new(),
            spec_cache: HashMap::new(),
            call_counts: HashMap::new(),
            hot_frames: HashMap::new(),
            hot_frames_active: HashSet::new(),
            bytecode_cache: HashMap::new(),
        }
    }
}

include!("interpreter/runtime.rs");

include!("interpreter/helpers.rs");

include!("interpreter/tests.rs");
