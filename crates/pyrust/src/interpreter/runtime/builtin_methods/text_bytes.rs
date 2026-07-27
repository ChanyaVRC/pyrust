// Text/bytes method implementations that require interpreter services.
impl Interpreter {
    /// Convert an already-bound splitlines `keepends` positional slot through
    /// Python's canonical truth protocol.  The interpreter-free text/bytes
    /// implementations receive only a plain Bool and never need to dispatch
    /// user `__bool__` / `__len__` themselves.
    fn truthify_splitlines_keepends(&mut self, args: &mut [Value]) -> Result<()> {
        if let Some(value) = args.first_mut() {
            // Bool/Int are the overwhelmingly common flags and cannot own a
            // user truth slot unless represented as PyInstance subclasses.
            // Avoid entering the general protocol router for these exact
            // values; every other kind still uses canonical truth dispatch.
            let keepends = match value.kind() {
                ValueKind::Bool(value) => value,
                ValueKind::Int(value) => value != 0,
                _ => self.truthy_value(value)?,
            };
            *value = Value::bool_(keepends);
        }
        Ok(())
    }

    /// Bind positional/keyword `keepends` using the existing signature owner,
    /// then canonicalise the sole value.  Callers must run their concrete
    /// type's keyword/positional validation first so established error
    /// precedence remains unchanged.
    fn bind_splitlines_keepends(&mut self, args: &[Value], kw: &PyDict) -> Result<Vec<Value>> {
        let mut bound =
            pyrust_builtins::bytes::merge_single_kwarg("splitlines", "keepends", args, kw)?;
        self.truthify_splitlines_keepends(&mut bound)?;
        Ok(bound)
    }

    /// Bytearray's BuiltinTypeOps validates positional arity before merging
    /// keyword slots.  Every bound/unbound/subclass adapter calls this one
    /// helper after keyword-policy validation, preserving that order without
    /// duplicating it at each dispatch site.
    fn bind_bytearray_splitlines_keepends(
        &mut self,
        method: &str,
        args: &[Value],
        kw: &PyDict,
    ) -> Result<Option<Vec<Value>>> {
        if method != "splitlines" {
            return Ok(None);
        }
        pyrust_builtins::bytearray::validate_method_positional_arity(method, args.len())?;
        self.bind_splitlines_keepends(args, kw).map(Some)
    }

    /// Dispatch any str method.  `join` is handled here to support generators
    /// and any custom iterable via `collect_iterable`; `format_map` is handled
    /// here because it routes through `format_str_template_map`; `format` is
    /// intercepted by the builtin-method bound dispatch (which has access to
    /// kwargs) before reaching this function. Everything else delegates to the
    /// interpreter-free `pyrust_builtins::string::call`.
    pub(crate) fn call_str_method(
        &mut self,
        method: &str,
        receiver: Value,
        mut args: Vec<Value>,
    ) -> Result<Value> {
        pyrust_builtins::string::validate_method_positional_arity(method, args.len())?;
        if method == "splitlines" {
            // Keyword binding, including duplicate/invalid-keyword errors, has
            // already happened in str_merge_kwargs.  Positional arity is
            // validated above before a user truth hook may run.
            self.truthify_splitlines_keepends(&mut args)?;
        }
        // CPython's str methods accept any str subclass wherever a str argument
        // is expected (#1927).  The receiver-only `pyrust_builtins::string`
        // extractors match an exact `ValueKind::Str`, so coerce str-subclass
        // instances to their backing here before delegating.  Coercion is a
        // cheap no-op for exact-str / non-instance args (the common case) and
        // for non-str-backed instances, so wrong-type args still raise the
        // existing TypeError.  startswith/endswith also accept a *tuple* of str
        // prefixes/suffixes — coerce each element of a tuple arg too.
        let args = coerce_str_subclass_method_args(method, args);
        if method == "format_map" {
            if args.len() != 1 {
                return Err(pyrust_core::type_err!(
                    "str.format_map() takes exactly one argument ({} given)",
                    args.len()
                ));
            }
            // Borrow template as &str from the receiver to avoid a heap allocation.
            // receiver is held by value for the lifetime of this block.
            let template: &str = match receiver.kind() {
                ValueKind::Str(s) => s,
                _ => return Err(pyrust_core::descriptor_requires!("format_map", "str")),
            };
            let mapping = args.into_iter().next().unwrap();
            return self.format_str_template_map(template, mapping);
        }
        if method == "join" {
            if args.len() != 1 {
                return Err(pyrust_core::type_err!(
                    "str.join() takes exactly one argument ({} given)",
                    args.len()
                ));
            }
            let iterable = args.into_iter().next().unwrap();
            // Fast paths: types already handled directly by the builtins join fn.
            // Check the tag first (drops the borrow) before deciding whether to
            // call collect_iterable — the borrow from kind() must not overlap
            // with the &mut self borrow that collect_iterable needs.
            let needs_collect = !matches!(
                iterable.kind(),
                ValueKind::List(_) | ValueKind::Tuple(_) | ValueKind::Str(_) | ValueKind::Dict(_)
            );
            let iterable = if needs_collect {
                let items = collect_join_iterable(self, &iterable)?;
                Value::list(items.into_iter().map(coerce_str_subclass_arg).collect())
            } else {
                // Fast-path containers (List/Tuple) may hold str-subclass items;
                // CPython joins them by their str value (#1927).  Materialise a
                // coerced copy only when an item actually needs coercing so the
                // common all-exact-str list pays nothing but a scan.
                coerce_str_subclass_join_iterable(iterable)
            };
            return pyrust_builtins::string::call_prevalidated(
                "join",
                &receiver,
                std::slice::from_ref(&iterable),
            );
        }
        if method == "translate" {
            // Dict fast path: delegate to pyrust-builtins which handles the
            // common `str.maketrans`-produced dict without needing the interpreter.
            if args.len() != 1 {
                return Err(pyrust_core::type_err!(
                    "str.translate() takes exactly one argument ({} given)",
                    args.len()
                ));
            }
            let is_dict = matches!(args[0].kind(), ValueKind::Dict(_));
            if is_dict {
                // pyrust-builtins matches mapping values on `ValueKind`
                // directly, so a builtin-subclass replacement value
                // (`class MyInt(int)` / `class MyStr(str)`) would be rejected
                // as a TypeError even though CPython accepts the inherited
                // int/str/None backing. pyrust-builtins has no interpreter
                // access to unwrap `__builtin_data__`, so do it here: when any
                // value needs unwrapping, hand pyrust-builtins a value-coerced
                // copy of the dict. The common `str.maketrans`-produced dict
                // (plain int / str values) needs no copy and pays only a scan.
                let table = args.into_iter().next().unwrap();
                let needs_coerce = |v: &Value| {
                    builtin_data_backing(v).is_some_and(|b| !matches!(b.kind(), ValueKind::Dict(_)))
                };
                let coerced = table.dict_with(|d| {
                    if d.values().any(&needs_coerce) {
                        let mut out = pyrust_core::PyDict::default();
                        for (k, v) in d.iter() {
                            let coerced_v = builtin_data_backing(v)
                                .filter(|b| !matches!(b.kind(), ValueKind::Dict(_)))
                                .unwrap_or_else(|| v.clone());
                            out.insert(k.clone(), coerced_v);
                        }
                        Some(Value::dict(out))
                    } else {
                        None
                    }
                });
                let table = coerced.flatten().unwrap_or(table);
                return pyrust_builtins::string::call_prevalidated(
                    "translate",
                    &receiver,
                    std::slice::from_ref(&table),
                );
            }
            // General mapping protocol: call table[ordinal] per codepoint.
            // KeyError / IndexError / LookupError → keep character;
            // None → delete; int → replace with chr(n); str → replace.
            // Materialise chars and reserve capacity under a narrow borrow so
            // that the &str from receiver.kind() drops before eval_index needs
            // a &mut self borrow (they are separate but keep scopes explicit).
            let (chars, out_capacity) = match receiver.kind() {
                ValueKind::Str(s) => (s.chars().collect::<Vec<char>>(), s.len()),
                _ => return Err(pyrust_core::descriptor_requires!("translate", "str")),
            };
            let table = args.into_iter().next().unwrap();
            let mut out = String::with_capacity(out_capacity);
            for c in chars {
                let cp = Value::int(c as i64);
                match self.eval_index(&table, cp) {
                    Ok(v) => {
                        // Resolve int/str subclass instances to their backing
                        // primitive before the value match. This covers:
                        //   int subclass  → Int/Bool/BigInt backing
                        //   str subclass  → Str backing
                        // A PyInstance without a relevant backing falls through
                        // to the TypeError arm below.
                        let v = builtin_data_backing(&v).unwrap_or(v);
                        match v.kind() {
                            ValueKind::None => { /* delete */ }
                            ValueKind::Int(n) => {
                                if !(0..=0x10FFFF).contains(&n) {
                                    return Err(pyrust_core::value_err!(
                                        "character mapping must be in range(0x110000)".to_string()
                                    ));
                                }
                                let replacement = char::from_u32(n as u32).ok_or_else(|| {
                                    pyrust_core::value_err!(
                                        "character mapping must be in range(0x110000)".to_string()
                                    )
                                })?;
                                out.push(replacement);
                            }
                            ValueKind::Bool(b) => {
                                let replacement =
                                    char::from_u32(b as u32).expect("0 and 1 are valid codepoints");
                                out.push(replacement);
                            }
                            ValueKind::BigInt(n) => {
                                // Use ToPrimitive::to_u32 then char::from_u32 to
                                // validate the range [0, 0x10FFFF] in one step.
                                // A negative or > u32::MAX BigInt yields None from
                                // to_u32(); char::from_u32 rejects surrogates and
                                // values > 0x10FFFF. Both map to the same ValueError.
                                use crate::value::PyToPrimitive;
                                let replacement =
                                    n.to_u32().and_then(char::from_u32).ok_or_else(|| {
                                        pyrust_core::value_err!(
                                            "character mapping must be in range(0x110000)"
                                                .to_string()
                                        )
                                    })?;
                                out.push(replacement);
                            }
                            ValueKind::Str(repl) => {
                                out.push_str(repl);
                            }
                            _ => {
                                return Err(pyrust_core::type_err!(
                                    "character mapping must return integer, None or str"
                                        .to_string()
                                ));
                            }
                        }
                    }
                    Err(e)
                        if e.class_name_is("KeyError")
                            || e.class_name_is("IndexError")
                            || e.class_name_is("LookupError") =>
                    {
                        out.push(c);
                    }
                    Err(e) => return Err(e),
                }
            }
            return Ok(Value::string(out));
        }
        pyrust_builtins::string::call_prevalidated(method, &receiver, &args)
    }

    /// Interpreter-aware bytes adapter.  Signature validation stays with the
    /// concrete bytes method table; only after it succeeds do we execute the
    /// Python truth protocol needed by splitlines.
    pub(crate) fn call_bytes_method_with_protocols(
        &mut self,
        method: &str,
        receiver: &Value,
        args: Vec<Value>,
        kw: &PyDict,
    ) -> Result<Value> {
        pyrust_builtins::bytes::validate_method_keywords(method, !kw.is_empty())?;
        pyrust_builtins::bytes::validate_method_positional_arity(method, args.len())?;
        self.call_bytes_method_with_protocols_prevalidated(method, receiver, args, kw)
    }

    /// Bytes adapter after the caller has validated keyword policy and
    /// positional arity (the fused container-method path).
    pub(crate) fn call_bytes_method_with_protocols_prevalidated(
        &mut self,
        method: &str,
        receiver: &Value,
        args: Vec<Value>,
        kw: &PyDict,
    ) -> Result<Value> {
        if method == "splitlines" {
            let bound = self.bind_splitlines_keepends(&args, kw)?;
            return call_bytes_method_coerced_prevalidated(
                method,
                receiver,
                bound,
                &PyDict::default(),
            );
        }
        call_bytes_method_coerced_prevalidated(method, receiver, args, kw)
    }

    /// Dispatch `bytes.join()` with support for generators and arbitrary iterables.
    /// All other bytes methods are handled directly by `pyrust_builtins::bytes::call`.
    pub(crate) fn call_bytes_join(&mut self, receiver: Value, args: Vec<Value>) -> Result<Value> {
        pyrust_builtins::bytes::validate_method_positional_arity("join", args.len())?;
        if args.len() != 1 {
            return Err(pyrust_core::type_err!(
                "bytes.join() takes exactly one argument ({} given)",
                args.len()
            ));
        }
        let iterable = args.into_iter().next().unwrap();
        let needs_collect = !matches!(iterable.kind(), ValueKind::List(_) | ValueKind::Tuple(_));
        let iterable = if needs_collect {
            let items = collect_join_iterable(self, &iterable)?;
            Value::list(items.into_iter().map(coerce_bytes_subclass_arg).collect())
        } else {
            // List/Tuple fast path may hold bytes-subclass / bytearray items;
            // CPython joins them by their bytes value (#1928).
            coerce_bytes_subclass_join_iterable(iterable)
        };
        pyrust_builtins::bytes::call_prevalidated(
            "join",
            &receiver,
            &[iterable],
            &PyDict::default(),
        )
    }
}
