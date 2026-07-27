/// `str.maketrans(x[, y[, z]])` — static method.
///
/// 1-arg form: x must be a dict mapping (int ordinal | single-char str | None key) → replacement.
/// 2-arg form: x and y must be equal-length strings; returns {ord(c): ord(d) for c,d in zip(x,y)}.
/// 3-arg form: same as 2-arg, plus {ord(c): None for c in z}.
///
/// Returns a dict with integer keys (codepoint ordinals).
pub fn str_maketrans(args: &[Value]) -> Result<Value> {
    if args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "maketrans expected at least 1 argument, got 0".to_string(),
        ));
    }
    if args.len() > 3 {
        return Err(PyError::named(
            "TypeError",
            format!("maketrans expected at most 3 arguments, got {}", args.len()),
        ));
    }

    let mut table: PyDict = PyDict::default();

    if args.len() == 1 {
        // 1-arg form: x must be a dict
        let dict = match args[0].kind() {
            ValueKind::Dict(d) => d,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "if you give only one argument to maketrans it must be a dict".to_string(),
                ));
            }
        };
        for (k, v) in dict.iter() {
            let ordinal: i64 = match k {
                PyKey::Int(n) => *n,
                PyKey::Bool(b) => *b as i64,
                PyKey::Str(sv) => {
                    let s = sv.as_str().unwrap_or("");
                    let mut chars = s.chars();
                    let first = chars.next().ok_or_else(|| {
                        PyError::named(
                            "ValueError",
                            "string keys in translate table must be of length 1".to_string(),
                        )
                    })?;
                    if chars.next().is_some() {
                        return Err(PyError::named(
                            "ValueError",
                            "string keys in translate table must be of length 1".to_string(),
                        ));
                    }
                    first as i64
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "keys in translate table must be strings or integers".to_string(),
                    ));
                }
            };
            table.insert(PyKey::Int(ordinal), v.clone());
        }
    } else {
        // 2-arg or 3-arg form: x and y must be equal-length strings
        let x = match args[0].kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "first maketrans argument must be a string if there is a second argument"
                        .to_string(),
                ));
            }
        };
        let y = match args[1].kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "maketrans() argument 2 must be str, not {}",
                        py_value_display_name(&args[1])
                    ),
                ));
            }
        };
        let x_chars: Vec<char> = x.chars().collect();
        let y_chars: Vec<char> = y.chars().collect();
        if x_chars.len() != y_chars.len() {
            return Err(PyError::named(
                "ValueError",
                "the first two maketrans arguments must have equal length".to_string(),
            ));
        }
        for (cx, cy) in x_chars.iter().zip(y_chars.iter()) {
            table.insert(PyKey::Int(*cx as i64), Value::int(*cy as i64));
        }
        if args.len() == 3 {
            let z = match args[2].kind() {
                ValueKind::Str(s) => s.to_string(),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "maketrans() argument 3 must be str, not {}",
                            py_value_display_name(&args[2])
                        ),
                    ));
                }
            };
            for cz in z.chars() {
                table.insert(PyKey::Int(cz as i64), Value::none());
            }
        }
    }

    Ok(Value::dict(table))
}

/// `str.translate(table)` — instance method.
///
/// Iterates over the Unicode codepoints of self. For each codepoint `cp`,
/// looks up `cp` in `table` (a dict with int keys, e.g. from `str.maketrans`):
/// - absent → keep character as-is
/// - `None`  → delete character
/// - `int`   → replace with `chr(int)`
/// - `str`   → replace with that string
fn str_translate(s: &str, args: &[Value]) -> Result<Value> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "str.translate() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    }
    let dict = match args[0].kind() {
        ValueKind::Dict(d) => d,
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "translate() argument must be a dict or mapping, not {}",
                    builtin_type_name(&args[0])
                ),
            ));
        }
    };

    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as i64;
        match dict.get(&PyKey::Int(cp)) {
            None => out.push(c),
            Some(v) => match v.kind() {
                ValueKind::None => { /* delete */ }
                ValueKind::Int(n) => {
                    if !(0..=0x10FFFF).contains(&n) {
                        return Err(PyError::named(
                            "ValueError",
                            "character mapping must be in range(0x110000)".to_string(),
                        ));
                    }
                    let cp = n as u32;
                    if (0xD800..=0xDFFF).contains(&cp) {
                        // Lone surrogates are not Unicode scalar values, so
                        // char::from_u32 rejects them. CPython's str type freely
                        // stores lone surrogates; match that by writing the
                        // CESU-8 three-byte sequence directly into the buffer.
                        // SAFETY: we hold &mut String's backing Vec exclusively,
                        // and the three bytes we push are a well-formed CESU-8
                        // encoding of a surrogate codepoint. Every other write to
                        // `out` goes through safe String methods, so the rest of
                        // the buffer is valid UTF-8. The combined byte sequence is
                        // the same representation pyrust uses for surrogate-
                        // containing strings throughout the runtime.
                        unsafe {
                            out.as_mut_vec().extend_from_slice(&[
                                0xE0 | (cp >> 12) as u8,
                                0x80 | ((cp >> 6) & 0x3F) as u8,
                                0x80 | (cp & 0x3F) as u8,
                            ]);
                        }
                    } else {
                        // Non-surrogate codepoints in 0..=0x10FFFF are valid
                        // Unicode scalar values; from_u32 is safe here.
                        let replacement = char::from_u32(cp)
                            .expect("non-surrogate in 0..=0x10FFFF is a valid char");
                        out.push(replacement);
                    }
                }
                ValueKind::BigInt(_) => {
                    // A BigInt is always outside the valid codepoint range
                    // 0..=0x10FFFF, so it can never be a legal mapping value.
                    return Err(PyError::named(
                        "ValueError",
                        "character mapping must be in range(0x110000)".to_string(),
                    ));
                }
                ValueKind::Bool(b) => {
                    // bool is a subclass of int; False=0 (NUL), True=1 (SOH)
                    let replacement =
                        char::from_u32(b as u32).expect("0 and 1 are valid codepoints");
                    out.push(replacement);
                }
                ValueKind::Str(repl) => {
                    out.push_str(repl);
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "character mapping must return integer, None or str".to_string(),
                    ));
                }
            },
        }
    }
    Ok(Value::string(out))
}
