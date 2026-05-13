// `functools` module — body for the `functools` entry in
// `pyrust_builtin_modules!`.  Currently exposes `reduce`; the rest of
// the CPython functools API (`partial`, `lru_cache`, `wraps`,
// `cached_property`, …) is out of scope for this initial landing.
//
// Reference: <https://docs.python.org/3/library/functools.html>

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{iter_values, reject_keyword_args_expanded};
use crate::value::Value;
use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: functools.reduce(function, iterable[, initializer]).
    /// Apply `function` of two arguments cumulatively to the items of
    /// `iterable` — left-to-right — so as to reduce the iterable to a
    /// single value.  With `initializer`, it's placed before the items.
    /// <https://docs.python.org/3/library/functools.html#functools.reduce>
    fn reduce(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() < 2 || args.len() > 3 {
            return Err(PyError::Runtime(format!(
                "{FN_NAME}() takes 2 or 3 arguments",
            )));
        }
        let func = args[0].value.clone();
        let items = iter_values(args[1].value.clone())?;
        let mut iter = items.into_iter();
        let mut acc = if args.len() == 3 {
            // Initializer supplied → it's the seed; the iterable is folded
            // onto it from the left.  This is the only branch that's
            // happy with an empty iterable.
            args[2].value.clone()
        } else {
            // No initializer → first item is the seed.  Empty iterable
            // is a hard error (mirrors CPython's TypeError text).
            match iter.next() {
                Some(v) => v,
                None => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() of empty iterable with no initial value"),
                )),
            }
        };
        for item in iter {
            acc = _interp.call_function_expanded(
                func.clone(),
                &[
                    ExpandedCallArg { name: None, value: acc },
                    ExpandedCallArg { name: None, value: item },
                ],
            )?;
        }
        Ok(acc)
    }
}
