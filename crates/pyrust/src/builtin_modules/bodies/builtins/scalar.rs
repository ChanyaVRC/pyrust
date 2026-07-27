use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: chr(i) — return the string of one Unicode codepoint i.
    /// <https://docs.python.org/3/library/functions.html#chr>
    ///
    /// Migrated to the typed-signature dialect (#400) as a three-element
    /// overload set: `PyInt` is the primary path; `PyBool` mirrors
    /// CPython's `bool ⊆ int` subtyping (`chr(True) == '\x01'`), since
    /// strict `PyInt` doesn't auto-coerce `bool` in the typed dialect.
    /// A trailing `PyValue` catch-all resolves a user object's `__index__`
    /// (CPython 3.12 honors the index protocol for `chr`, #1908) and
    /// otherwise raises `"'X' object cannot be interpreted as an integer"`,
    /// matching CPython 3.12 verbatim.  All parameters are
    /// `#[positional_only]` so the macro's positional-only fast-path
    /// applies (no kwarg-validation work).  Bignum inputs that don't fit
    /// in i64 raise `OverflowError("Python int too large to convert to C
    /// int")` — matching CPython 3.12's exact wording (#1584).  Values
    /// that fit in i64 but exceed the Unicode range raise `ValueError` via
    /// `chr_from_code_point`.
    #[arity_style(takes_exactly_one)]
    fn chr(#[positional_only] i: PyInt) -> Result<Value> {
        let code_point = i.as_i64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
            )
        })?;
        chr_from_code_point(code_point)
    }

    #[arity_style(takes_exactly_one)]
    fn chr(#[positional_only] i: PyBool) -> Result<Value> {
        // CPython: `chr(True) == '\x01'`, `chr(False) == '\x00'`.
        chr_from_code_point(if i.0 { 1 } else { 0 })
    }

    #[arity_style(takes_exactly_one)]
    fn chr(#[positional_only] i: PyValue) -> Result<Value> {
        // CPython 3.12: chr() honors the __index__ protocol. A plain int /
        // bool is handled by the typed overloads above; here we resolve a
        // user object's __index__ (mirroring bin/oct/hex) via the shared
        // index protocol (issue #2022), then apply the same range check as
        // the PyInt path. __int__ alone is not enough.
        let resolved = _interp.value_to_index(&i.0, not_an_integer_err)?;
        match resolved.kind() {
            ValueKind::Bool(b) => chr_from_code_point(if b { 1 } else { 0 }),
            ValueKind::Int(v) => chr_from_code_point(v),
            // A bignum codepoint always exceeds the Unicode range; CPython's
            // chr() converts via a C int first, so it overflows there.
            ValueKind::BigInt(_) => Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
            )),
            _ => unreachable!("value_to_index guarantees an integer"),
        }
    }

    /// CPython: ord(c) — return the Unicode codepoint of a one-character string.
    /// <https://docs.python.org/3/library/functions.html#ord>
    ///
    /// Migrated to the typed-signature dialect (#400) as a three-element
    /// overload set: `PyStr` is the primary path; `PyBytes` mirrors
    /// CPython's acceptance of one-byte bytes (`ord(b"A") == 65`); a
    /// trailing `PyValue` catch-all raises `"expected string of length 1,
    /// but TYPE found"` using the actual type name (CPython 3.12 wording,
    /// fixed in #1339).  Length-mismatch wording on the `PyStr` overload
    /// is preserved verbatim from CPython so parity output is stable.
    /// All parameters are `#[positional_only]` so the macro's
    /// positional-only fast-path applies.  The `PyBytes` overload is a
    /// new CPython-parity feature — the legacy body rejected `bytes`
    /// outright, but CPython has always accepted a 1-byte `bytes`
    /// (`ord(b"A") == 65`).
    #[arity_style(takes_exactly_one)]
    fn ord(#[positional_only] c: PyStr) -> Result<Value> {
        let s: &str = &c;
        // Use the surrogate-safe codepoint iterator, not `str::chars()`:
        // a string built from a lone-surrogate escape (`"\udc80"`) is stored
        // CESU-8, and `chars()` would feed the surrogate to
        // `char::from_u32_unchecked` → UB / debug-build abort (issue #1893).
        let mut cps = pyrust_core::cesu8_codepoints(s);
        let first = cps.next();
        let second = cps.next();
        match (first, second) {
            (Some(cp), None) => Ok(Value::int(cp as i64)),
            (None, _) => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() expected a character, but string of length 0 found"),
            )),
            (Some(_), Some(_)) => Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() expected a character, but string of length {} found",
                    pyrust_core::cesu8_codepoints(s).count()
                ),
            )),
        }
    }

    #[arity_style(takes_exactly_one)]
    fn ord(#[positional_only] c: PyBytes) -> Result<Value> {
        // CPython: `ord(b"A") == 65`; reject empty/multi-byte with the
        // same wording shape used by the `PyStr` overload above.
        match c.0.as_slice() {
            [b] => Ok(Value::int(*b as i64)),
            other => Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() expected a character, but string of length {} found",
                    other.len()
                ),
            )),
        }
    }

    #[arity_style(takes_exactly_one)]
    fn ord(#[positional_only] c: PyValue) -> Result<Value> {
        Err(PyError::named(
            "TypeError",
            format!(
                "{FN_NAME}() expected string of length 1, but {} found",
                value_type_name_str(&c.0),
            ),
        ))
    }

    /// CPython: bin(x) — integer to '0b…' / '-0b…' string.
    /// <https://docs.python.org/3/library/functions.html#bin>
    ///
    /// Migrated to the typed-signature dialect (#400) mirroring `hex`'s
    /// 3-overload pattern: `PyInt` is the primary path, `PyBool` mirrors
    /// CPython's `bool ⊆ int` subtyping, and a trailing `PyValue`
    /// catch-all reproduces CPython's exact "'X' object cannot be
    /// interpreted as an integer" TypeError wording verbatim.
    /// Both small (`i64`) and BigInt arguments are handled (#1226).
    ///
    /// `#[arity_style(takes_exactly_one)]` (#400/#2331) reproduces the
    /// METH_O wording `bin() takes exactly one argument (N given)`.
    #[arity_style(takes_exactly_one)]
    fn bin(#[positional_only] x: PyInt) -> Result<Value> {
        if let Some(v) = x.as_i64() {
            Ok(Value::string(format_bin_i64(v)))
        } else {
            Ok(Value::string(format_bigint_radix(&x.to_bigint(), 2, "0b")))
        }
    }

    #[arity_style(takes_exactly_one)]
    fn bin(#[positional_only] x: PyBool) -> Result<Value> {
        // CPython: `bin(True) == '0b1'`, `bin(False) == '0b0'`.
        Ok(Value::string(format_bin_i64(if x.0 { 1 } else { 0 })))
    }

    #[arity_style(takes_exactly_one)]
    fn bin(#[positional_only] x: PyValue) -> Result<Value> {
        // Issue #1929 / #2022: resolve int/bool subclasses and `__index__`
        // objects uniformly via the shared index protocol, then format.
        let resolved = _interp.value_to_index(&x.0, not_an_integer_err)?;
        Ok(Value::string(format_index_radix(&resolved, 2, "0b", format_bin_i64)))
    }

    /// CPython: oct(x) — integer to '0o…' / '-0o…' string.
    /// <https://docs.python.org/3/library/functions.html#oct>
    ///
    /// Migrated to the typed-signature dialect (#400) mirroring `hex`'s
    /// 3-overload pattern: `PyInt` is the primary path, `PyBool` mirrors
    /// CPython's `bool ⊆ int` subtyping, and a trailing `PyValue`
    /// catch-all reproduces CPython's exact "'X' object cannot be
    /// interpreted as an integer" TypeError wording verbatim.
    /// Both small (`i64`) and BigInt arguments are handled (#1226).
    ///
    /// `#[arity_style(takes_exactly_one)]` (#400/#2331) reproduces the
    /// METH_O wording `oct() takes exactly one argument (N given)`.
    #[arity_style(takes_exactly_one)]
    fn oct(#[positional_only] x: PyInt) -> Result<Value> {
        if let Some(v) = x.as_i64() {
            Ok(Value::string(format_oct_i64(v)))
        } else {
            Ok(Value::string(format_bigint_radix(&x.to_bigint(), 8, "0o")))
        }
    }

    #[arity_style(takes_exactly_one)]
    fn oct(#[positional_only] x: PyBool) -> Result<Value> {
        // CPython: `oct(True) == '0o1'`, `oct(False) == '0o0'`.
        Ok(Value::string(format_oct_i64(if x.0 { 1 } else { 0 })))
    }

    #[arity_style(takes_exactly_one)]
    fn oct(#[positional_only] x: PyValue) -> Result<Value> {
        // Issue #1929 / #2022: resolve via the shared index protocol, format.
        let resolved = _interp.value_to_index(&x.0, not_an_integer_err)?;
        Ok(Value::string(format_index_radix(&resolved, 8, "0o", format_oct_i64)))
    }

    /// CPython: hex(x) — integer to '0x…' / '-0x…' string.
    /// <https://docs.python.org/3/library/functions.html#hex>
    ///
    /// Migrated to the typed-signature dialect (#400) as a two-element
    /// overload set: `PyInt` is the primary path; `PyBool` mirrors
    /// CPython's `bool ⊆ int` subtyping (without it, strict `PyInt`
    /// would reject `hex(True)` — `bool` doesn't auto-coerce in the
    /// typed dialect, see [`builtin_args`]).  A trailing `PyValue`
    /// catch-all reproduces CPython's exact "'X' object cannot be
    /// interpreted as an integer" TypeError wording (the macro's own
    /// "unsupported argument type(s)" fallback would drift from the
    /// canonical message — preserved verbatim from the legacy body).
    /// Both small (`i64`) and BigInt arguments are handled (#1226).
    ///
    /// `#[arity_style(takes_exactly_one)]` (#400/#2331) reproduces the
    /// METH_O wording `hex() takes exactly one argument (N given)`.
    #[arity_style(takes_exactly_one)]
    fn hex(#[positional_only] x: PyInt) -> Result<Value> {
        if let Some(v) = x.as_i64() {
            Ok(Value::string(format_hex_i64(v)))
        } else {
            Ok(Value::string(format_bigint_radix(&x.to_bigint(), 16, "0x")))
        }
    }

    #[arity_style(takes_exactly_one)]
    fn hex(#[positional_only] x: PyBool) -> Result<Value> {
        // CPython: `hex(True) == '0x1'`, `hex(False) == '0x0'`.
        Ok(Value::string(format_hex_i64(if x.0 { 1 } else { 0 })))
    }

    #[arity_style(takes_exactly_one)]
    fn hex(#[positional_only] x: PyValue) -> Result<Value> {
        // Issue #1929 / #2022: resolve via the shared index protocol, format.
        let resolved = _interp.value_to_index(&x.0, not_an_integer_err)?;
        Ok(Value::string(format_index_radix(&resolved, 16, "0x", format_hex_i64)))
    }
}
