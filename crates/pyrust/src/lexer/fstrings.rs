/// Count the physical newlines in `chars[from..to]`.  Used to compute an
/// f-string field's line offset relative to the f-string's start (#2587).
fn count_newlines(chars: &[char], from: usize, to: usize) -> u32 {
    chars[from..to.min(chars.len())]
        .iter()
        .filter(|&&c| c == '\n')
        .count() as u32
}

/// Lex an f-string starting just after the opening quote character(s).
///
/// `start` points to the first content character (i.e. just after the `f"` /
/// `rf"` / `fr"` prefix and the opening quote). If the next two characters are
/// also `quote`, this is a triple-quoted f-string; `start` is advanced past
/// them automatically.
///
/// `quote` is the delimiter (`"` or `'`). When `raw` is true, backslash
/// sequences pass through literally while `{{` / `}}` escaping remains active.
///
/// Returns `(Token::FString(parts), next_pos)`.
fn lex_fstring(
    chars: &[char],
    start: usize,
    quote: char,
    raw: bool,
    line_start_pos: usize,
) -> Result<(Token, usize)> {
    let mut parts: Vec<FStringPart> = Vec::new();
    let mut literal = String::new();

    // Detect triple-quoted f-string.
    let (triple, content_start, mut pos) =
        if chars.get(start) == Some(&quote) && chars.get(start + 1) == Some(&quote) {
            (true, start + 2, start + 2)
        } else {
            (false, start, start)
        };

    loop {
        let c = match chars.get(pos).copied() {
            Some(c) => c,
            None => {
                return Err(PyError::Lex("unterminated f-string".to_string()));
            }
        };

        // Check for closing delimiter.
        if c == quote {
            if triple {
                if chars.get(pos + 1) == Some(&quote) && chars.get(pos + 2) == Some(&quote) {
                    if !literal.is_empty() {
                        parts.push(FStringPart::Literal(literal));
                    }
                    return Ok((Token::FString(parts), pos + 3));
                }
                // A lone quote inside a triple-quoted f-string is literal.
                literal.push(c);
                pos += 1;
                continue;
            } else {
                // Single-quoted: end of f-string.
                if !literal.is_empty() {
                    parts.push(FStringPart::Literal(literal));
                }
                return Ok((Token::FString(parts), pos + 1));
            }
        }

        if c == '{' {
            // Check for escaped {{ → literal {
            if chars.get(pos + 1) == Some(&'{') {
                literal.push('{');
                pos += 2;
                continue;
            }
            // Start of an expression.
            if !literal.is_empty() {
                parts.push(FStringPart::Literal(std::mem::take(&mut literal)));
            }
            let open_idx = pos;
            // Number of physical newlines from the f-string content start up
            // to this field's `{` — the field's line offset (issue #2587).
            let line_offset = count_newlines(chars, content_start, pos);
            pos += 1; // skip '{'
            let (src, conversion, format_spec, debug_text, next) =
                lex_fstring_expr(chars, pos, content_start)?;
            // PEP 657 (#2582): record the `{...}` field columns relative to the
            // f-string's source line.  `next - 1` is the closing `}`.  Skip when
            // the field crosses a physical line (multi-line f-string) — the
            // line-relative columns would be meaningless.
            let close_idx = next.saturating_sub(1);
            let field_cols = if open_idx >= line_start_pos
                && !chars[line_start_pos..=close_idx.min(chars.len().saturating_sub(1))]
                    .contains(&'\n')
            {
                let open_col = (open_idx - line_start_pos) as u32;
                let close_col = (close_idx + 1 - line_start_pos) as u32;
                Some((open_col, close_col))
            } else {
                None
            };
            parts.push(FStringPart::Expr {
                src,
                conversion,
                format_spec,
                debug_text,
                field_cols,
                line_offset,
            });
            pos = next;
            continue;
        }

        if c == '}' {
            // Check for escaped }} → literal }
            if chars.get(pos + 1) == Some(&'}') {
                literal.push('}');
                pos += 2;
                continue;
            }
            return Err(PyError::Lex("single '}' in f-string".to_string()));
        }

        if c == '\\' {
            pos += 1;
            let next_ch = chars
                .get(pos)
                .copied()
                .ok_or_else(|| PyError::Lex("unterminated escape in f-string".to_string()))?;
            if raw {
                // Raw mode: pass backslash and the following character verbatim.
                literal.push('\\');
                literal.push(next_ch);
                pos += 1;
            } else {
                // \<newline>: line continuation — consume backslash and newline, add nothing.
                // Also handle CRLF: \<CR><LF> drops all three characters.
                // At this point `pos` points at next_ch (the char after the backslash).
                if next_ch == '\n' {
                    pos += 1; // skip the newline
                } else if next_ch == '\r' {
                    pos += 1; // skip \r
                    if chars.get(pos) == Some(&'\n') {
                        pos += 1; // skip \n of CRLF
                    }
                } else {
                    let (s, next_pos) = parse_escape(chars, pos, content_start)?;
                    literal.push_str(&s);
                    pos = next_pos;
                }
            }
            continue;
        }

        literal.push(c);
        pos += 1;
    }
}

/// Parsed contents of an f-string replacement field:
/// `(expr_src, conversion, format_spec, debug_text, next_pos)`.
type FStringExpr = (
    String,
    Option<char>,
    Option<Vec<FStringPart>>,
    Option<String>,
    usize,
);

/// Lex the conversion segment after `!` in an f-string replacement field the way
/// CPython does: if `chars[pos]` is a NAME-start char (`is_alphabetic()` covers
/// ASCII + Unicode XID_Start, plus `_`), consume the whole NAME (name-start char
/// followed by name-continue chars) and return it; otherwise return an empty
/// string (the next char isn't a valid conversion at all, e.g. `5`/`.`).
///
/// The returned segment is then validated by `fstring_conversion_error` /
/// the caller, so multi-char segments like `sr` produce CPython's quoted-segment
/// message and non-name-start chars produce the no-detail "invalid conversion
/// character" form.
fn lex_conversion_segment(chars: &[char], pos: usize) -> String {
    match chars.get(pos).copied() {
        Some(c) if c.is_alphabetic() || c == '_' => {
            let mut segment = String::new();
            segment.push(c);
            let mut i = pos + 1;
            while let Some(&c) = chars.get(i) {
                if c.is_alphabetic() || c.is_ascii_digit() || c == '_' {
                    segment.push(c);
                    i += 1;
                } else {
                    break;
                }
            }
            segment
        }
        _ => String::new(),
    }
}

/// Build the `SyntaxError` message CPython 3.12 raises for a bad conversion
/// segment after `!` in an f-string replacement field.  CPython's PEG parser
/// distinguishes three cases:
///   * `!` is followed by a field terminator (`}`, `:`) or whitespace → there is
///     no conversion char at all: `f-string: missing conversion character`
///   * the next char isn't a NAME-start char (so the NAME-lex captured nothing,
///     e.g. `5`/`.`) → `f-string: invalid conversion character` (no quoted char)
///   * a non-empty NAME segment that isn't `s`/`r`/`a` (e.g. `z`, `sr`, `ra`) →
///     `f-string: invalid conversion character '<segment>': expected 's', 'r', or 'a'`
fn fstring_conversion_error(chars: &[char], pos: usize, segment: &str) -> PyError {
    if segment.is_empty() {
        match chars.get(pos).copied() {
            Some('}') | Some(':') => {
                PyError::Lex("f-string: missing conversion character".to_string())
            }
            Some(c) if c.is_whitespace() => {
                PyError::Lex("f-string: missing conversion character".to_string())
            }
            None => PyError::Lex("f-string: missing conversion character".to_string()),
            _ => PyError::Lex("f-string: invalid conversion character".to_string()),
        }
    } else {
        PyError::Lex(format!(
            "f-string: invalid conversion character '{segment}': expected 's', 'r', or 'a'"
        ))
    }
}

/// Parse the expression inside `{...}` of an f-string.
/// `pos` points to the first character after the opening `{`.
/// Returns `(expr_src, conversion, format_spec, debug_text, next_pos)` where
/// `next_pos` is just after the closing `}`.  `debug_text` is `Some(...)` for
/// the Python 3.8 debug form `f"{x=}"` and contains the verbatim source text
/// of the expression with the trailing `=` (whitespace preserved).
fn lex_fstring_expr(chars: &[char], start: usize, base: usize) -> Result<FStringExpr> {
    let mut pos = start;
    let mut depth = 0usize; // brace depth (for nested dicts/sets in expr)
    let mut src = String::new();

    // Collect the expression source, stopping at `}` (depth==0), `!`, `:`,
    // or a top-level `=` (the Python 3.8 debug form) that are NOT inside
    // nested brackets.  `=` is only recognised as the debug marker when it
    // is NOT followed by another `=` (which would make it the `==` operator).
    let mut paren_depth = 0usize; // () [] depth

    loop {
        match chars.get(pos).copied() {
            None => return Err(PyError::Lex("unterminated f-string expression".to_string())),
            Some('}') if depth == 0 && paren_depth == 0 => {
                // End of expression with no conversion or format spec.
                let expr_src = src.trim().to_string();
                return Ok((expr_src, None, None, None, pos + 1));
            }
            Some('{') => {
                depth += 1;
                src.push('{');
                pos += 1;
            }
            Some('}') => {
                depth -= 1;
                src.push('}');
                pos += 1;
            }
            Some('(') | Some('[') => {
                paren_depth += 1;
                src.push(chars[pos]);
                pos += 1;
            }
            Some(')') | Some(']') => {
                paren_depth = paren_depth.saturating_sub(1);
                src.push(chars[pos]);
                pos += 1;
            }
            // Python 3.8 debug form: `=` at top level (not `==`, `!=`, `<=`, `>=`).
            // The `=` is preceded by an expression and may be followed by an
            // optional conversion flag and/or format spec, then the closing `}`.
            // We also reject `=` when the previous non-space character of `src`
            // is itself `=` — that case is the second `=` of `==`, where the
            // first was already appended to `src` as part of the expression.
            Some('=')
                if depth == 0
                    && paren_depth == 0
                    && chars.get(pos + 1) != Some(&'=')
                    && !src.is_empty()
                    && !src
                        .trim_end_matches([' ', '\t'])
                        .ends_with(['!', '<', '>', '=']) =>
            {
                // Build the verbatim debug-text label: the raw source (with
                // leading/trailing whitespace preserved) plus the `=` itself,
                // plus any whitespace that follows `=` (CPython preserves it
                // in the label, not as a format spec).
                let mut debug_text = src.clone();
                debug_text.push('=');
                let expr_src = src.trim().to_string();
                pos += 1; // skip '='
                while let Some(&c) = chars.get(pos) {
                    if c == ' ' || c == '\t' {
                        debug_text.push(c);
                        pos += 1;
                    } else {
                        break;
                    }
                }
                // Optional conversion flag (!r, !s, !a) after `=` (and any
                // surrounding whitespace).
                let conversion = if chars.get(pos) == Some(&'!') {
                    pos += 1;
                    let segment = lex_conversion_segment(chars, pos);
                    match segment.as_str() {
                        "s" | "r" | "a" => {
                            let conv = segment.chars().next().unwrap();
                            pos += 1;
                            Some(conv)
                        }
                        _ => return Err(fstring_conversion_error(chars, pos, &segment)),
                    }
                } else {
                    None
                };
                // Optional format spec.
                let format_spec = if chars.get(pos) == Some(&':') {
                    pos += 1;
                    Some(lex_format_spec(chars, &mut pos, base)?)
                } else {
                    None
                };
                if chars.get(pos) != Some(&'}') {
                    return Err(PyError::Lex(
                        "expected '}' to close f-string expression".to_string(),
                    ));
                }
                return Ok((expr_src, conversion, format_spec, Some(debug_text), pos + 1));
            }
            // Conversion flag: !r, !s, !a — only at top level
            Some('!') if depth == 0 && paren_depth == 0 && chars.get(pos + 1) != Some(&'=') => {
                let expr_src = src.trim().to_string();
                pos += 1; // skip '!'
                let segment = lex_conversion_segment(chars, pos);
                let conv = match segment.as_str() {
                    "s" | "r" | "a" => segment.chars().next().unwrap(),
                    _ => return Err(fstring_conversion_error(chars, pos, &segment)),
                };
                pos += 1; // skip conversion char
                // Now check for format spec or closing }
                let format_spec = if chars.get(pos) == Some(&':') {
                    pos += 1;
                    Some(lex_format_spec(chars, &mut pos, base)?)
                } else {
                    None
                };
                if chars.get(pos) != Some(&'}') {
                    return Err(PyError::Lex(
                        "expected '}' to close f-string expression".to_string(),
                    ));
                }
                return Ok((expr_src, Some(conv), format_spec, None, pos + 1));
            }
            // Format spec: : — only at top level
            Some(':') if depth == 0 && paren_depth == 0 => {
                let expr_src = src.trim().to_string();
                pos += 1; // skip ':'
                let spec = lex_format_spec(chars, &mut pos, base)?;
                if chars.get(pos) != Some(&'}') {
                    return Err(PyError::Lex(
                        "expected '}' to close f-string expression".to_string(),
                    ));
                }
                return Ok((expr_src, None, Some(spec), None, pos + 1));
            }
            // Quoted strings inside the expression (so we don't mis-interpret their contents)
            Some(q @ ('"' | '\'')) => {
                consume_string_literal(chars, &mut pos, &mut src, q);
            }
            Some(other) => {
                src.push(other);
                pos += 1;
            }
        }
    }
}

/// Collect a format spec until we hit `}` (at depth 0), splitting it into a
/// list of f-string parts so that nested `{expr}` interpolations inside the
/// spec (e.g. `f"{x:>{width}}"`) are exposed as real sub-expressions instead
/// of being baked into an opaque string.  On return `*pos` points to the
/// closing `}` of the outer replacement field.
///
/// CPython's rule: a nested replacement field inside a format spec cannot
/// **itself contain another nested replacement field** (`f"{x:>{w:>{n}}}"`
/// is rejected at parse time).  Conversion flags (`!r`/`!s`/`!a`) on the
/// nested expression are accepted — this matches CPython and is implemented
/// in the loop below.  The nested expression also accepts a paren/bracket-
/// balanced Python expression (e.g. `f"{x:>{f(1)}}"`).  What is not
/// supported here is a further format spec on the nested field itself
/// (i.e. no `{w:>{n}:5}`) — that was kept out to bound recursion.
fn lex_format_spec(chars: &[char], pos: &mut usize, base: usize) -> Result<Vec<FStringPart>> {
    let mut parts: Vec<FStringPart> = Vec::new();
    let mut literal = String::new();
    loop {
        match chars.get(*pos).copied() {
            None => {
                return Err(PyError::Lex(
                    "unterminated f-string format spec".to_string(),
                ));
            }
            Some('}') => {
                // closing } of the outer expression — leave pos pointing at it
                if !literal.is_empty() {
                    parts.push(FStringPart::Literal(std::mem::take(&mut literal)));
                }
                break;
            }
            Some('{') => {
                // Start of a nested interpolation inside the spec.
                if !literal.is_empty() {
                    parts.push(FStringPart::Literal(std::mem::take(&mut literal)));
                }
                // Line offset of this nested field's `{` (issue #2587).
                let line_offset = count_newlines(chars, base, *pos);
                *pos += 1;
                // Collect expression source until matching `}` (no further
                // *replacement-field* nesting permitted — matching CPython).
                // We still respect paren / bracket / brace depth so e.g.
                // `{f(1)}` and `{ {'a':3}['a'] }` (dict/set literals) work,
                // and we accept a trailing `!r`/`!s`/`!a` conversion flag on
                // this nested expression below.
                let mut src = String::new();
                let mut paren_depth = 0usize;
                let mut brace_depth = 0usize; // dict/set literals in the nested expr
                let mut conversion: Option<char> = None;
                loop {
                    match chars.get(*pos).copied() {
                        None => {
                            return Err(PyError::Lex(
                                "unterminated nested expression in f-string format spec"
                                    .to_string(),
                            ));
                        }
                        Some('}') if paren_depth == 0 && brace_depth == 0 => {
                            *pos += 1;
                            break;
                        }
                        Some('{') => {
                            brace_depth += 1;
                            src.push('{');
                            *pos += 1;
                        }
                        Some('}') => {
                            brace_depth = brace_depth.saturating_sub(1);
                            src.push('}');
                            *pos += 1;
                        }
                        Some('(') | Some('[') => {
                            paren_depth += 1;
                            src.push(chars[*pos]);
                            *pos += 1;
                        }
                        Some(')') | Some(']') => {
                            paren_depth = paren_depth.saturating_sub(1);
                            src.push(chars[*pos]);
                            *pos += 1;
                        }
                        // Quoted strings inside the nested expression (including
                        // nested f-strings) are consumed verbatim so their `{`,
                        // `}`, `!` and `:` characters aren't mistaken for the end
                        // of the nested field or a conversion flag. The contents
                        // are re-lexed when `src` is later parsed as a sub-expr.
                        Some(q @ ('"' | '\'')) => {
                            consume_string_literal(chars, pos, &mut src, q);
                        }
                        Some('!')
                            if paren_depth == 0
                                && brace_depth == 0
                                && chars.get(*pos + 1) != Some(&'=') =>
                        {
                            *pos += 1;
                            let segment = lex_conversion_segment(chars, *pos);
                            let conv = match segment.as_str() {
                                "s" | "r" | "a" => segment.chars().next().unwrap(),
                                _ => return Err(fstring_conversion_error(chars, *pos, &segment)),
                            };
                            conversion = Some(conv);
                            *pos += 1;
                            if chars.get(*pos) != Some(&'}') {
                                return Err(PyError::Lex(
                                    "expected '}' to close nested f-string expression".to_string(),
                                ));
                            }
                            *pos += 1;
                            break;
                        }
                        Some(c) => {
                            src.push(c);
                            *pos += 1;
                        }
                    }
                }
                parts.push(FStringPart::Expr {
                    src: src.trim().to_string(),
                    conversion,
                    format_spec: None,
                    debug_text: None,
                    // Nested fields inside a format spec don't get their own
                    // caret anchor (#2582).
                    field_cols: None,
                    line_offset,
                });
            }
            Some(c) => {
                literal.push(c);
                *pos += 1;
            }
        }
    }
    Ok(parts)
}

/// Consume a quoted string literal (single- or triple-quoted) that appears
/// inside an f-string replacement field, appending it verbatim to `src`.
///
/// `*pos` must point at the opening quote `q`; on return it points just past
/// the closing quote. The contents — including any `{`, `}`, `!`, `:` and even
/// a nested f-string — are copied byte-for-byte so they aren't mistaken for
/// field/spec delimiters; the slice is re-lexed when `src` is parsed as a
/// sub-expression. Backslash escapes are preserved as two characters so a
/// quote can be embedded without prematurely terminating the literal.
fn consume_string_literal(chars: &[char], pos: &mut usize, src: &mut String, q: char) {
    // Triple-quoted? (e.g. `'''...'''` inside the field.)
    let triple = chars.get(*pos + 1) == Some(&q) && chars.get(*pos + 2) == Some(&q);
    if triple {
        src.push(q);
        src.push(q);
        src.push(q);
        *pos += 3;
        while let Some(&sc) = chars.get(*pos) {
            if sc == q && chars.get(*pos + 1) == Some(&q) && chars.get(*pos + 2) == Some(&q) {
                src.push(q);
                src.push(q);
                src.push(q);
                *pos += 3;
                return;
            }
            if sc == '\\' {
                src.push('\\');
                *pos += 1;
                if let Some(&esc) = chars.get(*pos) {
                    src.push(esc);
                    *pos += 1;
                }
            } else {
                src.push(sc);
                *pos += 1;
            }
        }
        return;
    }
    src.push(q);
    *pos += 1;
    while let Some(&sc) = chars.get(*pos) {
        *pos += 1;
        if sc == q {
            src.push(sc);
            return;
        }
        if sc == '\\' {
            src.push('\\');
            if let Some(&esc) = chars.get(*pos) {
                src.push(esc);
                *pos += 1;
            }
        } else {
            src.push(sc);
        }
    }
}
