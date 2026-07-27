impl Parser {
    /// Convert lexer-level `FStringPart`s into AST-level `FStringPart`s by
    /// running a sub-parser on each raw expression source string.
    /// `token_line` is the source line where the f-string fragment begins
    /// (the line of the `f"..."` token).  Each field's `line_offset` (counted
    /// by the lexer from the fragment start) is added to it to recover the
    /// field's absolute source line for tracebacks (issue #2587).  `0` means
    /// no line info is available.
    fn parse_fstring_parts(
        &self,
        lex_parts: Vec<LexFStringPart>,
        token_line: u32,
    ) -> Result<Vec<FStringPart>> {
        let mut ast_parts = Vec::new();
        for lp in lex_parts {
            match lp {
                LexFStringPart::Literal(s) => {
                    ast_parts.push(FStringPart::Literal(s));
                }
                LexFStringPart::Expr {
                    src,
                    conversion,
                    format_spec,
                    debug_text,
                    field_cols,
                    line_offset,
                } => {
                    let line = if token_line == 0 {
                        0
                    } else {
                        token_line + line_offset
                    };
                    // Parse the field expression with `line` as its base so a
                    // *nested* f-string (`f"{f'''…{x}…'''}"`) anchors its own
                    // inner fields on their absolute source lines, not the
                    // outer field's line (issue #2587).
                    let expr = parse_expr_str(&src, line)?;
                    // Recursively parse any nested expressions inside the
                    // format spec — they need to be visible to every AST
                    // recursor (scope-pass, closure-capture analyser, etc.).
                    // Spec fields' `line_offset` is measured from the f-string
                    // fragment start (same as a top-level field), so they share
                    // `token_line`, not this field's `line`.
                    let format_spec = match format_spec {
                        None => None,
                        Some(parts) => Some(self.parse_fstring_parts(parts, token_line)?),
                    };
                    // PEP 657 (#2582): the whole `{...}` field is underlined
                    // with `^` (full == prim), matching CPython's FORMAT_VALUE
                    // anchor.
                    let span = field_cols.and_then(|(open, close)| {
                        if open < close {
                            Some((open, open, close, close))
                        } else {
                            None
                        }
                    });
                    ast_parts.push(FStringPart::Expr {
                        expr: Box::new(expr),
                        conversion,
                        format_spec,
                        debug_text,
                        span,
                        line,
                    });
                }
            }
        }
        Ok(ast_parts)
    }
}
