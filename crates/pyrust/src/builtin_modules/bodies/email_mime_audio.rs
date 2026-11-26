// `email.mime.audio` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// `MIMEAudio`: class for generating `audio/*` MIME documents with header sniffing.
//
// Reference: <https://docs.python.org/3/library/email.mime.html>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBytesLike, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value, ValueKind};

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn sniff_audio_subtype(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"FORM")
        && data.len() >= 12
        && (&data[8..12] == b"AIFC" || &data[8..12] == b"AIFF")
    {
        return Some("x-aiff");
    }
    if data.starts_with(b".snd") {
        return Some("basic");
    }
    if data.starts_with(b"RIFF")
        && data.len() >= 16
        && &data[8..12] == b"WAVE"
        && &data[12..16] == b"fmt "
    {
        return Some("x-wav");
    }
    None
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let nm_mod = interp.load_module("email.mime.nonmultipart")?;
    let nm_cls_val = interp.get_attr(&nm_mod, "MIMENonMultipart")?;
    let ValueKind::PyClass(nm_cls) = nm_cls_val.kind() else {
        return Err(PyError::runtime(
            "email.mime.nonmultipart.MIMENonMultipart is not a class".to_string(),
        ));
    };
    let cls_val = module.borrow().attrs.get("MIMEAudio").cloned();
    if let Some(val) = cls_val
        && let ValueKind::PyClass(cls_rc) = val.kind()
    {
        link_class_bases(cls_rc, vec![Rc::clone(nm_cls)]);
    }
    Ok(())
}

fn all_list() -> Value {
    Value::list(vec![Value::string("MIMEAudio")])
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email.mime"),
        "__all__" => all_list(),
    }

    fn _what(data: PyBytesLike<'_>) -> Result<Value> {
        Ok(match sniff_audio_subtype(data.as_bytes()) {
            Some(s) => Value::string(s),
            None => Value::none(),
        })
    }

    class MIMEAudio {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            _audiodata: PyValue,
            #[default(OptionalArg::Omitted)] _subtype: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _encoder: OptValueRef<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            policy: OptValueRef<'_>,
            #[varkwargs] _params: IndexMap<String, Value>,
        ) -> Result<Value> {
            let subtype = match _subtype.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => {
                    let guessed = if let Ok(bl) = PyBytesLike::try_from_value_with_interp(
                        _interp,
                        &_audiodata.0,
                        FN_NAME,
                        "_audiodata",
                    ) {
                        sniff_audio_subtype(bl.as_bytes())
                    } else {
                        None
                    };
                    match guessed {
                        Some(s) => Value::string(s),
                        None => {
                            return Err(PyError::named(
                                "TypeError",
                                "Could not find audio MIME subtype".to_string(),
                            ));
                        }
                    }
                }
            };
            let encoder = match _encoder.as_value() {
                Some(v) => v.clone(),
                None => {
                    let enc_mod = _interp.load_module("email.encoders")?;
                    _interp.get_attr(&enc_mod, "encode_base64")?
                }
            };
            let nm_mod = _interp.load_module("email.mime.nonmultipart")?;
            let nm_cls = _interp.get_attr(&nm_mod, "MIMENonMultipart")?;
            let nm_init = _interp.get_attr(&nm_cls, "__init__")?;
            let mut expanded = vec![
                ExpandedCallArg {
                    name: None,
                    value: self_val.0.clone(),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::string("audio"),
                },
                ExpandedCallArg {
                    name: None,
                    value: subtype,
                },
                ExpandedCallArg {
                    name: Some("policy".to_string()),
                    value: policy.as_value().cloned().unwrap_or_else(Value::none),
                },
            ];
            for (k, v) in _params {
                expanded.push(ExpandedCallArg {
                    name: Some(k),
                    value: v,
                });
            }
            _interp.call_function_expanded(nm_init, &expanded)?;
            let set_payload = _interp.get_attr(self_val.0, "set_payload")?;
            call_pos(_interp, set_payload, &[_audiodata.0])?;
            call_pos(_interp, encoder, std::slice::from_ref(self_val.0))?;
            Ok(Value::none())
        }
    }
}
