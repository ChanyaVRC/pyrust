// `shelve` module — 100% native Rust implementation of Python 3.12's `shelve`
// persistent pickled-object dictionary (issue #3259).
//
// Provides `_ClosedDict`, `Shelf`, `BsdDbShelf`, `DbfilenameShelf`, and `open`
// with full `collections.abc.MutableMapping` compatibility, `writeback=True`
// caching + `.sync()` / `.close()`, custom `keyencoding`, and context manager
// protocol.
//
// Reference: <https://docs.python.org/3/library/shelve.html>

use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, NativeIterFrame};
use crate::value::{PyDict, Value, ValueKind};

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(
    interp: &mut Interpreter,
    obj: &Value,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    let f = interp.get_attr(obj, method)?;
    call_pos(interp, f, args)
}

fn encode_key(interp: &mut Interpreter, self_val: &Value, key: &Value) -> Result<Value> {
    let enc = interp.get_attr(self_val, "keyencoding")?;
    call_method(interp, key, "encode", &[enc])
}

fn decode_key(interp: &mut Interpreter, self_val: &Value, key: &Value) -> Result<Value> {
    let enc = interp.get_attr(self_val, "keyencoding")?;
    call_method(interp, key, "decode", &[enc])
}

fn pickle_dumps(interp: &mut Interpreter, value: Value, protocol: Value) -> Result<Value> {
    let pickle_mod = interp.load_module("pickle")?;
    let dumps_fn = interp.get_attr(&pickle_mod, "dumps")?;
    interp.call_function_expanded(
        dumps_fn,
        &[
            ExpandedCallArg { name: None, value },
            ExpandedCallArg {
                name: Some("protocol".to_string()),
                value: protocol,
            },
        ],
    )
}

fn pickle_loads(interp: &mut Interpreter, raw: Value) -> Result<Value> {
    let pickle_mod = interp.load_module("pickle")?;
    let loads_fn = interp.get_attr(&pickle_mod, "loads")?;
    call_pos(interp, loads_fn, &[raw])
}

fn dict_contains(interp: &mut Interpreter, mapping: &Value, key: &Value) -> Result<bool> {
    let res = interp.eval_binary(key.clone(), crate::ast::BinaryOp::In, mapping.clone())?;
    interp.truthy_value(&res)
}

fn decode_bsddb_pair(interp: &mut Interpreter, self_val: &Value, pair: Value) -> Result<Value> {
    let elems = interp.collect_iterable(&pair)?;
    if elems.len() != 2 {
        return Err(PyError::named(
            "ValueError",
            format!("expected 2 values to unpack, got {}", elems.len()),
        ));
    }
    let dec_k = decode_key(interp, self_val, &elems[0])?;
    let val = pickle_loads(interp, elems[1].clone())?;
    Ok(Value::tuple(vec![dec_k, val]))
}

/// Wire `_ClosedDict` and `Shelf` to inherit from `collections.abc.MutableMapping`.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };
    if let Ok(abc_mod) = interp.load_module("collections.abc")
        && let Ok(mm_val) = interp.get_attr(&abc_mod, "MutableMapping")
        && let ValueKind::PyClass(mm_cls) = mm_val.kind()
    {
        for name in ["_ClosedDict", "Shelf"] {
            if let Some(cls_val) = mod_rc.borrow().attrs.get(name).cloned()
                && let ValueKind::PyClass(cls_rc) = cls_val.kind()
            {
                crate::builtin_modules::macros::link_class_bases(cls_rc, vec![Rc::clone(mm_cls)]);
            }
        }
    }
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("Shelf"),
            Value::string("BsdDbShelf"),
            Value::string("DbfilenameShelf"),
            Value::string("open"),
        ]),
    }

    class _ClosedDict {
        fn closed(
            #[positional_only] _self: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            Err(PyError::named(
                "ValueError",
                "invalid operation on closed shelf",
            ))
        }

        fn __iter__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named(
                "ValueError",
                "invalid operation on closed shelf",
            ))
        }

        fn __len__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named(
                "ValueError",
                "invalid operation on closed shelf",
            ))
        }

        fn __getitem__(
            #[positional_only] _self: PyValueRef<'_>,
            _key: PyValue,
        ) -> Result<Value> {
            Err(PyError::named(
                "ValueError",
                "invalid operation on closed shelf",
            ))
        }

        fn __setitem__(
            #[positional_only] _self: PyValueRef<'_>,
            _key: PyValue,
            _val: PyValue,
        ) -> Result<Value> {
            Err(PyError::named(
                "ValueError",
                "invalid operation on closed shelf",
            ))
        }

        fn __delitem__(
            #[positional_only] _self: PyValueRef<'_>,
            _key: PyValue,
        ) -> Result<Value> {
            Err(PyError::named(
                "ValueError",
                "invalid operation on closed shelf",
            ))
        }

        fn keys(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named(
                "ValueError",
                "invalid operation on closed shelf",
            ))
        }

        fn __repr__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::string("<Closed Dictionary>"))
        }
    }

    class Shelf {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            dict: PyValue,
            #[default(OptionalArg::Omitted)] protocol: OptValueRef<'_>,
            #[default(false.into())] writeback: PyBool,
            #[default("utf-8".into())] keyencoding: PyStr<'_>,
        ) -> Result<Value> {
            let proto = match protocol.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => {
                    let pickle_mod = _interp.load_module("pickle")?;
                    _interp.get_attr(&pickle_mod, "DEFAULT_PROTOCOL")?
                }
            };
            _interp.assign_attr(self_val.0.clone(), "dict", dict.0)?;
            _interp.assign_attr(self_val.0.clone(), "_protocol", proto)?;
            _interp.assign_attr(self_val.0.clone(), "writeback", Value::bool_(*writeback))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "cache",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "keyencoding",
                Value::string(&*keyencoding),
            )?;
            Ok(Value::none())
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let dict = _interp.get_attr(self_val.0, "dict")?;
            let keys_val = call_method(_interp, &dict, "keys", &[])?;
            let raw_keys = _interp.collect_iterable(&keys_val)?;
            let mut decoded = Vec::with_capacity(raw_keys.len());
            for k in &raw_keys {
                decoded.push(decode_key(_interp, self_val.0, k)?);
            }
            Ok(Value::generator(Box::new(NativeIterFrame::generator(
                decoded,
            ))))
        }

        fn __len__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let dict = _interp.get_attr(self_val.0, "dict")?;
            call_pos(_interp, Value::builtin_function("len"), &[dict])
        }

        fn __contains__(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
        ) -> Result<Value> {
            let dict = _interp.get_attr(self_val.0, "dict")?;
            let enc_key = encode_key(_interp, self_val.0, &key.0)?;
            Ok(Value::bool_(dict_contains(_interp, &dict, &enc_key)?))
        }

        fn get(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
            #[default(OptionalArg::Omitted)] default: OptValueRef<'_>,
        ) -> Result<Value> {
            let dict = _interp.get_attr(self_val.0, "dict")?;
            let enc_key = encode_key(_interp, self_val.0, &key.0)?;
            if dict_contains(_interp, &dict, &enc_key)? {
                call_method(_interp, self_val.0, "__getitem__", &[key.0])
            } else {
                Ok(default.as_value().cloned().unwrap_or_else(Value::none))
            }
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
        ) -> Result<Value> {
            let cache = _interp.get_attr(self_val.0, "cache")?;
            if dict_contains(_interp, &cache, &key.0)? {
                return _interp.eval_index(&cache, key.0);
            }
            let dict = _interp.get_attr(self_val.0, "dict")?;
            let enc_key = encode_key(_interp, self_val.0, &key.0)?;
            let raw = _interp.eval_index(&dict, enc_key)?;
            let value = pickle_loads(_interp, raw)?;
            let wb = _interp.get_attr(self_val.0, "writeback")?;
            if _interp.truthy_value(&wb)? {
                call_method(_interp, &cache, "__setitem__", &[key.0, value.clone()])?;
            }
            Ok(value)
        }

        fn __setitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
            value: PyValue,
        ) -> Result<Value> {
            let wb = _interp.get_attr(self_val.0, "writeback")?;
            if _interp.truthy_value(&wb)? {
                let cache = _interp.get_attr(self_val.0, "cache")?;
                call_method(
                    _interp,
                    &cache,
                    "__setitem__",
                    &[key.0.clone(), value.0.clone()],
                )?;
            }
            let proto = _interp.get_attr(self_val.0, "_protocol")?;
            let raw = pickle_dumps(_interp, value.0, proto)?;
            let dict = _interp.get_attr(self_val.0, "dict")?;
            let enc_key = encode_key(_interp, self_val.0, &key.0)?;
            call_method(_interp, &dict, "__setitem__", &[enc_key, raw])?;
            Ok(Value::none())
        }

        fn __delitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
        ) -> Result<Value> {
            let dict = _interp.get_attr(self_val.0, "dict")?;
            let enc_key = encode_key(_interp, self_val.0, &key.0)?;
            call_method(_interp, &dict, "__delitem__", &[enc_key])?;
            let cache = _interp.get_attr(self_val.0, "cache")?;
            if dict_contains(_interp, &cache, &key.0)? {
                let _ = call_method(_interp, &cache, "__delitem__", &[key.0]);
            }
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "close", &[])?;
            Ok(Value::none())
        }

        fn sync(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let wb = _interp.get_attr(self_val.0, "writeback")?;
            let cache = _interp.get_attr(self_val.0, "cache")?;
            if _interp.truthy_value(&wb)? && _interp.truthy_value(&cache)? {
                _interp.assign_attr(self_val.0.clone(), "writeback", Value::bool_(false))?;
                let items_val = call_method(_interp, &cache, "items", &[])?;
                let pairs = _interp.collect_iterable(&items_val)?;
                let mut write_res = Ok(());
                for pair in pairs {
                    let kv = match _interp.collect_iterable(&pair) {
                        Ok(v) if v.len() == 2 => v,
                        Ok(_) => continue,
                        Err(e) => {
                            write_res = Err(e);
                            break;
                        }
                    };
                    if let Err(e) = call_method(
                        _interp,
                        self_val.0,
                        "__setitem__",
                        &[kv[0].clone(), kv[1].clone()],
                    ) {
                        write_res = Err(e);
                        break;
                    }
                }
                _interp.assign_attr(self_val.0.clone(), "writeback", Value::bool_(true))?;
                write_res?;
                _interp.assign_attr(
                    self_val.0.clone(),
                    "cache",
                    Value::dict(PyDict::default()),
                )?;
            }
            let dict = _interp.get_attr(self_val.0, "dict")?;
            if let Ok(sync_fn) = _interp.get_attr(&dict, "sync") {
                call_pos(_interp, sync_fn, &[])?;
            }
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let dict = _interp.get_attr(self_val.0, "dict")?;
            if dict.is_none() {
                return Ok(Value::none());
            }
            let close_res = (|| -> Result<()> {
                call_method(_interp, self_val.0, "sync", &[])?;
                let cur_dict = _interp.get_attr(self_val.0, "dict")?;
                if let Ok(close_fn) = _interp.get_attr(&cur_dict, "close") {
                    call_pos(_interp, close_fn, &[])?;
                }
                Ok(())
            })();
            if let Ok(shelve_mod) = _interp.load_module("shelve")
                && let Ok(closed_cls) = _interp.get_attr(&shelve_mod, "_ClosedDict")
                && let Ok(closed_inst) = call_pos(_interp, closed_cls, &[])
            {
                let _ = _interp.assign_attr(self_val.0.clone(), "dict", closed_inst);
            } else {
                let _ = _interp.assign_attr(self_val.0.clone(), "dict", Value::none());
            }
            close_res?;
            Ok(Value::none())
        }
    }

    class BsdDbShelf : "Shelf" {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            dict: PyValue,
            #[default(OptionalArg::Omitted)] protocol: OptValueRef<'_>,
            #[default(false.into())] writeback: PyBool,
            #[default("utf-8".into())] keyencoding: PyStr<'_>,
        ) -> Result<Value> {
            let shelve_mod = _interp.load_module("shelve")?;
            let shelf_cls = _interp.get_attr(&shelve_mod, "Shelf")?;
            let shelf_init = _interp.get_attr(&shelf_cls, "__init__")?;
            call_pos(
                _interp,
                shelf_init,
                &[
                    self_val.0.clone(),
                    dict.0,
                    protocol.as_value().cloned().unwrap_or_else(Value::none),
                    Value::bool_(*writeback),
                    Value::string(&*keyencoding),
                ],
            )
        }

        fn set_location(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
        ) -> Result<Value> {
            let dict = _interp.get_attr(self_val.0, "dict")?;
            let pair = call_method(_interp, &dict, "set_location", &[key.0])?;
            decode_bsddb_pair(_interp, self_val.0, pair)
        }

        fn next(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let dict = _interp.get_attr(self_val.0, "dict")?;
            let pair = call_pos(_interp, Value::builtin_function("next"), &[dict])?;
            decode_bsddb_pair(_interp, self_val.0, pair)
        }

        fn previous(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let dict = _interp.get_attr(self_val.0, "dict")?;
            let pair = call_method(_interp, &dict, "previous", &[])?;
            decode_bsddb_pair(_interp, self_val.0, pair)
        }

        fn first(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let dict = _interp.get_attr(self_val.0, "dict")?;
            let pair = call_method(_interp, &dict, "first", &[])?;
            decode_bsddb_pair(_interp, self_val.0, pair)
        }

        fn last(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let dict = _interp.get_attr(self_val.0, "dict")?;
            let pair = call_method(_interp, &dict, "last", &[])?;
            decode_bsddb_pair(_interp, self_val.0, pair)
        }
    }

    class DbfilenameShelf : "Shelf" {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyValue,
            #[default("c".into())] flag: PyStr<'_>,
            #[default(OptionalArg::Omitted)] protocol: OptValueRef<'_>,
            #[default(false.into())] writeback: PyBool,
        ) -> Result<Value> {
            let dbm_mod = _interp.load_module("dbm")?;
            let dbm_open = _interp.get_attr(&dbm_mod, "open")?;
            let db = call_pos(_interp, dbm_open, &[filename.0, Value::string(&*flag)])?;
            let shelve_mod = _interp.load_module("shelve")?;
            let shelf_cls = _interp.get_attr(&shelve_mod, "Shelf")?;
            let shelf_init = _interp.get_attr(&shelf_cls, "__init__")?;
            call_pos(
                _interp,
                shelf_init,
                &[
                    self_val.0.clone(),
                    db,
                    protocol.as_value().cloned().unwrap_or_else(Value::none),
                    Value::bool_(*writeback),
                ],
            )
        }

        fn clear(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cache = _interp.get_attr(self_val.0, "cache")?;
            call_method(_interp, &cache, "clear", &[])?;
            let dict = _interp.get_attr(self_val.0, "dict")?;
            call_method(_interp, &dict, "clear", &[])?;
            Ok(Value::none())
        }
    }

    fn open(
        filename: PyValue,
        #[default("c".into())] flag: PyStr<'_>,
        #[default(OptionalArg::Omitted)] protocol: OptValueRef<'_>,
        #[default(false.into())] writeback: PyBool,
    ) -> Result<Value> {
        let shelve_mod = _interp.load_module("shelve")?;
        let cls = _interp.get_attr(&shelve_mod, "DbfilenameShelf")?;
        call_pos(
            _interp,
            cls,
            &[
                filename.0,
                Value::string(&*flag),
                protocol.as_value().cloned().unwrap_or_else(Value::none),
                Value::bool_(*writeback),
            ],
        )
    }
}
