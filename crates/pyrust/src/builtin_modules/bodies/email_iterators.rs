// `email.iterators` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// Iterators and structure debugging helpers for email `Message` trees:
// `walk`, `body_line_iterator`, `typed_subpart_iterator`, and `_structure`.
//
// Reference: <https://docs.python.org/3/library/email.iterators.html>

use std::collections::VecDeque;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyIndex, PyStr, PyValue, PyValueRef,
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

fn collect_walk(interp: &mut Interpreter, msg: &Value, out: &mut Vec<Value>) -> Result<()> {
    out.push(msg.clone());
    let is_mp_fn = interp.get_attr(msg, "is_multipart")?;
    let is_mp = call_pos(interp, is_mp_fn, &[])?;
    if interp.truthy_value(&is_mp)? {
        let gp_fn = interp.get_attr(msg, "get_payload")?;
        let payload = call_pos(interp, gp_fn, &[])?;
        let parts = interp.collect_iterable(&payload)?;
        for subpart in parts {
            collect_walk(interp, &subpart, out)?;
        }
    }
    Ok(())
}

fn split_stringio_lines(s: &str) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut start = 0;
    let mut i = 0;
    while i < n {
        if bytes[i] == b'\n' {
            out.push(s[start..=i].to_string());
            i += 1;
            start = i;
        } else {
            i += 1;
        }
    }
    if start < n {
        out.push(s[start..n].to_string());
    }
    out
}

fn structure_impl(
    interp: &mut Interpreter,
    msg: &Value,
    fp: &Value,
    level: usize,
    include_default: bool,
) -> Result<()> {
    let tab = " ".repeat(level * 4);
    let get_ct = interp.get_attr(msg, "get_content_type")?;
    let ct_val = call_pos(interp, get_ct, &[])?;
    let ct = ct_val.as_str().unwrap_or("");
    let line = if include_default {
        let get_dt = interp.get_attr(msg, "get_default_type")?;
        let dt_val = call_pos(interp, get_dt, &[])?;
        let dt = dt_val.as_str().unwrap_or("");
        format!("{tab}{ct} [{dt}]\n")
    } else {
        format!("{tab}{ct}\n")
    };
    let write_fn = interp.get_attr(fp, "write")?;
    call_pos(interp, write_fn, &[Value::string(line)])?;

    let is_mp_fn = interp.get_attr(msg, "is_multipart")?;
    let is_mp = call_pos(interp, is_mp_fn, &[])?;
    if interp.truthy_value(&is_mp)? {
        let gp_fn = interp.get_attr(msg, "get_payload")?;
        let payload = call_pos(interp, gp_fn, &[])?;
        let parts = interp.collect_iterable(&payload)?;
        for subpart in parts {
            structure_impl(interp, &subpart, fp, level + 1, include_default)?;
        }
    }
    Ok(())
}

struct ValueQueueIterState {
    items: VecDeque<Value>,
}

fn make_queue_iter(interp: &mut Interpreter, items: Vec<Value>) -> Result<Value> {
    let m = interp.load_module("email.iterators")?;
    let cls = interp.get_attr(&m, "_EmailIterator")?;
    let inst = call_pos(interp, cls, &[Value::list(items)])?;
    Ok(inst)
}

fn all_list() -> Value {
    Value::list(vec![
        Value::string("body_line_iterator"),
        Value::string("typed_subpart_iterator"),
        Value::string("walk"),
    ])
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email"),
        "__all__" => all_list(),
    }

    #[native_state(ValueQueueIterState)]
    class _EmailIterator {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            items: PyValue,
        ) -> Result<Value> {
            let collected = _interp.collect_iterable(&items.0)?;
            set_state(
                self_val.0,
                ValueQueueIterState {
                    items: VecDeque::from(collected),
                },
            )?;
            Ok(Value::none())
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __next__(#[state_mut] st: &mut ValueQueueIterState) -> Result<Value> {
            match st.items.pop_front() {
                Some(v) => Ok(v),
                None => Err(PyError::named("StopIteration", String::new())),
            }
        }
    }

    fn walk(self_msg: PyValueRef<'_>) -> Result<Value> {
        let mut parts = Vec::new();
        collect_walk(_interp, self_msg.0, &mut parts)?;
        make_queue_iter(_interp, parts)
    }

    fn body_line_iterator(
        msg: PyValueRef<'_>,
        #[default(false.into())] decode: PyBool,
    ) -> Result<Value> {
        let mut parts = Vec::new();
        collect_walk(_interp, msg.0, &mut parts)?;
        let mut lines = Vec::new();
        for subpart in parts {
            let gp = _interp.get_attr(&subpart, "get_payload")?;
            let payload = call_kw(_interp, gp, &[], &[("decode", Value::bool_(decode.0))])?;
            if let Some(s) = payload.as_str() {
                for line in split_stringio_lines(s) {
                    lines.push(Value::string(line));
                }
            }
        }
        make_queue_iter(_interp, lines)
    }

    fn typed_subpart_iterator(
        msg: PyValueRef<'_>,
        #[default("text".into())] maintype: PyStr<'_>,
        #[default(OptionalArg::Omitted)] subtype: OptValueRef<'_>,
    ) -> Result<Value> {
        let sub_filter = subtype
            .as_value()
            .filter(|v| !v.is_none())
            .and_then(|v| v.as_str().map(str::to_string));
        let mut parts = Vec::new();
        collect_walk(_interp, msg.0, &mut parts)?;
        let mut matched = Vec::new();
        for subpart in parts {
            let get_mt = _interp.get_attr(&subpart, "get_content_maintype")?;
            let mt = call_pos(_interp, get_mt, &[])?;
            if mt.as_str() == Some(maintype.0.as_ref()) {
                if let Some(ref st_want) = sub_filter {
                    let get_st = _interp.get_attr(&subpart, "get_content_subtype")?;
                    let st = call_pos(_interp, get_st, &[])?;
                    if st.as_str() == Some(st_want.as_str()) {
                        matched.push(subpart);
                    }
                } else {
                    matched.push(subpart);
                }
            }
        }
        make_queue_iter(_interp, matched)
    }

    fn _structure(
        msg: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] fp: OptValueRef<'_>,
        #[default(0i64.into())] level: PyIndex,
        #[default(false.into())] include_default: PyBool,
    ) -> Result<Value> {
        let lvl = level.expect_i64(FN_NAME, "level")?.max(0) as usize;
        let out_fp = match fp.as_value() {
            Some(v) if !v.is_none() => v.clone(),
            _ => {
                let sys_mod = _interp.load_module("sys")?;
                _interp.get_attr(&sys_mod, "stdout")?
            }
        };
        structure_impl(_interp, msg.0, &out_fp, lvl, include_default.0)?;
        Ok(Value::none())
    }
}
