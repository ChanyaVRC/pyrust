/// Construct the runtime descriptor installed for a concrete `__slots__`
/// member. Class layout code owns the slot list; the type-object boundary owns
/// the native descriptor representation.
pub(crate) fn make_slot_member_descriptor(slot: &str, owner: &Rc<RefCell<PyClass>>) -> Value {
    pyrust_builtins::member_descriptor::member_descriptor(slot, owner)
}
