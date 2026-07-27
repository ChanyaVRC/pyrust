impl Interpreter {
    /// `bytes % args` / `bytearray % args` — PEP 461 printf-style formatting
    /// (#1883).  Mirrors [`Self::str_printf_format`]'s flag / width / precision
    /// parser but produces a `bytes` (or `bytearray` when `as_bytearray`) and
    /// applies PEP 461 conversion semantics:
    ///
    /// - `%b` / `%s`: a bytes-like argument (bytes, bytearray, bytes subclass)
    ///   or an object implementing `__bytes__`; a `str` argument is a
    ///   `TypeError`.
    /// - `%a`: the `ascii()` repr of the argument, encoded to bytes.
    /// - `%c`: an integer in `range(256)` or a single byte.
    /// - numeric / float codes (`%d %i %u %o %x %X %e %E %f %F %g %G`): reuse
    ///   the shared [`Self::str_printf_convert`] machinery — the output is pure
    ///   ASCII and identical to the `str %` path.
    pub(super) fn bytes_printf_format(
        &mut self,
        fmt: &[u8],
        args: Value,
        as_bytearray: bool,
    ) -> Result<Value> {
        // Mapping mode is triggered by a %(key) code in the format string,
        // exactly as in str %; the keys are bytes, not str.
        let has_named_key = {
            let mut found = false;
            let mut j = 0;
            while j + 1 < fmt.len() {
                if fmt[j] == b'%' && fmt[j + 1] == b'(' {
                    found = true;
                    break;
                }
                j += 1;
            }
            found
        };
        // Same mapping rule as the str path (issue #2089); the subclass /
        // custom-mapping lookup routes through `__getitem__` so `__missing__`
        // is honoured.
        let is_mapping = has_named_key && is_percent_format_mapping(&args);
        let positional: Option<&[Value]> = if is_mapping {
            None
        } else {
            match args.kind() {
                ValueKind::Tuple(items) => Some(items),
                _ => Some(std::slice::from_ref(&args)),
            }
        };
        let mut pos_idx: usize = 0;

        let mut out: Vec<u8> = Vec::with_capacity(fmt.len());
        let len = fmt.len();
        let mut i = 0;

        while i < len {
            if fmt[i] != b'%' {
                out.push(fmt[i]);
                i += 1;
                continue;
            }
            i += 1; // consume '%'
            if i >= len {
                return Err(pyrust_core::value_err!("incomplete format"));
            }

            // Named key: %(key)b — keys are bytes.
            let named_key: Option<&[u8]> = if fmt[i] == b'(' {
                i += 1;
                let start = i;
                while i < len && fmt[i] != b')' {
                    i += 1;
                }
                if i >= len {
                    return Err(pyrust_core::value_err!("incomplete format key"));
                }
                let key = &fmt[start..i];
                i += 1; // consume ')'
                Some(key)
            } else {
                None
            };

            // Flags: -, +, space, #, 0
            let mut flag_minus = false;
            let mut flag_plus = false;
            let mut flag_space = false;
            let mut flag_zero = false;
            let mut flag_hash = false;
            while i < len {
                match fmt[i] {
                    b'-' => flag_minus = true,
                    b'+' => flag_plus = true,
                    b' ' => flag_space = true,
                    b'0' => flag_zero = true,
                    b'#' => flag_hash = true,
                    _ => break,
                }
                i += 1;
            }

            // Width: integer or '*'
            let width: Option<usize> = if i < len && fmt[i] == b'*' {
                i += 1;
                let w = str_printf_take_positional(&positional, &mut pos_idx)?;
                match w.kind() {
                    ValueKind::Int(n) if n >= 0 => Some(n as usize),
                    ValueKind::Int(n) => {
                        flag_minus = true;
                        Some((-n) as usize)
                    }
                    _ => {
                        return Err(pyrust_core::type_err!("* wants int"));
                    }
                }
            } else if i < len && fmt[i].is_ascii_digit() {
                let start = i;
                while i < len && fmt[i].is_ascii_digit() {
                    i += 1;
                }
                Some(
                    std::str::from_utf8(&fmt[start..i])
                        .unwrap()
                        .parse::<usize>()
                        .unwrap(),
                )
            } else {
                None
            };

            // Precision: .integer or .*
            let precision: Option<usize> = if i < len && fmt[i] == b'.' {
                i += 1;
                if i < len && fmt[i] == b'*' {
                    i += 1;
                    let p = str_printf_take_positional(&positional, &mut pos_idx)?;
                    match p.kind() {
                        ValueKind::Int(n) if n >= 0 => Some(n as usize),
                        ValueKind::Int(_) => Some(0),
                        _ => {
                            return Err(pyrust_core::type_err!("* wants int"));
                        }
                    }
                } else {
                    let start = i;
                    while i < len && fmt[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i == start {
                        Some(0)
                    } else {
                        Some(
                            std::str::from_utf8(&fmt[start..i])
                                .unwrap()
                                .parse::<usize>()
                                .unwrap(),
                        )
                    }
                }
            } else {
                None
            };

            // Length modifier: h, l, L — ignored (CPython ignores them too).
            if i < len && matches!(fmt[i], b'h' | b'l' | b'L') {
                i += 1;
            }

            if i >= len {
                return Err(pyrust_core::value_err!("incomplete format"));
            }
            let conv = fmt[i] as char;
            i += 1;

            // %% — literal percent, no argument consumed.
            if conv == '%' {
                out.push(b'%');
                continue;
            }

            // Get the argument value.
            let arg: Value = if let Some(key) = named_key {
                if is_mapping {
                    match args.kind() {
                        ValueKind::Dict(d) => {
                            let k = PyKey::Bytes(Rc::new(key.to_vec()));
                            match d.get(&k) {
                                Some(v) => v.clone(),
                                None => {
                                    return Err(PyError::key_error(Value::bytes(key.to_vec())));
                                }
                            }
                        }
                        // dict subclass / custom mapping: subscript via
                        // `__getitem__` (bytes key) so `__missing__` and a custom
                        // `KeyError` are honoured (issue #2089).
                        _ => self.eval_index(&args, Value::bytes(key.to_vec()))?,
                    }
                } else {
                    return Err(pyrust_core::type_err!("format requires a mapping"));
                }
            } else {
                str_printf_take_positional(&positional, &mut pos_idx)?
            };

            // Format the argument according to the conversion code.  PEP 461
            // conversions produce bytes directly; numeric / float codes reuse
            // the shared str printf converter (ASCII output).
            match conv {
                'b' | 's' => {
                    let data = self.bytes_printf_to_bytes(arg)?;
                    let truncated = match precision {
                        Some(p) if data.len() > p => &data[..p],
                        _ => &data[..],
                    };
                    bytes_printf_apply_width(&mut out, truncated, width, flag_minus);
                }
                'a' => {
                    let repr = ascii_repr_interp(self, &arg)?;
                    let bytes = repr.into_bytes();
                    let truncated = match precision {
                        Some(p) if bytes.len() > p => &bytes[..p],
                        _ => &bytes[..],
                    };
                    bytes_printf_apply_width(&mut out, truncated, width, flag_minus);
                }
                'c' => {
                    let byte = self.bytes_printf_to_char(arg)?;
                    bytes_printf_apply_width(&mut out, &[byte], width, flag_minus);
                }
                _ => {
                    let formatted = self.str_printf_convert(
                        conv,
                        arg,
                        precision,
                        flag_plus,
                        flag_space,
                        flag_hash,
                        i - 1,
                        true,
                    )?;
                    let padded = apply_printf_width(formatted, width, flag_minus, flag_zero, conv);
                    out.extend_from_slice(padded.as_bytes());
                }
            }
        }

        // Unconsumed positional arguments: raise TypeError.
        if let Some(pos) = positional
            && pos_idx < pos.len()
        {
            return Err(pyrust_core::type_err!(
                "not all arguments converted during bytes formatting"
            ));
        }

        if as_bytearray {
            Ok(pyrust_builtins::bytearray::bytearray(out))
        } else {
            Ok(Value::bytes(out))
        }
    }

    /// Resolve a `%b` / `%s` argument to its bytes content (PEP 461).
    ///
    /// Accepts bytes, bytearray, bytes subclasses, and objects implementing
    /// `__bytes__`.  A `str` argument (or any other type) raises the
    /// CPython 3.12 `TypeError`, which always names `%b` (the canonical code)
    /// even when the source code used the `%s` alias.
    fn bytes_printf_to_bytes(&mut self, arg: Value) -> Result<Vec<u8>> {
        if let ValueKind::Bytes(rc) = arg.kind() {
            return Ok(rc.to_vec());
        }
        if let Some(data) = pyrust_builtins::bytearray::as_bytearray_snapshot(&arg) {
            return Ok(data);
        }
        if let Some(inst_rc) = arg.as_py_instance_rc() {
            // bytes subclass: extract the backing bytes directly.
            if let Some(backing) = builtin_data_backing(&arg)
                && let ValueKind::Bytes(rc) = backing.kind()
            {
                return Ok(rc.to_vec());
            }
            let class = Rc::clone(&inst_rc.borrow().class);
            if let Some(method) = lookup_class_attr(&class, "__bytes__") {
                let self_val = Value::py_instance(Rc::clone(inst_rc));
                let result = invoke_class_method(self, method, self_val, &[])?;
                return match result.kind() {
                    ValueKind::Bytes(rc) => Ok(rc.to_vec()),
                    _ => Err(pyrust_core::type_err!(
                        "__bytes__ returned non-bytes (type {})",
                        value_type_name_str(&result)
                    )),
                };
            }
        }
        Err(pyrust_core::type_err!(
            "%b requires a bytes-like object, or an object that implements __bytes__, not '{}'",
            value_type_name_str(&arg)
        ))
    }

    /// Resolve a `%c` argument to a single byte (PEP 461).
    ///
    /// Accepts an integer in `range(256)` (`OverflowError` otherwise) or a
    /// single byte / single-byte bytes-like (`TypeError` for multi-byte or
    /// other types).
    fn bytes_printf_to_char(&mut self, arg: Value) -> Result<u8> {
        // Single-byte bytes-like: b"A" or bytes([65]).
        if let ValueKind::Bytes(rc) = arg.kind() {
            return single_byte_or_err(rc);
        }
        if let Some(data) = pyrust_builtins::bytearray::as_bytearray_snapshot(&arg) {
            return single_byte_or_err(&data);
        }
        // Integer (or __index__): must be in range(256).
        let coerced = self.coerce_printf_int_arg(arg)?;
        match coerced.kind() {
            ValueKind::Int(n) if (0..=255).contains(&n) => Ok(n as u8),
            ValueKind::Bool(b) => Ok(b as u8),
            ValueKind::Int(_) | ValueKind::BigInt(_) => {
                Err(pyrust_core::overflow_err!("%c arg not in range(256)"))
            }
            _ => Err(pyrust_core::type_err!(
                "%c requires an integer in range(256) or a single byte"
            )),
        }
    }
}
