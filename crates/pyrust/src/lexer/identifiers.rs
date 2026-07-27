fn lex_ident_or_keyword(chars: &[char], start: usize) -> (Token, usize) {
    let mut pos = start;
    while chars
        .get(pos)
        .is_some_and(|&c| c.is_alphabetic() || c.is_ascii_digit() || c == '_')
    {
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
