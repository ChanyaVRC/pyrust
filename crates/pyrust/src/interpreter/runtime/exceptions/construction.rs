// Construction rules for built-in and user-defined exception classes.

impl Interpreter {
    /// Construct an instance of a built-in or user-defined exception class.
    /// Handles the user `__new__`/`__init__` dispatch for exception subclasses
    /// plus the special keyword/positional argument shapes CPython uses for
    /// `NameError`, `ImportError`, `SyntaxError`, the `Unicode*Error` family,
    /// and `BaseExceptionGroup`/`ExceptionGroup`.
    pub(super) fn construct_exception_instance(
        &mut self,
        class: Rc<RefCell<PyClass>>,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        // Classify the class once up front: a single non-cloning MRO walk
        // (issue #1967) that yields both the special-exception flags reused
        // throughout this function (and threaded into `instantiate_exception`)
        // and `has_user_new`/`has_user_init`.  The latter let the hot
        // `raise ValueError("x")` path skip the dedicated `__new__`/`__init__`
        // MRO lookups entirely — plain built-in exceptions have neither.
        let kinds = classify_exception_class(&class);

        // Issue #1420: if the class has a user-defined __new__ (UserFunction in
        // the MRO), call it with `cls` as the first argument before falling
        // through to instantiate_exception.  This mirrors the non-exception
        // __new__ dispatch below (issue #1143).
        let user_new = if kinds.has_user_new {
            lookup_class_attr(&class, "__new__")
                .filter(|v| matches!(v.kind(), ValueKind::UserFunction(_)))
        } else {
            None
        };
        if let Some(new_val) = user_new {
            let func = match new_val.kind() {
                ValueKind::UserFunction(f) => Rc::clone(f),
                _ => unreachable!(),
            };
            let new_result = self.call_user_function_expanded(
                func,
                args,
                &[Value::py_class(Rc::clone(&class))],
            )?;
            // After __new__, call __init__ only if the result is an instance
            // of cls (CPython parity).
            if let ValueKind::PyInstance(inst_rc) = new_result.kind() {
                let inst_class = inst_rc.borrow().class.clone();
                if class_is_subclass_of(&inst_class, &class) {
                    let init = lookup_class_attr(&inst_class, "__init__");
                    if let Some(init_val) = init
                        && matches!(
                            init_val.kind(),
                            ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_)
                        )
                    {
                        let result = invoke_class_method(
                            self,
                            init_val,
                            Value::py_instance(Rc::clone(inst_rc)),
                            args,
                        )?;
                        if !result.is_none() {
                            return Err(pyrust_core::type_err!(&format!(
                                "__init__() should return None, not '{}'",
                                pyrust_core::builtin_type_name(&result),
                            )));
                        }
                    }
                }
            }
            return Ok(new_result);
        }

        // Issue #1112: if the class has a user-defined __init__ (UserFunction in
        // the MRO), create the instance via instantiate_exception first (which sets
        // .args and any special attrs like StopIteration.value from the constructor
        // args), then call the user's __init__ so it can override .args via
        // super().__init__(...) and set its own instance attributes.
        let user_init = if kinds.has_user_init {
            lookup_class_attr(&class, "__init__")
                .filter(|v| matches!(v.kind(), ValueKind::UserFunction(_)))
        } else {
            None
        };
        if let Some(init_val) = user_init {
            let values: Vec<Value> = args
                .iter()
                .filter(|a| a.name.is_none())
                .map(|a| a.value.clone())
                .collect();
            let instance = instantiate_exception(Rc::clone(&class), values);
            let result = invoke_class_method(self, init_val, instance.clone(), args)?;
            if !result.is_none() {
                return Err(pyrust_core::type_err!(&format!(
                    "__init__() should return None, not '{}'",
                    pyrust_core::builtin_type_name(&result),
                )));
            }
            return Ok(instance);
        }

        // CPython 3.12: NameError.__init__ accepts exactly one keyword argument
        // (`name=`); ImportError.__init__ accepts two (`name=` and `path=`).
        // Extract any recognised keyword arguments before building the positional
        // values list; reject unrecognised keywords with the class-specific
        // error message CPython uses.
        //
        // IMPORTANT: CPython's error messages always use the *base* class name
        // ("NameError()" / "ImportError()"), even when the actual class is a
        // subclass like UnboundLocalError or ModuleNotFoundError.
        let class_name = class.borrow().name.clone();
        let is_name_error_class = kinds.name_error;
        let is_import_error_class = kinds.import_error;
        let mut kw_name: Option<Value> = None;
        let mut kw_path: Option<Value> = None;
        let mut values = Vec::with_capacity(args.len());
        if is_name_error_class {
            // CPython 3.12: NameError accepts at most 1 keyword argument (`name=`).
            // If total kwarg count > 1, raises "takes at most 1 keyword argument".
            // If total kwarg count == 1 and it is not `name=`, raises "invalid keyword".
            // Error messages always say "NameError()" regardless of the actual subclass.
            let kw_count = args.iter().filter(|a| a.name.is_some()).count();
            if kw_count > 1 {
                return Err(pyrust_core::type_err!(
                    "NameError() takes at most 1 keyword argument ({kw_count} given)"
                ));
            }
            for arg in args {
                match arg.name.as_deref() {
                    None => values.push(arg.value.clone()),
                    Some("name") => kw_name = Some(arg.value.clone()),
                    Some(other) => {
                        return Err(pyrust_core::type_err!(
                            "'{other}' is an invalid keyword argument for NameError()"
                        ));
                    }
                }
            }
        } else if is_import_error_class {
            // CPython 3.12: ImportError accepts `name=` and `path=`; any other
            // keyword raises "'X' is an invalid keyword argument for ImportError()".
            // Error messages always say "ImportError()" regardless of the actual subclass.
            for arg in args {
                match arg.name.as_deref() {
                    None => values.push(arg.value.clone()),
                    Some("name") => kw_name = Some(arg.value.clone()),
                    Some("path") => kw_path = Some(arg.value.clone()),
                    Some(other) => {
                        return Err(pyrust_core::type_err!(
                            "'{other}' is an invalid keyword argument for ImportError()"
                        ));
                    }
                }
            }
        } else {
            reject_keyword_args_expanded(&class_name, args)?;
            for arg in args {
                values.push(arg.value.clone());
            }
        }
        // CPython 3.12 SyntaxError.__init__ validates args[1] if present:
        // it must be an iterable that yields exactly 4 or 6 elements.
        // Non-iterables raise TypeError; the wrong number raises TypeError.
        if kinds.syntax_error && values.len() >= 2 {
            let second = &values[1];
            let items_opt: Option<Vec<Value>> = second
                .as_tuple()
                .map(|s| s.to_vec())
                .or_else(|| second.as_list().map(|s| s.to_vec()));
            match items_opt {
                None => {
                    // args[1] is not a sequence — CPython raises TypeError
                    return Err(pyrust_core::type_err!(&format!(
                        "'{}' object is not iterable",
                        pyrust_core::builtin_type_name(second)
                    )));
                }
                Some(ref items) if items.len() < 4 => {
                    return Err(pyrust_core::type_err!(&format!(
                        "function takes at least 4 arguments ({} given)",
                        items.len()
                    )));
                }
                Some(ref items) if items.len() == 5 => {
                    return Err(pyrust_core::type_err!(
                        "end_offset must be provided when end_lineno is provided"
                    ));
                }
                Some(ref items) if items.len() > 6 => {
                    return Err(pyrust_core::type_err!(&format!(
                        "function takes at most 6 arguments ({} given)",
                        items.len()
                    )));
                }
                _ => {}
            }
        }
        // CPython 3.12: UnicodeDecodeError and UnicodeEncodeError require
        // exactly 5 positional arguments; UnicodeTranslateError requires 4.
        // Also validate argument types (encoding must be str, object must be
        // bytes for Decode / str for Encode, start/end must be int-like,
        // reason must be str).
        if kinds.unicode_decode_error {
            self.validate_unicode_decode_args(&mut values)?;
        } else if kinds.unicode_encode_error {
            self.validate_unicode_encode_args(&mut values)?;
        } else if kinds.unicode_translate_error {
            self.validate_unicode_translate_args(&mut values)?;
        }
        // PEP 654 (Python 3.11+): BaseExceptionGroup and ExceptionGroup validation.
        // CPython validates in BaseExceptionGroup.__new__:
        //  - message must be a str
        //  - exceptions must be a non-empty sequence of BaseException instances
        //  - If calling ExceptionGroup, all exceptions must be Exception subclasses
        //  - If calling BaseExceptionGroup and all exceptions are Exception subclasses,
        //    the returned type is silently promoted to ExceptionGroup.
        let is_base_exception_group = kinds.base_exception_group;
        if is_base_exception_group {
            // Validate arg count.
            if values.len() != 2 {
                return Err(pyrust_core::type_err!(&format!(
                    "BaseExceptionGroup.__new__() takes exactly 2 arguments ({} given)",
                    values.len()
                )));
            }
            // Validate message is a str.
            // CPython: "BaseExceptionGroup.__new__() argument 1 must be str, not <type>"
            if !matches!(values[0].kind(), ValueKind::Str(_)) {
                return Err(pyrust_core::type_err!(&format!(
                    "BaseExceptionGroup.__new__() argument 1 must be str, not {}",
                    pyrust_core::builtin_type_name(&values[0])
                )));
            }
            // Validate exceptions is a non-empty sequence.
            let exc_items: Option<Vec<Value>> = values[1]
                .as_tuple()
                .map(|s| s.to_vec())
                .or_else(|| values[1].as_list().map(|s| s.to_vec()));
            let exc_items = if let Some(items) = exc_items {
                items
            } else {
                // CPython raises TypeError for non-sequence second argument
                // (e.g. an integer, a generator/iterator), and ValueError
                // for a sequence whose items are not exceptions (e.g. a string
                // whose characters are not exceptions).
                // Match CPython: str is a sequence, so each character is
                // checked and produces ValueError; everything else is TypeError.
                if let ValueKind::Str(s) = values[1].kind() {
                    if s.is_empty() {
                        return Err(pyrust_core::value_err!(
                            "second argument (exceptions) must be a non-empty sequence"
                        ));
                    }
                    return Err(pyrust_core::value_err!(
                        "Item 0 of second argument (exceptions) is not an exception"
                    ));
                }
                return Err(pyrust_core::type_err!(
                    "second argument (exceptions) must be a sequence"
                ));
            };
            if exc_items.is_empty() {
                return Err(pyrust_core::value_err!(
                    "second argument (exceptions) must be a non-empty sequence"
                ));
            }
            // Validate each exception is a BaseException instance.
            for (i, exc_val) in exc_items.iter().enumerate() {
                let ok = if let ValueKind::PyInstance(inst_rc) = exc_val.kind() {
                    pyrust_core::class_chain_contains_builtin_exception(
                        &inst_rc.borrow().class,
                        "BaseException",
                    )
                } else {
                    false
                };
                if !ok {
                    return Err(pyrust_core::value_err!(&format!(
                        "Item {} of second argument (exceptions) is not an exception",
                        i
                    )));
                }
            }
            // If ExceptionGroup, all exceptions must be Exception (not just BaseException).
            let is_eg =
                pyrust_core::class_chain_contains_builtin_exception(&class, "ExceptionGroup");
            if is_eg {
                for exc_val in &exc_items {
                    if let ValueKind::PyInstance(inst_rc) = exc_val.kind()
                        && !pyrust_core::class_chain_contains_builtin_exception(
                            &inst_rc.borrow().class,
                            "Exception",
                        )
                    {
                        let message = {
                            let borrowed = class.borrow();
                            if borrowed.builtin_exception_name == Some("ExceptionGroup") {
                                "Cannot nest BaseExceptions in an ExceptionGroup".to_string()
                            } else {
                                format!("Cannot nest BaseExceptions in '{}'", borrowed.name)
                            }
                        };
                        return Err(pyrust_core::type_err!("{message}"));
                    }
                }
            }
            // CPython: if calling BaseExceptionGroup and all exceptions are Exception
            // subclasses, the returned type is ExceptionGroup.
            let is_beg = class.borrow().builtin_exception_name == Some("BaseExceptionGroup");
            let actual_class = if is_beg {
                let all_exceptions = exc_items.iter().all(|exc_val| {
                    if let ValueKind::PyInstance(inst_rc) = exc_val.kind() {
                        pyrust_core::class_chain_contains_builtin_exception(
                            &inst_rc.borrow().class,
                            "Exception",
                        )
                    } else {
                        false
                    }
                });
                if all_exceptions {
                    // Promote to ExceptionGroup.
                    lookup_exc_class("ExceptionGroup").unwrap_or(class)
                } else {
                    class
                }
            } else {
                class
            };
            let instance = instantiate_exception(actual_class, values);
            return Ok(instance);
        }
        // Reuse the `kinds` classification computed above instead of running a
        // second MRO walk inside `instantiate_exception` (perf: one classify per
        // raise instead of two).
        let instance = instantiate_exception_with_kinds(class, values, &kinds);
        // Apply keyword arguments extracted above for NameError and ImportError.
        // `instantiate_exception` already initialised `.name` (and `.path`) to
        // `None`; override them with the caller-supplied values when provided.
        // CPython 3.12: keyword values are NOT included in `.args`.
        if let Some(name_val) = kw_name
            && let ValueKind::PyInstance(inst_rc) = instance.kind()
        {
            inst_rc.borrow_mut().attrs.insert_slot("name", name_val);
        }
        if let Some(path_val) = kw_path
            && let ValueKind::PyInstance(inst_rc) = instance.kind()
        {
            inst_rc.borrow_mut().attrs.insert_slot("path", path_val);
        }
        Ok(instance)
    }
}
