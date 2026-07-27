impl Interpreter {
    fn call_builtin_object_bound_method(
        &mut self,
        receiver: &Value,
        method: &str,
        pos: &mut Vec<Value>,
        kw: &PyDict,
    ) -> Result<Value> {
        let ValueKind::BuiltinObject { ops, state } = receiver.kind() else {
            unreachable!("receiver family checked by bound method dispatcher");
        };
        // Validate before bytearray join/extend can consume an iterable. The
        // concrete type modules own the method-name policy; this adapter only
        // selects them by stable runtime identity.
        match ops.canonical_class_tag() {
            Some(pyrust_core::CanonicalClassTag::Bytearray) => {
                pyrust_builtins::bytearray::validate_method_keywords(method, !kw.is_empty())?;
            }
            Some(pyrust_core::CanonicalClassTag::Frozenset) => {
                pyrust_builtins::frozenset::validate_method_keywords(method, !kw.is_empty())?;
            }
            _ if pyrust_builtins::slice::is_slice_ops(ops) => {
                pyrust_builtins::slice::validate_method_keywords(method, !kw.is_empty())?;
            }
            _ => {}
        }
        let mut args_vec: Vec<Value> = std::mem::take(pos);
        let bytearray =
            ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray);
        let splitlines = bytearray && method == "splitlines";
        if bytearray
            && let Some(bound) = self.bind_bytearray_splitlines_keepends(method, &args_vec, kw)?
        {
            args_vec = bound;
        }
        // bytearray methods accept bytes-subclass / bytearray args
        // (#1928); coerce them to a real `Bytes` value before the
        // receiver-only ops extractors (which match exact `Bytes`) see
        // them.  Other BuiltinObject types (frozenset) are untouched.
        if bytearray {
            if method == "join" {
                // #2538: drive a lazy-iterator `join` argument through the
                // interpreter (the ops table can only drain a
                // NativeIterFrame) and coerce its bytes-subclass /
                // bytearray elements.
                args_vec = self.prepare_bytearray_join_args(args_vec)?;
            } else {
                args_vec = coerce_bytes_subclass_method_args(method, args_vec);
            }
            // #2532: drive a lazy-iterator `extend` argument through the
            // interpreter before the receiver-only ops table sees it.
            if method == "extend" {
                args_vec = self.prepare_bytearray_extend_args(args_vec)?;
            }
        }
        let empty_kw = PyDict::default();
        let coerced_kw = if bytearray && !splitlines {
            coerce_bytes_subclass_method_kwargs(kw)
        } else {
            None
        };
        let kw = if splitlines {
            &empty_kw
        } else {
            coerced_kw.as_ref().unwrap_or(kw)
        };
        // Thread any keyword arguments through to the builtin object
        // (e.g. `bytearray.split(maxsplit=1)`); `call_method` keeps its
        // kwargs `String`-keyed.
        let kw_str: indexmap::IndexMap<String, Value> = kw
            .iter()
            .map(|(k, v)| {
                let key = match k {
                    PyKey::Str(s) => s.as_str().unwrap_or("").to_owned(),
                    _ => String::new(),
                };
                (key, v.clone())
            })
            .collect();
        ops.call_method(state, method, args_vec, &kw_str)
    }
}
