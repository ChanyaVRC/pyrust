// Exception coercion, chaining, matching, and PEP 654 group flow.

impl Interpreter {
    /// Materialise a raw runtime error variant into its Python exception value.
    ///
    /// Non-exception transport variants (`return`, `break`, and friends) remain
    /// errors so VM control flow can propagate them unchanged.
    pub(crate) fn materialize_pyerror(
        &mut self,
        error: PyError,
    ) -> std::result::Result<Value, PyError> {
        Ok(match error {
            PyError::Raised(value) => value,
            PyError::Runtime(message) => {
                if let Some(class) = self.exc_classes.get("RuntimeError") {
                    instantiate_exception(class, vec![Value::string(message)])
                } else {
                    self.instantiate_named_exception("RuntimeError", message)?
                }
            }
            PyError::Named(class, message) => self.instantiate_named_exception(&class, message)?,
            PyError::Class(class, message) => {
                let args = if message.is_empty() {
                    Vec::new()
                } else {
                    vec![Value::string(message)]
                };
                instantiate_exception(class, args)
            }
            PyError::KeyError(key) => {
                self.instantiate_named_exception_with_value("KeyError", key)?
            }
            PyError::NameError {
                class_name,
                message,
                name,
            } => self.instantiate_name_error_exception(class_name, message, name)?,
            PyError::AttributeError { message, name, obj } => {
                self.instantiate_attribute_error_exception(message, name, obj)?
            }
            PyError::ImportError {
                class_name,
                message,
                module_name,
            } => self.instantiate_import_error_exception(class_name, message, module_name)?,
            PyError::OsError {
                class_name,
                errno,
                strerror,
                filename,
                filename2,
            } => self
                .instantiate_os_error_exception(class_name, errno, strerror, filename, filename2)?,
            PyError::UnicodeDecodeError {
                encoding,
                object,
                start,
                end,
                reason,
            } => self
                .instantiate_unicode_decode_error_exception(encoding, object, start, end, reason)?,
            PyError::UnicodeEncodeError {
                encoding,
                object,
                start,
                end,
                reason,
            } => self
                .instantiate_unicode_encode_error_exception(encoding, object, start, end, reason)?,
            other => return Err(other),
        })
    }

    /// Attach implicit context before an uncaught error escapes a frame.
    pub(crate) fn escape_with_implicit_context(&mut self, error: PyError) -> PyError {
        if self.handled_exc_stack.is_empty() {
            return error;
        }
        let exception = match self.materialize_pyerror(error) {
            Ok(value) => value,
            Err(original) => return original,
        };
        self.attach_implicit_context(&exception);
        PyError::Raised(exception)
    }

    /// Update an exception's Python-visible traceback slot when a handler
    /// catches it. Handler selection and stack transfer remain VM concerns.
    pub(crate) fn record_caught_exception_traceback(
        &mut self,
        exception: &Value,
        catch_lineno: u32,
        is_bare_reraise: bool,
    ) {
        let ValueKind::PyInstance(instance) = exception.kind() else {
            return;
        };
        let mut instance = instance.borrow_mut();
        if let Some(slot) = instance.attrs.get_slot_mut("__traceback__") {
            if let Some(traceback) =
                self.caught_traceback_value(slot, catch_lineno as i64, is_bare_reraise)
            {
                *slot = traceback;
            }
        } else {
            let traceback = self.build_deferred_traceback(catch_lineno as i64);
            instance.attrs.insert_slot("__traceback__", traceback);
        }
    }

    /// Resolve and, when necessary, materialise an exception's deferred
    /// traceback for protocol consumers such as `with.__exit__`.
    pub(crate) fn exception_traceback_value(&mut self, exception: &Value) -> Value {
        let Some(instance) = exception.as_py_instance_rc() else {
            return Value::none();
        };
        let stored = instance.borrow().attrs.get_cloned_or_slot("__traceback__");
        match stored {
            Some(value) => match self.materialize_deferred_traceback(&value) {
                Some(traceback) => {
                    instance
                        .borrow_mut()
                        .attrs
                        .insert_slot("__traceback__", traceback.clone());
                    traceback
                }
                None => value,
            },
            None => Value::none(),
        }
    }

    /// Construct an AssertionError and attach the active implicit context.
    pub(crate) fn prepare_assertion_error(&self, message: Option<Value>) -> Result<Value> {
        let class = self
            .exc_classes
            .get("AssertionError")
            .or_else(|| lookup_exc_class("AssertionError"))
            .ok_or_else(|| {
                PyError::Runtime("built-in exception 'AssertionError' is not defined".to_string())
            })?;
        let exception = instantiate_exception(class, message.into_iter().collect());
        self.attach_implicit_context(&exception);
        Ok(exception)
    }

    /// Coerce and prepare an explicit `raise value`.
    pub(crate) fn prepare_explicit_raise(&mut self, value: Value) -> Result<Value> {
        let exception = self.coerce_to_exception(value)?;
        self.attach_implicit_context(&exception);
        self.reraise_is_bare = false;
        self.reset_captured_frames_if_reraise(&exception);
        Ok(exception)
    }

    /// Coerce and prepare `raise value from cause`.
    pub(crate) fn prepare_explicit_raise_from(
        &mut self,
        value: Value,
        cause: Value,
    ) -> Result<Value> {
        let exception = self.coerce_to_exception(value)?;
        let cause = self.coerce_to_exception_cause(cause)?;
        self.attach_implicit_context(&exception);
        if let ValueKind::PyInstance(instance) = exception.kind() {
            let mut instance = instance.borrow_mut();
            instance.attrs.insert_slot("__cause__", cause);
            instance
                .attrs
                .insert_slot("__suppress_context__", Value::bool_(true));
        }
        self.reraise_is_bare = false;
        self.reset_captured_frames_if_reraise(&exception);
        Ok(exception)
    }

    /// Prepare an unmatched `except*` remainder as a bare re-raise while
    /// preserving the original group's context.
    pub(crate) fn prepare_exception_group_residual(&mut self, residual: &Value) {
        let original_context = self.handled_exc_stack.last().and_then(|group| {
            let ValueKind::PyInstance(instance) = group.kind() else {
                return None;
            };
            instance.borrow().attrs.get_cloned_or_slot("__context__")
        });
        if let ValueKind::PyInstance(instance) = residual.kind() {
            let mut instance = instance.borrow_mut();
            instance
                .attrs
                .insert_slot("__context__", original_context.unwrap_or_else(Value::none));
            instance
                .attrs
                .insert_slot("__suppress_context__", Value::bool_(true));
        }
        self.reraise_is_bare = true;
        self.reset_captured_frames_if_reraise(residual);
    }

    /// Returns true when the two `Value`s wrap the same `PyInstance`
    /// (pointer-equal).  Used to detect when control is inside an
    /// active `except` handler body — i.e. when the interpreter's
    /// `active_exception` is the same instance as the top of
    /// `handled_exc_stack`.
    pub(crate) fn values_are_same_exception(a: &Value, b: &Value) -> bool {
        match (a.kind(), b.kind()) {
            (ValueKind::PyInstance(x), ValueKind::PyInstance(y)) => Rc::ptr_eq(x, y),
            _ => false,
        }
    }

    /// PEP 3134 implicit exception chaining: if a `raise` happens inside an
    /// active `except` handler, attach the currently-handled exception as
    /// the new exception's `__context__`.  Skipped if `__context__` is
    /// already set (e.g. via prior `raise X from Y`) or if the new
    /// exception IS the currently-handled one (a bare re-raise) — both
    /// cases would create a self-referential cycle.
    pub(crate) fn attach_implicit_context(&self, exc: &Value) {
        let Some(ctx) = self.handled_exc_stack.last() else {
            return;
        };
        let ValueKind::PyInstance(inst) = exc.kind() else {
            return;
        };
        // Avoid setting context to self (bare `raise` inside an except).
        if let ValueKind::PyInstance(ctx_inst) = ctx.kind()
            && Rc::ptr_eq(inst, ctx_inst)
        {
            return;
        }
        let mut borrow = inst.borrow_mut();
        // Don't clobber an existing __context__ (already attached on a
        // previous raise that propagated through here).
        if borrow.attrs.get_slot("__context__").is_some() {
            return;
        }
        borrow.attrs.insert_slot("__context__", ctx.clone());
    }

    pub(super) fn coerce_to_exception(&mut self, value: Value) -> Result<Value> {
        match value.kind() {
            ValueKind::PyInstance(instance) => {
                let instance = Rc::clone(instance);
                if is_exception_class(&instance.borrow().class) {
                    Ok(Value::py_instance(instance))
                } else {
                    Err(pyrust_core::type_err!(
                        "exceptions must derive from BaseException"
                    ))
                }
            }
            ValueKind::PyClass(class) => {
                let class = Rc::clone(class);
                if is_exception_class(&class) {
                    // Use call_class_expanded so that user-defined __init__ is
                    // invoked (e.g. `raise MyError` where MyError.__init__ has
                    // default args).  Mirrors CPython's do_raise behaviour.
                    self.call_class_expanded(class, &[])
                } else {
                    Err(pyrust_core::type_err!(
                        "exceptions must derive from BaseException"
                    ))
                }
            }
            _ => Err(pyrust_core::type_err!(
                "exceptions must derive from BaseException"
            )),
        }
    }

    /// Validate and coerce a `raise X from Y` cause value.
    ///
    /// CPython accepts `None` (clears cause) or any `BaseException` instance/
    /// subclass as cause.  A class is auto-instantiated with no args, matching
    /// CPython's `ceval.c::do_raise`.  Anything else raises
    /// `TypeError: exception causes must derive from BaseException`.
    pub(super) fn coerce_to_exception_cause(&mut self, value: Value) -> Result<Value> {
        if value.is_none() {
            return Ok(value);
        }
        match value.kind() {
            ValueKind::PyInstance(instance) => {
                let instance = Rc::clone(instance);
                if is_exception_class(&instance.borrow().class) {
                    Ok(Value::py_instance(instance))
                } else {
                    Err(pyrust_core::type_err!(
                        "exception causes must derive from BaseException"
                    ))
                }
            }
            ValueKind::PyClass(class) => {
                let class = Rc::clone(class);
                if is_exception_class(&class) {
                    // Use call_class_expanded so that user-defined __init__ is
                    // invoked when a class is used as a cause.
                    self.call_class_expanded(class, &[])
                } else {
                    Err(pyrust_core::type_err!(
                        "exception causes must derive from BaseException"
                    ))
                }
            }
            _ => Err(pyrust_core::type_err!(
                "exception causes must derive from BaseException"
            )),
        }
    }

    /// Build the canonical recursion-limit exception used by every execution
    /// strategy (native calls, the call trampoline, and generator driving).
    ///
    /// Keeping the Python exception-class name inside this domain prevents the
    /// VM and generic call router from depending on exception registration.
    #[inline]
    pub(super) fn recursion_limit_error(&self) -> Result<PyError> {
        const MESSAGE: &str = "maximum recursion depth exceeded";
        let exception = if let Some(class) = self.exc_classes.get("RecursionError") {
            instantiate_exception(class, vec![Value::string(MESSAGE)])
        } else {
            self.instantiate_named_exception("RecursionError", MESSAGE.to_string())?
        };
        Ok(PyError::Raised(exception))
    }

    /// Build the canonical completion exception for an exhausted generator.
    /// Generator control flow supplies only the Python-visible arguments and
    /// remains independent of the exception registry and object layout.
    #[inline]
    pub(super) fn generator_completion_error(&self, args: Vec<Value>) -> Result<PyError> {
        if let Some(class) = self.exc_classes.get("StopIteration") {
            Ok(PyError::Raised(instantiate_exception(class, args)))
        } else if args.is_empty() {
            Ok(pyrust_core::py_err!("StopIteration", String::new()))
        } else {
            Ok(pyrust_core::py_err!("StopIteration", args[0].to_py_str()))
        }
    }

    pub(super) fn instantiate_named_exception(&self, name: &str, message: String) -> Result<Value> {
        let class = lookup_exc_class(name).ok_or_else(|| {
            PyError::Runtime(format!("built-in exception '{}' is not defined", name))
        })?;
        let args = if message.is_empty() {
            vec![]
        } else {
            vec![Value::string(message)]
        };
        Ok(instantiate_exception(class, args))
    }

    /// Like [`instantiate_named_exception`] but stores a raw `Value` as
    /// `args[0]` instead of a `Value::string(message)`.  Used for `KeyError`
    /// so that `e.args[0]` returns the original key object, matching CPython.
    pub(super) fn instantiate_named_exception_with_value(
        &self,
        name: &str,
        arg: Value,
    ) -> Result<Value> {
        let class = lookup_exc_class(name).ok_or_else(|| {
            PyError::Runtime(format!("built-in exception '{}' is not defined", name))
        })?;
        Ok(instantiate_exception(class, vec![arg]))
    }

    /// Instantiate a `NameError` or `UnboundLocalError` with the CPython 3.12
    /// `.name` instance attribute set to the identifier that was not found.
    ///
    /// `class_name` must be `"NameError"` or `"UnboundLocalError"`.
    /// `name` is the identifier string (or `None` for `UnboundLocalError`).
    pub(super) fn instantiate_name_error_exception(
        &self,
        class_name: &str,
        message: String,
        name: Option<String>,
    ) -> Result<Value> {
        let class = lookup_exc_class(class_name).ok_or_else(|| {
            PyError::Runtime(format!("built-in exception '{class_name}' is not defined"))
        })?;
        Ok(instantiate_name_error(class, message, name))
    }

    /// Instantiate an `ImportError` or `ModuleNotFoundError` with the CPython
    /// 3.12 `.name` and `.path` instance attributes set.
    ///
    /// `class_name` must be `"ImportError"` or `"ModuleNotFoundError"`.
    pub(super) fn instantiate_import_error_exception(
        &self,
        class_name: &str,
        message: String,
        module_name: Option<String>,
    ) -> Result<Value> {
        let class = lookup_exc_class(class_name).ok_or_else(|| {
            PyError::Runtime(format!("built-in exception '{class_name}' is not defined"))
        })?;
        Ok(instantiate_import_error(class, message, module_name))
    }

    /// Instantiate an `AttributeError` with the CPython 3.12 `.name` and `.obj`
    /// instance attributes set to the missing attribute name and the receiver.
    pub(super) fn instantiate_attribute_error_exception(
        &self,
        message: String,
        name: Option<String>,
        obj: Option<Value>,
    ) -> Result<Value> {
        let class = lookup_exc_class("AttributeError").ok_or_else(|| {
            PyError::Runtime("built-in exception 'AttributeError' is not defined".to_string())
        })?;
        Ok(instantiate_attribute_error(class, message, name, obj))
    }

    /// Instantiate an `OSError` (or subclass) with `errno`, `strerror`, and
    /// `filename` instance attributes set, matching CPython 3.12's behaviour
    /// when raising OS errors from real filesystem operations.
    pub(super) fn instantiate_os_error_exception(
        &self,
        class_name: &str,
        errno: i64,
        strerror: String,
        filename: Option<String>,
        filename2: Option<String>,
    ) -> Result<Value> {
        let class = lookup_exc_class(class_name).ok_or_else(|| {
            PyError::Runtime(format!("built-in exception '{class_name}' is not defined"))
        })?;
        Ok(instantiate_os_error(
            class, errno, strerror, filename, filename2,
        ))
    }

    /// Instantiate a `UnicodeDecodeError` with all five structured attributes
    /// set from a `PyError::UnicodeDecodeError` variant raised internally (e.g.
    /// from `bytes.decode()`).
    pub(super) fn instantiate_unicode_decode_error_exception(
        &self,
        encoding: String,
        object: Vec<u8>,
        start: usize,
        end: usize,
        reason: String,
    ) -> Result<Value> {
        let class = lookup_exc_class("UnicodeDecodeError").ok_or_else(|| {
            PyError::Runtime("built-in exception 'UnicodeDecodeError' is not defined".to_string())
        })?;
        Ok(instantiate_unicode_decode_error(
            class, encoding, object, start, end, reason,
        ))
    }

    /// Instantiate a `UnicodeEncodeError` with all five structured attributes
    /// set from a `PyError::UnicodeEncodeError` variant raised internally (e.g.
    /// from `str.encode()`).
    pub(super) fn instantiate_unicode_encode_error_exception(
        &self,
        encoding: String,
        object: String,
        start: usize,
        end: usize,
        reason: String,
    ) -> Result<Value> {
        let class = lookup_exc_class("UnicodeEncodeError").ok_or_else(|| {
            PyError::Runtime("built-in exception 'UnicodeEncodeError' is not defined".to_string())
        })?;
        Ok(instantiate_unicode_encode_error(
            class, encoding, object, start, end, reason,
        ))
    }
}
