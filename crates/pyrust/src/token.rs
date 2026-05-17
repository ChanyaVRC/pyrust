/// One segment of an f-string literal.
#[derive(Debug, Clone, PartialEq)]
pub enum FStringPart {
    /// Plain text fragment (already escape-processed).
    Literal(String),
    /// Raw source text for the expression, plus optional conversion flag and format spec.
    /// `format_spec`, when present, is itself a list of f-string parts so that
    /// nested `{expr}` interpolations inside the spec (e.g. `f"{x:>{w}}"`) can
    /// be evaluated and their string values substituted into the spec.
    ///
    /// `debug_text`, when `Some`, holds the verbatim source text of the
    /// expression (including any surrounding whitespace and a trailing `=`)
    /// for the Python 3.8 debug form `f"{x=}"`.  At compile time, this text
    /// is emitted as a literal prefix before the formatted value, and the
    /// default conversion becomes `repr` (unless an explicit conversion
    /// flag or format spec overrides it).
    Expr {
        src: String,
        conversion: Option<char>,
        format_spec: Option<Vec<FStringPart>>,
        debug_text: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Int(i64),
    /// Integer literal that does not fit in i64; stored as a decimal string.
    BigInt(String),
    Float(f64),
    /// Imaginary literal, e.g. `3j` or `2.5j` (the f64 is the imaginary part)
    Imag(f64),
    Str(String),
    Bytes(Vec<u8>),
    FString(Vec<FStringPart>),
    Ident(String),
    // Arithmetic
    Plus,
    Minus,
    Star,
    StarStar,
    Slash,
    SlashSlash,
    Percent,
    At,
    // Augmented assignment
    PlusAssign,
    MinusAssign,
    StarAssign,
    StarStarAssign,
    SlashAssign,
    SlashSlashAssign,
    PercentAssign,
    AtAssign,
    AmpersandAssign,
    PipeAssign,
    CaretAssign,
    LShiftAssign,
    RShiftAssign,
    // Bitwise
    Ampersand,
    Pipe,
    Caret,
    Tilde,
    LShift,
    RShift,
    // Delimiters
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Walrus, // :=
    Semicolon,
    Dot,
    Arrow, // ->
    // Comparison / assignment
    Assign,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    // Keywords
    And,
    Or,
    Not,
    If,
    Elif,
    Else,
    While,
    For,
    In,
    Def,
    Class,
    With,
    Try,
    Except,
    Finally,
    Raise,
    As,
    Import,
    From,
    Global,
    Nonlocal,
    Return,
    Yield,
    Break,
    Continue,
    Pass,
    Del,
    Assert,
    Lambda,
    Is,
    True,
    False,
    None,
    // Layout
    Newline,
    Indent,
    Dedent,
    Eof,
}
