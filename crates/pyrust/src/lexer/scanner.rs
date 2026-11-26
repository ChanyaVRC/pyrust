/// Maximum number of indentation levels the lexer accepts, matching CPython's
/// tokenizer `MAXINDENT` (100).  The base (column-0) level is always present,
/// so this permits 99 nested indents and rejects the 100th with
/// `IndentationError: too many levels of indentation` — exactly CPython 3.12's
/// boundary.  Bounding indentation bounds compound-statement nesting depth,
/// which is what prevents deeply nested blocks from overflowing the parser's
/// native stack (issue #2221).
const MAX_INDENT_LEVELS: usize = 100;

/// Compile-time `SyntaxWarning` entry `(lineno, message)` emitted by the lexer.
pub type LexerWarning = (u32, String);

/// Tokens, per-token line/column spans, and lexer-level syntax warnings.
pub type TokensWithPosAndWarnings = (Vec<Token>, Vec<u32>, Vec<u32>, Vec<u32>, Vec<LexerWarning>);

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
    /// Pending compile-time `SyntaxWarning` entries `(1-based lineno, message)`
    /// collected during lexing (e.g. invalid escape sequences in string/bytes
    /// literals, matching CPython 3.12).
    warnings: Vec<(u32, String)>,
}

// Thread-local justification:
// (a) Why `thread_local!`: test-only counter observing how many Python
//     sources the current thread has tokenized, so a test can assert that a
//     code path (CLI startup, issue #3293) compiles no Python at all.
// (b) Multi-interpreter / multi-threaded impact: none outside tests; each test
//     thread counts its own sources.
#[cfg(test)]
thread_local! {
    static SOURCES_LEXED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many Python sources this thread has tokenized so far.
#[cfg(test)]
pub(crate) fn sources_lexed() -> usize {
    SOURCES_LEXED.with(std::cell::Cell::get)
}

impl Lexer {
    pub fn new(src: &str) -> Result<Self> {
        #[cfg(test)]
        SOURCES_LEXED.with(|count| count.set(count.get() + 1));
        let mut lexer = Self {
            tokens: Vec::new(),
            line_nos: Vec::new(),
            cols: Vec::new(),
            cols_end: Vec::new(),
            line: 1,
            line_start_pos: 0,
            cur_col: 0,
            warnings: Vec::new(),
        };
        lexer.lex_source_inner(src, false)?;
        Ok(lexer)
    }

    /// Lex an f-string replacement-field expression (`{ ... }`).  Inside `{...}`
    /// Python suppresses `NEWLINE` / `INDENT` / `DEDENT` tokens (just like inside
    /// `(...)`) while still tracking physical line numbers across newlines
    /// (PEP 701 / issue #2587).
    pub fn new_fstring_expr(src: &str) -> Result<Self> {
        let mut lexer = Self {
            tokens: Vec::new(),
            line_nos: Vec::new(),
            cols: Vec::new(),
            cols_end: Vec::new(),
            line: 1,
            line_start_pos: 0,
            cur_col: 0,
            warnings: Vec::new(),
        };
        lexer.lex_source_inner(src, true)?;
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

    /// Like [`Lexer::into_tokens_with_pos`] but also returns any compile-time
    /// `SyntaxWarning` entries `(lineno, message)` collected during lexing.
    pub fn into_tokens_with_pos_and_warnings(self) -> TokensWithPosAndWarnings {
        debug_assert_eq!(self.tokens.len(), self.line_nos.len());
        debug_assert_eq!(self.tokens.len(), self.cols.len());
        debug_assert_eq!(self.tokens.len(), self.cols_end.len());
        (
            self.tokens,
            self.line_nos,
            self.cols,
            self.cols_end,
            self.warnings,
        )
    }

    fn lex_source_inner(&mut self, src: &str, suppress_newlines: bool) -> Result<()> {
        // Work on the full source as a flat char array so that string literals
        // (including triple-quoted ones) can span line boundaries naturally.
        let chars: Vec<char> = src.chars().collect();
        self.lex_chars(&chars, suppress_newlines)
            .map_err(|error| self.locate_error(error, &chars, suppress_newlines))
    }

    /// Attach CPython's `SyntaxError` location to a lexing failure (#3402).
    ///
    /// Errors that already know their position ([`SyntaxErrorPending::Chars`]
    /// from the scanners, or lines from the indentation pass) are converted to
    /// lines; any other lexer error falls back to the start of the token being
    /// scanned, which is where CPython's tokenizer reports unterminated
    /// literals.  An f-string replacement-field sub-lexer
    /// (`suppress_newlines`) lexes a detached snippet whose coordinates mean
    /// nothing in the enclosing file, so it drops the location and lets the
    /// parser anchor the error on the f-string token instead.
    #[cold]
    fn locate_error(&self, error: PyError, chars: &[char], suppress_newlines: bool) -> PyError {
        if suppress_newlines {
            return match error.into_kind() {
                crate::error::PyErrorKind::Syntax(info) => PyError::lex(info.msg),
                other => PyError::new(other),
            };
        }
        match error.into_kind() {
            crate::error::PyErrorKind::Syntax(mut info) => {
                if let Some(crate::error::SyntaxErrorPending::Chars { cur, end_cur, rule }) =
                    info.pending
                {
                    let (lineno, offset) = char_line_and_column(chars, cur);
                    let (end_lineno, end_offset) = char_line_and_column(chars, end_cur);
                    let rule = match rule {
                        crate::error::SyntaxTextRule::TokenizerLine => rule,
                        crate::error::SyntaxTextRule::ParserLine { .. } => {
                            crate::error::SyntaxTextRule::ParserLine {
                                tokenizer_lineno: lineno,
                            }
                        }
                    };
                    info.lineno = Some(i64::from(lineno));
                    info.offset = Some(offset);
                    info.end_lineno = Some(i64::from(end_lineno));
                    info.end_offset = Some(end_offset);
                    info.pending = Some(crate::error::SyntaxErrorPending::Text {
                        rule,
                        newline_token: false,
                    });
                }
                PyError::new(crate::error::PyErrorKind::Syntax(info))
            }
            crate::error::PyErrorKind::Lex(msg) | crate::error::PyErrorKind::Parse(msg) => {
                let offset = i64::from(self.cur_col) + 1;
                PyError::syntax(crate::error::SyntaxErrorInfo::at_lines(
                    msg,
                    (self.line, offset),
                    (self.line, offset),
                    crate::error::SyntaxTextRule::TokenizerLine,
                    false,
                ))
            }
            other => PyError::new(other),
        }
    }

    fn lex_chars(&mut self, chars: &[char], suppress_newlines: bool) -> Result<()> {
        // CPython 3.12 dual-column indentation stack: `(col, altcol)` where
        // `col` expands tabs to 8-column tab stops and `altcol` counts tabs as
        // 1 column for inconsistent tab/space mixing (`TabError`) detection.
        let mut indent_stack: Vec<(usize, usize)> = vec![(0, 0)];
        let mut paren_depth: usize = 0; // track ( [ { for implicit line continuation
        let mut pos = 0;

        while pos <= chars.len() {
            // Measure the indentation of the current logical line.
            // We do this at the start of each physical line (pos points just
            // after the preceding '\n', or at 0 for the first line).
            let line_start = pos;
            // Track where this physical line begins so emitted tokens can record
            // their start column as `token_start - line_start_pos` (#2426).
            self.line_start_pos = line_start;

            // Measure leading whitespace using 8-column tab expansion plus
            // alternate 1-column tab counter for TabError detection.
            let (indent_chars, indent_col, indent_altcol) = if suppress_newlines {
                let mut n = 0;
                while matches!(chars.get(pos + n), Some(&' ') | Some(&'\t') | Some(&'\x0c')) {
                    n += 1;
                }
                (n, n, n)
            } else {
                count_indent_chars(&chars, pos)
            };
            pos += indent_chars;

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
                if !suppress_newlines && paren_depth == 0 {
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
            if !suppress_newlines && paren_depth == 0 {
                // INDENT / DEDENT sit just past the indentation, which is where
                // CPython anchors `unexpected indent` (#3402).
                self.cur_col = indent_chars as u32;
                self.handle_indent(
                    (indent_col, indent_altcol),
                    &mut indent_stack,
                    line_end - line_start,
                )?;
            }

            // Lex tokens on this logical line.  `lex_line_tokens` advances
            // `pos` past the last token on the line (including any '\n') and
            // returns the updated paren depth.  It may consume multiple physical
            // lines when it encounters a triple-quoted string.
            pos = content_start; // reset to after-indent start
            let _ = line_start; // silence unused-variable warning
            let new_depth;
            (pos, new_depth) = self.lex_line_tokens(&chars, pos, paren_depth, suppress_newlines)?;
            paren_depth = new_depth;
        }

        while indent_stack.len() > 1 {
            indent_stack.pop();
            self.emit(Token::Dedent);
        }

        // CPython reports a failure at the end of input on the last line, at
        // the tokenizer cursor just past that line's (CRLF-translated, or for
        // `exec` input appended) newline (#3402).
        if !suppress_newlines {
            let end = chars.len();
            let ends_with_newline = chars.last() == Some(&'\n');
            let content_end = end - usize::from(ends_with_newline);
            let line_start = chars[..content_end]
                .iter()
                .rposition(|&c| c == '\n')
                .map_or(0, |i| i + 1);
            let line_len = content_end
                - line_start
                - usize::from(chars[line_start..content_end].last() == Some(&'\r'));
            if ends_with_newline {
                self.line = self.line.saturating_sub(1).max(1);
            }
            self.cur_col = (line_len + 1) as u32;
        }
        self.emit(Token::Eof);
        Ok(())
    }

    /// An indentation failure on the current line, located like CPython 3.12's
    /// tokenizer-error path (`_Pypegen_tokenizer_error`): `TabError` and
    /// `too many levels of indentation` at `(line, 1)` with `end_offset` 0;
    /// `unindent does not match` one past the line's newline with
    /// `end_offset` -1.  Either way `text` is the line with its newline.
    /// CPython raises the `int_max_str_digits` limit error for a decimal
    /// literal from the parser, on the literal's line but deliberately without
    /// columns (`offset == end_offset == 0`: "avoid a wall of 1000s of '^'s"),
    /// #3402.  Other number errors pass through.
    #[cold]
    fn huge_literal_error(&self, error: PyError) -> PyError {
        match error.into_kind() {
            crate::error::PyErrorKind::Lex(msg)
                if msg.ends_with("to avoid decimal conversion limits.") =>
            {
                PyError::syntax(crate::error::SyntaxErrorInfo::at_lines(
                    msg,
                    (self.line, 0),
                    (self.line, 0),
                    crate::error::SyntaxTextRule::ParserLine {
                        tokenizer_lineno: self.line,
                    },
                    false,
                ))
            }
            other => PyError::new(other),
        }
    }

    #[cold]
    fn indentation_error(&self, msg: &str, offset: i64, end_offset: i64) -> PyError {
        PyError::syntax(crate::error::SyntaxErrorInfo::at_lines(
            msg,
            (self.line, offset),
            (self.line, end_offset),
            crate::error::SyntaxTextRule::ParserLine {
                tokenizer_lineno: self.line,
            },
            false,
        ))
    }

    fn handle_indent(
        &mut self,
        (col, altcol): (usize, usize),
        stack: &mut Vec<(usize, usize)>,
        line_len: usize,
    ) -> Result<()> {
        const TAB_ERROR: &str = "inconsistent use of tabs and spaces in indentation";
        let (cur_col, cur_altcol) = *stack.last().expect("indent stack is never empty");
        if col == cur_col {
            if altcol != cur_altcol {
                return Err(self.indentation_error(TAB_ERROR, 1, 0));
            }
            return Ok(());
        }
        if col > cur_col {
            if altcol <= cur_altcol {
                return Err(self.indentation_error(TAB_ERROR, 1, 0));
            }
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
                return Err(self.indentation_error("too many levels of indentation", 1, 0));
            }
            stack.push((col, altcol));
            self.emit(Token::Indent);
            return Ok(());
        }

        while col < stack.last().expect("indent stack is never empty").0 {
            stack.pop();
            self.emit(Token::Dedent);
        }
        let (after_col, after_altcol) = *stack.last().expect("indent stack is never empty");
        if col != after_col {
            // CPython reports the tokenizer cursor, which sits past the whole
            // line including its (CRLF-translated) newline.
            return Err(self.indentation_error(
                "unindent does not match any outer indentation level",
                line_len as i64 + 1,
                -1,
            ));
        }
        if altcol != after_altcol {
            return Err(self.indentation_error(TAB_ERROR, 1, 0));
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
        suppress_newlines: bool,
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
                    if !suppress_newlines && paren_depth == 0 && !line_continued {
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
                    if !suppress_newlines && paren_depth == 0 {
                        // CPython translates CRLF to LF before tokenizing, so a
                        // NEWLINE token starts at the `\r` (#3402).
                        if pos > 0 && chars[pos - 1] == '\r' {
                            self.cur_col = self.cur_col.saturating_sub(1);
                        }
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
                    let (tok, next) =
                        lex_number(chars, pos).map_err(|e| self.huge_literal_error(e))?;
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
                        let mut first_invalid = None;
                        let (tok, next) = lex_fstring(
                            chars,
                            pos + 3,
                            quote,
                            true,
                            self.line_start_pos,
                            &mut first_invalid,
                        )?;
                        self.emit(tok);
                        pos = next;
                    } else if (c == 'f' || c == 'F')
                        && matches!(chars.get(pos + 1), Some('"') | Some('\''))
                    {
                        // Check for f-string prefix: f" or f' (or F" / F')
                        let quote = chars[pos + 1];
                        let mut first_invalid = None;
                        let (tok, next) = lex_fstring(
                            chars,
                            pos + 2,
                            quote,
                            false,
                            self.line_start_pos,
                            &mut first_invalid,
                        )?;
                        if let Some(msg) = first_invalid {
                            self.warnings.push((self.line, msg));
                        }
                        self.emit(tok);
                        pos = next;
                    } else if (c == 'b' || c == 'B')
                        && matches!(chars.get(pos + 1), Some('"') | Some('\''))
                    {
                        // Bytes literal: b"..." or b'...'
                        let mut first_invalid = None;
                        let (tok, next) = lex_bytes(chars, pos + 1, false, &mut first_invalid)
                            .map_err(|e| literal_content_error(e, chars, tok_start))?;
                        if let Some(msg) = first_invalid {
                            self.warnings.push((self.line, msg));
                        }
                        self.emit(tok);
                        pos = next;
                    } else if (c == 'b' || c == 'B')
                        && matches!(chars.get(pos + 1), Some('r') | Some('R'))
                        && matches!(chars.get(pos + 2), Some('"') | Some('\''))
                    {
                        // Raw bytes literal: br"..." / bR"..." / BR"..." / Br"..."
                        let mut first_invalid = None;
                        let (tok, next) = lex_bytes(chars, pos + 2, true, &mut first_invalid)
                            .map_err(|e| literal_content_error(e, chars, tok_start))?;
                        self.emit(tok);
                        pos = next;
                    } else if (c == 'r' || c == 'R')
                        && matches!(chars.get(pos + 1), Some('"') | Some('\''))
                    {
                        // Raw string literal: r"..." / R"..."
                        let mut first_invalid = None;
                        let (tok, next) = lex_string(chars, pos + 1, true, &mut first_invalid)
                            .map_err(|e| literal_content_error(e, chars, tok_start))?;
                        self.emit(tok);
                        pos = next;
                    } else if (c == 'r' || c == 'R')
                        && matches!(chars.get(pos + 1), Some('b') | Some('B'))
                        && matches!(chars.get(pos + 2), Some('"') | Some('\''))
                    {
                        // Raw bytes literal: rb"..." / rB"..." / RB"..." / Rb"..."
                        let mut first_invalid = None;
                        let (tok, next) = lex_bytes(chars, pos + 2, true, &mut first_invalid)
                            .map_err(|e| literal_content_error(e, chars, tok_start))?;
                        self.emit(tok);
                        pos = next;
                    } else if (c == 'u' || c == 'U')
                        && matches!(chars.get(pos + 1), Some('"') | Some('\''))
                    {
                        // Unicode string literal: u"..." / U"..."
                        // In Python 3.3+, u"..." is identical to a plain string literal.
                        // Combinations like ur"", ub"" are not valid in Python 3.
                        let mut first_invalid = None;
                        let (tok, next) = lex_string(chars, pos + 1, false, &mut first_invalid)
                            .map_err(|e| literal_content_error(e, chars, tok_start))?;
                        if let Some(msg) = first_invalid {
                            self.warnings.push((self.line, msg));
                        }
                        self.emit(tok);
                        pos = next;
                    } else {
                        let (tok, next) = lex_ident_or_keyword(chars, pos);
                        self.emit(tok);
                        pos = next;
                    }
                }
                Some('"') | Some('\'') => {
                    let mut first_invalid = None;
                    let (tok, next) = lex_string(chars, pos, false, &mut first_invalid)
                        .map_err(|e| literal_content_error(e, chars, tok_start))?;
                    if let Some(msg) = first_invalid {
                        self.warnings.push((self.line, msg));
                    }
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
                        return Err(PyError::lex("expected '=' after '!'".to_string()));
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
                // CPython tokenizes an unknown character as an error token and
                // the parser reports `invalid syntax` on it (#3402).
                Some(_) => {
                    return Err(PyError::syntax(crate::error::SyntaxErrorInfo::at_chars(
                        "invalid syntax",
                        pos + 1,
                        pos + 2,
                        crate::error::SyntaxTextRule::ParserLine {
                            tokenizer_lineno: 0,
                        },
                    )));
                }
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

/// 1-based line and CPython `tok->cur`-style offset (the 0-based column of
/// `cur`) of the absolute char index `cur` (#3402).  The line is the one that
/// holds the char just before `cur`, so a cursor sitting right after the last
/// char of a line still reports that line.
#[cold]
fn char_line_and_column(chars: &[char], cur: usize) -> (u32, i64) {
    // `cur` may sit one past the end (an error on the input's last char).
    let before = &chars[..cur.saturating_sub(1).min(chars.len())];
    let line = 1 + before.iter().filter(|&&c| c == '\n').count() as u32;
    let line_start = before.iter().rposition(|&c| c == '\n').map_or(0, |i| i + 1);
    (line, (cur - line_start) as i64)
}

/// Locate a string/bytes literal scanning error (#3402).  CPython decodes
/// literals in the parser, so a content error (bad escape, non-ASCII byte)
/// spans the whole literal token, while an unterminated literal is a tokenizer
/// error at its first char — the lexer's default location, so those pass
/// through untouched.
#[cold]
fn literal_content_error(error: PyError, chars: &[char], tok_start: usize) -> PyError {
    match error.kind() {
        crate::error::PyErrorKind::Lex(msg)
            if !(msg.starts_with("unterminated") || msg.starts_with("EOL while scanning")) =>
        {
            let end = literal_token_end(chars, tok_start);
            PyError::syntax(crate::error::SyntaxErrorInfo::at_chars(
                msg.clone(),
                tok_start + 1,
                end + 1,
                crate::error::SyntaxTextRule::ParserLine {
                    tokenizer_lineno: 0,
                },
            ))
        }
        _ => error,
    }
}

/// One past the closing quote of the string/bytes literal that starts (prefix
/// letters included) at `start`, or the end of input when it never closes.
/// Only sizes a literal-content error, so it does not validate escapes.
#[cold]
fn literal_token_end(chars: &[char], start: usize) -> usize {
    let mut pos = start;
    while chars.get(pos).is_some_and(char::is_ascii_alphabetic) {
        pos += 1;
    }
    let Some(&quote) = chars.get(pos) else {
        return chars.len();
    };
    let triple = chars.get(pos + 1) == Some(&quote) && chars.get(pos + 2) == Some(&quote);
    pos += if triple { 3 } else { 1 };
    while pos < chars.len() {
        match chars[pos] {
            '\\' => pos += 2,
            c if c == quote => {
                if !triple {
                    return pos + 1;
                }
                if chars.get(pos + 1) == Some(&quote) && chars.get(pos + 2) == Some(&quote) {
                    return pos + 3;
                }
                pos += 1;
            }
            '\n' if !triple => return pos,
            _ => pos += 1,
        }
    }
    chars.len()
}
