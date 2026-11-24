fn lex_ident_or_keyword(chars: &[char], start: usize) -> (Token, usize) {
    let mut pos = start;
    let mut all_ascii = true;
    while let Some(&c) = chars.get(pos) {
        if c.is_ascii_alphanumeric() || c == '_' {
            pos += 1;
        } else if c.is_alphabetic() {
            all_ascii = false;
            pos += 1;
        } else {
            break;
        }
    }

    let mut stack_buf = [0u8; 64];
    let heap_buf: String;
    let text: &str = if all_ascii && pos - start <= stack_buf.len() {
        for (slot, &c) in stack_buf.iter_mut().zip(&chars[start..pos]) {
            *slot = c as u8;
        }
        // SAFETY: every character in `chars[start..pos]` was verified ASCII above.
        std::str::from_utf8(&stack_buf[..pos - start]).expect("ASCII identifier is valid UTF-8")
    } else {
        heap_buf = chars[start..pos].iter().collect();
        heap_buf.as_str()
    };

    let tok = match text {
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
        _ => Token::Ident(pyrust_core::Symbol::intern(text)),
    };

    (tok, pos)
}
