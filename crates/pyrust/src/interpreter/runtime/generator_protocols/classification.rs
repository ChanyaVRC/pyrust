/// True when `value` is a coroutine frame created by `async def` (an async
/// generator is one too).
///
/// Reads the immutable kind tag rather than the frame, so it stays correct
/// while the body is running and its state cell is checked out (#2978); the
/// previous `try_borrow` form silently answered `false` there.
pub(crate) fn is_coroutine_value(value: &Value) -> bool {
    matches!(
        value.kind(),
        ValueKind::Generator(cell)
            if matches!(
                cell.kind(),
                GeneratorKind::Coroutine | GeneratorKind::AsyncGenerator
            )
    )
}

/// True while a frame object's body is on the call stack — CPython's
/// `gi_running` / `cr_running` / `ag_running`.
///
/// A resume checks the state out of its cell in one of two ways: the native
/// path holds the cell mutably borrowed for the duration, and the gen-drive
/// trampoline (#2253) parks a [`GenDriving`] placeholder in it.  Both are the
/// same fact a re-entrant `next(g)` reports as "generator already executing"
/// (#2285).
pub(crate) fn generator_is_running(cell: &Rc<GeneratorCell>) -> bool {
    match cell.try_borrow() {
        Ok(state) => state.is::<GenDriving>(),
        Err(_) => true,
    }
}

/// True when `value` is an async generator (`async def` containing `yield`).
///
/// Reads the same immutable tag as [`is_coroutine_value`].
pub(crate) fn is_async_generator_value(value: &Value) -> bool {
    matches!(
        value.kind(),
        ValueKind::Generator(cell) if cell.kind() == GeneratorKind::AsyncGenerator
    )
}
