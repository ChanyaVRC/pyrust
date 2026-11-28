// Access to process-visible Python standard streams is a runtime service.
// Builtins such as `print` and `contextlib` use this boundary instead of
// reaching through the generic call implementation.

impl Interpreter {
    /// `input([prompt])` on redirected standard streams, CPython's
    /// non-console fallback: the prompt is written with `sys.stdout.write`
    /// (then `sys.stdout.flush()`, whose errors are ignored) and the line is
    /// read with `sys.stdin.readline()`; an empty line raises `EOFError` and a
    /// trailing newline is stripped.
    pub(crate) fn input_via_streams(&mut self, prompt: Option<&Value>) -> Result<Value> {
        let call = |interp: &mut Interpreter, callee: Value, args: &[Value]| {
            let expanded: Vec<crate::interpreter::ExpandedCallArg> = args
                .iter()
                .cloned()
                .map(|value| crate::interpreter::ExpandedCallArg { name: None, value })
                .collect();
            interp.call_function_expanded(callee, &expanded)
        };
        let fin = self.current_std_stream("stdin")?;
        if fin.is_none() {
            return Err(PyError::named(
                "RuntimeError",
                "input(): lost sys.stdin".to_string(),
            ));
        }
        let fout = self.current_std_stream("stdout")?;
        if fout.is_none() {
            return Err(PyError::named(
                "RuntimeError",
                "input(): lost sys.stdout".to_string(),
            ));
        }
        if let Some(prompt) = prompt {
            let text = call(
                self,
                Value::builtin_function("str"),
                std::slice::from_ref(prompt),
            )?;
            let write = self.get_attr(&fout, "write")?;
            call(self, write, &[text])?;
        }
        if let Ok(flush) = self.get_attr(&fout, "flush") {
            // CPython clears an error from this flush and goes on to read.
            let _ = call(self, flush, &[]);
        }
        let readline = self.get_attr(&fin, "readline")?;
        let line = call(self, readline, &[])?;
        let Some(text) = line.as_str() else {
            return Err(PyError::named(
                "TypeError",
                "object.readline() returned non-string".to_string(),
            ));
        };
        if text.is_empty() {
            return Err(PyError::named(
                "EOFError",
                "EOF when reading a line".to_string(),
            ));
        }
        Ok(Value::string(
            text.strip_suffix(char::from(10u8)).unwrap_or(text),
        ))
    }

    /// Return a reassigned `sys.stdout`/`sys.stderr`, or `None` when the stream
    /// still uses the native console fast path.
    pub(crate) fn redirected_std_stream(&self, name: &str) -> Option<Value> {
        let module_val = self.cold.module_cache.borrow().get("sys").cloned()?;
        let ValueKind::PyModule(module) = module_val.kind() else {
            return None;
        };
        let stream = module.borrow().get_attr_value(name)?;
        if pyrust_builtins::file::default_stdio_kind(&stream).is_some() {
            return None;
        }
        Some(stream)
    }

    /// Read `sys.<name>`, importing `sys` on demand.
    pub(crate) fn current_std_stream(&mut self, name: &str) -> Result<Value> {
        let module_val = self.load_module("sys")?;
        let ValueKind::PyModule(module) = module_val.kind() else {
            return Err(PyError::runtime(
                "internal: sys is not a module".to_string(),
            ));
        };
        let stream = module.borrow().get_attr_value(name);
        Ok(stream.unwrap_or_else(Value::none))
    }

    /// Assign `sys.<name>`, importing `sys` on demand.
    pub(crate) fn set_std_stream(&mut self, name: &str, value: Value) -> Result<()> {
        let module_val = self.load_module("sys")?;
        let ValueKind::PyModule(module) = module_val.kind() else {
            return Err(PyError::runtime(
                "internal: sys is not a module".to_string(),
            ));
        };
        module.borrow_mut().insert_attr(name.to_string(), value);
        Ok(())
    }
}
