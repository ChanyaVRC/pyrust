// `py_compile` module — 100% native Rust implementation of Python 3.12's
// `py_compile` module (issue #3249).
//
// Writes genuine CPython 3.12 `.pyc` files (16-byte header: magic `b"\xcb\r\r\n"`,
// bitfield, mtime+size or SipHash-1-3 source hash, followed by marshal v4
// `CodeType` payload) into `__pycache__/<stem>.cpython-312[.opt-N].pyc` or a
// custom `cfile`.
//
// Reference: <https://docs.python.org/3.12/library/py_compile.html>

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyBool, PyIndex, PyPath, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_class};
use crate::value::{Value, ValueKind};

/// CPython 3.12 bytecode magic number: 3531 (`0x0dcb` LE) + `\r\n`.
pub(crate) const PYC_MAGIC_NUMBER: [u8; 4] = [0xcb, 0x0d, 0x0d, 0x0a];

pub(crate) const INVALIDATION_TIMESTAMP: i64 = 1;
pub(crate) const INVALIDATION_CHECKED_HASH: i64 = 2;
pub(crate) const INVALIDATION_UNCHECKED_HASH: i64 = 3;

/// Install the genuine `enum.Enum` subclass `PycInvalidationMode` onto `py_compile`.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let members = [
        ("TIMESTAMP", INVALIDATION_TIMESTAMP),
        ("CHECKED_HASH", INVALIDATION_CHECKED_HASH),
        ("UNCHECKED_HASH", INVALIDATION_UNCHECKED_HASH),
    ];
    if let Ok(enum_cls) = crate::builtin_modules::macros::build_enum(
        interp,
        "py_compile",
        "PycInvalidationMode",
        &members,
    ) {
        let _ = crate::builtin_modules::macros::install_enum_on_module(
            interp,
            module,
            "PycInvalidationMode",
            enum_cls,
            false,
            &members,
        );
    }
}

/// SipHash-1-3 matching CPython 3.12's `_imp.source_hash(key, source)` (PEP 552).
pub(crate) fn siphash13(key: u64, data: &[u8]) -> u64 {
    let k0 = key;
    let k1 = 0u64;
    let mut v0 = 0x736f_6d65_7073_6575u64 ^ k0;
    let mut v1 = 0x646f_7261_6e64_6f6du64 ^ k1;
    let mut v2 = 0x6c79_6765_6e65_7261u64 ^ k0;
    let mut v3 = 0x7465_6462_7974_6573u64 ^ k1;

    macro_rules! sipround {
        () => {
            v0 = v0.wrapping_add(v1);
            v1 = v1.rotate_left(13);
            v1 ^= v0;
            v0 = v0.rotate_left(32);
            v2 = v2.wrapping_add(v3);
            v3 = v3.rotate_left(16);
            v3 ^= v2;
            v0 = v0.wrapping_add(v3);
            v3 = v3.rotate_left(21);
            v3 ^= v0;
            v2 = v2.wrapping_add(v1);
            v1 = v1.rotate_left(17);
            v1 ^= v2;
            v2 = v2.rotate_left(32);
        };
    }

    let (chunks, rem) = data.as_chunks::<8>();
    for chunk in chunks {
        let m = u64::from_le_bytes(*chunk);
        v3 ^= m;
        sipround!();
        v0 ^= m;
    }

    let mut b = ((data.len() as u64) & 0xff) << 56;
    for (i, &byte) in rem.iter().enumerate() {
        b |= (byte as u64) << (i * 8);
    }
    v3 ^= b;
    sipround!();
    v0 ^= b;

    v2 ^= 0xff;
    sipround!();
    sipround!();
    sipround!();
    v0 ^ v1 ^ v2 ^ v3
}

/// Compute the 8-byte PEP 552 source hash for `source_bytes`.
pub(crate) fn source_hash_bytes(source_bytes: &[u8]) -> [u8; 8] {
    let key = u64::from(u32::from_le_bytes(PYC_MAGIC_NUMBER));
    siphash13(key, source_bytes).to_le_bytes()
}

/// Extract the integer invalidation mode value (`1`, `2`, or `3`) from a
/// `PycInvalidationMode` enum member or `None`.
pub(crate) fn resolve_invalidation_mode(
    interp: &mut Interpreter,
    mode_opt: Option<&Value>,
) -> Result<i64> {
    match mode_opt {
        None => {
            if std::env::var_os("SOURCE_DATE_EPOCH").is_some() {
                Ok(INVALIDATION_CHECKED_HASH)
            } else {
                Ok(INVALIDATION_TIMESTAMP)
            }
        }
        Some(v) if v.is_none() => {
            if std::env::var_os("SOURCE_DATE_EPOCH").is_some() {
                Ok(INVALIDATION_CHECKED_HASH)
            } else {
                Ok(INVALIDATION_TIMESTAMP)
            }
        }
        Some(v) => {
            // Accept `PycInvalidationMode` enum members by reading `.value`.
            if let Ok(val_attr) = interp.get_attr(v, "value")
                && let ValueKind::Int(n) = val_attr.kind()
                && matches!(
                    n,
                    INVALIDATION_TIMESTAMP
                        | INVALIDATION_CHECKED_HASH
                        | INVALIDATION_UNCHECKED_HASH
                )
            {
                return Ok(n);
            }
            if let ValueKind::Int(n) = v.kind()
                && matches!(
                    n,
                    INVALIDATION_TIMESTAMP
                        | INVALIDATION_CHECKED_HASH
                        | INVALIDATION_UNCHECKED_HASH
                )
            {
                return Ok(n);
            }
            Err(PyError::named(
                "ValueError",
                format!("Invalid invalidation mode: {}", v.repr_raw()),
            ))
        }
    }
}

/// Compute the `.pyc` cache path for `source_path` and `optimize` level using
/// `importlib.util.cache_from_source`.
pub(crate) fn compute_cache_path(
    interp: &mut Interpreter,
    source_path: &str,
    optimize: i64,
) -> Result<String> {
    let util_mod = interp.load_module("importlib.util")?;
    let cfs_fn = interp.get_attr(&util_mod, "cache_from_source")?;
    let opt_val = if optimize < 0 {
        Value::none()
    } else if optimize == 0 {
        Value::string("")
    } else {
        Value::int(optimize)
    };
    let res = interp.call_function_expanded(
        cfs_fn,
        &[
            ExpandedCallArg {
                name: None,
                value: Value::string(source_path),
            },
            ExpandedCallArg {
                name: Some("optimization".to_string()),
                value: opt_val,
            },
        ],
    )?;
    res.as_str()
        .map(str::to_owned)
        .ok_or_else(|| PyError::runtime("cache_from_source did not return str".into()))
}

fn format_py_compile_error_msg(
    interp: &mut Interpreter,
    exc_type: &Value,
    exc_value: &Value,
    file: &str,
) -> String {
    if let Ok(tb_mod) = interp.load_module("traceback")
        && let Ok(fmt_fn) = interp.get_attr(&tb_mod, "format_exception_only")
        && let Ok(lines_val) = interp.call_function_expanded(
            fmt_fn,
            &[
                ExpandedCallArg {
                    name: None,
                    value: exc_type.clone(),
                },
                ExpandedCallArg {
                    name: None,
                    value: exc_value.clone(),
                },
            ],
        )
        && let Ok(lines) = interp.collect_iterable(&lines_val)
    {
        let mut out = String::new();
        for line in lines {
            if let Some(s) = line.as_str() {
                out.push_str(s);
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    let type_name = interp
        .get_attr(exc_type, "__name__")
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "Exception".to_string());
    let val_str = interp.render_value_as_str(exc_value).unwrap_or_default();
    if val_str.is_empty() {
        format!("Sorry: {type_name} ({file})\n")
    } else {
        format!("{type_name}: {val_str}\n")
    }
}

fn build_py_compile_error(interp: &mut Interpreter, err: &PyError, file: &str) -> Result<Value> {
    let exc_val = interp.materialize_pyerror(err.clone())?;
    let exc_type = value_class(&exc_val);
    let mod_val = interp.load_module("py_compile")?;
    let pce_cls = interp.get_attr(&mod_val, "PyCompileError")?;
    interp.call_function_expanded(
        pce_cls,
        &[
            ExpandedCallArg {
                name: None,
                value: exc_type,
            },
            ExpandedCallArg {
                name: None,
                value: exc_val,
            },
            ExpandedCallArg {
                name: None,
                value: Value::string(file),
            },
        ],
    )
}

fn write_stderr(interp: &mut Interpreter, text: &str) {
    if let Ok(sys_mod) = interp.load_module("sys")
        && let Ok(stderr) = interp.get_attr(&sys_mod, "stderr")
        && !stderr.is_none()
        && let Ok(write_fn) = interp.get_attr(&stderr, "write")
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

/// Build the 16-byte CPython 3.12 `.pyc` header.
pub(crate) fn build_pyc_header(
    invalidation_mode: i64,
    mtime: u32,
    source_bytes: &[u8],
) -> [u8; 16] {
    let mut header = [0u8; 16];
    header[0..4].copy_from_slice(&PYC_MAGIC_NUMBER);
    match invalidation_mode {
        INVALIDATION_CHECKED_HASH => {
            header[4..8].copy_from_slice(&3u32.to_le_bytes());
            header[8..16].copy_from_slice(&source_hash_bytes(source_bytes));
        }
        INVALIDATION_UNCHECKED_HASH => {
            header[4..8].copy_from_slice(&1u32.to_le_bytes());
            header[8..16].copy_from_slice(&source_hash_bytes(source_bytes));
        }
        _ => {
            header[4..8].copy_from_slice(&0u32.to_le_bytes());
            header[8..12].copy_from_slice(&mtime.to_le_bytes());
            header[12..16].copy_from_slice(&(source_bytes.len() as u32).to_le_bytes());
        }
    }
    header
}

fn file_mtime_u32(path: &Path) -> u32 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}

fn write_pyc_atomic(cfile: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = cfile.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_name = format!("{}.{}", cfile.to_string_lossy(), std::process::id());
    let tmp_path = PathBuf::from(&tmp_name);
    if std::fs::write(&tmp_path, data).is_ok() {
        if std::fs::rename(&tmp_path, cfile).is_ok() {
            return Ok(());
        }
        let _ = std::fs::remove_file(&tmp_path);
    }
    std::fs::write(cfile, data)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_impl(
    interp: &mut Interpreter,
    file: &str,
    cfile: Option<String>,
    dfile: Option<String>,
    doraise: bool,
    optimize: i64,
    invalidation_mode: Option<&Value>,
    quiet: i64,
) -> Result<Value> {
    let inv_mode = resolve_invalidation_mode(interp, invalidation_mode)?;

    let cfile_str = match cfile {
        Some(c) => c,
        None => compute_cache_path(interp, file, optimize)?,
    };

    let source_path = Path::new(file);
    let source_bytes =
        std::fs::read(source_path).map_err(|e| PyError::from_io_error(&e, Some(file)))?;
    let mtime = file_mtime_u32(source_path);

    let display_file = dfile.as_deref().unwrap_or(file);
    let compile_fn = Value::builtin_function("compile");
    let compile_res = interp.call_function_expanded(
        compile_fn,
        &[
            ExpandedCallArg {
                name: None,
                value: Value::bytes(source_bytes.clone()),
            },
            ExpandedCallArg {
                name: None,
                value: Value::string(display_file),
            },
            ExpandedCallArg {
                name: None,
                value: Value::string("exec"),
            },
            ExpandedCallArg {
                name: Some("dont_inherit".to_string()),
                value: Value::bool_(true),
            },
            ExpandedCallArg {
                name: Some("optimize".to_string()),
                value: Value::int(optimize),
            },
        ],
    );

    let code = match compile_res {
        Ok(c) => c,
        Err(err) => {
            let py_exc = build_py_compile_error(interp, &err, display_file)?;
            if quiet < 2 {
                if doraise {
                    return Err(PyError::raised(py_exc));
                }
                if quiet == 0 {
                    let msg = interp
                        .get_attr(&py_exc, "msg")
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_default();
                    if msg.ends_with('\n') {
                        write_stderr(interp, &msg);
                    } else {
                        write_stderr(interp, &format!("{msg}\n"));
                    }
                }
            }
            return Ok(Value::none());
        }
    };

    let marshaled = super::marshal::marshal_dumps_impl(&code, 4)?;
    let header = build_pyc_header(inv_mode, mtime, &source_bytes);
    let mut pyc_data = Vec::with_capacity(16 + marshaled.len());
    pyc_data.extend_from_slice(&header);
    pyc_data.extend_from_slice(&marshaled);

    let cfile_path = Path::new(&cfile_str);
    write_pyc_atomic(cfile_path, &pyc_data)
        .map_err(|e| PyError::from_io_error(&e, Some(&cfile_str)))?;

    Ok(Value::string(cfile_str))
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("compile"),
            Value::string("main"),
            Value::string("PyCompileError"),
            Value::string("PycInvalidationMode"),
        ]),
    }

    class PyCompileError("Exception") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            exc_type: PyValue,
            exc_value: PyValue,
            file: PyValue,
            #[default(PyStr(std::borrow::Cow::Borrowed("")))] msg: PyStr<'_>,
        ) -> Result<Value> {
            let exc_type_name = _interp
                .get_attr(&exc_type.0, "__name__")
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "Exception".to_string());
            let file_str = _interp.render_value_as_str(&file.0).unwrap_or_default();
            let final_msg = if !msg.0.is_empty() {
                msg.0.to_string()
            } else {
                format_py_compile_error_msg(_interp, &exc_type.0, &exc_value.0, &file_str)
            };
            _interp.assign_attr(
                self_val.0.clone(),
                "exc_type_name",
                Value::string(&exc_type_name),
            )?;
            _interp.assign_attr(self_val.0.clone(), "exc_value", exc_value.0)?;
            _interp.assign_attr(self_val.0.clone(), "file", file.0)?;
            _interp.assign_attr(self_val.0.clone(), "msg", Value::string(&final_msg))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "args",
                Value::tuple(vec![Value::string(&final_msg)]),
            )?;
            Ok(Value::none())
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            match _interp.get_attr(self_val.0, "msg") {
                Ok(v) => Ok(v),
                Err(_) => Ok(Value::string("")),
            }
        }
    }

    fn _get_default_invalidation_mode() -> Result<Value> {
        let mod_val = _interp.load_module("py_compile")?;
        let enum_cls = _interp.get_attr(&mod_val, "PycInvalidationMode")?;
        let member_name = if std::env::var_os("SOURCE_DATE_EPOCH").is_some() {
            "CHECKED_HASH"
        } else {
            "TIMESTAMP"
        };
        _interp.get_attr(&enum_cls, member_name)
    }

    fn compile(
        file: PyPath,
        #[default(None)] cfile: Option<PyPath>,
        #[default(None)] dfile: Option<PyPath>,
        #[default(PyBool(false))] doraise: PyBool,
        #[default(PyIndex::from(i64::from(-1)))] optimize: PyIndex,
        #[default(None)] invalidation_mode: Option<PyValue>,
        #[default(PyIndex::from(0i64))] quiet: PyIndex,
    ) -> Result<Value> {
        let file_str = file.as_path().to_string_lossy().into_owned();
        let cfile_str = cfile.map(|p| p.as_path().to_string_lossy().into_owned());
        let dfile_str = dfile.map(|p| p.as_path().to_string_lossy().into_owned());
        let opt = optimize.expect_i64("compile", "optimize")?;
        let quiet_i = quiet.expect_i64("compile", "quiet")?;
        compile_impl(
            _interp,
            &file_str,
            cfile_str,
            dfile_str,
            doraise.0,
            opt,
            invalidation_mode.as_ref().map(|v| &v.0),
            quiet_i,
        )
    }

    fn main(#[default(None)] args: Option<PyValue>) -> Result<Value> {
        let files: Vec<String> = match args {
            Some(v) if !v.0.is_none() => {
                let items = _interp.collect_iterable(&v.0)?;
                items
                    .into_iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            }
            _ => {
                let sys_mod = _interp.load_module("sys")?;
                let argv = _interp.get_attr(&sys_mod, "argv")?;
                let items = _interp.collect_iterable(&argv)?;
                items
                    .into_iter()
                    .skip(1)
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            }
        };
        let mut rv = 0i64;
        let mut quiet = 0i64;
        let mut targets: Vec<String> = Vec::new();
        for f in files {
            if f == "-q" {
                quiet = 1;
            } else {
                targets.push(f);
            }
        }
        for filename in targets {
            match compile_impl(_interp, &filename, None, None, true, -1, None, quiet) {
                Ok(_) => {}
                Err(err) => {
                    rv = 1;
                    if quiet == 0 {
                        let msg = match _interp.materialize_pyerror(err) {
                            Ok(exc) => _interp.render_value_as_str(&exc).unwrap_or_default(),
                            Err(e) => format!("{e}"),
                        };
                        write_stderr(_interp, &format!("{msg}\n"));
                    }
                }
            }
        }
        Ok(Value::int(rv))
    }
}
