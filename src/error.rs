use std::fmt;

use crate::value::Value;

#[derive(Debug, Clone)]
pub enum PyError {
    Lex(String),
    Parse(String),
    Runtime(String),
    Raised(Value),
}

impl fmt::Display for PyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PyError::Lex(s) => write!(f, "Lex error: {s}"),
            PyError::Parse(s) => write!(f, "Parse error: {s}"),
            PyError::Runtime(s) => write!(f, "Runtime error: {s}"),
            PyError::Raised(value) => write!(f, "Uncaught exception: {}", value.repr()),
        }
    }
}

pub type Result<T> = std::result::Result<T, PyError>;
