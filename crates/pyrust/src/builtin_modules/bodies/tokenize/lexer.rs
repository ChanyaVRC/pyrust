//! Python 3.12 full-fidelity stateful tokenizer (PEP 701 f-strings, comments,
//! NL/NEWLINE, INDENT/DEDENT, char AND UTF-8 byte offsets).
//!
//! Shared between the `tokenize` module (which consumes lines lazily via
//! `StatefulLexer::feed_line` / `finish`) and the `ast` module (which tokenizes
//! full source strings via `tokenize_source`).

// Token type constants matching CPython 3.12 `token.py`.
pub(crate) const TOK_ENDMARKER: i64 = 0;
pub(crate) const TOK_NAME: i64 = 1;
pub(crate) const TOK_NUMBER: i64 = 2;
pub(crate) const TOK_STRING: i64 = 3;
pub(crate) const TOK_NEWLINE: i64 = 4;
pub(crate) const TOK_INDENT: i64 = 5;
pub(crate) const TOK_DEDENT: i64 = 6;
pub(crate) const TOK_LPAR: i64 = 7;
pub(crate) const TOK_RPAR: i64 = 8;
pub(crate) const TOK_LSQB: i64 = 9;
pub(crate) const TOK_RSQB: i64 = 10;
pub(crate) const TOK_COLON: i64 = 11;
pub(crate) const TOK_COMMA: i64 = 12;
pub(crate) const TOK_SEMI: i64 = 13;
pub(crate) const TOK_PLUS: i64 = 14;
pub(crate) const TOK_MINUS: i64 = 15;
pub(crate) const TOK_STAR: i64 = 16;
pub(crate) const TOK_SLASH: i64 = 17;
pub(crate) const TOK_VBAR: i64 = 18;
pub(crate) const TOK_AMPER: i64 = 19;
pub(crate) const TOK_LESS: i64 = 20;
pub(crate) const TOK_GREATER: i64 = 21;
pub(crate) const TOK_EQUAL: i64 = 22;
pub(crate) const TOK_DOT: i64 = 23;
pub(crate) const TOK_PERCENT: i64 = 24;
pub(crate) const TOK_LBRACE: i64 = 25;
pub(crate) const TOK_RBRACE: i64 = 26;
pub(crate) const TOK_EQEQUAL: i64 = 27;
pub(crate) const TOK_NOTEQUAL: i64 = 28;
pub(crate) const TOK_LESSEQUAL: i64 = 29;
pub(crate) const TOK_GREATEREQUAL: i64 = 30;
pub(crate) const TOK_TILDE: i64 = 31;
pub(crate) const TOK_CIRCUMFLEX: i64 = 32;
pub(crate) const TOK_LEFTSHIFT: i64 = 33;
pub(crate) const TOK_RIGHTSHIFT: i64 = 34;
pub(crate) const TOK_DOUBLESTAR: i64 = 35;
pub(crate) const TOK_PLUSEQUAL: i64 = 36;
pub(crate) const TOK_MINEQUAL: i64 = 37;
pub(crate) const TOK_STAREQUAL: i64 = 38;
pub(crate) const TOK_SLASHEQUAL: i64 = 39;
pub(crate) const TOK_PERCENTEQUAL: i64 = 40;
pub(crate) const TOK_AMPEREQUAL: i64 = 41;
pub(crate) const TOK_VBAREQUAL: i64 = 42;
pub(crate) const TOK_CIRCUMFLEXEQUAL: i64 = 43;
pub(crate) const TOK_LEFTSHIFTEQUAL: i64 = 44;
pub(crate) const TOK_RIGHTSHIFTEQUAL: i64 = 45;
pub(crate) const TOK_DOUBLESTAREQUAL: i64 = 46;
pub(crate) const TOK_DOUBLESLASH: i64 = 47;
pub(crate) const TOK_DOUBLESLASHEQUAL: i64 = 48;
pub(crate) const TOK_AT: i64 = 49;
pub(crate) const TOK_ATEQUAL: i64 = 50;
pub(crate) const TOK_RARROW: i64 = 51;
pub(crate) const TOK_ELLIPSIS: i64 = 52;
pub(crate) const TOK_COLONEQUAL: i64 = 53;
pub(crate) const TOK_EXCLAMATION: i64 = 54;
pub(crate) const TOK_OP: i64 = 55;
pub(crate) const TOK_FSTRING_START: i64 = 61;
pub(crate) const TOK_FSTRING_MIDDLE: i64 = 62;
pub(crate) const TOK_FSTRING_END: i64 = 63;
pub(crate) const TOK_COMMENT: i64 = 64;
pub(crate) const TOK_NL: i64 = 65;
pub(crate) const TOK_ERRORTOKEN: i64 = 66;

/// A single token produced by [`StatefulLexer`].
#[derive(Debug, Clone)]
pub(crate) struct LexToken {
    /// Standard token type (`TOK_OP` for operators/delimiters).
    pub tok_type: i64,
    /// Exact token type (e.g. `TOK_PLUS` for `+`).
    pub exact_type: i64,
    /// Raw source text of the token.
    pub string: String,
    /// 1-based start line.
    pub start_line: u32,
    /// 0-based start char column (for `tokenize.TokenInfo`).
    pub start_col: u32,
    /// 0-based start UTF-8 byte column (for `ast` `col_offset`).
    pub start_byte_col: u32,
    /// 1-based end line.
    pub end_line: u32,
    /// 0-based end char column (for `tokenize.TokenInfo`).
    pub end_col: u32,
    /// 0-based end UTF-8 byte column (for `ast` `end_col_offset`).
    pub end_byte_col: u32,
    /// Source line text (for `tokenize.TokenInfo.line`).
    pub line: String,
}

/// Lexer error kind distinguishing `TokenError`, `IndentationError`, and `TabError`.
#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum LexError {
    TokenError {
        msg: String,
        line: u32,
        col: u32,
    },
    IndentationError {
        msg: String,
        line: u32,
        col: u32,
        text: String,
    },
    TabError {
        msg: String,
        line: u32,
        col: u32,
        text: String,
    },
}

/// Active lexical nesting mode on the mode stack.
#[derive(Debug, Clone)]
enum LexMode {
    /// Normal Python code (top-level or inside an f-string `{...}` field).
    Code {
        is_fstring_expr: bool,
        paren_depth: usize,
        brace_depth: usize,
    },
    /// Literal text portion of an f-string.
    FStringText {
        quote: String,
        is_triple: bool,
        is_raw: bool,
    },
    /// Format specification portion of an f-string field (after `:`).
    FStringFormatSpec {
        is_triple: bool,
        is_raw: bool,
        just_emitted_middle: bool,
    },
}

/// Multi-line string or f-string-middle accumulator across physical lines.
#[derive(Debug, Clone)]
struct MultilineAccum {
    is_fstring_middle: bool,
    quote: String,
    is_triple: bool,
    start_line: u32,
    start_col: u32,
    start_byte_col: u32,
    content: String,
    lines_accum: String,
}

/// Stateful line-by-line Python 3.12 tokenizer.
pub(crate) struct StatefulLexer {
    /// 1-based current line number.
    lnum: u32,
    /// Stack of indentation column counts (in 8-space tab-expanded columns).
    indents: Vec<usize>,
    /// Whether the current logical line has already emitted a real token.
    has_logical_token: bool,
    /// Whether the previous physical line ended with an explicit `\` continuation.
    continued: bool,
    /// Active mode stack (bottom is always `Code { is_fstring_expr: false, .. }`).
    modes: Vec<LexMode>,
    /// Active multi-line string (regular or f-string middle) spanning lines.
    multiline: Option<MultilineAccum>,
    /// Line number where the outermost unclosed delimiter or string started.
    paren_start: Option<(u32, u32)>,
    /// Whether any line has been fed yet.
    fed_any_line: bool,
    /// Whether the last fed line ended with a newline (`\n` or `\r`).
    last_ended_with_nl: bool,
    /// Last line text (for trailing implicit NEWLINE at EOF).
    last_line_text: String,
    /// Last line char length.
    last_line_char_len: u32,
    /// Last line byte length.
    last_line_byte_len: u32,
    /// Track indentation style for TabError detection: (spaces_count, tabs_count) per level.
    indent_styles: Vec<(usize, usize)>,
    /// Whether to raise `TabError` on mixed tabs/spaces (enabled for `ast.parse`,
    /// disabled for `tabnanny`).
    check_tabs: bool,
    /// `tokenize`-module mode: reproduce what CPython 3.12.3's C tokenizer
    /// (driven with `extra_tokens=True`) reports, including its error
    /// messages and `(lineno, offset)` positions, instead of the more
    /// lenient behaviour `ast` and `tabnanny` rely on.
    exact: bool,
    /// Parallel to `indents`: the indentation width with tabs counted as one
    /// column (CPython's `altcol`), for the exact-mode `TabError` check.
    alt_indents: Vec<usize>,
    /// An exact-mode error found while scanning the current line: the tokens
    /// before it are still returned by `feed_line`, and the error is handed
    /// out afterwards by [`StatefulLexer::take_deferred_error`], so a
    /// consumer sees them in CPython's order (tokens first, then the error).
    deferred_error: Option<LexError>,
}

impl StatefulLexer {
    /// The lexer behind `tokenize.generate_tokens` / `tokenize.tokenize`.
    pub(crate) fn for_tokenize() -> Self {
        let mut lexer = Self::with_tab_check(false);
        lexer.exact = true;
        lexer
    }

    /// The error found on the line just fed, if any (exact mode only).
    pub(crate) fn take_deferred_error(&mut self) -> Option<LexError> {
        self.deferred_error.take()
    }

    /// 1-based number of the last physical line fed.
    pub(crate) fn line_number(&self) -> u32 {
        self.lnum
    }

    /// Where the outermost still-open f-string, bracket or string started.
    pub(crate) fn open_origin(&self) -> Option<(u32, u32)> {
        self.paren_start
    }

    pub(crate) fn with_tab_check(check_tabs: bool) -> Self {
        Self {
            exact: false,
            alt_indents: vec![0],
            deferred_error: None,
            lnum: 0,
            indents: vec![0],
            has_logical_token: false,
            continued: false,
            modes: vec![LexMode::Code {
                is_fstring_expr: false,
                paren_depth: 0,
                brace_depth: 0,
            }],
            multiline: None,
            paren_start: None,
            fed_any_line: false,
            last_ended_with_nl: true,
            last_line_text: String::new(),
            last_line_char_len: 0,
            last_line_byte_len: 0,
            indent_styles: vec![(0, 0)],
            check_tabs,
        }
    }

    fn total_paren_depth(&self) -> usize {
        let mut depth = 0;
        for m in &self.modes {
            match m {
                LexMode::Code {
                    is_fstring_expr,
                    paren_depth,
                    brace_depth,
                } => {
                    depth += paren_depth + brace_depth;
                    if *is_fstring_expr {
                        depth += 1;
                    }
                }
                LexMode::FStringText { .. } | LexMode::FStringFormatSpec { .. } => {
                    depth += 1;
                }
            }
        }
        depth
    }

    fn in_implicit_continuation(&self) -> bool {
        self.total_paren_depth() > 0
    }

    /// Feed a single physical line (as returned by `readline()`, including any
    /// trailing `\n` / `\r\n` / `\r`) and return all tokens produced on it.
    pub(crate) fn feed_line(&mut self, line: &str) -> Result<Vec<LexToken>, LexError> {
        self.lnum += 1;
        self.fed_any_line = true;
        let lnum = self.lnum;
        let mut out = Vec::new();

        // Precompute char indices -> byte offsets for O(1) char/byte column conversion.
        let chars: Vec<char> = line.chars().collect();
        let mut char_to_byte: Vec<u32> = Vec::with_capacity(chars.len() + 1);
        let mut b = 0u32;
        for &c in &chars {
            char_to_byte.push(b);
            b += c.len_utf8() as u32;
        }
        char_to_byte.push(b);

        let line_char_len = chars.len() as u32;
        let line_byte_len = b;
        self.last_line_text = line.to_string();
        self.last_line_char_len = line_char_len;
        self.last_line_byte_len = line_byte_len;
        self.last_ended_with_nl = line.ends_with('\n') || line.ends_with('\r');

        if self.exact && chars.contains(&'\0') {
            return Err(LexError::TokenError {
                msg: "source code cannot contain null bytes".to_string(),
                line: lnum,
                col: 0,
            });
        }

        let mut pos: usize = 0;

        // If we are inside a multi-line triple-quoted string or f-string literal,
        // continue scanning it first.
        if let Some(accum) = self.multiline.take() {
            if accum.is_fstring_middle {
                match self.continue_fstring_middle(accum, &chars, &char_to_byte, line, &mut out)? {
                    Some(new_pos) => {
                        pos = new_pos;
                    }
                    None => {
                        // Still inside multi-line f-string middle.
                        return Ok(out);
                    }
                }
            } else {
                match self.continue_regular_string(accum, &chars, &char_to_byte, line, &mut out) {
                    Some(new_pos) => {
                        pos = new_pos;
                        self.has_logical_token = true;
                    }
                    None => {
                        // Still inside multi-line string.
                        return Ok(out);
                    }
                }
            }
        } else if !self.continued && !self.in_implicit_continuation() {
            // Beginning of a new logical line at parenlev == 0: check indentation
            // unless this is a blank or comment-only line.
            let mut col_idx = 0usize;
            let mut visual_col = 0usize;
            let mut alt_col = 0usize;
            let mut spaces = 0usize;
            let mut tabs = 0usize;
            while col_idx < chars.len() {
                match chars[col_idx] {
                    ' ' => {
                        visual_col += 1;
                        alt_col += 1;
                        spaces += 1;
                        col_idx += 1;
                    }
                    '\t' => {
                        visual_col = (visual_col / 8 + 1) * 8;
                        alt_col += 1;
                        tabs += 1;
                        col_idx += 1;
                    }
                    '\x0C' => {
                        // Form feed resets column to 0 in CPython.
                        visual_col = 0;
                        alt_col = 0;
                        col_idx += 1;
                    }
                    _ => break,
                }
            }
            pos = col_idx;

            // Blank or comment-only line?
            if pos >= chars.len() || matches!(chars[pos], '\n' | '\r' | '#') {
                if pos < chars.len() && chars[pos] == '#' {
                    let comment_start = pos;
                    while pos < chars.len() && !matches!(chars[pos], '\n' | '\r') {
                        pos += 1;
                    }
                    let comment_str: String = chars[comment_start..pos].iter().collect();
                    out.push(LexToken {
                        tok_type: TOK_COMMENT,
                        exact_type: TOK_COMMENT,
                        string: comment_str,
                        start_line: lnum,
                        start_col: comment_start as u32,
                        start_byte_col: char_to_byte[comment_start],
                        end_line: lnum,
                        end_col: pos as u32,
                        end_byte_col: char_to_byte[pos],
                        line: line.to_string(),
                    });
                }
                let nl_start = pos;
                let nl_str: String = chars[nl_start..].iter().collect();
                let (end_col, end_byte_col) = if nl_str.is_empty() {
                    (nl_start as u32 + 1, char_to_byte[nl_start] + 1)
                } else {
                    (line_char_len, line_byte_len)
                };
                out.push(LexToken {
                    tok_type: TOK_NL,
                    exact_type: TOK_NL,
                    string: nl_str,
                    start_line: lnum,
                    start_col: nl_start as u32,
                    start_byte_col: char_to_byte[nl_start],
                    end_line: lnum,
                    end_col,
                    end_byte_col,
                    line: line.to_string(),
                });
                return Ok(out);
            }

            // Non-blank line at start of logical line: emit INDENT / DEDENT.
            let current_indent = *self.indents.last().unwrap();
            if visual_col > current_indent {
                // Check for inconsistent tab/space mixing with previous level.
                if self.exact {
                    if alt_col <= self.alt_indents.last().copied().unwrap_or(0) {
                        return Err(LexError::TabError {
                            msg: "inconsistent use of tabs and spaces in indentation".to_string(),
                            line: lnum,
                            col: pos as u32,
                            text: line.to_string(),
                        });
                    }
                } else if self.check_tabs
                    && let Some(&(prev_s, prev_t)) = self.indent_styles.last()
                    && ((prev_s > 0 && tabs > 0 && spaces < prev_s)
                        || (prev_t > 0 && spaces > 0 && tabs < prev_t))
                {
                    return Err(LexError::TabError {
                        msg: "inconsistent use of tabs and spaces in indentation".to_string(),
                        line: lnum,
                        col: pos as u32,
                        text: line.to_string(),
                    });
                }
                self.indents.push(visual_col);
                self.indent_styles.push((spaces, tabs));
                self.alt_indents.push(alt_col);
                let indent_str: String = chars[0..pos].iter().collect();
                out.push(LexToken {
                    tok_type: TOK_INDENT,
                    exact_type: TOK_INDENT,
                    string: indent_str,
                    start_line: lnum,
                    start_col: 0,
                    start_byte_col: 0,
                    end_line: lnum,
                    end_col: pos as u32,
                    end_byte_col: char_to_byte[pos],
                    line: line.to_string(),
                });
            } else {
                while visual_col < *self.indents.last().unwrap() {
                    self.indents.pop();
                    self.indent_styles.pop();
                    self.alt_indents.pop();
                    out.push(LexToken {
                        tok_type: TOK_DEDENT,
                        exact_type: TOK_DEDENT,
                        string: String::new(),
                        start_line: lnum,
                        start_col: pos as u32,
                        start_byte_col: char_to_byte[pos],
                        end_line: lnum,
                        end_col: pos as u32,
                        end_byte_col: char_to_byte[pos],
                        line: line.to_string(),
                    });
                }
                if visual_col != *self.indents.last().unwrap() {
                    return Err(LexError::IndentationError {
                        msg: "unindent does not match any outer indentation level".to_string(),
                        line: lnum,
                        col: pos as u32,
                        text: line.to_string(),
                    });
                }
                // Check same indent column with mixed tabs/spaces.
                if self.exact {
                    if alt_col != self.alt_indents.last().copied().unwrap_or(0) {
                        return Err(LexError::TabError {
                            msg: "inconsistent use of tabs and spaces in indentation".to_string(),
                            line: lnum,
                            col: pos as u32,
                            text: line.to_string(),
                        });
                    }
                } else if self.check_tabs
                    && let Some(&(prev_s, prev_t)) = self.indent_styles.last()
                    && (spaces, tabs) != (prev_s, prev_t)
                    && (tabs > 0 || prev_t > 0)
                {
                    return Err(LexError::TabError {
                        msg: "inconsistent use of tabs and spaces in indentation".to_string(),
                        line: lnum,
                        col: pos as u32,
                        text: line.to_string(),
                    });
                }
            }
        } else {
            self.continued = false;
        }

        // Main per-line token scanning loop.
        while pos < chars.len() {
            if self.deferred_error.is_some() {
                break;
            }
            match self.modes.last().unwrap().clone() {
                LexMode::FStringText {
                    quote,
                    is_triple,
                    is_raw,
                } => {
                    pos = self.scan_fstring_text(
                        pos,
                        &chars,
                        &char_to_byte,
                        line,
                        &quote,
                        is_triple,
                        is_raw,
                        &mut out,
                    )?;
                    if self.multiline.is_some() {
                        return Ok(out);
                    }
                }
                LexMode::FStringFormatSpec {
                    is_triple,
                    is_raw,
                    just_emitted_middle,
                } => {
                    pos = self.scan_fstring_format_spec(
                        pos,
                        &chars,
                        &char_to_byte,
                        line,
                        is_triple,
                        is_raw,
                        just_emitted_middle,
                        &mut out,
                    )?;
                    if self.multiline.is_some() {
                        return Ok(out);
                    }
                }
                LexMode::Code { .. } => {
                    // Skip horizontal whitespace.
                    while pos < chars.len() && matches!(chars[pos], ' ' | '\t' | '\x0C') {
                        pos += 1;
                    }
                    if pos >= chars.len() {
                        break;
                    }

                    let c = chars[pos];

                    // Line ending.
                    if c == '\n' || c == '\r' {
                        let nl_start = pos;
                        let nl_str: String = chars[nl_start..].iter().collect();
                        let tok_type = if self.in_implicit_continuation() || !self.has_logical_token
                        {
                            TOK_NL
                        } else {
                            self.has_logical_token = false;
                            TOK_NEWLINE
                        };
                        out.push(LexToken {
                            tok_type,
                            exact_type: tok_type,
                            string: nl_str,
                            start_line: lnum,
                            start_col: nl_start as u32,
                            start_byte_col: char_to_byte[nl_start],
                            end_line: lnum,
                            end_col: line_char_len,
                            end_byte_col: line_byte_len,
                            line: line.to_string(),
                        });
                        break;
                    }

                    // Comment.
                    if c == '#' {
                        let comment_start = pos;
                        while pos < chars.len() && !matches!(chars[pos], '\n' | '\r') {
                            pos += 1;
                        }
                        let comment_str: String = chars[comment_start..pos].iter().collect();
                        out.push(LexToken {
                            tok_type: TOK_COMMENT,
                            exact_type: TOK_COMMENT,
                            string: comment_str,
                            start_line: lnum,
                            start_col: comment_start as u32,
                            start_byte_col: char_to_byte[comment_start],
                            end_line: lnum,
                            end_col: pos as u32,
                            end_byte_col: char_to_byte[pos],
                            line: line.to_string(),
                        });
                        continue;
                    }

                    // Explicit line continuation backslash.
                    if c == '\\' {
                        let next = chars.get(pos + 1).copied();
                        if matches!(next, Some('\n') | Some('\r')) || next.is_none() {
                            self.continued = true;
                            break;
                        }
                        if self.exact {
                            self.deferred_error = Some(LexError::TokenError {
                                msg: "unexpected character after line continuation character"
                                    .to_string(),
                                line: lnum,
                                col: line_char_len,
                            });
                            break;
                        }
                        // Stray backslash not followed by newline -> ERRORTOKEN.
                        self.has_logical_token = true;
                        out.push(LexToken {
                            tok_type: TOK_ERRORTOKEN,
                            exact_type: TOK_ERRORTOKEN,
                            string: "\\".to_string(),
                            start_line: lnum,
                            start_col: pos as u32,
                            start_byte_col: char_to_byte[pos],
                            end_line: lnum,
                            end_col: (pos + 1) as u32,
                            end_byte_col: char_to_byte[pos + 1],
                            line: line.to_string(),
                        });
                        pos += 1;
                        continue;
                    }

                    // Number literal (digit or `.digit`).
                    if c.is_ascii_digit()
                        || (c == '.' && chars.get(pos + 1).is_some_and(char::is_ascii_digit))
                    {
                        let start = pos;
                        if self.exact
                            && let Some((msg, offset)) = number_error(&chars, pos)
                        {
                            self.deferred_error = Some(LexError::TokenError {
                                msg,
                                line: lnum,
                                col: offset as u32,
                            });
                            break;
                        }
                        pos = scan_number(&chars, pos);
                        let text: String = chars[start..pos].iter().collect();
                        self.has_logical_token = true;
                        out.push(LexToken {
                            tok_type: TOK_NUMBER,
                            exact_type: TOK_NUMBER,
                            string: text,
                            start_line: lnum,
                            start_col: start as u32,
                            start_byte_col: char_to_byte[start],
                            end_line: lnum,
                            end_col: pos as u32,
                            end_byte_col: char_to_byte[pos],
                            line: line.to_string(),
                        });
                        continue;
                    }

                    // String or f-string (with optional prefix).
                    if let Some((prefix_len, is_fstring, is_raw)) = match_string_prefix(&chars, pos)
                    {
                        let quote_pos = pos + prefix_len;
                        let q = chars[quote_pos];
                        let is_triple = chars.get(quote_pos + 1) == Some(&q)
                            && chars.get(quote_pos + 2) == Some(&q);
                        let quote_len = if is_triple { 3 } else { 1 };
                        let quote_str: String =
                            chars[quote_pos..quote_pos + quote_len].iter().collect();

                        self.has_logical_token = true;
                        if is_fstring {
                            let start_end = quote_pos + quote_len;
                            let start_str: String = chars[pos..start_end].iter().collect();
                            out.push(LexToken {
                                tok_type: TOK_FSTRING_START,
                                exact_type: TOK_FSTRING_START,
                                string: start_str,
                                start_line: lnum,
                                start_col: pos as u32,
                                start_byte_col: char_to_byte[pos],
                                end_line: lnum,
                                end_col: start_end as u32,
                                end_byte_col: char_to_byte[start_end],
                                line: line.to_string(),
                            });
                            if self.paren_start.is_none() {
                                self.paren_start = Some((lnum, pos as u32));
                            }
                            self.modes.push(LexMode::FStringText {
                                quote: quote_str,
                                is_triple,
                                is_raw,
                            });
                            pos = start_end;
                            continue;
                        } else {
                            let str_start = pos;
                            pos = self.scan_regular_string(
                                str_start,
                                quote_pos + quote_len,
                                &quote_str,
                                is_triple,
                                &chars,
                                &char_to_byte,
                                line,
                                &mut out,
                            );
                            if self.multiline.is_some() {
                                return Ok(out);
                            }
                            continue;
                        }
                    }

                    // Identifier / keyword.  CPython's C tokenizer treats every
                    // non-ASCII character as an identifier character (the
                    // parser validates the name later), unlike the `ast` path.
                    let exact = self.exact;
                    if is_ident_start(c) || (exact && !c.is_ascii()) {
                        let start = pos;
                        pos += 1;
                        while pos < chars.len()
                            && (is_ident_continue(chars[pos]) || (exact && !chars[pos].is_ascii()))
                        {
                            pos += 1;
                        }
                        let text: String = chars[start..pos].iter().collect();
                        self.has_logical_token = true;
                        out.push(LexToken {
                            tok_type: TOK_NAME,
                            exact_type: TOK_NAME,
                            string: text,
                            start_line: lnum,
                            start_col: start as u32,
                            start_byte_col: char_to_byte[start],
                            end_line: lnum,
                            end_col: pos as u32,
                            end_byte_col: char_to_byte[pos],
                            line: line.to_string(),
                        });
                        continue;
                    }

                    // Operators and delimiters.  The C tokenizer also accepts
                    // `<>` (`NOTEQUAL`, for `barry_as_FLUFL`).
                    let operator = if self.exact && c == '<' && chars.get(pos + 1) == Some(&'>') {
                        Some((2, TOK_NOTEQUAL))
                    } else {
                        match_operator(&chars, pos)
                    };
                    if let Some((op_len, exact_type)) = operator {
                        let start = pos;
                        let end = pos + op_len;
                        let text: String = chars[start..end].iter().collect();
                        self.has_logical_token = true;

                        // Update paren/brace nesting and f-string expression transitions.
                        let mut push_format_spec: Option<(bool, bool)> = None;
                        let mut pop_fstring_expr = false;

                        match exact_type {
                            TOK_LPAR | TOK_LSQB => {
                                if self.total_paren_depth() == 0 {
                                    self.paren_start = Some((lnum, start as u32));
                                }
                                if let Some(LexMode::Code { paren_depth, .. }) =
                                    self.modes.last_mut()
                                {
                                    *paren_depth += 1;
                                }
                            }
                            TOK_RPAR | TOK_RSQB => {
                                if let Some(LexMode::Code { paren_depth, .. }) =
                                    self.modes.last_mut()
                                {
                                    *paren_depth = paren_depth.saturating_sub(1);
                                }
                                if self.total_paren_depth() == 0 {
                                    self.paren_start = None;
                                }
                            }
                            TOK_LBRACE => {
                                if self.total_paren_depth() == 0 {
                                    self.paren_start = Some((lnum, start as u32));
                                }
                                if let Some(LexMode::Code { brace_depth, .. }) =
                                    self.modes.last_mut()
                                {
                                    *brace_depth += 1;
                                }
                            }
                            TOK_RBRACE => {
                                if let Some(LexMode::Code {
                                    is_fstring_expr,
                                    brace_depth,
                                    ..
                                }) = self.modes.last_mut()
                                {
                                    if *brace_depth > 0 {
                                        *brace_depth -= 1;
                                    } else if *is_fstring_expr {
                                        pop_fstring_expr = true;
                                    }
                                }
                            }
                            TOK_COLON => {
                                // Inside an f-string expression at depth 0, `:` opens the format spec!
                                if let Some(LexMode::Code {
                                    is_fstring_expr: true,
                                    paren_depth: 0,
                                    brace_depth: 0,
                                }) = self.modes.last()
                                {
                                    // Inherit triple/raw from enclosing FStringText or FStringFormatSpec.
                                    let (is_triple, is_raw) = self
                                        .modes
                                        .iter()
                                        .rev()
                                        .skip(1)
                                        .find_map(|m| match m {
                                            LexMode::FStringText {
                                                is_triple, is_raw, ..
                                            }
                                            | LexMode::FStringFormatSpec {
                                                is_triple,
                                                is_raw,
                                                ..
                                            } => Some((*is_triple, *is_raw)),
                                            _ => None,
                                        })
                                        .unwrap_or((false, false));
                                    push_format_spec = Some((is_triple, is_raw));
                                }
                            }
                            _ => {}
                        }

                        out.push(LexToken {
                            tok_type: TOK_OP,
                            exact_type,
                            string: text,
                            start_line: lnum,
                            start_col: start as u32,
                            start_byte_col: char_to_byte[start],
                            end_line: lnum,
                            end_col: end as u32,
                            end_byte_col: char_to_byte[end],
                            line: line.to_string(),
                        });

                        if pop_fstring_expr {
                            self.modes.pop();
                        }
                        if let Some((is_triple, is_raw)) = push_format_spec {
                            self.modes.push(LexMode::FStringFormatSpec {
                                is_triple,
                                is_raw,
                                just_emitted_middle: false,
                            });
                        }
                        if self.total_paren_depth() == 0 {
                            self.paren_start = None;
                        }

                        pos = end;
                        continue;
                    }

                    // Any unrecognized character -> ERRORTOKEN.
                    self.has_logical_token = true;
                    if self.exact {
                        if matches!(c, '$' | '?' | '`') {
                            // The C tokenizer hands these out as plain operators.
                            out.push(LexToken {
                                tok_type: TOK_OP,
                                exact_type: TOK_OP,
                                string: c.to_string(),
                                start_line: lnum,
                                start_col: pos as u32,
                                start_byte_col: char_to_byte[pos],
                                end_line: lnum,
                                end_col: (pos + 1) as u32,
                                end_byte_col: char_to_byte[pos + 1],
                                line: line.to_string(),
                            });
                            pos += 1;
                            continue;
                        }
                        if c.is_ascii_control() {
                            self.deferred_error = Some(LexError::TokenError {
                                msg: format!("invalid non-printable character U+{:04X}", c as u32),
                                line: lnum,
                                col: pos as u32 + 1,
                            });
                            break;
                        }
                    }
                    out.push(LexToken {
                        tok_type: TOK_ERRORTOKEN,
                        exact_type: TOK_ERRORTOKEN,
                        string: c.to_string(),
                        start_line: lnum,
                        start_col: pos as u32,
                        start_byte_col: char_to_byte[pos],
                        end_line: lnum,
                        end_col: (pos + 1) as u32,
                        end_byte_col: char_to_byte[pos + 1],
                        line: line.to_string(),
                    });
                    pos += 1;
                }
            }
        }

        // If the line had no trailing newline and we just finished a comment at the
        // end of a logical line or normal tokens, nothing extra is emitted until finish().
        // Wait: if a non-empty line without `\n` ended in a comment after tokens,
        // `finish()` emits NEWLINE; if it was blank/comment-only, we already emitted NL above.
        Ok(out)
    }

    /// Signal EOF and emit remaining implicit `NEWLINE`, `DEDENT`, and `ENDMARKER`
    /// tokens (or return `TokenError` for unclosed strings/parentheses).
    pub(crate) fn finish(&mut self) -> Result<Vec<LexToken>, LexError> {
        let mut out = Vec::new();

        if let Some(accum) = self.multiline.take() {
            if self.exact {
                // An f-string's middle starts after its prefix and quote; the
                // error is reported at the whole f-string's start.
                let (line, col) = match self.paren_start {
                    Some(origin) if accum.is_fstring_middle => origin,
                    _ => (accum.start_line, accum.start_col),
                };
                let msg = match (accum.is_fstring_middle, accum.is_triple) {
                    (false, true) => "EOF in multi-line string".to_string(),
                    (false, false) => {
                        format!(
                            "unterminated string literal (detected at line {})",
                            self.lnum
                        )
                    }
                    (true, true) => format!(
                        "unterminated triple-quoted f-string literal (detected at line {})",
                        self.lnum
                    ),
                    (true, false) => {
                        format!(
                            "unterminated f-string literal (detected at line {})",
                            self.lnum
                        )
                    }
                };
                return Err(LexError::TokenError {
                    msg,
                    line,
                    col: col + 1,
                });
            }
            return Err(LexError::TokenError {
                msg: "EOF in multi-line string".to_string(),
                line: accum.start_line,
                col: accum.start_col,
            });
        }

        if self.exact && (self.in_implicit_continuation() || self.continued) {
            // The offset is the length of the line when a backslash
            // continuation runs into EOF after some code, else 0.
            let line_len = self.last_line_char_len + u32::from(!self.last_ended_with_nl);
            let col =
                if self.continued && (self.has_logical_token || self.in_implicit_continuation()) {
                    line_len
                } else {
                    0
                };
            return Err(LexError::TokenError {
                msg: "unexpected EOF in multi-line statement".to_string(),
                line: self.lnum.max(1),
                col,
            });
        }

        if self.in_implicit_continuation() {
            let (err_line, err_col) = self.paren_start.unwrap_or((self.lnum.max(1), 0));
            return Err(LexError::TokenError {
                msg: "EOF in multi-line statement".to_string(),
                line: err_line,
                col: err_col,
            });
        }

        // If the last line had tokens and didn't end with a newline, emit an implicit NEWLINE.
        if self.has_logical_token && !self.last_ended_with_nl {
            out.push(LexToken {
                tok_type: TOK_NEWLINE,
                exact_type: TOK_NEWLINE,
                string: String::new(),
                start_line: self.lnum,
                start_col: self.last_line_char_len,
                start_byte_col: self.last_line_byte_len,
                end_line: self.lnum,
                end_col: self.last_line_char_len + 1,
                end_byte_col: self.last_line_byte_len + 1,
                line: self.last_line_text.clone(),
            });
            self.has_logical_token = false;
        }

        let end_lnum = if self.fed_any_line { self.lnum + 1 } else { 1 };

        while self.indents.len() > 1 {
            self.indents.pop();
            self.indent_styles.pop();
            self.alt_indents.pop();
            out.push(LexToken {
                tok_type: TOK_DEDENT,
                exact_type: TOK_DEDENT,
                string: String::new(),
                start_line: end_lnum,
                start_col: 0,
                start_byte_col: 0,
                end_line: end_lnum,
                end_col: 0,
                end_byte_col: 0,
                line: String::new(),
            });
        }

        out.push(LexToken {
            tok_type: TOK_ENDMARKER,
            exact_type: TOK_ENDMARKER,
            string: String::new(),
            start_line: end_lnum,
            start_col: 0,
            start_byte_col: 0,
            end_line: end_lnum,
            end_col: 0,
            end_byte_col: 0,
            line: String::new(),
        });

        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_regular_string(
        &mut self,
        str_start: usize,
        mut pos: usize,
        quote: &str,
        is_triple: bool,
        chars: &[char],
        char_to_byte: &[u32],
        line: &str,
        out: &mut Vec<LexToken>,
    ) -> usize {
        let q_chars: Vec<char> = quote.chars().collect();
        let q_len = q_chars.len();
        let lnum = self.lnum;

        while pos < chars.len() {
            let c = chars[pos];
            if c == '\\' {
                if pos + 1 < chars.len() {
                    if chars[pos + 1] == '\r' && chars.get(pos + 2) == Some(&'\n') {
                        pos += 3;
                    } else {
                        pos += 2;
                    }
                    continue;
                }
                // Trailing backslash at end of line (will continue on next line).
                break;
            }
            if !is_triple && (c == '\n' || c == '\r') {
                if self.exact {
                    self.deferred_error = Some(unterminated_string(lnum, lnum, str_start));
                    return pos;
                }
                // Unterminated single-line string: emit ERRORTOKEN for the opening char
                // (the `ast` path turns it into a syntax error).
                out.push(LexToken {
                    tok_type: TOK_ERRORTOKEN,
                    exact_type: TOK_ERRORTOKEN,
                    string: chars[str_start..pos].iter().collect(),
                    start_line: lnum,
                    start_col: str_start as u32,
                    start_byte_col: char_to_byte[str_start],
                    end_line: lnum,
                    end_col: pos as u32,
                    end_byte_col: char_to_byte[pos],
                    line: line.to_string(),
                });
                return pos;
            }
            if pos + q_len <= chars.len() && chars[pos..pos + q_len] == q_chars[..] {
                let end = pos + q_len;
                let text: String = chars[str_start..end].iter().collect();
                out.push(LexToken {
                    tok_type: TOK_STRING,
                    exact_type: TOK_STRING,
                    string: text,
                    start_line: lnum,
                    start_col: str_start as u32,
                    start_byte_col: char_to_byte[str_start],
                    end_line: lnum,
                    end_col: end as u32,
                    end_byte_col: char_to_byte[end],
                    line: line.to_string(),
                });
                return end;
            }
            pos += 1;
        }

        // Reached end of line without closing quote.
        if is_triple
            || chars.ends_with(&['\\', '\n'])
            || chars.ends_with(&['\\', '\r', '\n'])
            || chars.ends_with(&['\\'])
        {
            self.multiline = Some(MultilineAccum {
                is_fstring_middle: false,
                quote: quote.to_string(),
                is_triple,
                start_line: lnum,
                start_col: str_start as u32,
                start_byte_col: char_to_byte[str_start],
                content: chars[str_start..].iter().collect(),
                lines_accum: line.to_string(),
            });
            chars.len()
        } else if self.exact {
            self.deferred_error = Some(unterminated_string(lnum, lnum, str_start));
            chars.len()
        } else {
            out.push(LexToken {
                tok_type: TOK_ERRORTOKEN,
                exact_type: TOK_ERRORTOKEN,
                string: chars[str_start..chars.len()].iter().collect(),
                start_line: lnum,
                start_col: str_start as u32,
                start_byte_col: char_to_byte[str_start],
                end_line: lnum,
                end_col: chars.len() as u32,
                end_byte_col: char_to_byte[chars.len()],
                line: line.to_string(),
            });
            chars.len()
        }
    }

    fn continue_regular_string(
        &mut self,
        mut accum: MultilineAccum,
        chars: &[char],
        char_to_byte: &[u32],
        line: &str,
        out: &mut Vec<LexToken>,
    ) -> Option<usize> {
        let q_chars: Vec<char> = accum.quote.chars().collect();
        let q_len = q_chars.len();
        let lnum = self.lnum;
        let mut pos = 0usize;

        while pos < chars.len() {
            let c = chars[pos];
            if c == '\\' {
                if pos + 1 < chars.len() {
                    if chars[pos + 1] == '\r' && chars.get(pos + 2) == Some(&'\n') {
                        pos += 3;
                    } else {
                        pos += 2;
                    }
                    continue;
                }
                break;
            }
            if !accum.is_triple && (c == '\n' || c == '\r') {
                if self.exact {
                    self.deferred_error = Some(unterminated_string(
                        accum.start_line,
                        lnum,
                        accum.start_col as usize,
                    ));
                    return Some(pos);
                }
                // Unterminated continued single-line string.
                accum.content.extend(&chars[..pos]);
                accum.lines_accum.push_str(line);
                out.push(LexToken {
                    tok_type: TOK_ERRORTOKEN,
                    exact_type: TOK_ERRORTOKEN,
                    string: accum.content,
                    start_line: accum.start_line,
                    start_col: accum.start_col,
                    start_byte_col: accum.start_byte_col,
                    end_line: lnum,
                    end_col: pos as u32,
                    end_byte_col: char_to_byte[pos],
                    line: accum.lines_accum,
                });
                return Some(pos);
            }
            if pos + q_len <= chars.len() && chars[pos..pos + q_len] == q_chars[..] {
                let end = pos + q_len;
                accum.content.extend(&chars[..end]);
                accum.lines_accum.push_str(line);
                out.push(LexToken {
                    tok_type: TOK_STRING,
                    exact_type: TOK_STRING,
                    string: accum.content,
                    start_line: accum.start_line,
                    start_col: accum.start_col,
                    start_byte_col: accum.start_byte_col,
                    end_line: lnum,
                    end_col: end as u32,
                    end_byte_col: char_to_byte[end],
                    line: accum.lines_accum,
                });
                return Some(end);
            }
            pos += 1;
        }

        accum.content.push_str(line);
        accum.lines_accum.push_str(line);
        self.multiline = Some(accum);
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_fstring_text(
        &mut self,
        start: usize,
        chars: &[char],
        char_to_byte: &[u32],
        line: &str,
        quote: &str,
        is_triple: bool,
        is_raw: bool,
        out: &mut Vec<LexToken>,
    ) -> Result<usize, LexError> {
        let q_chars: Vec<char> = quote.chars().collect();
        let q_len = q_chars.len();
        let lnum = self.lnum;
        let mut pos = start;

        // Check immediate closing quote.
        if pos + q_len <= chars.len() && chars[pos..pos + q_len] == q_chars[..] {
            let end = pos + q_len;
            out.push(LexToken {
                tok_type: TOK_FSTRING_END,
                exact_type: TOK_FSTRING_END,
                string: quote.to_string(),
                start_line: lnum,
                start_col: pos as u32,
                start_byte_col: char_to_byte[pos],
                end_line: lnum,
                end_col: end as u32,
                end_byte_col: char_to_byte[end],
                line: line.to_string(),
            });
            self.modes.pop();
            if self.total_paren_depth() == 0 {
                self.paren_start = None;
            }
            return Ok(end);
        }

        // Check immediate `{` (not `{{`).
        if chars[pos] == '{' && chars.get(pos + 1) != Some(&'{') {
            out.push(LexToken {
                tok_type: TOK_OP,
                exact_type: TOK_LBRACE,
                string: "{".to_string(),
                start_line: lnum,
                start_col: pos as u32,
                start_byte_col: char_to_byte[pos],
                end_line: lnum,
                end_col: (pos + 1) as u32,
                end_byte_col: char_to_byte[pos + 1],
                line: line.to_string(),
            });
            self.modes.push(LexMode::Code {
                is_fstring_expr: true,
                paren_depth: 0,
                brace_depth: 0,
            });
            return Ok(pos + 1);
        }

        // Scan FSTRING_MIDDLE chunk.
        let mut buf = String::new();
        while pos < chars.len() {
            if pos + q_len <= chars.len() && chars[pos..pos + q_len] == q_chars[..] {
                break;
            }
            let c = chars[pos];
            if c == '{' {
                if chars.get(pos + 1) == Some(&'{') {
                    // CPython 3.12: emit FSTRING_MIDDLE ending with single `{`, skip the second `{`.
                    buf.push('{');
                    let end_pos = pos + 1;
                    out.push(LexToken {
                        tok_type: TOK_FSTRING_MIDDLE,
                        exact_type: TOK_FSTRING_MIDDLE,
                        string: buf,
                        start_line: lnum,
                        start_col: start as u32,
                        start_byte_col: char_to_byte[start],
                        end_line: lnum,
                        end_col: end_pos as u32,
                        end_byte_col: char_to_byte[end_pos],
                        line: line.to_string(),
                    });
                    return Ok(pos + 2);
                }
                break;
            }
            if c == '}' {
                if chars.get(pos + 1) == Some(&'}') {
                    buf.push('}');
                    let end_pos = pos + 1;
                    out.push(LexToken {
                        tok_type: TOK_FSTRING_MIDDLE,
                        exact_type: TOK_FSTRING_MIDDLE,
                        string: buf,
                        start_line: lnum,
                        start_col: start as u32,
                        start_byte_col: char_to_byte[start],
                        end_line: lnum,
                        end_col: end_pos as u32,
                        end_byte_col: char_to_byte[end_pos],
                        line: line.to_string(),
                    });
                    return Ok(pos + 2);
                }
                // Lone `}` inside f-string text: flush middle then emit OP `}` so parser catches it.
                if !buf.is_empty() {
                    out.push(LexToken {
                        tok_type: TOK_FSTRING_MIDDLE,
                        exact_type: TOK_FSTRING_MIDDLE,
                        string: buf,
                        start_line: lnum,
                        start_col: start as u32,
                        start_byte_col: char_to_byte[start],
                        end_line: lnum,
                        end_col: pos as u32,
                        end_byte_col: char_to_byte[pos],
                        line: line.to_string(),
                    });
                }
                out.push(LexToken {
                    tok_type: TOK_OP,
                    exact_type: TOK_RBRACE,
                    string: "}".to_string(),
                    start_line: lnum,
                    start_col: pos as u32,
                    start_byte_col: char_to_byte[pos],
                    end_line: lnum,
                    end_col: (pos + 1) as u32,
                    end_byte_col: char_to_byte[pos + 1],
                    line: line.to_string(),
                });
                return Ok(pos + 1);
            }
            if c == '\\' {
                if !is_raw
                    && pos + 1 < chars.len()
                    && chars[pos + 1] == 'N'
                    && chars.get(pos + 2) == Some(&'{')
                {
                    // `\N{...}` unicode escape: consume up to closing `}` so `{` isn't treated as expression!
                    buf.push(c);
                    pos += 1;
                    while pos < chars.len() {
                        let nc = chars[pos];
                        buf.push(nc);
                        pos += 1;
                        if nc == '}' {
                            break;
                        }
                    }
                    continue;
                }
                if pos + 1 < chars.len() {
                    if chars[pos + 1] == '\r' && chars.get(pos + 2) == Some(&'\n') {
                        buf.push('\\');
                        buf.push('\r');
                        buf.push('\n');
                        // Continued onto next line!
                        self.multiline = Some(MultilineAccum {
                            is_fstring_middle: true,
                            quote: quote.to_string(),
                            is_triple,
                            start_line: lnum,
                            start_col: start as u32,
                            start_byte_col: char_to_byte[start],
                            content: buf,
                            lines_accum: line.to_string(),
                        });
                        return Ok(chars.len());
                    }
                    if chars[pos + 1] == '\n' {
                        buf.push('\\');
                        buf.push('\n');
                        self.multiline = Some(MultilineAccum {
                            is_fstring_middle: true,
                            quote: quote.to_string(),
                            is_triple,
                            start_line: lnum,
                            start_col: start as u32,
                            start_byte_col: char_to_byte[start],
                            content: buf,
                            lines_accum: line.to_string(),
                        });
                        return Ok(chars.len());
                    }
                    // For raw f-strings, `\"` still escapes the quote char from ending the string
                    // (except `\\"`), same as regular raw strings.
                    if chars[pos + 1] == '{' || chars[pos + 1] == '}' {
                        // In Python, `\{` does NOT escape `{`; `{` still begins an expression!
                        buf.push('\\');
                        pos += 1;
                        continue;
                    }
                    buf.push('\\');
                    buf.push(chars[pos + 1]);
                    pos += 2;
                    continue;
                }
                buf.push('\\');
                pos += 1;
                continue;
            }
            if !is_triple && (c == '\n' || c == '\r') {
                return Err(LexError::TokenError {
                    msg: "unterminated f-string literal".to_string(),
                    line: lnum,
                    col: start as u32,
                });
            }
            buf.push(c);
            pos += 1;
        }

        if pos >= chars.len() {
            if is_triple {
                self.multiline = Some(MultilineAccum {
                    is_fstring_middle: true,
                    quote: quote.to_string(),
                    is_triple,
                    start_line: lnum,
                    start_col: start as u32,
                    start_byte_col: char_to_byte[start],
                    content: buf,
                    lines_accum: line.to_string(),
                });
                return Ok(chars.len());
            }
            return Err(LexError::TokenError {
                msg: "unterminated f-string literal".to_string(),
                line: lnum,
                col: start as u32,
            });
        }

        if !buf.is_empty() {
            if let Some(LexMode::FStringFormatSpec {
                just_emitted_middle,
                ..
            }) = self.modes.last_mut()
            {
                *just_emitted_middle = true;
            }
            out.push(LexToken {
                tok_type: TOK_FSTRING_MIDDLE,
                exact_type: TOK_FSTRING_MIDDLE,
                string: buf,
                start_line: lnum,
                start_col: start as u32,
                start_byte_col: char_to_byte[start],
                end_line: lnum,
                end_col: pos as u32,
                end_byte_col: char_to_byte[pos],
                line: line.to_string(),
            });
        }
        Ok(pos)
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_fstring_format_spec(
        &mut self,
        start: usize,
        chars: &[char],
        char_to_byte: &[u32],
        line: &str,
        is_triple: bool,
        is_raw: bool,
        just_emitted_middle: bool,
        out: &mut Vec<LexToken>,
    ) -> Result<usize, LexError> {
        let lnum = self.lnum;
        let mut pos = start;

        // Immediate `}` closes format spec AND enclosing f-string expr!
        // CPython 3.12 emits an empty FSTRING_MIDDLE before `}` when no
        // non-empty literal chunk immediately preceded it.
        if chars[pos] == '}' {
            if !just_emitted_middle {
                out.push(LexToken {
                    tok_type: TOK_FSTRING_MIDDLE,
                    exact_type: TOK_FSTRING_MIDDLE,
                    string: String::new(),
                    start_line: lnum,
                    start_col: pos as u32,
                    start_byte_col: char_to_byte[pos],
                    end_line: lnum,
                    end_col: pos as u32,
                    end_byte_col: char_to_byte[pos],
                    line: line.to_string(),
                });
            }
            out.push(LexToken {
                tok_type: TOK_OP,
                exact_type: TOK_RBRACE,
                string: "}".to_string(),
                start_line: lnum,
                start_col: pos as u32,
                start_byte_col: char_to_byte[pos],
                end_line: lnum,
                end_col: (pos + 1) as u32,
                end_byte_col: char_to_byte[pos + 1],
                line: line.to_string(),
            });
            self.modes.pop(); // pop FStringFormatSpec
            self.modes.pop(); // pop Code(is_fstring_expr)
            if self.total_paren_depth() == 0 {
                self.paren_start = None;
            }
            return Ok(pos + 1);
        }

        // Immediate `{` opens a nested expression inside format spec!
        if chars[pos] == '{' {
            if let Some(LexMode::FStringFormatSpec {
                just_emitted_middle,
                ..
            }) = self.modes.last_mut()
            {
                *just_emitted_middle = false;
            }
            out.push(LexToken {
                tok_type: TOK_OP,
                exact_type: TOK_LBRACE,
                string: "{".to_string(),
                start_line: lnum,
                start_col: pos as u32,
                start_byte_col: char_to_byte[pos],
                end_line: lnum,
                end_col: (pos + 1) as u32,
                end_byte_col: char_to_byte[pos + 1],
                line: line.to_string(),
            });
            self.modes.push(LexMode::Code {
                is_fstring_expr: true,
                paren_depth: 0,
                brace_depth: 0,
            });
            return Ok(pos + 1);
        }

        let mut buf = String::new();
        while pos < chars.len() {
            let c = chars[pos];
            if c == '{' || c == '}' {
                break;
            }
            if c == '\\' && pos + 1 < chars.len() {
                if !is_raw && chars[pos + 1] == 'N' && chars.get(pos + 2) == Some(&'{') {
                    buf.push(c);
                    pos += 1;
                    while pos < chars.len() {
                        let nc = chars[pos];
                        buf.push(nc);
                        pos += 1;
                        if nc == '}' {
                            break;
                        }
                    }
                    continue;
                }
                if chars[pos + 1] == '{' || chars[pos + 1] == '}' {
                    buf.push('\\');
                    pos += 1;
                    continue;
                }
                buf.push('\\');
                buf.push(chars[pos + 1]);
                pos += 2;
                continue;
            }
            if !is_triple && (c == '\n' || c == '\r') {
                return Err(LexError::TokenError {
                    msg: "unterminated f-string literal".to_string(),
                    line: lnum,
                    col: start as u32,
                });
            }
            buf.push(c);
            pos += 1;
        }

        if pos >= chars.len() {
            if is_triple {
                self.multiline = Some(MultilineAccum {
                    is_fstring_middle: true,
                    quote: String::new(), // empty quote signals format spec continuation
                    is_triple,
                    start_line: lnum,
                    start_col: start as u32,
                    start_byte_col: char_to_byte[start],
                    content: buf,
                    lines_accum: line.to_string(),
                });
                return Ok(chars.len());
            }
            return Err(LexError::TokenError {
                msg: "unterminated f-string literal".to_string(),
                line: lnum,
                col: start as u32,
            });
        }

        if !buf.is_empty() {
            if let Some(LexMode::FStringFormatSpec {
                just_emitted_middle,
                ..
            }) = self.modes.last_mut()
            {
                *just_emitted_middle = true;
            }
            out.push(LexToken {
                tok_type: TOK_FSTRING_MIDDLE,
                exact_type: TOK_FSTRING_MIDDLE,
                string: buf,
                start_line: lnum,
                start_col: start as u32,
                start_byte_col: char_to_byte[start],
                end_line: lnum,
                end_col: pos as u32,
                end_byte_col: char_to_byte[pos],
                line: line.to_string(),
            });
        }
        Ok(pos)
    }

    fn continue_fstring_middle(
        &mut self,
        mut accum: MultilineAccum,
        chars: &[char],
        char_to_byte: &[u32],
        line: &str,
        out: &mut Vec<LexToken>,
    ) -> Result<Option<usize>, LexError> {
        let is_format_spec = accum.quote.is_empty();
        let q_chars: Vec<char> = accum.quote.chars().collect();
        let q_len = q_chars.len();
        let lnum = self.lnum;
        let mut pos = 0usize;

        while pos < chars.len() {
            if !is_format_spec
                && pos + q_len <= chars.len()
                && chars[pos..pos + q_len] == q_chars[..]
            {
                break;
            }
            let c = chars[pos];
            if c == '{' {
                if !is_format_spec && chars.get(pos + 1) == Some(&'{') {
                    accum.content.push('{');
                    accum.lines_accum.push_str(line);
                    let end_pos = pos + 1;
                    out.push(LexToken {
                        tok_type: TOK_FSTRING_MIDDLE,
                        exact_type: TOK_FSTRING_MIDDLE,
                        string: accum.content,
                        start_line: accum.start_line,
                        start_col: accum.start_col,
                        start_byte_col: accum.start_byte_col,
                        end_line: lnum,
                        end_col: end_pos as u32,
                        end_byte_col: char_to_byte[end_pos],
                        line: accum.lines_accum,
                    });
                    return Ok(Some(pos + 2));
                }
                break;
            }
            if c == '}' {
                if !is_format_spec && chars.get(pos + 1) == Some(&'}') {
                    accum.content.push('}');
                    accum.lines_accum.push_str(line);
                    let end_pos = pos + 1;
                    out.push(LexToken {
                        tok_type: TOK_FSTRING_MIDDLE,
                        exact_type: TOK_FSTRING_MIDDLE,
                        string: accum.content,
                        start_line: accum.start_line,
                        start_col: accum.start_col,
                        start_byte_col: accum.start_byte_col,
                        end_line: lnum,
                        end_col: end_pos as u32,
                        end_byte_col: char_to_byte[end_pos],
                        line: accum.lines_accum,
                    });
                    return Ok(Some(pos + 2));
                }
                break;
            }
            if c == '\\' && pos + 1 < chars.len() {
                if chars[pos + 1] == '\r' && chars.get(pos + 2) == Some(&'\n') {
                    accum.content.push_str("\\\r\n");
                    accum.lines_accum.push_str(line);
                    self.multiline = Some(accum);
                    return Ok(None);
                }
                if chars[pos + 1] == '\n' {
                    accum.content.push_str("\\\n");
                    accum.lines_accum.push_str(line);
                    self.multiline = Some(accum);
                    return Ok(None);
                }
                if chars[pos + 1] == '{' || chars[pos + 1] == '}' {
                    accum.content.push('\\');
                    pos += 1;
                    continue;
                }
                accum.content.push('\\');
                accum.content.push(chars[pos + 1]);
                pos += 2;
                continue;
            }
            if !accum.is_triple && (c == '\n' || c == '\r') {
                return Err(LexError::TokenError {
                    msg: "unterminated f-string literal".to_string(),
                    line: accum.start_line,
                    col: accum.start_col,
                });
            }
            accum.content.push(c);
            pos += 1;
        }

        if pos >= chars.len() {
            if accum.is_triple {
                accum.lines_accum.push_str(line);
                self.multiline = Some(accum);
                return Ok(None);
            }
            return Err(LexError::TokenError {
                msg: "unterminated f-string literal".to_string(),
                line: accum.start_line,
                col: accum.start_col,
            });
        }

        accum.lines_accum.push_str(line);
        if !accum.content.is_empty() {
            out.push(LexToken {
                tok_type: TOK_FSTRING_MIDDLE,
                exact_type: TOK_FSTRING_MIDDLE,
                string: accum.content,
                start_line: accum.start_line,
                start_col: accum.start_col,
                start_byte_col: accum.start_byte_col,
                end_line: lnum,
                end_col: pos as u32,
                end_byte_col: char_to_byte[pos],
                line: accum.lines_accum,
            });
        }
        Ok(Some(pos))
    }
}

/// Tokenize an entire source string into a `Vec<LexToken>`, splitting on
/// `\r\n`, `\n`, and `\r` exactly like Python's universal newlines.
pub(crate) fn tokenize_source(src: &str) -> Result<Vec<LexToken>, LexError> {
    tokenize_source_impl(src, true)
}

pub(crate) fn tokenize_source_relaxed(src: &str) -> Result<Vec<LexToken>, LexError> {
    tokenize_source_impl(src, false)
}

fn tokenize_source_impl(src: &str, check_tabs: bool) -> Result<Vec<LexToken>, LexError> {
    let mut lexer = StatefulLexer::with_tab_check(check_tabs);
    let mut all = Vec::new();

    if !src.is_empty() {
        let bytes = src.as_bytes();
        let mut start = 0usize;
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i] == b'\n' {
                let line = &src[start..=i];
                all.extend(lexer.feed_line(line)?);
                i += 1;
                start = i;
            } else if bytes[i] == b'\r' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    let line = &src[start..=i + 1];
                    all.extend(lexer.feed_line(line)?);
                    i += 2;
                    start = i;
                } else {
                    let line = &src[start..=i];
                    all.extend(lexer.feed_line(line)?);
                    i += 1;
                    start = i;
                }
            } else {
                i += 1;
            }
        }
        if start < bytes.len() {
            let line = &src[start..];
            all.extend(lexer.feed_line(line)?);
        }
    }

    all.extend(lexer.finish()?);
    Ok(all)
}

/// The exact-mode error for a single-quoted string that is not closed on its
/// last line: reported at the string's start (1-based offset), naming the line
/// the tokenizer was on when it noticed.
fn unterminated_string(start_line: u32, detected_line: u32, start_col: usize) -> LexError {
    LexError::TokenError {
        msg: format!("unterminated string literal (detected at line {detected_line})"),
        line: start_line,
        col: start_col as u32 + 1,
    }
}

/// The `SyntaxError` CPython 3.12.3's C tokenizer raises for a malformed
/// numeric literal starting at `chars[start]`, as `(message, offset)`, or
/// `None` when the literal is accepted (the `tokenize` module hands out
/// `1a`, `09` and the like as separate tokens).  `offset` is the index of the
/// first character the tokenizer had not consumed yet (`tok->cur`), except
/// for the "invalid digit" errors, which point just past the offending digit.
fn number_error(chars: &[char], start: usize) -> Option<(String, usize)> {
    let at = |i: usize| chars.get(i).copied();
    let digit = |i: usize| at(i).is_some_and(|c| c.is_ascii_digit());
    let decimal = |at_index: usize| ("invalid decimal literal".to_string(), at_index);

    // `tok_decimal_tail`: digits with single underscores between them.
    let decimal_tail = |mut cur: usize| -> std::result::Result<usize, (String, usize)> {
        loop {
            while digit(cur) {
                cur += 1;
            }
            if at(cur) != Some('_') {
                return Ok(cur);
            }
            cur += 1;
            if !digit(cur) {
                return Err(decimal(cur));
            }
        }
    };

    // Everything after the integer part: `.fraction`, `e` exponent.
    let after_integer =
        |mut cur: usize, mut in_fraction: bool| -> std::result::Result<(), (String, usize)> {
            if !in_fraction && at(cur) == Some('.') {
                cur += 1;
                in_fraction = true;
            }
            if in_fraction && digit(cur) {
                cur = decimal_tail(cur)?;
            }
            if matches!(at(cur), Some('e' | 'E')) {
                cur += 1;
                if matches!(at(cur), Some('+' | '-')) {
                    cur += 1;
                    if !digit(cur) {
                        return Err(decimal(cur));
                    }
                } else if !digit(cur) {
                    // Not an exponent: the number ends before the `e`.
                    return Ok(());
                }
                decimal_tail(cur)?;
            }
            Ok(())
        };

    // `0x` / `0o` / `0b` digit runs; `digit_ok` accepts the radix's digits.
    let radix_run = |mut cur: usize,
                     digit_ok: &dyn Fn(char) -> bool,
                     kind: &str|
     -> std::result::Result<(), (String, usize)> {
        loop {
            if at(cur) == Some('_') {
                cur += 1;
            }
            match at(cur) {
                Some(c) if digit_ok(c) => {}
                Some(c) if kind != "hexadecimal" && c.is_ascii_digit() => {
                    return Err((format!("invalid digit '{c}' in {kind} literal"), cur + 1));
                }
                _ => return Err((format!("invalid {kind} literal"), cur)),
            }
            while at(cur).is_some_and(digit_ok) {
                cur += 1;
            }
            if at(cur) != Some('_') {
                break;
            }
        }
        if kind != "hexadecimal"
            && let Some(c) = at(cur)
            && c.is_ascii_digit()
        {
            return Err((format!("invalid digit '{c}' in {kind} literal"), cur + 1));
        }
        Ok(())
    };

    let result = if chars[start] == '.' {
        // `.5`: a fraction.
        after_integer(start + 1, true)
    } else if chars[start] == '0' {
        match at(start + 1) {
            Some('x' | 'X') => radix_run(start + 2, &|c| c.is_ascii_hexdigit(), "hexadecimal"),
            Some('o' | 'O') => radix_run(start + 2, &|c| ('0'..='7').contains(&c), "octal"),
            Some('b' | 'B') => radix_run(start + 2, &|c| matches!(c, '0' | '1'), "binary"),
            _ => {
                // Zeros (with underscores), then maybe more decimal digits.
                let mut cur = start + 1;
                let mut failed = None;
                loop {
                    if at(cur) == Some('_') {
                        cur += 1;
                        if !digit(cur) {
                            failed = Some(decimal(cur));
                            break;
                        }
                    }
                    if at(cur) != Some('0') {
                        break;
                    }
                    cur += 1;
                }
                match failed {
                    Some(err) => Err(err),
                    None => {
                        let tail = if digit(cur) {
                            decimal_tail(cur)
                        } else {
                            Ok(cur)
                        };
                        tail.and_then(|cur| after_integer(cur, false))
                    }
                }
            }
        }
    } else {
        decimal_tail(start).and_then(|cur| after_integer(cur, false))
    };
    result.err()
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic() || (!c.is_ascii() && c.is_alphabetic())
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric() || (!c.is_ascii() && c.is_alphanumeric())
}

/// Check if `chars[pos..]` starts a string or f-string literal.
/// Returns `Some((prefix_len, is_fstring, is_raw))`.
fn match_string_prefix(chars: &[char], pos: usize) -> Option<(usize, bool, bool)> {
    let c0 = *chars.get(pos)?;
    if c0 == '\'' || c0 == '"' {
        return Some((0, false, false));
    }
    let c1 = *chars.get(pos + 1)?;
    let l0 = c0.to_ascii_lowercase();
    if c1 == '\'' || c1 == '"' {
        return match l0 {
            'r' => Some((1, false, true)),
            'b' | 'u' => Some((1, false, false)),
            'f' => Some((1, true, false)),
            _ => None,
        };
    }
    let c2 = *chars.get(pos + 2)?;
    if c2 == '\'' || c2 == '"' {
        let l1 = c1.to_ascii_lowercase();
        return match (l0, l1) {
            ('r', 'b') | ('b', 'r') => Some((2, false, true)),
            ('r', 'f') | ('f', 'r') => Some((2, true, true)),
            _ => None,
        };
    }
    None
}

/// Scan a Python 3.12 numeric literal starting at `pos`.
fn scan_number(chars: &[char], start: usize) -> usize {
    let mut pos = start;
    if chars[pos] == '0' && pos + 1 < chars.len() {
        match chars[pos + 1] {
            'x' | 'X' => {
                let mut p = pos + 2;
                let mut saw_digit = false;
                while p < chars.len() {
                    if chars[p].is_ascii_hexdigit() {
                        saw_digit = true;
                        p += 1;
                    } else if chars[p] == '_'
                        && chars.get(p + 1).is_some_and(char::is_ascii_hexdigit)
                    {
                        p += 1;
                    } else {
                        break;
                    }
                }
                if saw_digit {
                    return p;
                }
            }
            'b' | 'B' => {
                let mut p = pos + 2;
                let mut saw_digit = false;
                while p < chars.len() {
                    if matches!(chars[p], '0' | '1') {
                        saw_digit = true;
                        p += 1;
                    } else if chars[p] == '_' && matches!(chars.get(p + 1), Some('0' | '1')) {
                        p += 1;
                    } else {
                        break;
                    }
                }
                if saw_digit {
                    return p;
                }
            }
            'o' | 'O' => {
                let mut p = pos + 2;
                let mut saw_digit = false;
                while p < chars.len() {
                    if ('0'..='7').contains(&chars[p]) {
                        saw_digit = true;
                        p += 1;
                    } else if chars[p] == '_'
                        && chars.get(p + 1).is_some_and(|c| ('0'..='7').contains(c))
                    {
                        p += 1;
                    } else {
                        break;
                    }
                }
                if saw_digit {
                    return p;
                }
            }
            _ => {}
        }
    }

    // Decimal / float / imaginary.
    while pos < chars.len() {
        if chars[pos].is_ascii_digit()
            || (chars[pos] == '_' && chars.get(pos + 1).is_some_and(char::is_ascii_digit))
        {
            pos += 1;
        } else {
            break;
        }
    }

    // Fractional part.
    if chars.get(pos) == Some(&'.') {
        pos += 1;
        while pos < chars.len() {
            if chars[pos].is_ascii_digit()
                || (chars[pos] == '_' && chars.get(pos + 1).is_some_and(char::is_ascii_digit))
            {
                pos += 1;
            } else {
                break;
            }
        }
    }

    // Exponent part.
    if matches!(chars.get(pos), Some('e' | 'E')) {
        let mut exp_pos = pos + 1;
        if matches!(chars.get(exp_pos), Some('+' | '-')) {
            exp_pos += 1;
        }
        if chars.get(exp_pos).is_some_and(char::is_ascii_digit) {
            pos = exp_pos;
            while pos < chars.len() {
                if chars[pos].is_ascii_digit()
                    || (chars[pos] == '_' && chars.get(pos + 1).is_some_and(char::is_ascii_digit))
                {
                    pos += 1;
                } else {
                    break;
                }
            }
        }
    }

    // Imaginary suffix.
    if matches!(chars.get(pos), Some('j' | 'J')) {
        pos += 1;
    }

    pos
}

/// Match a 3-char, 2-char, or 1-char Python operator/delimiter at `pos`.
fn match_operator(chars: &[char], pos: usize) -> Option<(usize, i64)> {
    let c0 = *chars.get(pos)?;
    let c1 = chars.get(pos + 1).copied();
    let c2 = chars.get(pos + 2).copied();

    // 3-char operators
    match (c0, c1, c2) {
        ('.', Some('.'), Some('.')) => return Some((3, TOK_ELLIPSIS)),
        ('<', Some('<'), Some('=')) => return Some((3, TOK_LEFTSHIFTEQUAL)),
        ('>', Some('>'), Some('=')) => return Some((3, TOK_RIGHTSHIFTEQUAL)),
        ('*', Some('*'), Some('=')) => return Some((3, TOK_DOUBLESTAREQUAL)),
        ('/', Some('/'), Some('=')) => return Some((3, TOK_DOUBLESLASHEQUAL)),
        _ => {}
    }

    // 2-char operators
    match (c0, c1) {
        ('=', Some('=')) => return Some((2, TOK_EQEQUAL)),
        ('!', Some('=')) => return Some((2, TOK_NOTEQUAL)),
        ('<', Some('=')) => return Some((2, TOK_LESSEQUAL)),
        ('>', Some('=')) => return Some((2, TOK_GREATEREQUAL)),
        ('<', Some('<')) => return Some((2, TOK_LEFTSHIFT)),
        ('>', Some('>')) => return Some((2, TOK_RIGHTSHIFT)),
        ('*', Some('*')) => return Some((2, TOK_DOUBLESTAR)),
        ('/', Some('/')) => return Some((2, TOK_DOUBLESLASH)),
        ('+', Some('=')) => return Some((2, TOK_PLUSEQUAL)),
        ('-', Some('=')) => return Some((2, TOK_MINEQUAL)),
        ('*', Some('=')) => return Some((2, TOK_STAREQUAL)),
        ('/', Some('=')) => return Some((2, TOK_SLASHEQUAL)),
        ('%', Some('=')) => return Some((2, TOK_PERCENTEQUAL)),
        ('&', Some('=')) => return Some((2, TOK_AMPEREQUAL)),
        ('|', Some('=')) => return Some((2, TOK_VBAREQUAL)),
        ('^', Some('=')) => return Some((2, TOK_CIRCUMFLEXEQUAL)),
        ('@', Some('=')) => return Some((2, TOK_ATEQUAL)),
        ('-', Some('>')) => return Some((2, TOK_RARROW)),
        (':', Some('=')) => return Some((2, TOK_COLONEQUAL)),
        _ => {}
    }

    // 1-char operators
    let single = match c0 {
        '(' => TOK_LPAR,
        ')' => TOK_RPAR,
        '[' => TOK_LSQB,
        ']' => TOK_RSQB,
        ':' => TOK_COLON,
        ',' => TOK_COMMA,
        ';' => TOK_SEMI,
        '+' => TOK_PLUS,
        '-' => TOK_MINUS,
        '*' => TOK_STAR,
        '/' => TOK_SLASH,
        '|' => TOK_VBAR,
        '&' => TOK_AMPER,
        '<' => TOK_LESS,
        '>' => TOK_GREATER,
        '=' => TOK_EQUAL,
        '.' => TOK_DOT,
        '%' => TOK_PERCENT,
        '{' => TOK_LBRACE,
        '}' => TOK_RBRACE,
        '~' => TOK_TILDE,
        '^' => TOK_CIRCUMFLEX,
        '@' => TOK_AT,
        '!' => TOK_EXCLAMATION,
        _ => return None,
    };
    Some((1, single))
}
