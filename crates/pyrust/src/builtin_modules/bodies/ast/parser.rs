//! Full Python 3.12 recursive-descent parser for the `ast` module.
//!
//! Consumes `LexToken`s from `super::super::tokenize::lexer::tokenize_source`
//! and directly materialises genuine Python `ast.AST` `PyInstance` nodes with
//! exact CPython 3.12 fields, `expr_context`s (`Load`/`Store`/`Del`),
//! `type_comments`, `type_ignores`, and UTF-8 byte spans
//! `(lineno, col_offset, end_lineno, end_col_offset)`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::value::{InstanceAttrs, PyClass, PyInstance, PyKey, Value, ValueKind};

use super::super::tokenize::lexer::{
    LexError, LexToken, TOK_AMPER, TOK_AMPEREQUAL, TOK_AT, TOK_ATEQUAL, TOK_CIRCUMFLEX,
    TOK_CIRCUMFLEXEQUAL, TOK_COLON, TOK_COLONEQUAL, TOK_COMMA, TOK_COMMENT, TOK_DEDENT, TOK_DOT,
    TOK_DOUBLESLASH, TOK_DOUBLESLASHEQUAL, TOK_DOUBLESTAR, TOK_DOUBLESTAREQUAL, TOK_ELLIPSIS,
    TOK_ENDMARKER, TOK_EQEQUAL, TOK_EQUAL, TOK_ERRORTOKEN, TOK_EXCLAMATION, TOK_FSTRING_END,
    TOK_FSTRING_MIDDLE, TOK_FSTRING_START, TOK_GREATER, TOK_GREATEREQUAL, TOK_INDENT, TOK_LBRACE,
    TOK_LEFTSHIFT, TOK_LEFTSHIFTEQUAL, TOK_LESS, TOK_LESSEQUAL, TOK_LPAR, TOK_LSQB, TOK_MINEQUAL,
    TOK_MINUS, TOK_NAME, TOK_NEWLINE, TOK_NL, TOK_NOTEQUAL, TOK_NUMBER, TOK_PERCENT,
    TOK_PERCENTEQUAL, TOK_PLUS, TOK_PLUSEQUAL, TOK_RARROW, TOK_RBRACE, TOK_RIGHTSHIFT,
    TOK_RIGHTSHIFTEQUAL, TOK_RPAR, TOK_RSQB, TOK_SEMI, TOK_SLASH, TOK_SLASHEQUAL, TOK_STAR,
    TOK_STAREQUAL, TOK_STRING, TOK_TILDE, TOK_VBAR, TOK_VBAREQUAL, tokenize_source,
};
use super::literals::{DecodedString, decode_fstring_middle, decode_number, decode_string_token};

/// Hard keywords in Python 3.12.
fn is_hard_keyword(s: &str) -> bool {
    matches!(
        s,
        "False"
            | "None"
            | "True"
            | "and"
            | "as"
            | "assert"
            | "async"
            | "await"
            | "break"
            | "class"
            | "continue"
            | "def"
            | "del"
            | "elif"
            | "else"
            | "except"
            | "finally"
            | "for"
            | "from"
            | "global"
            | "if"
            | "import"
            | "in"
            | "is"
            | "lambda"
            | "nonlocal"
            | "not"
            | "or"
            | "pass"
            | "raise"
            | "return"
            | "try"
            | "while"
            | "with"
            | "yield"
    )
}

#[derive(Debug, Clone, Copy)]
struct Span {
    lineno: u32,
    col_offset: u32,
    end_lineno: u32,
    end_col_offset: u32,
}

impl Span {
    fn from_tok(t: &LexToken) -> Self {
        Self {
            lineno: t.start_line,
            col_offset: t.start_byte_col,
            end_lineno: t.end_line,
            end_col_offset: t.end_byte_col,
        }
    }

    fn merge(self, end: Span) -> Self {
        Self {
            lineno: self.lineno,
            col_offset: self.col_offset,
            end_lineno: end.end_lineno,
            end_col_offset: end.end_col_offset,
        }
    }
}

/// Structured syntax error returned to `ast.parse` so Python raises a
/// CPython-compatible `SyntaxError` / `IndentationError` / `TabError`.
#[derive(Debug, Clone)]
pub(crate) struct AstSyntaxError {
    pub kind: &'static str,
    pub msg: String,
    pub lineno: u32,
    pub offset: u32,
    pub text: String,
    pub end_lineno: u32,
    pub end_offset: u32,
}

/// Parsed expression carrying its AST `Value`, exact source `Span`, and whether
/// it was explicitly enclosed in parentheses (important for `AnnAssign.simple`,
/// nested `BoolOp`, and invalid-target checks).
#[derive(Clone)]
struct ParsedExpr {
    node: Value,
    span: Span,
    parenthesized: bool,
    kind: ExprTag,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExprTag {
    Other,
    Name,
    Attribute,
    Subscript,
    Tuple,
    List,
    Starred,
    BoolAnd,
    BoolOr,
}

/// Parsed statement with its exact source span.
#[derive(Clone)]
struct ParsedStmt {
    node: Value,
    span: Span,
}

type FStringPartItem = (Value, Span, Option<String>);

/// Lookup table mapping AST class names (`"Module"`, `"Name"`, …) to their
/// live Python `PyClass` objects from `ast_py.py`.
pub(crate) struct AstClasses {
    map: HashMap<String, Rc<RefCell<PyClass>>>,
}

impl AstClasses {
    pub(crate) fn from_dict(dict_val: &Value) -> Result<Self, String> {
        let dict = dict_val
            .as_dict()
            .ok_or_else(|| "internal error: AST class table must be a dict".to_string())?;
        let mut map = HashMap::new();
        for (k, v) in dict.iter() {
            if let PyKey::Str(name) = k
                && let ValueKind::PyClass(cls) = v.kind()
            {
                map.insert(name.to_string(), Rc::clone(cls));
            }
        }
        Ok(Self { map })
    }

    fn class(&self, name: &str) -> Rc<RefCell<PyClass>> {
        Rc::clone(
            self.map
                .get(name)
                .unwrap_or_else(|| panic!("missing AST class {name}")),
        )
    }

    fn make(&self, name: &str, fields: &[(&str, Value)], span: Option<Span>) -> Value {
        let class = self.class(name);
        let mut attrs = InstanceAttrs::new();
        for (k, v) in fields {
            attrs.insert(*k, v.clone());
        }
        if let Some(sp) = span {
            attrs.insert("lineno", Value::int(sp.lineno as i64));
            attrs.insert("col_offset", Value::int(sp.col_offset as i64));
            attrs.insert("end_lineno", Value::int(sp.end_lineno as i64));
            attrs.insert("end_col_offset", Value::int(sp.end_col_offset as i64));
        }
        Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
    }

    fn singleton(&self, name: &str) -> Value {
        self.make(name, &[], None)
    }
}

/// Public entry point called by `bodies/ast.rs`.
pub(crate) fn parse_python_ast(
    source: &str,
    mode: &str,
    type_comments: bool,
    feature_minor: u32,
    classes: &AstClasses,
) -> Result<Value, AstSyntaxError> {
    let raw_tokens = tokenize_source(source).map_err(|e| match e {
        LexError::TokenError { msg, line, col } => AstSyntaxError {
            kind: "SyntaxError",
            msg,
            lineno: line.max(1),
            offset: col + 1,
            text: source_line_at(source, line),
            end_lineno: line.max(1),
            end_offset: col + 2,
        },
        LexError::IndentationError {
            msg,
            line,
            col,
            text,
        } => AstSyntaxError {
            kind: "IndentationError",
            msg,
            lineno: line.max(1),
            offset: col + 1,
            text,
            end_lineno: line.max(1),
            end_offset: col + 1,
        },
        LexError::TabError {
            msg,
            line,
            col,
            text,
        } => AstSyntaxError {
            kind: "TabError",
            msg,
            lineno: line.max(1),
            offset: col + 1,
            text,
            end_lineno: line.max(1),
            end_offset: col + 1,
        },
    })?;

    // Separate comments for type_comments / type_ignores, and filter NL / COMMENT
    // out of the parser token stream.
    let mut tokens: Vec<LexToken> = Vec::with_capacity(raw_tokens.len());
    let mut line_type_comments: HashMap<u32, String> = HashMap::new();
    let mut type_ignores: Vec<(u32, String)> = Vec::new();

    for tok in raw_tokens {
        match tok.tok_type {
            TOK_ERRORTOKEN => {
                return Err(AstSyntaxError {
                    kind: "SyntaxError",
                    msg: "invalid syntax".to_string(),
                    lineno: tok.start_line.max(1),
                    offset: tok.start_col + 1,
                    text: tok.line,
                    end_lineno: tok.end_line.max(1),
                    end_offset: tok.end_col + 1,
                });
            }
            TOK_COMMENT => {
                if type_comments && let Some(rest) = parse_type_comment_prefix(&tok.string) {
                    if let Some(tag) = parse_type_ignore_tag(rest) {
                        type_ignores.push((tok.start_line, tag));
                    } else {
                        line_type_comments
                            .entry(tok.start_line)
                            .or_insert_with(|| rest.trim().to_string());
                    }
                }
            }
            TOK_NL => {}
            _ => tokens.push(tok),
        }
    }

    let mut parser = AstParser {
        source,
        tokens,
        pos: 0,
        classes,
        type_comments,
        feature_minor,
        line_type_comments,
        depth: 0,
    };

    match mode {
        "exec" => {
            let stmts = parser.parse_module_body()?;
            let body: Vec<Value> = stmts.into_iter().map(|s| s.node).collect();
            let ignores: Vec<Value> = type_ignores
                .into_iter()
                .map(|(ln, tag)| {
                    classes.make(
                        "TypeIgnore",
                        &[
                            ("lineno", Value::int(ln as i64)),
                            ("tag", Value::string(tag)),
                        ],
                        None,
                    )
                })
                .collect();
            Ok(classes.make(
                "Module",
                &[
                    ("body", Value::list(body)),
                    ("type_ignores", Value::list(ignores)),
                ],
                None,
            ))
        }
        "eval" => {
            parser.skip_newlines();
            if parser.at_eof() {
                return Err(parser.err_here("invalid syntax"));
            }
            let expr = parser.parse_expr_or_star_tuple(false)?;
            parser.skip_newlines();
            if !parser.at_eof() {
                return Err(parser.err_here("invalid syntax"));
            }
            Ok(classes.make("Expression", &[("body", expr.node)], None))
        }
        "single" => {
            parser.skip_newlines();
            if parser.at_eof() {
                return Ok(classes.make("Interactive", &[("body", Value::list(vec![]))], None));
            }
            let stmts = parser.parse_stmt_line()?;
            parser.skip_newlines();
            if !parser.at_eof() {
                return Err(
                    parser.err_here("multiple statements found while compiling a single statement")
                );
            }
            let body: Vec<Value> = stmts.into_iter().map(|s| s.node).collect();
            Ok(classes.make("Interactive", &[("body", Value::list(body))], None))
        }
        "func_type" => parser.parse_func_type(),
        _ => Err(AstSyntaxError {
            kind: "ValueError",
            msg: "compile() mode must be 'exec', 'eval' or 'single'".to_string(),
            lineno: 0,
            offset: 0,
            text: String::new(),
            end_lineno: 0,
            end_offset: 0,
        }),
    }
}

fn source_line_at(src: &str, lineno: u32) -> String {
    if lineno == 0 {
        return String::new();
    }
    src.lines()
        .nth((lineno - 1) as usize)
        .map(|l| format!("{l}\n"))
        .unwrap_or_default()
}

fn parse_type_comment_prefix(comment: &str) -> Option<&str> {
    let body = comment.strip_prefix('#')?.trim_start_matches([' ', '\t']);
    body.strip_prefix("type:")
}

fn parse_type_ignore_tag(rest: &str) -> Option<String> {
    let trimmed = rest.trim_start_matches([' ', '\t']);
    let after = trimmed.strip_prefix("ignore")?;
    if after.is_empty()
        || !after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        Some(after.trim_end().to_string())
    } else {
        None
    }
}

const MAX_PARSE_DEPTH: usize = 200;

struct AstParser<'a> {
    source: &'a str,
    tokens: Vec<LexToken>,
    pos: usize,
    classes: &'a AstClasses,
    type_comments: bool,
    feature_minor: u32,
    line_type_comments: HashMap<u32, String>,
    depth: usize,
}

impl<'a> AstParser<'a> {
    fn enter_depth(&mut self) -> Result<(), AstSyntaxError> {
        self.depth += 1;
        if self.depth > MAX_PARSE_DEPTH {
            self.depth -= 1;
            return Err(self.err_here("too many nested parentheses"));
        }
        Ok(())
    }

    fn leave_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn cur(&self) -> &LexToken {
        self.tokens
            .get(self.pos)
            .unwrap_or_else(|| self.tokens.last().expect("tokens always has ENDMARKER"))
    }

    fn peek(&self, offset: usize) -> &LexToken {
        self.tokens
            .get(self.pos + offset)
            .unwrap_or_else(|| self.tokens.last().expect("tokens always has ENDMARKER"))
    }

    fn prev(&self) -> &LexToken {
        if self.pos > 0 {
            &self.tokens[self.pos - 1]
        } else {
            self.cur()
        }
    }

    fn at_eof(&self) -> bool {
        self.cur().tok_type == TOK_ENDMARKER
    }

    fn is_exact(&self, exact: i64) -> bool {
        self.cur().exact_type == exact
    }

    fn is_kw(&self, kw: &str) -> bool {
        self.cur().tok_type == TOK_NAME && self.cur().string == kw
    }

    fn is_name(&self) -> bool {
        self.cur().tok_type == TOK_NAME && !is_hard_keyword(&self.cur().string)
    }

    fn bump(&mut self) -> LexToken {
        let tok = self.cur().clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn skip_newlines(&mut self) {
        while self.is_exact(TOK_NEWLINE) {
            self.bump();
        }
    }

    fn at_stmt_end(&self) -> bool {
        matches!(
            self.cur().exact_type,
            TOK_NEWLINE | TOK_SEMI | TOK_DEDENT | TOK_ENDMARKER
        )
    }

    fn expect_exact(&mut self, exact: i64, what: &str) -> Result<LexToken, AstSyntaxError> {
        if self.is_exact(exact) {
            Ok(self.bump())
        } else {
            Err(self.err_here(&format!("expected {what}")))
        }
    }

    /// Like `expect_exact`, but a stray token inside a pattern is CPython's
    /// plain `invalid syntax` rather than `expected <what>`.
    fn expect_pattern_exact(&mut self, exact: i64, what: &str) -> Result<LexToken, AstSyntaxError> {
        if self.is_exact(exact) || self.at_eof() {
            self.expect_exact(exact, what)
        } else {
            Err(self.err_here("invalid syntax"))
        }
    }

    fn expect_kw(&mut self, kw: &str) -> Result<LexToken, AstSyntaxError> {
        if self.is_kw(kw) {
            Ok(self.bump())
        } else {
            Err(self.err_here(&format!("expected '{kw}'")))
        }
    }

    fn expect_ident(&mut self) -> Result<LexToken, AstSyntaxError> {
        if self.is_name() {
            Ok(self.bump())
        } else {
            Err(self.err_here("invalid syntax"))
        }
    }

    fn err_here(&self, msg: &str) -> AstSyntaxError {
        let t = self.cur();
        AstSyntaxError {
            kind: "SyntaxError",
            msg: msg.to_string(),
            lineno: t.start_line.max(1),
            offset: t.start_col + 1,
            text: if t.line.is_empty() {
                source_line_at(self.source, t.start_line)
            } else {
                t.line.clone()
            },
            end_lineno: t.end_line.max(1),
            end_offset: t.end_col + 1,
        }
    }

    fn err_at_span(&self, span: Span, msg: &str) -> AstSyntaxError {
        AstSyntaxError {
            kind: "SyntaxError",
            msg: msg.to_string(),
            lineno: span.lineno.max(1),
            offset: span.col_offset + 1,
            text: source_line_at(self.source, span.lineno),
            end_lineno: span.end_lineno.max(1),
            end_offset: span.end_col_offset + 1,
        }
    }

    fn take_type_comment(&mut self, lineno: u32) -> Value {
        if !self.type_comments {
            return Value::none();
        }
        match self.line_type_comments.remove(&lineno) {
            Some(s) => Value::string(s),
            None => Value::none(),
        }
    }

    // ── Module / Suite / Statements ───────────────────────────────────────

    fn parse_module_body(&mut self) -> Result<Vec<ParsedStmt>, AstSyntaxError> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !self.at_eof() {
            if self.is_exact(TOK_INDENT) {
                return Err(AstSyntaxError {
                    kind: "IndentationError",
                    msg: "unexpected indent".to_string(),
                    lineno: self.cur().start_line.max(1),
                    offset: self.cur().start_col + 1,
                    text: self.cur().line.clone(),
                    end_lineno: self.cur().end_line.max(1),
                    end_offset: self.cur().end_col + 1,
                });
            }
            stmts.extend(self.parse_stmt_line()?);
            self.skip_newlines();
        }
        Ok(stmts)
    }

    fn parse_suite(&mut self) -> Result<(Vec<ParsedStmt>, Span), AstSyntaxError> {
        if self.is_exact(TOK_NEWLINE) {
            self.bump();
            self.skip_newlines();
            if !self.is_exact(TOK_INDENT) {
                return Err(AstSyntaxError {
                    kind: "IndentationError",
                    msg: "expected an indented block".to_string(),
                    lineno: self.cur().start_line.max(1),
                    offset: self.cur().start_col + 1,
                    text: self.cur().line.clone(),
                    end_lineno: self.cur().end_line.max(1),
                    end_offset: self.cur().end_col + 1,
                });
            }
            self.bump(); // INDENT
            self.skip_newlines();
            let mut stmts = Vec::new();
            while !self.is_exact(TOK_DEDENT) && !self.at_eof() {
                stmts.extend(self.parse_stmt_line()?);
                self.skip_newlines();
            }
            self.expect_exact(TOK_DEDENT, "dedent")?;
            let end_span = stmts
                .last()
                .map(|s| s.span)
                .unwrap_or_else(|| Span::from_tok(self.prev()));
            Ok((stmts, end_span))
        } else {
            let stmts = self.parse_simple_stmts()?;
            let end_span = stmts
                .last()
                .map(|s| s.span)
                .unwrap_or_else(|| Span::from_tok(self.prev()));
            Ok((stmts, end_span))
        }
    }

    fn parse_stmt_line(&mut self) -> Result<Vec<ParsedStmt>, AstSyntaxError> {
        if self.is_compound_start() {
            Ok(vec![self.parse_compound_stmt()?])
        } else {
            self.parse_simple_stmts()
        }
    }

    fn is_compound_start(&self) -> bool {
        if self.is_exact(TOK_AT) {
            return true;
        }
        if self.cur().tok_type != TOK_NAME {
            return false;
        }
        match self.cur().string.as_str() {
            "def" | "class" | "if" | "while" | "for" | "try" | "with" => true,
            "async" => {
                self.peek(1).tok_type == TOK_NAME
                    && matches!(self.peek(1).string.as_str(), "def" | "for" | "with")
            }
            "match" => self.feature_minor >= 10 && self.looks_like_match_stmt(),
            _ => false,
        }
    }

    fn looks_like_match_stmt(&self) -> bool {
        // Scan forward from `match` to matching top-level `:` followed by NEWLINE INDENT `case`.
        let mut i = self.pos + 1;
        let mut depth = 0usize;
        while i < self.tokens.len() {
            match self.tokens[i].exact_type {
                TOK_ENDMARKER => return false,
                TOK_LPAR | TOK_LSQB | TOK_LBRACE => depth += 1,
                TOK_RPAR | TOK_RSQB | TOK_RBRACE => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                TOK_COLON if depth == 0 => {
                    let mut j = i + 1;
                    while j < self.tokens.len() && self.tokens[j].exact_type == TOK_NEWLINE {
                        j += 1;
                    }
                    if j < self.tokens.len() && self.tokens[j].exact_type == TOK_INDENT {
                        j += 1;
                        while j < self.tokens.len() && self.tokens[j].exact_type == TOK_NEWLINE {
                            j += 1;
                        }
                        return j < self.tokens.len()
                            && self.tokens[j].tok_type == TOK_NAME
                            && self.tokens[j].string == "case";
                    }
                    return false;
                }
                TOK_NEWLINE if depth == 0 => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    fn looks_like_type_alias(&self) -> bool {
        if self.feature_minor < 12 || !self.is_kw("type") {
            return false;
        }
        let next = self.peek(1);
        if next.tok_type != TOK_NAME || is_hard_keyword(&next.string) {
            return false;
        }
        let mut j = self.pos + 2;
        if j < self.tokens.len() && self.tokens[j].exact_type == TOK_LSQB {
            let mut depth = 0usize;
            while j < self.tokens.len() {
                match self.tokens[j].exact_type {
                    TOK_LSQB => depth += 1,
                    TOK_RSQB => {
                        depth -= 1;
                        if depth == 0 {
                            j += 1;
                            break;
                        }
                    }
                    TOK_ENDMARKER | TOK_NEWLINE => return false,
                    _ => {}
                }
                j += 1;
            }
        }
        j < self.tokens.len() && self.tokens[j].exact_type == TOK_EQUAL
    }

    fn parse_simple_stmts(&mut self) -> Result<Vec<ParsedStmt>, AstSyntaxError> {
        let mut stmts = Vec::new();
        stmts.push(self.parse_simple_stmt()?);
        while self.is_exact(TOK_SEMI) {
            self.bump();
            if self.is_exact(TOK_NEWLINE) || self.at_eof() || self.is_exact(TOK_DEDENT) {
                break;
            }
            stmts.push(self.parse_simple_stmt()?);
        }
        if self.is_exact(TOK_NEWLINE) {
            self.bump();
        } else if !self.at_eof() && !self.is_exact(TOK_DEDENT) {
            return Err(self.err_here("invalid syntax"));
        }
        Ok(stmts)
    }

    fn parse_simple_stmt(&mut self) -> Result<ParsedStmt, AstSyntaxError> {
        if self.cur().tok_type == TOK_NAME {
            match self.cur().string.as_str() {
                "pass" => {
                    let t = self.bump();
                    let sp = Span::from_tok(&t);
                    return Ok(ParsedStmt {
                        node: self.classes.make("Pass", &[], Some(sp)),
                        span: sp,
                    });
                }
                "break" => {
                    let t = self.bump();
                    let sp = Span::from_tok(&t);
                    return Ok(ParsedStmt {
                        node: self.classes.make("Break", &[], Some(sp)),
                        span: sp,
                    });
                }
                "continue" => {
                    let t = self.bump();
                    let sp = Span::from_tok(&t);
                    return Ok(ParsedStmt {
                        node: self.classes.make("Continue", &[], Some(sp)),
                        span: sp,
                    });
                }
                "return" => return self.parse_return(),
                "raise" => return self.parse_raise(),
                "del" => return self.parse_delete(),
                "assert" => return self.parse_assert(),
                "import" => return self.parse_import(),
                "from" => return self.parse_import_from(),
                "global" => return self.parse_global_or_nonlocal("Global"),
                "nonlocal" => return self.parse_global_or_nonlocal("Nonlocal"),
                "type" if self.looks_like_type_alias() => return self.parse_type_alias(),
                _ => {}
            }
        }
        self.parse_expr_or_assign_stmt()
    }

    fn parse_return(&mut self) -> Result<ParsedStmt, AstSyntaxError> {
        let start = Span::from_tok(&self.bump());
        if self.at_stmt_end() {
            return Ok(ParsedStmt {
                node: self
                    .classes
                    .make("Return", &[("value", Value::none())], Some(start)),
                span: start,
            });
        }
        let val = self.parse_expr_or_star_tuple(false)?;
        let sp = start.merge(val.span);
        Ok(ParsedStmt {
            node: self
                .classes
                .make("Return", &[("value", val.node)], Some(sp)),
            span: sp,
        })
    }

    fn parse_raise(&mut self) -> Result<ParsedStmt, AstSyntaxError> {
        let start = Span::from_tok(&self.bump());
        if self.at_stmt_end() {
            return Ok(ParsedStmt {
                node: self.classes.make(
                    "Raise",
                    &[("exc", Value::none()), ("cause", Value::none())],
                    Some(start),
                ),
                span: start,
            });
        }
        let exc = self.parse_expr()?;
        let (cause_val, end_sp) = if self.is_kw("from") {
            self.bump();
            let c = self.parse_expr()?;
            (c.node, c.span)
        } else {
            (Value::none(), exc.span)
        };
        let sp = start.merge(end_sp);
        Ok(ParsedStmt {
            node: self.classes.make(
                "Raise",
                &[("exc", exc.node), ("cause", cause_val)],
                Some(sp),
            ),
            span: sp,
        })
    }

    fn parse_delete(&mut self) -> Result<ParsedStmt, AstSyntaxError> {
        let start = Span::from_tok(&self.bump());
        let mut targets = Vec::new();
        let mut end_sp;
        loop {
            let item = self.parse_expr()?;
            end_sp = item.span;
            targets.push(self.convert_target_ctx(item, "Del")?);
            if self.is_exact(TOK_COMMA) {
                self.bump();
                if self.at_stmt_end() {
                    break;
                }
            } else {
                break;
            }
        }
        let sp = start.merge(end_sp);
        Ok(ParsedStmt {
            node: self
                .classes
                .make("Delete", &[("targets", Value::list(targets))], Some(sp)),
            span: sp,
        })
    }

    fn parse_assert(&mut self) -> Result<ParsedStmt, AstSyntaxError> {
        let start = Span::from_tok(&self.bump());
        let test = self.parse_expr()?;
        let (msg_val, end_sp) = if self.is_exact(TOK_COMMA) {
            self.bump();
            let m = self.parse_expr()?;
            (m.node, m.span)
        } else {
            (Value::none(), test.span)
        };
        let sp = start.merge(end_sp);
        Ok(ParsedStmt {
            node: self
                .classes
                .make("Assert", &[("test", test.node), ("msg", msg_val)], Some(sp)),
            span: sp,
        })
    }

    fn parse_global_or_nonlocal(&mut self, cls: &str) -> Result<ParsedStmt, AstSyntaxError> {
        let start = Span::from_tok(&self.bump());
        let mut names = Vec::new();
        let first = self.expect_ident()?;
        let mut end_sp = Span::from_tok(&first);
        names.push(Value::string(first.string));
        while self.is_exact(TOK_COMMA) {
            self.bump();
            let tok = self.expect_ident()?;
            end_sp = Span::from_tok(&tok);
            names.push(Value::string(tok.string));
        }
        let sp = start.merge(end_sp);
        Ok(ParsedStmt {
            node: self
                .classes
                .make(cls, &[("names", Value::list(names))], Some(sp)),
            span: sp,
        })
    }

    fn parse_import(&mut self) -> Result<ParsedStmt, AstSyntaxError> {
        let start = Span::from_tok(&self.bump());
        let mut aliases = Vec::new();
        let mut end_sp;
        loop {
            let (name_str, name_start, mut alias_end) = self.parse_dotted_name()?;
            let asname = if self.is_kw("as") {
                self.bump();
                let as_tok = self.expect_ident()?;
                alias_end = Span::from_tok(&as_tok);
                Value::string(as_tok.string)
            } else {
                Value::none()
            };
            let alias_sp = name_start.merge(alias_end);
            end_sp = alias_sp;
            aliases.push(self.classes.make(
                "alias",
                &[("name", Value::string(name_str)), ("asname", asname)],
                Some(alias_sp),
            ));
            if self.is_exact(TOK_COMMA) {
                self.bump();
            } else {
                break;
            }
        }
        let sp = start.merge(end_sp);
        Ok(ParsedStmt {
            node: self
                .classes
                .make("Import", &[("names", Value::list(aliases))], Some(sp)),
            span: sp,
        })
    }

    fn parse_dotted_name(&mut self) -> Result<(String, Span, Span), AstSyntaxError> {
        let first = self.expect_ident()?;
        let start = Span::from_tok(&first);
        let mut end = start;
        let mut name = first.string;
        while self.is_exact(TOK_DOT) {
            self.bump();
            let part = self.expect_ident()?;
            end = Span::from_tok(&part);
            name.push('.');
            name.push_str(&part.string);
        }
        Ok((name, start, end))
    }

    fn parse_import_from(&mut self) -> Result<ParsedStmt, AstSyntaxError> {
        let start = Span::from_tok(&self.bump());
        let mut level: i64 = 0;
        loop {
            if self.is_exact(TOK_DOT) {
                self.bump();
                level += 1;
            } else if self.is_exact(TOK_ELLIPSIS) {
                self.bump();
                level += 3;
            } else {
                break;
            }
        }
        let module = if self.is_kw("import") {
            if level == 0 {
                return Err(self.err_here("invalid syntax"));
            }
            Value::none()
        } else {
            let (mod_name, _, _) = self.parse_dotted_name()?;
            Value::string(mod_name)
        };
        self.expect_kw("import")?;

        let mut aliases = Vec::new();
        let end_sp = if self.is_exact(TOK_STAR) {
            let star_tok = self.bump();
            let sp = Span::from_tok(&star_tok);
            aliases.push(self.classes.make(
                "alias",
                &[("name", Value::string("*")), ("asname", Value::none())],
                Some(sp),
            ));
            sp
        } else {
            let parens = self.is_exact(TOK_LPAR);
            if parens {
                self.bump();
            }
            let mut last_sp = start;
            loop {
                if parens && self.is_exact(TOK_RPAR) {
                    if aliases.is_empty() {
                        return Err(self.err_here("invalid syntax"));
                    }
                    break;
                }
                let tok = self.expect_ident()?;
                let a_start = Span::from_tok(&tok);
                let mut a_end = a_start;
                let asname = if self.is_kw("as") {
                    self.bump();
                    let as_tok = self.expect_ident()?;
                    a_end = Span::from_tok(&as_tok);
                    Value::string(as_tok.string)
                } else {
                    Value::none()
                };
                let alias_sp = a_start.merge(a_end);
                last_sp = alias_sp;
                aliases.push(self.classes.make(
                    "alias",
                    &[("name", Value::string(tok.string)), ("asname", asname)],
                    Some(alias_sp),
                ));
                if self.is_exact(TOK_COMMA) {
                    self.bump();
                    if parens && self.is_exact(TOK_RPAR) {
                        break;
                    }
                } else {
                    break;
                }
            }
            if parens {
                let rp = self.expect_exact(TOK_RPAR, "')'")?;
                Span::from_tok(&rp)
            } else {
                last_sp
            }
        };

        let sp = start.merge(end_sp);
        Ok(ParsedStmt {
            node: self.classes.make(
                "ImportFrom",
                &[
                    ("module", module),
                    ("names", Value::list(aliases)),
                    ("level", Value::int(level)),
                ],
                Some(sp),
            ),
            span: sp,
        })
    }

    fn parse_type_alias(&mut self) -> Result<ParsedStmt, AstSyntaxError> {
        let start = Span::from_tok(&self.bump()); // `type`
        let name_tok = self.expect_ident()?;
        let name_sp = Span::from_tok(&name_tok);
        let name_node = self.classes.make(
            "Name",
            &[
                ("id", Value::string(name_tok.string)),
                ("ctx", self.classes.singleton("Store")),
            ],
            Some(name_sp),
        );
        let type_params = if self.is_exact(TOK_LSQB) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };
        self.expect_exact(TOK_EQUAL, "'='")?;
        let val = self.parse_expr()?;
        let sp = start.merge(val.span);
        Ok(ParsedStmt {
            node: self.classes.make(
                "TypeAlias",
                &[
                    ("name", name_node),
                    ("type_params", Value::list(type_params)),
                    ("value", val.node),
                ],
                Some(sp),
            ),
            span: sp,
        })
    }

    fn parse_type_params(&mut self) -> Result<Vec<Value>, AstSyntaxError> {
        if self.feature_minor < 12 {
            return Err(self.err_here("invalid syntax"));
        }
        self.expect_exact(TOK_LSQB, "'['")?;
        let mut params = Vec::new();
        while !self.is_exact(TOK_RSQB) {
            if self.is_exact(TOK_DOUBLESTAR) {
                let start = Span::from_tok(&self.bump());
                let tok = self.expect_ident()?;
                let sp = start.merge(Span::from_tok(&tok));
                params.push(self.classes.make(
                    "ParamSpec",
                    &[("name", Value::string(tok.string))],
                    Some(sp),
                ));
            } else if self.is_exact(TOK_STAR) {
                let start = Span::from_tok(&self.bump());
                let tok = self.expect_ident()?;
                let sp = start.merge(Span::from_tok(&tok));
                params.push(self.classes.make(
                    "TypeVarTuple",
                    &[("name", Value::string(tok.string))],
                    Some(sp),
                ));
            } else {
                let tok = self.expect_ident()?;
                let start = Span::from_tok(&tok);
                let (bound, end_sp) = if self.is_exact(TOK_COLON) {
                    self.bump();
                    let b = self.parse_expr()?;
                    (b.node, b.span)
                } else {
                    (Value::none(), start)
                };
                let sp = start.merge(end_sp);
                params.push(self.classes.make(
                    "TypeVar",
                    &[("name", Value::string(tok.string)), ("bound", bound)],
                    Some(sp),
                ));
            }
            if self.is_exact(TOK_COMMA) {
                self.bump();
            } else {
                break;
            }
        }
        if params.is_empty() {
            return Err(self.err_here("invalid syntax"));
        }
        self.expect_exact(TOK_RSQB, "']'")?;
        Ok(params)
    }

    fn parse_expr_or_assign_stmt(&mut self) -> Result<ParsedStmt, AstSyntaxError> {
        let first = self.parse_expr_or_star_tuple(true)?;

        // Annotated assignment: target: annotation [= value]
        if self.is_exact(TOK_COLON) {
            let is_simple = first.kind == ExprTag::Name && !first.parenthesized;
            if !matches!(
                first.kind,
                ExprTag::Name | ExprTag::Attribute | ExprTag::Subscript
            ) {
                return Err(self.err_at_span(first.span, "illegal target for annotation"));
            }
            self.bump(); // `:`
            let ann = self.parse_expr()?;
            let (val_opt, end_sp) = if self.is_exact(TOK_EQUAL) {
                self.bump();
                let rhs = self.parse_yield_or_star_tuple()?;
                (rhs.node, rhs.span)
            } else {
                (Value::none(), ann.span)
            };
            let sp = first.span.merge(end_sp);
            let target = self.convert_target_ctx(first, "Store")?;
            return Ok(ParsedStmt {
                node: self.classes.make(
                    "AnnAssign",
                    &[
                        ("target", target),
                        ("annotation", ann.node),
                        ("value", val_opt),
                        ("simple", Value::int(if is_simple { 1 } else { 0 })),
                    ],
                    Some(sp),
                ),
                span: sp,
            });
        }

        // Augmented assignment: target op= value
        if let Some(op_cls) = augassign_op_class(self.cur().exact_type) {
            if !matches!(
                first.kind,
                ExprTag::Name | ExprTag::Attribute | ExprTag::Subscript
            ) {
                return Err(
                    self.err_at_span(first.span, "illegal expression for augmented assignment")
                );
            }
            self.bump();
            let rhs = self.parse_yield_or_star_tuple()?;
            let sp = first.span.merge(rhs.span);
            let target = self.convert_target_ctx(first, "Store")?;
            return Ok(ParsedStmt {
                node: self.classes.make(
                    "AugAssign",
                    &[
                        ("target", target),
                        ("op", self.classes.singleton(op_cls)),
                        ("value", rhs.node),
                    ],
                    Some(sp),
                ),
                span: sp,
            });
        }

        // Plain / chained assignment: t1 = t2 = ... = value
        if self.is_exact(TOK_EQUAL) {
            let mut chain = vec![first];
            while self.is_exact(TOK_EQUAL) {
                self.bump();
                let next = self.parse_yield_or_star_tuple()?;
                chain.push(next);
            }
            let rhs = chain.pop().unwrap();
            let start_sp = chain[0].span;
            let sp = start_sp.merge(rhs.span);
            let mut targets = Vec::with_capacity(chain.len());
            for t in chain {
                targets.push(self.convert_target_ctx(t, "Store")?);
            }
            let tc = self.take_type_comment(sp.end_lineno);
            return Ok(ParsedStmt {
                node: self.classes.make(
                    "Assign",
                    &[
                        ("targets", Value::list(targets)),
                        ("value", rhs.node),
                        ("type_comment", tc),
                    ],
                    Some(sp),
                ),
                span: sp,
            });
        }

        // Bare starred expression is not valid as an expression statement.
        if first.kind == ExprTag::Starred {
            return Err(self.err_at_span(first.span, "can't use starred expression here"));
        }

        let sp = first.span;
        Ok(ParsedStmt {
            node: self
                .classes
                .make("Expr", &[("value", first.node)], Some(sp)),
            span: sp,
        })
    }

    fn parse_yield_or_star_tuple(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        if self.is_kw("yield") {
            self.parse_yield_expr()
        } else {
            self.parse_expr_or_star_tuple(true)
        }
    }

    // ── Compound Statements ───────────────────────────────────────────────

    fn parse_compound_stmt(&mut self) -> Result<ParsedStmt, AstSyntaxError> {
        let mut decorators = Vec::new();
        while self.is_exact(TOK_AT) {
            self.bump();
            let dec = self.parse_expr()?;
            self.expect_exact(TOK_NEWLINE, "newline")?;
            self.skip_newlines();
            decorators.push(dec.node);
        }

        if !decorators.is_empty() {
            if self.is_kw("def") {
                return self.parse_funcdef(false, decorators);
            }
            if self.is_kw("async")
                && self.peek(1).tok_type == TOK_NAME
                && self.peek(1).string == "def"
            {
                return self.parse_funcdef(true, decorators);
            }
            if self.is_kw("class") {
                return self.parse_classdef(decorators);
            }
            return Err(self.err_here("invalid syntax"));
        }

        match self.cur().string.as_str() {
            "def" => self.parse_funcdef(false, vec![]),
            "class" => self.parse_classdef(vec![]),
            "if" => self.parse_if(),
            "while" => self.parse_while(),
            "for" => self.parse_for(false),
            "try" => self.parse_try(),
            "with" => self.parse_with(false),
            "match" => self.parse_match(),
            "async" => match self.peek(1).string.as_str() {
                "def" => self.parse_funcdef(true, vec![]),
                "for" => self.parse_for(true),
                "with" => self.parse_with(true),
                _ => Err(self.err_here("invalid syntax")),
            },
            _ => Err(self.err_here("invalid syntax")),
        }
    }

    fn parse_funcdef(
        &mut self,
        is_async: bool,
        decorators: Vec<Value>,
    ) -> Result<ParsedStmt, AstSyntaxError> {
        let start = if is_async {
            let a = self.bump(); // async
            self.expect_kw("def")?;
            Span::from_tok(&a)
        } else {
            Span::from_tok(&self.bump()) // def
        };
        let name_tok = self.expect_ident()?;
        let type_params = if self.is_exact(TOK_LSQB) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };
        self.expect_exact(TOK_LPAR, "'('")?;
        let args = self.parse_parameters(true)?;
        self.expect_exact(TOK_RPAR, "')'")?;
        let returns = if self.is_exact(TOK_RARROW) {
            self.bump();
            self.parse_expr()?.node
        } else {
            Value::none()
        };
        let colon = self.expect_exact(TOK_COLON, "':'")?;
        // Function type comment can sit on the header line or on the first line of the body.
        let mut tc = self.take_type_comment(colon.start_line);
        let (body_stmts, end_sp) = self.parse_suite()?;
        if tc.is_none()
            && let Some(first) = body_stmts.first()
            && first.span.lineno > colon.start_line + 1
        {
            for ln in (colon.start_line + 1)..first.span.lineno {
                let candidate = self.take_type_comment(ln);
                if !candidate.is_none() {
                    tc = candidate;
                    break;
                }
            }
        }
        let body: Vec<Value> = body_stmts.into_iter().map(|s| s.node).collect();
        let sp = start.merge(end_sp);
        let cls = if is_async {
            "AsyncFunctionDef"
        } else {
            "FunctionDef"
        };
        Ok(ParsedStmt {
            node: self.classes.make(
                cls,
                &[
                    ("name", Value::string(name_tok.string)),
                    ("args", args),
                    ("body", Value::list(body)),
                    ("decorator_list", Value::list(decorators)),
                    ("returns", returns),
                    ("type_comment", tc),
                    ("type_params", Value::list(type_params)),
                ],
                Some(sp),
            ),
            span: sp,
        })
    }

    fn parse_parameters(&mut self, allow_annotations: bool) -> Result<Value, AstSyntaxError> {
        let mut posonlyargs: Vec<Value> = Vec::new();
        let mut args: Vec<Value> = Vec::new();
        let mut vararg: Value = Value::none();
        let mut kwonlyargs: Vec<Value> = Vec::new();
        let mut kw_defaults: Vec<Value> = Vec::new();
        let mut kwarg: Value = Value::none();
        let mut defaults: Vec<Value> = Vec::new();

        let mut seen_names: HashSet<String> = HashSet::new();
        let mut seen_star = false;
        let mut seen_slash = false;
        let mut seen_kwarg = false;
        let mut seen_pos_default = false;
        let mut bare_star_pending = false;

        let end_tok = if allow_annotations {
            TOK_RPAR
        } else {
            TOK_COLON
        };

        while !self.is_exact(end_tok) {
            if seen_kwarg {
                return Err(self.err_here("invalid syntax"));
            }
            if self.is_exact(TOK_SLASH) {
                if seen_slash || seen_star || args.is_empty() {
                    return Err(self.err_here("invalid syntax"));
                }
                self.bump();
                seen_slash = true;
                posonlyargs.append(&mut args);
            } else if self.is_exact(TOK_DOUBLESTAR) {
                self.bump();
                let tok = self.expect_ident()?;
                if !seen_names.insert(tok.string.clone()) {
                    return Err(self.err_here(&format!(
                        "duplicate argument '{}' in function definition",
                        tok.string
                    )));
                }
                let (arg_node, _) = self.finish_arg(tok, allow_annotations)?;
                kwarg = arg_node;
                seen_kwarg = true;
            } else if self.is_exact(TOK_STAR) {
                if seen_star {
                    return Err(self.err_here("invalid syntax"));
                }
                self.bump();
                seen_star = true;
                if self.is_name() {
                    let tok = self.bump();
                    if !seen_names.insert(tok.string.clone()) {
                        return Err(self.err_here(&format!(
                            "duplicate argument '{}' in function definition",
                            tok.string
                        )));
                    }
                    let (arg_node, _) = self.finish_arg(tok, allow_annotations)?;
                    vararg = arg_node;
                } else {
                    bare_star_pending = true;
                }
            } else {
                let tok = self.expect_ident()?;
                if !seen_names.insert(tok.string.clone()) {
                    return Err(self.err_here(&format!(
                        "duplicate argument '{}' in function definition",
                        tok.string
                    )));
                }
                let (arg_node, _) = self.finish_arg(tok, allow_annotations)?;
                let default_opt = if self.is_exact(TOK_EQUAL) {
                    self.bump();
                    Some(self.parse_expr()?.node)
                } else {
                    None
                };

                if seen_star {
                    bare_star_pending = false;
                    kwonlyargs.push(arg_node);
                    kw_defaults.push(default_opt.unwrap_or_else(Value::none));
                } else {
                    if let Some(d) = default_opt {
                        seen_pos_default = true;
                        defaults.push(d);
                    } else if seen_pos_default {
                        return Err(self.err_here(
                            "parameter without a default follows parameter with a default",
                        ));
                    }
                    args.push(arg_node);
                }
            }

            if self.is_exact(TOK_COMMA) {
                self.bump();
            } else {
                break;
            }
        }

        if bare_star_pending {
            return Err(self.err_here("named arguments must follow bare *"));
        }

        Ok(self.classes.make(
            "arguments",
            &[
                ("posonlyargs", Value::list(posonlyargs)),
                ("args", Value::list(args)),
                ("vararg", vararg),
                ("kwonlyargs", Value::list(kwonlyargs)),
                ("kw_defaults", Value::list(kw_defaults)),
                ("kwarg", kwarg),
                ("defaults", Value::list(defaults)),
            ],
            None,
        ))
    }

    fn finish_arg(
        &mut self,
        tok: LexToken,
        allow_annotation: bool,
    ) -> Result<(Value, Span), AstSyntaxError> {
        let start = Span::from_tok(&tok);
        let (ann, end_sp) = if allow_annotation && self.is_exact(TOK_COLON) {
            self.bump();
            let e = self.parse_expr()?;
            (e.node, e.span)
        } else {
            (Value::none(), start)
        };
        let sp = start.merge(end_sp);
        let node = self.classes.make(
            "arg",
            &[
                ("arg", Value::string(tok.string)),
                ("annotation", ann),
                ("type_comment", Value::none()),
            ],
            Some(sp),
        );
        Ok((node, sp))
    }

    fn parse_classdef(&mut self, decorators: Vec<Value>) -> Result<ParsedStmt, AstSyntaxError> {
        let start = Span::from_tok(&self.bump()); // class
        let name_tok = self.expect_ident()?;
        let type_params = if self.is_exact(TOK_LSQB) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };
        let (bases, keywords) = if self.is_exact(TOK_LPAR) {
            self.bump();
            let (b, kw) = self.parse_call_arguments()?;
            self.expect_exact(TOK_RPAR, "')'")?;
            (b, kw)
        } else {
            (Vec::new(), Vec::new())
        };
        self.expect_exact(TOK_COLON, "':'")?;
        let (body_stmts, end_sp) = self.parse_suite()?;
        let body: Vec<Value> = body_stmts.into_iter().map(|s| s.node).collect();
        let sp = start.merge(end_sp);
        Ok(ParsedStmt {
            node: self.classes.make(
                "ClassDef",
                &[
                    ("name", Value::string(name_tok.string)),
                    ("bases", Value::list(bases)),
                    ("keywords", Value::list(keywords)),
                    ("body", Value::list(body)),
                    ("decorator_list", Value::list(decorators)),
                    ("type_params", Value::list(type_params)),
                ],
                Some(sp),
            ),
            span: sp,
        })
    }

    fn parse_if(&mut self) -> Result<ParsedStmt, AstSyntaxError> {
        let start = Span::from_tok(&self.bump()); // `if` or `elif`
        let test = self.parse_expr()?;
        self.expect_exact(TOK_COLON, "':'")?;
        let (body_stmts, mut end_sp) = self.parse_suite()?;
        let body: Vec<Value> = body_stmts.into_iter().map(|s| s.node).collect();
        let orelse = if self.is_kw("elif") {
            let elif_stmt = self.parse_if()?;
            end_sp = elif_stmt.span;
            vec![elif_stmt.node]
        } else if self.is_kw("else") {
            self.bump();
            self.expect_exact(TOK_COLON, "':'")?;
            let (else_stmts, else_end) = self.parse_suite()?;
            end_sp = else_end;
            else_stmts.into_iter().map(|s| s.node).collect()
        } else {
            Vec::new()
        };
        let sp = start.merge(end_sp);
        Ok(ParsedStmt {
            node: self.classes.make(
                "If",
                &[
                    ("test", test.node),
                    ("body", Value::list(body)),
                    ("orelse", Value::list(orelse)),
                ],
                Some(sp),
            ),
            span: sp,
        })
    }

    fn parse_while(&mut self) -> Result<ParsedStmt, AstSyntaxError> {
        let start = Span::from_tok(&self.bump()); // while
        let test = self.parse_expr()?;
        self.expect_exact(TOK_COLON, "':'")?;
        let (body_stmts, mut end_sp) = self.parse_suite()?;
        let body: Vec<Value> = body_stmts.into_iter().map(|s| s.node).collect();
        let orelse = if self.is_kw("else") {
            self.bump();
            self.expect_exact(TOK_COLON, "':'")?;
            let (else_stmts, else_end) = self.parse_suite()?;
            end_sp = else_end;
            else_stmts.into_iter().map(|s| s.node).collect()
        } else {
            Vec::new()
        };
        let sp = start.merge(end_sp);
        Ok(ParsedStmt {
            node: self.classes.make(
                "While",
                &[
                    ("test", test.node),
                    ("body", Value::list(body)),
                    ("orelse", Value::list(orelse)),
                ],
                Some(sp),
            ),
            span: sp,
        })
    }

    fn parse_for(&mut self, is_async: bool) -> Result<ParsedStmt, AstSyntaxError> {
        let start = if is_async {
            let a = self.bump();
            self.expect_kw("for")?;
            Span::from_tok(&a)
        } else {
            Span::from_tok(&self.bump())
        };
        let target_expr = self.parse_for_targets()?;
        let target = self.convert_target_ctx(target_expr, "Store")?;
        self.expect_kw("in")?;
        let iter = self.parse_expr_or_star_tuple(false)?;
        let colon = self.expect_exact(TOK_COLON, "':'")?;
        let tc = self.take_type_comment(colon.start_line);
        let (body_stmts, mut end_sp) = self.parse_suite()?;
        let body: Vec<Value> = body_stmts.into_iter().map(|s| s.node).collect();
        let orelse = if self.is_kw("else") {
            self.bump();
            self.expect_exact(TOK_COLON, "':'")?;
            let (else_stmts, else_end) = self.parse_suite()?;
            end_sp = else_end;
            else_stmts.into_iter().map(|s| s.node).collect()
        } else {
            Vec::new()
        };
        let sp = start.merge(end_sp);
        let cls = if is_async { "AsyncFor" } else { "For" };
        Ok(ParsedStmt {
            node: self.classes.make(
                cls,
                &[
                    ("target", target),
                    ("iter", iter.node),
                    ("body", Value::list(body)),
                    ("orelse", Value::list(orelse)),
                    ("type_comment", tc),
                ],
                Some(sp),
            ),
            span: sp,
        })
    }

    fn parse_for_targets(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let first = self.parse_target_atom()?;
        if !self.is_exact(TOK_COMMA) {
            return Ok(first);
        }
        let start = first.span;
        let mut elts = vec![first.node];
        let mut end_sp = start;
        while self.is_exact(TOK_COMMA) {
            end_sp = Span::from_tok(&self.bump());
            if self.is_kw("in") {
                break;
            }
            let item = self.parse_target_atom()?;
            end_sp = item.span;
            elts.push(item.node);
        }
        let sp = start.merge(end_sp);
        Ok(ParsedExpr {
            node: self.classes.make(
                "Tuple",
                &[
                    ("elts", Value::list(elts)),
                    ("ctx", self.classes.singleton("Load")),
                ],
                Some(sp),
            ),
            span: sp,
            parenthesized: false,
            kind: ExprTag::Tuple,
        })
    }

    fn parse_target_atom(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        if self.is_exact(TOK_STAR) {
            let start = Span::from_tok(&self.bump());
            let inner = self.parse_bitor()?;
            let sp = start.merge(inner.span);
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "Starred",
                    &[
                        ("value", inner.node),
                        ("ctx", self.classes.singleton("Load")),
                    ],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Starred,
            });
        }
        // `for x in ...` — stop before `in`, so parse at bitwise-or level (below comparison).
        self.parse_bitor()
    }

    fn parse_with(&mut self, is_async: bool) -> Result<ParsedStmt, AstSyntaxError> {
        let start = if is_async {
            let a = self.bump();
            self.expect_kw("with")?;
            Span::from_tok(&a)
        } else {
            Span::from_tok(&self.bump())
        };

        let mut items = Vec::new();
        // Handle Python 3.10+ parenthesized multi-item `with (a as x, b as y):` vs
        // a parenthesized context expression like `with (a) as x:`.
        let parenthesized_items = self.is_exact(TOK_LPAR) && self.looks_like_paren_with_items();
        if parenthesized_items {
            self.bump(); // `(`
            while !self.is_exact(TOK_RPAR) {
                items.push(self.parse_withitem()?);
                if self.is_exact(TOK_COMMA) {
                    self.bump();
                } else {
                    break;
                }
            }
            self.expect_exact(TOK_RPAR, "')'")?;
        } else {
            loop {
                items.push(self.parse_withitem()?);
                if self.is_exact(TOK_COMMA) {
                    self.bump();
                } else {
                    break;
                }
            }
        }

        let colon = self.expect_exact(TOK_COLON, "':'")?;
        let tc = self.take_type_comment(colon.start_line);
        let (body_stmts, end_sp) = self.parse_suite()?;
        let body: Vec<Value> = body_stmts.into_iter().map(|s| s.node).collect();
        let sp = start.merge(end_sp);
        let cls = if is_async { "AsyncWith" } else { "With" };
        Ok(ParsedStmt {
            node: self.classes.make(
                cls,
                &[
                    ("items", Value::list(items)),
                    ("body", Value::list(body)),
                    ("type_comment", tc),
                ],
                Some(sp),
            ),
            span: sp,
        })
    }

    fn looks_like_paren_with_items(&self) -> bool {
        // Scan inside `(...)`: if we find top-level `as` or `,` before `)` AND
        // the token immediately after `)` is `:`, treat as parenthesized with-items.
        let mut i = self.pos + 1;
        let mut depth = 0usize;
        let mut saw_as_or_comma = false;
        while i < self.tokens.len() {
            match self.tokens[i].exact_type {
                TOK_LPAR | TOK_LSQB | TOK_LBRACE => depth += 1,
                TOK_RPAR if depth == 0 => {
                    return saw_as_or_comma
                        && self
                            .tokens
                            .get(i + 1)
                            .is_some_and(|t| t.exact_type == TOK_COLON);
                }
                TOK_RPAR | TOK_RSQB | TOK_RBRACE => {
                    depth = depth.saturating_sub(1);
                }
                TOK_COMMA if depth == 0 => saw_as_or_comma = true,
                TOK_NAME if depth == 0 && self.tokens[i].string == "as" => saw_as_or_comma = true,
                TOK_ENDMARKER => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    fn parse_withitem(&mut self) -> Result<Value, AstSyntaxError> {
        let ctx_expr = self.parse_expr()?;
        let opt_vars = if self.is_kw("as") {
            self.bump();
            let t = self.parse_target_atom()?;
            self.convert_target_ctx(t, "Store")?
        } else {
            Value::none()
        };
        Ok(self.classes.make(
            "withitem",
            &[("context_expr", ctx_expr.node), ("optional_vars", opt_vars)],
            None,
        ))
    }

    fn parse_try(&mut self) -> Result<ParsedStmt, AstSyntaxError> {
        let start = Span::from_tok(&self.bump()); // try
        self.expect_exact(TOK_COLON, "':'")?;
        let (body_stmts, mut end_sp) = self.parse_suite()?;
        let body: Vec<Value> = body_stmts.into_iter().map(|s| s.node).collect();

        let mut handlers = Vec::new();
        let mut is_star_mode: Option<bool> = None;
        let mut seen_bare_except = false;

        while self.is_kw("except") {
            let h_start = Span::from_tok(&self.bump());
            let is_star = if self.is_exact(TOK_STAR) {
                if self.feature_minor < 11 {
                    return Err(self.err_here("invalid syntax"));
                }
                self.bump();
                true
            } else {
                false
            };
            if let Some(prev_star) = is_star_mode {
                if prev_star != is_star {
                    return Err(
                        self.err_here("cannot have both 'except' and 'except*' on the same 'try'")
                    );
                }
            } else {
                is_star_mode = Some(is_star);
            }

            let (exc_type, exc_name) = if self.is_exact(TOK_COLON) {
                if is_star {
                    return Err(self.err_here("expected one or more exception types"));
                }
                if seen_bare_except {
                    return Err(self.err_here("default 'except:' must be last"));
                }
                seen_bare_except = true;
                (Value::none(), Value::none())
            } else {
                if seen_bare_except {
                    return Err(self.err_here("default 'except:' must be last"));
                }
                let ty = self.parse_expr()?;
                let name = if self.is_kw("as") {
                    self.bump();
                    let nt = self.expect_ident()?;
                    Value::string(nt.string)
                } else {
                    Value::none()
                };
                (ty.node, name)
            };
            self.expect_exact(TOK_COLON, "':'")?;
            let (h_body_stmts, h_end) = self.parse_suite()?;
            end_sp = h_end;
            let h_body: Vec<Value> = h_body_stmts.into_iter().map(|s| s.node).collect();
            let h_sp = h_start.merge(h_end);
            handlers.push(self.classes.make(
                "ExceptHandler",
                &[
                    ("type", exc_type),
                    ("name", exc_name),
                    ("body", Value::list(h_body)),
                ],
                Some(h_sp),
            ));
        }

        let orelse = if self.is_kw("else") {
            if handlers.is_empty() {
                return Err(self.err_here("expected 'except' or 'finally' block"));
            }
            self.bump();
            self.expect_exact(TOK_COLON, "':'")?;
            let (else_stmts, else_end) = self.parse_suite()?;
            end_sp = else_end;
            else_stmts.into_iter().map(|s| s.node).collect()
        } else {
            Vec::new()
        };

        let finalbody = if self.is_kw("finally") {
            self.bump();
            self.expect_exact(TOK_COLON, "':'")?;
            let (fin_stmts, fin_end) = self.parse_suite()?;
            end_sp = fin_end;
            fin_stmts.into_iter().map(|s| s.node).collect()
        } else {
            Vec::new()
        };

        if handlers.is_empty() && finalbody.is_empty() {
            return Err(self.err_here("expected 'except' or 'finally' block"));
        }

        let sp = start.merge(end_sp);
        let cls = if is_star_mode == Some(true) {
            "TryStar"
        } else {
            "Try"
        };
        Ok(ParsedStmt {
            node: self.classes.make(
                cls,
                &[
                    ("body", Value::list(body)),
                    ("handlers", Value::list(handlers)),
                    ("orelse", Value::list(orelse)),
                    ("finalbody", Value::list(finalbody)),
                ],
                Some(sp),
            ),
            span: sp,
        })
    }

    // ── Match / Patterns (PEP 634) ────────────────────────────────────────

    fn parse_match(&mut self) -> Result<ParsedStmt, AstSyntaxError> {
        let start = Span::from_tok(&self.bump()); // match
        let subject = self.parse_expr_or_star_tuple(false)?;
        self.expect_exact(TOK_COLON, "':'")?;
        self.expect_exact(TOK_NEWLINE, "newline")?;
        self.skip_newlines();
        self.expect_exact(TOK_INDENT, "indent")?;
        self.skip_newlines();

        let mut cases = Vec::new();
        let mut end_sp = start;
        while !self.is_exact(TOK_DEDENT) && !self.at_eof() {
            self.expect_kw("case")?;
            let (pat, _) = self.parse_patterns()?;
            let guard = if self.is_kw("if") {
                self.bump();
                self.parse_expr()?.node
            } else {
                Value::none()
            };
            if !self.is_exact(TOK_COLON) {
                // CPython only reports `expected ':'` when the header ends at
                // the newline; any other stray token is plain `invalid syntax`.
                return Err(self.err_here(if self.is_exact(TOK_NEWLINE) {
                    "expected ':'"
                } else {
                    "invalid syntax"
                }));
            }
            self.bump();
            let (body_stmts, case_end) = self.parse_suite()?;
            end_sp = case_end;
            let body: Vec<Value> = body_stmts.into_iter().map(|s| s.node).collect();
            cases.push(self.classes.make(
                "match_case",
                &[
                    ("pattern", pat),
                    ("guard", guard),
                    ("body", Value::list(body)),
                ],
                None,
            ));
            self.skip_newlines();
        }
        if cases.is_empty() {
            return Err(self.err_here("expected 'case' block"));
        }
        self.expect_exact(TOK_DEDENT, "dedent")?;
        let sp = start.merge(end_sp);
        Ok(ParsedStmt {
            node: self.classes.make(
                "Match",
                &[("subject", subject.node), ("cases", Value::list(cases))],
                Some(sp),
            ),
            span: sp,
        })
    }

    fn parse_patterns(&mut self) -> Result<(Value, Span), AstSyntaxError> {
        let (first, first_sp, is_star) = self.parse_maybe_star_pattern()?;
        if !self.is_exact(TOK_COMMA) {
            if is_star {
                return Err(self.err_at_span(first_sp, "invalid syntax"));
            }
            return Ok((first, first_sp));
        }
        let mut pats = vec![first];
        let mut end_sp = first_sp;
        while self.is_exact(TOK_COMMA) {
            end_sp = Span::from_tok(&self.bump());
            if self.is_exact(TOK_COLON) || self.is_kw("if") {
                break;
            }
            let (p, p_sp, _) = self.parse_maybe_star_pattern()?;
            end_sp = p_sp;
            pats.push(p);
        }
        let sp = first_sp.merge(end_sp);
        Ok((
            self.classes.make(
                "MatchSequence",
                &[("patterns", Value::list(pats))],
                Some(sp),
            ),
            sp,
        ))
    }

    fn parse_maybe_star_pattern(&mut self) -> Result<(Value, Span, bool), AstSyntaxError> {
        if self.is_exact(TOK_STAR) {
            let start = Span::from_tok(&self.bump());
            let tok = self.expect_ident()?;
            let sp = start.merge(Span::from_tok(&tok));
            let name = if tok.string == "_" {
                Value::none()
            } else {
                Value::string(tok.string)
            };
            return Ok((
                self.classes.make("MatchStar", &[("name", name)], Some(sp)),
                sp,
                true,
            ));
        }
        let (p, sp) = self.parse_pattern()?;
        Ok((p, sp, false))
    }

    fn parse_pattern(&mut self) -> Result<(Value, Span), AstSyntaxError> {
        let (pat, pat_sp) = self.parse_or_pattern()?;
        if self.is_kw("as") {
            self.bump();
            let name_tok = self.expect_ident()?;
            if name_tok.string == "_" {
                return Err(self.err_here("cannot use '_' as a target"));
            }
            let sp = pat_sp.merge(Span::from_tok(&name_tok));
            return Ok((
                self.classes.make(
                    "MatchAs",
                    &[("pattern", pat), ("name", Value::string(name_tok.string))],
                    Some(sp),
                ),
                sp,
            ));
        }
        Ok((pat, pat_sp))
    }

    fn parse_or_pattern(&mut self) -> Result<(Value, Span), AstSyntaxError> {
        let (first, first_sp) = self.parse_closed_pattern()?;
        if !self.is_exact(TOK_VBAR) {
            return Ok((first, first_sp));
        }
        let mut alts = vec![first];
        let mut end_sp = first_sp;
        while self.is_exact(TOK_VBAR) {
            self.bump();
            let (p, p_sp) = self.parse_closed_pattern()?;
            end_sp = p_sp;
            alts.push(p);
        }
        let sp = first_sp.merge(end_sp);
        Ok((
            self.classes
                .make("MatchOr", &[("patterns", Value::list(alts))], Some(sp)),
            sp,
        ))
    }

    fn parse_closed_pattern(&mut self) -> Result<(Value, Span), AstSyntaxError> {
        self.enter_depth()?;
        let res = self.parse_closed_pattern_inner();
        self.leave_depth();
        res
    }

    fn parse_closed_pattern_inner(&mut self) -> Result<(Value, Span), AstSyntaxError> {
        // Singleton literals: True, False, None
        if self.is_kw("True") || self.is_kw("False") || self.is_kw("None") {
            let tok = self.bump();
            let sp = Span::from_tok(&tok);
            let val = match tok.string.as_str() {
                "True" => Value::bool_(true),
                "False" => Value::bool_(false),
                _ => Value::none(),
            };
            return Ok((
                self.classes
                    .make("MatchSingleton", &[("value", val)], Some(sp)),
                sp,
            ));
        }

        // Literal number / signed number / complex literal (a +/- bj)
        if self.cur().tok_type == TOK_NUMBER || self.is_exact(TOK_MINUS) {
            let expr = self.parse_literal_pattern_expr()?;
            let sp = expr.span;
            return Ok((
                self.classes
                    .make("MatchValue", &[("value", expr.node)], Some(sp)),
                sp,
            ));
        }

        // String / bytes literal (adjacent literals concatenate; f-strings parse
        // here and are rejected later at compile time, as in CPython)
        if matches!(self.cur().tok_type, TOK_STRING | TOK_FSTRING_START) {
            let expr = self.parse_string_or_fstring()?;
            let sp = expr.span;
            return Ok((
                self.classes
                    .make("MatchValue", &[("value", expr.node)], Some(sp)),
                sp,
            ));
        }

        // Sequence pattern `[...]`
        if self.is_exact(TOK_LSQB) {
            let start = Span::from_tok(&self.bump());
            let mut pats = Vec::new();
            while !self.is_exact(TOK_RSQB) {
                let (p, _, _) = self.parse_maybe_star_pattern()?;
                pats.push(p);
                if self.is_exact(TOK_COMMA) {
                    self.bump();
                } else {
                    break;
                }
            }
            let end = Span::from_tok(&self.expect_pattern_exact(TOK_RSQB, "']'")?);
            let sp = start.merge(end);
            return Ok((
                self.classes.make(
                    "MatchSequence",
                    &[("patterns", Value::list(pats))],
                    Some(sp),
                ),
                sp,
            ));
        }

        // Group or sequence pattern `(...)`
        if self.is_exact(TOK_LPAR) {
            let start = Span::from_tok(&self.bump());
            if self.is_exact(TOK_RPAR) {
                let end = Span::from_tok(&self.bump());
                let sp = start.merge(end);
                return Ok((
                    self.classes.make(
                        "MatchSequence",
                        &[("patterns", Value::list(vec![]))],
                        Some(sp),
                    ),
                    sp,
                ));
            }
            let (first, _, first_star) = self.parse_maybe_star_pattern()?;
            if self.is_exact(TOK_COMMA) {
                let mut pats = vec![first];
                while self.is_exact(TOK_COMMA) {
                    self.bump();
                    if self.is_exact(TOK_RPAR) {
                        break;
                    }
                    let (p, _, _) = self.parse_maybe_star_pattern()?;
                    pats.push(p);
                }
                let end = Span::from_tok(&self.expect_pattern_exact(TOK_RPAR, "')'")?);
                let sp = start.merge(end);
                return Ok((
                    self.classes.make(
                        "MatchSequence",
                        &[("patterns", Value::list(pats))],
                        Some(sp),
                    ),
                    sp,
                ));
            }
            if first_star {
                return Err(self.err_here("invalid syntax"));
            }
            let _end = self.expect_pattern_exact(TOK_RPAR, "')'")?;
            return Ok((first, start.merge(Span::from_tok(self.prev()))));
        }

        // Mapping pattern `{k: p, **rest}`
        if self.is_exact(TOK_LBRACE) {
            let start = Span::from_tok(&self.bump());
            let mut keys = Vec::new();
            let mut pats = Vec::new();
            let mut rest = Value::none();
            while !self.is_exact(TOK_RBRACE) {
                if self.is_exact(TOK_DOUBLESTAR) {
                    self.bump();
                    let tok = self.expect_ident()?;
                    if tok.string == "_" {
                        return Err(self.err_here("cannot use '_' as a target"));
                    }
                    rest = Value::string(tok.string);
                    if self.is_exact(TOK_COMMA) {
                        self.bump();
                    }
                    break;
                }
                let key_expr = self.parse_mapping_pattern_key()?;
                self.expect_pattern_exact(TOK_COLON, "':'")?;
                let (val_pat, _) = self.parse_pattern()?;
                keys.push(key_expr.node);
                pats.push(val_pat);
                if self.is_exact(TOK_COMMA) {
                    self.bump();
                } else {
                    break;
                }
            }
            let end = Span::from_tok(&self.expect_pattern_exact(TOK_RBRACE, "'}'")?);
            let sp = start.merge(end);
            return Ok((
                self.classes.make(
                    "MatchMapping",
                    &[
                        ("keys", Value::list(keys)),
                        ("patterns", Value::list(pats)),
                        ("rest", rest),
                    ],
                    Some(sp),
                ),
                sp,
            ));
        }

        // Name, wildcard `_`, dotted value `a.b`, or class pattern `Cls(...)`
        if self.is_name() {
            let first = self.bump();
            let start = Span::from_tok(&first);
            if !self.is_exact(TOK_DOT) && !self.is_exact(TOK_LPAR) {
                let name_val = if first.string == "_" {
                    Value::none()
                } else {
                    Value::string(first.string)
                };
                return Ok((
                    self.classes.make(
                        "MatchAs",
                        &[("pattern", Value::none()), ("name", name_val)],
                        Some(start),
                    ),
                    start,
                ));
            }

            let mut expr_node = self.classes.make(
                "Name",
                &[
                    ("id", Value::string(first.string)),
                    ("ctx", self.classes.singleton("Load")),
                ],
                Some(start),
            );
            let mut expr_sp = start;
            let mut has_dot = false;
            while self.is_exact(TOK_DOT) {
                has_dot = true;
                self.bump();
                let attr = self.expect_ident()?;
                expr_sp = start.merge(Span::from_tok(&attr));
                expr_node = self.classes.make(
                    "Attribute",
                    &[
                        ("value", expr_node),
                        ("attr", Value::string(attr.string)),
                        ("ctx", self.classes.singleton("Load")),
                    ],
                    Some(expr_sp),
                );
            }

            if self.is_exact(TOK_LPAR) {
                self.bump();
                let mut pos_pats = Vec::new();
                let mut kwd_attrs = Vec::new();
                let mut kwd_pats = Vec::new();
                let mut seen_kw = false;
                while !self.is_exact(TOK_RPAR) {
                    if self.is_name() && self.peek(1).exact_type == TOK_EQUAL {
                        seen_kw = true;
                        let attr = self.bump();
                        self.bump(); // `=`
                        let (p, _) = self.parse_pattern()?;
                        kwd_attrs.push(Value::string(attr.string));
                        kwd_pats.push(p);
                    } else {
                        if seen_kw {
                            return Err(
                                self.err_here("positional patterns follow keyword patterns")
                            );
                        }
                        let (p, _) = self.parse_pattern()?;
                        pos_pats.push(p);
                    }
                    if self.is_exact(TOK_COMMA) {
                        self.bump();
                    } else {
                        break;
                    }
                }
                let end = Span::from_tok(&self.expect_pattern_exact(TOK_RPAR, "')'")?);
                let sp = start.merge(end);
                return Ok((
                    self.classes.make(
                        "MatchClass",
                        &[
                            ("cls", expr_node),
                            ("patterns", Value::list(pos_pats)),
                            ("kwd_attrs", Value::list(kwd_attrs)),
                            ("kwd_patterns", Value::list(kwd_pats)),
                        ],
                        Some(sp),
                    ),
                    sp,
                ));
            }

            if has_dot {
                return Ok((
                    self.classes
                        .make("MatchValue", &[("value", expr_node)], Some(expr_sp)),
                    expr_sp,
                ));
            }
        }

        Err(self.err_here("invalid syntax"))
    }

    /// PEP 634 `signed_number` / `complex_number`:
    /// `['-'] NUMBER [('+'|'-') imaginary NUMBER]`.  Mirrors CPython's
    /// `ensure_real` / `ensure_imaginary` actions: the left operand of a complex
    /// literal must be real and the right operand must be imaginary.
    fn parse_literal_pattern_expr(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        if self.is_exact(TOK_MINUS) && self.peek(1).tok_type != TOK_NUMBER {
            self.bump();
            return Err(self.err_here("invalid syntax"));
        }
        if !self.is_exact(TOK_MINUS) && self.cur().tok_type != TOK_NUMBER {
            return Err(self.err_here("invalid syntax"));
        }
        let left = self.parse_signed_number_pattern()?;
        let left_imaginary = self.prev().string.ends_with(['j', 'J']);
        if self.is_exact(TOK_PLUS) || self.is_exact(TOK_MINUS) {
            if left_imaginary {
                return Err(self.err_at_span(left.span, "real number required in complex literal"));
            }
            let op_cls = if self.bump().exact_type == TOK_PLUS {
                "Add"
            } else {
                "Sub"
            };
            if self.cur().tok_type != TOK_NUMBER {
                return Err(self.err_here("invalid syntax"));
            }
            if !self.cur().string.ends_with(['j', 'J']) {
                let sp = Span::from_tok(self.cur());
                return Err(self.err_at_span(sp, "imaginary number required in complex literal"));
            }
            let right = self.parse_number_constant()?;
            let sp = left.span.merge(right.span);
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "BinOp",
                    &[
                        ("left", left.node),
                        ("op", self.classes.singleton(op_cls)),
                        ("right", right.node),
                    ],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }
        Ok(left)
    }

    fn parse_signed_number_pattern(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        if self.is_exact(TOK_MINUS) {
            let start = Span::from_tok(&self.bump());
            let num = self.parse_number_constant()?;
            let sp = start.merge(num.span);
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "UnaryOp",
                    &[
                        ("op", self.classes.singleton("USub")),
                        ("operand", num.node),
                    ],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }
        self.parse_number_constant()
    }

    fn parse_mapping_pattern_key(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        if self.is_kw("True") || self.is_kw("False") || self.is_kw("None") {
            let tok = self.bump();
            let sp = Span::from_tok(&tok);
            let val = match tok.string.as_str() {
                "True" => Value::bool_(true),
                "False" => Value::bool_(false),
                _ => Value::none(),
            };
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "Constant",
                    &[("value", val), ("kind", Value::none())],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }
        if self.cur().tok_type == TOK_NUMBER || self.is_exact(TOK_MINUS) {
            return self.parse_literal_pattern_expr();
        }
        if matches!(self.cur().tok_type, TOK_STRING | TOK_FSTRING_START) {
            return self.parse_string_or_fstring();
        }
        if self.is_name() {
            let first = self.bump();
            let start = Span::from_tok(&first);
            let mut node = self.classes.make(
                "Name",
                &[
                    ("id", Value::string(first.string)),
                    ("ctx", self.classes.singleton("Load")),
                ],
                Some(start),
            );
            let mut sp = start;
            if !self.is_exact(TOK_DOT) {
                return Err(self.err_here("invalid syntax"));
            }
            while self.is_exact(TOK_DOT) {
                self.bump();
                let attr = self.expect_ident()?;
                sp = start.merge(Span::from_tok(&attr));
                node = self.classes.make(
                    "Attribute",
                    &[
                        ("value", node),
                        ("attr", Value::string(attr.string)),
                        ("ctx", self.classes.singleton("Load")),
                    ],
                    Some(sp),
                );
            }
            return Ok(ParsedExpr {
                node,
                span: sp,
                parenthesized: false,
                kind: ExprTag::Attribute,
            });
        }
        Err(self.err_here("invalid syntax"))
    }

    // ── FunctionType mode (`mode='func_type'`) ────────────────────────────

    fn parse_func_type(&mut self) -> Result<Value, AstSyntaxError> {
        self.skip_newlines();
        self.expect_exact(TOK_LPAR, "'('")?;
        let mut argtypes = Vec::new();
        if !self.is_exact(TOK_RPAR) {
            if self.is_exact(TOK_ELLIPSIS) {
                let tok = self.bump();
                let sp = Span::from_tok(&tok);
                argtypes.push(self.classes.make(
                    "Constant",
                    &[("value", Value::ellipsis()), ("kind", Value::none())],
                    Some(sp),
                ));
            } else {
                loop {
                    if self.is_exact(TOK_STAR) || self.is_exact(TOK_DOUBLESTAR) {
                        self.bump();
                    }
                    argtypes.push(self.parse_expr()?.node);
                    if self.is_exact(TOK_COMMA) {
                        self.bump();
                        if self.is_exact(TOK_RPAR) {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
        }
        self.expect_exact(TOK_RPAR, "')'")?;
        self.expect_exact(TOK_RARROW, "'->'")?;
        let returns = self.parse_expr()?.node;
        self.skip_newlines();
        if !self.at_eof() {
            return Err(self.err_here("invalid syntax"));
        }
        Ok(self.classes.make(
            "FunctionType",
            &[("argtypes", Value::list(argtypes)), ("returns", returns)],
            None,
        ))
    }

    // ── Expressions ───────────────────────────────────────────────────────

    fn parse_expr_or_star_tuple(
        &mut self,
        allow_yield: bool,
    ) -> Result<ParsedExpr, AstSyntaxError> {
        if allow_yield && self.is_kw("yield") {
            return self.parse_yield_expr();
        }
        let first = self.parse_star_or_expr()?;
        if !self.is_exact(TOK_COMMA) {
            return Ok(first);
        }
        let start = first.span;
        let mut elts = vec![first.node];
        let mut end_sp = start;
        while self.is_exact(TOK_COMMA) {
            end_sp = Span::from_tok(&self.bump());
            if self.at_stmt_end()
                || self.is_exact(TOK_EQUAL)
                || self.is_exact(TOK_COLON)
                || self.is_exact(TOK_RPAR)
                || self.is_exact(TOK_RSQB)
                || self.is_exact(TOK_RBRACE)
            {
                break;
            }
            let item = self.parse_star_or_expr()?;
            end_sp = item.span;
            elts.push(item.node);
        }
        let sp = start.merge(end_sp);
        Ok(ParsedExpr {
            node: self.classes.make(
                "Tuple",
                &[
                    ("elts", Value::list(elts)),
                    ("ctx", self.classes.singleton("Load")),
                ],
                Some(sp),
            ),
            span: sp,
            parenthesized: false,
            kind: ExprTag::Tuple,
        })
    }

    fn parse_star_or_expr(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        if self.is_exact(TOK_STAR) {
            let start = Span::from_tok(&self.bump());
            let inner = self.parse_bitor()?;
            let sp = start.merge(inner.span);
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "Starred",
                    &[
                        ("value", inner.node),
                        ("ctx", self.classes.singleton("Load")),
                    ],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Starred,
            });
        }
        self.parse_named_expr()
    }

    fn parse_yield_expr(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let start = Span::from_tok(&self.bump()); // yield
        if self.is_kw("from") {
            self.bump();
            let val = self.parse_expr()?;
            let sp = start.merge(val.span);
            return Ok(ParsedExpr {
                node: self
                    .classes
                    .make("YieldFrom", &[("value", val.node)], Some(sp)),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }
        if self.at_stmt_end()
            || self.is_exact(TOK_RPAR)
            || self.is_exact(TOK_RSQB)
            || self.is_exact(TOK_RBRACE)
            || self.is_exact(TOK_EQUAL)
            || self.is_exact(TOK_COMMA)
        {
            return Ok(ParsedExpr {
                node: self
                    .classes
                    .make("Yield", &[("value", Value::none())], Some(start)),
                span: start,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }
        let val = self.parse_expr_or_star_tuple(false)?;
        let sp = start.merge(val.span);
        Ok(ParsedExpr {
            node: self.classes.make("Yield", &[("value", val.node)], Some(sp)),
            span: sp,
            parenthesized: false,
            kind: ExprTag::Other,
        })
    }

    fn parse_named_expr(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let expr = self.parse_expr()?;
        if self.is_exact(TOK_COLONEQUAL) {
            if self.feature_minor < 8 {
                return Err(self.err_here("invalid syntax"));
            }
            if expr.kind != ExprTag::Name || expr.parenthesized {
                return Err(self.err_at_span(
                    expr.span,
                    "cannot use assignment expressions with this target",
                ));
            }
            self.bump();
            let rhs = self.parse_expr()?;
            let sp = expr.span.merge(rhs.span);
            let target = self.convert_target_ctx(expr, "Store")?;
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "NamedExpr",
                    &[("target", target), ("value", rhs.node)],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }
        Ok(expr)
    }

    fn parse_expr(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        self.enter_depth()?;
        let res = self.parse_expr_inner();
        self.leave_depth();
        res
    }

    fn parse_expr_inner(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        if self.is_kw("lambda") {
            return self.parse_lambda();
        }
        let body = self.parse_or()?;
        if self.is_kw("if") {
            self.bump();
            let test = self.parse_or()?;
            self.expect_kw("else")?;
            let orelse = self.parse_expr()?;
            let sp = body.span.merge(orelse.span);
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "IfExp",
                    &[
                        ("test", test.node),
                        ("body", body.node),
                        ("orelse", orelse.node),
                    ],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }
        Ok(body)
    }

    fn parse_lambda(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let start = Span::from_tok(&self.bump()); // lambda
        let args = self.parse_parameters(false)?;
        self.expect_exact(TOK_COLON, "':'")?;
        let body = self.parse_expr()?;
        // The lambda ends at the last consumed token, which is the closing `)`
        // when the body is parenthesised (`lambda: (yield)`), not at the
        // parenthesised expression's inner span.
        let sp = start.merge(body.span).merge(Span::from_tok(self.prev()));
        Ok(ParsedExpr {
            node: self
                .classes
                .make("Lambda", &[("args", args), ("body", body.node)], Some(sp)),
            span: sp,
            parenthesized: false,
            kind: ExprTag::Other,
        })
    }

    fn parse_or(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let first = self.parse_and()?;
        if !self.is_kw("or") {
            return Ok(first);
        }
        let start = first.span;
        let mut values = vec![first.node];
        let mut end_sp = start;
        while self.is_kw("or") {
            self.bump();
            let next = self.parse_and()?;
            end_sp = next.span;
            values.push(next.node);
        }
        let sp = start.merge(end_sp);
        Ok(ParsedExpr {
            node: self.classes.make(
                "BoolOp",
                &[
                    ("op", self.classes.singleton("Or")),
                    ("values", Value::list(values)),
                ],
                Some(sp),
            ),
            span: sp,
            parenthesized: false,
            kind: ExprTag::BoolOr,
        })
    }

    fn parse_and(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let first = self.parse_not()?;
        if !self.is_kw("and") {
            return Ok(first);
        }
        let start = first.span;
        let mut values = vec![first.node];
        let mut end_sp = start;
        while self.is_kw("and") {
            self.bump();
            let next = self.parse_not()?;
            end_sp = next.span;
            values.push(next.node);
        }
        let sp = start.merge(end_sp);
        Ok(ParsedExpr {
            node: self.classes.make(
                "BoolOp",
                &[
                    ("op", self.classes.singleton("And")),
                    ("values", Value::list(values)),
                ],
                Some(sp),
            ),
            span: sp,
            parenthesized: false,
            kind: ExprTag::BoolAnd,
        })
    }

    fn parse_not(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        if self.is_kw("not") {
            let start = Span::from_tok(&self.bump());
            let operand = self.parse_not()?;
            let sp = start.merge(operand.span);
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "UnaryOp",
                    &[
                        ("op", self.classes.singleton("Not")),
                        ("operand", operand.node),
                    ],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let left = self.parse_bitor()?;
        let mut ops = Vec::new();
        let mut comparators = Vec::new();
        let mut end_sp = left.span;

        while let Some(op_cls) = self.match_cmp_op() {
            ops.push(self.classes.singleton(op_cls));
            let right = self.parse_bitor()?;
            end_sp = right.span;
            comparators.push(right.node);
        }

        if ops.is_empty() {
            return Ok(left);
        }
        let sp = left.span.merge(end_sp);
        Ok(ParsedExpr {
            node: self.classes.make(
                "Compare",
                &[
                    ("left", left.node),
                    ("ops", Value::list(ops)),
                    ("comparators", Value::list(comparators)),
                ],
                Some(sp),
            ),
            span: sp,
            parenthesized: false,
            kind: ExprTag::Other,
        })
    }

    fn match_cmp_op(&mut self) -> Option<&'static str> {
        match self.cur().exact_type {
            TOK_EQEQUAL => {
                self.bump();
                Some("Eq")
            }
            TOK_NOTEQUAL => {
                self.bump();
                Some("NotEq")
            }
            TOK_LESS => {
                self.bump();
                Some("Lt")
            }
            TOK_LESSEQUAL => {
                self.bump();
                Some("LtE")
            }
            TOK_GREATER => {
                self.bump();
                Some("Gt")
            }
            TOK_GREATEREQUAL => {
                self.bump();
                Some("GtE")
            }
            TOK_NAME => match self.cur().string.as_str() {
                "in" => {
                    self.bump();
                    Some("In")
                }
                "not" if self.peek(1).tok_type == TOK_NAME && self.peek(1).string == "in" => {
                    self.bump();
                    self.bump();
                    Some("NotIn")
                }
                "is" => {
                    self.bump();
                    if self.is_kw("not") {
                        self.bump();
                        Some("IsNot")
                    } else {
                        Some("Is")
                    }
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn parse_bitor(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let mut left = self.parse_bitxor()?;
        while self.is_exact(TOK_VBAR) {
            self.bump();
            let right = self.parse_bitxor()?;
            left = self.make_binop(left, "BitOr", right);
        }
        Ok(left)
    }

    fn parse_bitxor(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let mut left = self.parse_bitand()?;
        while self.is_exact(TOK_CIRCUMFLEX) {
            self.bump();
            let right = self.parse_bitand()?;
            left = self.make_binop(left, "BitXor", right);
        }
        Ok(left)
    }

    fn parse_bitand(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let mut left = self.parse_shift()?;
        while self.is_exact(TOK_AMPER) {
            self.bump();
            let right = self.parse_shift()?;
            left = self.make_binop(left, "BitAnd", right);
        }
        Ok(left)
    }

    fn parse_shift(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let mut left = self.parse_sum()?;
        loop {
            let op = match self.cur().exact_type {
                TOK_LEFTSHIFT => "LShift",
                TOK_RIGHTSHIFT => "RShift",
                _ => break,
            };
            self.bump();
            let right = self.parse_sum()?;
            left = self.make_binop(left, op, right);
        }
        Ok(left)
    }

    fn parse_sum(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let mut left = self.parse_term()?;
        loop {
            let op = match self.cur().exact_type {
                TOK_PLUS => "Add",
                TOK_MINUS => "Sub",
                _ => break,
            };
            self.bump();
            let right = self.parse_term()?;
            left = self.make_binop(left, op, right);
        }
        Ok(left)
    }

    fn parse_term(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let mut left = self.parse_factor()?;
        loop {
            let op = match self.cur().exact_type {
                TOK_STAR => "Mult",
                TOK_AT => "MatMult",
                TOK_SLASH => "Div",
                TOK_DOUBLESLASH => "FloorDiv",
                TOK_PERCENT => "Mod",
                _ => break,
            };
            self.bump();
            let right = self.parse_factor()?;
            left = self.make_binop(left, op, right);
        }
        Ok(left)
    }

    fn parse_factor(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let op = match self.cur().exact_type {
            TOK_PLUS => Some("UAdd"),
            TOK_MINUS => Some("USub"),
            TOK_TILDE => Some("Invert"),
            _ => None,
        };
        if let Some(op_cls) = op {
            let start = Span::from_tok(&self.bump());
            let operand = self.parse_factor()?;
            let sp = start.merge(operand.span);
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "UnaryOp",
                    &[
                        ("op", self.classes.singleton(op_cls)),
                        ("operand", operand.node),
                    ],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }
        self.parse_power()
    }

    fn parse_power(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let left = self.parse_await_primary()?;
        if self.is_exact(TOK_DOUBLESTAR) {
            self.bump();
            let right = self.parse_factor()?;
            return Ok(self.make_binop(left, "Pow", right));
        }
        Ok(left)
    }

    fn make_binop(&self, left: ParsedExpr, op_cls: &str, right: ParsedExpr) -> ParsedExpr {
        let sp = left.span.merge(right.span);
        ParsedExpr {
            node: self.classes.make(
                "BinOp",
                &[
                    ("left", left.node),
                    ("op", self.classes.singleton(op_cls)),
                    ("right", right.node),
                ],
                Some(sp),
            ),
            span: sp,
            parenthesized: false,
            kind: ExprTag::Other,
        }
    }

    fn parse_await_primary(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        if self.is_kw("await") {
            let start = Span::from_tok(&self.bump());
            let val = self.parse_primary()?;
            let sp = start.merge(val.span);
            return Ok(ParsedExpr {
                node: self.classes.make("Await", &[("value", val.node)], Some(sp)),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let mut expr = self.parse_atom()?;
        loop {
            if self.is_exact(TOK_DOT) {
                self.bump();
                let attr = self.expect_ident()?;
                let sp = expr.span.merge(Span::from_tok(&attr));
                expr = ParsedExpr {
                    node: self.classes.make(
                        "Attribute",
                        &[
                            ("value", expr.node),
                            ("attr", Value::string(attr.string)),
                            ("ctx", self.classes.singleton("Load")),
                        ],
                        Some(sp),
                    ),
                    span: sp,
                    parenthesized: false,
                    kind: ExprTag::Attribute,
                };
            } else if self.is_exact(TOK_LPAR) {
                self.bump();
                let (args, keywords) = self.parse_call_arguments()?;
                let rp = self.expect_exact(TOK_RPAR, "')'")?;
                let sp = expr.span.merge(Span::from_tok(&rp));
                expr = ParsedExpr {
                    node: self.classes.make(
                        "Call",
                        &[
                            ("func", expr.node),
                            ("args", Value::list(args)),
                            ("keywords", Value::list(keywords)),
                        ],
                        Some(sp),
                    ),
                    span: sp,
                    parenthesized: false,
                    kind: ExprTag::Other,
                };
            } else if self.is_exact(TOK_LSQB) {
                self.bump();
                let slice = self.parse_slices()?;
                let rb = self.expect_exact(TOK_RSQB, "']'")?;
                let sp = expr.span.merge(Span::from_tok(&rb));
                expr = ParsedExpr {
                    node: self.classes.make(
                        "Subscript",
                        &[
                            ("value", expr.node),
                            ("slice", slice.node),
                            ("ctx", self.classes.singleton("Load")),
                        ],
                        Some(sp),
                    ),
                    span: sp,
                    parenthesized: false,
                    kind: ExprTag::Subscript,
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_call_arguments(&mut self) -> Result<(Vec<Value>, Vec<Value>), AstSyntaxError> {
        let mut args = Vec::new();
        let mut keywords = Vec::new();
        let mut seen_kw = false;
        let mut seen_double_star = false;
        let mut seen_kw_names: HashSet<String> = HashSet::new();

        while !self.is_exact(TOK_RPAR) {
            if self.is_exact(TOK_DOUBLESTAR) {
                seen_double_star = true;
                let start = Span::from_tok(&self.bump());
                let val = self.parse_expr()?;
                let sp = start.merge(val.span);
                keywords.push(self.classes.make(
                    "keyword",
                    &[("arg", Value::none()), ("value", val.node)],
                    Some(sp),
                ));
            } else if self.is_exact(TOK_STAR) {
                if seen_double_star {
                    return Err(self.err_here(
                        "iterable argument unpacking follows keyword argument unpacking",
                    ));
                }
                let start = Span::from_tok(&self.bump());
                let val = self.parse_expr()?;
                let sp = start.merge(val.span);
                args.push(self.classes.make(
                    "Starred",
                    &[("value", val.node), ("ctx", self.classes.singleton("Load"))],
                    Some(sp),
                ));
            } else if self.is_name() && self.peek(1).exact_type == TOK_EQUAL {
                seen_kw = true;
                let name_tok = self.bump();
                let start = Span::from_tok(&name_tok);
                if !seen_kw_names.insert(name_tok.string.clone()) {
                    return Err(self.err_at_span(
                        start,
                        &format!("keyword argument repeated: {}", name_tok.string),
                    ));
                }
                self.bump(); // `=`
                let val = self.parse_expr()?;
                let sp = start.merge(val.span);
                keywords.push(self.classes.make(
                    "keyword",
                    &[("arg", Value::string(name_tok.string)), ("value", val.node)],
                    Some(sp),
                ));
            } else {
                if seen_kw || seen_double_star {
                    return Err(self.err_here("positional argument follows keyword argument"));
                }
                let item = self.parse_named_expr()?;
                if self.is_comp_start() {
                    if !args.is_empty() || !keywords.is_empty() {
                        return Err(self.err_here("Generator expression must be parenthesized"));
                    }
                    let (gens, end_sp) = self.parse_comp_clauses()?;
                    let sp = item.span.merge(end_sp);
                    args.push(self.classes.make(
                        "GeneratorExp",
                        &[("elt", item.node), ("generators", Value::list(gens))],
                        Some(sp),
                    ));
                    if self.is_exact(TOK_COMMA) {
                        return Err(self.err_here("Generator expression must be parenthesized"));
                    }
                    break;
                }
                args.push(item.node);
            }

            if self.is_exact(TOK_COMMA) {
                self.bump();
            } else {
                break;
            }
        }

        Ok((args, keywords))
    }

    fn parse_slices(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let first = self.parse_slice_item()?;
        if !self.is_exact(TOK_COMMA) {
            return Ok(first);
        }
        let start = first.span;
        let mut elts = vec![first.node];
        let mut end_sp = start;
        while self.is_exact(TOK_COMMA) {
            end_sp = Span::from_tok(&self.bump());
            if self.is_exact(TOK_RSQB) {
                break;
            }
            let item = self.parse_slice_item()?;
            end_sp = item.span;
            elts.push(item.node);
        }
        let sp = start.merge(end_sp);
        Ok(ParsedExpr {
            node: self.classes.make(
                "Tuple",
                &[
                    ("elts", Value::list(elts)),
                    ("ctx", self.classes.singleton("Load")),
                ],
                Some(sp),
            ),
            span: sp,
            parenthesized: false,
            kind: ExprTag::Tuple,
        })
    }

    fn parse_slice_item(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let (lower, start_sp) = if self.is_exact(TOK_COLON) {
            let sp = Span::from_tok(self.cur());
            (Value::none(), sp)
        } else {
            let first = self.parse_star_or_expr()?;
            if !self.is_exact(TOK_COLON) {
                return Ok(first);
            }
            // `a[*x:1]` — a starred expression cannot be a slice bound.
            if first.kind == ExprTag::Starred {
                return Err(self.err_at_span(first.span, "invalid syntax"));
            }
            (first.node, first.span)
        };

        // Consume first `:`
        let mut end_sp = Span::from_tok(&self.bump());

        let upper =
            if self.is_exact(TOK_COLON) || self.is_exact(TOK_COMMA) || self.is_exact(TOK_RSQB) {
                Value::none()
            } else {
                let u = self.parse_expr()?;
                end_sp = u.span;
                u.node
            };

        let step = if self.is_exact(TOK_COLON) {
            end_sp = Span::from_tok(&self.bump());
            if self.is_exact(TOK_COMMA) || self.is_exact(TOK_RSQB) {
                Value::none()
            } else {
                let s = self.parse_expr()?;
                end_sp = s.span;
                s.node
            }
        } else {
            Value::none()
        };

        let sp = start_sp.merge(end_sp);
        Ok(ParsedExpr {
            node: self.classes.make(
                "Slice",
                &[("lower", lower), ("upper", upper), ("step", step)],
                Some(sp),
            ),
            span: sp,
            parenthesized: false,
            kind: ExprTag::Other,
        })
    }

    fn parse_atom(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        match self.cur().tok_type {
            TOK_NAME => {
                let tok = self.bump();
                let sp = Span::from_tok(&tok);
                match tok.string.as_str() {
                    "True" => Ok(ParsedExpr {
                        node: self.classes.make(
                            "Constant",
                            &[("value", Value::bool_(true)), ("kind", Value::none())],
                            Some(sp),
                        ),
                        span: sp,
                        parenthesized: false,
                        kind: ExprTag::Other,
                    }),
                    "False" => Ok(ParsedExpr {
                        node: self.classes.make(
                            "Constant",
                            &[("value", Value::bool_(false)), ("kind", Value::none())],
                            Some(sp),
                        ),
                        span: sp,
                        parenthesized: false,
                        kind: ExprTag::Other,
                    }),
                    "None" => Ok(ParsedExpr {
                        node: self.classes.make(
                            "Constant",
                            &[("value", Value::none()), ("kind", Value::none())],
                            Some(sp),
                        ),
                        span: sp,
                        parenthesized: false,
                        kind: ExprTag::Other,
                    }),
                    s if is_hard_keyword(s) => Err(self.err_at_span(sp, "invalid syntax")),
                    _ => Ok(ParsedExpr {
                        node: self.classes.make(
                            "Name",
                            &[
                                ("id", Value::string(tok.string)),
                                ("ctx", self.classes.singleton("Load")),
                            ],
                            Some(sp),
                        ),
                        span: sp,
                        parenthesized: false,
                        kind: ExprTag::Name,
                    }),
                }
            }
            TOK_NUMBER => self.parse_number_constant(),
            TOK_STRING | TOK_FSTRING_START => self.parse_string_or_fstring(),
            _ => match self.cur().exact_type {
                TOK_ELLIPSIS => {
                    let tok = self.bump();
                    let sp = Span::from_tok(&tok);
                    Ok(ParsedExpr {
                        node: self.classes.make(
                            "Constant",
                            &[("value", Value::ellipsis()), ("kind", Value::none())],
                            Some(sp),
                        ),
                        span: sp,
                        parenthesized: false,
                        kind: ExprTag::Other,
                    })
                }
                TOK_LPAR => self.parse_paren_expr(),
                TOK_LSQB => self.parse_list_or_comp(),
                TOK_LBRACE => self.parse_dict_or_set(),
                _ => Err(self.err_here("invalid syntax")),
            },
        }
    }

    fn parse_number_constant(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let tok = self.bump();
        let sp = Span::from_tok(&tok);
        let val = decode_number(&tok.string).map_err(|msg| self.err_at_span(sp, &msg))?;
        Ok(ParsedExpr {
            node: self.classes.make(
                "Constant",
                &[("value", val), ("kind", Value::none())],
                Some(sp),
            ),
            span: sp,
            parenthesized: false,
            kind: ExprTag::Other,
        })
    }

    // ── Strings & PEP 701 F-Strings ───────────────────────────────────────

    fn parse_string_or_fstring(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let start_sp = Span::from_tok(self.cur());
        let mut end_sp = start_sp;
        let mut saw_fstring = false;
        let mut saw_bytes = false;
        let mut saw_str = false;
        let mut first_kind: Option<&'static str> = None;
        let mut first_piece = true;

        // Collect pieces: either static str/bytes or JoinedStr sub-nodes.
        let mut bytes_buf: Vec<u8> = Vec::new();
        let mut fstring_parts: Vec<(Value, Span, Option<String>)> = Vec::new();

        while matches!(self.cur().tok_type, TOK_STRING | TOK_FSTRING_START) {
            if self.cur().tok_type == TOK_STRING {
                let tok = self.bump();
                let tok_sp = Span::from_tok(&tok);
                end_sp = tok_sp;
                match decode_string_token(&tok.string)
                    .map_err(|msg| self.err_at_span(tok_sp, &msg))?
                {
                    DecodedString::Bytes(b) => {
                        if saw_str || saw_fstring {
                            return Err(
                                self.err_at_span(tok_sp, "cannot mix bytes and nonbytes literals")
                            );
                        }
                        saw_bytes = true;
                        bytes_buf.extend(b);
                    }
                    DecodedString::Str { value, kind } => {
                        if saw_bytes {
                            return Err(
                                self.err_at_span(tok_sp, "cannot mix bytes and nonbytes literals")
                            );
                        }
                        saw_str = true;
                        if first_piece {
                            first_kind = kind;
                        }
                        fstring_parts.push((Value::none(), tok_sp, Some(value)));
                    }
                }
            } else {
                if saw_bytes {
                    return Err(self.err_here("cannot mix bytes and nonbytes literals"));
                }
                saw_fstring = true;
                let (parts, fs_end) = self.parse_single_fstring()?;
                end_sp = fs_end;
                fstring_parts.extend(parts);
            }
            first_piece = false;
        }

        let sp = start_sp.merge(end_sp);

        if saw_bytes {
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "Constant",
                    &[("value", Value::bytes(bytes_buf)), ("kind", Value::none())],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }

        if !saw_fstring {
            let mut combined = String::new();
            for (_, _, text) in fstring_parts {
                if let Some(s) = text {
                    combined.push_str(&s);
                }
            }
            let kind_val = match first_kind {
                Some(k) => Value::string(k),
                None => Value::none(),
            };
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "Constant",
                    &[("value", Value::string(combined)), ("kind", kind_val)],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }

        // Merge adjacent literal fragments into single Constant(str) nodes inside JoinedStr.
        let mut values: Vec<Value> = Vec::new();
        let mut pending_str: Option<(String, Span)> = None;
        for (node, p_sp, maybe_str) in fstring_parts {
            if let Some(s) = maybe_str {
                if !s.is_empty() {
                    if let Some((ref mut buf, ref mut span)) = pending_str {
                        buf.push_str(&s);
                        *span = span.merge(p_sp);
                    } else {
                        pending_str = Some((s, p_sp));
                    }
                }
            } else {
                if let Some((buf, span)) = pending_str.take() {
                    values.push(self.classes.make(
                        "Constant",
                        &[("value", Value::string(buf)), ("kind", Value::none())],
                        Some(span),
                    ));
                }
                values.push(node);
            }
        }
        if let Some((buf, span)) = pending_str.take() {
            values.push(self.classes.make(
                "Constant",
                &[("value", Value::string(buf)), ("kind", Value::none())],
                Some(span),
            ));
        }

        Ok(ParsedExpr {
            node: self
                .classes
                .make("JoinedStr", &[("values", Value::list(values))], Some(sp)),
            span: sp,
            parenthesized: false,
            kind: ExprTag::Other,
        })
    }

    /// Parse a single `FSTRING_START ... FSTRING_END` token sequence.
    fn parse_single_fstring(&mut self) -> Result<(Vec<FStringPartItem>, Span), AstSyntaxError> {
        let start_tok = self.expect_exact(TOK_FSTRING_START, "f-string start")?;
        let is_raw = start_tok.string.contains('r') || start_tok.string.contains('R');
        let mut parts: Vec<FStringPartItem> = Vec::new();

        while !self.is_exact(TOK_FSTRING_END) {
            if self.at_eof() {
                return Err(self.err_here("unterminated f-string literal"));
            }
            if self.is_exact(TOK_FSTRING_MIDDLE) {
                let tok = self.bump();
                let sp = Span::from_tok(&tok);
                let decoded = decode_fstring_middle(&tok.string, is_raw)
                    .map_err(|msg| self.err_at_span(sp, &msg))?;
                parts.push((Value::none(), sp, Some(decoded)));
            } else if self.is_exact(TOK_LBRACE) {
                let (field_parts, _) = self.parse_fstring_field(is_raw)?;
                parts.extend(field_parts);
            } else if self.is_exact(TOK_RBRACE) {
                return Err(self.err_here("f-string: single '}' is not allowed"));
            } else {
                return Err(self.err_here("invalid syntax in f-string"));
            }
        }
        let end_tok = self.expect_exact(TOK_FSTRING_END, "f-string end")?;
        Ok((parts, Span::from_tok(&end_tok)))
    }

    fn parse_fstring_field(
        &mut self,
        is_raw: bool,
    ) -> Result<(Vec<FStringPartItem>, Span), AstSyntaxError> {
        let lbrace = self.expect_exact(TOK_LBRACE, "'{'")?;
        let field_start = Span::from_tok(&lbrace);

        if self.is_exact(TOK_RBRACE) {
            return Err(self.err_here("f-string: valid expression required before '}'"));
        }

        // Parse the inner expression (can be yield, star-tuple, or named expr).
        let expr = self.parse_yield_or_star_tuple()?;

        // Check for Python 3.8 debug `=` specifier (e.g. `f"{x=}"`, `f"{x = !s}"`).
        let mut debug_text: Option<(String, Span)> = None;
        if self.is_exact(TOK_EQUAL) {
            let eq_tok = self.bump();
            let eq_sp = Span::from_tok(&eq_tok);
            // Extract exact verbatim source slice from right after `{` through `=`
            // (and any whitespace between `=` and `!`/`:`/`}`!).
            let next_tok = self.cur();
            let raw_debug = self.extract_source_slice(
                lbrace.end_line,
                lbrace.end_byte_col,
                next_tok.start_line,
                next_tok.start_byte_col,
            );
            debug_text = Some((raw_debug, field_start.merge(eq_sp)));
        }

        // Optional conversion: `!r`, `!s`, `!a`.
        let mut explicit_conversion: Option<i64> = None;
        if self.is_exact(TOK_EXCLAMATION) {
            self.bump();
            let conv_tok = self.expect_ident()?;
            let code = match conv_tok.string.as_str() {
                "s" => 115,
                "r" => 114,
                "a" => 97,
                other => {
                    return Err(self.err_at_span(
                        Span::from_tok(&conv_tok),
                        &format!("f-string: invalid conversion character '{other}': expected 's', 'r', or 'a'"),
                    ));
                }
            };
            explicit_conversion = Some(code);
        }

        // Optional format spec after `:`.
        let mut format_spec = Value::none();
        let mut has_format_spec = false;
        if self.is_exact(TOK_COLON) {
            has_format_spec = true;
            let colon_sp = Span::from_tok(&self.bump());
            let mut spec_parts: Vec<(Value, Span, Option<String>)> = Vec::new();
            let mut spec_end = colon_sp;
            while !self.is_exact(TOK_RBRACE) {
                if self.at_eof() {
                    return Err(self.err_here("unterminated f-string format spec"));
                }
                if self.is_exact(TOK_FSTRING_MIDDLE) {
                    let tok = self.bump();
                    let sp = Span::from_tok(&tok);
                    spec_end = sp;
                    let decoded = decode_fstring_middle(&tok.string, is_raw)
                        .map_err(|msg| self.err_at_span(sp, &msg))?;
                    spec_parts.push((Value::none(), sp, Some(decoded)));
                } else if self.is_exact(TOK_LBRACE) {
                    let (nested, n_sp) = self.parse_fstring_field(is_raw)?;
                    spec_end = n_sp;
                    spec_parts.extend(nested);
                } else {
                    return Err(self.err_here("invalid syntax in format specifier"));
                }
            }
            let mut spec_values: Vec<Value> = Vec::new();
            for (node, p_sp, maybe_str) in spec_parts {
                if let Some(s) = maybe_str {
                    if !s.is_empty() {
                        spec_values.push(self.classes.make(
                            "Constant",
                            &[("value", Value::string(s)), ("kind", Value::none())],
                            Some(p_sp),
                        ));
                    }
                } else {
                    spec_values.push(node);
                }
            }
            let spec_sp = colon_sp.merge(spec_end);
            format_spec = self.classes.make(
                "JoinedStr",
                &[("values", Value::list(spec_values))],
                Some(spec_sp),
            );
        }

        let rbrace = self.expect_exact(TOK_RBRACE, "'}'")?;
        let field_sp = field_start.merge(Span::from_tok(&rbrace));

        // CPython 3.12 debug conversion rule:
        // - If explicit `!s`/`!r`/`!a` -> use it.
        // - Else if debug `=` AND no `:format_spec` -> default to `'r'` (114).
        // - Else -> `-1`.
        let conversion = match explicit_conversion {
            Some(c) => c,
            None if debug_text.is_some() && !has_format_spec => 114,
            None => -1,
        };

        let fv = self.classes.make(
            "FormattedValue",
            &[
                ("value", expr.node),
                ("conversion", Value::int(conversion)),
                ("format_spec", format_spec),
            ],
            Some(field_sp),
        );

        let mut result = Vec::new();
        if let Some((dbg, dbg_sp)) = debug_text {
            result.push((Value::none(), dbg_sp, Some(dbg)));
        }
        result.push((fv, field_sp, None));
        Ok((result, field_sp))
    }

    fn extract_source_slice(
        &self,
        start_line: u32,
        start_byte_col: u32,
        end_line: u32,
        end_byte_col: u32,
    ) -> String {
        let mut out = String::new();
        let bytes = self.source.as_bytes();
        let mut line = 1u32;
        let mut line_start = 0usize;
        let mut i = 0usize;
        let mut start_abs: Option<usize> = None;
        let mut end_abs: Option<usize> = None;

        while i <= bytes.len() {
            if line == start_line && start_abs.is_none() {
                start_abs = Some((line_start + start_byte_col as usize).min(bytes.len()));
            }
            if line == end_line && end_abs.is_none() {
                end_abs = Some((line_start + end_byte_col as usize).min(bytes.len()));
            }
            if start_abs.is_some() && end_abs.is_some() {
                break;
            }
            if i == bytes.len() {
                break;
            }
            if bytes[i] == b'\n' {
                line += 1;
                line_start = i + 1;
                i += 1;
            } else if bytes[i] == b'\r' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    line += 1;
                    line_start = i + 2;
                    i += 2;
                } else {
                    line += 1;
                    line_start = i + 1;
                    i += 1;
                }
            } else {
                i += 1;
            }
        }

        if let (Some(s), Some(e)) = (start_abs, end_abs)
            && s <= e
            && let Some(slice) = self.source.get(s..e)
        {
            out.push_str(slice);
        }
        out
    }

    // ── Parenthesized, List, Set, Dict, Comprehensions ────────────────────

    fn parse_paren_expr(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let start = Span::from_tok(&self.bump()); // `(`
        if self.is_exact(TOK_RPAR) {
            let end = Span::from_tok(&self.bump());
            let sp = start.merge(end);
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "Tuple",
                    &[
                        ("elts", Value::list(vec![])),
                        ("ctx", self.classes.singleton("Load")),
                    ],
                    Some(sp),
                ),
                span: sp,
                parenthesized: true,
                kind: ExprTag::Tuple,
            });
        }

        if self.is_kw("yield") {
            let mut y = self.parse_yield_expr()?;
            self.expect_exact(TOK_RPAR, "')'")?;
            y.parenthesized = true;
            return Ok(y);
        }

        let first = self.parse_star_or_expr()?;

        // Generator expression: `(elt for ...)`
        if self.is_comp_start() {
            if first.kind == ExprTag::Starred {
                return Err(self.err_at_span(
                    first.span,
                    "iterable unpacking cannot be used in comprehension",
                ));
            }
            let (gens, _) = self.parse_comp_clauses()?;
            let end = Span::from_tok(&self.expect_exact(TOK_RPAR, "')'")?);
            let sp = start.merge(end);
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "GeneratorExp",
                    &[("elt", first.node), ("generators", Value::list(gens))],
                    Some(sp),
                ),
                span: sp,
                parenthesized: true,
                kind: ExprTag::Other,
            });
        }

        // Tuple: `(a, ...)`
        if self.is_exact(TOK_COMMA) {
            let mut elts = vec![first.node];
            while self.is_exact(TOK_COMMA) {
                self.bump();
                if self.is_exact(TOK_RPAR) {
                    break;
                }
                elts.push(self.parse_star_or_expr()?.node);
            }
            let end = Span::from_tok(&self.expect_exact(TOK_RPAR, "')'")?);
            let sp = start.merge(end);
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "Tuple",
                    &[
                        ("elts", Value::list(elts)),
                        ("ctx", self.classes.singleton("Load")),
                    ],
                    Some(sp),
                ),
                span: sp,
                parenthesized: true,
                kind: ExprTag::Tuple,
            });
        }

        if first.kind == ExprTag::Starred {
            return Err(self.err_at_span(first.span, "can't use starred expression here"));
        }

        self.expect_exact(TOK_RPAR, "')'")?;
        Ok(ParsedExpr {
            node: first.node,
            span: first.span,
            parenthesized: true,
            kind: first.kind,
        })
    }

    fn parse_list_or_comp(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let start = Span::from_tok(&self.bump()); // `[`
        if self.is_exact(TOK_RSQB) {
            let end = Span::from_tok(&self.bump());
            let sp = start.merge(end);
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "List",
                    &[
                        ("elts", Value::list(vec![])),
                        ("ctx", self.classes.singleton("Load")),
                    ],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::List,
            });
        }

        let first = self.parse_star_or_expr()?;
        if self.is_comp_start() {
            if first.kind == ExprTag::Starred {
                return Err(self.err_at_span(
                    first.span,
                    "iterable unpacking cannot be used in comprehension",
                ));
            }
            let (gens, _) = self.parse_comp_clauses()?;
            let end = Span::from_tok(&self.expect_exact(TOK_RSQB, "']'")?);
            let sp = start.merge(end);
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "ListComp",
                    &[("elt", first.node), ("generators", Value::list(gens))],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }

        let mut elts = vec![first.node];
        while self.is_exact(TOK_COMMA) {
            self.bump();
            if self.is_exact(TOK_RSQB) {
                break;
            }
            elts.push(self.parse_star_or_expr()?.node);
        }
        let end = Span::from_tok(&self.expect_exact(TOK_RSQB, "']'")?);
        let sp = start.merge(end);
        Ok(ParsedExpr {
            node: self.classes.make(
                "List",
                &[
                    ("elts", Value::list(elts)),
                    ("ctx", self.classes.singleton("Load")),
                ],
                Some(sp),
            ),
            span: sp,
            parenthesized: false,
            kind: ExprTag::List,
        })
    }

    fn parse_dict_or_set(&mut self) -> Result<ParsedExpr, AstSyntaxError> {
        let start = Span::from_tok(&self.bump()); // `{`
        if self.is_exact(TOK_RBRACE) {
            let end = Span::from_tok(&self.bump());
            let sp = start.merge(end);
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "Dict",
                    &[
                        ("keys", Value::list(vec![])),
                        ("values", Value::list(vec![])),
                    ],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }

        // Starts with `**expr` -> Dict literal.
        if self.is_exact(TOK_DOUBLESTAR) {
            self.bump();
            let val = self.parse_bitor()?;
            let mut keys = vec![Value::none()];
            let mut values = vec![val.node];
            while self.is_exact(TOK_COMMA) {
                self.bump();
                if self.is_exact(TOK_RBRACE) {
                    break;
                }
                if self.is_exact(TOK_DOUBLESTAR) {
                    self.bump();
                    keys.push(Value::none());
                    values.push(self.parse_bitor()?.node);
                } else {
                    let k = self.parse_expr()?;
                    self.expect_exact(TOK_COLON, "':'")?;
                    let v = self.parse_expr()?;
                    keys.push(k.node);
                    values.push(v.node);
                }
            }
            let end = Span::from_tok(&self.expect_exact(TOK_RBRACE, "'}'")?);
            let sp = start.merge(end);
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "Dict",
                    &[("keys", Value::list(keys)), ("values", Value::list(values))],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }

        let first = self.parse_star_or_expr()?;

        // Dict or DictComp (`key: value`)
        if self.is_exact(TOK_COLON) {
            if first.kind == ExprTag::Starred {
                return Err(self.err_at_span(first.span, "invalid syntax"));
            }
            self.bump();
            let first_val = self.parse_expr()?;
            if self.is_comp_start() {
                let (gens, _) = self.parse_comp_clauses()?;
                let end = Span::from_tok(&self.expect_exact(TOK_RBRACE, "'}'")?);
                let sp = start.merge(end);
                return Ok(ParsedExpr {
                    node: self.classes.make(
                        "DictComp",
                        &[
                            ("key", first.node),
                            ("value", first_val.node),
                            ("generators", Value::list(gens)),
                        ],
                        Some(sp),
                    ),
                    span: sp,
                    parenthesized: false,
                    kind: ExprTag::Other,
                });
            }
            let mut keys = vec![first.node];
            let mut values = vec![first_val.node];
            while self.is_exact(TOK_COMMA) {
                self.bump();
                if self.is_exact(TOK_RBRACE) {
                    break;
                }
                if self.is_exact(TOK_DOUBLESTAR) {
                    self.bump();
                    keys.push(Value::none());
                    values.push(self.parse_bitor()?.node);
                } else {
                    let k = self.parse_expr()?;
                    self.expect_exact(TOK_COLON, "':'")?;
                    let v = self.parse_expr()?;
                    keys.push(k.node);
                    values.push(v.node);
                }
            }
            let end = Span::from_tok(&self.expect_exact(TOK_RBRACE, "'}'")?);
            let sp = start.merge(end);
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "Dict",
                    &[("keys", Value::list(keys)), ("values", Value::list(values))],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }

        // SetComp
        if self.is_comp_start() {
            if first.kind == ExprTag::Starred {
                return Err(self.err_at_span(
                    first.span,
                    "iterable unpacking cannot be used in comprehension",
                ));
            }
            let (gens, _) = self.parse_comp_clauses()?;
            let end = Span::from_tok(&self.expect_exact(TOK_RBRACE, "'}'")?);
            let sp = start.merge(end);
            return Ok(ParsedExpr {
                node: self.classes.make(
                    "SetComp",
                    &[("elt", first.node), ("generators", Value::list(gens))],
                    Some(sp),
                ),
                span: sp,
                parenthesized: false,
                kind: ExprTag::Other,
            });
        }

        // Set literal
        let mut elts = vec![first.node];
        while self.is_exact(TOK_COMMA) {
            self.bump();
            if self.is_exact(TOK_RBRACE) {
                break;
            }
            elts.push(self.parse_star_or_expr()?.node);
        }
        let end = Span::from_tok(&self.expect_exact(TOK_RBRACE, "'}'")?);
        let sp = start.merge(end);
        Ok(ParsedExpr {
            node: self
                .classes
                .make("Set", &[("elts", Value::list(elts))], Some(sp)),
            span: sp,
            parenthesized: false,
            kind: ExprTag::Other,
        })
    }

    fn is_comp_start(&self) -> bool {
        self.is_kw("for")
            || (self.is_kw("async")
                && self.peek(1).tok_type == TOK_NAME
                && self.peek(1).string == "for")
    }

    fn parse_comp_clauses(&mut self) -> Result<(Vec<Value>, Span), AstSyntaxError> {
        let mut gens = Vec::new();
        let mut end_sp = Span::from_tok(self.cur());
        while self.is_comp_start() {
            let is_async = if self.is_kw("async") {
                self.bump();
                1i64
            } else {
                0i64
            };
            self.expect_kw("for")?;
            let target_expr = self.parse_for_targets()?;
            let target = self.convert_target_ctx(target_expr, "Store")?;
            self.expect_kw("in")?;
            let iter = self.parse_or()?;
            end_sp = iter.span;
            let mut ifs = Vec::new();
            while self.is_kw("if") {
                self.bump();
                let cond = self.parse_or()?;
                end_sp = cond.span;
                ifs.push(cond.node);
            }
            gens.push(self.classes.make(
                "comprehension",
                &[
                    ("target", target),
                    ("iter", iter.node),
                    ("ifs", Value::list(ifs)),
                    ("is_async", Value::int(is_async)),
                ],
                None,
            ));
        }
        Ok((gens, end_sp))
    }

    // ── Context conversion (`Load` -> `Store` / `Del`) ────────────────────

    fn convert_target_ctx(
        &self,
        expr: ParsedExpr,
        ctx_name: &str,
    ) -> Result<Value, AstSyntaxError> {
        self.set_node_ctx(&expr.node, ctx_name, expr.span)?;
        Ok(expr.node)
    }

    fn set_node_ctx(&self, node: &Value, ctx_name: &str, span: Span) -> Result<(), AstSyntaxError> {
        let ValueKind::PyInstance(inst_rc) = node.kind() else {
            return Err(self.err_at_span(span, "cannot assign to expression"));
        };
        let cls_name = inst_rc.borrow().class.borrow().name.clone();
        match cls_name.as_str() {
            "Name" | "Attribute" | "Subscript" => {
                inst_rc
                    .borrow_mut()
                    .attrs
                    .insert("ctx", self.classes.singleton(ctx_name));
                Ok(())
            }
            "Starred" => {
                if ctx_name == "Del" {
                    return Err(self.err_at_span(span, "cannot delete starred"));
                }
                let inner = inst_rc.borrow().attrs.get("value").cloned();
                inst_rc
                    .borrow_mut()
                    .attrs
                    .insert("ctx", self.classes.singleton(ctx_name));
                if let Some(inner_val) = inner {
                    self.set_node_ctx(&inner_val, ctx_name, span)?;
                }
                Ok(())
            }
            "Tuple" | "List" => {
                let elts = inst_rc.borrow().attrs.get("elts").cloned();
                inst_rc
                    .borrow_mut()
                    .attrs
                    .insert("ctx", self.classes.singleton(ctx_name));
                if let Some(elts_val) = elts
                    && let ValueKind::List(list_ref) = elts_val.kind()
                {
                    let items: Vec<Value> = list_ref.clone();
                    let mut star_count = 0usize;
                    for item in &items {
                        if let ValueKind::PyInstance(ir) = item.kind()
                            && ir.borrow().class.borrow().name == "Starred"
                        {
                            star_count += 1;
                            if star_count > 1 {
                                return Err(self.err_at_span(
                                    span,
                                    "multiple starred expressions in assignment",
                                ));
                            }
                        }
                        self.set_node_ctx(item, ctx_name, span)?;
                    }
                }
                Ok(())
            }
            _ => {
                let msg = if ctx_name == "Del" {
                    "cannot delete expression"
                } else {
                    "cannot assign to expression"
                };
                Err(self.err_at_span(span, msg))
            }
        }
    }
}

fn augassign_op_class(exact: i64) -> Option<&'static str> {
    match exact {
        TOK_PLUSEQUAL => Some("Add"),
        TOK_MINEQUAL => Some("Sub"),
        TOK_STAREQUAL => Some("Mult"),
        TOK_ATEQUAL => Some("MatMult"),
        TOK_SLASHEQUAL => Some("Div"),
        TOK_PERCENTEQUAL => Some("Mod"),
        TOK_DOUBLESTAREQUAL => Some("Pow"),
        TOK_LEFTSHIFTEQUAL => Some("LShift"),
        TOK_RIGHTSHIFTEQUAL => Some("RShift"),
        TOK_VBAREQUAL => Some("BitOr"),
        TOK_CIRCUMFLEXEQUAL => Some("BitXor"),
        TOK_AMPEREQUAL => Some("BitAnd"),
        TOK_DOUBLESLASHEQUAL => Some("FloorDiv"),
        _ => None,
    }
}
