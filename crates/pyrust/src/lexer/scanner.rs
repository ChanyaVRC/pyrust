/// Maximum number of indentation levels the lexer accepts, matching CPython's
/// tokenizer `MAXINDENT` (100).  The base (column-0) level is always present,
/// so this permits 99 nested indents and rejects the 100th with
/// `IndentationError: too many levels of indentation` — exactly CPython 3.12's
/// boundary.  Bounding indentation bounds compound-statement nesting depth,
/// which is what prevents deeply nested blocks from overflowing the parser's
/// native stack (issue #2221).
const MAX_INDENT_LEVELS: usize = 100;

pub struct Lexer {
    tokens: Vec<Token>,
    /// 1-based source line number of each token, recorded as the token is
    /// emitted (parallel to `tokens`).  Tracking the line during lexing — as
    /// opposed to counting `Token::Newline` post-hoc — is what lets tokens that
    /// follow a physical newline *inside* an open bracket group (implicit line
    /// continuation, where no `Newline` token is emitted) or *inside* a
    /// multi-line string literal carry their true line number (issue #2227).
    line_nos: Vec<u32>,
    /// 0-based **char** column at which each token starts within its physical
    /// line, parallel to `tokens` (issue #2426).  Recorded as the token is
    /// emitted, from `cur_col`.  Used by the parser to derive PEP 657 caret
    /// anchors for the highest-value expression forms.  Structural tokens
    /// (Newline / Indent / Dedent / Eof) carry whatever `cur_col` held; their
    /// column is never consulted.
    cols: Vec<u32>,
    /// 0-based **char** column one past the last char of each token, parallel to
    /// `tokens` (issue #2411).  Recorded at the bottom of the lex loop from the
    /// `pos` reached after the token's arm ran.  Lets the parser derive the
    /// *end* column of a sub-expression (its last token's end col) so PEP 657
    /// caret anchors can span multi-token forms (calls, binary ops, subscripts).
    /// A token whose lexing crossed a physical newline (multi-line string) has a
    /// meaningless end col on the original line, so it is recorded as 0; the
    /// parser treats 0 as "no end col" and omits the caret rather than emit a
    /// wrong one.
    cols_end: Vec<u32>,
    /// Current 1-based physical line.  Advanced on every physical `\n` consumed
    /// anywhere — including newlines suppressed inside brackets and newlines
    /// spanned by a single multi-line string/f-string token.
    line: u32,
    /// `chars` index at which the current physical line begins (issue #2426).
    /// Maintained alongside `line` so a token's start column is
    /// `token_start - line_start_pos`.  Updated whenever a physical `\n` is
    /// consumed (the new line starts just after it).
    line_start_pos: usize,
    /// Start column of the token about to be emitted (issue #2426).  Set at the
    /// single per-token dispatch point in `lex_line_tokens`; read by `emit`.
    cur_col: u32,
}

impl Lexer {
    pub fn new(src: &str) -> Result<Self> {
        let mut lexer = Self {
            tokens: Vec::new(),
            line_nos: Vec::new(),
            cols: Vec::new(),
            cols_end: Vec::new(),
            line: 1,
            line_start_pos: 0,
            cur_col: 0,
        };
        lexer.lex_source(src)?;
        Ok(lexer)
    }

    pub fn into_tokens(self) -> Vec<Token> {
        self.tokens
    }

    /// Push a token, recording the current physical line alongside it.
    #[inline]
    fn emit(&mut self, tok: Token) {
        self.line_nos.push(self.line);
        self.cols.push(self.cur_col);
        // End col is filled in at the bottom of the lex loop once `pos` has
        // advanced past the token (issue #2411); push a placeholder 0 to keep
        // the vector parallel.
        self.cols_end.push(0);
        self.tokens.push(tok);
    }

    /// Advance the physical-line counter by the number of `\n` characters in
    /// `chars[from..to]`.  Used after sub-lexers that may consume a span
    /// crossing physical lines (multi-line strings / f-strings).
    #[inline]
    fn advance_line_over(&mut self, chars: &[char], from: usize, to: usize) {
        for (i, &c) in chars[from..to].iter().enumerate() {
            if c == '\n' {
                self.line += 1;
                // The next physical line starts just after this '\n' (#2426).
                self.line_start_pos = from + i + 1;
            }
        }
    }

    /// Consume the lexer, returning the token stream together with a parallel
    /// vector of 1-based source line numbers (one entry per token), recorded
    /// during lexing by [`Lexer::emit`].  A token that begins on a continuation
    /// line inside brackets, or after a multi-line string, carries its true
    /// physical line number.
    ///
    /// Superseded for the script path by [`Lexer::into_tokens_with_pos`] (which
    /// also returns columns for #2426); retained as the line-only accessor.
    #[allow(dead_code)]
    pub fn into_tokens_with_linenos(self) -> (Vec<Token>, Vec<u32>) {
        debug_assert_eq!(self.tokens.len(), self.line_nos.len());
        (self.tokens, self.line_nos)
    }

    /// Like [`Lexer::into_tokens_with_linenos`] but also returns the parallel
    /// per-token start-column vector (0-based char offset within the token's
    /// physical line) recorded during lexing for PEP 657 caret anchors
    /// (issue #2426).
    pub fn into_tokens_with_pos(self) -> (Vec<Token>, Vec<u32>, Vec<u32>, Vec<u32>) {
        debug_assert_eq!(self.tokens.len(), self.line_nos.len());
        debug_assert_eq!(self.tokens.len(), self.cols.len());
        debug_assert_eq!(self.tokens.len(), self.cols_end.len());
        (self.tokens, self.line_nos, self.cols, self.cols_end)
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
            // Track where this physical line begins so emitted tokens can record
            // their start column as `token_start - line_start_pos` (#2426).
            self.line_start_pos = line_start;

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
                    self.emit(Token::Newline);
                }
                // Advance past the '\n' (or reach end-of-source).
                if eol < chars.len() {
                    pos = eol + 1;
                    self.line += 1; // physical newline consumed
                } else {
                    pos = chars.len() + 1;
                }
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
            self.emit(Token::Dedent);
        }

        self.emit(Token::Eof);
        Ok(())
    }

    fn handle_indent(&mut self, indent: usize, stack: &mut Vec<usize>) -> Result<()> {
        let current = *stack.last().expect("indent stack is never empty");
        if indent > current {
            // CPython's tokenizer caps indentation nesting at `MAXINDENT` (100)
            // levels, raising `IndentationError: too many levels of indentation`.
            // Because a deeper *compound statement* always requires a deeper
            // indent, this is what bounds statement-nesting depth.  Enforcing it
            // here turns pathological input — issue #2221's 10k-deep nested
            // `if True:` blocks — into a catchable exception instead of
            // overflowing the parser's native stack (SIGABRT).  `stack.len()`
            // counts the open indentation levels (the base level `0` is always
            // present), so a push would create level `stack.len()`; reject once
            // we already hold `MAX_INDENT_LEVELS` levels, matching CPython 3.12
            // which accepts 99 nested levels and rejects the 100th.
            if stack.len() >= MAX_INDENT_LEVELS {
                return Err(PyError::Lex("too many levels of indentation".to_string()));
            }
            stack.push(indent);
            self.emit(Token::Indent);
            return Ok(());
        }

        if indent < current {
            while indent < *stack.last().expect("indent stack is never empty") {
                stack.pop();
                self.emit(Token::Dedent);
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
            // Position at the start of the token about to be lexed.  After the
            // match, any physical newlines the token *span* crossed (only
            // possible for multi-line string / f-string / bytes literals) are
            // folded into the line counter.  The `'\n'` and EOF arms manage
            // their own line tracking and `return`/`continue` before reaching
            // the post-match advance, so they are never double-counted.
            let tok_start = pos;
            // Record the start column of the token about to be emitted (#2426):
            // its char offset within the current physical line.  `saturating_sub`
            // guards the (not expected) case where a continuation re-entry leaves
            // `pos` before the tracked line start.
            self.cur_col = tok_start.saturating_sub(self.line_start_pos) as u32;
            // Token count and line before the match: used to fill in the end
            // column of a token that was emitted this iteration (issue #2411).
            let tokens_before = self.tokens.len();
            let line_before = self.line;
            match chars.get(pos).copied() {
                None => {
                    // End of source.
                    if paren_depth == 0 && !line_continued {
                        self.emit(Token::Newline);
                    }
                    return Ok((chars.len() + 1, paren_depth));
                }
                Some('\n') => {
                    if line_continued {
                        // Backslash continuation: consume '\n' and keep going
                        // on the next physical line (do NOT emit Newline).
                        pos += 1;
                        self.line += 1; // physical newline consumed
                        self.line_start_pos = pos; // new physical line begins here (#2426)
                        // Skip leading whitespace on the continuation line.
                        while matches!(chars.get(pos), Some(&' ') | Some(&'\t')) {
                            pos += 1;
                        }
                        line_continued = false;
                        continue;
                    }
                    if paren_depth == 0 {
                        self.emit(Token::Newline);
                    }
                    // Advance past '\n' and return so that lex_source can
                    // process the indentation of the next line.  The physical
                    // newline is consumed here whether or not a `Newline` token
                    // was emitted (inside brackets it is suppressed but the line
                    // counter must still advance — issue #2227).
                    self.line += 1;
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
                    self.emit(tok);
                    pos = next;
                }
                Some('a'..='z') | Some('A'..='Z') | Some('_') => {
                    let c = chars[pos];
                    // rf"..." / fr"..." and all case variants — raw f-string
                    if ((c == 'r' || c == 'R')
                        && matches!(chars.get(pos + 1), Some('f') | Some('F'))
                        && matches!(chars.get(pos + 2), Some('"') | Some('\'')))
                        || ((c == 'f' || c == 'F')
                            && matches!(chars.get(pos + 1), Some('r') | Some('R'))
                            && matches!(chars.get(pos + 2), Some('"') | Some('\'')))
                    {
                        let quote = chars[pos + 2];
                        let (tok, next) =
                            lex_fstring(chars, pos + 3, quote, true, self.line_start_pos)?;
                        self.emit(tok);
                        pos = next;
                    } else if (c == 'f' || c == 'F')
                        && matches!(chars.get(pos + 1), Some('"') | Some('\''))
                    {
                        // Check for f-string prefix: f" or f' (or F" / F')
                        let quote = chars[pos + 1];
                        let (tok, next) =
                            lex_fstring(chars, pos + 2, quote, false, self.line_start_pos)?;
                        self.emit(tok);
                        pos = next;
                    } else if (c == 'b' || c == 'B')
                        && matches!(chars.get(pos + 1), Some('"') | Some('\''))
                    {
                        // Bytes literal: b"..." or b'...'
                        let (tok, next) = lex_bytes(chars, pos + 1, false)?;
                        self.emit(tok);
                        pos = next;
                    } else if (c == 'b' || c == 'B')
                        && matches!(chars.get(pos + 1), Some('r') | Some('R'))
                        && matches!(chars.get(pos + 2), Some('"') | Some('\''))
                    {
                        // Raw bytes literal: br"..." / bR"..." / BR"..." / Br"..."
                        let (tok, next) = lex_bytes(chars, pos + 2, true)?;
                        self.emit(tok);
                        pos = next;
                    } else if (c == 'r' || c == 'R')
                        && matches!(chars.get(pos + 1), Some('"') | Some('\''))
                    {
                        // Raw string literal: r"..." / R"..."
                        let (tok, next) = lex_string(chars, pos + 1, true)?;
                        self.emit(tok);
                        pos = next;
                    } else if (c == 'r' || c == 'R')
                        && matches!(chars.get(pos + 1), Some('b') | Some('B'))
                        && matches!(chars.get(pos + 2), Some('"') | Some('\''))
                    {
                        // Raw bytes literal: rb"..." / rB"..." / RB"..." / Rb"..."
                        let (tok, next) = lex_bytes(chars, pos + 2, true)?;
                        self.emit(tok);
                        pos = next;
                    } else if (c == 'u' || c == 'U')
                        && matches!(chars.get(pos + 1), Some('"') | Some('\''))
                    {
                        // Unicode string literal: u"..." / U"..."
                        // In Python 3.3+, u"..." is identical to a plain string literal.
                        // Combinations like ur"", ub"" are not valid in Python 3.
                        let (tok, next) = lex_string(chars, pos + 1, false)?;
                        self.emit(tok);
                        pos = next;
                    } else {
                        let (tok, next) = lex_ident_or_keyword(chars, pos);
                        self.emit(tok);
                        pos = next;
                    }
                }
                Some('"') | Some('\'') => {
                    let (tok, next) = lex_string(chars, pos, false)?;
                    self.emit(tok);
                    pos = next;
                }
                Some('+') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.emit(Token::PlusAssign);
                        pos += 2;
                    } else {
                        self.emit(Token::Plus);
                        pos += 1;
                    }
                }
                Some('-') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.emit(Token::MinusAssign);
                        pos += 2;
                    } else if chars.get(pos + 1) == Some(&'>') {
                        self.emit(Token::Arrow);
                        pos += 2;
                    } else {
                        self.emit(Token::Minus);
                        pos += 1;
                    }
                }
                Some('@') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.emit(Token::AtAssign);
                        pos += 2;
                    } else {
                        self.emit(Token::At);
                        pos += 1;
                    }
                }
                Some(';') => {
                    self.emit(Token::Semicolon);
                    pos += 1;
                }
                Some('*') => {
                    if chars.get(pos + 1) == Some(&'*') {
                        if chars.get(pos + 2) == Some(&'=') {
                            self.emit(Token::StarStarAssign);
                            pos += 3;
                        } else {
                            self.emit(Token::StarStar);
                            pos += 2;
                        }
                    } else if chars.get(pos + 1) == Some(&'=') {
                        self.emit(Token::StarAssign);
                        pos += 2;
                    } else {
                        self.emit(Token::Star);
                        pos += 1;
                    }
                }
                Some('/') => {
                    if chars.get(pos + 1) == Some(&'/') {
                        if chars.get(pos + 2) == Some(&'=') {
                            self.emit(Token::SlashSlashAssign);
                            pos += 3;
                        } else {
                            self.emit(Token::SlashSlash);
                            pos += 2;
                        }
                    } else if chars.get(pos + 1) == Some(&'=') {
                        self.emit(Token::SlashAssign);
                        pos += 2;
                    } else {
                        self.emit(Token::Slash);
                        pos += 1;
                    }
                }
                Some('%') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.emit(Token::PercentAssign);
                        pos += 2;
                    } else {
                        self.emit(Token::Percent);
                        pos += 1;
                    }
                }
                Some('&') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.emit(Token::AmpersandAssign);
                        pos += 2;
                    } else {
                        self.emit(Token::Ampersand);
                        pos += 1;
                    }
                }
                Some('|') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.emit(Token::PipeAssign);
                        pos += 2;
                    } else {
                        self.emit(Token::Pipe);
                        pos += 1;
                    }
                }
                Some('^') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.emit(Token::CaretAssign);
                        pos += 2;
                    } else {
                        self.emit(Token::Caret);
                        pos += 1;
                    }
                }
                Some('~') => {
                    self.emit(Token::Tilde);
                    pos += 1;
                }
                Some('<') => {
                    if chars.get(pos + 1) == Some(&'<') {
                        if chars.get(pos + 2) == Some(&'=') {
                            self.emit(Token::LShiftAssign);
                            pos += 3;
                        } else {
                            self.emit(Token::LShift);
                            pos += 2;
                        }
                    } else if chars.get(pos + 1) == Some(&'=') {
                        self.emit(Token::Le);
                        pos += 2;
                    } else {
                        self.emit(Token::Lt);
                        pos += 1;
                    }
                }
                Some('>') => {
                    if chars.get(pos + 1) == Some(&'>') {
                        if chars.get(pos + 2) == Some(&'=') {
                            self.emit(Token::RShiftAssign);
                            pos += 3;
                        } else {
                            self.emit(Token::RShift);
                            pos += 2;
                        }
                    } else if chars.get(pos + 1) == Some(&'=') {
                        self.emit(Token::Ge);
                        pos += 2;
                    } else {
                        self.emit(Token::Gt);
                        pos += 1;
                    }
                }
                Some('(') => {
                    self.emit(Token::LParen);
                    paren_depth += 1;
                    pos += 1;
                }
                Some(')') => {
                    self.emit(Token::RParen);
                    paren_depth = paren_depth.saturating_sub(1);
                    pos += 1;
                }
                Some('[') => {
                    self.emit(Token::LBracket);
                    paren_depth += 1;
                    pos += 1;
                }
                Some(']') => {
                    self.emit(Token::RBracket);
                    paren_depth = paren_depth.saturating_sub(1);
                    pos += 1;
                }
                Some('{') => {
                    self.emit(Token::LBrace);
                    paren_depth += 1;
                    pos += 1;
                }
                Some('}') => {
                    self.emit(Token::RBrace);
                    paren_depth = paren_depth.saturating_sub(1);
                    pos += 1;
                }
                Some(',') => {
                    self.emit(Token::Comma);
                    pos += 1;
                }
                Some(':') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.emit(Token::Walrus);
                        pos += 2;
                    } else {
                        self.emit(Token::Colon);
                        pos += 1;
                    }
                }
                Some('.') => {
                    if chars.get(pos + 1) == Some(&'.') && chars.get(pos + 2) == Some(&'.') {
                        // Ellipsis literal: `...`
                        self.emit(Token::Ellipsis);
                        pos += 3;
                    } else if matches!(chars.get(pos + 1), Some('0'..='9')) {
                        // Leading-dot float: `.5`, `.5e-3` etc.  Check whether the
                        // character immediately following the dot is a decimal digit;
                        // if so, lex the whole thing as a float literal.  Otherwise
                        // emit a plain Dot token (used for attribute access and import
                        // relative-path notation).
                        let (tok, next) = lex_leading_dot_float(chars, pos)?;
                        self.emit(tok);
                        pos = next;
                    } else {
                        self.emit(Token::Dot);
                        pos += 1;
                    }
                }
                Some('=') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.emit(Token::Eq);
                        pos += 2;
                    } else {
                        self.emit(Token::Assign);
                        pos += 1;
                    }
                }
                Some('!') => {
                    if chars.get(pos + 1) == Some(&'=') {
                        self.emit(Token::Ne);
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
                Some(c) if c.is_alphabetic() => {
                    // Unicode ID_Start character (non-ASCII): dispatch to identifier lexer.
                    let (tok, next) = lex_ident_or_keyword(chars, pos);
                    self.emit(tok);
                    pos = next;
                }
                Some(_) => return Err(PyError::Lex("invalid syntax".to_string())),
            }
            // Fill in the end column of the token emitted this iteration
            // (issue #2411).  `pos` now sits one past the token's last char, so
            // its end col is `pos - line_start_pos`.  Skip tokens whose lexing
            // crossed a physical newline (`line_before != self.line` would hold
            // after the fold below, but we test the source span directly): their
            // end col on the original line is meaningless, so leave the
            // placeholder 0 and let the parser treat it as "no end col".
            if self.tokens.len() > tokens_before
                && line_before == self.line
                && !chars[tok_start..pos].contains(&'\n')
            {
                let last = self.cols_end.len() - 1;
                self.cols_end[last] = pos.saturating_sub(self.line_start_pos) as u32;
            }
            // Fold any physical newlines crossed by the token just lexed into
            // the line counter (multi-line string / f-string / bytes literals).
            // For single-line tokens this span contains no '\n' and is a no-op.
            if pos > tok_start {
                self.advance_line_over(chars, tok_start, pos);
            }
        }
    }
}
