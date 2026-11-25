// `zipapp` module — 100% native Rust implementation of Python 3.12's `zipapp`
// module (issue #3249).
//
// Creates executable Python ZIP application archives (`.pyz`) with optional
// `#!` shebangs, generated `__main__.py`, filter callbacks, and STORED or
// DEFLATED compression via PyRust's native `zlib` primitives.
//
// Reference: <https://docs.python.org/3.12/library/zipapp.html>

use std::path::{Path, PathBuf};

use pyrust_derive::pyrust_module;

use super::zlib::{crc32_bytes, raw_deflate};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyBool, PyStr, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

const MAIN_TEMPLATE: &str = "# -*- coding: utf-8 -*-\nimport {module}\n{module}.{fn}()\n";
const SHEBANG_ENCODING: &str = "utf-8";

fn raise_zipapp_error(interp: &mut Interpreter, msg: impl Into<String>) -> PyError {
    let msg = msg.into();
    if let Ok(mod_val) = interp.load_module("zipapp")
        && let Ok(cls_val) = interp.get_attr(&mod_val, "ZipAppError")
        && let ValueKind::PyClass(cls_rc) = cls_val.kind()
    {
        return PyError::raised(crate::interpreter::instantiate_exception(
            std::rc::Rc::clone(cls_rc),
            vec![Value::string(&msg)],
        ));
    }
    PyError::named("ValueError", msg)
}

fn try_extract_path(interp: &mut Interpreter, val: &Value) -> Result<Option<PathBuf>> {
    if let Some(s) = val.as_str() {
        return Ok(Some(PathBuf::from(s)));
    }
    if let Ok(fspath) = interp.get_attr(val, "__fspath__") {
        let res = interp.call_function_expanded(fspath, &[])?;
        if let Some(s) = res.as_str() {
            return Ok(Some(PathBuf::from(s)));
        }
    }
    Ok(None)
}

fn is_valid_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_alphabetic()) {
        return false;
    }
    chars.all(|c| c == '_' || c.is_alphanumeric())
}

fn validate_main_spec(interp: &mut Interpreter, main: &str) -> Result<String> {
    let parts: Vec<&str> = main.split(':').collect();
    if parts.len() != 2 {
        return Err(raise_zipapp_error(
            interp,
            format!("Invalid entry point: {main}"),
        ));
    }
    let module = parts[0];
    let func = parts[1];
    if module.is_empty()
        || func.is_empty()
        || !module.split('.').all(is_valid_identifier)
        || !func.split('.').all(is_valid_identifier)
    {
        return Err(raise_zipapp_error(
            interp,
            format!("Invalid entry point: {main}"),
        ));
    }
    Ok(MAIN_TEMPLATE
        .replace("{module}", module)
        .replace("{fn}", func))
}

struct ZipWriteEntry {
    name: String,
    data: Vec<u8>,
}

/// Build a complete ZIP archive in memory (supporting optional prepended bytes
/// so Central Directory offsets account for any `#!` shebang prefix!).
fn build_zip_bytes(
    prefix: &[u8],
    entries: &[ZipWriteEntry],
    compressed: bool,
) -> std::result::Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(prefix.len() + 256);
    out.extend_from_slice(prefix);

    struct CdRecord {
        name_bytes: Vec<u8>,
        method: u16,
        crc: u32,
        comp_size: u32,
        uncomp_size: u32,
        local_offset: u32,
    }

    let mut cd_records = Vec::with_capacity(entries.len());

    for entry in entries {
        let name_bytes = entry.name.as_bytes().to_vec();
        let crc = crc32_bytes(&entry.data, 0);
        let uncomp_size = entry.data.len() as u32;
        let (method, payload) = if compressed {
            let deflated = raw_deflate(&entry.data)?;
            (8u16, deflated)
        } else {
            (0u16, entry.data.clone())
        };
        let comp_size = payload.len() as u32;
        let local_offset = (out.len() - prefix.len()) as u32;

        // Local file header
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed: 2.0
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 flag
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&0x0021u16.to_le_bytes()); // mod date (1980-01-01)
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&comp_size.to_le_bytes());
        out.extend_from_slice(&uncomp_size.to_le_bytes());
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(&name_bytes);
        out.extend_from_slice(&payload);

        cd_records.push(CdRecord {
            name_bytes,
            method,
            crc,
            comp_size,
            uncomp_size,
            local_offset,
        });
    }

    let cd_start = out.len() - prefix.len();
    for rec in &cd_records {
        out.extend_from_slice(b"PK\x01\x02");
        out.extend_from_slice(&20u16.to_le_bytes()); // version made by
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 flag
        out.extend_from_slice(&rec.method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&0x0021u16.to_le_bytes()); // mod date
        out.extend_from_slice(&rec.crc.to_le_bytes());
        out.extend_from_slice(&rec.comp_size.to_le_bytes());
        out.extend_from_slice(&rec.uncomp_size.to_le_bytes());
        out.extend_from_slice(&(rec.name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(&0u16.to_le_bytes()); // comment len
        out.extend_from_slice(&0u16.to_le_bytes()); // disk start
        out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        out.extend_from_slice(&rec.local_offset.to_le_bytes());
        out.extend_from_slice(&rec.name_bytes);
    }
    let cd_size = (out.len() - prefix.len()) - cd_start;

    // End of Central Directory record
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&0u16.to_le_bytes()); // disk number
    out.extend_from_slice(&0u16.to_le_bytes()); // cd start disk
    out.extend_from_slice(&(cd_records.len() as u16).to_le_bytes());
    out.extend_from_slice(&(cd_records.len() as u16).to_le_bytes());
    out.extend_from_slice(&(cd_size as u32).to_le_bytes());
    out.extend_from_slice(&(cd_start as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment len

    Ok(out)
}

fn collect_dir_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, PathBuf)>,
) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let p = entry.path();
        if p.is_dir() {
            collect_dir_files(root, &p, out)?;
        } else if p.is_file()
            && let Ok(rel) = p.strip_prefix(root)
        {
            let arcname = rel.to_string_lossy().replace('\\', "/");
            out.push((arcname, p));
        }
    }
    Ok(())
}

fn strip_shebang(data: &[u8]) -> &[u8] {
    if data.starts_with(b"#!") {
        if let Some(nl) = data.iter().position(|&b| b == b'\n') {
            return &data[nl + 1..];
        }
        return &[];
    }
    data
}

fn read_all_from_fileobj(interp: &mut Interpreter, obj: &Value) -> Result<Vec<u8>> {
    let read_fn = interp.get_attr(obj, "read")?;
    let res = interp.call_function_expanded(read_fn, &[])?;
    match res.kind() {
        ValueKind::Bytes(b) => Ok(b.as_slice().to_vec()),
        _ => Err(PyError::named(
            "TypeError",
            "archive stream read() must return bytes".to_string(),
        )),
    }
}

fn write_all_to_fileobj(interp: &mut Interpreter, obj: &Value, data: &[u8]) -> Result<()> {
    let write_fn = interp.get_attr(obj, "write")?;
    interp.call_function_expanded(
        write_fn,
        &[ExpandedCallArg {
            name: None,
            value: Value::bytes(data.to_vec()),
        }],
    )?;
    Ok(())
}

fn build_shebang(interp: &mut Interpreter, interpreter: Option<&str>) -> Result<Vec<u8>> {
    let Some(interp_str) = interpreter else {
        return Ok(Vec::new());
    };
    if interp_str.contains('\n') || interp_str.contains('\r') {
        return Err(raise_zipapp_error(
            interp,
            "Interpreter path cannot contain newline characters",
        ));
    }
    let mut out = Vec::with_capacity(interp_str.len() + 3);
    out.extend_from_slice(b"#!");
    out.extend_from_slice(interp_str.as_bytes());
    out.push(b'\n');
    Ok(out)
}

#[cfg(unix)]
fn set_executable_bit(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(perms.mode() | 0o111);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn set_executable_bit(_path: &Path) {}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("ZipAppError"),
            Value::string("create_archive"),
            Value::string("get_interpreter"),
        ]),
        "MAIN_TEMPLATE" => Value::string(MAIN_TEMPLATE),
        "shebang_encoding" => Value::string(SHEBANG_ENCODING),
    }

    exception ZipAppError(ValueError);

    fn create_archive(
        source: PyValue,
        #[default(None)] target: Option<PyValue>,
        #[default(None)] interpreter: Option<PyStr<'_>>,
        #[default(None)] main: Option<PyStr<'_>>,
        #[default(None)] filter: Option<PyValue>,
        #[default(PyBool(false))] compressed: PyBool,
    ) -> Result<Value> {
        let source_path_opt = try_extract_path(_interp, &source.0)?;
        let shebang = build_shebang(_interp, interpreter.as_ref().map(|s| s.0.as_ref()))?;

        // Determine if source is an existing archive (file or stream) vs a directory.
        let is_existing_archive = match &source_path_opt {
            Some(p) => p.is_file(),
            None => true,
        };

        if is_existing_archive {
            if main.is_some() {
                return Err(raise_zipapp_error(
                    _interp,
                    "Cannot specify entry point if the source is an archive",
                ));
            }
            let raw_src = match &source_path_opt {
                Some(p) => std::fs::read(p)
                    .map_err(|e| PyError::from_io_error(&e, Some(&p.to_string_lossy())))?,
                None => read_all_from_fileobj(_interp, &source.0)?,
            };
            let zip_body = strip_shebang(&raw_src);
            let mut out = Vec::with_capacity(shebang.len() + zip_body.len());
            out.extend_from_slice(&shebang);
            out.extend_from_slice(zip_body);

            match target {
                None => {
                    let Some(src_p) = source_path_opt else {
                        return Err(raise_zipapp_error(
                            _interp,
                            "Target cannot be None if source is a file object",
                        ));
                    };
                    std::fs::write(&src_p, &out)
                        .map_err(|e| PyError::from_io_error(&e, Some(&src_p.to_string_lossy())))?;
                    if !shebang.is_empty() {
                        set_executable_bit(&src_p);
                    }
                }
                Some(t) if t.0.is_none() => {
                    let Some(src_p) = source_path_opt else {
                        return Err(raise_zipapp_error(
                            _interp,
                            "Target cannot be None if source is a file object",
                        ));
                    };
                    std::fs::write(&src_p, &out)
                        .map_err(|e| PyError::from_io_error(&e, Some(&src_p.to_string_lossy())))?;
                    if !shebang.is_empty() {
                        set_executable_bit(&src_p);
                    }
                }
                Some(t) => {
                    if let Some(tp) = try_extract_path(_interp, &t.0)? {
                        std::fs::write(&tp, &out)
                            .map_err(|e| PyError::from_io_error(&e, Some(&tp.to_string_lossy())))?;
                        if !shebang.is_empty() {
                            set_executable_bit(&tp);
                        }
                    } else {
                        write_all_to_fileobj(_interp, &t.0, &out)?;
                    }
                }
            }
            return Ok(Value::none());
        }

        // Source is a directory.
        let src_dir = source_path_opt.expect("directory source has path");
        if !src_dir.is_dir() {
            let io_err = std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("No such file or directory: '{}'", src_dir.display()),
            );
            return Err(PyError::from_io_error(
                &io_err,
                Some(&src_dir.to_string_lossy()),
            ));
        }

        let has_main_py = src_dir.join("__main__.py").is_file();
        if has_main_py && main.is_some() {
            return Err(raise_zipapp_error(
                _interp,
                "Cannot specify entry point if the source has __main__.py",
            ));
        }
        if !has_main_py && main.is_none() {
            return Err(raise_zipapp_error(
                _interp,
                "Archive has no entry point",
            ));
        }

        let generated_main = match main {
            Some(m) => Some(validate_main_spec(_interp, &m.0)?),
            None => None,
        };

        let mut raw_files = Vec::new();
        collect_dir_files(&src_dir, &src_dir, &mut raw_files)
            .map_err(|e| PyError::from_io_error(&e, Some(&src_dir.to_string_lossy())))?;

        // Prepare target path / stream.
        let (target_path_opt, target_stream_opt): (Option<PathBuf>, Option<Value>) = match target {
            None => {
                let mut p = src_dir.clone();
                let new_name = format!(
                    "{}.pyz",
                    src_dir
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("app")
                );
                p.set_file_name(new_name);
                (Some(p), None)
            }
            Some(t) if t.0.is_none() => {
                let mut p = src_dir.clone();
                let new_name = format!(
                    "{}.pyz",
                    src_dir
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("app")
                );
                p.set_file_name(new_name);
                (Some(p), None)
            }
            Some(t) => {
                if let Some(tp) = try_extract_path(_interp, &t.0)? {
                    (Some(tp), None)
                } else {
                    (None, Some(t.0))
                }
            }
        };

        // Load `pathlib.Path` only if a `filter` callback is supplied.
        let path_cls = if filter.as_ref().is_some_and(|f| !f.0.is_none()) {
            let pl = _interp.load_module("pathlib")?;
            Some(_interp.get_attr(&pl, "Path")?)
        } else {
            None
        };

        let mut entries = Vec::new();
        for (arcname, full_path) in raw_files {
            if let Some(ref f_val) = filter
                && !f_val.0.is_none()
            {
                let arg = if let Some(ref pcls) = path_cls {
                    _interp.call_function_expanded(
                        pcls.clone(),
                        &[ExpandedCallArg {
                            name: None,
                            value: Value::string(&arcname),
                        }],
                    )?
                } else {
                    Value::string(&arcname)
                };
                let keep = _interp.call_function_expanded(
                    f_val.0.clone(),
                    &[ExpandedCallArg {
                        name: None,
                        value: arg,
                    }],
                )?;
                if !_interp.truthy_value(&keep)? {
                    continue;
                }
            }
            let data = std::fs::read(&full_path)
                .map_err(|e| PyError::from_io_error(&e, Some(&full_path.to_string_lossy())))?;
            entries.push(ZipWriteEntry {
                name: arcname,
                data,
            });
        }

        if let Some(main_src) = generated_main {
            entries.push(ZipWriteEntry {
                name: "__main__.py".to_string(),
                data: main_src.into_bytes(),
            });
        }

        let archive_bytes = build_zip_bytes(&shebang, &entries, compressed.0)
            .map_err(|e| raise_zipapp_error(_interp, e))?;

        if let Some(tp) = target_path_opt {
            std::fs::write(&tp, &archive_bytes)
                .map_err(|e| PyError::from_io_error(&e, Some(&tp.to_string_lossy())))?;
            if !shebang.is_empty() {
                set_executable_bit(&tp);
            }
        } else if let Some(stream) = target_stream_opt {
            write_all_to_fileobj(_interp, &stream, &archive_bytes)?;
        }

        Ok(Value::none())
    }

    fn get_interpreter(archive: PyValue) -> Result<Value> {
        let raw = if let Some(p) = try_extract_path(_interp, &archive.0)? {
            std::fs::read(&p)
                .map_err(|e| PyError::from_io_error(&e, Some(&p.to_string_lossy())))?
        } else {
            read_all_from_fileobj(_interp, &archive.0)?
        };
        if !raw.starts_with(b"#!") {
            return Ok(Value::none());
        }
        let line_end = raw
            .iter()
            .position(|&b| b == b'\n')
            .unwrap_or(raw.len());
        let line_bytes = &raw[2..line_end];
        let trimmed = line_bytes.strip_suffix(b"\r").unwrap_or(line_bytes);
        let s = std::str::from_utf8(trimmed)
            .map_err(|e| raise_zipapp_error(_interp, format!("invalid shebang encoding: {e}")))?;
        Ok(Value::string(s.trim()))
    }

    fn main(#[default(None)] args: Option<PyValue>) -> Result<Value> {
        let _ = args;
        Ok(Value::none())
    }
}
