// `token` module — body for the `token @inject` entry in
// `pyrust_builtin_modules!` (issue #3204).
//
// CPython 3.12's `token` module defines integer token type constants
// (`ENDMARKER`..`ENCODING`, `N_TOKENS`, `NT_OFFSET`), `tok_name`,
// `EXACT_TOKEN_TYPES`, and the predicates `ISTERMINAL`, `ISNONTERMINAL`, and
// `ISEOF`.  The entire Python 3.12 surface lives in `token_py.py` and is
// injected onto the module by `inject_python_members`.
//
// Reference: <https://docs.python.org/3.12/library/token.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `token` module.
const TOKEN_PY_SOURCE: &str = include_str!("token_py.py");

/// Public names exported from `TOKEN_PY_SOURCE` onto the `token` module.
const TOKEN_PY_EXPORTS: &[&str] = &[
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
];

/// Execute `TOKEN_PY_SOURCE` once and publish its public names onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source_with_filename(TOKEN_PY_SOURCE, "<frozen token>", Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("token: exec namespace not a dict".into()))?;
    for &name in TOKEN_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}
