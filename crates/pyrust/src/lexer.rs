use num_bigint::BigInt;

use crate::error::{PyError, Result};
use crate::token::{FStringPart, Token};

pub struct Lexer {
    tokens: Vec<Token>,
}

impl Lexer {
    pub fn new(src: &str) -> Result<Self> {
        let mut lexer = Self { tokens: Vec::new() };
        lexer.lex_source(src)?;
        Ok(lexer)
    }

    pub fn into_tokens(self) -> Vec<Token> {
        self.tokens
    }

    fn lex_source(&mut self, src: &str) -> Result<()> {
        let mut indent_stack: Vec<usize> = vec![0];
        let mut paren_depth: usize = 0; // track ( [ { for implicit line continuation

        // Work on the full source as a flat char array so that string literals
        // (including triple-quoted ones) can span line boundaries naturally.
        let chars: Vec<char> = src.chars().collect();
        let mut pos = 0;

        while pos <= chars.len() {
            // Measure the indentation of the current logical line.
            // We do this at the start of each physical line (pos points just
            // after the preceding '\n', or at 0 for the first line).
            let line_start = pos;

            // Count leading spaces (tabs are rejected by count_indent_chars).
            let indent = count_indent_chars(&chars, pos)?;
            pos += indent;

            // Collect the rest of this line (up to '\n' or end-of-source) to
            // decide whether it is blank / comment-only before we emit INDENT /
            // DEDENT tokens.
            let content_start = pos;
            let mut eol = pos;
            while eol < chars.len() && chars[eol] != '\n' {
                eol += 1;
            }
            // Trim a trailing '\r' if present (CRLF sources).
            let line_end = if eol > 0 && chars[eol.saturating_sub(1)] == '\r' {
                eol - 1
            } else {
                eol
            };

            // Determine whether the visible content of this line is empty or a
            // comment.  We check from content_start to line_end.
            let visible: String = chars[content_start..line_end].iter().collect();
            let is_blank_or_comment =
                visible.trim().is_empty() || visible.trim_start().starts_with('#');

            if is_blank_or_comment {
                // Blank / comment lines do not affect indentation or emit
                // logical-line tokens (they emit Newline only at the top level,
                // matching CPython's behaviour for blank lines outside parens).
                if paren_depth == 0 {
                    self.tokens.push(Token::Newline);
                }
                // Advance past the '\n' (or reach end-of-source).
                pos = if eol < chars.len() {
                    eol + 1
                } else {
                    chars.len() + 1
                };
                continue;
            }

            // Non-blank line: handle indentation.
            if paren_depth == 0 {
                self.handle_indent(indent, &mut indent_stack)?;
            }

            // Lex tokens on this logical line.  `lex_line_tokens` advances
            // `pos` past the last token on the line (including any '\n') and
            // returns the updated paren depth.  It may consume multiple physical
            // lines when it encounters a triple-quoted string.
            pos = content_start; // reset to after-indent start
            let _ = line_start; // silence unused-variable warning
            let new_depth;
            (pos, new_depth) = self.lex_line_tokens(&chars, pos, paren_depth)?;
            paren_depth = new_depth;
        }

        while indent_stack.len() > 1 {
            indent_stack.pop();
            self.tokens.push(Token::Dedent);
        }

        self.tokens.push(Token::Eof);
        Ok(())
    }

    fn handle_indent(&mut self, indent: usize, stack: &mut Vec<usize>) -> Result<()> {
        let current = *stack.last().expect("indent stack is never empty");
        if indent > current {
            stack.push(indent);
            self.tokens.push(Token::Indent);
            return Ok(());
        }

        if indent < current {
            while indent < *stack.last().expect("indent stack is never empty") {
                stack.pop();
                self.tokens.push(Token::Dedent);
            }
            let after = *stack.last().expect("indent stack is never empty");
            if indent != after {
                return Err(PyError::Lex("inconsistent indentation".to_string()));
            }
        }
        Ok(())
    }

    /// Lex tokens starting at `pos` in the full source `chars`, up to (and
    /// including) the logical end of the current line.  Returns `(next_pos,
    /// new_paren_depth)` where `next_pos` points to the character just after
    /// the consumed '\n' (or `chars.len() + 1` when at EOF).
    ///
    /// Triple-quoted string literals are read across '\n' boundaries here, so
    /// the returned `next_pos` may skip many physical lines.
    fn lex_line_tokens(
        &mut self,
        chars: &[char],
        start: usize,
        mut paren_depth: usize,
    ) -> Result<(usize, usize)> {
        let mut pos = start;
        let mut line_continued = false;

        loop {
            match chars.get(pos).copied() {
                None => {
                    // End of source.
                    if paren_depth == 0 && !line_continued {
                        self.tokens.push(Token::Newline);
                    }
                    return Ok((chars.len() + 1, paren_depth));
                }
                Some('\n') => {
                    if line_continued {
                        // Backslash continuation: consume '\n' and keep going
                        // on the next physical line (do NOT emit Newline).
                        pos += 1;
                        // Skip leading whitespace on the continuation line.
                        while matches!(chars.get(pos), Some(&' ') | Some(&'\t')) {
                            pos += 1;
                        }
                        line_continued = false;
                        continue;
                    }
                    if paren_depth == 0 {
                        self.tokens.push(Token::Newline);
                    }
                    // Advance past '\n' and return so that lex_source can
                    // process the indentation of the next line.
                    return Ok((pos + 1, paren_depth));
                }
                Some('\r') => {
                    // Bare CR: skip (CRLF handled by consuming \r then \n above)
                    pos += 1;
                }
                Some(' ') | Some('\t') => {
                    pos += 1;
                }
                Some('#') => {
                    // Comment: skip to end of line.
                    while pos < chars.len() && chars[pos] != '\n' {
                        pos += 1;
                    }
                    // '\n' will be handled on the next iteration.
                }
                Some('0'..='9') => {
                    let (tok, next) = lex_number(chars, pos)?;
                    self.tokens.push(tok);
                    pos = next;
                }
                Some('a'..='z') | Some('A'..='Z') | Some('_') => {
                    let c = chars[pos];
                    // Check for f-string prefix: f" or f' (or F" / F')
                    if (c == 'f' || c == 'F')
                        && matches!(chars.get(pos + 1), Some('"') | Some('\''))
                    {
                        let quote = chars[pos + 1];
                        let (tok, next) = lex_fstring(chars, pos + 2, quote)?;
                        self.tokens.push(tok);
                        pos = next;
                    } else if (c == 'b' || c == 'B')
                        && matches!(chars.get(pos + 1), Some('"') | Some('\''))
                    {
                        // Bytes literal: b"..." or b'...' (no rb/br combos yet)
                        let (tok, next) = lex_bytes(chars, pos + 1)?;
                        self.tokens.push(tok);
                        pos = next;
                    } else {
                        let (tok, next) = lex_ident_or_keyword(chars, pos);
                        self.tokens.push(tok);
                        pos = next;
                    }
                }
                Some('"') | Some('\'') => {
                    // lex_string now receives the full chars slice so triple-
                    // quoted strings can span physical lines.
                    let (tok, next) = lex_string(chars, pos)?;
                    self.tokens.push(tok);
                    pos = next;
                }
                Some('+') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::PlusAssign);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Plus);
                        pos += 1;
                    }
                }
                Some('-') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::MinusAssign);
                        pos += 2;
                    } else if chars.get(pos + 1) == Some(&'>') {
                        self.tokens.push(Token::Arrow);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Minus);
                        pos += 1;
                    }
                }
                Some('@') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::AtAssign);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::At);
                        pos += 1;
                    }
                }
                Some(';') => {
                    self.tokens.push(Token::Semicolon);
                    pos += 1;
                }
                Some('*') => {
                    if chars.get(pos + 1) == Some(&'*') {
                        if chars.get(pos + 2) == Some(&'=') {
                            self.tokens.push(Token::StarStarAssign);
                            pos += 3;
                        } else {
                            self.tokens.push(Token::StarStar);
                            pos += 2;
                        }
                    } else if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::StarAssign);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Star);
                        pos += 1;
                    }
                }
                Some('/') => {
                    if chars.get(pos + 1) == Some(&'/') {
                        if chars.get(pos + 2) == Some(&'=') {
                            self.tokens.push(Token::SlashSlashAssign);
                            pos += 3;
                        } else {
                            self.tokens.push(Token::SlashSlash);
                            pos += 2;
                        }
                    } else if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::SlashAssign);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Slash);
                        pos += 1;
                    }
                }
                Some('%') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::PercentAssign);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Percent);
                        pos += 1;
                    }
                }
                Some('&') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::AmpersandAssign);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Ampersand);
                        pos += 1;
                    }
                }
                Some('|') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::PipeAssign);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Pipe);
                        pos += 1;
                    }
                }
                Some('^') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::CaretAssign);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Caret);
                        pos += 1;
                    }
                }
                Some('~') => {
                    self.tokens.push(Token::Tilde);
                    pos += 1;
                }
                Some('<') => {
                    if chars.get(pos + 1) == Some(&'<') {
                        if chars.get(pos + 2) == Some(&'=') {
                            self.tokens.push(Token::LShiftAssign);
                            pos += 3;
                        } else {
                            self.tokens.push(Token::LShift);
                            pos += 2;
                        }
                    } else if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::Le);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Lt);
                        pos += 1;
                    }
                }
                Some('>') => {
                    if chars.get(pos + 1) == Some(&'>') {
                        if chars.get(pos + 2) == Some(&'=') {
                            self.tokens.push(Token::RShiftAssign);
                            pos += 3;
                        } else {
                            self.tokens.push(Token::RShift);
                            pos += 2;
                        }
                    } else if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::Ge);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Gt);
                        pos += 1;
                    }
                }
                Some('(') => {
                    self.tokens.push(Token::LParen);
                    paren_depth += 1;
                    pos += 1;
                }
                Some(')') => {
                    self.tokens.push(Token::RParen);
                    paren_depth = paren_depth.saturating_sub(1);
                    pos += 1;
                }
                Some('[') => {
                    self.tokens.push(Token::LBracket);
                    paren_depth += 1;
                    pos += 1;
                }
                Some(']') => {
                    self.tokens.push(Token::RBracket);
                    paren_depth = paren_depth.saturating_sub(1);
                    pos += 1;
                }
                Some('{') => {
                    self.tokens.push(Token::LBrace);
                    paren_depth += 1;
                    pos += 1;
                }
                Some('}') => {
                    self.tokens.push(Token::RBrace);
                    paren_depth = paren_depth.saturating_sub(1);
                    pos += 1;
                }
                Some(',') => {
                    self.tokens.push(Token::Comma);
                    pos += 1;
                }
                Some(':') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::Walrus);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Colon);
                        pos += 1;
                    }
                }
                Some('.') => {
                    self.tokens.push(Token::Dot);
                    pos += 1;
                }
                Some('=') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::Eq);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Assign);
                        pos += 1;
                    }
                }
                Some('!') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::Ne);
                        pos += 2;
                    } else {
                        return Err(PyError::Lex("expected '=' after '!'".to_string()));
                    }
                }
                Some('\\') => {
                    // Backslash continuation: consume the '\' and set flag.
                    // The '\n' will be consumed on the next iteration.
                    line_continued = true;
                    pos += 1;
                }
                Some(c) => return Err(PyError::Lex(format!("unexpected character '{c}'"))),
            }
        }
    }
}

/// Count the leading spaces in `chars` starting at `start`.
/// Tabs are rejected (same rule as before).  Stops at any non-space, non-tab
/// character (including '\n') so it is safe to call on the full source array.
fn count_indent_chars(chars: &[char], start: usize) -> Result<usize> {
    let mut count = 0;
    let mut pos = start;
    loop {
        match chars.get(pos) {
            Some(&' ') => {
                count += 1;
                pos += 1;
            }
            Some(&'\t') => {
                return Err(PyError::Lex(
                    "tabs are not supported; use spaces".to_string(),
                ));
            }
            _ => break,
        }
    }
    Ok(count)
}

/// Validate underscore placement in a raw digit string (before stripping `_`).
///
/// CPython rules (same across 3.11 / 3.12):
/// - A trailing underscore is a SyntaxError.
/// - Two consecutive underscores (`__`) are a SyntaxError.
/// - A leading underscore after the base prefix (e.g. `0x_FF`) is **valid**.
///
/// `kind` is used only in the error message (e.g. `"decimal"`, `"hexadecimal"`).
fn validate_underscores(raw: &[char], kind: &str) -> Result<()> {
    let mut prev_was_under = false;
    for &c in raw {
        if c == '_' {
            if prev_was_under {
                return Err(PyError::Lex(format!("invalid {kind} literal")));
            }
            prev_was_under = true;
        } else {
            prev_was_under = false;
        }
    }
    if prev_was_under {
        return Err(PyError::Lex(format!("invalid {kind} literal")));
    }
    Ok(())
}

fn lex_number(chars: &[char], start: usize) -> Result<(Token, usize)> {
    let mut pos = start;
    // Hex
    if chars.get(pos) == Some(&'0') && matches!(chars.get(pos + 1), Some(&'x') | Some(&'X')) {
        pos += 2;
        let hex_start = pos;
        while matches!(
            chars.get(pos),
            Some('0'..='9' | 'a'..='f' | 'A'..='F' | '_')
        ) {
            pos += 1;
        }
        let raw_hex = &chars[hex_start..pos];
        if raw_hex.is_empty() || raw_hex.iter().all(|&c| c == '_') {
            return Err(PyError::Lex("invalid hexadecimal literal".to_string()));
        }
        validate_underscores(raw_hex, "hexadecimal")?;
        let text: String = raw_hex.iter().filter(|&&c| c != '_').collect();
        return match i64::from_str_radix(&text, 16) {
            Ok(val) => Ok((Token::Int(val), pos)),
            Err(_) => {
                // Overflow: parse as BigInt and store decimal representation.
                let big = BigInt::parse_bytes(text.as_bytes(), 16)
                    .ok_or_else(|| PyError::Lex(format!("invalid hexadecimal literal")))?;
                Ok((Token::BigInt(big.to_string()), pos))
            }
        };
    }
    // Octal
    if chars.get(pos) == Some(&'0') && matches!(chars.get(pos + 1), Some(&'o') | Some(&'O')) {
        pos += 2;
        let oct_start = pos;
        while matches!(chars.get(pos), Some('0'..='7' | '_')) {
            pos += 1;
        }
        let raw_oct = &chars[oct_start..pos];
        if raw_oct.is_empty() || raw_oct.iter().all(|&c| c == '_') {
            return Err(PyError::Lex("invalid octal literal".to_string()));
        }
        validate_underscores(raw_oct, "octal")?;
        let text: String = raw_oct.iter().filter(|&&c| c != '_').collect();
        return match i64::from_str_radix(&text, 8) {
            Ok(val) => Ok((Token::Int(val), pos)),
            Err(_) => {
                let big = BigInt::parse_bytes(text.as_bytes(), 8)
                    .ok_or_else(|| PyError::Lex("invalid octal literal".to_string()))?;
                Ok((Token::BigInt(big.to_string()), pos))
            }
        };
    }
    // Binary
    if chars.get(pos) == Some(&'0') && matches!(chars.get(pos + 1), Some(&'b') | Some(&'B')) {
        pos += 2;
        let bin_start = pos;
        while matches!(chars.get(pos), Some('0'..='1' | '_')) {
            pos += 1;
        }
        let raw_bin = &chars[bin_start..pos];
        if raw_bin.is_empty() || raw_bin.iter().all(|&c| c == '_') {
            return Err(PyError::Lex("invalid binary literal".to_string()));
        }
        validate_underscores(raw_bin, "binary")?;
        let text: String = raw_bin.iter().filter(|&&c| c != '_').collect();
        return match i64::from_str_radix(&text, 2) {
            Ok(val) => Ok((Token::Int(val), pos)),
            Err(_) => {
                let big = BigInt::parse_bytes(text.as_bytes(), 2)
                    .ok_or_else(|| PyError::Lex("invalid binary literal".to_string()))?;
                Ok((Token::BigInt(big.to_string()), pos))
            }
        };
    }

    while matches!(chars.get(pos), Some('0'..='9' | '_')) {
        pos += 1;
    }

    if chars.get(pos) == Some(&'.') && matches!(chars.get(pos + 1), Some('0'..='9')) {
        pos += 1;
        while matches!(chars.get(pos), Some('0'..='9' | '_')) {
            pos += 1;
        }
        // Optional exponent
        if matches!(chars.get(pos), Some(&'e') | Some(&'E')) {
            pos += 1;
            if matches!(chars.get(pos), Some(&'+') | Some(&'-')) {
                pos += 1;
            }
            while matches!(chars.get(pos), Some('0'..='9')) {
                pos += 1;
            }
        }
        let text: String = chars[start..pos].iter().filter(|&&c| c != '_').collect();
        let val = text
            .parse::<f64>()
            .map_err(|_| PyError::Lex(format!("invalid float '{text}'")))?;
        // Imaginary suffix: 3.14j
        if matches!(chars.get(pos), Some(&'j') | Some(&'J')) {
            return Ok((Token::Imag(val), pos + 1));
        }
        Ok((Token::Float(val), pos))
    } else {
        // Optional exponent on integer-looking floats like 1e5
        if matches!(chars.get(pos), Some(&'e') | Some(&'E')) {
            pos += 1;
            if matches!(chars.get(pos), Some(&'+') | Some(&'-')) {
                pos += 1;
            }
            let exp_start = pos;
            while matches!(chars.get(pos), Some('0'..='9')) {
                pos += 1;
            }
            if pos > exp_start {
                let text: String = chars[start..pos].iter().collect();
                let val = text
                    .parse::<f64>()
                    .map_err(|_| PyError::Lex(format!("invalid float '{text}'")))?;
                if matches!(chars.get(pos), Some(&'j') | Some(&'J')) {
                    return Ok((Token::Imag(val), pos + 1));
                }
                return Ok((Token::Float(val), pos));
            }
        }
        // Imaginary suffix on bare int: 5j
        if matches!(chars.get(pos), Some(&'j') | Some(&'J')) {
            let raw_imag = &chars[start..pos];
            validate_underscores(raw_imag, "decimal")?;
            let text: String = raw_imag.iter().filter(|&&c| c != '_').collect();
            let val = text
                .parse::<f64>()
                .map_err(|_| PyError::Lex(format!("invalid imaginary literal '{text}j'")))?;
            return Ok((Token::Imag(val), pos + 1));
        }
        let raw_dec = &chars[start..pos];
        validate_underscores(raw_dec, "decimal")?;
        let text: String = raw_dec.iter().filter(|&&c| c != '_').collect();
        match text.parse::<i64>() {
            Ok(val) => Ok((Token::Int(val), pos)),
            Err(_) => {
                // Overflow: try parsing as an arbitrary-precision integer.
                let big = text
                    .parse::<BigInt>()
                    .map_err(|_| PyError::Lex(format!("invalid integer '{text}'")))?;
                Ok((Token::BigInt(big.to_string()), pos))
            }
        }
    }
}

fn lex_ident_or_keyword(chars: &[char], start: usize) -> (Token, usize) {
    let mut pos = start;
    while matches!(
        chars.get(pos),
        Some('a'..='z' | 'A'..='Z' | '0'..='9' | '_')
    ) {
        pos += 1;
    }

    let text: String = chars[start..pos].iter().collect();
    let tok = match text.as_str() {
        "and" => Token::And,
        "or" => Token::Or,
        "not" => Token::Not,
        "if" => Token::If,
        "elif" => Token::Elif,
        "else" => Token::Else,
        "while" => Token::While,
        "for" => Token::For,
        "in" => Token::In,
        "def" => Token::Def,
        "class" => Token::Class,
        "try" => Token::Try,
        "except" => Token::Except,
        "finally" => Token::Finally,
        "raise" => Token::Raise,
        "as" => Token::As,
        "import" => Token::Import,
        "from" => Token::From,
        "global" => Token::Global,
        "nonlocal" => Token::Nonlocal,
        "return" => Token::Return,
        "yield" => Token::Yield,
        "break" => Token::Break,
        "continue" => Token::Continue,
        "pass" => Token::Pass,
        "del" => Token::Del,
        "assert" => Token::Assert,
        "lambda" => Token::Lambda,
        "with" => Token::With,
        "is" => Token::Is,
        "True" => Token::True,
        "False" => Token::False,
        "None" => Token::None,
        _ => Token::Ident(text),
    };

    (tok, pos)
}

fn lex_bytes(chars: &[char], start: usize) -> Result<(Token, usize)> {
    let quote = chars[start];
    let mut pos = start + 1;
    let mut out: Vec<u8> = Vec::new();
    while let Some(c) = chars.get(pos).copied() {
        if c == quote {
            return Ok((Token::Bytes(out), pos + 1));
        }
        if c == '\\' {
            pos += 1;
            let esc = chars
                .get(pos)
                .copied()
                .ok_or_else(|| PyError::Lex("unterminated bytes escape".to_string()))?;
            let mapped = match esc {
                'n' => 0x0a,
                't' => 0x09,
                'r' => 0x0d,
                '0' => 0x00,
                '\\' => 0x5c,
                '\'' => 0x27,
                '"' => 0x22,
                'x' => {
                    // \xNN — two hex digits
                    let hi = chars
                        .get(pos + 1)
                        .copied()
                        .ok_or_else(|| PyError::Lex("incomplete \\x escape".to_string()))?;
                    let lo = chars
                        .get(pos + 2)
                        .copied()
                        .ok_or_else(|| PyError::Lex("incomplete \\x escape".to_string()))?;
                    let v = u8::from_str_radix(&format!("{hi}{lo}"), 16)
                        .map_err(|_| PyError::Lex("invalid \\x escape".to_string()))?;
                    pos += 2;
                    v
                }
                other => {
                    return Err(PyError::Lex(format!(
                        "unsupported escape in bytes literal: \\{other}"
                    )));
                }
            };
            out.push(mapped);
            pos += 1;
            continue;
        }
        // Non-ASCII characters not allowed in bytes literals
        if (c as u32) > 0x7f {
            return Err(PyError::Lex(format!(
                "bytes can only contain ASCII literal characters (got {c:?})"
            )));
        }
        out.push(c as u8);
        pos += 1;
    }
    Err(PyError::Lex("unterminated bytes literal".to_string()))
}

fn lex_string(chars: &[char], start: usize) -> Result<(Token, usize)> {
    let quote = chars[start];

    // Triple-quoted strings
    if chars.get(start + 1) == Some(&quote) && chars.get(start + 2) == Some(&quote) {
        let mut pos = start + 3;
        let mut out = String::new();
        loop {
            if pos + 2 < chars.len()
                && chars[pos] == quote
                && chars[pos + 1] == quote
                && chars[pos + 2] == quote
            {
                return Ok((Token::Str(out), pos + 3));
            }
            match chars.get(pos) {
                None => {
                    return Err(PyError::Lex(
                        "unterminated triple-quoted string".to_string(),
                    ));
                }
                Some(&'\\') => {
                    pos += 1;
                    let escaped = chars
                        .get(pos)
                        .copied()
                        .ok_or_else(|| PyError::Lex("unterminated escape".to_string()))?;
                    // \<newline> inside a triple-quoted string: skip both
                    // (line continuation within the string literal, same as CPython).
                    // Also handle CRLF (\r\n): \<CR><LF> drops all three characters.
                    if escaped == '\n' {
                        pos += 1;
                    } else if escaped == '\r' {
                        pos += 1;
                        // If \r is followed by \n (CRLF), skip the \n too.
                        if chars.get(pos) == Some(&'\n') {
                            pos += 1;
                        }
                    } else {
                        out.push(map_escape(escaped)?);
                        pos += 1;
                    }
                }
                Some(&'\r') => {
                    // Normalize CR and CRLF to LF inside triple-quoted strings,
                    // matching CPython's universal-newline handling (tokenize.c).
                    pos += 1;
                    if chars.get(pos) == Some(&'\n') {
                        pos += 1;
                    }
                    out.push('\n');
                }
                Some(&c) => {
                    out.push(c);
                    pos += 1;
                }
            }
        }
    }

    let mut pos = start + 1;
    let mut out = String::new();

    while let Some(c) = chars.get(pos).copied() {
        if c == quote {
            return Ok((Token::Str(out), pos + 1));
        }
        if c == '\\' {
            pos += 1;
            let escaped = chars
                .get(pos)
                .copied()
                .ok_or_else(|| PyError::Lex("unterminated escape sequence".to_string()))?;
            out.push(map_escape(escaped)?);
            pos += 1;
            continue;
        }
        out.push(c);
        pos += 1;
    }

    Err(PyError::Lex("unterminated string literal".to_string()))
}

/// Lex an f-string starting just after the opening quote character.
/// `quote` is the quote char (`"` or `'`).
/// Returns `(Token::FString(parts), next_pos)`.
fn lex_fstring(chars: &[char], start: usize, quote: char) -> Result<(Token, usize)> {
    let mut parts: Vec<FStringPart> = Vec::new();
    let mut pos = start;
    let mut literal = String::new();

    while let Some(&c) = chars.get(pos) {
        if c == quote {
            // End of f-string.
            if !literal.is_empty() {
                parts.push(FStringPart::Literal(literal));
            }
            return Ok((Token::FString(parts), pos + 1));
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
            pos += 1; // skip '{'
            let (src, conversion, format_spec, debug_text, next) = lex_fstring_expr(chars, pos)?;
            parts.push(FStringPart::Expr {
                src,
                conversion,
                format_spec,
                debug_text,
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
            let escaped = chars
                .get(pos)
                .copied()
                .ok_or_else(|| PyError::Lex("unterminated escape in f-string".to_string()))?;
            literal.push(map_escape(escaped)?);
            pos += 1;
            continue;
        }
        literal.push(c);
        pos += 1;
    }
    Err(PyError::Lex("unterminated f-string".to_string()))
}

/// Parse the expression inside `{...}` of an f-string.
/// `pos` points to the first character after the opening `{`.
/// Returns `(expr_src, conversion, format_spec, debug_text, next_pos)` where
/// `next_pos` is just after the closing `}`.  `debug_text` is `Some(...)` for
/// the Python 3.8 debug form `f"{x=}"` and contains the verbatim source text
/// of the expression with the trailing `=` (whitespace preserved).
fn lex_fstring_expr(
    chars: &[char],
    start: usize,
) -> Result<(
    String,
    Option<char>,
    Option<Vec<FStringPart>>,
    Option<String>,
    usize,
)> {
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
                        .trim_end_matches(|c: char| c == ' ' || c == '\t')
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
                    let conv = chars.get(pos).copied().ok_or_else(|| {
                        PyError::Lex("expected conversion flag after '!'".to_string())
                    })?;
                    if !matches!(conv, 'r' | 's' | 'a') {
                        return Err(PyError::Lex(format!("unknown conversion flag '{conv}'")));
                    }
                    pos += 1;
                    Some(conv)
                } else {
                    None
                };
                // Optional format spec.
                let format_spec = if chars.get(pos) == Some(&':') {
                    pos += 1;
                    Some(lex_format_spec(chars, &mut pos)?)
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
                let conv = chars.get(pos).copied().ok_or_else(|| {
                    PyError::Lex("expected conversion flag after '!'".to_string())
                })?;
                if !matches!(conv, 'r' | 's' | 'a') {
                    return Err(PyError::Lex(format!("unknown conversion flag '{conv}'")));
                }
                pos += 1; // skip conversion char
                // Now check for format spec or closing }
                let format_spec = if chars.get(pos) == Some(&':') {
                    pos += 1;
                    Some(lex_format_spec(chars, &mut pos)?)
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
                let spec = lex_format_spec(chars, &mut pos)?;
                if chars.get(pos) != Some(&'}') {
                    return Err(PyError::Lex(
                        "expected '}' to close f-string expression".to_string(),
                    ));
                }
                return Ok((expr_src, None, Some(spec), None, pos + 1));
            }
            // Quoted strings inside the expression (so we don't mis-interpret their contents)
            Some(q @ ('"' | '\'')) => {
                src.push(q);
                pos += 1;
                while let Some(&sc) = chars.get(pos) {
                    pos += 1;
                    if sc == q {
                        src.push(sc);
                        break;
                    }
                    if sc == '\\' {
                        if let Some(&esc) = chars.get(pos) {
                            src.push('\\');
                            src.push(esc);
                            pos += 1;
                        }
                    } else {
                        src.push(sc);
                    }
                }
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
fn lex_format_spec(chars: &[char], pos: &mut usize) -> Result<Vec<FStringPart>> {
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
                *pos += 1;
                // Collect expression source until matching `}` (no further
                // *replacement-field* nesting permitted — matching CPython).
                // We still respect paren / bracket depth so e.g. `{f(1)}`
                // works, and we accept a trailing `!r`/`!s`/`!a` conversion
                // flag on this nested expression below.
                let mut src = String::new();
                let mut paren_depth = 0usize;
                let mut conversion: Option<char> = None;
                loop {
                    match chars.get(*pos).copied() {
                        None => {
                            return Err(PyError::Lex(
                                "unterminated nested expression in f-string format spec"
                                    .to_string(),
                            ));
                        }
                        Some('}') if paren_depth == 0 => {
                            *pos += 1;
                            break;
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
                        Some('!') if paren_depth == 0 => {
                            *pos += 1;
                            let conv = chars.get(*pos).copied().ok_or_else(|| {
                                PyError::Lex("expected conversion flag after '!'".to_string())
                            })?;
                            if !matches!(conv, 'r' | 's' | 'a') {
                                return Err(PyError::Lex(format!(
                                    "unknown conversion flag '{conv}'"
                                )));
                            }
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

fn map_escape(c: char) -> Result<char> {
    match c {
        'n' => Ok('\n'),
        't' => Ok('\t'),
        'r' => Ok('\r'),
        '\\' => Ok('\\'),
        '\'' => Ok('\''),
        '"' => Ok('"'),
        '0' => Ok('\0'),
        'a' => Ok('\x07'),
        'b' => Ok('\x08'),
        'f' => Ok('\x0C'),
        'v' => Ok('\x0B'),
        other => Err(PyError::Lex(format!("unsupported escape \\{other}"))),
    }
}
