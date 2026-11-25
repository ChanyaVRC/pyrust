// `email.generator` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// Message tree serializers: `Generator`, `BytesGenerator`, and `DecodedGenerator`.
//
// Reference: <https://docs.python.org/3/library/email.generator.html>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, OptValueRef, OptionalArg, PyBool, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, Value, ValueKind};

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

fn mangle_from_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at_line_start = true;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if at_line_start && text[i..].starts_with("From ") {
            out.push_str(">From ");
            i += 5;
            at_line_start = false;
            continue;
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
        at_line_start = ch == '\n';
    }
    out
}

fn normalize_newlines(text: &str, nl: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < n {
        if bytes[i] == b'\r' {
            out.push_str(nl);
            if i + 1 < n && bytes[i + 1] == b'\n' {
                i += 2;
            } else {
                i += 1;
            }
        } else if bytes[i] == b'\n' {
            out.push_str(nl);
            i += 1;
        } else {
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn make_boundary_impl(interp: &mut Interpreter, text: Option<&Value>) -> Result<String> {
    let random_mod = interp.load_module("random")?;
    let randrange = interp.get_attr(&random_mod, "randrange")?;
    let sys_mod = interp.load_module("sys")?;
    let maxsize = interp.get_attr(&sys_mod, "maxsize")?;
    let max_i = maxsize.as_int().unwrap_or(i64::MAX);
    let width = (max_i.saturating_sub(1)).to_string().len();
    let tok_val = call_pos(interp, randrange, &[maxsize])?;
    let tok = tok_val.as_int().unwrap_or(0);
    let base = format!("==============={tok:0width$}==");

    let Some(t) = text.filter(|v| !v.is_none()) else {
        return Ok(base);
    };
    let haystack_str: String = if let Some(s) = t.as_str() {
        s.to_string()
    } else if let Ok(bl) = crate::interpreter::builtin_args::PyBytesLike::try_from_value_with_interp(
        interp,
        t,
        "_make_boundary",
        "text",
    ) {
        String::from_utf8_lossy(bl.as_bytes()).into_owned()
    } else {
        super::email_errors::py_str(interp, t)?
    };

    let mut b = base.clone();
    let mut counter = 0usize;
    loop {
        let needle_start = format!("--{b}");
        let needle_end = format!("--{b}--");
        let found = haystack_str.lines().any(|line| {
            let trimmed = line.trim_end_matches('\r');
            trimmed == needle_start || trimmed == needle_end
        });
        if !found {
            return Ok(b);
        }
        b = format!("{base}.{counter}");
        counter += 1;
    }
}

fn has_surrogates(interp: &mut Interpreter, val: &Value) -> bool {
    let Ok(utils) = interp.load_module("email.utils") else {
        return false;
    };
    let Ok(f) = interp.get_attr(&utils, "_has_surrogates") else {
        return false;
    };
    call_pos(interp, f, std::slice::from_ref(val))
        .and_then(|v| interp.truthy_value(&v))
        .unwrap_or(false)
}

fn is_bytes_generator(interp: &mut Interpreter, self_val: &Value) -> bool {
    let Ok(m) = interp.load_module("email.generator") else {
        return false;
    };
    let Ok(bg) = interp.get_attr(&m, "BytesGenerator") else {
        return false;
    };
    call_pos(
        interp,
        Value::builtin_function("isinstance"),
        &[self_val.clone(), bg],
    )
    .and_then(|v| interp.truthy_value(&v))
    .unwrap_or(false)
}

fn gen_write(interp: &mut Interpreter, self_val: &Value, s: &str) -> Result<()> {
    let write_fn = interp.get_attr(self_val, "write")?;
    call_pos(interp, write_fn, &[Value::string(s)])?;
    Ok(())
}

fn gen_write_val(interp: &mut Interpreter, self_val: &Value, val: Value) -> Result<()> {
    let write_fn = interp.get_attr(self_val, "write")?;
    call_pos(interp, write_fn, &[val])?;
    Ok(())
}

fn gen_fp_write(interp: &mut Interpreter, self_val: &Value, val: Value) -> Result<()> {
    let fp = interp.get_attr(self_val, "_fp")?;
    let write_fn = interp.get_attr(&fp, "write")?;
    call_pos(interp, write_fn, &[val])?;
    Ok(())
}

fn gen_write_headers(
    interp: &mut Interpreter,
    self_val: &Value,
    msg: &Value,
    is_bytes: bool,
) -> Result<()> {
    let policy = interp.get_attr(self_val, "policy")?;
    let nl = interp
        .get_attr(self_val, "_NL")?
        .as_str()
        .unwrap_or("\n")
        .to_string();
    let raw_items_fn = interp.get_attr(msg, "raw_items")?;
    let raw_iter = call_pos(interp, raw_items_fn, &[])?;
    let pairs = interp.collect_iterable(&raw_iter)?;
    let fold_name = if is_bytes { "fold_binary" } else { "fold" };
    let fold_fn = interp.get_attr(&policy, fold_name)?;
    for pair in pairs {
        let kv = interp.collect_iterable(&pair)?;
        if kv.len() >= 2 {
            let folded = call_pos(interp, fold_fn.clone(), &[kv[0].clone(), kv[1].clone()])?;
            if is_bytes {
                gen_fp_write(interp, self_val, folded)?;
            } else {
                gen_write_val(interp, self_val, folded)?;
            }
        }
    }
    gen_write(interp, self_val, &nl)?;
    Ok(())
}

fn gen_write_lines(interp: &mut Interpreter, self_val: &Value, text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let nl = interp
        .get_attr(self_val, "_NL")?
        .as_str()
        .unwrap_or("\n")
        .to_string();
    let normalized = normalize_newlines(text, &nl);
    gen_write(interp, self_val, &normalized)
}

fn gen_write_msg(interp: &mut Interpreter, self_val: &Value, msg: &Value) -> Result<()> {
    let is_bytes = is_bytes_generator(interp, self_val);
    let oldfp = interp.get_attr(self_val, "_fp")?;
    let new_buf_fn = interp.get_attr(self_val, "_new_buffer")?;
    let sfp = call_pos(interp, new_buf_fn, &[])?;

    interp.assign_attr(self_val.clone(), "_munge_cte", Value::none())?;
    interp.assign_attr(self_val.clone(), "_fp", sfp.clone())?;
    let dispatch_res = (|| -> Result<()> {
        let dispatch_fn = interp.get_attr(self_val, "_dispatch")?;
        call_pos(interp, dispatch_fn, std::slice::from_ref(msg))?;
        Ok(())
    })();
    interp.assign_attr(self_val.clone(), "_fp", oldfp)?;
    let munge_cte = interp
        .get_attr(self_val, "_munge_cte")
        .unwrap_or_else(|_| Value::none());
    dispatch_res?;

    let target_msg = if !munge_cte.is_none() {
        let copy_mod = interp.load_module("copy")?;
        let deepcopy = interp.get_attr(&copy_mod, "deepcopy")?;
        let cloned = call_pos(interp, deepcopy, std::slice::from_ref(msg))?;
        let items = interp.collect_iterable(&munge_cte)?;
        if items.len() >= 2 {
            let get_fn = interp.get_attr(&cloned, "get")?;
            let existing = call_pos(
                interp,
                get_fn,
                &[Value::string("content-transfer-encoding")],
            )?;
            if existing.is_none() {
                let setitem = interp.get_attr(&cloned, "__setitem__")?;
                call_pos(
                    interp,
                    setitem,
                    &[Value::string("Content-Transfer-Encoding"), items[0].clone()],
                )?;
            } else {
                let rep = interp.get_attr(&cloned, "replace_header")?;
                call_pos(
                    interp,
                    rep,
                    &[Value::string("content-transfer-encoding"), items[0].clone()],
                )?;
            }
            let rep2 = interp.get_attr(&cloned, "replace_header")?;
            call_pos(
                interp,
                rep2,
                &[Value::string("content-type"), items[1].clone()],
            )?;
        }
        cloned
    } else {
        msg.clone()
    };

    if let Ok(meth) = interp.get_attr(&target_msg, "_write_headers")
        && !meth.is_none()
    {
        call_pos(interp, meth, std::slice::from_ref(self_val))?;
    } else {
        gen_write_headers(interp, self_val, &target_msg, is_bytes)?;
    }

    let getval = interp.get_attr(&sfp, "getvalue")?;
    let body_buf = call_pos(interp, getval, &[])?;
    gen_fp_write(interp, self_val, body_buf)?;
    Ok(())
}

fn gen_handle_text_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    msg: &Value,
    is_bytes: bool,
) -> Result<()> {
    let mangle = interp
        .get_attr(self_val, "_mangle_from_")
        .and_then(|v| interp.truthy_value(&v))
        .unwrap_or(false);

    if is_bytes {
        let raw_payload = interp.get_attr(msg, "_payload")?;
        if raw_payload.is_none() {
            return Ok(());
        }
        let policy = interp.get_attr(self_val, "policy")?;
        let cte_type = interp
            .get_attr(&policy, "cte_type")
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "8bit".to_string());
        if has_surrogates(interp, &raw_payload) && cte_type != "7bit" {
            let mut s = raw_payload.as_str().unwrap_or("").to_string();
            if mangle {
                s = mangle_from_lines(&s);
            }
            return gen_write_lines(interp, self_val, &s);
        }
    }

    let gp = interp.get_attr(msg, "get_payload")?;
    let payload_val = call_pos(interp, gp, &[])?;
    if payload_val.is_none() {
        return Ok(());
    }
    let Some(mut payload_str) = payload_val.as_str().map(str::to_string) else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "string payload expected: {}",
                crate::interpreter::value_type_name_str(&payload_val)
            ),
        ));
    };

    if let Ok(raw_p) = interp.get_attr(msg, "_payload")
        && has_surrogates(interp, &raw_p)
    {
        let get_param = interp.get_attr(msg, "get_param")?;
        let cs = call_pos(interp, get_param, &[Value::string("charset")])?;
        if !cs.is_none() {
            let copy_mod = interp.load_module("copy")?;
            let deepcopy = interp.get_attr(&copy_mod, "deepcopy")?;
            let cloned = call_pos(interp, deepcopy, std::slice::from_ref(msg))?;
            let delitem = interp.get_attr(&cloned, "__delitem__")?;
            let _ = call_pos(
                interp,
                delitem,
                &[Value::string("content-transfer-encoding")],
            );
            let sp = interp.get_attr(&cloned, "set_payload")?;
            call_pos(interp, sp, &[raw_p, cs])?;
            let gp2 = interp.get_attr(&cloned, "get_payload")?;
            let new_p = call_pos(interp, gp2, &[])?;
            payload_str = new_p.as_str().unwrap_or("").to_string();
            let getitem = interp.get_attr(&cloned, "__getitem__")?;
            let cte = call_pos(
                interp,
                getitem.clone(),
                &[Value::string("content-transfer-encoding")],
            )?;
            let ct = call_pos(interp, getitem, &[Value::string("content-type")])?;
            interp.assign_attr(self_val.clone(), "_munge_cte", Value::tuple(vec![cte, ct]))?;
        }
    }

    if mangle {
        payload_str = mangle_from_lines(&payload_str);
    }
    gen_write_lines(interp, self_val, &payload_str)
}

const DECODED_DEFAULT_FMT: &str =
    "[Non-text (%(type)s) part of message omitted, filename %(filename)s]";

fn all_list() -> Value {
    Value::list(vec![
        Value::string("Generator"),
        Value::string("DecodedGenerator"),
        Value::string("BytesGenerator"),
    ])
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email"),
        "__all__" => all_list(),
        "UNDERSCORE" => Value::string("_"),
        "NL" => Value::string("\n"),
    }

    fn _make_boundary(
        #[default(OptionalArg::Omitted)] text: OptValueRef<'_>,
    ) -> Result<Value> {
        let b = make_boundary_impl(_interp, text.as_value())?;
        Ok(Value::string(b))
    }

    class Generator {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            outfp: PyValue,
            #[default(OptionalArg::Omitted)] mangle_from_: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] maxheaderlen: OptValueRef<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            policy: OptValueRef<'_>,
        ) -> Result<Value> {
            let pol = policy.as_value().cloned().unwrap_or_else(Value::none);
            let mangle = match mangle_from_.as_value() {
                Some(v) if !v.is_none() => _interp.truthy_value(v)?,
                _ => {
                    if pol.is_none() {
                        true
                    } else {
                        let mf = _interp.get_attr(&pol, "mangle_from_")?;
                        _interp.truthy_value(&mf)?
                    }
                }
            };
            let maxhl = maxheaderlen.as_value().cloned().unwrap_or_else(Value::none);
            _interp.assign_attr(self_val.0.clone(), "_fp", outfp.0)?;
            _interp.assign_attr(self_val.0.clone(), "_mangle_from_", Value::bool_(mangle))?;
            _interp.assign_attr(self_val.0.clone(), "maxheaderlen", maxhl)?;
            _interp.assign_attr(self_val.0.clone(), "policy", pol)?;
            Ok(Value::none())
        }

        fn write(#[positional_only] self_val: PyValueRef<'_>, s: PyValue) -> Result<Value> {
            let fp = _interp.get_attr(self_val.0, "_fp")?;
            let write_fn = _interp.get_attr(&fp, "write")?;
            call_pos(_interp, write_fn, &[s.0])
        }

        fn flatten(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
            #[default(false.into())] unixfrom: PyBool,
            #[default(OptionalArg::Omitted)] linesep: OptValueRef<'_>,
        ) -> Result<Value> {
            let old_gen_policy = _interp.get_attr(self_val.0, "policy")?;
            let old_msg_policy = _interp.get_attr(&msg.0, "policy")?;
            let mut pol = if old_gen_policy.is_none() {
                old_msg_policy.clone()
            } else {
                old_gen_policy.clone()
            };
            if let Some(ls) = linesep.as_value()
                && !ls.is_none()
            {
                let clone_fn = _interp.get_attr(&pol, "clone")?;
                pol = call_kw(_interp, clone_fn, &[], &[("linesep", ls.clone())])?;
            }
            let maxhl = _interp.get_attr(self_val.0, "maxheaderlen")?;
            if !maxhl.is_none() {
                let clone_fn = _interp.get_attr(&pol, "clone")?;
                pol = call_kw(_interp, clone_fn, &[], &[("max_line_length", maxhl)])?;
            }
            let nl = _interp.get_attr(&pol, "linesep")?;
            let nl_str = nl.as_str().unwrap_or("\n").to_string();
            let enc_fn = _interp.get_attr(self_val.0, "_encode")?;
            let enc_nl = call_pos(_interp, enc_fn.clone(), std::slice::from_ref(&nl))?;
            let enc_empty = call_pos(_interp, enc_fn, &[Value::string("")])?;
            _interp.assign_attr(self_val.0.clone(), "_NL", nl)?;
            _interp.assign_attr(self_val.0.clone(), "_encoded_NL", enc_nl)?;
            _interp.assign_attr(self_val.0.clone(), "_EMPTY", Value::string(""))?;
            _interp.assign_attr(self_val.0.clone(), "_encoded_EMPTY", enc_empty)?;

            _interp.assign_attr(self_val.0.clone(), "policy", pol.clone())?;
            _interp.assign_attr(msg.0.clone(), "policy", pol)?;
            let run_res = (|| -> Result<()> {
                if unixfrom.0 {
                    let get_uf = _interp.get_attr(&msg.0, "get_unixfrom")?;
                    let uf_val = call_pos(_interp, get_uf, &[])?;
                    let ufrom = if let Some(s) = uf_val.as_str()
                        && !s.is_empty()
                    {
                        s.to_string()
                    } else {
                        let time_mod = _interp.load_module("time")?;
                        let time_fn = _interp.get_attr(&time_mod, "time")?;
                        let now = call_pos(_interp, time_fn, &[])?;
                        let ctime_fn = _interp.get_attr(&time_mod, "ctime")?;
                        let ct = call_pos(_interp, ctime_fn, &[now])?;
                        format!("From nobody {}", ct.as_str().unwrap_or(""))
                    };
                    gen_write(_interp, self_val.0, &format!("{ufrom}{nl_str}"))?;
                }
                gen_write_msg(_interp, self_val.0, &msg.0)
            })();
            let _ = _interp.assign_attr(self_val.0.clone(), "policy", old_gen_policy);
            let _ = _interp.assign_attr(msg.0.clone(), "policy", old_msg_policy);
            run_res?;
            Ok(Value::none())
        }

        fn clone(#[positional_only] self_val: PyValueRef<'_>, fp: PyValue) -> Result<Value> {
            let cls = _interp.get_attr(self_val.0, "__class__")?;
            let mangle = _interp.get_attr(self_val.0, "_mangle_from_")?;
            let policy = _interp.get_attr(self_val.0, "policy")?;
            call_kw(
                _interp,
                cls,
                &[fp.0, mangle, Value::none()],
                &[("policy", policy)],
            )
        }

        fn _new_buffer(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            let io_mod = _interp.load_module("io")?;
            let sio = _interp.get_attr(&io_mod, "StringIO")?;
            call_pos(_interp, sio, &[])
        }

        fn _encode(#[positional_only] _self_val: PyValueRef<'_>, s: PyValue) -> Result<Value> {
            Ok(s.0)
        }

        fn _write_headers(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
        ) -> Result<Value> {
            let is_bytes = is_bytes_generator(_interp, self_val.0);
            gen_write_headers(_interp, self_val.0, &msg.0, is_bytes)?;
            Ok(Value::none())
        }

        fn _dispatch(#[positional_only] self_val: PyValueRef<'_>, msg: PyValue) -> Result<Value> {
            let get_mt = _interp.get_attr(&msg.0, "get_content_maintype")?;
            let mt_val = call_pos(_interp, get_mt, &[])?;
            let main = mt_val.as_str().unwrap_or("text");
            let get_st = _interp.get_attr(&msg.0, "get_content_subtype")?;
            let st_val = call_pos(_interp, get_st, &[])?;
            let sub = st_val.as_str().unwrap_or("plain");

            let specific = format!("_handle_{}_{}", main.replace('-', "_"), sub.replace('-', "_"));
            if let Ok(meth) = _interp.get_attr(self_val.0, &specific)
                && !meth.is_none()
            {
                return call_pos(_interp, meth, &[msg.0]);
            }
            let generic = format!("_handle_{}", main.replace('-', "_"));
            if let Ok(meth) = _interp.get_attr(self_val.0, &generic)
                && !meth.is_none()
            {
                return call_pos(_interp, meth, &[msg.0]);
            }
            let wb = _interp.get_attr(self_val.0, "_writeBody")?;
            call_pos(_interp, wb, &[msg.0])
        }

        fn _handle_text(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
        ) -> Result<Value> {
            let is_bytes = is_bytes_generator(_interp, self_val.0);
            gen_handle_text_impl(_interp, self_val.0, &msg.0, is_bytes)?;
            Ok(Value::none())
        }

        fn _writeBody(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
        ) -> Result<Value> {
            let is_bytes = is_bytes_generator(_interp, self_val.0);
            gen_handle_text_impl(_interp, self_val.0, &msg.0, is_bytes)?;
            Ok(Value::none())
        }

        fn _handle_multipart(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
        ) -> Result<Value> {
            let gp = _interp.get_attr(&msg.0, "get_payload")?;
            let subparts_val = call_pos(_interp, gp, &[])?;
            if let Some(s) = subparts_val.as_str() {
                gen_write(_interp, self_val.0, s)?;
                return Ok(Value::none());
            }
            let subparts: Vec<Value> = if subparts_val.is_none() {
                Vec::new()
            } else if subparts_val.as_list().is_some() {
                _interp.collect_iterable(&subparts_val)?
            } else {
                vec![subparts_val]
            };

            let nl = _interp.get_attr(self_val.0, "_NL")?;
            let nl_str = nl.as_str().unwrap_or("\n").to_string();
            let enc_nl = _interp.get_attr(self_val.0, "_encoded_NL")?;
            let mut msgtexts: Vec<Value> = Vec::with_capacity(subparts.len());

            for part in subparts {
                let new_buf = _interp.get_attr(self_val.0, "_new_buffer")?;
                let s = call_pos(_interp, new_buf, &[])?;
                let clone_fn = _interp.get_attr(self_val.0, "clone")?;
                let g = call_pos(_interp, clone_fn, std::slice::from_ref(&s))?;
                let flat_fn = _interp.get_attr(&g, "flatten")?;
                call_kw(
                    _interp,
                    flat_fn,
                    &[part],
                    &[("unixfrom", Value::bool_(false)), ("linesep", nl.clone())],
                )?;
                let gv = _interp.get_attr(&s, "getvalue")?;
                msgtexts.push(call_pos(_interp, gv, &[])?);
            }

            let get_b = _interp.get_attr(&msg.0, "get_boundary")?;
            let b_val = call_pos(_interp, get_b, &[])?;
            let boundary = if let Some(b) = b_val.as_str()
                && !b.is_empty()
            {
                b.to_string()
            } else {
                let join_fn = _interp.get_attr(&enc_nl, "join")?;
                let alltext = call_pos(_interp, join_fn, &[Value::list(msgtexts.clone())])?;
                let b = make_boundary_impl(_interp, Some(&alltext))?;
                let set_b = _interp.get_attr(&msg.0, "set_boundary")?;
                call_pos(_interp, set_b, &[Value::string(&b)])?;
                b
            };

            let mangle = _interp
                .get_attr(self_val.0, "_mangle_from_")
                .and_then(|v| _interp.truthy_value(&v))
                .unwrap_or(false);

            let preamble = _interp.get_attr(&msg.0, "preamble")?;
            if let Some(p) = preamble.as_str() {
                let out_p = if mangle {
                    mangle_from_lines(p)
                } else {
                    p.to_string()
                };
                gen_write_lines(_interp, self_val.0, &out_p)?;
                gen_write(_interp, self_val.0, &nl_str)?;
            }

            gen_write(_interp, self_val.0, &format!("--{boundary}{nl_str}"))?;
            let mut iter = msgtexts.into_iter();
            if let Some(first) = iter.next() {
                gen_fp_write(_interp, self_val.0, first)?;
            }
            for part_body in iter {
                gen_write(
                    _interp,
                    self_val.0,
                    &format!("{nl_str}--{boundary}{nl_str}"),
                )?;
                gen_fp_write(_interp, self_val.0, part_body)?;
            }
            gen_write(
                _interp,
                self_val.0,
                &format!("{nl_str}--{boundary}--{nl_str}"),
            )?;

            let epilogue = _interp.get_attr(&msg.0, "epilogue")?;
            if let Some(e) = epilogue.as_str() {
                let out_e = if mangle {
                    mangle_from_lines(e)
                } else {
                    e.to_string()
                };
                gen_write_lines(_interp, self_val.0, &out_e)?;
            }
            Ok(Value::none())
        }

        fn _handle_multipart_signed(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
        ) -> Result<Value> {
            let p = _interp.get_attr(self_val.0, "policy")?;
            let clone_fn = _interp.get_attr(&p, "clone")?;
            let p0 = call_kw(_interp, clone_fn, &[], &[("max_line_length", Value::int(0))])?;
            _interp.assign_attr(self_val.0.clone(), "policy", p0)?;
            let hm = _interp.get_attr(self_val.0, "_handle_multipart")?;
            let res = call_pos(_interp, hm, &[msg.0]);
            let _ = _interp.assign_attr(self_val.0.clone(), "policy", p);
            res
        }

        fn _handle_message_delivery_status(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
        ) -> Result<Value> {
            let nl = _interp.get_attr(self_val.0, "_NL")?;
            let enc_nl = _interp.get_attr(self_val.0, "_encoded_NL")?;
            let gp = _interp.get_attr(&msg.0, "get_payload")?;
            let payload_val = call_pos(_interp, gp, &[])?;
            let parts = _interp.collect_iterable(&payload_val)?;
            let mut blocks: Vec<Value> = Vec::with_capacity(parts.len());
            for part in parts {
                let new_buf = _interp.get_attr(self_val.0, "_new_buffer")?;
                let s = call_pos(_interp, new_buf, &[])?;
                let clone_fn = _interp.get_attr(self_val.0, "clone")?;
                let g = call_pos(_interp, clone_fn, std::slice::from_ref(&s))?;
                let flat_fn = _interp.get_attr(&g, "flatten")?;
                call_kw(
                    _interp,
                    flat_fn,
                    &[part],
                    &[("unixfrom", Value::bool_(false)), ("linesep", nl.clone())],
                )?;
                let gv = _interp.get_attr(&s, "getvalue")?;
                let text = call_pos(_interp, gv, &[])?;
                let split_fn = _interp.get_attr(&text, "split")?;
                let lines_val = call_pos(_interp, split_fn, std::slice::from_ref(&enc_nl))?;
                let mut lines = _interp.collect_iterable(&lines_val)?;
                if let Some(last) = lines.last()
                    && (last.as_str() == Some("")
                        || matches!(last.kind(), ValueKind::Bytes(b) if b.is_empty()))
                {
                    lines.pop();
                    let join_fn = _interp.get_attr(&enc_nl, "join")?;
                    blocks.push(call_pos(_interp, join_fn, &[Value::list(lines)])?);
                } else {
                    blocks.push(text);
                }
            }
            let join_fn = _interp.get_attr(&enc_nl, "join")?;
            let joined = call_pos(_interp, join_fn, &[Value::list(blocks)])?;
            gen_fp_write(_interp, self_val.0, joined)?;
            Ok(Value::none())
        }

        fn _handle_message(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
        ) -> Result<Value> {
            let nl = _interp.get_attr(self_val.0, "_NL")?;
            let raw_payload = _interp.get_attr(&msg.0, "_payload")?;
            if raw_payload.as_list().is_some() {
                let new_buf = _interp.get_attr(self_val.0, "_new_buffer")?;
                let s = call_pos(_interp, new_buf, &[])?;
                let clone_fn = _interp.get_attr(self_val.0, "clone")?;
                let g = call_pos(_interp, clone_fn, std::slice::from_ref(&s))?;
                let gp = _interp.get_attr(&msg.0, "get_payload")?;
                let inner = call_pos(_interp, gp, &[Value::int(0)])?;
                let flat_fn = _interp.get_attr(&g, "flatten")?;
                call_kw(
                    _interp,
                    flat_fn,
                    &[inner],
                    &[("unixfrom", Value::bool_(false)), ("linesep", nl)],
                )?;
                let gv = _interp.get_attr(&s, "getvalue")?;
                let rendered = call_pos(_interp, gv, &[])?;
                gen_fp_write(_interp, self_val.0, rendered)?;
            } else {
                let enc_fn = _interp.get_attr(self_val.0, "_encode")?;
                let rendered = call_pos(_interp, enc_fn, &[raw_payload])?;
                gen_fp_write(_interp, self_val.0, rendered)?;
            }
            Ok(Value::none())
        }

        #[classmethod]
        fn _make_boundary(
            #[positional_only] _cls: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] text: OptValueRef<'_>,
        ) -> Result<Value> {
            let b = make_boundary_impl(_interp, text.as_value())?;
            Ok(Value::string(b))
        }
    }

    class BytesGenerator("Generator") {
        fn write(#[positional_only] self_val: PyValueRef<'_>, s: PyValue) -> Result<Value> {
            let enc_fn = _interp.get_attr(&s.0, "encode")?;
            let encoded = call_pos(
                _interp,
                enc_fn,
                &[Value::string("ascii"), Value::string("surrogateescape")],
            )?;
            let fp = _interp.get_attr(self_val.0, "_fp")?;
            let write_fn = _interp.get_attr(&fp, "write")?;
            call_pos(_interp, write_fn, &[encoded])
        }

        fn _new_buffer(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            let io_mod = _interp.load_module("io")?;
            let bio = _interp.get_attr(&io_mod, "BytesIO")?;
            call_pos(_interp, bio, &[])
        }

        fn _encode(#[positional_only] _self_val: PyValueRef<'_>, s: PyStr<'_>) -> Result<Value> {
            let s_val = Value::string(s.0.as_ref());
            let enc_fn = _interp.get_attr(&s_val, "encode")?;
            call_pos(_interp, enc_fn, &[Value::string("ascii")])
        }
    }

    class DecodedGenerator("Generator") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            outfp: PyValue,
            #[default(OptionalArg::Omitted)] mangle_from_: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] maxheaderlen: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] fmt: OptValueRef<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            policy: OptValueRef<'_>,
        ) -> Result<Value> {
            let gen_mod = _interp.load_module("email.generator")?;
            let gen_cls = _interp.get_attr(&gen_mod, "Generator")?;
            let init_fn = _interp.get_attr(&gen_cls, "__init__")?;
            let mf = mangle_from_.as_value().cloned().unwrap_or_else(Value::none);
            let mhl = maxheaderlen.as_value().cloned().unwrap_or_else(Value::none);
            let pol = policy.as_value().cloned().unwrap_or_else(Value::none);
            call_kw(
                _interp,
                init_fn,
                &[self_val.0.clone(), outfp.0, mf, mhl],
                &[("policy", pol)],
            )?;
            let fmt_val = match fmt.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => Value::string(DECODED_DEFAULT_FMT),
            };
            _interp.assign_attr(self_val.0.clone(), "_fmt", fmt_val)?;
            Ok(Value::none())
        }

        fn _dispatch(#[positional_only] self_val: PyValueRef<'_>, msg: PyValue) -> Result<Value> {
            let walk_fn = _interp.get_attr(&msg.0, "walk")?;
            let walked = call_pos(_interp, walk_fn, &[])?;
            let parts = _interp.collect_iterable(&walked)?;
            let fmt = _interp.get_attr(self_val.0, "_fmt")?;
            let print_fn = Value::builtin_function("print");
            for part in parts {
                let get_mt = _interp.get_attr(&part, "get_content_maintype")?;
                let mt = call_pos(_interp, get_mt, &[])?;
                match mt.as_str() {
                    Some("text") => {
                        let gp = _interp.get_attr(&part, "get_payload")?;
                        let payload = call_kw(_interp, gp, &[], &[("decode", Value::bool_(false))])?;
                        call_kw(
                            _interp,
                            print_fn.clone(),
                            &[payload],
                            &[("file", self_val.0.clone())],
                        )?;
                    }
                    Some("multipart") => {}
                    _ => {
                        let get_ct = _interp.get_attr(&part, "get_content_type")?;
                        let ct = call_pos(_interp, get_ct, &[])?;
                        let get_st = _interp.get_attr(&part, "get_content_subtype")?;
                        let st = call_pos(_interp, get_st, &[])?;
                        let get_fn = _interp.get_attr(&part, "get_filename")?;
                        let fname = call_pos(_interp, get_fn, &[Value::string("[no filename]")])?;
                        let get_hdr = _interp.get_attr(&part, "get")?;
                        let desc = call_pos(
                            _interp,
                            get_hdr.clone(),
                            &[
                                Value::string("Content-Description"),
                                Value::string("[no description]"),
                            ],
                        )?;
                        let cte = call_pos(
                            _interp,
                            get_hdr,
                            &[
                                Value::string("Content-Transfer-Encoding"),
                                Value::string("[no encoding]"),
                            ],
                        )?;
                        let mut d = PyDict::default();
                        d.insert(PyKey::str_from("type"), ct);
                        d.insert(PyKey::str_from("maintype"), mt);
                        d.insert(PyKey::str_from("subtype"), st);
                        d.insert(PyKey::str_from("filename"), fname);
                        d.insert(PyKey::str_from("description"), desc);
                        d.insert(PyKey::str_from("encoding"), cte);
                        let mod_fn = _interp.get_attr(&fmt, "__mod__")?;
                        let formatted = call_pos(_interp, mod_fn, &[Value::dict(d)])?;
                        call_kw(
                            _interp,
                            print_fn.clone(),
                            &[formatted],
                            &[("file", self_val.0.clone())],
                        )?;
                    }
                }
            }
            Ok(Value::none())
        }
    }
}
