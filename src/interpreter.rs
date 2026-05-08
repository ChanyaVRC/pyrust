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
    Value,
};

#[derive(Debug, Clone, PartialEq)]
enum ExecSignal {
    None,
    Break,
    Continue,
    Return(Value),
}

pub struct Interpreter {
    env: EnvRef,
    class_closure_env: Option<EnvRef>,
    active_exception: Option<Value>,
    script_dir: Option<PathBuf>,
    module_cache: HashMap<String, Value>,
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
            module_cache: HashMap::new(),
        }
    }
}

include!("interpreter/runtime.rs");

include!("interpreter/helpers.rs");

include!("interpreter/tests.rs");
