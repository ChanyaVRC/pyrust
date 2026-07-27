/// Typed policy for one native field on a built-in exception family.
///
/// Python-visible class attributes remain authoritative: the attribute domain
/// applies this policy only after proving that no user class in the MRO
/// overrides the field. The policy owns the native default, assignment
/// validation, and deletion contract so those rules cannot drift across the
/// lookup/set/delete adapters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExceptionSlotPolicy {
    Args,
    Cause,
    Context,
    SuppressContext,
    Traceback,
    /// `UnicodeError.start` / `.end`: accept only an actual integer (including
    /// bool/int subclasses), normalize to an exact index-sized int, and reject
    /// deletion.
    UnicodeIndex,
    /// `BlockingIOError.characters_written`: use the full `__index__`
    /// protocol, normalize to an exact index-sized int, and allow deletion
    /// only while the optional field is populated.
    CharactersWritten,
    Nullable,
    ReadOnly,
}

fn normalize_characters_written_index(original: &Value, normalized: &Value) -> Result<Value> {
    let integer = value_to_bigint(normalized)
        .and_then(|integer| integer.to_i64())
        .ok_or_else(|| {
            PyError::named(
                "ValueError",
                format!(
                    "cannot fit '{}' into an index-sized integer",
                    value_type_name_str(original)
                ),
            )
        })?;
    Ok(Value::int(integer))
}

/// CPython gives exact `BlockingIOError` a constructor-only third-argument
/// discriminator. Values with an integer conversion slot are treated as a
/// character count and must satisfy `__index__`; ordinary non-numeric objects
/// retain the inherited `OSError.filename` meaning.
///
/// `float`/`complex` and objects defining only `__int__`/`__float__` enter the
/// numeric branch but still fail the index protocol. This distinction matters:
/// silently treating such a value as a filename differs from CPython.
fn blocking_io_numeric_candidate(value: &Value) -> bool {
    if matches!(value.kind(), ValueKind::Float(_) | ValueKind::Complex(_, _)) {
        return true;
    }
    if let Some(backing) = builtin_data_backing(value)
        && matches!(
            backing.kind(),
            ValueKind::Int(_)
                | ValueKind::Bool(_)
                | ValueKind::BigInt(_)
                | ValueKind::Float(_)
                | ValueKind::Complex(_, _)
        )
    {
        return true;
    }
    lookup_value_special_method(value, "__int__").is_some()
        || lookup_value_special_method(value, "__float__").is_some()
}

fn prepare_blocking_io_constructor_count(
    interpreter: &mut Interpreter,
    value: &Value,
) -> Result<Option<Value>> {
    let Some(normalized) = interpreter.try_value_to_index(value)? else {
        if blocking_io_numeric_candidate(value) {
            return Err(pyrust_core::type_err!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(value)
            ));
        }
        return Ok(None);
    };
    normalize_characters_written_index(value, &normalized).map(Some)
}

/// Finish the exact built-in `BlockingIOError` constructor contract after the
/// generic `OSError` allocator has selected its final errno-specific class.
///
/// The exact-class check intentionally excludes user subclasses. It includes a
/// plain `OSError(EAGAIN/EALREADY/EINPROGRESS, ...)` that the allocator remapped
/// to canonical `BlockingIOError`. The original arguments remain the public
/// `args` tuple when the third value is a character count; only the normalized
/// index-sized integer is stored in the native field.
pub(super) fn finalize_blocking_io_constructor(
    interpreter: &mut Interpreter,
    exception: &Value,
    args: &[Value],
) -> Result<()> {
    if args.len() < 3 {
        return Ok(());
    }
    let ValueKind::PyInstance(instance) = exception.kind() else {
        return Ok(());
    };
    let class = Rc::clone(&instance.borrow().class);
    if class.borrow().builtin_exception_name != Some("BlockingIOError") {
        return Ok(());
    }

    let characters_written = prepare_blocking_io_constructor_count(interpreter, &args[2])?;
    let mut instance = instance.borrow_mut();
    if let Some(characters_written) = characters_written {
        instance
            .attrs
            .insert_slot("args", Value::tuple(args.to_vec()));
        instance.attrs.insert_slot("filename", Value::none());
        instance.attrs.insert_slot("filename2", Value::none());
        instance
            .attrs
            .insert_slot("characters_written", characters_written);
    } else if args[2].is_none() {
        // `filename=None` is the one inherited-filename case for which CPython
        // keeps the complete constructor tuple and ignores a fifth filename2.
        instance
            .attrs
            .insert_slot("args", Value::tuple(args.to_vec()));
        instance.attrs.insert_slot("filename2", Value::none());
        instance.attrs.shift_remove_slot("characters_written");
    }
    Ok(())
}

impl ExceptionSlotPolicy {
    /// Value exposed when the native slot has not been explicitly populated.
    pub(super) fn lookup_default(self, name: &str) -> Result<Value> {
        Ok(match self {
            Self::Args => Value::tuple(Vec::new()),
            Self::SuppressContext => Value::bool_(false),
            Self::CharactersWritten => {
                return Err(PyError::named("AttributeError", name.to_string()));
            }
            Self::Cause
            | Self::Context
            | Self::Traceback
            | Self::UnicodeIndex
            | Self::Nullable
            | Self::ReadOnly => Value::none(),
        })
    }

    /// Deferred traceback tokens require interpreter-aware materialization on
    /// first read; every other slot value can be returned directly.
    pub(super) const fn materializes_deferred_traceback(self) -> bool {
        matches!(self, Self::Traceback)
    }

    /// Normalize one assignment according to the native exception field's
    /// storage contract.
    ///
    /// This deliberately returns the value to store: several C-level setters
    /// do more than validation (`args` materializes a tuple, integer fields
    /// normalize subclasses / `__index__` results to an exact int).
    pub(super) fn prepare_assignment(
        self,
        interpreter: &mut Interpreter,
        value: Value,
    ) -> Result<Value> {
        match self {
            Self::Args => {
                if matches!(value.kind(), ValueKind::Tuple(_)) {
                    // `BaseException.args = exact_tuple` preserves identity.
                    return Ok(value);
                }
                return interpreter.collect_iterable(&value).map(Value::tuple);
            }
            Self::Cause | Self::Context => {
                let valid = match value.kind() {
                    ValueKind::None => true,
                    ValueKind::PyInstance(instance) => is_exception_class(&instance.borrow().class),
                    _ => false,
                };
                if !valid {
                    let label = if self == Self::Cause {
                        "cause"
                    } else {
                        "context"
                    };
                    return Err(pyrust_core::type_err!(
                        "exception {label} must be None or derive from BaseException"
                    ));
                }
            }
            Self::SuppressContext if !matches!(value.kind(), ValueKind::Bool(_)) => {
                return Err(pyrust_core::type_err!("attribute value type must be bool"));
            }
            Self::Traceback
                if !value.is_none() && !pyrust_builtins::traceback::is_traceback(&value) =>
            {
                return Err(pyrust_core::type_err!(
                    "__traceback__ must be a traceback or None"
                ));
            }
            Self::UnicodeIndex => {
                // Unlike the general index protocol, these UnicodeError
                // members use PyLong_AsSsize_t semantics: an actual integer
                // (including bool/int subclasses) is required, and an
                // arbitrary object with __index__ is rejected.
                let normalized = if matches!(
                    value.kind(),
                    ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                ) {
                    value
                } else {
                    match value.kind() {
                        ValueKind::PyInstance(instance) => {
                            let class = Rc::clone(&instance.borrow().class);
                            let int_class =
                                canonical_class_by_tag(pyrust_core::CanonicalClassTag::Int);
                            if !class_is_subclass_of(&class, &int_class) {
                                return Err(pyrust_core::type_err!("an integer is required"));
                            }
                            builtin_data_backing(&value)
                                .filter(|backing| {
                                    matches!(
                                        backing.kind(),
                                        ValueKind::Int(_)
                                            | ValueKind::Bool(_)
                                            | ValueKind::BigInt(_)
                                    )
                                })
                                .ok_or_else(|| pyrust_core::type_err!("an integer is required"))?
                        }
                        _ => return Err(pyrust_core::type_err!("an integer is required")),
                    }
                };
                let integer = value_to_bigint(&normalized)
                    .and_then(|integer| integer.to_i64())
                    .ok_or_else(|| {
                        pyrust_core::overflow_err!("Python int too large to convert to C ssize_t")
                    })?;
                return Ok(Value::int(integer));
            }
            Self::CharactersWritten => {
                let normalized = interpreter.value_to_index(&value, |value| {
                    pyrust_core::type_err!(
                        "'{}' object cannot be interpreted as an integer",
                        pyrust_core::builtin_type_name(value)
                    )
                })?;
                return normalize_characters_written_index(&value, &normalized);
            }
            Self::ReadOnly => {
                return Err(pyrust_core::py_err!("AttributeError", "readonly attribute"));
            }
            Self::SuppressContext | Self::Traceback | Self::Nullable => {}
        }
        Ok(value)
    }

    /// Apply CPython's deletion contract and clear nullable native storage.
    pub(super) fn delete(self, instance: &Rc<RefCell<PyInstance>>, name: &str) -> Result<()> {
        match self {
            Self::Args | Self::Cause | Self::Context | Self::Traceback => {
                Err(pyrust_core::type_err!("{name} may not be deleted"))
            }
            Self::SuppressContext | Self::UnicodeIndex => Err(pyrust_core::type_err!(
                "can't delete numeric/char attribute"
            )),
            Self::CharactersWritten => {
                if instance
                    .borrow_mut()
                    .attrs
                    .shift_remove_slot(name)
                    .is_some()
                {
                    Ok(())
                } else {
                    Err(PyError::named("AttributeError", name.to_string()))
                }
            }
            Self::Nullable => {
                // Pointer-style structured fields become NULL on the C object.
                // Their getters present that state as None, and repeated
                // deletion succeeds.
                instance.borrow_mut().attrs.shift_remove_slot(name);
                Ok(())
            }
            Self::ReadOnly => Err(pyrust_core::py_err!("AttributeError", "readonly attribute")),
        }
    }
}

/// Resolve the immutable built-in exception identity and native field policy.
///
/// Presentation names are deliberately ignored: a renamed canonical exception
/// keeps its slots, while a user class merely named `OSError` gains none.
pub(super) fn exception_slot_policy(
    class: &Rc<RefCell<PyClass>>,
    name: &str,
) -> Option<ExceptionSlotPolicy> {
    let is = |built_in| class_is_builtin_exception_subclass(class, built_in);

    if is("BaseException") {
        let policy = match name {
            "args" => ExceptionSlotPolicy::Args,
            "__cause__" => ExceptionSlotPolicy::Cause,
            "__context__" => ExceptionSlotPolicy::Context,
            "__suppress_context__" => ExceptionSlotPolicy::SuppressContext,
            "__traceback__" => ExceptionSlotPolicy::Traceback,
            _ => {
                if is("StopIteration") && name == "value"
                    || is("SystemExit") && name == "code"
                    || is("SyntaxError")
                        && matches!(
                            name,
                            "msg"
                                | "filename"
                                | "lineno"
                                | "offset"
                                | "text"
                                | "end_lineno"
                                | "end_offset"
                                | "print_file_and_line"
                        )
                    || is("OSError")
                        && (matches!(name, "errno" | "strerror" | "filename" | "filename2")
                            || cfg!(windows) && name == "winerror")
                    || is("ImportError") && matches!(name, "msg" | "name" | "path")
                    || is("NameError") && name == "name"
                    || is("AttributeError") && matches!(name, "name" | "obj")
                    || (is("UnicodeDecodeError")
                        || is("UnicodeEncodeError")
                        || is("UnicodeTranslateError"))
                        && matches!(name, "encoding" | "object" | "reason")
                {
                    ExceptionSlotPolicy::Nullable
                } else if is("BaseExceptionGroup") && matches!(name, "message" | "exceptions") {
                    ExceptionSlotPolicy::ReadOnly
                } else if is("BlockingIOError") && name == "characters_written" {
                    ExceptionSlotPolicy::CharactersWritten
                } else if (is("UnicodeDecodeError")
                    || is("UnicodeEncodeError")
                    || is("UnicodeTranslateError"))
                    && matches!(name, "start" | "end")
                {
                    ExceptionSlotPolicy::UnicodeIndex
                } else {
                    return None;
                }
            }
        };
        return Some(policy);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        ExceptionSlotPolicy, PyClass, PyInstance, Rc, RefCell, Value, ValueKind,
        exception_slot_policy,
    };
    use crate::Interpreter;
    use crate::value::InstanceAttrs;

    fn exception_class(
        name: &'static str,
        base: Option<Rc<RefCell<PyClass>>>,
    ) -> Rc<RefCell<PyClass>> {
        Rc::new(RefCell::new(PyClass {
            name: name.to_string(),
            qualname: name.to_string(),
            base,
            builtin_exception_name: Some(name),
            ..PyClass::default()
        }))
    }

    #[test]
    fn slot_schema_uses_immutable_exception_identity() {
        let base = exception_class("BaseException", None);
        let os_error = exception_class("OSError", Some(Rc::clone(&base)));
        let blocking = exception_class("BlockingIOError", Some(Rc::clone(&os_error)));
        let syntax = exception_class("SyntaxError", Some(Rc::clone(&base)));
        let unicode = exception_class("UnicodeDecodeError", Some(Rc::clone(&base)));

        assert_eq!(
            exception_slot_policy(&base, "args"),
            Some(ExceptionSlotPolicy::Args)
        );
        assert_eq!(
            exception_slot_policy(&syntax, "end_offset"),
            Some(ExceptionSlotPolicy::Nullable)
        );
        assert_eq!(
            exception_slot_policy(&os_error, "errno"),
            Some(ExceptionSlotPolicy::Nullable)
        );
        assert_eq!(
            exception_slot_policy(&blocking, "characters_written"),
            Some(ExceptionSlotPolicy::CharactersWritten)
        );
        assert_eq!(
            exception_slot_policy(&unicode, "start"),
            Some(ExceptionSlotPolicy::UnicodeIndex)
        );
        assert_eq!(exception_slot_policy(&syntax, "not_a_slot"), None);

        os_error.borrow_mut().name = "renamed".to_string();
        assert_eq!(
            exception_slot_policy(&os_error, "errno"),
            Some(ExceptionSlotPolicy::Nullable)
        );
        let impostor = Rc::new(RefCell::new(PyClass {
            name: "OSError".to_string(),
            qualname: "OSError".to_string(),
            ..PyClass::default()
        }));
        assert_eq!(exception_slot_policy(&impostor, "errno"), None);
    }

    #[test]
    fn slot_policy_centralizes_defaults_validation_and_deletion() {
        let class = exception_class("BaseException", None);
        let instance = Rc::new(RefCell::new(PyInstance {
            class,
            attrs: InstanceAttrs::new(),
        }));
        instance
            .borrow_mut()
            .attrs
            .insert_slot("value", Value::int(7));

        assert!(matches!(
            ExceptionSlotPolicy::Args
                .lookup_default("args")
                .unwrap()
                .kind(),
            ValueKind::Tuple(_)
        ));
        assert!(matches!(
            ExceptionSlotPolicy::SuppressContext
                .lookup_default("__suppress_context__")
                .unwrap()
                .kind(),
            ValueKind::Bool(false)
        ));
        assert!(
            ExceptionSlotPolicy::SuppressContext
                .prepare_assignment(&mut Interpreter::default(), Value::string("invalid"))
                .is_err()
        );

        let exact_tuple = Value::tuple(vec![Value::int(1), Value::int(2)]);
        let prepared_tuple = ExceptionSlotPolicy::Args
            .prepare_assignment(&mut Interpreter::default(), exact_tuple.clone())
            .unwrap();
        assert_eq!(
            prepared_tuple.value_id(),
            exact_tuple.value_id(),
            "an exact tuple assigned to args must preserve identity"
        );
        let prepared_list = ExceptionSlotPolicy::Args
            .prepare_assignment(
                &mut Interpreter::default(),
                Value::list(vec![Value::int(1), Value::int(2)]),
            )
            .unwrap();
        assert!(matches!(prepared_list.kind(), ValueKind::Tuple(_)));

        let prepared_character_count = ExceptionSlotPolicy::CharactersWritten
            .prepare_assignment(&mut Interpreter::default(), Value::bool_(true))
            .unwrap();
        assert!(matches!(prepared_character_count.kind(), ValueKind::Int(1)));
        assert!(
            ExceptionSlotPolicy::UnicodeIndex
                .prepare_assignment(&mut Interpreter::default(), Value::string("invalid"))
                .is_err()
        );
        assert!(
            ExceptionSlotPolicy::ReadOnly
                .prepare_assignment(&mut Interpreter::default(), Value::int(1))
                .is_err()
        );
        assert!(
            ExceptionSlotPolicy::CharactersWritten
                .lookup_default("characters_written")
                .is_err()
        );

        assert!(ExceptionSlotPolicy::Args.delete(&instance, "args").is_err());
        assert!(
            ExceptionSlotPolicy::UnicodeIndex
                .delete(&instance, "start")
                .is_err()
        );
        assert!(
            ExceptionSlotPolicy::ReadOnly
                .delete(&instance, "message")
                .is_err()
        );
        assert!(
            ExceptionSlotPolicy::CharactersWritten
                .delete(&instance, "characters_written")
                .is_err()
        );
        instance
            .borrow_mut()
            .attrs
            .insert_slot("characters_written", Value::int(3));
        ExceptionSlotPolicy::CharactersWritten
            .delete(&instance, "characters_written")
            .unwrap();
        ExceptionSlotPolicy::Nullable
            .delete(&instance, "value")
            .unwrap();
        assert!(instance.borrow().attrs.get_slot("value").is_none());
    }
}
