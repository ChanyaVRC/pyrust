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
                    // Codepoints, not `char`s: a lone-surrogate key is legal
                    // (#3315).
                    let s = sv.as_str().unwrap_or("");
                    let mut codepoints = cesu8_codepoints(s);
                    match (codepoints.next(), codepoints.next()) {
                        (Some(cp), None) => i64::from(cp),
                        _ => {
                            return Err(PyError::named(
                                "ValueError",
                                "string keys in translate table must be of length 1".to_string(),
                            ));
                        }
                    }
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
        // Codepoints, not `char`s, so lone surrogates map like any other
        // character (#3315).
        let x_cps: Vec<u32> = cesu8_codepoints(&x).collect();
        let y_cps: Vec<u32> = cesu8_codepoints(&y).collect();
        if x_cps.len() != y_cps.len() {
            return Err(PyError::named(
                "ValueError",
                "the first two maketrans arguments must have equal length".to_string(),
            ));
        }
        for (&cx, &cy) in x_cps.iter().zip(y_cps.iter()) {
            table.insert(PyKey::Int(i64::from(cx)), Value::int(i64::from(cy)));
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
            for cz in cesu8_codepoints(&z) {
                table.insert(PyKey::Int(i64::from(cz)), Value::none());
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
fn str_translate(s: &str, is_ascii: bool, args: &[Value]) -> Result<Value> {
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
    if !is_ascii && str_has_surrogates(s) {
        // Lone surrogates are not `char`s (decoding one is UB, #3315): look
        // up raw codepoints and re-push unmapped ones as CESU-8.
        for cp in cesu8_codepoints(s) {
            match dict.get(&PyKey::Int(i64::from(cp))) {
                None => push_codepoint(&mut out, cp),
                Some(v) => push_translation(v, &mut out)?,
            }
        }
    } else {
        for c in s.chars() {
            match dict.get(&PyKey::Int(c as i64)) {
                None => out.push(c),
                Some(v) => push_translation(v, &mut out)?,
            }
        }
    }
    Ok(Value::string(out))
}

/// Write the `str.translate` replacement `v` (a found `table[cp]` value) to
/// `out`.
fn push_translation(v: &Value, out: &mut String) -> Result<()> {
    match v.kind() {
        ValueKind::None => { /* delete */ }
        ValueKind::Int(n) => {
            if !(0..=0x10FFFF).contains(&n) {
                return Err(PyError::named(
                    "ValueError",
                    "character mapping must be in range(0x110000)".to_string(),
                ));
            }
            // CPython's str freely stores lone surrogates, so a surrogate
            // mapping value is written as its CESU-8 bytes.
            push_codepoint(out, n as u32);
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
            push_codepoint(out, u32::from(b));
        }
        ValueKind::Str(repl) => out.push_str(repl),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "character mapping must return integer, None or str".to_string(),
            ));
        }
    }
    Ok(())
}
