// `plistlib` module — 100% native Rust implementation of Python 3.12's
// `plistlib` supporting BOTH Apple XML (`FMT_XML`) and Apple Binary (`bplist00`
// / `FMT_BINARY`) formats with bidirectional CPython 3.12 wire-format parity
// (issue #3259).
//
// Reference: <https://docs.python.org/3/library/plistlib.html>

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, OptValueRef, OptionalArg, PyBool, PyBytesLike, PyValue, PyValueRef,
};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, primitive_class_by_name, render_value_repr, value_class,
};
use crate::value::{Value, ValueKind};

pyrust_int_enum! {
    pub(crate) PlistFormat in "plistlib" {
        FMT_XML = 1,
        FMT_BINARY = 2,
    }
}

include!("plistlib/xml.rs");
include!("plistlib/binary.rs");

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

fn py_isinstance(interp: &mut Interpreter, obj: &Value, cls: &Value) -> Result<bool> {
    let res = call_pos(
        interp,
        Value::builtin_function("isinstance"),
        &[obj.clone(), cls.clone()],
    )?;
    interp.truthy_value(&res)
}

fn raise_invalid_file(interp: &mut Interpreter, msg: &str) -> PyError {
    if let Ok(plist_mod) = interp.load_module("plistlib")
        && let Ok(exc_cls) = interp.get_attr(&plist_mod, "InvalidFileException")
        && let Ok(inst) = call_pos(interp, exc_cls, &[Value::string(msg)])
    {
        return PyError::raised(inst);
    }
    PyError::named("ValueError", msg)
}

fn value_type_repr(interp: &mut Interpreter, val: &Value) -> String {
    let cls = value_class(val);
    render_value_repr(interp, &cls).unwrap_or_else(|_| "<unknown>".to_string())
}

fn get_datetime_class(interp: &mut Interpreter) -> Result<Value> {
    let dt_mod = interp.load_module("datetime")?;
    interp.get_attr(&dt_mod, "datetime")
}

fn is_datetime_instance(interp: &mut Interpreter, val: &Value) -> Result<bool> {
    let dt_cls = get_datetime_class(interp)?;
    py_isinstance(interp, val, &dt_cls)
}

fn is_uid_instance(interp: &mut Interpreter, val: &Value) -> Result<bool> {
    let plist_mod = interp.load_module("plistlib")?;
    let uid_cls = interp.get_attr(&plist_mod, "UID")?;
    py_isinstance(interp, val, &uid_cls)
}

fn is_dict_subclass(interp: &mut Interpreter, val: &Value) -> Result<bool> {
    let Some(cls) = primitive_class_by_name("dict") else {
        return Ok(false);
    };
    py_isinstance(interp, val, &Value::py_class(cls))
}

fn is_str_subclass(interp: &mut Interpreter, val: &Value) -> Result<bool> {
    let Some(cls) = primitive_class_by_name("str") else {
        return Ok(false);
    };
    py_isinstance(interp, val, &Value::py_class(cls))
}

fn is_int_subclass(interp: &mut Interpreter, val: &Value) -> Result<bool> {
    let Some(cls) = primitive_class_by_name("int") else {
        return Ok(false);
    };
    py_isinstance(interp, val, &Value::py_class(cls))
}

fn is_list_or_tuple_subclass(interp: &mut Interpreter, val: &Value) -> Result<bool> {
    if let Some(cls) = primitive_class_by_name("list")
        && py_isinstance(interp, val, &Value::py_class(cls))?
    {
        return Ok(true);
    }
    if let Some(cls) = primitive_class_by_name("tuple")
        && py_isinstance(interp, val, &Value::py_class(cls))?
    {
        return Ok(true);
    }
    Ok(false)
}

fn extract_str_value(interp: &mut Interpreter, val: &Value) -> Result<String> {
    if let Some(s) = val.as_str() {
        return Ok(s.to_string());
    }
    let str_cls = primitive_class_by_name("str")
        .map(Value::py_class)
        .unwrap_or_else(|| Value::builtin_function("str"));
    let s = call_pos(interp, str_cls, std::slice::from_ref(val))?;
    Ok(s.as_str().unwrap_or("").to_string())
}

fn extract_int_i128(interp: &mut Interpreter, val: &Value) -> Result<i128> {
    match val.kind() {
        ValueKind::Int(n) => Ok(n as i128),
        ValueKind::Bool(b) => Ok(if b { 1 } else { 0 }),
        ValueKind::BigInt(bi) => {
            i128::try_from(bi).map_err(|_| PyError::named("OverflowError", bi.to_string()))
        }
        _ => {
            let int_cls = primitive_class_by_name("int")
                .map(Value::py_class)
                .unwrap_or_else(|| Value::builtin_function("int"));
            let iv = call_pos(interp, int_cls, std::slice::from_ref(val))?;
            match iv.kind() {
                ValueKind::Int(n) => Ok(n as i128),
                ValueKind::BigInt(bi) => {
                    i128::try_from(bi).map_err(|_| PyError::named("OverflowError", bi.to_string()))
                }
                _ => Err(PyError::named("TypeError", "expected int")),
            }
        }
    }
}

fn extract_datetime_parts(
    interp: &mut Interpreter,
    dt: &Value,
) -> Result<(i32, u8, u8, u8, u8, u8, u32)> {
    let year = interp.get_attr(dt, "year")?.as_int().unwrap_or(0) as i32;
    let month = interp.get_attr(dt, "month")?.as_int().unwrap_or(0) as u8;
    let day = interp.get_attr(dt, "day")?.as_int().unwrap_or(0) as u8;
    let hour = interp.get_attr(dt, "hour")?.as_int().unwrap_or(0) as u8;
    let minute = interp.get_attr(dt, "minute")?.as_int().unwrap_or(0) as u8;
    let second = interp.get_attr(dt, "second")?.as_int().unwrap_or(0) as u8;
    let microsecond = interp.get_attr(dt, "microsecond")?.as_int().unwrap_or(0) as u32;
    Ok((year, month, day, hour, minute, second, microsecond))
}

fn format_datetime_iso(interp: &mut Interpreter, dt: &Value) -> Result<String> {
    let (y, m, d, hh, mm, ss, _) = extract_datetime_parts(interp, dt)?;
    Ok(format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z"))
}

fn datetime_to_apple_timestamp(interp: &mut Interpreter, dt: &Value) -> Result<f64> {
    let dt_cls = get_datetime_class(interp)?;
    let epoch = call_pos(
        interp,
        dt_cls,
        &[Value::int(2001), Value::int(1), Value::int(1)],
    )?;
    let delta = interp.eval_binary(dt.clone(), BinaryOp::Sub, epoch)?;
    let secs_val = call_method(interp, &delta, "total_seconds", &[])?;
    match secs_val.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(n) => Ok(n as f64),
        _ => Err(PyError::named(
            "TypeError",
            "total_seconds() did not return a float",
        )),
    }
}

fn make_datetime_from_apple_timestamp(interp: &mut Interpreter, secs: f64) -> Result<Value> {
    let dt_mod = interp.load_module("datetime")?;
    let dt_cls = interp.get_attr(&dt_mod, "datetime")?;
    let td_cls = interp.get_attr(&dt_mod, "timedelta")?;
    let epoch = call_pos(
        interp,
        dt_cls,
        &[Value::int(2001), Value::int(1), Value::int(1)],
    )?;
    let delta = interp.call_function_expanded(
        td_cls,
        &[ExpandedCallArg {
            name: Some("seconds".to_string()),
            value: Value::float(secs),
        }],
    )?;
    interp.eval_binary(epoch, BinaryOp::Add, delta)
}

fn collect_dict_items(
    interp: &mut Interpreter,
    dict_val: &Value,
    sort_keys: bool,
    skipkeys: bool,
) -> Result<Vec<(String, Value)>> {
    let items_val = call_method(interp, dict_val, "items", &[])?;
    let pairs = interp.collect_iterable(&items_val)?;
    let mut out: Vec<(String, Value)> = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let kv = interp.collect_iterable(&pair)?;
        if kv.len() != 2 {
            continue;
        }
        let key_str = match kv[0].kind() {
            ValueKind::Str(s) => s.to_string(),
            ValueKind::PyInstance(_) if is_str_subclass(interp, &kv[0])? => {
                extract_str_value(interp, &kv[0])?
            }
            _ => {
                if skipkeys {
                    continue;
                }
                return Err(PyError::named("TypeError", "keys must be strings"));
            }
        };
        out.push((key_str, kv[1].clone()));
    }
    if sort_keys {
        out.sort_by(|a, b| a.0.cmp(&b.0));
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResolvedFmt {
    Xml,
    Binary,
}

fn resolve_fmt_value(interp: &mut Interpreter, fmt: &Value) -> Result<Option<ResolvedFmt>> {
    if fmt.is_none() {
        return Ok(None);
    }
    if matches!(fmt.kind(), ValueKind::Bool(_)) {
        return Ok(None);
    }
    if let Some(i) = fmt.as_int() {
        return match i {
            1 => Ok(Some(ResolvedFmt::Xml)),
            2 => Ok(Some(ResolvedFmt::Binary)),
            _ => Ok(None),
        };
    }
    if let Ok(plist_mod) = interp.load_module("plistlib")
        && let Ok(fmt_cls) = interp.get_attr(&plist_mod, "PlistFormat")
        && py_isinstance(interp, fmt, &fmt_cls)?
        && let Ok(val) = interp.get_attr(fmt, "value")
        && let Some(i) = val.as_int()
    {
        return match i {
            1 => Ok(Some(ResolvedFmt::Xml)),
            2 => Ok(Some(ResolvedFmt::Binary)),
            _ => Ok(None),
        };
    }
    Ok(None)
}

fn default_dict_type() -> Value {
    primitive_class_by_name("dict")
        .map(Value::py_class)
        .unwrap_or_else(|| Value::builtin_function("dict"))
}

fn read_all_bytes_from_fp(interp: &mut Interpreter, fp: &Value) -> Result<Vec<u8>> {
    let raw = call_method(interp, fp, "read", &[])?;
    let bytes = PyBytesLike::try_from_value_with_interp(interp, &raw, "load", "fp")?;
    Ok(bytes.as_bytes().to_vec())
}

fn parse_bytes_with_fmt(
    interp: &mut Interpreter,
    data: &[u8],
    fmt: Option<&Value>,
    dict_type: Value,
) -> Result<Value> {
    let resolved = match fmt {
        None => None,
        Some(v) if v.is_none() => None,
        Some(v) => match resolve_fmt_value(interp, v)? {
            Some(f) => Some(f),
            None => {
                if let Some(cls) = crate::interpreter::lookup_exc_class("KeyError")
                    && let Ok(inst) =
                        call_pos(interp, Value::py_class(cls), std::slice::from_ref(v))
                {
                    return Err(PyError::raised(inst));
                }
                return Err(PyError::named("KeyError", "unsupported plist format"));
            }
        },
    };

    let format = match resolved {
        Some(f) => f,
        None => {
            if is_fmt_binary(data) {
                ResolvedFmt::Binary
            } else if is_fmt_xml(data) {
                ResolvedFmt::Xml
            } else {
                return Err(raise_invalid_file(interp, "Invalid file"));
            }
        }
    };

    match format {
        ResolvedFmt::Xml => parse_xml_plist(interp, data, dict_type),
        ResolvedFmt::Binary => parse_binary_plist(interp, data, dict_type),
    }
}

fn serialize_with_fmt(
    interp: &mut Interpreter,
    value: &Value,
    fmt: Option<&Value>,
    sort_keys: bool,
    skipkeys: bool,
) -> Result<Vec<u8>> {
    let format = match fmt {
        None => ResolvedFmt::Xml,
        Some(v) => match resolve_fmt_value(interp, v)? {
            Some(f) => f,
            None => {
                let repr = render_value_repr(interp, v)?;
                return Err(PyError::named(
                    "ValueError",
                    format!("Unsupported format: {repr}"),
                ));
            }
        },
    };
    match format {
        ResolvedFmt::Xml => dump_xml_plist(interp, value, sort_keys, skipkeys),
        ResolvedFmt::Binary => dump_binary_plist(interp, value, sort_keys, skipkeys),
    }
}

/// Install `PlistFormat` (`IntEnum`) along with `FMT_XML` and `FMT_BINARY`
/// module globals when `plistlib` is first loaded.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let _ = PlistFormat::install_with_globals(interp, module);
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("InvalidFileException"),
            Value::string("FMT_XML"),
            Value::string("FMT_BINARY"),
            Value::string("load"),
            Value::string("dump"),
            Value::string("loads"),
            Value::string("dumps"),
            Value::string("UID"),
        ]),
        "PLISTHEADER" => Value::bytes(PLIST_XML_HEADER.to_vec()),
    }

    /// Raised when a plist file cannot be parsed or its format is unrecognized.
    exception InvalidFileException(ValueError);

    class UID {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyValue,
        ) -> Result<Value> {
            let is_int = matches!(
                data.0.kind(),
                ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_)
            ) || is_int_subclass(_interp, &data.0)?;
            if !is_int {
                return Err(PyError::named("TypeError", "data must be an int"));
            }
            let n = extract_int_i128(_interp, &data.0).map_err(|_| {
                PyError::named("ValueError", "UIDs cannot be >= 2**64")
            })?;
            if n >= (1i128 << 64) {
                return Err(PyError::named("ValueError", "UIDs cannot be >= 2**64"));
            }
            if n < 0 {
                return Err(PyError::named("ValueError", "UIDs must be positive"));
            }
            _interp.assign_attr(self_val.0.clone(), "data", data.0)?;
            Ok(Value::none())
        }

        fn __index__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "data")
        }

        fn __int__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let d = _interp.get_attr(self_val.0, "data")?;
            let int_cls = primitive_class_by_name("int")
                .map(Value::py_class)
                .unwrap_or_else(|| Value::builtin_function("int"));
            call_pos(_interp, int_cls, &[d])
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = value_class(self_val.0);
            let name = _interp
                .get_attr(&cls, "__name__")
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "UID".to_string());
            let data = _interp.get_attr(self_val.0, "data")?;
            let data_repr = render_value_repr(_interp, &data)?;
            Ok(Value::string(format!("{name}({data_repr})")))
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = value_class(self_val.0);
            let data = _interp.get_attr(self_val.0, "data")?;
            Ok(Value::tuple(vec![cls, Value::tuple(vec![data])]))
        }

        fn __eq__(
            #[positional_only] self_val: PyValueRef<'_>,
            other: PyValue,
        ) -> Result<Value> {
            if !is_uid_instance(_interp, &other.0)? {
                return Ok(Value::not_implemented());
            }
            let a = _interp.get_attr(self_val.0, "data")?;
            let b = _interp.get_attr(&other.0, "data")?;
            _interp.eval_binary(a, BinaryOp::Eq, b)
        }

        fn __hash__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let d = _interp.get_attr(self_val.0, "data")?;
            call_pos(_interp, Value::builtin_function("hash"), &[d])
        }
    }

    fn load(
        fp: PyValue,
        #[keyword_only]
        #[default(OptionalArg::Omitted)]
        fmt: OptValueRef<'_>,
        #[keyword_only]
        #[default(OptionalArg::Omitted)]
        dict_type: OptValueRef<'_>,
    ) -> Result<Value> {
        let dict_ty = dict_type
            .as_value()
            .cloned()
            .unwrap_or_else(default_dict_type);
        let fmt_opt = fmt.as_value();
        if fmt_opt.is_none() || fmt_opt.is_some_and(Value::is_none) {
            let hdr_val = call_method(_interp, &fp.0, "read", &[Value::int(32)])?;
            let hdr_bytes =
                PyBytesLike::try_from_value_with_interp(_interp, &hdr_val, FN_NAME, "fp")?;
            let header = hdr_bytes.as_bytes().to_vec();
            call_method(_interp, &fp.0, "seek", &[Value::int(0)])?;
            if !is_fmt_binary(&header) && !is_fmt_xml(&header) {
                return Err(raise_invalid_file(_interp, "Invalid file"));
            }
        }
        let data = read_all_bytes_from_fp(_interp, &fp.0)?;
        parse_bytes_with_fmt(_interp, &data, fmt_opt, dict_ty)
    }

    fn loads(
        value: PyValue,
        #[keyword_only]
        #[default(OptionalArg::Omitted)]
        fmt: OptValueRef<'_>,
        #[keyword_only]
        #[default(OptionalArg::Omitted)]
        dict_type: OptValueRef<'_>,
    ) -> Result<Value> {
        let dict_ty = dict_type
            .as_value()
            .cloned()
            .unwrap_or_else(default_dict_type);
        let fmt_opt = fmt.as_value();
        let data = if let Some(s) = value.0.as_str() {
            if let Some(fv) = fmt_opt
                && matches!(resolve_fmt_value(_interp, fv)?, Some(ResolvedFmt::Binary))
            {
                return Err(PyError::named(
                    "TypeError",
                    "value must be bytes-like object when fmt is FMT_BINARY",
                ));
            }
            s.as_bytes().to_vec()
        } else {
            let bytes = PyBytesLike::try_from_value_with_interp(_interp, &value.0, FN_NAME, "value")?;
            bytes.as_bytes().to_vec()
        };
        parse_bytes_with_fmt(_interp, &data, fmt_opt, dict_ty)
    }

    fn dump(
        value: PyValue,
        fp: PyValue,
        #[keyword_only]
        #[default(OptionalArg::Omitted)]
        fmt: OptValueRef<'_>,
        #[keyword_only]
        #[default(true.into())]
        sort_keys: PyBool,
        #[keyword_only]
        #[default(false.into())]
        skipkeys: PyBool,
    ) -> Result<Value> {
        let bytes = serialize_with_fmt(_interp, &value.0, fmt.as_value(), *sort_keys, *skipkeys)?;
        call_method(_interp, &fp.0, "write", &[Value::bytes(bytes)])?;
        Ok(Value::none())
    }

    fn dumps(
        value: PyValue,
        #[keyword_only]
        #[default(OptionalArg::Omitted)]
        fmt: OptValueRef<'_>,
        #[keyword_only]
        #[default(false.into())]
        skipkeys: PyBool,
        #[keyword_only]
        #[default(true.into())]
        sort_keys: PyBool,
    ) -> Result<Value> {
        let bytes = serialize_with_fmt(_interp, &value.0, fmt.as_value(), *sort_keys, *skipkeys)?;
        Ok(Value::bytes(bytes))
    }
}
