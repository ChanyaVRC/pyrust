/// Produce Python's `repr()` of a string value, matching CPython's output.
///
/// Rules (same as CPython's `repr()` for `str`):
/// - Prefer single-quote delimiters.
/// - If the string contains a single quote but no double quote, use double-quote
///   delimiters instead (avoids the need to escape `'`).
/// - If both quote types appear, use single-quote delimiters and escape `'` as `\'`.
/// - Escape backslashes, non-printable control characters, and surrogates.
fn py_repr_str(s: &str) -> String {
    let has_single = s.contains('\'');
    let has_double = s.contains('"');
    let quote = if has_single && !has_double { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\x0b' => out.push_str("\\v"),
            '\'' if quote == '\'' => out.push_str("\\'"),
            '"' if quote == '"' => out.push_str("\\\""),
            c if (c as u32) < 0x20 || c == '\x7f' => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Convert an annotation expression to its source-text string representation,
/// as required by PEP 563 (`from __future__ import annotations`).
///
/// CPython stores the unparsed source text of the annotation as a string in
/// `__annotations__`.  We reconstruct a canonical form from the AST that
/// matches CPython output for the annotation expressions commonly found in
/// real code.  String-literal annotations are preserved with their quotes
/// (e.g. `x: 'Foo'` → `"'Foo'"`), consistent with CPython 3.12 behaviour.
fn stringify_annotation(expr: &Expr) -> String {
    match expr {
        Expr::Var(name, _) => name.clone(),
        Expr::None => "None".to_string(),
        Expr::Ellipsis => "...".to_string(),
        Expr::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        Expr::Int(n) => n.to_string(),
        Expr::Float(f) => format!("{f}"),
        Expr::Str(s) => py_repr_str(s),
        Expr::Attr { target, name, .. } => {
            format!("{}.{}", stringify_annotation(target), name)
        }
        Expr::Index { target, index, .. } => {
            // In subscript position, a tuple is rendered without outer parens:
            // `dict[str, int]` not `dict[(str, int)]`.
            let index_str = match index.as_ref() {
                Expr::Tuple(elts) => {
                    let parts: Vec<String> = elts.iter().map(stringify_annotation).collect();
                    parts.join(", ")
                }
                other => stringify_annotation(other),
            };
            format!("{}[{}]", stringify_annotation(target), index_str)
        }
        Expr::List(elts) => {
            let parts: Vec<String> = elts.iter().map(stringify_annotation).collect();
            format!("[{}]", parts.join(", "))
        }
        Expr::Tuple(elts) => {
            let parts: Vec<String> = elts.iter().map(stringify_annotation).collect();
            format!("({})", parts.join(", "))
        }
        Expr::Binary {
            left, op, right, ..
        } => {
            let op_str = match op {
                BinaryOp::BitOr => " | ",
                BinaryOp::Add => " + ",
                BinaryOp::Sub => " - ",
                BinaryOp::Mul => " * ",
                _ => " | ",
            };
            format!(
                "{}{}{}",
                stringify_annotation(left),
                op_str,
                stringify_annotation(right)
            )
        }
        Expr::Unary { op, expr, .. } => {
            let op_str = match op {
                UnaryOp::Neg => "-",
                UnaryOp::Pos => "+",
                UnaryOp::Not => "not ",
                UnaryOp::BitNot => "~",
            };
            format!("{}{}", op_str, stringify_annotation(expr))
        }
        // For anything else (calls, comprehensions, etc.) fall back to a
        // best-effort representation — these are rarely used as annotations.
        _ => format!("{expr:?}"),
    }
}
