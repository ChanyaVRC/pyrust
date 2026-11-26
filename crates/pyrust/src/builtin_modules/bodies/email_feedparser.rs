// `email.feedparser` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// Incremental RFC 2822 / MIME feed parsers: `FeedParser`, `BytesFeedParser`,
// `BufferedSubFile`, and `NeedMoreData`.
//
// Reference: <https://docs.python.org/3/library/email.feedparser.html>

use std::collections::VecDeque;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBytesLike, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::Value;

fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some((*k).to_string()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    call_kw(interp, callee, args, &[])
}

fn default_policy(interp: &mut Interpreter) -> Result<Value> {
    let policy_mod = interp.load_module("email.policy")?;
    interp.get_attr(&policy_mod, "compat32")
}

fn make_message(
    interp: &mut Interpreter,
    factory: &Option<Value>,
    policy: &Value,
) -> Result<Value> {
    match factory {
        None => {
            let msg_factory = interp.get_attr(policy, "message_factory")?;
            if msg_factory.is_none() {
                let msg_mod = interp.load_module("email.message")?;
                let msg_cls = interp.get_attr(&msg_mod, "Message")?;
                call_kw(interp, msg_cls, &[], &[("policy", policy.clone())])
            } else {
                call_kw(interp, msg_factory, &[], &[("policy", policy.clone())])
            }
        }
        Some(f) => match call_kw(interp, f.clone(), &[], &[("policy", policy.clone())]) {
            Ok(v) => Ok(v),
            Err(e) if e.class_name_is("TypeError") => call_pos(interp, f.clone(), &[]),
            Err(e) => Err(e),
        },
    }
}

fn split_lines_keepends(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut lines = Vec::new();
    let mut i = 0;
    while i < n {
        let start = i;
        while i < n && bytes[i] != b'\r' && bytes[i] != b'\n' {
            i += 1;
        }
        if i < n {
            if bytes[i] == b'\r' && i + 1 < n && bytes[i + 1] == b'\n' {
                i += 2;
            } else {
                i += 1;
            }
        }
        lines.push(&text[start..i]);
    }
    lines
}

fn strip_one_trailing_eol(text: &str) -> &str {
    if let Some(s) = text.strip_suffix("\r\n") {
        s
    } else if let Some(s) = text.strip_suffix('\n') {
        s
    } else if let Some(s) = text.strip_suffix('\r') {
        s
    } else {
        text
    }
}

fn is_empty_line(line: &str) -> bool {
    matches!(line, "\r\n" | "\n" | "\r")
}

fn handle_defect(
    interp: &mut Interpreter,
    policy: &Value,
    msg: &Value,
    defect_name: &str,
    arg: Option<Value>,
) -> Result<()> {
    let defect = super::email_errors::make_defect(interp, defect_name, arg)?;
    let handler = interp.get_attr(policy, "handle_defect")?;
    call_pos(interp, handler, &[msg.clone(), defect])?;
    Ok(())
}

fn is_boundary_line(line: &str, boundary: &str) -> Option<bool> {
    let rest = line.strip_prefix("--")?.strip_prefix(boundary)?;
    let (is_end, tail) = if let Some(after_end) = rest.strip_prefix("--") {
        (true, after_end)
    } else {
        (false, rest)
    };
    let tail = tail.trim_start_matches([' ', '\t']);
    if tail.is_empty() || matches!(tail, "\r\n" | "\n" | "\r") {
        Some(is_end)
    } else {
        None
    }
}

fn parse_message_text(
    interp: &mut Interpreter,
    text: &str,
    factory: &Option<Value>,
    policy: &Value,
    headersonly: bool,
    default_type: Option<&str>,
) -> Result<Value> {
    let msg = make_message(interp, factory, policy)?;
    if let Some(dt) = default_type {
        let set_dt = interp.get_attr(&msg, "set_default_type")?;
        call_pos(interp, set_dt, &[Value::string(dt)])?;
    }

    let lines = split_lines_keepends(text);
    let mut idx = 0usize;
    let n = lines.len();

    if idx < n && lines[idx].starts_with("From ") {
        let ufrom = lines[idx].trim_end_matches(['\r', '\n']);
        let set_uf = interp.get_attr(&msg, "set_unixfrom")?;
        call_pos(interp, set_uf, &[Value::string(ufrom)])?;
        idx += 1;
    }

    let mut cur_header: Vec<String> = Vec::new();
    let flush_header = |interp: &mut Interpreter, cur: &mut Vec<String>| -> Result<()> {
        if cur.is_empty() {
            return Ok(());
        }
        if cur[0].contains(':') {
            let hsp = interp.get_attr(policy, "header_source_parse")?;
            let list_val = Value::list(cur.iter().map(Value::string).collect());
            let pair = call_pos(interp, hsp, &[list_val])?;
            let items = interp.collect_iterable(&pair)?;
            if items.len() >= 2 {
                let set_raw = interp.get_attr(&msg, "set_raw")?;
                call_pos(interp, set_raw, &[items[0].clone(), items[1].clone()])?;
            }
        }
        cur.clear();
        Ok(())
    };

    while idx < n {
        let line = lines[idx];
        if is_empty_line(line) {
            flush_header(interp, &mut cur_header)?;
            idx += 1;
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if !cur_header.is_empty() {
                cur_header.push(line.to_string());
                idx += 1;
                continue;
            }
            handle_defect(
                interp,
                policy,
                &msg,
                "FirstHeaderLineIsContinuationDefect",
                Some(Value::string(line)),
            )?;
            break;
        }
        if let Some(colon) = line.find(':')
            && colon > 0
            && !line[..colon]
                .bytes()
                .any(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        {
            flush_header(interp, &mut cur_header)?;
            cur_header.push(line.to_string());
            idx += 1;
        } else {
            flush_header(interp, &mut cur_header)?;
            handle_defect(
                interp,
                policy,
                &msg,
                "MissingHeaderBodySeparatorDefect",
                None,
            )?;
            break;
        }
    }
    flush_header(interp, &mut cur_header)?;

    let body: String = lines[idx..].concat();

    if headersonly {
        let sp = interp.get_attr(&msg, "set_payload")?;
        call_pos(interp, sp, &[Value::string(body)])?;
        return Ok(msg);
    }

    let get_mt = interp.get_attr(&msg, "get_content_maintype")?;
    let mt_val = call_pos(interp, get_mt, &[])?;
    let maintype = mt_val.as_str().unwrap_or("text").to_string();

    let get_st = interp.get_attr(&msg, "get_content_subtype")?;
    let st_val = call_pos(interp, get_st, &[])?;
    let subtype = st_val.as_str().unwrap_or("plain").to_string();

    if maintype == "multipart" {
        let get_b = interp.get_attr(&msg, "get_boundary")?;
        let b_val = call_pos(interp, get_b, &[])?;
        if b_val.is_none() {
            handle_defect(interp, policy, &msg, "NoBoundaryInMultipartDefect", None)?;
            let sp = interp.get_attr(&msg, "set_payload")?;
            call_pos(interp, sp, &[Value::string(body)])?;
            return Ok(msg);
        }
        let boundary = b_val.as_str().unwrap_or("").to_string();
        parse_multipart_body(interp, &msg, &body, &boundary, factory, policy, &subtype)?;
        return Ok(msg);
    }

    if maintype == "message" && matches!(subtype.as_str(), "rfc822" | "news" | "global") {
        let submsg = parse_message_text(interp, &body, factory, policy, false, None)?;
        let sp = interp.get_attr(&msg, "set_payload")?;
        call_pos(interp, sp, &[Value::list(vec![submsg])])?;
        return Ok(msg);
    }

    let sp = interp.get_attr(&msg, "set_payload")?;
    call_pos(interp, sp, &[Value::string(body)])?;
    Ok(msg)
}

fn parse_multipart_body(
    interp: &mut Interpreter,
    msg: &Value,
    body: &str,
    boundary: &str,
    factory: &Option<Value>,
    policy: &Value,
    subtype: &str,
) -> Result<()> {
    let body_lines = split_lines_keepends(body);
    let mut preamble_lines: Vec<&str> = Vec::new();
    let mut parts_chunks: Vec<Vec<&str>> = Vec::new();
    let mut epilogue_lines: Option<Vec<&str>> = None;
    let mut current_part: Option<Vec<&str>> = None;
    let mut seen_start = false;

    for line in body_lines {
        if let Some(ref mut epi) = epilogue_lines {
            epi.push(line);
            continue;
        }
        if let Some(is_end) = is_boundary_line(line, boundary) {
            if !seen_start {
                seen_start = true;
                if is_end {
                    epilogue_lines = Some(Vec::new());
                } else {
                    current_part = Some(Vec::new());
                }
            } else {
                if let Some(cp) = current_part.take() {
                    parts_chunks.push(cp);
                }
                if is_end {
                    epilogue_lines = Some(Vec::new());
                } else {
                    current_part = Some(Vec::new());
                }
            }
        } else if !seen_start {
            preamble_lines.push(line);
        } else if let Some(ref mut cp) = current_part {
            cp.push(line);
        }
    }

    if let Some(cp) = current_part.take() {
        parts_chunks.push(cp);
    }

    if !seen_start {
        handle_defect(interp, policy, msg, "StartBoundaryNotFoundDefect", None)?;
        let sp = interp.get_attr(msg, "set_payload")?;
        call_pos(interp, sp, &[Value::list(Vec::new())])?;
        if !body.is_empty() {
            interp.assign_attr(msg.clone(), "preamble", Value::string(body))?;
        }
        return Ok(());
    }

    if epilogue_lines.is_none() {
        handle_defect(interp, policy, msg, "CloseBoundaryNotFoundDefect", None)?;
    }

    if !preamble_lines.is_empty() {
        let joined = preamble_lines.concat();
        let trimmed = strip_one_trailing_eol(&joined);
        interp.assign_attr(msg.clone(), "preamble", Value::string(trimmed))?;
    } else {
        interp.assign_attr(msg.clone(), "preamble", Value::none())?;
    }

    if let Some(epi) = epilogue_lines {
        interp.assign_attr(msg.clone(), "epilogue", Value::string(epi.concat()))?;
    } else {
        interp.assign_attr(msg.clone(), "epilogue", Value::none())?;
    }

    let child_default_type = if subtype == "digest" {
        Some("message/rfc822")
    } else {
        None
    };

    let mut subparts = Vec::with_capacity(parts_chunks.len());
    for chunk in parts_chunks {
        let joined = chunk.concat();
        let raw_part = strip_one_trailing_eol(&joined);
        let submsg =
            parse_message_text(interp, raw_part, factory, policy, false, child_default_type)?;
        subparts.push(submsg);
    }
    let sp = interp.get_attr(msg, "set_payload")?;
    call_pos(interp, sp, &[Value::list(subparts)])?;
    Ok(())
}

struct BufferedSubFileState {
    partial: String,
    lines: VecDeque<String>,
    eofstack: Vec<Value>,
    closed: bool,
}

struct FeedParserState {
    factory: Option<Value>,
    headersonly: bool,
    chunks: Vec<Value>,
}

fn need_more_data_singleton() -> Value {
    let obj_cls = crate::interpreter::object_class_singleton();
    let inst = crate::value::PyInstance {
        class: obj_cls,
        attrs: crate::value::InstanceAttrs::default(),
    };
    Value::py_instance(std::rc::Rc::new(std::cell::RefCell::new(inst)))
}

fn all_list() -> Value {
    Value::list(vec![
        Value::string("FeedParser"),
        Value::string("BytesFeedParser"),
    ])
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email"),
        "__all__" => all_list(),
        "NL" => Value::string("\n"),
        "EMPTYSTRING" => Value::string(""),
        "NeedMoreData" => need_more_data_singleton(),
    }

    #[native_state(BufferedSubFileState)]
    class BufferedSubFile {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            set_state(
                self_val.0,
                BufferedSubFileState {
                    partial: String::new(),
                    lines: VecDeque::new(),
                    eofstack: Vec::new(),
                    closed: false,
                },
            )?;
            Ok(Value::none())
        }

        fn push_eof_matcher(
            #[state_mut] st: &mut BufferedSubFileState,
            pred: PyValue,
        ) -> Result<Value> {
            st.eofstack.push(pred.0);
            Ok(Value::none())
        }

        fn pop_eof_matcher(#[state_mut] st: &mut BufferedSubFileState) -> Result<Value> {
            st.eofstack
                .pop()
                .ok_or_else(|| PyError::named("IndexError", "pop from empty list".to_string()))
        }

        fn close(#[state_mut] st: &mut BufferedSubFileState) -> Result<Value> {
            if !st.partial.is_empty() {
                let tail = std::mem::take(&mut st.partial);
                st.lines.push_front(tail);
            }
            st.closed = true;
            Ok(Value::none())
        }

        fn readline(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let line_opt = with_state_mut(self_val.0, |st| {
                if let Some(line) = st.lines.pop_back() {
                    Ok(Some(line))
                } else if st.closed {
                    Ok(Some(String::new()))
                } else {
                    Ok(None)
                }
            })?;
            let Some(line) = line_opt else {
                let m = _interp.load_module("email.feedparser")?;
                return _interp.get_attr(&m, "NeedMoreData");
            };
            if line.is_empty() {
                return Ok(Value::string(""));
            }
            let matchers: Vec<Value> = with_state(self_val.0, |st| {
                Ok(st.eofstack.iter().rev().cloned().collect())
            })?;
            let line_val = Value::string(&line);
            for matcher in matchers {
                let matched = call_pos(_interp, matcher, std::slice::from_ref(&line_val))?;
                if _interp.truthy_value(&matched)? {
                    with_state_mut(self_val.0, |st| {
                        st.lines.push_back(line.clone());
                        Ok(())
                    })?;
                    return Ok(Value::string(""));
                }
            }
            Ok(line_val)
        }

        fn unreadline(
            #[state_mut] st: &mut BufferedSubFileState,
            line: PyStr<'_>,
        ) -> Result<Value> {
            st.lines.push_back(line.0.to_string());
            Ok(Value::none())
        }

        fn push(#[state_mut] st: &mut BufferedSubFileState, data: PyStr<'_>) -> Result<Value> {
            let mut combined = std::mem::take(&mut st.partial);
            combined.push_str(data.0.as_ref());
            let parts = split_lines_keepends(&combined);
            if parts.is_empty() {
                return Ok(Value::none());
            }
            let last_idx = parts.len() - 1;
            let last = parts[last_idx];
            let last_complete = last.ends_with('\n') || (last.ends_with('\r') && last.len() > 1);
            let complete_end = if last_complete && !last.ends_with('\r') {
                parts.len()
            } else {
                st.partial = last.to_string();
                last_idx
            };
            for part in &parts[..complete_end] {
                st.lines.push_front((*part).to_string());
            }
            Ok(Value::none())
        }

        fn pushlines(
            #[state_mut] st: &mut BufferedSubFileState,
            lines: PyValue,
        ) -> Result<Value> {
            let items = _interp.collect_iterable(&lines.0)?;
            for item in items.into_iter().rev() {
                if let Some(s) = item.as_str() {
                    st.lines.push_back(s.to_string());
                }
            }
            Ok(Value::none())
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __next__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let rl = _interp.get_attr(self_val.0, "readline")?;
            let line = call_pos(_interp, rl, &[])?;
            if line.as_str() == Some("") {
                return Err(PyError::named("StopIteration", String::new()));
            }
            Ok(line)
        }
    }

    #[native_state(FeedParserState)]
    class FeedParser {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _factory: OptValueRef<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            policy: OptValueRef<'_>,
        ) -> Result<Value> {
            let pol = match policy.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => default_policy(_interp)?,
            };
            let fac = _factory.as_value().filter(|v| !v.is_none()).cloned();
            _interp.assign_attr(self_val.0.clone(), "policy", pol)?;
            set_state(
                self_val.0,
                FeedParserState {
                    factory: fac,
                    headersonly: false,
                    chunks: Vec::new(),
                },
            )?;
            Ok(Value::none())
        }

        fn _set_headersonly(#[state_mut] st: &mut FeedParserState) -> Result<Value> {
            st.headersonly = true;
            Ok(Value::none())
        }

        fn feed(
            #[state_mut] st: &mut FeedParserState,
            data: PyValue,
        ) -> Result<Value> {
            if data.0.as_str().is_none() {
                return Err(PyError::named(
                    "TypeError",
                    "FeedParser.feed() expects a str".to_string(),
                ));
            }
            st.chunks.push(data.0);
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (factory, headersonly, chunks) = with_state_mut(self_val.0, |st| {
                let chunks = std::mem::take(&mut st.chunks);
                Ok((st.factory.clone(), st.headersonly, chunks))
            })?;
            let policy = _interp.get_attr(self_val.0, "policy")?;
            let empty = Value::string("");
            let join_fn = _interp.get_attr(&empty, "join")?;
            let text_val = call_pos(_interp, join_fn, &[Value::list(chunks)])?;
            let text = text_val.as_str().unwrap_or("");
            parse_message_text(_interp, text, &factory, &policy, headersonly, None)
        }
    }

    class BytesFeedParser("FeedParser") {
        fn feed(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyBytesLike<'_>,
        ) -> Result<Value> {
            let b_val = Value::bytes(data.as_bytes().to_vec());
            let decode_fn = _interp.get_attr(&b_val, "decode")?;
            let decoded = call_pos(
                _interp,
                decode_fn,
                &[Value::string("ascii"), Value::string("surrogateescape")],
            )?;
            let m = _interp.load_module("email.feedparser")?;
            let fp_cls = _interp.get_attr(&m, "FeedParser")?;
            let feed_fn = _interp.get_attr(&fp_cls, "feed")?;
            call_pos(_interp, feed_fn, &[self_val.0.clone(), decoded])
        }
    }
}
