// Interpreter-aware `%` formatting.
impl Interpreter {
    /// Coerce a `PyInstance` argument to its int backing (for int subclasses) or
    /// call `__index__` (for objects that define it), ready for integer printf
    /// format codes (`%d`, `%i`, `%u`, `%o`, `%x`, `%X`).
    ///
    /// Non-`PyInstance` values are returned unchanged; `str_printf_to_int` will
    /// handle them (or raise `TypeError`) as before.  This mirrors CPython's
    /// `PyNumber_Index` pre-coercion that happens before `formatlong`.
    fn coerce_printf_int_arg(&mut self, val: Value) -> Result<Value> {
        // Use a tag enum so the borrow from val.kind() ends before we move val.
        enum Tag {
            Instance(Rc<RefCell<PyInstance>>),
            Other,
        }
        let tag = match val.kind() {
            ValueKind::PyInstance(inst) => Tag::Instance(Rc::clone(inst)),
            _ => Tag::Other,
        };
        let inst_rc = match tag {
            Tag::Other => return Ok(val),
            Tag::Instance(rc) => rc,
        };
        // Int subclass: extract the backing primitive (Int or BigInt).
        if let Some(backing) = builtin_data_backing(&val)
            && matches!(
                backing.kind(),
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            )
        {
            return Ok(backing);
        }
        // Non-int-subclass: look for __index__.
        let class = Rc::clone(&inst_rc.borrow().class);
        let Some(method_val) = lookup_class_attr(&class, "__index__") else {
            // No backing and no __index__: return original; str_printf_to_int
            // will produce the correct TypeError.
            return Ok(val);
        };
        let result = invoke_class_method(
            self,
            method_val,
            Value::py_instance(Rc::clone(&inst_rc)),
            &[],
        )?;
        // CPython: if __index__ returns non-int, the printf format code falls
        // back to its standard error ("a real number is required, not Foo").
        // Return val unchanged so str_printf_to_int produces the right message.
        let is_int = matches!(
            result.kind(),
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
        );
        if is_int { Ok(result) } else { Ok(val) }
    }

    /// Coerce a Python user object to `f64` for float printf format codes
    /// (`%e`, `%E`, `%f`, `%F`, `%g`, `%G`), including class objects whose
    /// metaclass supplies the numeric slots.
    ///
    /// Tries `__float__` first (float subclasses carry a float backing value),
    /// then `__index__` (int-like objects acceptable as float arguments).
    /// Other values are returned unchanged.
    fn coerce_printf_float_arg(&mut self, val: Value) -> Result<Value> {
        if !matches!(val.kind(), ValueKind::PyInstance(_) | ValueKind::PyClass(_)) {
            return Ok(val);
        }
        // Float or int subclass: extract the backing primitive directly.
        if let Some(backing) = builtin_data_backing(&val)
            && matches!(
                backing.kind(),
                ValueKind::Float(_) | ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            )
        {
            return Ok(backing);
        }
        // Try __float__ first.
        if let Some(method_val) = lookup_value_special_method(&val, "__float__").transpose()? {
            let result = invoke_class_method(self, method_val, val.clone(), &[])?;
            if let Some(normalized) = normalize_float_slot_result(&result) {
                return Ok(normalized);
            }
            return Err(pyrust_core::type_err!(
                "{}.__float__ returned non-float (type {})",
                value_type_name_str(&val),
                value_type_name_str(&result),
            ));
        }
        // Try __index__ as fallback (CPython accepts integer-like objects for %f).
        if let Some(result) = self.try_value_to_index(&val)? {
            return Ok(result);
        }
        // No __float__, no __index__: return original; str_printf_to_float
        // will produce the correct TypeError.
        Ok(val)
    }
    /// Format one `%`-conversion argument — the `match conv` body lifted out
    /// of `str_printf_format`.  `conv_index` is the 0-based source index of
    /// `conv`, used only for the unsupported-conversion error message.
    #[allow(clippy::too_many_arguments)]
    fn str_printf_convert(
        &mut self,
        conv: char,
        arg: Value,
        precision: Option<usize>,
        flag_plus: bool,
        flag_space: bool,
        flag_hash: bool,
        conv_index: usize,
        bytes_mode: bool,
    ) -> Result<String> {
        Ok(match conv {
            's' => apply_str_precision(self.render_value_as_str(&arg)?, precision),
            'r' => apply_str_precision(render_instance_repr(self, &arg)?, precision),
            // %a — ascii repr (like the ascii() builtin); mirrors %r but escapes
            // non-ASCII. The bytes path already implements this; the str path was
            // missing the arm (#2073).
            'a' => apply_str_precision(ascii_repr_interp(self, &arg)?, precision),
            'd' | 'i' | 'u' => {
                let coerced_int = self.coerce_printf_int_arg(arg)?;
                match str_printf_to_int(&coerced_int, conv, bytes_mode)? {
                    PrintfInt::Small(n) => {
                        if n < 0 {
                            format!("{}", n)
                        } else if flag_plus {
                            format!("+{}", n)
                        } else if flag_space {
                            format!(" {}", n)
                        } else {
                            format!("{}", n)
                        }
                    }
                    PrintfInt::Big(b) => {
                        // gh-95778: %d/%i/%u render base 10 — subject to
                        // int_max_str_digits (%x/%o are exempt).
                        if pyrust_core::bigint_str_digits_exceed_limit(&b) {
                            return Err(pyrust_core::int_max_str_digits_format_error());
                        }
                        // to_str_radix(10) includes the '-' sign for negatives.
                        let mut s = b.to_str_radix(10);
                        if !s.starts_with('-') && flag_plus {
                            s.insert(0, '+');
                        } else if !s.starts_with('-') && flag_space {
                            s.insert(0, ' ');
                        }
                        s
                    }
                }
            }
            'o' => {
                let coerced_int = self.coerce_printf_int_arg(arg)?;
                match str_printf_to_int(&coerced_int, conv, bytes_mode)? {
                    PrintfInt::Small(n) => {
                        if n < 0 {
                            // CPython uses sign-magnitude (not two's complement) for negative octal.
                            let u = (n as u64).wrapping_neg();
                            if flag_hash {
                                format!("-0o{:o}", u)
                            } else {
                                format!("-{:o}", u)
                            }
                        } else if flag_hash {
                            // CPython applies 0o prefix for all values (including 0) when # is set.
                            if flag_plus {
                                format!("+0o{:o}", n)
                            } else if flag_space {
                                format!(" 0o{:o}", n)
                            } else {
                                format!("0o{:o}", n)
                            }
                        } else if flag_plus {
                            format!("+{:o}", n)
                        } else if flag_space {
                            format!(" {:o}", n)
                        } else {
                            format!("{:o}", n)
                        }
                    }
                    PrintfInt::Big(b) => format_printf_bigint_radix(
                        &b, 8, "0o", false, flag_hash, flag_plus, flag_space,
                    ),
                }
            }
            'x' => {
                let coerced_int = self.coerce_printf_int_arg(arg)?;
                match str_printf_to_int(&coerced_int, conv, bytes_mode)? {
                    PrintfInt::Small(n) => {
                        if n < 0 {
                            let u = (n as u64).wrapping_neg();
                            if flag_hash {
                                format!("-0x{:x}", u)
                            } else {
                                format!("-{:x}", u)
                            }
                        } else if flag_hash {
                            // CPython applies 0x prefix for all values (including 0) when # is set.
                            if flag_plus {
                                format!("+0x{:x}", n)
                            } else if flag_space {
                                format!(" 0x{:x}", n)
                            } else {
                                format!("0x{:x}", n)
                            }
                        } else if flag_plus {
                            format!("+{:x}", n)
                        } else if flag_space {
                            format!(" {:x}", n)
                        } else {
                            format!("{:x}", n)
                        }
                    }
                    PrintfInt::Big(b) => format_printf_bigint_radix(
                        &b, 16, "0x", false, flag_hash, flag_plus, flag_space,
                    ),
                }
            }
            'X' => {
                let coerced_int = self.coerce_printf_int_arg(arg)?;
                match str_printf_to_int(&coerced_int, conv, bytes_mode)? {
                    PrintfInt::Small(n) => {
                        if n < 0 {
                            let u = (n as u64).wrapping_neg();
                            if flag_hash {
                                format!("-0X{:X}", u)
                            } else {
                                format!("-{:X}", u)
                            }
                        } else if flag_hash {
                            // CPython applies 0X prefix for all values (including 0) when # is set.
                            if flag_plus {
                                format!("+0X{:X}", n)
                            } else if flag_space {
                                format!(" 0X{:X}", n)
                            } else {
                                format!("0X{:X}", n)
                            }
                        } else if flag_plus {
                            format!("+{:X}", n)
                        } else if flag_space {
                            format!(" {:X}", n)
                        } else {
                            format!("{:X}", n)
                        }
                    }
                    PrintfInt::Big(b) => format_printf_bigint_radix(
                        &b, 16, "0X", true, flag_hash, flag_plus, flag_space,
                    ),
                }
            }
            'e' | 'E' => {
                let coerced_float = self.coerce_printf_float_arg(arg)?;
                let f = str_printf_to_float(&coerced_float, conv, bytes_mode)?;
                let prec = precision.unwrap_or(6);
                let mut s = format_scientific(f, prec, conv == 'E');
                // Alt-form (#) with precision 0 keeps the decimal point even
                // though no fractional digits are emitted: "3.e+00" (#2029).
                // Non-finite values (inf/nan) never get a point.
                if flag_hash
                    && prec == 0
                    && f.is_finite()
                    && let Some(e_pos) = s.find(['e', 'E'])
                {
                    s.insert(e_pos, '.');
                }
                if f.is_sign_positive() && flag_plus {
                    s.insert(0, '+');
                } else if f.is_sign_positive() && flag_space {
                    s.insert(0, ' ');
                }
                s
            }
            'f' | 'F' => {
                let coerced_float = self.coerce_printf_float_arg(arg)?;
                let f = str_printf_to_float(&coerced_float, conv, bytes_mode)?;
                let upper = conv == 'F';
                // Special-case NaN and Inf before calling format!(), which
                // produces Rust-style 'NaN'/'inf' rather than CPython-style
                // 'nan'/'inf'/'NAN'/'INF'.
                let mut s = if f.is_nan() {
                    if upper {
                        "NAN".to_string()
                    } else {
                        "nan".to_string()
                    }
                } else if f.is_infinite() {
                    if f > 0.0 {
                        if upper {
                            "INF".to_string()
                        } else {
                            "inf".to_string()
                        }
                    } else if upper {
                        "-INF".to_string()
                    } else {
                        "-inf".to_string()
                    }
                } else {
                    let prec = precision.unwrap_or(6);
                    let mut body = format!("{:.prec$}", f, prec = prec);
                    // Alt-form (#) with precision 0 keeps a trailing decimal
                    // point: "3." rather than "3" (#2029).
                    if flag_hash && prec == 0 {
                        body.push('.');
                    }
                    body
                };
                if f.is_sign_positive() && flag_plus {
                    s.insert(0, '+');
                } else if f.is_sign_positive() && flag_space {
                    s.insert(0, ' ');
                }
                s
            }
            'g' | 'G' => {
                let coerced_float = self.coerce_printf_float_arg(arg)?;
                let f = str_printf_to_float(&coerced_float, conv, bytes_mode)?;
                let prec = precision.unwrap_or(6).max(1);
                let mut s = format_general_float(f, prec, conv == 'G', flag_hash);
                if f.is_sign_positive() && flag_plus {
                    s.insert(0, '+');
                } else if f.is_sign_positive() && flag_space {
                    s.insert(0, ' ');
                }
                s
            }
            'c' => {
                // Coerce int subclasses and __index__ objects the same way
                // as %d/%x etc.  If __index__ returns non-int, we fall back
                // to the original value so the match below emits the correct
                // "%c requires int or char" TypeError.
                let coerced_char = self.coerce_printf_int_arg(arg)?;
                match coerced_char.kind() {
                    ValueKind::Str(s) => {
                        let mut cs = s.chars();
                        let c = cs
                            .next()
                            .ok_or_else(|| pyrust_core::type_err!("%c requires int or char"))?;
                        if cs.next().is_some() {
                            return Err(pyrust_core::type_err!("%c requires a single character"));
                        }
                        c.to_string()
                    }
                    ValueKind::Int(n) => char::from_u32(n as u32)
                        .ok_or_else(|| pyrust_core::overflow_err!("%c arg not in range(0x110000)"))?
                        .to_string(),
                    ValueKind::Bool(b) => char::from_u32(b as u32)
                        .ok_or_else(|| pyrust_core::overflow_err!("%c arg not in range(0x110000)"))?
                        .to_string(),
                    ValueKind::BigInt(b) => {
                        // A BigInt may be in range [0, 0x10ffff] or not.
                        use crate::value::PyToPrimitive;
                        let n = b.to_u32();
                        let c = n.and_then(char::from_u32).ok_or_else(|| {
                            pyrust_core::overflow_err!("%c arg not in range(0x110000)")
                        })?;
                        c.to_string()
                    }
                    _ => {
                        return Err(pyrust_core::type_err!("%c requires int or char"));
                    }
                }
            }
            _ => {
                return Err(pyrust_core::value_err!(
                    "unsupported format character '{}' (0x{:02x}) at index {}",
                    conv,
                    conv as u32,
                    conv_index
                ));
            }
        })
    }
}
