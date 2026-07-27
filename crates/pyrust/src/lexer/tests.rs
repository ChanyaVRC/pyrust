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
}
