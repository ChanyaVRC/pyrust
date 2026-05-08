use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use crate::ast::{
    AssignTarget, BinaryOp, CallArg, CmpOp, ExceptHandler, Expr, FunctionParam, Stmt, UnaryOp,
};
use crate::error::{PyError, Result};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::value::{
    EnvRef, Environment, PyClass, PyInstance, PyKey, PyModule, UserFunction, UserFunctionParam,
    Value, range_len,
};

#[derive(Debug, Clone, PartialEq)]
enum ExecSignal {
    None,
    Break,
    Continue,
    Return(Value),
}

type ModuleCache = Rc<RefCell<HashMap<String, Value>>>;

const MAX_CALL_DEPTH: usize = 1000;
const ENV_POOL_MAX: usize = 64;

pub struct Interpreter {
    env: EnvRef,
    class_closure_env: Option<EnvRef>,
    active_exception: Option<Value>,
    script_dir: Option<PathBuf>,
    module_cache: ModuleCache,
    call_depth: usize,
    env_pool: Vec<EnvRef>,
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
        }
    }
}

include!("interpreter/runtime.rs");

include!("interpreter/helpers.rs");

include!("interpreter/tests.rs");
