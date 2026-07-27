// Generic callable classification.
//
// This integration boundary knows the runtime representations that can be
// called, but it does not implement concrete builtin descriptors or methods.
// Those are offered through `try_call_builtin_callable`.

impl Interpreter {
    pub(crate) fn call_function_expanded(
        &mut self,
        function: Value,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        // User functions dominate ordinary Python workloads. Keep this check
        // before the builtin-adapter probes.
        if let ValueKind::UserFunction(function) = function.kind() {
            let function = Rc::clone(function);
            return self.call_user_function_expanded(function, args, &[]);
        }

        // A plain registry builtin is never one of the opaque descriptor
        // adapters below. Route it before those adapters inspect/downcast the
        // value; `len(...)`, module functions, and internal typing helpers all
        // use this representation.
        if matches!(function.kind(), ValueKind::BuiltinFunction(_)) {
            return self.call_builtin_function_value(&function, args);
        }

        // Class values have a distinct runtime representation, so route them
        // directly to the builtin-class adapter before generic construction.
        // This avoids testing every unrelated BuiltinObject adapter on each
        // ordinary `Class(...)` call.
        if let ValueKind::PyClass(class) = function.kind() {
            let class = Rc::clone(class);
            if let Some(value) = self.try_call_builtin_class(&class, args)? {
                return Ok(value);
            }
            return self.call_class_expanded(class, args);
        }

        // These ordinary callable representations likewise cannot be one of
        // the BuiltinObject adapters. In particular, a callable PyInstance
        // such as functools' LRU wrapper must not pay every opaque-adapter
        // probe on each invocation.
        if let ValueKind::BoundMethod { function, receiver } = function.kind() {
            let function = Rc::clone(function);
            let receiver = Rc::clone(receiver);
            return self.call_user_function_expanded(
                function,
                args,
                &[Value::py_instance(receiver)],
            );
        }
        if let ValueKind::ClassBoundMethod { function, class } = function.kind() {
            let function = Rc::clone(function);
            let class = Rc::clone(class);
            return self.call_user_function_expanded(function, args, &[Value::py_class(class)]);
        }
        if let ValueKind::PyInstance(instance) = function.kind() {
            let instance = Rc::clone(instance);
            let class = Rc::clone(&instance.borrow().class);
            if let Some(call) = lookup_class_attr(&class, "__call__") {
                return invoke_class_method(self, call, Value::py_instance(instance), args);
            }
            return Err(pyrust_core::type_err!(
                "'{}' object is not callable",
                class.borrow().name
            ));
        }

        // Every adapter recognised by `try_call_builtin_callable` has the
        // opaque BuiltinObject representation. Keep the semantic probes
        // behind that representation check so non-callable primitive values
        // also fail without walking the adapter inventory.
        if matches!(function.kind(), ValueKind::BuiltinObject { .. })
            && let Some(value) = self.try_call_builtin_callable(&function, args)?
        {
            return Ok(value);
        }

        Err(pyrust_core::type_err!(
            "'{}' object is not callable",
            pyrust_core::builtin_type_name(&function)
        ))
    }
}
