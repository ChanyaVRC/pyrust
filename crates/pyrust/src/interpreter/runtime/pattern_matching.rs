// Python structural-pattern protocols.
//
// The VM emits a MatchClassPositional instruction, while this boundary owns
// `__match_args__`, match-self builtin classification, and Python diagnostics.

impl Interpreter {
    pub(crate) fn match_class_positional_values(
        &mut self,
        subject: &Value,
        class_value: &Value,
        count: usize,
    ) -> Result<Vec<Value>> {
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
                    return Ok(if count == 1 {
                        vec![subject.clone()]
                    } else {
                        Vec::new()
                    });
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
            values.push(self.get_attr(subject, &attribute)?);
        }
        Ok(values)
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
