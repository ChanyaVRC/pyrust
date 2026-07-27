impl Interpreter {
    /// `str % args` — CPython-compatible printf-style string formatting (#1393).
    ///
    /// Handles positional (`%s`, `%d`, …) and named (`%(key)s`) format codes.
    /// The right-hand side may be a single value (implicitly a one-element
    /// positional tuple), a tuple (positional), or a dict (named lookup).
    pub(super) fn str_printf_format(&mut self, fmt_val: Value, args: Value) -> Result<Value> {
        // Borrow the format string directly from the Value to avoid a heap allocation.
        // fmt_val is held by value for the duration of this function, so the &str is valid.
        let fmt: &str = match fmt_val.kind() {
            ValueKind::Str(s) => s,
            _ => unreachable!("str_printf_format called with non-str left"),
        };

        // CPython mapping mode is triggered by the format string, not by the RHS type.
        // A dict RHS is only used as a mapping when the format string contains %(key) codes;
        // if the format has only positional codes, the dict is treated as a single positional arg.
        let has_named_key = {
            let b = fmt.as_bytes();
            let mut found = false;
            let mut j = 0;
            while j + 1 < b.len() {
                if b[j] == b'%' && b[j + 1] == b'(' {
                    found = true;
                    break;
                }
                j += 1;
            }
            found
        };
        // CPython enters mapping mode when the format has a `%(key)` code and the
        // rhs is a mapping (issue #2089): a `dict`, a `dict` subclass, or any
        // non-tuple/non-str object exposing `__getitem__`.  A plain `dict` keeps
        // the fast `d.get` lookup; a subclass / custom mapping routes the lookup
        // through `__getitem__` so `__missing__` and a custom `KeyError` are
        // honoured.
        let is_mapping = has_named_key && is_percent_format_mapping(&args);
        // Wrap a non-tuple, non-mapping rhs in a virtual single-element tuple.
        // Use &[Value] to avoid cloning the tuple's items upfront; borrow from
        // args directly for the single-value case to avoid an extra clone.
        let positional: Option<&[Value]> = if is_mapping {
            None
        } else {
            match args.kind() {
                ValueKind::Tuple(items) => Some(items),
                _ => Some(std::slice::from_ref(&args)),
            }
        };
        let mut pos_idx: usize = 0;

        let mut out = String::with_capacity(fmt.len());
        let bytes = fmt.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            if bytes[i] != b'%' {
                // Copy the whole run of literal (non-'%') bytes in one shot
                // rather than decoding and pushing char-by-char.  `bytes` is a
                // valid UTF-8 slice and `%` is ASCII, so a run boundary never
                // splits a multibyte char.
                let start = i;
                i += 1;
                while i < len && bytes[i] != b'%' {
                    i += 1;
                }
                out.push_str(&fmt[start..i]);
                continue;
            }
            i += 1; // consume '%'
            if i >= len {
                return Err(pyrust_core::value_err!("incomplete format"));
            }

            // Named key: %(key)s — borrow a slice of fmt directly to avoid allocating.
            let named_key: Option<&str> = if bytes[i] == b'(' {
                i += 1;
                let start = i;
                while i < len && bytes[i] != b')' {
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
                match bytes[i] {
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
            let width: Option<usize> = if i < len && bytes[i] == b'*' {
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
            } else if i < len && bytes[i].is_ascii_digit() {
                let start = i;
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                Some(fmt[start..i].parse::<usize>().unwrap())
            } else {
                None
            };

            // Precision: .integer or .*
            let precision: Option<usize> = if i < len && bytes[i] == b'.' {
                i += 1;
                if i < len && bytes[i] == b'*' {
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
                    while i < len && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i == start {
                        Some(0)
                    } else {
                        Some(fmt[start..i].parse::<usize>().unwrap())
                    }
                }
            } else {
                None
            };

            // Length modifier: h, l, L — ignored (CPython ignores them too).
            if i < len && matches!(bytes[i], b'h' | b'l' | b'L') {
                i += 1;
            }

            if i >= len {
                return Err(pyrust_core::value_err!("incomplete format"));
            }
            let conv = bytes[i] as char;
            i += 1;

            // %% — literal percent, no argument consumed.
            if conv == '%' {
                out.push('%');
                continue;
            }

            // Get the argument value.
            let arg: Value = if let Some(key) = named_key {
                if is_mapping {
                    match args.kind() {
                        ValueKind::Dict(d) => {
                            let k = PyKey::Str(intern_string(key));
                            match d.get(&k) {
                                Some(v) => v.clone(),
                                None => {
                                    return Err(PyError::key_error(Value::string(key)));
                                }
                            }
                        }
                        // dict subclass / custom mapping: subscript via
                        // `__getitem__` so `__missing__` and a custom `KeyError`
                        // are honoured (issue #2089).
                        _ => self.eval_index(&args, Value::string(key))?,
                    }
                } else {
                    return Err(pyrust_core::type_err!("format requires a mapping"));
                }
            } else {
                str_printf_take_positional(&positional, &mut pos_idx)?
            };

            // Fast path: when no width padding is requested we don't need the
            // formatted length, so the two hottest conversions can render
            // straight into `out`, skipping the per-conversion temporary
            // `String` (and, for `%s` on a plain str, all allocation).
            if width.is_none() {
                match conv {
                    's' if precision.is_none() => {
                        if let ValueKind::Str(s) = arg.kind() {
                            out.push_str(s);
                            continue;
                        }
                    }
                    'd' | 'i' | 'u' if !flag_plus && !flag_space => {
                        if let ValueKind::Int(n) = arg.kind() {
                            use std::fmt::Write as _;
                            let _ = write!(out, "{n}");
                            continue;
                        }
                    }
                    _ => {}
                }
            }

            // Format the argument according to the conversion code.
            let formatted = self.str_printf_convert(
                conv,
                arg,
                precision,
                flag_plus,
                flag_space,
                flag_hash,
                i - 1,
                false,
            )?;

            // Apply width and alignment.
            let padded = apply_printf_width(formatted, width, flag_minus, flag_zero, conv);
            out.push_str(&padded);
        }

        // Unconsumed positional arguments: raise TypeError.
        if let Some(pos) = positional
            && pos_idx < pos.len()
        {
            return Err(pyrust_core::type_err!(
                "not all arguments converted during string formatting"
            ));
        }

        Ok(Value::string(out))
    }
}
