// `zipimport` module — 100% native Rust implementation of Python 3.12's
// `zipimport` module (issue #3249).
//
// Reads ZIP archives natively in Rust (Central Directory + STORED/DEFLATED
// decompression via `zlib::raw_inflate`, supporting SFX/shebang-prefixed `.pyz`
// archives), exposes `ZipImportError(ImportError)` and `zipimporter`, and
// integrates directly with `sys.path_hooks` so putting a `.zip` archive on
// `sys.path` lets `import pkg_in_zip` / `from pkg_in_zip import mod` work
// transparently.
//
// Reference: <https://docs.python.org/3.12/library/zipimport.html>

use std::path::{MAIN_SEPARATOR, Path, PathBuf};

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use super::marshal::marshal_loads_impl;
use super::py_compile::PYC_MAGIC_NUMBER;
use super::zlib::raw_inflate;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyPath, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{PyDict, PyKey, Value, ValueKind};

const END_CENTRAL_DIR_SIZE: usize = 22;
const MAX_COMMENT_LEN: usize = 65535;
const STRING_END_ARCHIVE: &[u8; 4] = b"PK\x05\x06";
const STRING_CENTRAL_DIR: &[u8; 4] = b"PK\x01\x02";
const STRING_LOCAL_HEADER: &[u8; 4] = b"PK\x03\x04";

const COMPRESS_STORED: u16 = 0;
const COMPRESS_DEFLATED: u16 = 8;

/// Metadata for a single file inside a ZIP archive's Central Directory.
#[derive(Debug, Clone)]
pub(crate) struct ZipTocEntry {
    /// Entry path normalized to forward slashes (`/`).
    pub zip_name: String,
    pub compress_method: u16,
    pub compressed_size: usize,
    pub uncompressed_size: usize,
    pub local_header_offset: usize,
    pub dos_time: u16,
    pub dos_date: u16,
    pub crc32: u32,
}

struct ZipImporterState {
    archive: String,
    /// Prefix inside the archive, using platform separator (`MAIN_SEPARATOR`)
    /// with a trailing separator when non-empty (matches CPython `zipimporter.prefix`).
    prefix: String,
    /// Prefix normalized with `/` and trailing `/` when non-empty.
    prefix_slash: String,
    /// Map from forward-slash zip member path to TOC entry.
    toc: IndexMap<String, ZipTocEntry>,
}

fn raise_zip_import_error(interp: &mut Interpreter, msg: impl Into<String>) -> PyError {
    let msg = msg.into();
    if let Ok(mod_val) = interp.load_module("zipimport")
        && let Ok(cls_val) = interp.get_attr(&mod_val, "ZipImportError")
        && let ValueKind::PyClass(cls_rc) = cls_val.kind()
    {
        return PyError::raised(crate::interpreter::instantiate_exception(
            std::rc::Rc::clone(cls_rc),
            vec![Value::string(&msg)],
        ));
    }
    PyError::import_error("ImportError", msg, None)
}

fn read_u16_le(data: &[u8], off: usize) -> Option<u16> {
    let b = data.get(off..off + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

fn read_u32_le(data: &[u8], off: usize) -> Option<u32> {
    let b = data.get(off..off + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Parse the ZIP Central Directory from raw archive bytes (supports shebang /
/// SFX prepended bytes via `archive_offset` adjustment).
pub(crate) fn parse_zip_toc(
    data: &[u8],
) -> std::result::Result<IndexMap<String, ZipTocEntry>, String> {
    if data.len() < END_CENTRAL_DIR_SIZE {
        return Err("not a Zip file".to_string());
    }
    let search_start = data
        .len()
        .saturating_sub(END_CENTRAL_DIR_SIZE + MAX_COMMENT_LEN);
    let mut eocd_pos = None;
    let mut pos = data.len() - END_CENTRAL_DIR_SIZE;
    loop {
        if &data[pos..pos + 4] == STRING_END_ARCHIVE {
            let comment_len = read_u16_le(data, pos + 20).unwrap_or(0) as usize;
            if pos + END_CENTRAL_DIR_SIZE + comment_len == data.len() {
                eocd_pos = Some(pos);
                break;
            }
        }
        if pos == search_start {
            break;
        }
        pos -= 1;
    }
    let eocd = eocd_pos.ok_or_else(|| "not a Zip file".to_string())?;
    let cd_size = read_u32_le(data, eocd + 12).ok_or("corrupt EOCD")? as usize;
    let cd_offset = read_u32_le(data, eocd + 16).ok_or("corrupt EOCD")? as usize;
    if cd_size > eocd {
        return Err("corrupt Zip central directory".to_string());
    }
    // Support prepended data (e.g. `#!` shebang in `.pyz` archives).
    let actual_cd_start = eocd - cd_size;
    let archive_offset = actual_cd_start.saturating_sub(cd_offset);

    let mut toc = IndexMap::new();
    let mut cur = actual_cd_start;
    let cd_end = eocd;
    while cur + 46 <= cd_end {
        if &data[cur..cur + 4] != STRING_CENTRAL_DIR {
            break;
        }
        let compress_method = read_u16_le(data, cur + 10).ok_or("corrupt CD entry")?;
        let dos_time = read_u16_le(data, cur + 12).ok_or("corrupt CD entry")?;
        let dos_date = read_u16_le(data, cur + 14).ok_or("corrupt CD entry")?;
        let crc32 = read_u32_le(data, cur + 16).ok_or("corrupt CD entry")?;
        let compressed_size = read_u32_le(data, cur + 20).ok_or("corrupt CD entry")? as usize;
        let uncompressed_size = read_u32_le(data, cur + 24).ok_or("corrupt CD entry")? as usize;
        let fname_len = read_u16_le(data, cur + 28).ok_or("corrupt CD entry")? as usize;
        let extra_len = read_u16_le(data, cur + 30).ok_or("corrupt CD entry")? as usize;
        let comment_len = read_u16_le(data, cur + 32).ok_or("corrupt CD entry")? as usize;
        let rel_header_offset = read_u32_le(data, cur + 42).ok_or("corrupt CD entry")? as usize;

        let name_start = cur + 46;
        let name_end = name_start + fname_len;
        if name_end > cd_end {
            return Err("truncated Zip central directory entry".to_string());
        }
        let raw_name = &data[name_start..name_end];
        let zip_name = String::from_utf8_lossy(raw_name).replace('\\', "/");
        let entry = ZipTocEntry {
            zip_name: zip_name.clone(),
            compress_method,
            compressed_size,
            uncompressed_size,
            local_header_offset: archive_offset + rel_header_offset,
            dos_time,
            dos_date,
            crc32,
        };
        toc.insert(zip_name, entry);
        cur = name_end + extra_len + comment_len;
    }
    Ok(toc)
}

/// Read and decompress a single member from raw ZIP bytes.
pub(crate) fn extract_zip_member(
    data: &[u8],
    entry: &ZipTocEntry,
) -> std::result::Result<Vec<u8>, String> {
    let off = entry.local_header_offset;
    if off + 30 > data.len() || &data[off..off + 4] != STRING_LOCAL_HEADER {
        return Err("bad local file header in Zip archive".to_string());
    }
    let fname_len = read_u16_le(data, off + 26).ok_or("corrupt local header")? as usize;
    let extra_len = read_u16_le(data, off + 28).ok_or("corrupt local header")? as usize;
    let payload_start = off + 30 + fname_len + extra_len;
    let payload_end = payload_start + entry.compressed_size;
    if payload_end > data.len() {
        return Err("truncated Zip entry payload".to_string());
    }
    let payload = &data[payload_start..payload_end];
    match entry.compress_method {
        COMPRESS_STORED => Ok(payload.to_vec()),
        COMPRESS_DEFLATED => raw_inflate(payload),
        m => Err(format!("unsupported Zip compression method: {m}")),
    }
}

fn read_archive_toc(archive: &str) -> std::result::Result<IndexMap<String, ZipTocEntry>, String> {
    let bytes = std::fs::read(archive).map_err(|e| format!("can't open Zip file: {e}"))?;
    parse_zip_toc(&bytes)
}

fn read_member_from_disk(
    archive: &str,
    entry: &ZipTocEntry,
) -> std::result::Result<Vec<u8>, String> {
    let bytes = std::fs::read(archive).map_err(|e| format!("can't open Zip file: {e}"))?;
    extract_zip_member(&bytes, entry)
}

/// Split an archive path like `/tmp/app.zip/pkg/sub` into
/// `(archive_file_path, prefix_with_platform_sep, prefix_with_forward_slash)`.
fn split_archive_and_prefix(path_str: &str) -> Option<(String, String, String)> {
    if path_str.is_empty() {
        return None;
    }
    // Try progressively shorter prefixes split at `/` or `\`.
    let mut candidate = path_str.to_string();
    let mut tail_parts: Vec<String> = Vec::new();
    loop {
        let p = Path::new(&candidate);
        if p.is_file() {
            tail_parts.reverse();
            if tail_parts.is_empty() {
                return Some((candidate, String::new(), String::new()));
            }
            let sep = MAIN_SEPARATOR.to_string();
            let prefix = format!("{}{}", tail_parts.join(&sep), sep);
            let prefix_slash = format!("{}/", tail_parts.join("/"));
            return Some((candidate, prefix, prefix_slash));
        }
        // Split last separator (`/` or `\` on Windows).
        let split_idx = candidate.rfind(|c| c == '/' || (cfg!(windows) && c == '\\'));
        match split_idx {
            Some(idx) if idx > 0 => {
                let tail = &candidate[idx + 1..];
                if !tail.is_empty() {
                    tail_parts.push(tail.to_string());
                }
                candidate.truncate(idx);
            }
            _ => return None,
        }
    }
}

struct ModuleMatch {
    entry: ZipTocEntry,
    is_package: bool,
    is_bytecode: bool,
}

fn find_module_in_toc(st: &ZipImporterState, fullname: &str) -> Option<ModuleMatch> {
    let tail = fullname.rsplit('.').next().unwrap_or(fullname);
    let base = format!("{}{}", st.prefix_slash, tail);
    // Prefer `.py` source so PyRust compiles executable bytecode; fall back to
    // `.pyc` when sourceless. Check package (`/__init__`) before plain module.
    let candidates = [
        (format!("{base}/__init__.py"), true, false),
        (format!("{base}/__init__.pyc"), true, true),
        (format!("{base}.py"), false, false),
        (format!("{base}.pyc"), false, true),
    ];
    for (zip_path, is_pkg, is_pyc) in candidates {
        if let Some(entry) = st.toc.get(&zip_path) {
            return Some(ModuleMatch {
                entry: entry.clone(),
                is_package: is_pkg,
                is_bytecode: is_pyc,
            });
        }
    }
    None
}

fn has_namespace_dir_in_toc(st: &ZipImporterState, fullname: &str) -> bool {
    let tail = fullname.rsplit('.').next().unwrap_or(fullname);
    let dir_prefix = format!("{}{}/", st.prefix_slash, tail);
    st.toc.keys().any(|k| k.starts_with(&dir_prefix))
}

fn format_zip_entry_filename(archive: &str, zip_name: &str) -> String {
    let sep = MAIN_SEPARATOR;
    let normalized = if sep == '\\' {
        zip_name.replace('/', "\\")
    } else {
        zip_name.to_string()
    };
    format!("{archive}{sep}{normalized}")
}

fn compile_zip_entry(interp: &mut Interpreter, archive: &str, m: &ModuleMatch) -> Result<Value> {
    let raw =
        read_member_from_disk(archive, &m.entry).map_err(|e| raise_zip_import_error(interp, e))?;
    let filename = format_zip_entry_filename(archive, &m.entry.zip_name);
    if m.is_bytecode {
        if raw.len() < 16 || raw[0..4] != PYC_MAGIC_NUMBER {
            return Err(raise_zip_import_error(
                interp,
                format!("bad magic number in {filename}"),
            ));
        }
        return marshal_loads_impl(interp, &raw[16..]);
    }
    let compile_fn = Value::builtin_function("compile");
    interp.call_function_expanded(
        compile_fn,
        &[
            ExpandedCallArg {
                name: None,
                value: Value::bytes(raw),
            },
            ExpandedCallArg {
                name: None,
                value: Value::string(filename),
            },
            ExpandedCallArg {
                name: None,
                value: Value::string("exec"),
            },
            ExpandedCallArg {
                name: Some("dont_inherit".to_string()),
                value: Value::bool_(true),
            },
        ],
    )
}

fn make_files_dict(archive: &str, toc: &IndexMap<String, ZipTocEntry>) -> Value {
    let mut d = PyDict::default();
    for (k, e) in toc {
        let key_str = if MAIN_SEPARATOR == '\\' {
            k.replace('/', "\\")
        } else {
            k.clone()
        };
        let full = format_zip_entry_filename(archive, k);
        let tup = Value::tuple(vec![
            Value::string(full),
            Value::int(i64::from(e.compress_method)),
            Value::int(e.compressed_size as i64),
            Value::int(e.uncompressed_size as i64),
            Value::int(e.local_header_offset as i64),
            Value::int(i64::from(e.dos_time)),
            Value::int(i64::from(e.dos_date)),
            Value::int(i64::from(e.crc32)),
        ]);
        d.insert(PyKey::str_from(&key_str), tup);
    }
    Value::dict(d)
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("ZipImportError"),
            Value::string("zipimporter"),
        ]),
        "END_CENTRAL_DIR_SIZE" => Value::int(END_CENTRAL_DIR_SIZE as i64),
        "MAX_COMMENT_LEN" => Value::int(MAX_COMMENT_LEN as i64),
        "STRING_END_ARCHIVE" => Value::bytes(STRING_END_ARCHIVE.to_vec()),
        "path_sep" => Value::string(MAIN_SEPARATOR.to_string()),
        "alt_path_sep" => if cfg!(windows) { Value::string("/") } else { Value::none() },
    }

    exception ZipImportError(ImportError);

    #[native_state(ZipImporterState)]
    class zipimporter {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            path: PyPath,
        ) -> Result<Value> {
            if path.is_bytes() {
                return Err(PyError::named(
                    "TypeError",
                    "zipimporter() argument must be str or os.PathLike, not bytes".to_string(),
                ));
            }
            let path_str = path.as_path().to_string_lossy().into_owned();
            if path_str.is_empty() {
                return Err(raise_zip_import_error(_interp, "archive path is empty"));
            }
            let Some((archive, prefix, prefix_slash)) = split_archive_and_prefix(&path_str) else {
                return Err(raise_zip_import_error(
                    _interp,
                    format!("not a Zip file: {path_str:?}"),
                ));
            };
            let toc = read_archive_toc(&archive)
                .map_err(|e| raise_zip_import_error(_interp, format!("{e}: {archive:?}")))?;
            set_state(
                self_val.0,
                ZipImporterState {
                    archive,
                    prefix,
                    prefix_slash,
                    toc,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn archive(#[state] st: &ZipImporterState) -> Result<Value> {
            Ok(Value::string(&st.archive))
        }

        #[getter]
        fn prefix(#[state] st: &ZipImporterState) -> Result<Value> {
            Ok(Value::string(&st.prefix))
        }

        #[getter]
        fn _files(#[state] st: &ZipImporterState) -> Result<Value> {
            Ok(make_files_dict(&st.archive, &st.toc))
        }

        fn __repr__(#[state] st: &ZipImporterState) -> Result<Value> {
            let sep = MAIN_SEPARATOR;
            Ok(Value::string(format!(
                "<zipimporter object \"{}{}{}\">",
                st.archive, sep, st.prefix
            )))
        }

        fn find_spec(
            #[positional_only] self_val: PyValueRef<'_>,
            fullname: PyStr<'_>,
            #[default(None)] target: Option<PyValue>,
        ) -> Result<Value> {
            let _ = target;
            let (archive, matched, ns_dir) = with_state(self_val.0, |st| {
                let m = find_module_in_toc(st, &fullname.0);
                let ns = if m.is_none() && has_namespace_dir_in_toc(st, &fullname.0) {
                    let tail = fullname.0.rsplit('.').next().unwrap_or(&fullname.0);
                    let sub = format!("{}{}", st.prefix_slash, tail);
                    Some(format_zip_entry_filename(&st.archive, &sub))
                } else {
                    None
                };
                Ok((st.archive.clone(), m, ns))
            })?;

            let machinery = _interp.load_module("importlib.machinery")?;
            let spec_cls = _interp.get_attr(&machinery, "ModuleSpec")?;

            if let Some(m) = matched {
                let origin = format_zip_entry_filename(&archive, &m.entry.zip_name);
                let spec = _interp.call_function_expanded(
                    spec_cls,
                    &[
                        ExpandedCallArg {
                            name: None,
                            value: Value::string(&fullname.0),
                        },
                        ExpandedCallArg {
                            name: None,
                            value: self_val.0.clone(),
                        },
                        ExpandedCallArg {
                            name: Some("origin".to_string()),
                            value: Value::string(&origin),
                        },
                        ExpandedCallArg {
                            name: Some("is_package".to_string()),
                            value: Value::bool_(m.is_package),
                        },
                    ],
                )?;
                let _ = _interp.assign_attr(spec.clone(), "has_location", Value::bool_(true));
                if m.is_package {
                    let pkg_dir = PathBuf::from(&origin)
                        .parent()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or(archive);
                    let _ = _interp.assign_attr(
                        spec.clone(),
                        "submodule_search_locations",
                        Value::list(vec![Value::string(pkg_dir)]),
                    );
                }
                return Ok(spec);
            }

            if let Some(ns_path) = ns_dir {
                let spec = _interp.call_function_expanded(
                    spec_cls,
                    &[
                        ExpandedCallArg {
                            name: None,
                            value: Value::string(fullname.0),
                        },
                        ExpandedCallArg {
                            name: None,
                            value: Value::none(),
                        },
                    ],
                )?;
                let _ = _interp.assign_attr(
                    spec.clone(),
                    "submodule_search_locations",
                    Value::list(vec![Value::string(ns_path)]),
                );
                return Ok(spec);
            }

            Ok(Value::none())
        }

        fn create_module(
            #[positional_only] _self: PyValueRef<'_>,
            spec: PyValue,
        ) -> Result<Value> {
            let _ = spec;
            Ok(Value::none())
        }

        fn exec_module(
            #[positional_only] self_val: PyValueRef<'_>,
            module: PyValue,
        ) -> Result<Value> {
            let fullname = _interp
                .get_attr(&module.0, "__name__")?
                .as_str()
                .ok_or_else(|| {
                    PyError::named("TypeError", "module.__name__ must be str".to_string())
                })?
                .to_string();
            let (archive, matched) = with_state(self_val.0, |st| {
                Ok((st.archive.clone(), find_module_in_toc(st, &fullname)))
            })?;
            let Some(m) = matched else {
                return Err(raise_zip_import_error(
                    _interp,
                    format!("can't find module {fullname:?}"),
                ));
            };
            let code = compile_zip_entry(_interp, &archive, &m)?;
            let mod_dict = _interp.get_attr(&module.0, "__dict__")?;
            _interp.call_function_expanded(
                Value::builtin_function("exec"),
                &[
                    ExpandedCallArg {
                        name: None,
                        value: code,
                    },
                    ExpandedCallArg {
                        name: None,
                        value: mod_dict,
                    },
                ],
            )?;
            Ok(Value::none())
        }

        fn get_code(
            #[positional_only] self_val: PyValueRef<'_>,
            fullname: PyStr<'_>,
        ) -> Result<Value> {
            let (archive, matched) = with_state(self_val.0, |st| {
                Ok((st.archive.clone(), find_module_in_toc(st, &fullname.0)))
            })?;
            let Some(m) = matched else {
                return Err(raise_zip_import_error(
                    _interp,
                    format!("can't find module {:?}", fullname.0),
                ));
            };
            compile_zip_entry(_interp, &archive, &m)
        }

        fn get_data(
            #[positional_only] self_val: PyValueRef<'_>,
            pathname: PyValue,
        ) -> Result<Value> {
            let raw_path = pathname.0.as_str().ok_or_else(|| {
                PyError::named(
                    "TypeError",
                    format!(
                        "get_data() argument must be str, not {}",
                        value_type_name_str(&pathname.0)
                    ),
                )
            })?;
            let (archive, entry_opt) = with_state(self_val.0, |st| {
                let archive_norm = st.archive.replace('\\', "/");
                let path_norm = raw_path.replace('\\', "/");
                let rel = if let Some(rest) = path_norm.strip_prefix(&archive_norm) {
                    rest.trim_start_matches('/').to_string()
                } else {
                    path_norm.trim_start_matches('/').to_string()
                };
                Ok((st.archive.clone(), st.toc.get(&rel).cloned()))
            })?;
            let Some(entry) = entry_opt else {
                let io_err = std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("No such file in Zip archive: {raw_path:?}"),
                );
                return Err(PyError::from_io_error(&io_err, Some(raw_path)));
            };
            let data = read_member_from_disk(&archive, &entry).map_err(|e| {
                let io_err = std::io::Error::other(e);
                PyError::from_io_error(&io_err, Some(raw_path))
            })?;
            Ok(Value::bytes(data))
        }

        fn get_filename(
            #[positional_only] self_val: PyValueRef<'_>,
            fullname: PyStr<'_>,
        ) -> Result<Value> {
            let (archive, matched) = with_state(self_val.0, |st| {
                Ok((st.archive.clone(), find_module_in_toc(st, &fullname.0)))
            })?;
            let Some(m) = matched else {
                return Err(raise_zip_import_error(
                    _interp,
                    format!("can't find module {:?}", fullname.0),
                ));
            };
            Ok(Value::string(format_zip_entry_filename(
                &archive,
                &m.entry.zip_name,
            )))
        }

        fn get_source(
            #[positional_only] self_val: PyValueRef<'_>,
            fullname: PyStr<'_>,
        ) -> Result<Value> {
            let (archive, matched) = with_state(self_val.0, |st| {
                Ok((st.archive.clone(), find_module_in_toc(st, &fullname.0)))
            })?;
            let Some(m) = matched else {
                return Err(raise_zip_import_error(
                    _interp,
                    format!("can't find module {:?}", fullname.0),
                ));
            };
            if m.is_bytecode {
                return Ok(Value::none());
            }
            let raw = read_member_from_disk(&archive, &m.entry)
                .map_err(|e| raise_zip_import_error(_interp, e))?;
            let text = String::from_utf8_lossy(&raw)
                .replace("\r\n", "\n")
                .replace('\r', "\n");
            Ok(Value::string(text))
        }

        fn is_package(
            #[positional_only] self_val: PyValueRef<'_>,
            fullname: PyStr<'_>,
        ) -> Result<Value> {
            let matched = with_state(self_val.0, |st| Ok(find_module_in_toc(st, &fullname.0)))?;
            let Some(m) = matched else {
                return Err(raise_zip_import_error(
                    _interp,
                    format!("can't find module {:?}", fullname.0),
                ));
            };
            Ok(Value::bool_(m.is_package))
        }

        fn invalidate_caches(#[state_mut] st: &mut ZipImporterState) -> Result<Value> {
            if let Ok(toc) = read_archive_toc(&st.archive) {
                st.toc = toc;
            }
            Ok(Value::none())
        }
    }
}
