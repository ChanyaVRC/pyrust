// `token` — 100% native Rust implementation of CPython 3.12's `token` module
// (issues #3204, #3348).
//
// CPython's `Lib/token.py` defines the integer token type constants
// (`ENDMARKER`..`ENCODING`, `N_TOKENS`, `NT_OFFSET`), the reverse map
// `tok_name`, the operator map `EXACT_TOKEN_TYPES`, and the predicates
// `ISTERMINAL`, `ISNONTERMINAL`, and `ISEOF`.  Everything is generated here
// from two declarative tables (see `token_module!`); there is no Python-source
// injection.
//
// Module namespace order matches CPython: `__doc__`, `__all__`, the constants
// in value order, `tok_name`, `EXACT_TOKEN_TYPES`, then the predicates.
// `tok_name` and `__all__` are derived from the constant table exactly like
// `Lib/token.py` does (`tok_name` maps every int constant back to its name,
// `__all__` lists `tok_name`, the three predicates, then every constant name).
//
// Reference: <https://docs.python.org/3.12/library/token.html>

use crate::ast::BinaryOp;
use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::PyValue;
use crate::value::{PyDict, PyKey, Value};
use pyrust_derive::pyrust_module;

const TOKEN_DOC: &str = "Token constants.";

/// Value of `NT_OFFSET`, used by the predicates.
const NT_OFFSET: i64 = 256;

/// Declares the constant table, derived helpers, and the `pyrust_module!`
/// body from the two tables below, so each token name is written exactly once.
macro_rules! token_module {
    (
        constants { $($name:literal = $value:literal,)+ }
        exact { $($op:literal => $target:literal,)+ }
    ) => {
        /// `(name, value)` for every token constant, in value order.
        const TOKEN_CONSTANTS: &[(&str, i64)] = &[$(($name, $value)),+];

        /// `(operator text, constant name)` for `EXACT_TOKEN_TYPES`, in
        /// CPython's (sorted) order.
        const EXACT_TOKENS: &[(&str, &str)] = &[$(($op, $target)),+];

        pyrust_module! {
            constants {
                "__doc__" => Value::string(TOKEN_DOC),
                "__all__" => build_all(),
                $($name => Value::int($value),)+
                "tok_name" => build_tok_name(),
                "EXACT_TOKEN_TYPES" => build_exact_token_types(),
            }

            fn ISTERMINAL(x: PyValue) -> Result<Value> {
                _interp.eval_binary(x.0, BinaryOp::Lt, Value::int(NT_OFFSET))
            }

            fn ISNONTERMINAL(x: PyValue) -> Result<Value> {
                _interp.eval_binary(x.0, BinaryOp::Ge, Value::int(NT_OFFSET))
            }

            fn ISEOF(x: PyValue) -> Result<Value> {
                _interp.eval_binary(x.0, BinaryOp::Eq, Value::int(0))
            }
        }
    };
}

/// `__all__`: `tok_name`, the three predicates, then every constant name.
fn build_all() -> Value {
    let head = ["tok_name", "ISTERMINAL", "ISNONTERMINAL", "ISEOF"];
    Value::list(
        head.iter()
            .copied()
            .chain(TOKEN_CONSTANTS.iter().map(|&(name, _)| name))
            .map(Value::string)
            .collect(),
    )
}

/// `tok_name`: every integer constant mapped back to its name.
fn build_tok_name() -> Value {
    let mut d = PyDict::default();
    for &(name, value) in TOKEN_CONSTANTS {
        d.insert(PyKey::Int(value), Value::string(name));
    }
    Value::dict(d)
}

/// `EXACT_TOKEN_TYPES`: operator / delimiter text mapped to its constant.
fn build_exact_token_types() -> Value {
    let mut d = PyDict::default();
    for &(op, target) in EXACT_TOKENS {
        let value = TOKEN_CONSTANTS
            .iter()
            .find(|&&(name, _)| name == target)
            .map(|&(_, value)| value)
            .expect("EXACT_TOKEN_TYPES entry names an undefined token constant");
        d.insert(PyKey::str_from(op), Value::int(value));
    }
    Value::dict(d)
}

token_module! {
    constants {
        "ENDMARKER" = 0,
        "NAME" = 1,
        "NUMBER" = 2,
        "STRING" = 3,
        "NEWLINE" = 4,
        "INDENT" = 5,
        "DEDENT" = 6,
        "LPAR" = 7,
        "RPAR" = 8,
        "LSQB" = 9,
        "RSQB" = 10,
        "COLON" = 11,
        "COMMA" = 12,
        "SEMI" = 13,
        "PLUS" = 14,
        "MINUS" = 15,
        "STAR" = 16,
        "SLASH" = 17,
        "VBAR" = 18,
        "AMPER" = 19,
        "LESS" = 20,
        "GREATER" = 21,
        "EQUAL" = 22,
        "DOT" = 23,
        "PERCENT" = 24,
        "LBRACE" = 25,
        "RBRACE" = 26,
        "EQEQUAL" = 27,
        "NOTEQUAL" = 28,
        "LESSEQUAL" = 29,
        "GREATEREQUAL" = 30,
        "TILDE" = 31,
        "CIRCUMFLEX" = 32,
        "LEFTSHIFT" = 33,
        "RIGHTSHIFT" = 34,
        "DOUBLESTAR" = 35,
        "PLUSEQUAL" = 36,
        "MINEQUAL" = 37,
        "STAREQUAL" = 38,
        "SLASHEQUAL" = 39,
        "PERCENTEQUAL" = 40,
        "AMPEREQUAL" = 41,
        "VBAREQUAL" = 42,
        "CIRCUMFLEXEQUAL" = 43,
        "LEFTSHIFTEQUAL" = 44,
        "RIGHTSHIFTEQUAL" = 45,
        "DOUBLESTAREQUAL" = 46,
        "DOUBLESLASH" = 47,
        "DOUBLESLASHEQUAL" = 48,
        "AT" = 49,
        "ATEQUAL" = 50,
        "RARROW" = 51,
        "ELLIPSIS" = 52,
        "COLONEQUAL" = 53,
        "EXCLAMATION" = 54,
        "OP" = 55,
        "AWAIT" = 56,
        "ASYNC" = 57,
        "TYPE_IGNORE" = 58,
        "TYPE_COMMENT" = 59,
        "SOFT_KEYWORD" = 60,
        "FSTRING_START" = 61,
        "FSTRING_MIDDLE" = 62,
        "FSTRING_END" = 63,
        "COMMENT" = 64,
        "NL" = 65,
        "ERRORTOKEN" = 66,
        "ENCODING" = 67,
        "N_TOKENS" = 68,
        "NT_OFFSET" = 256,
    }
    exact {
        "!" => "EXCLAMATION",
        "!=" => "NOTEQUAL",
        "%" => "PERCENT",
        "%=" => "PERCENTEQUAL",
        "&" => "AMPER",
        "&=" => "AMPEREQUAL",
        "(" => "LPAR",
        ")" => "RPAR",
        "*" => "STAR",
        "**" => "DOUBLESTAR",
        "**=" => "DOUBLESTAREQUAL",
        "*=" => "STAREQUAL",
        "+" => "PLUS",
        "+=" => "PLUSEQUAL",
        "," => "COMMA",
        "-" => "MINUS",
        "-=" => "MINEQUAL",
        "->" => "RARROW",
        "." => "DOT",
        "..." => "ELLIPSIS",
        "/" => "SLASH",
        "//" => "DOUBLESLASH",
        "//=" => "DOUBLESLASHEQUAL",
        "/=" => "SLASHEQUAL",
        ":" => "COLON",
        ":=" => "COLONEQUAL",
        ";" => "SEMI",
        "<" => "LESS",
        "<<" => "LEFTSHIFT",
        "<<=" => "LEFTSHIFTEQUAL",
        "<=" => "LESSEQUAL",
        "=" => "EQUAL",
        "==" => "EQEQUAL",
        ">" => "GREATER",
        ">=" => "GREATEREQUAL",
        ">>" => "RIGHTSHIFT",
        ">>=" => "RIGHTSHIFTEQUAL",
        "@" => "AT",
        "@=" => "ATEQUAL",
        "[" => "LSQB",
        "]" => "RSQB",
        "^" => "CIRCUMFLEX",
        "^=" => "CIRCUMFLEXEQUAL",
        "{" => "LBRACE",
        "|" => "VBAR",
        "|=" => "VBAREQUAL",
        "}" => "RBRACE",
        "~" => "TILDE",
    }
}
