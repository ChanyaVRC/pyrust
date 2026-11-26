#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::Token;

    fn lex_one(src: &str) -> Token {
        let lexer = Lexer::new(src).expect("lex failed");
        let mut tokens = lexer.into_tokens();
        // tokens: [<the token>, Newline, Eof]
        tokens.remove(0)
    }

    /// Unrecognised escape sequences in bytes literals must produce two literal
    /// bytes: 0x5C (backslash) followed by the character itself.  This mirrors
    /// CPython 3.12, which emits a SyntaxWarning and keeps both bytes verbatim.
    #[test]
    fn bytes_unrecognised_escape_keeps_backslash_and_char() {
        // b'\z' -> [0x5C, 0x7A]
        assert_eq!(lex_one(r"b'\z'"), Token::Bytes(vec![0x5C, 0x7A]));

        // b'\q\j' -> [0x5C, 0x71, 0x5C, 0x6A]
        assert_eq!(
            lex_one(r"b'\q\j'"),
            Token::Bytes(vec![0x5C, 0x71, 0x5C, 0x6A])
        );

        // Mixed: recognised + unrecognised + recognised
        // b'\n\z\t' -> [0x0A, 0x5C, 0x7A, 0x09]
        assert_eq!(
            lex_one(r"b'\n\z\t'"),
            Token::Bytes(vec![0x0A, 0x5C, 0x7A, 0x09])
        );
    }

    /// Recognised bytes escape sequences must continue to work correctly.
    #[test]
    fn bytes_recognised_escapes_work() {
        assert_eq!(lex_one(r"b'\n'"), Token::Bytes(vec![0x0A]));
        assert_eq!(lex_one(r"b'\t'"), Token::Bytes(vec![0x09]));
        assert_eq!(lex_one(r"b'\r'"), Token::Bytes(vec![0x0D]));
        assert_eq!(lex_one(r"b'\\'"), Token::Bytes(vec![0x5C]));
        assert_eq!(lex_one(r"b'\x41'"), Token::Bytes(vec![0x41]));
        assert_eq!(lex_one(r"b'\101'"), Token::Bytes(vec![0x41])); // octal
    }

    /// Octal escapes > 0xFF in bytes literals must truncate to the low byte,
    /// matching CPython 3.12 (which emits SyntaxWarning + truncates).
    #[test]
    fn bytes_octal_escape_overflow_truncates() {
        // \400 = 256 decimal → low byte 0x00
        assert_eq!(lex_one("b'\\400'"), Token::Bytes(vec![0x00]));
        // \777 = 511 decimal → low byte 0xFF
        assert_eq!(lex_one("b'\\777'"), Token::Bytes(vec![0xFF]));
        // \377 = 255 decimal → 0xFF (no overflow, sanity check)
        assert_eq!(lex_one("b'\\377'"), Token::Bytes(vec![0xFF]));
    }

    /// PEP 701: `# ...` comments inside multi-line f-string `{...}` expressions
    /// must not let `}` inside the comment prematurely terminate the field.
    #[test]
    fn fstring_comment_with_brace_does_not_terminate_expr() {
        let tok = lex_one("f\"\"\"{1 # }\n}\"\"\"");
        match tok {
            Token::FString(parts) => {
                assert_eq!(parts.len(), 1);
                match &parts[0] {
                    crate::token::FStringPart::Expr {
                        src,
                        conversion,
                        format_spec,
                        debug_text,
                        ..
                    } => {
                        assert_eq!(src.trim(), "1");
                        assert_eq!(*conversion, None);
                        assert!(format_spec.is_none());
                        assert!(debug_text.is_none());
                    }
                    other => panic!("expected Expr part, got {other:?}"),
                }
            }
            other => panic!("expected FString token, got {other:?}"),
        }

        // Unterminated when comment swallows the only closing `}` with no newline.
        assert!(Lexer::new("f\"{1 # }\"").is_err());
    }

    /// PEP 701: nested format specifiers up to CPython's depth-2 limit
    /// (`f"{1:{2:{3}}}"`) lex recursively; depth 3 (`f"{1:{2:{3:{4}}}}"`) fails
    /// with CPython's exact message.
    #[test]
    fn fstring_nested_format_specs_and_depth_limit() {
        let tok = lex_one("f\"{1:{2:{3}}}\"");
        match tok {
            Token::FString(parts) => {
                assert_eq!(parts.len(), 1);
                let crate::token::FStringPart::Expr {
                    format_spec: Some(spec1),
                    ..
                } = &parts[0]
                else {
                    panic!("expected outer format spec");
                };
                assert_eq!(spec1.len(), 1);
                let crate::token::FStringPart::Expr {
                    format_spec: Some(spec2),
                    ..
                } = &spec1[0]
                else {
                    panic!("expected second-level format spec");
                };
                assert_eq!(spec2.len(), 1);
                assert!(matches!(
                    &spec2[0],
                    crate::token::FStringPart::Expr {
                        src,
                        format_spec: None,
                        ..
                    } if src.trim() == "3"
                ));
            }
            other => panic!("expected FString token, got {other:?}"),
        }

        // Depth 3 exceeds CPython's limit.
        let err = match Lexer::new("f\"{1:{2:{3:{4}}}}\"") {
            Err(e) => e,
            Ok(_) => panic!("expected depth-3 format spec to fail"),
        };
        // Top-level lexer errors carry their location (#3402).
        assert_eq!(
            err.to_string(),
            "SyntaxError: f-string: expressions nested too deeply"
        );
    }

    /// PEP 701: same-quote reuse in nested f-strings (`f"{f"{"a"}"}"`,
    /// `f"{f"{"}"}"}"`) must keep the entire inner f-string intact inside `src`.
    #[test]
    fn fstring_same_quote_nested_fstring_lexes_intact() {
        let tok = lex_one("f\"{f\"{\"}\"}\"}\"");
        match tok {
            Token::FString(parts) => {
                assert_eq!(parts.len(), 1);
                match &parts[0] {
                    crate::token::FStringPart::Expr { src, .. } => {
                        assert_eq!(src, "f\"{\"}\"}\"");
                    }
                    other => panic!("expected Expr part, got {other:?}"),
                }
            }
            other => panic!("expected FString token, got {other:?}"),
        }
    }

    #[test]
    fn tab_indentation_8_col_expansion_and_tab_error() {
        let tokens = Lexer::new("if True:\n\tx = 1\n\tif True:\n\t\ty = 2\n")
            .expect("pure tab indentation should succeed")
            .into_tokens();
        assert!(tokens.contains(&Token::Indent));
        assert!(tokens.contains(&Token::Dedent));

        let err = Lexer::new("if True:\n\tx = 1\n        y = 2\n")
            .err()
            .expect("mixed tab and 8 spaces must fail");
        assert!(
            err.to_string()
                .contains("inconsistent use of tabs and spaces in indentation"),
            "unexpected error: {err}"
        );

        let err2 = Lexer::new("if True:\n    x = 1\n\ty = 2\n")
            .err()
            .expect("4 spaces followed by tab must fail");
        assert!(
            err2.to_string()
                .contains("inconsistent use of tabs and spaces in indentation"),
            "unexpected error: {err2}"
        );
    }

    #[test]
    fn invalid_escape_records_syntax_warning_3_12_message() {
        let (_, _, _, _, warnings) = Lexer::new(r"x = '\d'; y = b'\w'; z = b'\400'; r = r'\d'")
            .expect("lex failed")
            .into_tokens_with_pos_and_warnings();
        assert_eq!(
            warnings,
            vec![
                (1, "invalid escape sequence '\\d'".to_string()),
                (1, "invalid escape sequence '\\w'".to_string()),
                (1, "invalid octal escape sequence '\\400'".to_string()),
            ]
        );
    }
}
