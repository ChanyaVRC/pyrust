// Python structural-pattern protocols.
//
// The VM emits a MatchClassPositional instruction, while this boundary owns
// `__match_args__`, match-self builtin classification, and Python diagnostics.

impl Interpreter {
    /// Fetch the attribute values a class pattern's sub-patterns match against:
    /// `count` positional ones (via `__match_args__`) followed by the keyword
    /// attributes `kw_names`.
    ///
    /// Like CPython's `MATCH_CLASS`, an attribute that is missing
    /// (`AttributeError`, from a builtin, native or user type alike) makes the
    /// whole class pattern fail to match (`Ok(None)`); any other exception
    /// raised while looking an attribute up propagates.
    pub(crate) fn match_class_attribute_values(
        &mut self,
        subject: &Value,
        class_value: &Value,
        count: usize,
        kw_names: &[String],
    ) -> Result<Option<Vec<Value>>> {
        let mut values = Vec::with_capacity(count + kw_names.len());
        if count > 0 {
            let Some(positional) =
                self.match_class_positional_values(subject, class_value, count)?
            else {
                return Ok(None);
            };
            values = positional;
        }
        for name in kw_names {
            match self.get_attr(subject, name) {
                Ok(value) => values.push(value),
                Err(error) if error.class_name_is("AttributeError") => return Ok(None),
                Err(error) => return Err(error),
            }
        }
        Ok(Some(values))
    }

    fn match_class_positional_values(
        &mut self,
        subject: &Value,
        class_value: &Value,
        count: usize,
    ) -> Result<Option<Vec<Value>>> {
        let class_name = match class_value.kind() {
            ValueKind::PyClass(class) => class.borrow().name.clone(),
            _ => "<class>".to_string(),
        };

        let match_args = match self.get_attr(class_value, "__match_args__") {
            Ok(value) => value,
            Err(error) if error.class_name_is("AttributeError") => {
                if Self::class_matches_itself(class_value) {
                    if count > 1 {
                        return Err(pyrust_core::type_err!(
                            "{class_name}() accepts 1 positional sub-pattern ({count} given)"
                        ));
                    }
                    return Ok(Some(if count == 1 {
                        vec![subject.clone()]
                    } else {
                        Vec::new()
                    }));
                }
                return Err(pyrust_core::type_err!(
                    "{class_name}() accepts 0 positional sub-patterns ({count} given)"
                ));
            }
            Err(error) => return Err(error),
        };

        let match_args_len = match match_args.as_tuple() {
            Some(items) => items.len(),
            None => {
                let actual = value_type_name_str(&match_args);
                return Err(pyrust_core::type_err!(
                    "{class_name}.__match_args__ must be a tuple (got {actual})"
                ));
            }
        };
        if match_args_len < count {
            let plural = if match_args_len == 1 { "" } else { "s" };
            return Err(pyrust_core::type_err!(
                "{class_name}() accepts {match_args_len} positional sub-pattern{plural} ({count} given)"
            ));
        }

        let mut values = Vec::with_capacity(count);
        for index in 0..count {
            let attribute = {
                let items = match_args
                    .as_tuple()
                    .expect("validated tuple remains alive for this call");
                let name = &items[index];
                name.as_str().map(str::to_owned).ok_or_else(|| {
                    let actual = value_type_name_str(name);
                    pyrust_core::type_err!("__match_args__ elements must be strings (got {actual})")
                })?
            };
            match self.get_attr(subject, &attribute) {
                Ok(value) => values.push(value),
                Err(error) if error.class_name_is("AttributeError") => return Ok(None),
                Err(error) => return Err(error),
            }
        }
        Ok(Some(values))
    }

    fn class_matches_itself(class_value: &Value) -> bool {
        const MATCH_SELF_TYPES: &[&str] = &[
            "bool",
            "bytearray",
            "bytes",
            "dict",
            "float",
            "frozenset",
            "int",
            "list",
            "set",
            "str",
            "tuple",
        ];

        let ValueKind::PyClass(class) = class_value.kind() else {
            return false;
        };
        MATCH_SELF_TYPES.iter().any(|name| {
            crate::interpreter::primitive_class_by_name(name)
                .is_some_and(|primitive| class_is_subclass_of(class, &primitive))
        })
    }
}
