use crate::error::{PyError, Result};
use crate::token::Token;

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

        let lines: Vec<&str> = src.split('\n').collect();
        let mut i = 0;
        while i < lines.len() {
            let raw = lines[i];
            let line = raw.trim_end_matches('\r');
            let indent = count_indent(line)?;
            let content = &line[indent..];

            if content.trim().is_empty() || content.trim_start().starts_with('#') {
                self.tokens.push(Token::Newline);
                i += 1;
                continue;
            }

            if paren_depth == 0 {
                self.handle_indent(indent, &mut indent_stack)?;
            }
            paren_depth = self.lex_content_tracking(content, paren_depth)?;
            if paren_depth == 0 {
                self.tokens.push(Token::Newline);
            }
            i += 1;
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

    fn lex_content_tracking(&mut self, content: &str, mut paren_depth: usize) -> Result<usize> {
        let chars: Vec<char> = content.chars().collect();
        let mut pos = 0;

        while let Some(c) = chars.get(pos).copied() {
            match c {
                ' ' | '\t' => pos += 1,
                '#' => break,
                '0'..='9' => {
                    let (tok, next) = lex_number(&chars, pos)?;
                    self.tokens.push(tok);
                    pos = next;
                }
                'a'..='z' | 'A'..='Z' | '_' => {
                    let (tok, next) = lex_ident_or_keyword(&chars, pos);
                    self.tokens.push(tok);
                    pos = next;
                }
                '"' | '\'' => {
                    let (tok, next) = lex_string(&chars, pos)?;
                    self.tokens.push(tok);
                    pos = next;
                }
                '+' => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::PlusAssign);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Plus);
                        pos += 1;
                    }
                }
                '-' => {
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
                '@' => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::AtAssign);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::At);
                        pos += 1;
                    }
                }
                ';' => {
                    self.tokens.push(Token::Semicolon);
                    pos += 1;
                }
                '*' => {
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
                '/' => {
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
                '%' => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::PercentAssign);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Percent);
                        pos += 1;
                    }
                }
                '&' => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::AmpersandAssign);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Ampersand);
                        pos += 1;
                    }
                }
                '|' => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::PipeAssign);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Pipe);
                        pos += 1;
                    }
                }
                '^' => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::CaretAssign);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Caret);
                        pos += 1;
                    }
                }
                '~' => {
                    self.tokens.push(Token::Tilde);
                    pos += 1;
                }
                '<' => {
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
                '>' => {
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
                '(' => {
                    self.tokens.push(Token::LParen);
                    paren_depth += 1;
                    pos += 1;
                }
                ')' => {
                    self.tokens.push(Token::RParen);
                    if paren_depth > 0 { paren_depth -= 1; }
                    pos += 1;
                }
                '[' => {
                    self.tokens.push(Token::LBracket);
                    paren_depth += 1;
                    pos += 1;
                }
                ']' => {
                    self.tokens.push(Token::RBracket);
                    if paren_depth > 0 { paren_depth -= 1; }
                    pos += 1;
                }
                '{' => {
                    self.tokens.push(Token::LBrace);
                    paren_depth += 1;
                    pos += 1;
                }
                '}' => {
                    self.tokens.push(Token::RBrace);
                    if paren_depth > 0 { paren_depth -= 1; }
                    pos += 1;
                }
                ',' => {
                    self.tokens.push(Token::Comma);
                    pos += 1;
                }
                ':' => {
                    self.tokens.push(Token::Colon);
                    pos += 1;
                }
                '.' => {
                    self.tokens.push(Token::Dot);
                    pos += 1;
                }
                '=' => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::Eq);
                        pos += 2;
                    } else {
                        self.tokens.push(Token::Assign);
                        pos += 1;
                    }
                }
                '!' => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.tokens.push(Token::Ne);
                        pos += 2;
                    } else {
                        return Err(PyError::Lex("expected '=' after '!'".to_string()));
                    }
                }
                '\\' => {
                    // Line continuation: skip rest of line
                    break;
                }
                _ => return Err(PyError::Lex(format!("unexpected character '{c}'"))),
            }
        }

        Ok(paren_depth)
    }
}

fn count_indent(line: &str) -> Result<usize> {
    let mut count = 0;
    for c in line.chars() {
        match c {
            ' ' => count += 1,
            '\t' => {
                return Err(PyError::Lex(
                    "tabs are not supported; use spaces".to_string(),
                ));
            }
            _ => break,
        }
    }
    Ok(count)
}

fn lex_number(chars: &[char], start: usize) -> Result<(Token, usize)> {
    let mut pos = start;
    // Hex
    if chars.get(pos) == Some(&'0')
        && matches!(chars.get(pos + 1), Some(&'x') | Some(&'X'))
    {
        pos += 2;
        let hex_start = pos;
        while matches!(chars.get(pos), Some('0'..='9' | 'a'..='f' | 'A'..='F')) {
            pos += 1;
        }
        let text: String = chars[hex_start..pos].iter().collect();
        let val = i64::from_str_radix(&text, 16)
            .map_err(|_| PyError::Lex(format!("invalid hex literal '0x{text}'")))?;
        return Ok((Token::Int(val), pos));
    }
    // Octal
    if chars.get(pos) == Some(&'0')
        && matches!(chars.get(pos + 1), Some(&'o') | Some(&'O'))
    {
        pos += 2;
        let oct_start = pos;
        while matches!(chars.get(pos), Some('0'..='7')) {
            pos += 1;
        }
        let text: String = chars[oct_start..pos].iter().collect();
        let val = i64::from_str_radix(&text, 8)
            .map_err(|_| PyError::Lex(format!("invalid octal literal '0o{text}'")))?;
        return Ok((Token::Int(val), pos));
    }
    // Binary
    if chars.get(pos) == Some(&'0')
        && matches!(chars.get(pos + 1), Some(&'b') | Some(&'B'))
    {
        pos += 2;
        let bin_start = pos;
        while matches!(chars.get(pos), Some('0'..='1')) {
            pos += 1;
        }
        let text: String = chars[bin_start..pos].iter().collect();
        let val = i64::from_str_radix(&text, 2)
            .map_err(|_| PyError::Lex(format!("invalid binary literal '0b{text}'")))?;
        return Ok((Token::Int(val), pos));
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
                return Ok((Token::Float(val), pos));
            }
        }
        let text: String = chars[start..pos].iter().filter(|&&c| c != '_').collect();
        let val = text
            .parse::<i64>()
            .map_err(|_| PyError::Lex(format!("invalid integer '{text}'")))?;
        Ok((Token::Int(val), pos))
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
                None => return Err(PyError::Lex("unterminated triple-quoted string".to_string())),
                Some(&'\\') => {
                    pos += 1;
                    let escaped = chars
                        .get(pos)
                        .copied()
                        .ok_or_else(|| PyError::Lex("unterminated escape".to_string()))?;
                    out.push(map_escape(escaped)?);
                    pos += 1;
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
