impl Interpreter {
    /// Parse the interpreter-aware `print()` options. The concrete keyword
    /// names live with the builtin adapter, not with generic user-function
    /// binding.
    pub(crate) fn parse_print_options_expanded(
        &mut self,
        args: &[ExpandedCallArg],
    ) -> Result<PrintOptions> {
        for arg in args {
            if let Some(name) = arg.name.as_deref()
                && !matches!(name, "sep" | "end" | "file" | "flush")
            {
                return Err(pyrust_core::type_err!(
                    "'{}' is an invalid keyword argument for print()",
                    name
                ));
            }
        }

        let mut values = Vec::new();
        let mut sep = String::from(" ");
        let mut end = String::from("\n");
        let mut file = None;
        let mut flush = false;

        for arg in args {
            let value = arg.value.clone();
            match arg.name.as_deref() {
                None => values.push(value),
                Some("sep") => {
                    sep = extract_optional_string(value, "sep")?.unwrap_or_else(|| " ".to_string());
                }
                Some("end") => {
                    end =
                        extract_optional_string(value, "end")?.unwrap_or_else(|| "\n".to_string());
                }
                Some("file") => {
                    if !value.is_none() {
                        file = Some(value);
                    }
                }
                Some("flush") => flush = self.truthy_value(&value)?,
                Some(_) => unreachable!("unknown print keyword was rejected above"),
            }
        }

        Ok(PrintOptions {
            values,
            sep,
            end,
            file,
            flush,
        })
    }
}
