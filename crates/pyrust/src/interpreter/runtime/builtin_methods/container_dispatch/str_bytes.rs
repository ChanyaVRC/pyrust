// Built-in str and bytes container method dispatch + str templating methods.

impl Interpreter {
    fn dispatch_str_container_method(
        &mut self,
        receiver: Value,
        method: &str,
        pos: smallvec::SmallVec<[Value; 4]>,
        kw: &PyDict,
    ) -> Result<Value> {
        if let Some(template_method) = pyrust_builtins::string::interpreter_method(method) {
            return self.str_template_method(&receiver, template_method, pos, kw);
        }
        if kw.is_empty() {
            self.call_str_method(method, receiver, pos)
        } else if let Some(binder) = pyrust_builtins::string::keyword_binder(method) {
            let mut pos = pos.into_vec();
            str_merge_kwargs(binder, &mut pos, kw.clone())?;
            self.call_str_method(method, receiver, pos.into())
        } else {
            pyrust_builtins::string::validate_method_keywords(method, true)?;
            self.call_str_method(method, receiver, pos)
        }
    }

    fn dispatch_bytes_container_method(
        &mut self,
        receiver: Value,
        method: &str,
        pos: smallvec::SmallVec<[Value; 4]>,
        kw: &PyDict,
    ) -> Result<Value> {
        pyrust_builtins::bytes::validate_method_keywords(method, !kw.is_empty())?;
        pyrust_builtins::bytes::validate_method_positional_arity(method, pos.len())?;
        if method == "join" {
            self.call_bytes_join(receiver, pos.into_vec())
        } else {
            self.call_bytes_method_with_protocols_prevalidated(
                method,
                &receiver,
                pos.into_vec(),
                kw,
            )
        }
    }

    /// `str.format` / `str.format_map` / `str.maketrans` — the templating
    /// methods that must run in the interpreter rather than
    /// `pyrust_builtins::string::call`.  Both opcodes share this (#431).
    fn str_template_method(
        &mut self,
        receiver: &Value,
        method: pyrust_builtins::string::InterpreterMethod,
        pos: smallvec::SmallVec<[Value; 4]>,
        kw: &PyDict,
    ) -> Result<Value> {
        use pyrust_builtins::string::InterpreterMethod;
        pyrust_builtins::string::validate_interpreter_method_keywords(method, !kw.is_empty())?;
        match method {
            InterpreterMethod::Format => {
                pyrust_builtins::string::validate_interpreter_method_positional_arity(
                    method,
                    pos.len(),
                )?;
                let mut keyword: Vec<(&str, Value)> = Vec::with_capacity(kw.len());
                for (k, v) in kw {
                    if let PyKey::Str(name) = k {
                        keyword.push((name.as_str().unwrap_or(""), v.clone()));
                    }
                }
                let template = receiver
                    .as_str()
                    .ok_or_else(|| PyError::runtime("internal: expected str".to_string()))?;
                self.format_str_template(template, &pos, &keyword)
            }
            InterpreterMethod::FormatMap => {
                pyrust_builtins::string::validate_interpreter_method_positional_arity(
                    method,
                    pos.len(),
                )?;
                if pos.len() != 1 {
                    return Err(pyrust_core::type_err!(
                        "str.format_map() takes exactly one argument ({} given)",
                        pos.len()
                    ));
                }
                let mapping = pos.into_iter().next().unwrap();
                let template = receiver
                    .as_str()
                    .ok_or_else(|| PyError::runtime("internal: expected str".to_string()))?
                    .to_string();
                self.format_str_template_map(&template, mapping)
            }
            // `maketrans` is a staticmethod on str: the receiver is discarded
            // and the call is forwarded to str_maketrans exactly like
            // `str.maketrans(...)` would be.
            InterpreterMethod::MakeTrans => {
                pyrust_builtins::string::validate_interpreter_method_positional_arity(
                    method,
                    pos.len(),
                )?;
                pyrust_builtins::string::str_maketrans(&pos)
            }
        }
    }
}
