// User-function bytecode lookup shared by normal and cached call paths.

impl Interpreter {
    /// Return the compiled `FnCode` for `function`.
    /// Returns `None` only if `precompiled_code` is absent.
    pub(super) fn get_or_compile_bytecode(
        &mut self,
        function: &Rc<UserFunction>,
    ) -> Option<Rc<FnCode>> {
        function
            .precompiled_code
            .as_ref()
            .and_then(|rc| Rc::clone(rc).downcast::<FnCode>().ok())
    }
}
