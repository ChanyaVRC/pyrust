// `keyword` module — 100% native Rust implementation of Python 3.12's `keyword`
// module (issue #3248).
//
// Exposes the exact 35 Python 3.12 keywords (`kwlist`), the 4 soft keywords
// (`softkwlist`), and membership predicates (`iskeyword`, `issoftkeyword`)
// matching CPython's `frozenset(kwlist).__contains__` semantics (returning
// `False` for hashable non-strings and raising `TypeError` for unhashable
// values).
//
// Reference: <https://docs.python.org/3/library/keyword.html>

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::builtin_args::PyValueRef;
use crate::interpreter::{ExpandedCallArg, hash_value_with_interp};
use crate::value::{Value, ValueKind};

/// Exact Python 3.12 keyword table (35 keywords, sorted).
const KWLIST: [&str; 35] = [
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

/// Exact Python 3.12 soft-keyword table (4 soft keywords, sorted).
const SOFTKWLIST: [&str; 4] = ["_", "case", "match", "type"];

fn str_list(items: &[&str]) -> Value {
    Value::list(items.iter().map(|s| Value::string(*s)).collect())
}

fn contains_keyword(
    interp: &mut crate::interpreter::Interpreter,
    table: &[&str],
    value: &Value,
) -> Result<Value> {
    if let Some(s) = value.as_str() {
        return Ok(Value::bool_(table.binary_search(&s).is_ok()));
    }
    // CPython's `frozenset.__contains__` coerces `set` keys to a temporary
    // `frozenset` for lookup rather than raising TypeError; since keyword
    // tables only contain strings, a `set` is never a member.
    if matches!(value.kind(), ValueKind::Set(_)) {
        return Ok(Value::bool_(false));
    }
    // Match `frozenset.__contains__`: validate hashability (raising TypeError
    // for unhashable inputs like lists/dicts) and return False for any
    // hashable non-str value.
    let _ = hash_value_with_interp(interp, value)?;
    Ok(Value::bool_(false))
}

pyrust_module! {
    constants {
        "__all__" => str_list(&["iskeyword", "issoftkeyword", "kwlist", "softkwlist"]),
        "kwlist" => str_list(&KWLIST),
        "softkwlist" => str_list(&SOFTKWLIST),
    }

    #[arity_style(takes_exactly_one)]
    fn iskeyword(#[positional_only] s: PyValueRef<'_>) -> Result<Value> {
        contains_keyword(_interp, &KWLIST, s.0)
    }

    #[arity_style(takes_exactly_one)]
    fn issoftkeyword(#[positional_only] s: PyValueRef<'_>) -> Result<Value> {
        contains_keyword(_interp, &SOFTKWLIST, s.0)
    }
}
