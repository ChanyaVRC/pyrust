use crate::ast::{
    AssignTarget, BinaryOp, CallArg, CmpOp, CompClause, ExceptHandler, Expr, FunctionParam, Stmt,
    UnaryOp,
};
use crate::error::{PyError, Result};
use crate::token::Token;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    pub fn parse_program(&mut self) -> Result<Vec<Stmt>> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !self.is(&Token::Eof) {
            stmts.extend(self.parse_stmt_sequence()?);
            self.skip_newlines();
        }
        Ok(stmts)
    }

    fn parse_stmt_sequence(&mut self) -> Result<Vec<Stmt>> {
        let mut stmts = Vec::new();
        stmts.push(self.parse_stmt()?);
        while self.is(&Token::Semicolon) {
            self.bump();
            if self.at_stmt_end() {
                break;
            }
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    // ── Statements ─────────────────────────────────────────────────────────

    fn parse_stmt(&mut self) -> Result<Stmt> {
        // Collect decorators before def/class
        let mut decorators: Vec<Expr> = Vec::new();
        while self.is(&Token::At) {
            decorators.push(self.parse_decorator()?);
            self.skip_newlines();
        }

        match self.current() {
            Some(Token::Def) => self.parse_def(decorators),
            Some(Token::Class) => self.parse_class(decorators),
            _ if !decorators.is_empty() => Err(PyError::Parse(
                "decorator must be followed by def or class".to_string(),
            )),
            Some(Token::Global) => self.parse_global(),
            Some(Token::Nonlocal) => self.parse_nonlocal(),
            Some(Token::If) => self.parse_if(),
            Some(Token::While) => self.parse_while(),
            Some(Token::For) => self.parse_for(),
            Some(Token::Try) => self.parse_try(),
            Some(Token::Raise) => self.parse_raise(),
            Some(Token::Import) => self.parse_import(),
            Some(Token::From) => self.parse_import_from(),
            Some(Token::Del) => self.parse_del(),
            Some(Token::Assert) => self.parse_assert(),
            Some(Token::With) => self.parse_with(),
            Some(Token::Return) => {
                self.bump();
                if self.at_stmt_end() {
                    Ok(Stmt::Return(None))
                } else {
                    Ok(Stmt::Return(Some(self.parse_expr()?)))
                }
            }
            Some(Token::Break) => {
                self.bump();
                Ok(Stmt::Break)
            }
            Some(Token::Continue) => {
                self.bump();
                Ok(Stmt::Continue)
            }
            Some(Token::Pass) => {
                self.bump();
                Ok(Stmt::Pass)
            }
            _ => self.parse_expr_stmt(),
        }
    }

    fn parse_decorator(&mut self) -> Result<Expr> {
        self.expect(&Token::At)?;
        let expr = self.parse_postfix()?;
        // skip newline after decorator
        if self.is(&Token::Newline) {
            self.bump();
        }
        Ok(expr)
    }

    fn parse_expr_stmt(&mut self) -> Result<Stmt> {
        // Parse a comma-separated list (possible tuple or unpack target)
        let first = self.parse_expr()?;

        // Check for more comma-separated items (tuple or unpack target)
        let targets = if self.is(&Token::Comma) {
            let mut items = vec![first];
            while self.is(&Token::Comma) {
                self.bump();
                if self.at_stmt_end() {
                    break;
                }
                items.push(self.parse_expr()?);
            }
            items
        } else {
            vec![first]
        };

        // Assignment: = rhs
        if self.is(&Token::Assign) {
            self.bump();
            // Parse the RHS; collect commas so "a, b = 1, 2" gives rhs=Tuple([1,2])
            let rhs_first = self.parse_expr()?;
            let rhs = if self.is(&Token::Comma) {
                let mut items = vec![rhs_first];
                while self.is(&Token::Comma) {
                    self.bump();
                    if self.at_stmt_end() {
                        break;
                    }
                    items.push(self.parse_expr()?);
                }
                Expr::Tuple(items)
            } else {
                rhs_first
            };
            if targets.len() == 1 {
                return match &targets[0] {
                    Expr::Var(name) => Ok(Stmt::Assign(AssignTarget::Name(name.clone()), rhs)),
                    Expr::Attr { target, name } => Ok(Stmt::AttrAssign {
                        target: *target.clone(),
                        name: name.clone(),
                        expr: rhs,
                    }),
                    Expr::Index { target, index } => Ok(Stmt::IndexAssign {
                        target: target.clone(),
                        index: index.clone(),
                        expr: rhs,
                    }),
                    Expr::Slice {
                        target,
                        lower,
                        upper,
                        step,
                    } => Ok(Stmt::SliceAssign {
                        target: target.clone(),
                        lower: lower.clone(),
                        upper: upper.clone(),
                        step: step.clone(),
                        expr: rhs,
                    }),
                    Expr::Tuple(elems) => {
                        let targets: Result<Vec<AssignTarget>> =
                            elems.iter().map(|e| expr_to_assign_target(e)).collect();
                        Ok(Stmt::Assign(AssignTarget::Tuple(targets?), rhs))
                    }
                    _ => Err(PyError::Parse(
                        "cannot assign to this expression".to_string(),
                    )),
                };
            }
            // Multi-target: tuple unpack
            let assign_targets: Result<Vec<AssignTarget>> =
                targets.iter().map(|e| expr_to_assign_target(e)).collect();
            return Ok(Stmt::Assign(AssignTarget::Tuple(assign_targets?), rhs));
        }

        // Augmented assignment: += -= etc.
        if let Some(aug_op) = self.current_aug_op() {
            self.bump();
            let rhs = self.parse_expr()?;
            let target = if targets.len() == 1 {
                expr_to_assign_target(&targets[0])?
            } else {
                return Err(PyError::Parse(
                    "invalid augmented assignment target".to_string(),
                ));
            };
            return Ok(Stmt::AugAssign {
                target,
                op: aug_op,
                expr: rhs,
            });
        }

        // Plain expression statement
        if targets.len() == 1 {
            Ok(Stmt::Expr(targets.into_iter().next().unwrap()))
        } else {
            Ok(Stmt::Expr(Expr::Tuple(targets)))
        }
    }

    fn current_aug_op(&self) -> Option<BinaryOp> {
        match self.current() {
            Some(Token::PlusAssign) => Some(BinaryOp::Add),
            Some(Token::MinusAssign) => Some(BinaryOp::Sub),
            Some(Token::StarAssign) => Some(BinaryOp::Mul),
            Some(Token::SlashAssign) => Some(BinaryOp::Div),
            Some(Token::SlashSlashAssign) => Some(BinaryOp::FloorDiv),
            Some(Token::PercentAssign) => Some(BinaryOp::Mod),
            Some(Token::StarStarAssign) => Some(BinaryOp::Pow),
            Some(Token::AmpersandAssign) => Some(BinaryOp::BitAnd),
            Some(Token::PipeAssign) => Some(BinaryOp::BitOr),
            Some(Token::CaretAssign) => Some(BinaryOp::BitXor),
            Some(Token::LShiftAssign) => Some(BinaryOp::LShift),
            Some(Token::RShiftAssign) => Some(BinaryOp::RShift),
            Some(Token::AtAssign) => Some(BinaryOp::MatMul),
            _ => None,
        }
    }

    fn parse_def(&mut self, decorators: Vec<Expr>) -> Result<Stmt> {
        self.expect(&Token::Def)?;
        let name = self.expect_ident("function name")?;
        self.expect(&Token::LParen)?;
        let params = self.parse_params()?;
        self.expect(&Token::RParen)?;
        // Optional return annotation
        if self.is(&Token::Arrow) {
            self.bump();
            self.parse_expr()?; // consume but discard annotation
        }
        self.expect(&Token::Colon)?;
        let body = self.parse_suite()?;
        Ok(Stmt::Def {
            name,
            params,
            body,
            decorators,
        })
    }

    fn parse_params(&mut self) -> Result<Vec<FunctionParam>> {
        let mut params = Vec::new();
        let mut seen_default = false;
        let mut seen_args = false;
        let mut seen_kwargs = false;
        let mut seen_star = false; // bare * or *args seen — params after are keyword-only

        if self.is(&Token::RParen) {
            return Ok(params);
        }

        loop {
            if seen_kwargs {
                return Err(PyError::Parse("parameter after **kwargs".to_string()));
            }

            if self.is(&Token::StarStar) {
                self.bump();
                let name = self.expect_ident("kwargs parameter name")?;
                let default = if self.is(&Token::Assign) {
                    self.bump();
                    seen_default = true;
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                params.push(FunctionParam {
                    name,
                    default,
                    is_args: false,
                    is_kwargs: true,
                    is_keyword_only: false,
                });
                seen_kwargs = true;
            } else if self.is(&Token::Star) {
                self.bump();
                seen_star = true;
                if self.is(&Token::Comma) || self.is(&Token::RParen) {
                    // bare * separator: keyword-only follows
                } else {
                    let name = self.expect_ident("args parameter name")?;
                    params.push(FunctionParam {
                        name,
                        default: None,
                        is_args: true,
                        is_kwargs: false,
                        is_keyword_only: false,
                    });
                    seen_args = true;
                }
            } else {
                match self.current().cloned() {
                    Some(Token::Ident(name)) => {
                        self.bump();
                        // Optional annotation
                        if self.is(&Token::Colon) {
                            self.bump();
                            self.parse_expr()?; // discard
                        }
                        let default = if self.is(&Token::Assign) {
                            self.bump();
                            seen_default = true;
                            Some(self.parse_expr()?)
                        } else {
                            if seen_default && !seen_args && !seen_star {
                                return Err(PyError::Parse(
                                    "non-default argument follows default argument".to_string(),
                                ));
                            }
                            None
                        };
                        params.push(FunctionParam {
                            name,
                            default,
                            is_args: false,
                            is_kwargs: false,
                            is_keyword_only: seen_star,
                        });
                    }
                    other => {
                        return Err(PyError::Parse(format!(
                            "expected parameter name, found {other:?}"
                        )));
                    }
                }
            }

            if self.is(&Token::RParen) {
                break;
            }
            self.expect(&Token::Comma)?;
            if self.is(&Token::RParen) {
                break;
            }
        }

        Ok(params)
    }

    fn parse_class(&mut self, decorators: Vec<Expr>) -> Result<Stmt> {
        self.expect(&Token::Class)?;
        let name = self.expect_ident("class name")?;

        let bases = if self.is(&Token::LParen) {
            self.bump();
            let mut bases = Vec::new();
            if !self.is(&Token::RParen) {
                loop {
                    bases.push(self.parse_expr()?);
                    if self.is(&Token::RParen) {
                        break;
                    }
                    self.expect(&Token::Comma)?;
                    if self.is(&Token::RParen) {
                        break;
                    }
                }
            }
            self.expect(&Token::RParen)?;
            bases
        } else {
            Vec::new()
        };

        self.expect(&Token::Colon)?;
        let body = self.parse_suite()?;
        Ok(Stmt::Class {
            name,
            bases,
            body,
            decorators,
        })
    }

    fn parse_global(&mut self) -> Result<Stmt> {
        self.expect(&Token::Global)?;
        let names = self.parse_name_list("global")?;
        Ok(Stmt::Global(names))
    }

    fn parse_nonlocal(&mut self) -> Result<Stmt> {
        self.expect(&Token::Nonlocal)?;
        let names = self.parse_name_list("nonlocal")?;
        Ok(Stmt::Nonlocal(names))
    }

    fn parse_name_list(&mut self, ctx: &str) -> Result<Vec<String>> {
        let mut names = Vec::new();
        loop {
            names.push(self.expect_ident(ctx)?);
            if !self.is(&Token::Comma) {
                break;
            }
            self.bump();
        }
        Ok(names)
    }

    fn parse_if(&mut self) -> Result<Stmt> {
        self.expect(&Token::If)?;
        let cond = self.parse_expr()?;
        self.expect(&Token::Colon)?;
        let body = self.parse_suite()?;
        let mut branches = vec![(cond, body)];
        let mut else_branch = None;
        while self.is(&Token::Elif) {
            self.bump();
            let c = self.parse_expr()?;
            self.expect(&Token::Colon)?;
            let b = self.parse_suite()?;
            branches.push((c, b));
        }
        if self.is(&Token::Else) {
            self.bump();
            self.expect(&Token::Colon)?;
            else_branch = Some(self.parse_suite()?);
        }
        Ok(Stmt::If {
            branches,
            else_branch,
        })
    }

    fn parse_while(&mut self) -> Result<Stmt> {
        self.expect(&Token::While)?;
        let cond = self.parse_expr()?;
        self.expect(&Token::Colon)?;
        let body = self.parse_suite()?;
        let else_branch = self.parse_optional_else()?;
        Ok(Stmt::While {
            cond,
            body,
            else_branch,
        })
    }

    fn parse_for(&mut self) -> Result<Stmt> {
        self.expect(&Token::For)?;
        // Parse possibly-tuple target
        let first = self.expect_ident("for loop variable")?;
        let target = if self.is(&Token::Comma) {
            let mut names = vec![AssignTarget::Name(first)];
            while self.is(&Token::Comma) {
                self.bump();
                if self.is(&Token::In) {
                    break;
                }
                names.push(AssignTarget::Name(self.expect_ident("for loop variable")?));
            }
            AssignTarget::Tuple(names)
        } else {
            AssignTarget::Name(first)
        };
        self.expect(&Token::In)?;
        let iter = self.parse_expr()?;
        self.expect(&Token::Colon)?;
        let body = self.parse_suite()?;
        let else_branch = self.parse_optional_else()?;
        Ok(Stmt::For {
            target,
            iter,
            body,
            else_branch,
        })
    }

    fn parse_optional_else(&mut self) -> Result<Option<Vec<Stmt>>> {
        if self.is(&Token::Else) {
            self.bump();
            self.expect(&Token::Colon)?;
            Ok(Some(self.parse_suite()?))
        } else {
            Ok(None)
        }
    }

    fn parse_dotted_name(&mut self) -> Result<String> {
        let first = self.expect_ident("module name")?;
        let mut parts = vec![first];
        while self.is(&Token::Dot) {
            self.bump();
            parts.push(self.expect_ident("module name after '.'")?)
        }
        Ok(parts.join("."))
    }

    fn parse_import(&mut self) -> Result<Stmt> {
        self.expect(&Token::Import)?;
        let mut names = Vec::new();
        loop {
            let module = self.parse_dotted_name()?;
            let alias = if self.is(&Token::As) {
                self.bump();
                Some(self.expect_ident("alias after 'as'")?)
            } else {
                None
            };
            names.push((module, alias));
            if !self.is(&Token::Comma) {
                break;
            }
            self.bump();
        }
        Ok(Stmt::Import { names })
    }

    fn parse_import_from(&mut self) -> Result<Stmt> {
        self.expect(&Token::From)?;
        let module = self.parse_dotted_name()?;
        self.expect(&Token::Import)?;
        let mut names = Vec::new();
        if self.is(&Token::Star) {
            self.bump();
            names.push(("*".to_string(), None));
        } else {
            let paren = self.is(&Token::LParen);
            if paren {
                self.bump();
            }
            loop {
                let name = self.expect_ident("name in import list")?;
                let alias = if self.is(&Token::As) {
                    self.bump();
                    Some(self.expect_ident("alias after 'as'")?)
                } else {
                    None
                };
                names.push((name, alias));
                if !self.is(&Token::Comma) {
                    break;
                }
                self.bump();
                if paren && self.is(&Token::RParen) {
                    break;
                }
            }
            if paren {
                self.expect(&Token::RParen)?;
            }
        }
        Ok(Stmt::ImportFrom { module, names })
    }

    fn parse_try(&mut self) -> Result<Stmt> {
        self.expect(&Token::Try)?;
        self.expect(&Token::Colon)?;
        let body = self.parse_suite()?;
        let mut handlers = Vec::new();
        let mut else_branch = None;
        let mut finally_branch = None;
        let mut saw_bare_except = false;

        while self.is(&Token::Except) {
            self.bump();
            let kind = if self.is(&Token::Colon) {
                saw_bare_except = true;
                None
            } else {
                if saw_bare_except {
                    return Err(PyError::Parse("bare except must be last".to_string()));
                }
                Some(self.parse_expr()?)
            };
            let name = if self.is(&Token::As) {
                self.bump();
                Some(self.expect_ident("exception variable")?)
            } else {
                None
            };
            self.expect(&Token::Colon)?;
            let handler_body = self.parse_suite()?;
            handlers.push(ExceptHandler {
                kind,
                name,
                body: handler_body,
            });
        }

        if self.is(&Token::Else) {
            self.bump();
            self.expect(&Token::Colon)?;
            else_branch = Some(self.parse_suite()?);
        }
        if self.is(&Token::Finally) {
            self.bump();
            self.expect(&Token::Colon)?;
            finally_branch = Some(self.parse_suite()?);
        }
        if handlers.is_empty() && finally_branch.is_none() {
            return Err(PyError::Parse(
                "try statement must have at least one except or finally clause".to_string(),
            ));
        }
        Ok(Stmt::Try {
            body,
            handlers,
            else_branch,
            finally_branch,
        })
    }

    fn parse_raise(&mut self) -> Result<Stmt> {
        self.expect(&Token::Raise)?;
        if self.at_stmt_end() {
            Ok(Stmt::Raise {
                expr: None,
                cause: None,
            })
        } else {
            let expr = Some(self.parse_expr()?);
            let cause = if self.is(&Token::From) {
                self.bump();
                Some(self.parse_expr()?)
            } else {
                None
            };
            Ok(Stmt::Raise { expr, cause })
        }
    }

    fn parse_del(&mut self) -> Result<Stmt> {
        self.expect(&Token::Del)?;
        let mut targets = vec![self.parse_expr()?];
        while self.is(&Token::Comma) {
            self.bump();
            if self.at_stmt_end() {
                break;
            }
            targets.push(self.parse_expr()?);
        }
        Ok(Stmt::Delete(targets))
    }

    fn parse_assert(&mut self) -> Result<Stmt> {
        self.expect(&Token::Assert)?;
        let test = self.parse_expr()?;
        let msg = if self.is(&Token::Comma) {
            self.bump();
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(Stmt::Assert { test, msg })
    }

    fn parse_with(&mut self) -> Result<Stmt> {
        self.expect(&Token::With)?;
        let mut items = Vec::new();
        loop {
            let expr = self.parse_expr()?;
            let var = if self.is(&Token::As) {
                self.bump();
                Some(expr_to_assign_target(&self.parse_expr()?)?)
            } else {
                None
            };
            items.push((expr, var));
            if !self.is(&Token::Comma) {
                break;
            }
            self.bump();
        }
        self.expect(&Token::Colon)?;
        let body = self.parse_suite()?;
        Ok(Stmt::With { items, body })
    }

    fn parse_suite(&mut self) -> Result<Vec<Stmt>> {
        if self.is(&Token::Newline) {
            self.bump();
            self.expect(&Token::Indent)?;
            let mut out = Vec::new();
            self.skip_newlines();
            while !self.is(&Token::Dedent) && !self.is(&Token::Eof) {
                out.extend(self.parse_stmt_sequence()?);
                self.skip_newlines();
            }
            self.expect(&Token::Dedent)?;
            Ok(out)
        } else {
            self.parse_stmt_sequence()
        }
    }

    // ── Expressions (precedence: low → high) ───────────────────────────────
    //
    // parse_expr      (ternary: X if C else Y, lambda)
    // parse_or
    // parse_and
    // parse_not
    // parse_comparison  (==, !=, <, <=, >, >=, in, not in, is, is not)
    // parse_bitor     (|)
    // parse_bitxor    (^)
    // parse_bitand    (&)
    // parse_shift     (<<, >>)
    // parse_term      (+, -)
    // parse_factor    (*, /, //, %)
    // parse_unary     (-, +, ~)
    // parse_power     (**  right-assoc)
    // parse_postfix   (call, index, attr)
    // parse_primary

    fn parse_expr(&mut self) -> Result<Expr> {
        // Lambda
        if self.is(&Token::Lambda) {
            return self.parse_lambda();
        }
        let expr = self.parse_or()?;
        // Ternary: expr if cond else other
        if self.is(&Token::If) {
            self.bump();
            let cond = self.parse_or()?;
            self.expect(&Token::Else)?;
            let else_ = self.parse_expr()?;
            return Ok(Expr::Ternary {
                cond: Box::new(cond),
                then: Box::new(expr),
                else_: Box::new(else_),
            });
        }
        Ok(expr)
    }

    fn parse_lambda(&mut self) -> Result<Expr> {
        self.expect(&Token::Lambda)?;
        let mut params = Vec::new();
        if !self.is(&Token::Colon) {
            loop {
                params.push(self.expect_ident("lambda parameter")?);
                if !self.is(&Token::Comma) {
                    break;
                }
                self.bump();
            }
        }
        self.expect(&Token::Colon)?;
        let body = self.parse_expr()?;
        Ok(Expr::Lambda {
            params,
            body: Box::new(body),
        })
    }

    fn parse_or(&mut self) -> Result<Expr> {
        let mut expr = self.parse_and()?;
        while self.is(&Token::Or) {
            self.bump();
            let right = self.parse_and()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op: BinaryOp::Or,
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut expr = self.parse_not()?;
        while self.is(&Token::And) {
            self.bump();
            let right = self.parse_not()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op: BinaryOp::And,
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_not(&mut self) -> Result<Expr> {
        if self.is(&Token::Not) {
            self.bump();
            let expr = self.parse_not()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(expr),
            });
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<Expr> {
        let left = self.parse_bitor()?;
        let mut ops: Vec<(CmpOp, Expr)> = Vec::new();

        loop {
            let op = match self.current() {
                Some(Token::Eq) => {
                    self.bump();
                    CmpOp::Eq
                }
                Some(Token::Ne) => {
                    self.bump();
                    CmpOp::Ne
                }
                Some(Token::Lt) => {
                    self.bump();
                    CmpOp::Lt
                }
                Some(Token::Le) => {
                    self.bump();
                    CmpOp::Le
                }
                Some(Token::Gt) => {
                    self.bump();
                    CmpOp::Gt
                }
                Some(Token::Ge) => {
                    self.bump();
                    CmpOp::Ge
                }
                Some(Token::In) => {
                    self.bump();
                    CmpOp::In
                }
                Some(Token::Not) if self.peek() == Some(&Token::In) => {
                    self.bump();
                    self.bump();
                    CmpOp::NotIn
                }
                Some(Token::Is) => {
                    self.bump();
                    if self.is(&Token::Not) {
                        self.bump();
                        CmpOp::IsNot
                    } else {
                        CmpOp::Is
                    }
                }
                _ => break,
            };
            ops.push((op, self.parse_bitor()?));
        }

        if ops.is_empty() {
            return Ok(left);
        }
        // Single comparison: desugar to Binary for simplicity
        if ops.len() == 1 {
            let (op, right) = ops.remove(0);
            return Ok(Expr::Binary {
                left: Box::new(left),
                op: cmp_op_to_binary(op),
                right: Box::new(right),
            });
        }
        Ok(Expr::Compare {
            left: Box::new(left),
            ops,
        })
    }

    fn parse_bitor(&mut self) -> Result<Expr> {
        let mut expr = self.parse_bitxor()?;
        while self.is(&Token::Pipe) {
            self.bump();
            let right = self.parse_bitxor()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op: BinaryOp::BitOr,
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_bitxor(&mut self) -> Result<Expr> {
        let mut expr = self.parse_bitand()?;
        while self.is(&Token::Caret) {
            self.bump();
            let right = self.parse_bitand()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op: BinaryOp::BitXor,
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_bitand(&mut self) -> Result<Expr> {
        let mut expr = self.parse_shift()?;
        while self.is(&Token::Ampersand) {
            self.bump();
            let right = self.parse_shift()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op: BinaryOp::BitAnd,
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_shift(&mut self) -> Result<Expr> {
        let mut expr = self.parse_term()?;
        loop {
            let op = if self.is(&Token::LShift) {
                BinaryOp::LShift
            } else if self.is(&Token::RShift) {
                BinaryOp::RShift
            } else {
                break;
            };
            self.bump();
            let right = self.parse_term()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_term(&mut self) -> Result<Expr> {
        let mut expr = self.parse_factor()?;
        loop {
            let op = if self.is(&Token::Plus) {
                BinaryOp::Add
            } else if self.is(&Token::Minus) {
                BinaryOp::Sub
            } else {
                break;
            };
            self.bump();
            let right = self.parse_factor()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_factor(&mut self) -> Result<Expr> {
        let mut expr = self.parse_unary()?;
        loop {
            let op = if self.is(&Token::Star) {
                BinaryOp::Mul
            } else if self.is(&Token::At) {
                BinaryOp::MatMul
            } else if self.is(&Token::Slash) {
                BinaryOp::Div
            } else if self.is(&Token::SlashSlash) {
                BinaryOp::FloorDiv
            } else if self.is(&Token::Percent) {
                BinaryOp::Mod
            } else {
                break;
            };
            self.bump();
            let right = self.parse_unary()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<Expr> {
        if self.is(&Token::Minus) {
            self.bump();
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(self.parse_unary()?),
            });
        }
        if self.is(&Token::Tilde) {
            self.bump();
            return Ok(Expr::Unary {
                op: UnaryOp::BitNot,
                expr: Box::new(self.parse_unary()?),
            });
        }
        if self.is(&Token::Plus) {
            self.bump();
            return Ok(Expr::Unary {
                op: UnaryOp::Pos,
                expr: Box::new(self.parse_unary()?),
            });
        }
        self.parse_power()
    }

    fn parse_power(&mut self) -> Result<Expr> {
        let base = self.parse_postfix()?;
        if self.is(&Token::StarStar) {
            self.bump();
            let exp = self.parse_unary()?; // right-associative
            return Ok(Expr::Binary {
                left: Box::new(base),
                op: BinaryOp::Pow,
                right: Box::new(exp),
            });
        }
        Ok(base)
    }

    fn parse_postfix(&mut self) -> Result<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.is(&Token::LParen) {
                self.bump();
                let args = self.parse_call_args()?;
                self.expect(&Token::RParen)?;
                expr = Expr::Call {
                    func: Box::new(expr),
                    args,
                };
                continue;
            }
            if self.is(&Token::LBracket) {
                self.bump();
                expr = self.parse_subscript(expr)?;
                continue;
            }
            if self.is(&Token::Dot) {
                self.bump();
                let name = match self.current().cloned() {
                    Some(Token::Ident(name)) => {
                        self.bump();
                        name
                    }
                    other => {
                        return Err(PyError::Parse(format!(
                            "expected attribute name after '.', found {other:?}"
                        )));
                    }
                };
                expr = Expr::Attr {
                    target: Box::new(expr),
                    name,
                };
                continue;
            }
            break;
        }
        Ok(expr)
    }

    fn parse_subscript(&mut self, target: Expr) -> Result<Expr> {
        // Inside [ already consumed. Parse optional slice or index.
        let first = if self.is(&Token::Colon) {
            None
        } else {
            Some(self.parse_expr()?)
        };

        if self.is(&Token::Colon) {
            // Slice
            self.bump();
            let upper = if self.is(&Token::RBracket) || self.is(&Token::Colon) {
                None
            } else {
                Some(Box::new(self.parse_expr()?))
            };
            let step = if self.is(&Token::Colon) {
                self.bump();
                if self.is(&Token::RBracket) {
                    None
                } else {
                    Some(Box::new(self.parse_expr()?))
                }
            } else {
                None
            };
            self.expect(&Token::RBracket)?;
            Ok(Expr::Slice {
                target: Box::new(target),
                lower: first.map(Box::new),
                upper,
                step,
            })
        } else {
            // Plain index
            self.expect(&Token::RBracket)?;
            Ok(Expr::Index {
                target: Box::new(target),
                index: Box::new(
                    first.ok_or_else(|| PyError::Parse("empty subscript".to_string()))?,
                ),
            })
        }
    }

    fn parse_call_args(&mut self) -> Result<Vec<CallArg>> {
        let mut args = Vec::new();
        let mut seen_keyword = false;
        if self.is(&Token::RParen) {
            return Ok(args);
        }
        loop {
            if self.is(&Token::StarStar) {
                self.bump();
                seen_keyword = true;
                args.push(CallArg {
                    name: None,
                    value: self.parse_expr()?,
                    splat: false,
                    double_splat: true,
                });
            } else if self.is(&Token::Star) {
                self.bump();
                if seen_keyword {
                    return Err(PyError::Parse(
                        "positional argument follows keyword argument".to_string(),
                    ));
                }
                args.push(CallArg {
                    name: None,
                    value: self.parse_expr()?,
                    splat: true,
                    double_splat: false,
                });
            } else if let Some(Token::Ident(name)) = self.current().cloned() {
                if self.peek() == Some(&Token::Assign) {
                    self.bump();
                    self.bump();
                    seen_keyword = true;
                    args.push(CallArg {
                        name: Some(name),
                        value: self.parse_expr()?,
                        splat: false,
                        double_splat: false,
                    });
                } else {
                    if seen_keyword {
                        return Err(PyError::Parse(
                            "positional argument follows keyword argument".to_string(),
                        ));
                    }
                    args.push(CallArg {
                        name: None,
                        value: self.parse_expr()?,
                        splat: false,
                        double_splat: false,
                    });
                }
            } else {
                if seen_keyword {
                    return Err(PyError::Parse(
                        "positional argument follows keyword argument".to_string(),
                    ));
                }
                args.push(CallArg {
                    name: None,
                    value: self.parse_expr()?,
                    splat: false,
                    double_splat: false,
                });
            }
            if self.is(&Token::RParen) {
                break;
            }
            self.expect(&Token::Comma)?;
            if self.is(&Token::RParen) {
                break;
            }
        }
        Ok(args)
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        match self.current().cloned() {
            Some(Token::Int(v)) => {
                self.bump();
                Ok(Expr::Int(v))
            }
            Some(Token::Float(v)) => {
                self.bump();
                Ok(Expr::Float(v))
            }
            Some(Token::Str(v)) => {
                self.bump();
                // Adjacent string literal concatenation
                let mut s = v;
                while let Some(Token::Str(next)) = self.current().cloned() {
                    self.bump();
                    s.push_str(&next);
                }
                Ok(Expr::Str(s))
            }
            Some(Token::True) => {
                self.bump();
                Ok(Expr::Bool(true))
            }
            Some(Token::False) => {
                self.bump();
                Ok(Expr::Bool(false))
            }
            Some(Token::None) => {
                self.bump();
                Ok(Expr::None)
            }
            Some(Token::Ident(name)) => {
                self.bump();
                Ok(Expr::Var(name))
            }
            Some(Token::LParen) => {
                self.bump();
                if self.is(&Token::RParen) {
                    self.bump();
                    return Ok(Expr::Tuple(vec![]));
                }
                let first = self.parse_expr()?;
                if self.is(&Token::Comma) {
                    // Tuple
                    let mut items = vec![first];
                    while self.is(&Token::Comma) {
                        self.bump();
                        if self.is(&Token::RParen) {
                            break;
                        }
                        items.push(self.parse_expr()?);
                    }
                    self.expect(&Token::RParen)?;
                    Ok(Expr::Tuple(items))
                } else {
                    self.expect(&Token::RParen)?;
                    Ok(first)
                }
            }
            Some(Token::LBracket) => self.parse_list_literal(),
            Some(Token::LBrace) => self.parse_dict_or_set_literal(),
            other => Err(PyError::Parse(format!(
                "unexpected token in expression: {other:?}"
            ))),
        }
    }

    fn parse_list_literal(&mut self) -> Result<Expr> {
        self.expect(&Token::LBracket)?;
        if self.is(&Token::RBracket) {
            self.bump();
            return Ok(Expr::List(vec![]));
        }
        let first = self.parse_expr()?;
        // Detect list comprehension: [expr for ...]
        if self.is(&Token::For) {
            let clauses = self.parse_comp_clauses()?;
            self.expect(&Token::RBracket)?;
            return Ok(Expr::ListComp {
                elt: Box::new(first),
                clauses,
            });
        }
        let mut items = vec![first];
        while self.is(&Token::Comma) {
            self.bump();
            if self.is(&Token::RBracket) {
                break;
            }
            items.push(self.parse_expr()?);
        }
        self.expect(&Token::RBracket)?;
        Ok(Expr::List(items))
    }

    /// Parse one or more comprehension clauses: `for target in iter (if cond)? ...`
    fn parse_comp_clauses(&mut self) -> Result<Vec<CompClause>> {
        let mut clauses = Vec::new();
        while self.is(&Token::For) {
            self.bump(); // consume `for`
            // Parse possibly-tuple target (same as for-loop)
            let first = self.expect_ident("comprehension variable")?;
            let target = if self.is(&Token::Comma) {
                let mut names = vec![AssignTarget::Name(first)];
                while self.is(&Token::Comma) {
                    self.bump();
                    if self.is(&Token::In) {
                        break;
                    }
                    names.push(AssignTarget::Name(
                        self.expect_ident("comprehension variable")?,
                    ));
                }
                AssignTarget::Tuple(names)
            } else {
                AssignTarget::Name(first)
            };
            self.expect(&Token::In)?;
            let iter = self.parse_or()?; // parse iterable (no comma-list at this level)
            let cond = if self.is(&Token::If) {
                self.bump();
                Some(self.parse_or()?)
            } else {
                None
            };
            clauses.push(CompClause { target, iter, cond });
        }
        Ok(clauses)
    }

    fn parse_dict_or_set_literal(&mut self) -> Result<Expr> {
        self.expect(&Token::LBrace)?;
        if self.is(&Token::RBrace) {
            self.bump();
            return Ok(Expr::Dict(vec![]));
        }
        let first = self.parse_expr()?;
        if self.is(&Token::Colon) {
            // Dict or dict comprehension
            self.bump();
            let val = self.parse_expr()?;
            if self.is(&Token::For) {
                // Dict comprehension: {key: val for ...}
                let clauses = self.parse_comp_clauses()?;
                self.expect(&Token::RBrace)?;
                return Ok(Expr::DictComp {
                    key: Box::new(first),
                    val: Box::new(val),
                    clauses,
                });
            }
            let mut items = vec![(first, val)];
            while self.is(&Token::Comma) {
                self.bump();
                if self.is(&Token::RBrace) {
                    break;
                }
                let k = self.parse_expr()?;
                self.expect(&Token::Colon)?;
                let v = self.parse_expr()?;
                items.push((k, v));
            }
            self.expect(&Token::RBrace)?;
            Ok(Expr::Dict(items))
        } else {
            // Set or set comprehension
            if self.is(&Token::For) {
                // Set comprehension: {elt for ...}
                let clauses = self.parse_comp_clauses()?;
                self.expect(&Token::RBrace)?;
                return Ok(Expr::SetComp {
                    elt: Box::new(first),
                    clauses,
                });
            }
            let mut items = vec![first];
            while self.is(&Token::Comma) {
                self.bump();
                if self.is(&Token::RBrace) {
                    break;
                }
                items.push(self.parse_expr()?);
            }
            self.expect(&Token::RBrace)?;
            Ok(Expr::Set(items))
        }
    }

    // ── Helpers ─────────────────────────────────────────────────────────────

    fn expect_ident(&mut self, ctx: &str) -> Result<String> {
        match self.current().cloned() {
            Some(Token::Ident(name)) => {
                self.bump();
                Ok(name)
            }
            other => Err(PyError::Parse(format!("expected {ctx}, found {other:?}"))),
        }
    }

    fn current(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos + 1)
    }

    fn is(&self, token: &Token) -> bool {
        self.current() == Some(token)
    }

    fn bump(&mut self) {
        self.pos += 1;
    }

    fn expect(&mut self, token: &Token) -> Result<()> {
        if self.is(token) {
            self.bump();
            Ok(())
        } else {
            Err(PyError::Parse(format!(
                "expected {token:?}, found {:?}",
                self.current()
            )))
        }
    }

    fn skip_newlines(&mut self) {
        while self.is(&Token::Newline) {
            self.bump();
        }
    }

    fn at_stmt_end(&self) -> bool {
        matches!(
            self.current(),
            Some(Token::Newline)
                | Some(Token::Semicolon)
                | Some(Token::Dedent)
                | Some(Token::Eof)
                | None
        )
    }
}

fn expr_to_assign_target(expr: &Expr) -> Result<AssignTarget> {
    match expr {
        Expr::Var(name) => Ok(AssignTarget::Name(name.clone())),
        Expr::Attr { target, name } => Ok(AssignTarget::Attr(target.clone(), name.clone())),
        Expr::Index { target, index } => Ok(AssignTarget::Index(target.clone(), index.clone())),
        Expr::Tuple(items) => {
            let targets: Result<Vec<AssignTarget>> =
                items.iter().map(|e| expr_to_assign_target(e)).collect();
            Ok(AssignTarget::Tuple(targets?))
        }
        _ => Err(PyError::Parse(
            "cannot assign to this expression".to_string(),
        )),
    }
}

fn cmp_op_to_binary(op: CmpOp) -> BinaryOp {
    match op {
        CmpOp::Eq => BinaryOp::Eq,
        CmpOp::Ne => BinaryOp::Ne,
        CmpOp::Lt => BinaryOp::Lt,
        CmpOp::Le => BinaryOp::Le,
        CmpOp::Gt => BinaryOp::Gt,
        CmpOp::Ge => BinaryOp::Ge,
        CmpOp::In => BinaryOp::In,
        CmpOp::NotIn => BinaryOp::NotIn,
        CmpOp::Is => BinaryOp::Is,
        CmpOp::IsNot => BinaryOp::IsNot,
    }
}
