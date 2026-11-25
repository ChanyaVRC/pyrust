// `tabnanny` module — 100% native Rust implementation of Python 3.12's
// `tabnanny` (detection of ambiguous indentation) module (issue #3249).
//
// Implements CPython's exact `Whitespace` tabsize-witness algorithm,
// `NannyNag(Exception)`, `process_tokens(tokens)`, `check(file)`,
// `format_witnesses`, `errprint`, `verbose`, and `filename_only`.
//
// Reference: <https://docs.python.org/3.12/library/tabnanny.html>

use std::path::Path;

use pyrust_derive::pyrust_module;

use super::tokenize::lexer::{
    LexError, TOK_COMMENT, TOK_DEDENT, TOK_INDENT, TOK_NEWLINE, TOK_NL, tokenize_source_relaxed,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyIndex, PyPath, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

// ── Exact CPython `Whitespace` indentation model ─────────────────────────────

#[derive(Debug, Clone)]
struct WsModel {
    raw: String,
    n: usize,
    nt: usize,
    is_simple: bool,
}

impl WsModel {
    fn new(ws: &str) -> Self {
        let mut n = 0usize;
        let mut nt = 0usize;
        for ch in ws.chars() {
            if ch == ' ' {
                n += 1;
            } else if ch == '\t' {
                nt += 1;
                n = 0;
            } else {
                break;
            }
        }
        let is_simple = nt == 0;
        Self {
            raw: ws.to_string(),
            n,
            nt,
            is_simple,
        }
    }

    fn longest_run_of_spaces(&self) -> usize {
        let mut best = self.n;
        let mut cur = 0usize;
        for ch in self.raw.chars() {
            if ch == ' ' {
                cur += 1;
                best = best.max(cur);
            } else if ch == '\t' {
                cur = 0;
            } else {
                break;
            }
        }
        best
    }

    fn indent_level(&self, tabsize: usize) -> usize {
        let ts = tabsize.max(1);
        let mut col = 0usize;
        for ch in self.raw.chars() {
            if ch == ' ' {
                col += 1;
            } else if ch == '\t' {
                col = (col / ts + 1) * ts;
            } else {
                break;
            }
        }
        col
    }

    fn max_check_tabsize(&self, other: &Self) -> usize {
        self.longest_run_of_spaces()
            .max(other.longest_run_of_spaces())
            + 1
    }

    fn equal(&self, other: &Self) -> bool {
        if self.raw == other.raw {
            return true;
        }
        let max_ts = self.max_check_tabsize(other);
        for ts in 1..=max_ts {
            if self.indent_level(ts) != other.indent_level(ts) {
                return false;
            }
        }
        true
    }

    fn not_equal_witness(&self, other: &Self) -> Vec<(usize, usize, usize)> {
        let max_ts = self.max_check_tabsize(other);
        let mut a = Vec::new();
        for ts in 1..=max_ts {
            let l1 = self.indent_level(ts);
            let l2 = other.indent_level(ts);
            if l1 != l2 {
                a.push((ts, l1, l2));
            }
        }
        a
    }

    fn less(&self, other: &Self) -> bool {
        if self.indent_level(1) >= other.indent_level(1) {
            return false;
        }
        if self.is_simple && other.is_simple {
            return self.n < other.n;
        }
        let max_ts = self.max_check_tabsize(other);
        for ts in 2..=max_ts {
            if self.indent_level(ts) >= other.indent_level(ts) {
                return false;
            }
        }
        true
    }

    fn not_less_witness(&self, other: &Self) -> Vec<(usize, usize, usize)> {
        let max_ts = self.max_check_tabsize(other);
        let mut a = Vec::new();
        for ts in 1..=max_ts {
            let l1 = self.indent_level(ts);
            let l2 = other.indent_level(ts);
            if l1 >= l2 {
                a.push((ts, l1, l2));
            }
        }
        a
    }
}

fn format_witnesses_str(witnesses: &[(usize, usize, usize)]) -> String {
    let firsts: Vec<String> = witnesses.iter().map(|(ts, _, _)| ts.to_string()).collect();
    let prefix = if firsts.len() == 1 {
        "at tab size "
    } else {
        "at tab sizes "
    };
    format!("{prefix}{}", firsts.join(", "))
}

fn leading_whitespace(line: &str) -> &str {
    let end = line
        .char_indices()
        .find(|(_, c)| *c != ' ' && *c != '\t')
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    &line[..end]
}

fn raise_nanny_nag(interp: &mut Interpreter, lineno: i64, msg: String, line: String) -> PyError {
    if let Ok(mod_val) = interp.load_module("tabnanny")
        && let Ok(cls) = interp.get_attr(&mod_val, "NannyNag")
        && let Ok(exc) = interp.call_function_expanded(
            cls,
            &[
                ExpandedCallArg {
                    name: None,
                    value: Value::int(lineno),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::string(&msg),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::string(&line),
                },
            ],
        )
    {
        return PyError::raised(exc);
    }
    PyError::named("Exception", msg)
}

fn write_stream(interp: &mut Interpreter, stream_name: &str, text: &str) {
    if let Ok(sys_mod) = interp.load_module("sys")
        && let Ok(stream) = interp.get_attr(&sys_mod, stream_name)
        && !stream.is_none()
        && let Ok(write_fn) = interp.get_attr(&stream, "write")
    {
        let _ = interp.call_function_expanded(
            write_fn,
            &[ExpandedCallArg {
                name: None,
                value: Value::string(text),
            }],
        );
    }
}

fn get_module_int_flag(interp: &mut Interpreter, attr: &str) -> i64 {
    if let Ok(m) = interp.load_module("tabnanny")
        && let Ok(v) = interp.get_attr(&m, attr)
    {
        return match v.kind() {
            ValueKind::Int(n) => n,
            ValueKind::Bool(b) => i64::from(b),
            _ => 0,
        };
    }
    0
}

struct TokenRecord {
    tok_type: i64,
    string: String,
    start_line: i64,
    line: String,
}

fn run_token_check(interp: &mut Interpreter, tokens: &[TokenRecord]) -> Result<()> {
    let mut indents = vec![WsModel::new("")];
    let mut check_equal = false;

    for tok in tokens {
        if tok.tok_type == TOK_NEWLINE {
            check_equal = true;
        } else if tok.tok_type == TOK_INDENT {
            check_equal = false;
            let thisguy = WsModel::new(&tok.string);
            let top = indents.last().expect("indents stack non-empty");
            if !top.less(&thisguy) {
                let witness = top.not_less_witness(&thisguy);
                let msg = format!("indent not greater e.g. {}", format_witnesses_str(&witness));
                return Err(raise_nanny_nag(
                    interp,
                    tok.start_line,
                    msg,
                    tok.line.clone(),
                ));
            }
            indents.push(thisguy);
        } else if tok.tok_type == TOK_DEDENT {
            check_equal = true;
            if indents.len() > 1 {
                indents.pop();
            }
        } else if check_equal && tok.tok_type != TOK_NL && tok.tok_type != TOK_COMMENT {
            check_equal = false;
            let ws = leading_whitespace(&tok.line);
            let thisguy = WsModel::new(ws);
            let top = indents.last().expect("indents stack non-empty");
            if !top.equal(&thisguy) {
                let witness = top.not_equal_witness(&thisguy);
                let msg = format!("indent not equal e.g. {}", format_witnesses_str(&witness));
                return Err(raise_nanny_nag(
                    interp,
                    tok.start_line,
                    msg,
                    tok.line.clone(),
                ));
            }
        }
    }
    Ok(())
}

fn check_single_file(interp: &mut Interpreter, file_str: &str) -> Result<()> {
    let verbose = get_module_int_flag(interp, "verbose");
    let filename_only = get_module_int_flag(interp, "filename_only");

    if verbose > 0 {
        write_stream(interp, "stdout", &format!("checking {file_str:?} ...\n"));
    }

    let source = match std::fs::read_to_string(file_str) {
        Ok(s) => s,
        Err(e) => {
            write_stream(interp, "stderr", &format!("{file_str:?}: I/O Error: {e}\n"));
            return Ok(());
        }
    };

    let lex_tokens = match tokenize_source_relaxed(&source) {
        Ok(t) => t,
        Err(LexError::TokenError { msg, line, col }) => {
            write_stream(
                interp,
                "stderr",
                &format!("{file_str:?}: Token Error: ({msg:?}, ({line}, {col}))\n"),
            );
            return Ok(());
        }
        Err(LexError::IndentationError { msg, line, .. })
        | Err(LexError::TabError { msg, line, .. }) => {
            write_stream(
                interp,
                "stderr",
                &format!("{file_str:?}: Indentation Error: {msg} (line {line})\n"),
            );
            return Ok(());
        }
    };

    let records: Vec<TokenRecord> = lex_tokens
        .into_iter()
        .map(|t| TokenRecord {
            tok_type: t.tok_type,
            string: t.string,
            start_line: t.start_line as i64,
            line: t.line,
        })
        .collect();

    match run_token_check(interp, &records) {
        Ok(()) => {
            if verbose > 0 {
                write_stream(
                    interp,
                    "stdout",
                    &format!("{file_str:?}: Clean bill of health.\n"),
                );
            }
        }
        Err(err) => {
            if let Ok(exc) = interp.materialize_pyerror(err) {
                let lineno = interp
                    .get_attr(&exc, "lineno")
                    .ok()
                    .and_then(|v| match v.kind() {
                        ValueKind::Int(n) => Some(n),
                        _ => None,
                    })
                    .unwrap_or(0);
                let line = interp
                    .get_attr(&exc, "line")
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                if verbose > 0 {
                    write_stream(
                        interp,
                        "stdout",
                        &format!("{file_str:?}: *** Line {lineno}: trouble in tab city! ***\n"),
                    );
                    write_stream(interp, "stdout", &format!("offending line: {line:?}\n"));
                } else {
                    let display = if file_str.contains(' ') {
                        format!("{file_str:?}")
                    } else {
                        file_str.to_string()
                    };
                    if filename_only != 0 {
                        write_stream(interp, "stdout", &format!("{display}\n"));
                    } else {
                        write_stream(interp, "stdout", &format!("{display} {lineno} {line:?}\n"));
                    }
                }
            }
        }
    }
    Ok(())
}

fn check_path_recursive(interp: &mut Interpreter, path: &Path) -> Result<()> {
    let verbose = get_module_int_flag(interp, "verbose");
    if path.is_dir() && !path.is_symlink() {
        if verbose > 0 {
            write_stream(
                interp,
                "stdout",
                &format!("{:?}: listing directory\n", path.to_string_lossy()),
            );
        }
        let mut names: Vec<String> = match std::fs::read_dir(path) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(e) => {
                write_stream(
                    interp,
                    "stderr",
                    &format!("{:?}: I/O Error: {e}\n", path.to_string_lossy()),
                );
                return Ok(());
            }
        };
        names.sort();
        for name in names {
            let full = path.join(&name);
            if (full.is_dir() && !full.is_symlink()) || name.ends_with(".py") {
                check_path_recursive(interp, &full)?;
            }
        }
        return Ok(());
    }
    check_single_file(interp, &path.to_string_lossy())
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("check"),
            Value::string("NannyNag"),
            Value::string("process_tokens"),
        ]),
        "verbose" => Value::int(0),
        "filename_only" => Value::int(0),
    }

    class NannyNag("Exception") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            lineno: PyIndex,
            msg: PyStr<'_>,
            line: PyStr<'_>,
        ) -> Result<Value> {
            let ln = lineno.expect_i64("NannyNag", "lineno")?;
            _interp.assign_attr(self_val.0.clone(), "lineno", Value::int(ln))?;
            _interp.assign_attr(self_val.0.clone(), "msg", Value::string(&msg.0))?;
            _interp.assign_attr(self_val.0.clone(), "line", Value::string(&line.0))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "args",
                Value::tuple(vec![
                    Value::int(ln),
                    Value::string(&msg.0),
                    Value::string(&line.0),
                ]),
            )?;
            Ok(Value::none())
        }

        fn get_lineno(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "lineno")
        }

        fn get_msg(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "msg")
        }

        fn get_line(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "line")
        }
    }

    #[native_state(WsModel)]
    class Whitespace {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            ws: PyStr<'_>,
        ) -> Result<Value> {
            let model = WsModel::new(&ws.0);
            _interp.assign_attr(self_val.0.clone(), "raw", Value::string(&model.raw))?;
            _interp.assign_attr(self_val.0.clone(), "n", Value::int(model.n as i64))?;
            _interp.assign_attr(self_val.0.clone(), "nt", Value::int(model.nt as i64))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "is_simple",
                Value::bool_(model.is_simple),
            )?;
            set_state(self_val.0, model)?;
            Ok(Value::none())
        }

        fn longest_run_of_spaces(#[state] ws: &WsModel) -> Result<Value> {
            Ok(Value::int(ws.longest_run_of_spaces() as i64))
        }

        fn indent_level(#[state] ws: &WsModel, tabsize: PyIndex) -> Result<Value> {
            let ts = tabsize.expect_i64("indent_level", "tabsize")?.max(1) as usize;
            Ok(Value::int(ws.indent_level(ts) as i64))
        }

        fn equal(#[state] ws: &WsModel, other: PyValue) -> Result<Value> {
            let ok = with_state(&other.0, |o| Ok(ws.equal(o)))?;
            Ok(Value::bool_(ok))
        }

        fn not_equal_witness(#[state] ws: &WsModel, other: PyValue) -> Result<Value> {
            let w = with_state(&other.0, |o| Ok(ws.not_equal_witness(o)))?;
            Ok(Value::list(
                w.into_iter()
                    .map(|(a, b, c)| Value::tuple(vec![Value::int(a as i64), Value::int(b as i64), Value::int(c as i64)]))
                    .collect(),
            ))
        }

        fn less(#[state] ws: &WsModel, other: PyValue) -> Result<Value> {
            let ok = with_state(&other.0, |o| Ok(ws.less(o)))?;
            Ok(Value::bool_(ok))
        }

        fn not_less_witness(#[state] ws: &WsModel, other: PyValue) -> Result<Value> {
            let w = with_state(&other.0, |o| Ok(ws.not_less_witness(o)))?;
            Ok(Value::list(
                w.into_iter()
                    .map(|(a, b, c)| Value::tuple(vec![Value::int(a as i64), Value::int(b as i64), Value::int(c as i64)]))
                    .collect(),
            ))
        }
    }

    fn format_witnesses(w: PyValue) -> Result<Value> {
        let items = _interp.collect_iterable(&w.0)?;
        let mut witnesses = Vec::with_capacity(items.len());
        for item in items {
            let elems = _interp.collect_iterable(&item)?;
            let first = elems
                .first()
                .and_then(|v| match v.kind() {
                    ValueKind::Int(n) => Some(n as usize),
                    _ => None,
                })
                .unwrap_or(0);
            witnesses.push((first, 0, 0));
        }
        Ok(Value::string(format_witnesses_str(&witnesses)))
    }

    fn errprint(#[varargs] args: &[Value]) -> Result<Value> {
        let mut parts = Vec::with_capacity(args.len());
        for a in args {
            parts.push(_interp.render_value_as_str(a)?);
        }
        let mut out = parts.join(" ");
        out.push('\n');
        write_stream(_interp, "stderr", &out);
        Ok(Value::none())
    }

    fn process_tokens(tokens: PyValue) -> Result<Value> {
        let items = _interp.collect_iterable(&tokens.0)?;
        let mut records = Vec::with_capacity(items.len());
        for item in items {
            let fields = _interp.collect_iterable(&item)?;
            if fields.len() < 5 {
                return Err(PyError::named(
                    "ValueError",
                    "not enough values to unpack (expected 5)".to_string(),
                ));
            }
            let tok_type = match fields[0].kind() {
                ValueKind::Int(n) => n,
                _ => 0,
            };
            let string = fields[1].as_str().unwrap_or_default().to_string();
            let start_fields = _interp.collect_iterable(&fields[2])?;
            let start_line = start_fields
                .first()
                .and_then(|v| match v.kind() {
                    ValueKind::Int(n) => Some(n),
                    _ => None,
                })
                .unwrap_or(0);
            let line = fields[4].as_str().unwrap_or_default().to_string();
            records.push(TokenRecord {
                tok_type,
                string,
                start_line,
                line,
            });
        }
        run_token_check(_interp, &records)?;
        Ok(Value::none())
    }

    fn check(file: PyPath) -> Result<Value> {
        check_path_recursive(_interp, file.as_path())?;
        Ok(Value::none())
    }

    fn main() -> Result<Value> {
        let sys_mod = _interp.load_module("sys")?;
        let argv = _interp.get_attr(&sys_mod, "argv")?;
        let items = _interp.collect_iterable(&argv)?;
        for arg in items.into_iter().skip(1) {
            if let Some(s) = arg.as_str()
                && !s.starts_with('-')
            {
                check_path_recursive(_interp, Path::new(s))?;
            }
        }
        Ok(Value::none())
    }
}
