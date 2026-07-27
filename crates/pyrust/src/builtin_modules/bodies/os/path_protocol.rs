//! Interpreter-facing `os.fspath` protocol dispatch.
//!
//! Path protocol lookup is kept separate from host I/O: it calls Python code
//! and validates the returned object, but never touches the filesystem.

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, reject_keyword_args_expanded, value_type_name_str,
};
use crate::value::{Value, ValueKind};

pub(super) fn fspath(
    interpreter: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    }

    let object = args[0].value.clone();
    if matches!(object.kind(), ValueKind::Str(_) | ValueKind::Bytes(_)) {
        return Ok(object);
    }

    if let Ok(method) = interpreter.get_attr(&object, "__fspath__") {
        let result = interpreter.call_function_expanded(method, &[])?;
        if matches!(result.kind(), ValueKind::Str(_) | ValueKind::Bytes(_)) {
            return Ok(result);
        }
        return Err(PyError::named(
            "TypeError",
            format!(
                "expected {}.__fspath__() to return str or bytes, not {}",
                value_type_name_str(&object),
                value_type_name_str(&result),
            ),
        ));
    }

    Err(PyError::named(
        "TypeError",
        format!(
            "expected str, bytes or os.PathLike object, not {}",
            value_type_name_str(&object),
        ),
    ))
}
