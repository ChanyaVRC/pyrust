use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: list([iterable]) — list constructor.
    /// <https://docs.python.org/3/library/functions.html#list>
    ///
    /// Migrated to the typed-signature dialect (#400): `OptValueRef` keeps
    /// omitted `list()` (`[]`) distinct from `list(None)` (`TypeError`), and
    /// `#[arity_style(expected_got)]` reproduces CPython's wording while
    /// enabling the vectorcall fast entry.
    ///
    /// This can run user code by dispatching `__iter__` and
    /// `__next__` when consuming a user-defined iterable.
    #[arity_style(expected_got)]
    fn list(
        #[positional_only] #[default(OptionalArg::Omitted)] iterable: OptValueRef,
    ) -> Result<Value> {
        match iterable.as_value() {
            None => Ok(Value::list(vec![])),
            Some(val) => Ok(Value::list(
                _interp.collect_sequence_constructor_iterable(val)?,
            )),
        }
    }

    /// CPython: tuple([iterable]) — tuple constructor.
    /// <https://docs.python.org/3/library/functions.html#tuple>
    ///
    /// Migrated to the typed-signature dialect (#400) — same `OptValueRef`
    /// pattern as `list`.
    ///
    /// This can run user code by dispatching `__iter__` and
    /// `__next__` when consuming a user-defined iterable.
    #[arity_style(expected_got)]
    fn tuple(
        #[positional_only] #[default(OptionalArg::Omitted)] iterable: OptValueRef,
    ) -> Result<Value> {
        match iterable.as_value() {
            None => Ok(Value::tuple(vec![])),
            Some(val) => Ok(Value::tuple(
                _interp.collect_sequence_constructor_iterable(val)?,
            )),
        }
    }

}
