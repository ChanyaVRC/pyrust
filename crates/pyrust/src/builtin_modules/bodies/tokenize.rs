// `tokenize` module — body for the `tokenize @inject` entry in
// `pyrust_builtin_modules!` (issue #3204).
//
// Native stateful Python 3.12 tokenizer (`tokenize/lexer.rs`, shared with
// `ast`) backs the lazy `generate_tokens(readline)` and `tokenize(readline)`
// generators in `tokenize_py.py`, alongside `TokenInfo`, `TokenError`,
// `StopTokenizing`, `detect_encoding`, `untokenize`, `open`, and the
// re-exported `token` constants.
//
// Reference: <https://docs.python.org/3.12/library/tokenize.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_core::{BuiltinTypeOps, builtin_ops_is};
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyValue;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyKey, Value, ValueKind};

#[path = "tokenize/lexer.rs"]
pub(crate) mod lexer;

use self::lexer::{LexError, LexToken, StatefulLexer};

/// Python-source definitions for the `tokenize` module.
const TOKENIZE_PY_SOURCE: &str = include_str!("tokenize_py.py");

/// Private native helpers moved into `TOKENIZE_PY_SOURCE`'s namespace and
/// removed from `dir(tokenize)` after injection.
const TOKENIZE_PRIVATE_HELPERS: &[&str] = &["_lexer_new", "_lexer_feed_line", "_lexer_finish"];

/// Public names exported from `TOKENIZE_PY_SOURCE` onto the `tokenize` module.
const TOKENIZE_PY_EXPORTS: &[&str] = &[
    "__all__",
    "ENDMARKER",
    "NAME",
    "NUMBER",
    "STRING",
    "NEWLINE",
    "INDENT",
    "DEDENT",
    "LPAR",
    "RPAR",
    "LSQB",
    "RSQB",
    "COLON",
    "COMMA",
    "SEMI",
    "PLUS",
    "MINUS",
    "STAR",
    "SLASH",
    "VBAR",
    "AMPER",
    "LESS",
    "GREATER",
    "EQUAL",
    "DOT",
    "PERCENT",
    "LBRACE",
    "RBRACE",
    "EQEQUAL",
    "NOTEQUAL",
    "LESSEQUAL",
    "GREATEREQUAL",
    "TILDE",
    "CIRCUMFLEX",
    "LEFTSHIFT",
    "RIGHTSHIFT",
    "DOUBLESTAR",
    "PLUSEQUAL",
    "MINEQUAL",
    "STAREQUAL",
    "SLASHEQUAL",
    "PERCENTEQUAL",
    "AMPEREQUAL",
    "VBAREQUAL",
    "CIRCUMFLEXEQUAL",
    "LEFTSHIFTEQUAL",
    "RIGHTSHIFTEQUAL",
    "DOUBLESTAREQUAL",
    "DOUBLESLASH",
    "DOUBLESLASHEQUAL",
    "AT",
    "ATEQUAL",
    "RARROW",
    "ELLIPSIS",
    "COLONEQUAL",
    "EXCLAMATION",
    "OP",
    "AWAIT",
    "ASYNC",
    "TYPE_IGNORE",
    "TYPE_COMMENT",
    "SOFT_KEYWORD",
    "FSTRING_START",
    "FSTRING_MIDDLE",
    "FSTRING_END",
    "COMMENT",
    "NL",
    "ERRORTOKEN",
    "ENCODING",
    "N_TOKENS",
    "NT_OFFSET",
    "tok_name",
    "EXACT_TOKEN_TYPES",
    "ISTERMINAL",
    "ISNONTERMINAL",
    "ISEOF",
    "TokenInfo",
    "TokenError",
    "StopTokenizing",
    "generate_tokens",
    "tokenize",
    "detect_encoding",
    "untokenize",
    "open",
    "Untokenizer",
];

struct LexerHandleOps;
const LEXER_HANDLE_OPS: &LexerHandleOps = &LexerHandleOps;

impl BuiltinTypeOps for LexerHandleOps {
    fn type_name(&self) -> &'static str {
        "_tokenize_lexer"
    }

    fn is_internal_storage(&self) -> bool {
        true
    }
}

fn lex_tokens_to_py_result(res: std::result::Result<Vec<LexToken>, LexError>) -> Value {
    match res {
        Ok(tokens) => {
            let items: Vec<Value> = tokens
                .into_iter()
                .map(|t| {
                    Value::tuple(vec![
                        Value::int(t.tok_type),
                        Value::string(t.string),
                        Value::tuple(vec![
                            Value::int(t.start_line as i64),
                            Value::int(t.start_col as i64),
                        ]),
                        Value::tuple(vec![
                            Value::int(t.end_line as i64),
                            Value::int(t.end_col as i64),
                        ]),
                        Value::string(t.line),
                    ])
                })
                .collect();
            Value::tuple(vec![Value::bool_(true), Value::list(items)])
        }
        Err(LexError::TokenError { msg, line, col }) => Value::tuple(vec![
            Value::bool_(false),
            Value::string("TokenError"),
            Value::string(msg),
            Value::int(line as i64),
            Value::int(col as i64),
            Value::string(""),
        ]),
        Err(LexError::IndentationError {
            msg,
            line,
            col,
            text,
        }) => Value::tuple(vec![
            Value::bool_(false),
            Value::string("IndentationError"),
            Value::string(msg),
            Value::int(line as i64),
            Value::int(col as i64),
            Value::string(text),
        ]),
        Err(LexError::TabError {
            msg,
            line,
            col,
            text,
        }) => Value::tuple(vec![
            Value::bool_(false),
            Value::string("TabError"),
            Value::string(msg),
            Value::int(line as i64),
            Value::int(col as i64),
            Value::string(text),
        ]),
    }
}

pyrust_module! {
    #[arity_style(takes_no_args)]
    fn _lexer_new() -> Result<Value> {
        Ok(Value::builtin_object(
            LEXER_HANDLE_OPS,
            Box::new(StatefulLexer::new()),
        ))
    }

    #[arity_style(clinic)]
    fn _lexer_feed_line(
        #[positional_only] handle: PyValue,
        #[positional_only] line: PyValue,
    ) -> Result<Value> {
        let line_str = line.0.as_str().ok_or_else(|| {
            PyError::named("TypeError", "expected str line".to_string())
        })?;
        let ValueKind::BuiltinObject { ops, state } = handle.0.kind() else {
            return Err(PyError::named("TypeError", "invalid lexer handle".to_string()));
        };
        if !builtin_ops_is::<LexerHandleOps>(ops) {
            return Err(PyError::named("TypeError", "invalid lexer handle".to_string()));
        }
        let mut borrowed = state.borrow_mut();
        let lexer = borrowed
            .downcast_mut::<StatefulLexer>()
            .ok_or_else(|| PyError::runtime("corrupt lexer handle".to_string()))?;
        let res = lexer.feed_line(line_str);
        Ok(lex_tokens_to_py_result(res))
    }

    #[arity_style(takes_exactly_one)]
    fn _lexer_finish(#[positional_only] handle: PyValue) -> Result<Value> {
        let ValueKind::BuiltinObject { ops, state } = handle.0.kind() else {
            return Err(PyError::named("TypeError", "invalid lexer handle".to_string()));
        };
        if !builtin_ops_is::<LexerHandleOps>(ops) {
            return Err(PyError::named("TypeError", "invalid lexer handle".to_string()));
        }
        let mut borrowed = state.borrow_mut();
        let lexer = borrowed
            .downcast_mut::<StatefulLexer>()
            .ok_or_else(|| PyError::runtime("corrupt lexer handle".to_string()))?;
        let res = lexer.finish();
        Ok(lex_tokens_to_py_result(res))
    }
}

/// Execute `TOKENIZE_PY_SOURCE` once, wire its wrappers to the native stateful
/// lexer helpers, hide private helpers from `dir(tokenize)`, and publish the
/// public CPython 3.12 `tokenize` surface onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for &helper in TOKENIZE_PRIVATE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source_with_filename(
        TOKENIZE_PY_SOURCE,
        "<frozen tokenize>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("tokenize: exec namespace not a dict".into()))?;
    for &name in TOKENIZE_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("tokenize"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}
