/// The annotation-scope expression whose syntax is being validated.
///
/// PEP 695 gives type-parameter bounds and type-alias values their own scope,
/// but that scope is not a normal function body: a directly-owned assignment,
/// yield, or await expression is forbidden. Keeping the owner here prevents
/// the function/class/type-alias emitters from duplicating the traversal and
/// CPython-compatible diagnostics.
#[derive(Clone, Copy)]
enum Pep695ExpressionOwner {
    TypeVarBound,
    TypeAlias,
}

impl Pep695ExpressionOwner {
    fn label(self) -> &'static str {
        match self {
            Self::TypeVarBound => "a TypeVar bound",
            Self::TypeAlias => "a type alias",
        }
    }
}

#[derive(Clone, Copy)]
enum Pep695ExpressionViolation {
    NamedExpression,
    YieldExpression,
    AwaitExpression,
    AssignmentWithinComprehension,
    AssignmentInComprehensionIterable,
}

impl Pep695ExpressionViolation {
    fn message(self, owner: Pep695ExpressionOwner) -> String {
        match self {
            Self::NamedExpression => {
                format!("named expression cannot be used within {}", owner.label())
            }
            Self::YieldExpression => {
                format!("yield expression cannot be used within {}", owner.label())
            }
            Self::AwaitExpression => {
                format!("await expression cannot be used within {}", owner.label())
            }
            Self::AssignmentWithinComprehension => format!(
                "assignment expression within a comprehension cannot be used in {}",
                owner.label()
            ),
            Self::AssignmentInComprehensionIterable => {
                "assignment expression cannot be used in a comprehension iterable expression"
                    .to_string()
            }
        }
    }
}

/// Whether the current expression is evaluated directly by the PEP 695
/// annotation scope or by a comprehension nested inside it.
///
/// A comprehension's outermost iterable still belongs to its enclosing scope.
/// Its result, filters, and later iterables belong to the implicit
/// comprehension function, where `:=` gets the more specific PEP 695
/// diagnostic and yield/await remain the comprehension compiler's concern.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pep695ExpressionContext {
    AnnotationScope,
    ComprehensionBody,
}

fn validate_pep695_expression(expr: &Expr, owner: Pep695ExpressionOwner) -> Option<String> {
    pep695_expression_violation(expr, Pep695ExpressionContext::AnnotationScope)
        .map(|violation| violation.message(owner))
}

fn validate_type_parameter_bounds(type_params: &[TypeParam]) -> Option<String> {
    for parameter in type_params {
        match &parameter.bound {
            None => {}
            Some(TypeParamBound::Bound(expr)) => {
                if let Some(message) =
                    validate_pep695_expression(expr, Pep695ExpressionOwner::TypeVarBound)
                {
                    return Some(message);
                }
            }
            Some(TypeParamBound::Constraints(expressions)) => {
                for expr in expressions {
                    if let Some(message) =
                        validate_pep695_expression(expr, Pep695ExpressionOwner::TypeVarBound)
                    {
                        return Some(message);
                    }
                }
            }
        }
    }
    None
}

fn pep695_expression_violation(
    expr: &Expr,
    context: Pep695ExpressionContext,
) -> Option<Pep695ExpressionViolation> {
    match expr {
        Expr::Named { .. } => {
            let violation = match context {
                Pep695ExpressionContext::AnnotationScope => {
                    Pep695ExpressionViolation::NamedExpression
                }
                Pep695ExpressionContext::ComprehensionBody => {
                    Pep695ExpressionViolation::AssignmentWithinComprehension
                }
            };
            Some(violation)
        }
        Expr::Yield(_) | Expr::YieldFrom(_)
            if context == Pep695ExpressionContext::AnnotationScope =>
        {
            Some(Pep695ExpressionViolation::YieldExpression)
        }
        Expr::Await(_) if context == Pep695ExpressionContext::AnnotationScope => {
            Some(Pep695ExpressionViolation::AwaitExpression)
        }
        // Yield/await in a comprehension body have comprehension-specific
        // diagnostics and async classification. Do not replace those here.
        Expr::Yield(_) | Expr::YieldFrom(_) | Expr::Await(_) => None,
        Expr::Lambda { params, .. } => params.iter().find_map(|parameter| {
            parameter
                .default
                .as_ref()
                .and_then(|expr| pep695_expression_violation(expr, context))
                .or_else(|| {
                    parameter
                        .annotation
                        .as_ref()
                        .and_then(|expr| pep695_expression_violation(expr, context))
                })
        }),
        Expr::ListComp { elt, clauses }
        | Expr::SetComp { elt, clauses }
        | Expr::GenExp { elt, clauses } => pep695_comprehension_violation(&[elt], clauses, context),
        Expr::DictComp { key, val, clauses } => {
            pep695_comprehension_violation(&[key, val], clauses, context)
        }
        Expr::Binary { left, right, .. } => pep695_expression_violation(left, context)
            .or_else(|| pep695_expression_violation(right, context)),
        Expr::Unary { expr, .. } | Expr::Starred(expr) => {
            pep695_expression_violation(expr, context)
        }
        Expr::Compare { left, ops } => pep695_expression_violation(left, context).or_else(|| {
            ops.iter()
                .find_map(|(_, operand)| pep695_expression_violation(operand, context))
        }),
        Expr::Ternary { cond, then, else_ } => pep695_expression_violation(cond, context)
            .or_else(|| pep695_expression_violation(then, context))
            .or_else(|| pep695_expression_violation(else_, context)),
        Expr::Call { func, args, .. } => pep695_expression_violation(func, context).or_else(|| {
            args.iter()
                .find_map(|argument| pep695_expression_violation(&argument.value, context))
        }),
        Expr::Attr { target, .. } => pep695_expression_violation(target, context),
        Expr::Index { target, index, .. } => pep695_expression_violation(target, context)
            .or_else(|| pep695_expression_violation(index, context)),
        Expr::Slice {
            target,
            lower,
            upper,
            step,
        } => pep695_expression_violation(target, context).or_else(|| {
            [lower, upper, step]
                .iter()
                .flat_map(|bound| bound.as_deref())
                .find_map(|bound| pep695_expression_violation(bound, context))
        }),
        Expr::Tuple(items) | Expr::List(items) | Expr::Set(items) => items
            .iter()
            .find_map(|item| pep695_expression_violation(item, context)),
        Expr::Dict(items) => items.iter().find_map(|item| match item {
            DictItem::Pair(key, value) => pep695_expression_violation(key, context)
                .or_else(|| pep695_expression_violation(value, context)),
            DictItem::DoubleSplat(expr) => pep695_expression_violation(expr, context),
        }),
        Expr::FString(parts) => pep695_fstring_violation(parts, context),
        Expr::Var(_, _)
        | Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis => None,
    }
}

fn pep695_comprehension_violation(
    result_exprs: &[&Expr],
    clauses: &[CompClause],
    context: Pep695ExpressionContext,
) -> Option<Pep695ExpressionViolation> {
    let first_clause = clauses.first()?;

    // Only a top-level comprehension's first iterable is evaluated directly
    // in the annotation scope. Once already inside a comprehension, every
    // further comprehension iterable is subject to PEP 572's iterable rule.
    if context == Pep695ExpressionContext::AnnotationScope {
        if let Some(violation) = pep695_expression_violation(
            &first_clause.iter,
            Pep695ExpressionContext::AnnotationScope,
        ) {
            return Some(violation);
        }
    } else if expr_contains_assignment_expression(&first_clause.iter) {
        return Some(Pep695ExpressionViolation::AssignmentInComprehensionIterable);
    }

    // Later iterables always execute in the implicit comprehension function.
    // PEP 572's iterable prohibition crosses lambda/nested-comprehension syntax,
    // hence the deliberately lexical helper used here.
    if clauses
        .iter()
        .skip(1)
        .any(|clause| expr_contains_assignment_expression(&clause.iter))
    {
        return Some(Pep695ExpressionViolation::AssignmentInComprehensionIterable);
    }

    let body_context = Pep695ExpressionContext::ComprehensionBody;
    result_exprs
        .iter()
        .find_map(|expr| pep695_expression_violation(expr, body_context))
        .or_else(|| {
            clauses.iter().find_map(|clause| {
                clause
                    .cond
                    .as_ref()
                    .and_then(|condition| pep695_expression_violation(condition, body_context))
            })
        })
}

fn pep695_fstring_violation(
    parts: &[FStringPart],
    context: Pep695ExpressionContext,
) -> Option<Pep695ExpressionViolation> {
    parts.iter().find_map(|part| match part {
        FStringPart::Literal(_) => None,
        FStringPart::Expr {
            expr, format_spec, ..
        } => pep695_expression_violation(expr, context).or_else(|| {
            format_spec
                .as_deref()
                .and_then(|parts| pep695_fstring_violation(parts, context))
        }),
    })
}
