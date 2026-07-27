use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: list([iterable]) — list constructor.
    /// <https://docs.python.org/3/library/functions.html#list>
    ///
    /// Not migrated to the typed-signature dialect in this batch: the
    /// macro's overload set requires all overloads to share the same arity,
    /// so the 0-arg / 1-arg split can't be expressed as two typed overloads.
    /// `Option<PyValue>` would conflate "absent" with "Python None",
    /// turning `list(None)` into `[]` instead of the correct `TypeError`.
    /// Remaining as `(args)` until the macro supports variable-arity
    /// overloads (tracked under #400).
    ///
    /// This can run user code by dispatching `__iter__` and
    /// `__next__` when consuming a user-defined iterable.
    fn list(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        match args.len() {
            0 => Ok(Value::list(vec![])),
            1 => Ok(Value::list(
                _interp.collect_sequence_constructor_iterable(&args[0].value)?,
            )),
            _ => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at most 1 argument, got {}", args.len()),
            )),
        }
    }

    /// CPython: tuple([iterable]) — tuple constructor.
    /// <https://docs.python.org/3/library/functions.html#tuple>
    ///
    /// Not migrated in this batch — same arity-split constraint as `list`.
    ///
    /// This can run user code by dispatching `__iter__` and
    /// `__next__` when consuming a user-defined iterable.
    fn tuple(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        match args.len() {
            0 => Ok(Value::tuple(vec![])),
            1 => Ok(Value::tuple(
                _interp.collect_sequence_constructor_iterable(&args[0].value)?,
            )),
            _ => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at most 1 argument, got {}", args.len()),
            )),
        }
    }

}
