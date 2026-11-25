// `_compression` internal module — 100% native-Rust implementation of Python
// 3.12's `_compression` helper module used by `gzip`, `bz2`, and `lzma`
// (issue #3250).
//
// Exposes `BUFFER_SIZE`, `BaseStream(io.BufferedIOBase)`, and
// `DecompressReader(io.RawIOBase)`.
//
// Reference: CPython 3.12 `Lib/_compression.py`

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyIndex, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

const BUFFER_SIZE: i64 = 8192;

fn call_positional(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_with_kwargs(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kwargs_dict: &Value,
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    if let Some(dict) = kwargs_dict.as_dict() {
        for (k, v) in dict.iter() {
            if let PyKey::Str(s) = k
                && let Some(name) = s.as_str()
            {
                expanded.push(ExpandedCallArg {
                    name: Some(name.to_string()),
                    value: v.clone(),
                });
            }
        }
    }
    interp.call_function_expanded(callee, &expanded)
}

fn raise_unsupported(interp: &mut Interpreter, msg: &str) -> Result<Value> {
    let io_mod = interp.load_module("io")?;
    let exc_cls = interp.get_attr(&io_mod, "UnsupportedOperation")?;
    let exc = call_positional(interp, exc_cls, &[Value::string(msg)])?;
    Err(PyError::raised(exc))
}

fn is_matching_exception(interp: &mut Interpreter, err: &PyError, exc_spec: &Value) -> bool {
    if let ValueKind::Tuple(items) = exc_spec.kind()
        && items.is_empty()
    {
        return false;
    }
    let Ok(exc_val) = interp.materialize_pyerror(err.clone()) else {
        return false;
    };
    call_positional(
        interp,
        Value::builtin_function("isinstance"),
        &[exc_val, exc_spec.clone()],
    )
    .ok()
    .and_then(|v| interp.truthy_value(&v).ok())
    .unwrap_or(false)
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let io_mod = interp.load_module("io")?;
    let default_buf = interp.get_attr(&io_mod, "DEFAULT_BUFFER_SIZE")?;
    let buffered_base = interp.get_attr(&io_mod, "BufferedIOBase")?;
    let raw_base = interp.get_attr(&io_mod, "RawIOBase")?;

    let base_stream = module.borrow().get_attr_value("BaseStream");
    let decomp_reader = module.borrow().get_attr_value("DecompressReader");

    if let (Some(bs), ValueKind::PyClass(bc)) = (base_stream.as_ref(), buffered_base.kind())
        && let ValueKind::PyClass(lc) = bs.kind()
    {
        link_class_bases(lc, vec![Rc::clone(bc)]);
    }
    if let (Some(dr), ValueKind::PyClass(rc)) = (decomp_reader.as_ref(), raw_base.kind())
        && let ValueKind::PyClass(lc) = dr.kind()
    {
        link_class_bases(lc, vec![Rc::clone(rc)]);
    }
    module
        .borrow_mut()
        .insert_attr("BUFFER_SIZE".to_string(), default_buf);
    Ok(())
}

pyrust_module! {
    constants {
        "BUFFER_SIZE" => Value::int(BUFFER_SIZE),
    }

    class BaseStream {
        fn _check_not_closed(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let closed = _interp.get_attr(self_val.0, "closed")?;
            if _interp.truthy_value(&closed)? {
                return Err(PyError::named("ValueError", "I/O operation on closed file"));
            }
            Ok(Value::none())
        }

        fn _check_can_read(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let readable_fn = _interp.get_attr(self_val.0, "readable")?;
            let ok = call_positional(_interp, readable_fn, &[])?;
            if !_interp.truthy_value(&ok)? {
                return raise_unsupported(_interp, "File not open for reading");
            }
            Ok(Value::none())
        }

        fn _check_can_write(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let writable_fn = _interp.get_attr(self_val.0, "writable")?;
            let ok = call_positional(_interp, writable_fn, &[])?;
            if !_interp.truthy_value(&ok)? {
                return raise_unsupported(_interp, "File not open for writing");
            }
            Ok(Value::none())
        }

        fn _check_can_seek(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let readable_fn = _interp.get_attr(self_val.0, "readable")?;
            let ok_r = call_positional(_interp, readable_fn, &[])?;
            if !_interp.truthy_value(&ok_r)? {
                return raise_unsupported(
                    _interp,
                    "Seeking is only supported on files open for reading",
                );
            }
            let seekable_fn = _interp.get_attr(self_val.0, "seekable")?;
            let ok_s = call_positional(_interp, seekable_fn, &[])?;
            if !_interp.truthy_value(&ok_s)? {
                return raise_unsupported(
                    _interp,
                    "The underlying file object does not support seeking",
                );
            }
            Ok(Value::none())
        }
    }

    class DecompressReader {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            fp: PyValueRef<'_>,
            decomp_factory: PyValueRef<'_>,
            #[default(PyValue(Value::tuple(Vec::new())))] trailing_error: PyValue,
            #[varkwargs] decomp_args: IndexMap<String, Value>,
        ) -> Result<Value> {
            let mut kw_dict = PyDict::default();
            for (k, v) in decomp_args {
                kw_dict.insert(PyKey::str_from(&k), v);
            }
            let kw_val = Value::dict(kw_dict);
            let decomp = call_with_kwargs(_interp, decomp_factory.0.clone(), &[], &kw_val)?;

            _interp.assign_attr(self_val.0.clone(), "_fp", fp.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "_eof", Value::bool_(false))?;
            _interp.assign_attr(self_val.0.clone(), "_pos", Value::int(0))?;
            _interp.assign_attr(self_val.0.clone(), "_size", Value::int(-1))?;
            _interp.assign_attr(self_val.0.clone(), "_closed", Value::bool_(false))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_decomp_factory",
                decomp_factory.0.clone(),
            )?;
            _interp.assign_attr(self_val.0.clone(), "_decomp_args", kw_val)?;
            _interp.assign_attr(self_val.0.clone(), "_decompressor", decomp)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_trailing_error",
                trailing_error.0.clone(),
            )?;
            Ok(Value::none())
        }

        fn readable(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(true))
        }

        fn seekable(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let fp = _interp.get_attr(self_val.0, "_fp")?;
            let seekable_fn = _interp.get_attr(&fp, "seekable")?;
            call_positional(_interp, seekable_fn, &[])
        }

        #[getter]
        fn closed(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            match _interp.get_attr(self_val.0, "_closed") {
                Ok(v) => Ok(v),
                Err(_) => Ok(Value::bool_(false)),
            }
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_decompressor", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "_closed", Value::bool_(true))?;
            Ok(Value::none())
        }

        fn readinto(
            #[positional_only] self_val: PyValueRef<'_>,
            b: PyValueRef<'_>,
        ) -> Result<Value> {
            let mv_fn = Value::builtin_function("memoryview");
            let view = call_positional(_interp, mv_fn, std::slice::from_ref(b.0))?;
            let cast_fn = _interp.get_attr(&view, "cast")?;
            let byte_view = call_positional(_interp, cast_fn, &[Value::string("B")])?;
            let len_val = call_positional(
                _interp,
                Value::builtin_function("len"),
                std::slice::from_ref(&byte_view),
            )?;
            let read_fn = _interp.get_attr(self_val.0, "read")?;
            let data = call_positional(_interp, read_fn, &[len_val])?;
            let data_len = call_positional(
                _interp,
                Value::builtin_function("len"),
                std::slice::from_ref(&data),
            )?;
            let slice_obj = call_positional(
                _interp,
                Value::builtin_function("slice"),
                &[Value::none(), data_len.clone()],
            )?;
            let setitem = _interp.get_attr(&byte_view, "__setitem__")?;
            call_positional(_interp, setitem, &[slice_obj, data])?;
            if let Ok(rel) = _interp.get_attr(&byte_view, "release") {
                let _ = call_positional(_interp, rel, &[]);
            }
            if let Ok(rel) = _interp.get_attr(&view, "release") {
                let _ = call_positional(_interp, rel, &[]);
            }
            Ok(data_len)
        }

        fn read(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default((-1i64).into())] size: PyIndex,
        ) -> Result<Value> {
            let size_i = size.expect_i64(FN_NAME, "size")?;
            if size_i < 0 {
                let readall_fn = _interp.get_attr(self_val.0, "readall")?;
                return call_positional(_interp, readall_fn, &[]);
            }
            let eof_val = _interp.get_attr(self_val.0, "_eof")?;
            if size_i == 0 || _interp.truthy_value(&eof_val)? {
                return Ok(Value::bytes(Vec::new()));
            }

            let mut data: Option<Value> = None;
            loop {
                let decomp = _interp.get_attr(self_val.0, "_decompressor")?;
                let decomp_eof = _interp.get_attr(&decomp, "eof")?;
                if _interp.truthy_value(&decomp_eof)? {
                    let unused = _interp.get_attr(&decomp, "unused_data")?;
                    let rawblock = if _interp.truthy_value(&unused)? {
                        unused
                    } else {
                        let fp = _interp.get_attr(self_val.0, "_fp")?;
                        let fp_read = _interp.get_attr(&fp, "read")?;
                        call_positional(_interp, fp_read, &[Value::int(BUFFER_SIZE)])?
                    };
                    if !_interp.truthy_value(&rawblock)? {
                        break;
                    }
                    let factory = _interp.get_attr(self_val.0, "_decomp_factory")?;
                    let kw_args = _interp.get_attr(self_val.0, "_decomp_args")?;
                    let new_decomp = call_with_kwargs(_interp, factory, &[], &kw_args)?;
                    _interp.assign_attr(
                        self_val.0.clone(),
                        "_decompressor",
                        new_decomp.clone(),
                    )?;
                    let decompress_fn = _interp.get_attr(&new_decomp, "decompress")?;
                    let trailing_err = _interp.get_attr(self_val.0, "_trailing_error")?;
                    match call_positional(
                        _interp,
                        decompress_fn,
                        &[rawblock, Value::int(size_i)],
                    ) {
                        Ok(d) => data = Some(d),
                        Err(e) if is_matching_exception(_interp, &e, &trailing_err) => break,
                        Err(e) => return Err(e),
                    }
                } else {
                    let needs_input = _interp.get_attr(&decomp, "needs_input")?;
                    let rawblock = if _interp.truthy_value(&needs_input)? {
                        let fp = _interp.get_attr(self_val.0, "_fp")?;
                        let fp_read = _interp.get_attr(&fp, "read")?;
                        let blk = call_positional(_interp, fp_read, &[Value::int(BUFFER_SIZE)])?;
                        if !_interp.truthy_value(&blk)? {
                            return Err(PyError::named(
                                "EOFError",
                                "Compressed file ended before the end-of-stream marker was reached",
                            ));
                        }
                        blk
                    } else {
                        Value::bytes(Vec::new())
                    };
                    let decompress_fn = _interp.get_attr(&decomp, "decompress")?;
                    data = Some(call_positional(
                        _interp,
                        decompress_fn,
                        &[rawblock, Value::int(size_i)],
                    )?);
                }
                if let Some(ref d) = data
                    && _interp.truthy_value(d)?
                {
                    break;
                }
            }

            let Some(out_data) = data.filter(|d| _interp.truthy_value(d).unwrap_or(false)) else {
                let pos = _interp.get_attr(self_val.0, "_pos")?;
                _interp.assign_attr(self_val.0.clone(), "_eof", Value::bool_(true))?;
                _interp.assign_attr(self_val.0.clone(), "_size", pos)?;
                return Ok(Value::bytes(Vec::new()));
            };
            let pos = _interp.get_attr(self_val.0, "_pos")?.as_int().unwrap_or(0);
            let out_len = call_positional(
                _interp,
                Value::builtin_function("len"),
                std::slice::from_ref(&out_data),
            )?
            .as_int()
            .unwrap_or(0);
            _interp.assign_attr(self_val.0.clone(), "_pos", Value::int(pos + out_len))?;
            Ok(out_data)
        }

        fn readall(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let mut chunks = Vec::new();
            let read_fn = _interp.get_attr(self_val.0, "read")?;
            loop {
                let chunk = call_positional(_interp, read_fn.clone(), &[Value::int(i64::MAX)])?;
                if !_interp.truthy_value(&chunk)? {
                    break;
                }
                if let ValueKind::Bytes(rc) = chunk.kind() {
                    chunks.extend_from_slice(rc.as_slice());
                }
            }
            Ok(Value::bytes(chunks))
        }

        fn _rewind(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let fp = _interp.get_attr(self_val.0, "_fp")?;
            let seek_fn = _interp.get_attr(&fp, "seek")?;
            call_positional(_interp, seek_fn, &[Value::int(0)])?;
            _interp.assign_attr(self_val.0.clone(), "_eof", Value::bool_(false))?;
            _interp.assign_attr(self_val.0.clone(), "_pos", Value::int(0))?;
            let factory = _interp.get_attr(self_val.0, "_decomp_factory")?;
            let kw_args = _interp.get_attr(self_val.0, "_decomp_args")?;
            let decomp = call_with_kwargs(_interp, factory, &[], &kw_args)?;
            _interp.assign_attr(self_val.0.clone(), "_decompressor", decomp)?;
            Ok(Value::none())
        }

        fn seek(
            #[positional_only] self_val: PyValueRef<'_>,
            offset: PyIndex,
            #[default(0i64.into())] whence: PyIndex,
        ) -> Result<Value> {
            let mut off = offset.expect_i64(FN_NAME, "offset")?;
            let wh = whence.expect_i64(FN_NAME, "whence")?;
            let pos = _interp.get_attr(self_val.0, "_pos")?.as_int().unwrap_or(0);
            match wh {
                0 => {}
                1 => off += pos,
                2 => {
                    let mut size = _interp.get_attr(self_val.0, "_size")?.as_int().unwrap_or(-1);
                    if size < 0 {
                        let read_fn = _interp.get_attr(self_val.0, "read")?;
                        loop {
                            let d = call_positional(
                                _interp,
                                read_fn.clone(),
                                &[Value::int(BUFFER_SIZE)],
                            )?;
                            if !_interp.truthy_value(&d)? {
                                break;
                            }
                        }
                        size = _interp.get_attr(self_val.0, "_size")?.as_int().unwrap_or(0);
                    }
                    off += size;
                }
                _ => {
                    return Err(PyError::named(
                        "ValueError",
                        format!("Invalid value for whence: {wh}"),
                    ));
                }
            }

            let cur_pos = _interp.get_attr(self_val.0, "_pos")?.as_int().unwrap_or(0);
            if off < cur_pos {
                let rewind_fn = _interp.get_attr(self_val.0, "_rewind")?;
                call_positional(_interp, rewind_fn, &[])?;
            } else {
                off -= cur_pos;
            }

            let read_fn = _interp.get_attr(self_val.0, "read")?;
            while off > 0 {
                let step = BUFFER_SIZE.min(off);
                let d = call_positional(_interp, read_fn.clone(), &[Value::int(step)])?;
                let dlen = call_positional(
                    _interp,
                    Value::builtin_function("len"),
                    std::slice::from_ref(&d),
                )?
                .as_int()
                .unwrap_or(0);
                if dlen == 0 {
                    break;
                }
                off -= dlen;
            }

            _interp.get_attr(self_val.0, "_pos")
        }

        fn tell(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_pos")
        }
    }
}
