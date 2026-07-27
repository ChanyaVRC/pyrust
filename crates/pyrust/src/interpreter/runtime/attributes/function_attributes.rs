// Function-object attribute semantics.
impl Interpreter {
    pub(super) fn get_function_attribute(
        &mut self,
        target: &Value,
        func: &Rc<UserFunction>,
        name: &str,
    ) -> Result<Value> {
        match name {
            "__name__" => {
                return Ok(Value::string(func.effective_name()));
            }
            "__qualname__" => {
                let q = func.effective_qualname();
                return Ok(Value::string(q));
            }
            "__module__" => return Ok(func.module_value()),
            "__doc__" => return Ok(func.doc.borrow().clone()),
            "__dict__" => {
                // Return the live dict object — CPython returns the same
                // object every time, so `d = f.__dict__; d['x'] = 1`
                // makes `f.x` visible.  Initialise lazily on first access.
                let attrs_rc = func_attrs_rc(func);
                return Ok(attrs_rc.borrow().clone());
            }
            "__annotations__" => {
                // Lazily materialised (#2256) and stored, so repeated reads
                // yield the same object identity, matching CPython:
                // `f.__annotations__ is f.__annotations__` is True.
                return Ok(func.annotations_value());
            }
            "__defaults__" => {
                // #2395: tuple of positional defaults, or the per-object
                // override set via `f.__defaults__ = …`.  `None` when no
                // positional default exists (CPython semantics).
                return Ok(func.defaults_value());
            }
            "__kwdefaults__" => {
                // #2395: dict of keyword-only defaults, or the per-object
                // override set via `f.__kwdefaults__ = …`.  `None` when
                // none exist (CPython returns `None`, not an empty dict).
                return Ok(func.kwdefaults_value());
            }
            "__globals__" => {
                // The globals provider captured by this function's defining
                // root, which may be an explicit exec/eval dictionary rather
                // than the interpreter currently invoking it.
                return Ok(self.globals_for_environment(&func.env));
            }
            "__closure__" => {
                // A tuple of `cell` objects (one per free variable, in
                // `co_freevars` order), or `None` when the function has
                // no free variables (issue #2106).  pyrust resolves free
                // variables through the captured `env` chain rather than
                // CPython-style cells; `build_closure` recovers the cell
                // set from that chain.
                return Ok(self.build_closure(func));
            }
            "__code__" => {
                // A lightweight code object carrying the introspection
                // attributes most consumers read: co_name / co_argcount /
                // co_varnames (CPython parameter order), plus the
                // best-effort co_flags / co_filename / co_firstlineno /
                // co_consts / co_names (issues #1959, #2171).  co_name is
                // the original declared name baked into the (immutable)
                // code object; reassigning `f.__name__` does not change
                // `f.__code__.co_name`, so `build_code_object` uses
                // `func.name` rather than the mutable `user_name`.
                return Ok(self.build_code_object(func));
            }
            "__func__"
                if matches!(
                    func.kind,
                    UserFunctionKind::StaticMethod | UserFunctionKind::ClassMethod
                ) =>
            {
                // `staticmethod.__func__` and `classmethod.__func__` return the
                // exact object that was passed to staticmethod()/classmethod(),
                // preserving identity (`sm.__func__ is f`).
                // `wrapped_func` holds the original Rc from the wrapping call.
                // Fall back to stripping the kind tag when there is no stored
                // `wrapped_func` (compile-time tagging of a Builtin, or any
                // path that predates this field).
                return if let Some(inner) = func.wrapped_func.as_ref() {
                    Ok(Value::user_function(Rc::clone(inner)))
                } else {
                    Ok(Value::with_function_kind(
                        Rc::clone(func),
                        UserFunctionKind::Regular,
                    ))
                };
            }
            "__get__" if func.kind == UserFunctionKind::ClassMethod => {
                // `classmethod.__get__(instance, owner)` — returns a binder
                // that, when called, creates a ClassBoundMethod.  The
                // interpreter's `call_function_expanded` resolves the binder
                // (see guard arm for `as_class_method_get_binder`).
                return Ok(pyrust_builtins::classmethod::class_method_get_binder(
                    Rc::clone(func),
                ));
            }
            "__get__" if func.kind == UserFunctionKind::StaticMethod => {
                // `staticmethod.__get__(instance, owner)` — returns a binder
                // that, when called, returns the underlying plain function.
                return Ok(pyrust_builtins::classmethod::static_method_get_binder(
                    Rc::clone(func),
                ));
            }
            "__call__" if !matches!(func.kind, UserFunctionKind::ClassMethod) => {
                // Issue #2550: a plain function, lambda, or staticmethod is
                // callable, so CPython exposes `f.__call__ ==
                // <method-wrapper '__call__' of function object at 0x...>`
                // (and `hasattr(f, '__call__') is True`).  Surface a wrapper
                // bound to the function; calling it re-dispatches onto `f`
                // (handled by `as_type_call_wrapper` in
                // `call_function_expanded`).  `classmethod` is excluded —
                // CPython's classmethod object is not itself callable.
                return Ok(pyrust_builtins::type_call_wrapper::call_wrapper(
                    target.clone(),
                    "function",
                ));
            }
            _ => {}
        }
        // Fall through to arbitrary dynamic attrs.
        // Short-circuit without initialising if no attrs have been stored yet.
        if let Some(rc) = func.attrs.borrow().as_ref().map(Rc::clone)
            && let Some(v) = rc
                .borrow()
                .as_dict()
                .and_then(|d| d.get(&StrKey(name)).cloned())
        {
            return Ok(v);
        }
        let type_name = match func.kind {
            UserFunctionKind::StaticMethod => pyrust_builtins::classmethod::STATIC_TYPE_NAME,
            UserFunctionKind::ClassMethod => pyrust_builtins::classmethod::CLASS_TYPE_NAME,
            _ => "function",
        };
        Err(PyError::attribute_error(
            format!("'{type_name}' object has no attribute '{name}'"),
            Some(name.to_string()),
            Some(target.clone()),
        ))
    }

    pub(super) fn try_get_bound_method_attribute(
        &self,
        target: &Value,
        name: &str,
    ) -> Option<Result<Value>> {
        match target.kind() {
            ValueKind::BoundMethod { function, receiver } => {
                if name == "__func__" {
                    return Some(Ok(Value::user_function(Rc::clone(function))));
                }
                if name == "__self__" {
                    return Some(Ok(Value::py_instance(Rc::clone(receiver))));
                }
                if name == "__call__" {
                    return Some(Ok(pyrust_builtins::type_call_wrapper::call_wrapper(
                        target.clone(),
                        "method",
                    )));
                }
                bound_method_common_attr(function, name)
            }
            ValueKind::ClassBoundMethod { function, class } => {
                if name == "__func__" {
                    let value = if function.kind == UserFunctionKind::ClassMethod {
                        if let Some(inner) = function.wrapped_func.as_ref() {
                            Value::user_function(Rc::clone(inner))
                        } else {
                            Value::with_function_kind(
                                Rc::clone(function),
                                UserFunctionKind::Regular,
                            )
                        }
                    } else {
                        Value::user_function(Rc::clone(function))
                    };
                    return Some(Ok(value));
                }
                if name == "__self__" {
                    return Some(Ok(Value::py_class(Rc::clone(class))));
                }
                if name == "__call__" {
                    return Some(Ok(pyrust_builtins::type_call_wrapper::call_wrapper(
                        target.clone(),
                        "method",
                    )));
                }
                bound_method_common_attr(function, name)
            }
            _ => None,
        }
    }
}
