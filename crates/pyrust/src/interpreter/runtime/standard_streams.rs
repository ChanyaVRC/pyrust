// Access to process-visible Python standard streams is a runtime service.
// Builtins such as `print` and `contextlib` use this boundary instead of
// reaching through the generic call implementation.

impl Interpreter {
    /// Return a reassigned `sys.stdout`/`sys.stderr`, or `None` when the stream
    /// still uses the native console fast path.
    pub(crate) fn redirected_std_stream(&self, name: &str) -> Option<Value> {
        let module_val = self.module_cache.borrow().get("sys").cloned()?;
        let ValueKind::PyModule(module) = module_val.kind() else {
            return None;
        };
        let stream = module.borrow().attrs.get(name).cloned()?;
        if pyrust_builtins::file::default_stdio_kind(&stream).is_some() {
            return None;
        }
        Some(stream)
    }

    /// Read `sys.<name>`, importing `sys` on demand.
    pub(crate) fn current_std_stream(&mut self, name: &str) -> Result<Value> {
        let module_val = self.load_module("sys")?;
        let ValueKind::PyModule(module) = module_val.kind() else {
            return Err(PyError::Runtime(
                "internal: sys is not a module".to_string(),
            ));
        };
        let stream = module.borrow().attrs.get(name).cloned();
        Ok(stream.unwrap_or_else(Value::none))
    }

    /// Assign `sys.<name>`, importing `sys` on demand.
    pub(crate) fn set_std_stream(&mut self, name: &str, value: Value) -> Result<()> {
        let module_val = self.load_module("sys")?;
        let ValueKind::PyModule(module) = module_val.kind() else {
            return Err(PyError::Runtime(
                "internal: sys is not a module".to_string(),
            ));
        };
        module.borrow_mut().insert_attr(name.to_string(), value);
        Ok(())
    }
}
