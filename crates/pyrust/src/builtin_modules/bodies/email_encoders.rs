// `email.encoders` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// Encoders for MIME message payloads: `encode_base64`, `encode_quopri`,
// `encode_7or8bit`, `encode_noop`.
//
// Reference: <https://docs.python.org/3/library/email.encoders.html>

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::builtin_args::{FromValue, PyBytesLike, PyValueRef};
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

fn bencode_bytes(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut out = String::new();
    // base64.encodebytes uses 57 raw bytes (76 base64 chars) per line + '\n'.
    for chunk in data.chunks(57) {
        out.push_str(&super::email_base64mime::b64_encode_bytes(chunk));
        out.push('\n');
    }
    out.into_bytes()
}

fn qencode_bytes(interp: &mut Interpreter, data: &[u8]) -> Result<Vec<u8>> {
    let quopri = interp.load_module("quopri")?;
    let enc_fn = interp.get_attr(&quopri, "encodestring")?;
    let res = call_kw(
        interp,
        enc_fn,
        &[Value::bytes(data.to_vec())],
        &[("quotetabs", Value::bool_(true))],
    )?;
    let bl = PyBytesLike::try_from_value_with_interp(interp, &res, "_qencode", "s")?;
    let raw = bl.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    for &b in raw {
        if b == b' ' {
            out.extend_from_slice(b"=20");
        } else {
            out.push(b);
        }
    }
    Ok(out)
}

fn all_list() -> Value {
    Value::list(vec![
        Value::string("encode_7or8bit"),
        Value::string("encode_base64"),
        Value::string("encode_noop"),
        Value::string("encode_quopri"),
    ])
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email"),
        "__all__" => all_list(),
    }

    fn _bencode(s: PyBytesLike<'_>) -> Result<Value> {
        Ok(Value::bytes(bencode_bytes(s.as_bytes())))
    }

    fn _qencode(s: PyBytesLike<'_>) -> Result<Value> {
        Ok(Value::bytes(qencode_bytes(_interp, s.as_bytes())?))
    }

    fn encode_base64(msg: PyValueRef<'_>) -> Result<Value> {
        let get_payload = _interp.get_attr(msg.0, "get_payload")?;
        let orig = call_kw(_interp, get_payload, &[], &[("decode", Value::bool_(true))])?;
        let bl = PyBytesLike::try_from_value_with_interp(_interp, &orig, FN_NAME, "payload")?;
        let enc_bytes = bencode_bytes(bl.as_bytes());
        let enc_str = String::from_utf8(enc_bytes).unwrap_or_default();
        let set_payload = _interp.get_attr(msg.0, "set_payload")?;
        call_pos(_interp, set_payload, &[Value::string(enc_str)])?;
        let setitem = _interp.get_attr(msg.0, "__setitem__")?;
        call_pos(
            _interp,
            setitem,
            &[
                Value::string("Content-Transfer-Encoding"),
                Value::string("base64"),
            ],
        )?;
        Ok(Value::none())
    }

    fn encode_quopri(msg: PyValueRef<'_>) -> Result<Value> {
        let get_payload = _interp.get_attr(msg.0, "get_payload")?;
        let orig = call_kw(_interp, get_payload, &[], &[("decode", Value::bool_(true))])?;
        let bl = PyBytesLike::try_from_value_with_interp(_interp, &orig, FN_NAME, "payload")?;
        let enc_bytes = qencode_bytes(_interp, bl.as_bytes())?;
        let set_payload = _interp.get_attr(msg.0, "set_payload")?;
        call_pos(_interp, set_payload, &[Value::bytes(enc_bytes)])?;
        let setitem = _interp.get_attr(msg.0, "__setitem__")?;
        call_pos(
            _interp,
            setitem,
            &[
                Value::string("Content-Transfer-Encoding"),
                Value::string("quoted-printable"),
            ],
        )?;
        Ok(Value::none())
    }

    fn encode_7or8bit(msg: PyValueRef<'_>) -> Result<Value> {
        let get_payload = _interp.get_attr(msg.0, "get_payload")?;
        let orig = call_kw(_interp, get_payload, &[], &[("decode", Value::bool_(true))])?;
        let cte = if orig.is_none() {
            "7bit"
        } else {
            let bl = PyBytesLike::try_from_value_with_interp(_interp, &orig, FN_NAME, "payload")?;
            if bl.as_bytes().iter().all(|&b| b < 128) {
                "7bit"
            } else {
                "8bit"
            }
        };
        let setitem = _interp.get_attr(msg.0, "__setitem__")?;
        call_pos(
            _interp,
            setitem,
            &[
                Value::string("Content-Transfer-Encoding"),
                Value::string(cte),
            ],
        )?;
        Ok(Value::none())
    }

    fn encode_noop(_msg: PyValueRef<'_>) -> Result<Value> {
        Ok(Value::none())
    }
}
