/// Encode a Python `str` to `bytes`.
///
/// Supports `utf-8`, `ascii`, `latin-1` (and CPython aliases).
/// Other encoding names raise `LookupError: unknown encoding: <name>`.
///
/// `errors="strict"` raises `UnicodeEncodeError` on unencodable characters;
/// `"ignore"` silently drops them; `"replace"` substitutes `b'?'`;
/// `"backslashreplace"` substitutes `\xHH`, `\uHHHH`, or `\UHHHHHHHH`;
/// `"xmlcharrefreplace"` substitutes `&#NNN;` (decimal codepoint);
/// `"namereplace"` substitutes `\N{NAME}`, falling back to the backslash form.
pub fn encode_str_to_bytes(source: &str, encoding: &str, errors: &str) -> Result<Value> {
    fn normalize(name: &str) -> String {
        name.to_ascii_lowercase().replace('_', "-")
    }
    let canonical = normalize(encoding);

    enum Handler {
        Strict,
        Ignore,
        Replace,
        BackslashReplace,
        XmlCharRefReplace,
        NameReplace,
        SurrogatePass,
    }

    fn resolve_handler(errors: &str) -> Result<Handler> {
        match errors {
            "strict" => Ok(Handler::Strict),
            "ignore" => Ok(Handler::Ignore),
            "replace" => Ok(Handler::Replace),
            "backslashreplace" => Ok(Handler::BackslashReplace),
            "xmlcharrefreplace" => Ok(Handler::XmlCharRefReplace),
            "namereplace" => Ok(Handler::NameReplace),
            "surrogatepass" => Ok(Handler::SurrogatePass),
            other => Err(PyError::named(
                "LookupError",
                format!("unknown error handler name '{other}'"),
            )),
        }
    }

    /// True for a lone surrogate codepoint (U+D800..=U+DFFF).
    fn is_surrogate(cp: u32) -> bool {
        (0xD800..=0xDFFF).contains(&cp)
    }

    /// Produce the backslash-escape bytes for a single unencodable codepoint.
    /// `\xHH` for cp < 0x100, `\uHHHH` for cp < 0x10000, `\UHHHHHHHH` otherwise.
    fn backslash_escape_bytes(cp: u32) -> Vec<u8> {
        if cp < 0x100 {
            format!("\\x{:02x}", cp).into_bytes()
        } else if cp < 0x10000 {
            format!("\\u{:04x}", cp).into_bytes()
        } else {
            format!("\\U{:08x}", cp).into_bytes()
        }
    }

    fn encode_single_byte_codec(
        source: &str,
        codec_name: &str,
        fits: impl Fn(u32) -> bool,
        range_label: &str,
        errors: &str,
    ) -> Result<Value> {
        // Iterate codepoints via cesu8_codepoints so surrogate bytes never reach
        // char::chars() (a debug-abort hazard on the CESU-8 strings pyrust stores).
        let cps: Vec<u32> = cesu8_codepoints(source).collect();
        let mut out = Vec::with_capacity(source.len());
        let mut idx = 0usize;
        while idx < cps.len() {
            let cp = cps[idx];
            if fits(cp) {
                out.push(cp as u8);
                idx += 1;
            } else {
                match resolve_handler(errors)? {
                    Handler::Ignore => {
                        idx += 1;
                    }
                    Handler::Replace => {
                        out.push(b'?');
                        idx += 1;
                    }
                    Handler::BackslashReplace => {
                        out.extend_from_slice(&backslash_escape_bytes(cp));
                        idx += 1;
                    }
                    Handler::XmlCharRefReplace => {
                        out.extend_from_slice(format!("&#{};", cp).as_bytes());
                        idx += 1;
                    }
                    Handler::NameReplace => {
                        // Surrogates (from_u32 == None) have no name; fall back to
                        // the backslash escape form.
                        let replacement = match char::from_u32(cp).and_then(unicode_names2::name) {
                            Some(name) => format!("\\N{{{}}}", name).into_bytes(),
                            None => backslash_escape_bytes(cp),
                        };
                        out.extend_from_slice(&replacement);
                        idx += 1;
                    }
                    // For single-byte codecs CPython treats surrogatepass like
                    // strict (the surrogate still doesn't fit the byte range).
                    Handler::Strict | Handler::SurrogatePass => {
                        let run_start = idx;
                        let mut run_end = idx + 1;
                        while run_end < cps.len() && !fits(cps[run_end]) {
                            run_end += 1;
                        }
                        return Err(PyError::UnicodeEncodeError {
                            encoding: codec_name.to_string(),
                            object: source.to_string(),
                            start: run_start,
                            end: run_end,
                            reason: format!("ordinal not in range({range_label})"),
                        });
                    }
                }
            }
        }
        Ok(Value::bytes(out))
    }

    /// Surrogate-aware UTF-N encoder.
    ///
    /// Iterates codepoints via `cesu8_codepoints` (never `str::chars`, which would
    /// abort on the CESU-8 surrogate bytes pyrust stores), encoding non-surrogate
    /// codepoints with `encode_cp` and applying the resolved `errors` handler to
    /// lone surrogates.  `surrogate_bytes` produces the raw UTF-N bytes emitted by
    /// the `surrogatepass` handler.  Error positions are codepoint indices,
    /// matching CPython; consecutive surrogates are coalesced into one error run.
    fn encode_utf_surrogate_aware(
        source: &str,
        codec_name: &str,
        bom: &[u8],
        mut encode_cp: impl FnMut(u32, &mut Vec<u8>),
        surrogate_bytes: impl Fn(u32) -> Vec<u8>,
        errors: &str,
    ) -> Result<Value> {
        let cps: Vec<u32> = cesu8_codepoints(source).collect();
        let mut out = Vec::with_capacity(bom.len() + source.len());
        out.extend_from_slice(bom);
        let mut idx = 0usize;
        while idx < cps.len() {
            let cp = cps[idx];
            if !is_surrogate(cp) {
                encode_cp(cp, &mut out);
                idx += 1;
                continue;
            }
            match resolve_handler(errors)? {
                Handler::SurrogatePass => {
                    out.extend_from_slice(&surrogate_bytes(cp));
                    idx += 1;
                }
                Handler::Ignore => {
                    idx += 1;
                }
                Handler::Replace => {
                    out.push(b'?');
                    idx += 1;
                }
                Handler::BackslashReplace => {
                    out.extend_from_slice(&backslash_escape_bytes(cp));
                    idx += 1;
                }
                Handler::XmlCharRefReplace => {
                    out.extend_from_slice(format!("&#{};", cp).as_bytes());
                    idx += 1;
                }
                Handler::NameReplace => {
                    // Surrogates have no Unicode name; fall back to the backslash form.
                    out.extend_from_slice(&backslash_escape_bytes(cp));
                    idx += 1;
                }
                Handler::Strict => {
                    let run_start = idx;
                    let mut run_end = idx + 1;
                    while run_end < cps.len() && is_surrogate(cps[run_end]) {
                        run_end += 1;
                    }
                    return Err(PyError::UnicodeEncodeError {
                        encoding: codec_name.to_string(),
                        object: source.to_string(),
                        start: run_start,
                        end: run_end,
                        reason: "surrogates not allowed".to_string(),
                    });
                }
            }
        }
        Ok(Value::bytes(out))
    }

    // UTF-16 surrogatepass: emit the surrogate as a single 16-bit code unit.
    fn utf16_surrogate_bytes(cp: u32, big_endian: bool) -> Vec<u8> {
        let unit = cp as u16;
        if big_endian {
            unit.to_be_bytes().to_vec()
        } else {
            unit.to_le_bytes().to_vec()
        }
    }

    // Encode a non-surrogate scalar codepoint into UTF-16 code units (LE or BE).
    fn encode_cp_utf16(cp: u32, big_endian: bool, out: &mut Vec<u8>) {
        let c = char::from_u32(cp).expect("non-surrogate codepoint is a valid char");
        let mut buf = [0u16; 2];
        for unit in c.encode_utf16(&mut buf) {
            if big_endian {
                out.extend_from_slice(&unit.to_be_bytes());
            } else {
                out.extend_from_slice(&unit.to_le_bytes());
            }
        }
    }

    match canonical.as_str() {
        "utf-8" | "utf8" | "u8" | "utf" => {
            // Fast path: a string with no 0xED byte cannot contain a CESU-8 lone
            // surrogate, so its bytes are already valid UTF-8 — copy them directly.
            if !source.as_bytes().contains(&0xED) {
                return Ok(Value::bytes(source.as_bytes().to_vec()));
            }
            encode_utf_surrogate_aware(
                source,
                "utf-8",
                b"",
                |cp, out| {
                    let c = char::from_u32(cp).expect("non-surrogate codepoint is a valid char");
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                },
                |cp| pyrust_core::cesu8_encode_codepoint(cp).into_bytes(),
                errors,
            )
        }
        // UTF-8-SIG: prepend U+FEFF BOM (EF BB BF) then UTF-8 encoded content.
        "utf-8-sig" => {
            if !source.as_bytes().contains(&0xED) {
                let mut out = Vec::with_capacity(3 + source.len());
                out.extend_from_slice(b"\xef\xbb\xbf");
                out.extend_from_slice(source.as_bytes());
                return Ok(Value::bytes(out));
            }
            encode_utf_surrogate_aware(
                source,
                "utf-8-sig",
                b"\xef\xbb\xbf",
                |cp, out| {
                    let c = char::from_u32(cp).expect("non-surrogate codepoint is a valid char");
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                },
                |cp| pyrust_core::cesu8_encode_codepoint(cp).into_bytes(),
                errors,
            )
        }
        "ascii" | "us-ascii" | "646" => {
            encode_single_byte_codec(source, "ascii", |cp| cp < 0x80, "128", errors)
        }
        "latin-1" | "iso-8859-1" | "8859" | "cp819" | "latin1" | "l1" => {
            encode_single_byte_codec(source, "latin-1", |cp| cp < 0x100, "256", errors)
        }
        "cp1252" | "windows-1252" => encode_cp1252(source, errors),
        "unicode-escape" => Ok(Value::bytes(encode_unicode_escape(source))),
        "raw-unicode-escape" => Ok(Value::bytes(encode_raw_unicode_escape(source))),
        "utf-7" => Ok(Value::bytes(encode_utf7(source))),
        // UTF-16 with LE BOM: \xff\xfe followed by LE-encoded code units.
        // "utf16" is the no-separator alias (normalize replaces _ with - but not
        // nothing, so "utf16" stays as-is and must be listed separately).
        "utf-16" | "utf16" => encode_utf_surrogate_aware(
            source,
            "utf-16",
            b"\xff\xfe",
            |cp, out| encode_cp_utf16(cp, false, out),
            |cp| utf16_surrogate_bytes(cp, false),
            errors,
        ),
        "utf-16-le" => encode_utf_surrogate_aware(
            source,
            "utf-16-le",
            b"",
            |cp, out| encode_cp_utf16(cp, false, out),
            |cp| utf16_surrogate_bytes(cp, false),
            errors,
        ),
        "utf-16-be" => encode_utf_surrogate_aware(
            source,
            "utf-16-be",
            b"",
            |cp, out| encode_cp_utf16(cp, true, out),
            |cp| utf16_surrogate_bytes(cp, true),
            errors,
        ),
        // UTF-32 with LE BOM: \xff\xfe\x00\x00 followed by LE-encoded code points.
        "utf-32" | "utf32" => encode_utf_surrogate_aware(
            source,
            "utf-32",
            b"\xff\xfe\x00\x00",
            |cp, out| out.extend_from_slice(&cp.to_le_bytes()),
            |cp| cp.to_le_bytes().to_vec(),
            errors,
        ),
        "utf-32-le" => encode_utf_surrogate_aware(
            source,
            "utf-32-le",
            b"",
            |cp, out| out.extend_from_slice(&cp.to_le_bytes()),
            |cp| cp.to_le_bytes().to_vec(),
            errors,
        ),
        "utf-32-be" => encode_utf_surrogate_aware(
            source,
            "utf-32-be",
            b"",
            |cp, out| out.extend_from_slice(&cp.to_be_bytes()),
            |cp| cp.to_be_bytes().to_vec(),
            errors,
        ),
        _ => Err(PyError::named(
            "LookupError",
            format!("unknown encoding: {encoding}"),
        )),
    }
}

// ---------------------------------------------------------------------------
// Additional codecs: cp1252, unicode_escape, raw_unicode_escape, utf-7
// ---------------------------------------------------------------------------
