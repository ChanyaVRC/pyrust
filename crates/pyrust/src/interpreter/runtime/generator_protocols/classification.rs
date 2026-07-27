/// True when `value` is a coroutine frame created by `async def`.
pub(crate) fn is_coroutine_value(value: &Value) -> bool {
    if let ValueKind::Generator(state) = value.kind()
        && let Ok(state) = state.try_borrow()
        && let Some(frame) = state.downcast_ref::<GeneratorFrame>()
    {
        return frame.is_coroutine;
    }
    false
}

/// True when `value` is an async generator (`async def` containing `yield`).
pub(crate) fn is_async_generator_value(value: &Value) -> bool {
    if let ValueKind::Generator(state) = value.kind()
        && let Ok(state) = state.try_borrow()
        && let Some(frame) = state.downcast_ref::<GeneratorFrame>()
    {
        return frame.is_async_generator();
    }
    false
}
