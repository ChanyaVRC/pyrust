//! Native `os.scandir`, `ScandirIterator`, and `DirEntry` backing state and logic.
//!
//! Uses `#[native_state(T)]` classes declared in `bodies/os.rs` so Python code
//! sees genuine `DirEntry` / `ScandirIterator` objects with cached stat results,
//! context-manager support, and `os.PathLike` (`__fspath__`) integration.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::builtin_modules::genericpath::ops::{PathValue, fspath_resolve, path_buf_to_bytes};
use crate::builtin_modules::macros::native_state::set_native_state;
use crate::builtin_modules::ntpath::ops::path_is_junction;
use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value, ValueKind};

use super::result_types::make_stat_result_for_path;

/// Typed native payload for `os.DirEntry`.
pub(super) struct DirEntryState {
    name_str: String,
    name_bytes: Vec<u8>,
    path_str: String,
    path_bytes: Vec<u8>,
    path_buf: PathBuf,
    is_bytes: bool,
    lstat_cache: Option<std::fs::Metadata>,
    stat_cache: Option<std::fs::Metadata>,
}

impl DirEntryState {
    fn ensure_lstat(&mut self) -> std::io::Result<&std::fs::Metadata> {
        if self.lstat_cache.is_none() {
            let meta = std::fs::symlink_metadata(&self.path_buf)?;
            self.lstat_cache = Some(meta);
        }
        Ok(self.lstat_cache.as_ref().expect("populated above"))
    }

    fn ensure_stat(&mut self) -> std::io::Result<&std::fs::Metadata> {
        if self.stat_cache.is_none() {
            let meta = std::fs::metadata(&self.path_buf)?;
            self.stat_cache = Some(meta);
        }
        Ok(self.stat_cache.as_ref().expect("populated above"))
    }
}

/// Typed native payload for `ScandirIterator`.
pub(super) struct ScandirState {
    read_dir: Option<std::fs::ReadDir>,
    base_str: String,
    base_bytes: Vec<u8>,
    is_bytes: bool,
    dir_entry_class: Rc<RefCell<PyClass>>,
}

fn lookup_os_class(interp: &mut Interpreter, class_name: &str) -> Result<Rc<RefCell<PyClass>>> {
    let os_mod = interp.load_module("os")?;
    let cls_val = interp.get_attr(&os_mod, class_name)?;
    match cls_val.kind() {
        ValueKind::PyClass(rc) => Ok(Rc::clone(rc)),
        _ => Err(PyError::runtime(format!(
            "internal: os.{class_name} is not a class"
        ))),
    }
}

fn make_native_instance<T: 'static>(class: Rc<RefCell<PyClass>>, state: T) -> Result<Value> {
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(&inst, state)?;
    Ok(inst)
}

pub(super) fn scandir_open(interp: &mut Interpreter, path_arg: Option<&Value>) -> Result<Value> {
    let (path_buf, base_str, base_bytes, is_bytes) = match path_arg {
        None => (PathBuf::from("."), ".".to_string(), b".".to_vec(), false),
        Some(v) if v.is_none() => (PathBuf::from("."), ".".to_string(), b".".to_vec(), false),
        Some(v) => {
            let pv = fspath_resolve(interp, v)?;
            let pb = pv.to_path_buf()?;
            match pv {
                PathValue::Str(s) => {
                    let b = s.as_bytes().to_vec();
                    (pb, s, b, false)
                }
                PathValue::Bytes(b) => {
                    let s = String::from_utf8_lossy(&b).into_owned();
                    (pb, s, b, true)
                }
            }
        }
    };

    let read_dir =
        std::fs::read_dir(&path_buf).map_err(|e| PyError::from_io_error(&e, Some(&base_str)))?;
    let iter_cls = lookup_os_class(interp, "ScandirIterator")?;
    let entry_cls = lookup_os_class(interp, "DirEntry")?;

    make_native_instance(
        iter_cls,
        ScandirState {
            read_dir: Some(read_dir),
            base_str,
            base_bytes,
            is_bytes,
            dir_entry_class: entry_cls,
        },
    )
}

pub(super) fn scandir_next(st: &mut ScandirState) -> Result<Value> {
    let Some(read_dir) = st.read_dir.as_mut() else {
        return Err(PyError::named("StopIteration", String::new()));
    };
    match read_dir.next() {
        None => {
            st.read_dir = None;
            Err(PyError::named("StopIteration", String::new()))
        }
        Some(Err(e)) => {
            st.read_dir = None;
            Err(PyError::from_io_error(&e, Some(&st.base_str)))
        }
        Some(Ok(entry)) => {
            let os_name = entry.file_name();
            let name_str = os_name.to_string_lossy().into_owned();
            let name_bytes = path_buf_to_bytes(std::path::Path::new(&os_name));
            let sep = std::path::MAIN_SEPARATOR;
            let sep_b = sep as u8;

            let path_str = if st.base_str.is_empty() {
                name_str.clone()
            } else if st.base_str.ends_with(sep) || (cfg!(windows) && st.base_str.ends_with('/')) {
                format!("{}{}", st.base_str, name_str)
            } else {
                format!("{}{}{}", st.base_str, sep, name_str)
            };

            let mut path_bytes = st.base_bytes.clone();
            if !path_bytes.is_empty()
                && !path_bytes.ends_with(&[sep_b])
                && !(cfg!(windows) && path_bytes.ends_with(b"/"))
            {
                path_bytes.push(sep_b);
            }
            path_bytes.extend_from_slice(&name_bytes);

            let path_buf = entry.path();
            let dir_entry_cls = Rc::clone(&st.dir_entry_class);
            make_native_instance(
                dir_entry_cls,
                DirEntryState {
                    name_str,
                    name_bytes,
                    path_str,
                    path_bytes,
                    path_buf,
                    is_bytes: st.is_bytes,
                    lstat_cache: None,
                    stat_cache: None,
                },
            )
        }
    }
}

pub(super) fn scandir_close(st: &mut ScandirState) -> Result<Value> {
    st.read_dir = None;
    Ok(Value::none())
}

// ── DirEntry methods & getters ───────────────────────────────────────────────

pub(super) fn entry_name(st: &DirEntryState) -> Result<Value> {
    if st.is_bytes {
        Ok(Value::bytes(st.name_bytes.clone()))
    } else {
        Ok(Value::string(&st.name_str))
    }
}

pub(super) fn entry_path(st: &DirEntryState) -> Result<Value> {
    if st.is_bytes {
        Ok(Value::bytes(st.path_bytes.clone()))
    } else {
        Ok(Value::string(&st.path_str))
    }
}

pub(super) fn entry_repr(st: &DirEntryState) -> Result<Value> {
    let name_val = entry_name(st)?;
    Ok(Value::string(format!("<DirEntry {}>", name_val.repr_raw())))
}

pub(super) fn entry_inode(st: &mut DirEntryState) -> Result<Value> {
    let p_str = st.path_str.clone();
    let pb = st.path_buf.clone();
    let meta = st
        .ensure_lstat()
        .map_err(|e| PyError::from_io_error(&e, Some(&p_str)))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = pb;
        Ok(Value::int(meta.ino() as i64))
    }
    #[cfg(windows)]
    {
        let _ = meta;
        let (ino, _, _) = super::result_types::win_path_file_ids(&pb, false).unwrap_or((0, 0, 1));
        if ino <= i64::MAX as u64 {
            Ok(Value::int(ino as i64))
        } else {
            Ok(Value::bigint(crate::value::PyBigInt::from(ino)))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (meta, pb);
        Ok(Value::int(0))
    }
}

pub(super) fn entry_is_symlink(st: &mut DirEntryState) -> Result<Value> {
    let ok = st.ensure_lstat().is_ok_and(|m| m.file_type().is_symlink());
    Ok(Value::bool_(ok))
}

pub(super) fn entry_is_junction(st: &DirEntryState) -> Result<Value> {
    Ok(Value::bool_(path_is_junction(&st.path_buf)))
}

pub(super) fn entry_is_dir(st: &mut DirEntryState, follow_symlinks: bool) -> Result<Value> {
    let ok = if follow_symlinks {
        st.ensure_stat().is_ok_and(|m| m.is_dir())
    } else {
        st.ensure_lstat()
            .is_ok_and(|m| !m.file_type().is_symlink() && m.is_dir())
    };
    Ok(Value::bool_(ok))
}

pub(super) fn entry_is_file(st: &mut DirEntryState, follow_symlinks: bool) -> Result<Value> {
    let ok = if follow_symlinks {
        st.ensure_stat().is_ok_and(|m| m.is_file())
    } else {
        st.ensure_lstat()
            .is_ok_and(|m| !m.file_type().is_symlink() && m.is_file())
    };
    Ok(Value::bool_(ok))
}

pub(super) fn entry_stat(st: &mut DirEntryState, follow_symlinks: bool) -> Result<Value> {
    let p_str = st.path_str.clone();
    let pb = st.path_buf.clone();
    let meta = if follow_symlinks {
        st.ensure_stat()
    } else {
        st.ensure_lstat()
    }
    .map_err(|e| PyError::from_io_error(&e, Some(&p_str)))?
    .clone();
    Ok(make_stat_result_for_path(&pb, &meta, follow_symlinks))
}
