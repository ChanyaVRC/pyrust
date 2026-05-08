use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};

use crate::ast::Stmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PyKey {
    Int(i64),
    Float(u64),
    Str(String),
    Bool(bool),
    None,
}

impl Hash for PyKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            PyKey::Int(v) => v.hash(state),
            PyKey::Bool(b) => b.hash(state),
            PyKey::Float(bits) => bits.hash(state),
            PyKey::Str(s) => s.hash(state),
            PyKey::None => {}
        }
    }
}

pub type NameSet = Rc<HashSet<String>>;

#[derive(Debug, Clone)]
pub struct Environment {
    pub values: HashMap<String, Value>,
    pub local_names: NameSet,
    pub global_names: NameSet,
    pub nonlocal_names: NameSet,
    pub parent: Option<EnvRef>,
}

pub type EnvRef = Rc<RefCell<Environment>>;

impl Environment {
    pub fn new(parent: Option<EnvRef>) -> EnvRef {
        Rc::new(RefCell::new(Self {
            values: HashMap::new(),
            local_names: Rc::new(HashSet::new()),
            global_names: Rc::new(HashSet::new()),
            nonlocal_names: Rc::new(HashSet::new()),
            parent,
        }))
    }
}

#[derive(Debug, Clone)]
pub struct UserFunctionParam {
    pub name: String,
    pub default: Option<Value>,
    pub is_args: bool,
    pub is_kwargs: bool,
}

#[derive(Debug, Clone)]
pub struct UserFunction {
    pub name: String,
    pub params: Vec<UserFunctionParam>,
    pub body: Vec<Stmt>,
    pub local_names: NameSet,
    pub global_names: NameSet,
    pub nonlocal_names: NameSet,
    pub env: EnvRef,
}

#[derive(Debug, Clone)]
pub struct PyClass {
    pub name: String,
    pub base: Option<Rc<RefCell<PyClass>>>,
    pub attrs: HashMap<String, Value>,
}

#[derive(Debug, Clone)]
pub struct PyInstance {
    pub class: Rc<RefCell<PyClass>>,
    pub attrs: HashMap<String, Value>,
}

#[derive(Debug, Clone)]
pub struct PyModule {
    pub name: String,
    pub attrs: HashMap<String, Value>,
}

#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    None,
    List(Vec<Value>),
    Dict(IndexMap<PyKey, Value>),
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
    Builtin(&'static str),
    Function(Rc<UserFunction>),
    Class(Rc<RefCell<PyClass>>),
    Instance(Rc<RefCell<PyInstance>>),
    BoundMethod {
        function: Rc<UserFunction>,
        receiver: Rc<RefCell<PyInstance>>,
    },
    Module(Rc<RefCell<PyModule>>),
    Tuple(Vec<Value>),
    Set(IndexSet<PyKey>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::None, Value::None) => true,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Dict(a), Value::Dict(b)) => a == b,
            (
                Value::Range {
                    start: a_start,
                    stop: a_stop,
                    step: a_step,
                },
                Value::Range {
                    start: b_start,
                    stop: b_stop,
                    step: b_step,
                },
            ) => a_start == b_start && a_stop == b_stop && a_step == b_step,
            (Value::Builtin(a), Value::Builtin(b)) => a == b,
            (Value::Function(a), Value::Function(b)) => Rc::ptr_eq(a, b),
            (Value::Class(a), Value::Class(b)) => Rc::ptr_eq(a, b),
            (Value::Instance(a), Value::Instance(b)) => Rc::ptr_eq(a, b),
            (Value::Module(a), Value::Module(b)) => Rc::ptr_eq(a, b),
            (
                Value::BoundMethod {
                    function: a_function,
                    receiver: a_receiver,
                },
                Value::BoundMethod {
                    function: b_function,
                    receiver: b_receiver,
                },
            ) => Rc::ptr_eq(a_function, b_function) && Rc::ptr_eq(a_receiver, b_receiver),
            (Value::Tuple(a), Value::Tuple(b)) => a == b,
            (Value::Set(a), Value::Set(b)) => a == b,
            _ => false,
        }
    }
}

impl Value {
    pub fn truthy(&self) -> bool {
        match self {
            Value::Bool(v) => *v,
            Value::Int(v) => *v != 0,
            Value::Float(v) => *v != 0.0,
            Value::Str(v) => !v.is_empty(),
            Value::None => false,
            Value::List(v) => !v.is_empty(),
            Value::Dict(v) => !v.is_empty(),
            Value::Range { start, stop, step } => range_len(*start, *stop, *step) > 0,
            Value::Builtin(_) => true,
            Value::Function(_) => true,
            Value::Class(_) => true,
            Value::Instance(_) => true,
            Value::BoundMethod { .. } => true,
            Value::Module(_) => true,
            Value::Tuple(v) => !v.is_empty(),
            Value::Set(v) => !v.is_empty(),
        }
    }

    pub fn to_py_str(&self) -> String {
        match self {
            Value::Instance(instance) if is_exception_class(instance) => {
                exception_to_string(instance)
            }
            Value::Str(s) => s.clone(),
            _ => self.repr(),
        }
    }

    pub fn repr(&self) -> String {
        match self {
            Value::Int(v) => v.to_string(),
            Value::Float(v) => {
                if v.fract() == 0.0 {
                    format!("{v:.1}")
                } else {
                    v.to_string()
                }
            }
            Value::Str(v) => format!("'{}'", escape_str(v)),
            Value::Bool(v) => {
                if *v {
                    "True".to_string()
                } else {
                    "False".to_string()
                }
            }
            Value::None => "None".to_string(),
            Value::List(items) => {
                let inner = items
                    .iter()
                    .map(|v| v.repr())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{inner}]")
            }
            Value::Dict(items) => {
                let mut out = String::new();
                out.push('{');
                for (i, (k, v)) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&key_repr(k));
                    out.push_str(": ");
                    out.push_str(&v.repr());
                }
                out.push('}');
                out
            }
            Value::Range { start, stop, step } => {
                if *step == 1 {
                    format!("range({start}, {stop})")
                } else {
                    format!("range({start}, {stop}, {step})")
                }
            }
            Value::Builtin(name) => format!("<built-in function {name}>"),
            Value::Function(func) => format!("<function {}>", func.name),
            Value::Class(class) => {
                let name = class.borrow().name.clone();
                format!("<class '{name}'>")
            }
            Value::Instance(instance) => {
                if is_exception_class(instance) {
                    return exception_repr(instance);
                }
                let class_name = instance.borrow().class.borrow().name.clone();
                format!("<{class_name} object>")
            }
            Value::BoundMethod { function, receiver } => {
                let class_name = receiver.borrow().class.borrow().name.clone();
                format!("<bound method {class_name}.{}>", function.name)
            }
            Value::Module(m) => format!("<module '{}'>", m.borrow().name),
            Value::Tuple(items) => {
                let inner = items
                    .iter()
                    .map(|v| v.repr())
                    .collect::<Vec<_>>()
                    .join(", ");
                if items.len() == 1 {
                    format!("({inner},)")
                } else {
                    format!("({inner})")
                }
            }
            Value::Set(items) => {
                if items.is_empty() {
                    return "set()".to_string();
                }
                let inner = items.iter().map(key_repr).collect::<Vec<_>>().join(", ");
                format!("{{{inner}}}")
            }
        }
    }

    pub fn to_key(&self) -> Option<PyKey> {
        match self {
            Value::Int(v) => Some(PyKey::Int(*v)),
            Value::Float(v) => Some(PyKey::Float(v.to_bits())),
            Value::Str(v) => Some(PyKey::Str(v.clone())),
            Value::Bool(v) => Some(PyKey::Bool(*v)),
            Value::None => Some(PyKey::None),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_py_str())
    }
}

fn key_repr(key: &PyKey) -> String {
    match key {
        PyKey::Int(v) => v.to_string(),
        PyKey::Float(v) => {
            let as_f = f64::from_bits(*v);
            if as_f.fract() == 0.0 {
                format!("{as_f:.1}")
            } else {
                as_f.to_string()
            }
        }
        PyKey::Str(v) => format!("'{}'", escape_str(v)),
        PyKey::Bool(v) => {
            if *v {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        PyKey::None => "None".to_string(),
    }
}

fn is_exception_class(instance: &Rc<RefCell<PyInstance>>) -> bool {
    let class = Rc::clone(&instance.borrow().class);
    class_chain_contains_exception(&class)
}

fn class_chain_contains_exception(class: &Rc<RefCell<PyClass>>) -> bool {
    let (name, base) = {
        let borrowed = class.borrow();
        (borrowed.name.clone(), borrowed.base.clone())
    };
    if name == "Exception" {
        return true;
    }
    base.is_some_and(|base| class_chain_contains_exception(&base))
}

fn exception_args(instance: &Rc<RefCell<PyInstance>>) -> Vec<Value> {
    match instance.borrow().attrs.get("args") {
        Some(Value::List(args)) => args.clone(),
        _ => Vec::new(),
    }
}

fn format_exception_args(args: &[Value], repr_mode: bool) -> String {
    match args {
        [] => String::new(),
        [value] => {
            if repr_mode {
                value.repr()
            } else {
                value.to_py_str()
            }
        }
        _ => {
            let inner = args
                .iter()
                .map(|value| value.repr())
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
    }
}

fn exception_to_string(instance: &Rc<RefCell<PyInstance>>) -> String {
    let args = exception_args(instance);
    format_exception_args(&args, false)
}

fn exception_repr(instance: &Rc<RefCell<PyInstance>>) -> String {
    let class_name = instance.borrow().class.borrow().name.clone();
    let args = exception_args(instance);
    if args.is_empty() {
        format!("{class_name}()")
    } else {
        format!("{class_name}({})", format_exception_args(&args, true))
    }
}

fn escape_str(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
        .replace('\'', "\\'")
}

pub(crate) fn range_len(start: i64, stop: i64, step: i64) -> i64 {
    if step == 0 {
        return 0;
    }
    if step > 0 {
        if start >= stop {
            0
        } else {
            ((stop - start - 1) / step) + 1
        }
    } else if start <= stop {
        0
    } else {
        ((start - stop - 1) / (-step)) + 1
    }
}
