/// Return the concrete Python type name for a value.
///
/// Generator-tagged values share one storage representation, so their native
/// iterator/coroutine state must be inspected instead of relying on the coarse
/// core value tag.
pub(crate) fn full_type_name_str(value: &Value) -> std::borrow::Cow<'static, str> {
    if let ValueKind::Generator(cell) = value.kind() {
        // Frame kinds come from the immutable tag, which is readable even
        // while the body executes and the state cell is checked out (#2978).
        if let Some(name) = cell.kind().frame_type_name() {
            return std::borrow::Cow::Borrowed(name);
        }
        let state = cell.borrow();
        if state.downcast_ref::<MapIter>().is_some() {
            return std::borrow::Cow::Borrowed("map");
        }
        if state.downcast_ref::<FilterIter>().is_some() {
            return std::borrow::Cow::Borrowed("filter");
        }
        if let Some(iterator) = state.downcast_ref::<ProviderIterator>() {
            return std::borrow::Cow::Borrowed(iterator.full_type_name());
        }
        if state.downcast_ref::<EnumerateIter>().is_some() {
            return std::borrow::Cow::Borrowed("enumerate");
        }
        if state.downcast_ref::<ZipIter>().is_some() {
            return std::borrow::Cow::Borrowed("zip");
        }
        if state.downcast_ref::<CallableIter>().is_some() {
            return std::borrow::Cow::Borrowed("callable_iterator");
        }
        if let Some(iterator) = state.downcast_ref::<GetItemIter>() {
            return std::borrow::Cow::Borrowed(if iterator.step < 0 {
                "reversed"
            } else {
                "iterator"
            });
        }
        if state.downcast_ref::<RangeIter>().is_some() {
            return std::borrow::Cow::Borrowed("range_iterator");
        }
        if state.downcast_ref::<BigRangeIter>().is_some() {
            return std::borrow::Cow::Borrowed("longrange_iterator");
        }
        if let Some(native) = state.downcast_ref::<NativeIterFrame>() {
            return std::borrow::Cow::Borrowed(native.type_name);
        }
        if state.downcast_ref::<AsyncGenASend>().is_some() {
            return std::borrow::Cow::Borrowed("async_generator_asend");
        }
    }
    value_type_name_str(value)
}
