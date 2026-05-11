#[derive(Debug, Clone)]
pub struct FunctionParam {
    pub name: String,
    pub default: Option<Expr>,
    pub is_args: bool,         // *args
    pub is_kwargs: bool,       // **kwargs
    pub is_keyword_only: bool, // declared after * or *args
}

#[derive(Debug, Clone)]
pub struct CallArg {
    pub name: Option<String>,
    pub value: Expr,
    pub splat: bool,        // *expr
    pub double_splat: bool, // **expr
}

#[derive(Debug, Clone)]
pub struct ExceptHandler {
    pub kind: Option<Expr>,
    pub name: Option<String>,
    pub body: Vec<Stmt>,
}

/// Assignment target (left-hand side of = or augmented =)
#[derive(Debug, Clone)]
pub enum AssignTarget {
    Name(String),
    Attr(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    /// Unpack: a, b, c = ...
    Tuple(Vec<AssignTarget>),
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Assign(AssignTarget, Expr),
    AttrAssign {
        target: Expr,
        name: String,
        expr: Expr,
    },
    IndexAssign {
        target: Box<Expr>,
        index: Box<Expr>,
        expr: Expr,
    },
    SliceAssign {
        target: Box<Expr>,
        lower: Option<Box<Expr>>,
        upper: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
        expr: Expr,
    },
    AugAssign {
        target: AssignTarget,
        op: BinaryOp,
        expr: Expr,
    },
    Expr(Expr),
    Def {
        name: String,
        params: Vec<FunctionParam>,
        body: Vec<Stmt>,
        decorators: Vec<Expr>,
    },
    Class {
        name: String,
        bases: Vec<Expr>,
        body: Vec<Stmt>,
        decorators: Vec<Expr>,
    },
    Global(Vec<String>),
    Nonlocal(Vec<String>),
    If {
        branches: Vec<(Expr, Vec<Stmt>)>,
        else_branch: Option<Vec<Stmt>>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
        else_branch: Option<Vec<Stmt>>,
    },
    For {
        target: AssignTarget,
        iter: Expr,
        body: Vec<Stmt>,
        else_branch: Option<Vec<Stmt>>,
    },
    Try {
        body: Vec<Stmt>,
        handlers: Vec<ExceptHandler>,
        else_branch: Option<Vec<Stmt>>,
        finally_branch: Option<Vec<Stmt>>,
    },
    Raise {
        expr: Option<Expr>,
        cause: Option<Expr>,
    },
    Import {
        names: Vec<(String, Option<String>)>,
    },
    ImportFrom {
        module: String,
        names: Vec<(String, Option<String>)>,
    },
    Return(Option<Expr>),
    Break,
    Continue,
    Pass,
    Delete(Vec<Expr>),
    Assert {
        test: Expr,
        msg: Option<Expr>,
    },
    With {
        items: Vec<(Expr, Option<AssignTarget>)>,
        body: Vec<Stmt>,
    },
}

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    None,
    Var(String),
    List(Vec<Expr>),
    Tuple(Vec<Expr>),
    Dict(Vec<(Expr, Expr)>),
    Set(Vec<Expr>),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    /// Chained: a < b < c  →  Compare { left: a, ops: [(Lt, b), (Lt, c)] }
    Compare {
        left: Box<Expr>,
        ops: Vec<(CmpOp, Expr)>,
    },
    Ternary {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
    },
    Lambda {
        params: Vec<String>,
        body: Box<Expr>,
    },
    Call {
        func: Box<Expr>,
        args: Vec<CallArg>,
    },
    Attr {
        target: Box<Expr>,
        name: String,
    },
    Index {
        target: Box<Expr>,
        index: Box<Expr>,
    },
    Slice {
        target: Box<Expr>,
        lower: Option<Box<Expr>>,
        upper: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
    Pos,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    MatMul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    BitAnd,
    BitOr,
    BitXor,
    LShift,
    RShift,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    In,
    NotIn,
    Is,
    IsNot,
}

/// Comparison operators (used in chained comparisons)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    NotIn,
    Is,
    IsNot,
}
