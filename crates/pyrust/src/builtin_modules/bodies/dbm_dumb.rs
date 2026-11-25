// `dbm.dumb` module — 100% native Rust implementation of Python 3.12's
// `dbm.dumb` persistent key-value database (issue #3259).
//
// Exact bidirectional CPython 3.12 on-disk `.dir` + `.dat` (+ `.bak`) wire
// format parity: databases created by CPython `dbm.dumb` open in PyRust and
// vice versa on both Unix and Windows.
//
// Reference: <https://docs.python.org/3/library/dbm.html#module-dbm.dumb>

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyIndex, PyPath, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, NativeIterFrame, lookup_exc_class};
use crate::value::{Value, ValueKind};

const BLOCKSIZE: u64 = 512;

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn raise_key_error(interp: &mut Interpreter, key: Value) -> PyError {
    if let Some(cls) = lookup_exc_class("KeyError")
        && let Ok(inst) = call_pos(interp, Value::py_class(cls), &[key])
    {
        return PyError::raised(inst);
    }
    PyError::named("KeyError", "key not found")
}

fn raise_dbm_os_error(msg: &str) -> PyError {
    PyError::named("OSError", msg)
}

fn append_suffix(base: &Path, suffix: &str) -> PathBuf {
    let mut os = OsString::from(base.as_os_str());
    os.push(suffix);
    PathBuf::from(os)
}

#[cfg(unix)]
fn apply_umask(mode: u32) -> u32 {
    unsafe {
        let um = libc::umask(0);
        libc::umask(um);
        mode & !(um as u32)
    }
}

#[cfg(not(unix))]
fn apply_umask(mode: u32) -> u32 {
    mode
}

#[cfg(unix)]
fn chmod_file(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777));
}

#[cfg(not(unix))]
fn chmod_file(_path: &Path, _mode: u32) {}

/// Format a raw key byte slice using CPython's `repr(key.decode('Latin-1'))`
/// encoded back to Latin-1 bytes, matching `dbm.dumb`'s `.dir` line format
/// byte-for-byte.
fn format_latin1_repr_bytes(key: &[u8]) -> Vec<u8> {
    let has_single = key.contains(&b'\'');
    let has_double = key.contains(&b'"');
    let quote = if has_single && !has_double {
        b'"'
    } else {
        b'\''
    };

    let mut out = Vec::with_capacity(key.len() + 2);
    out.push(quote);
    for &b in key {
        match b {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            c if c == quote => {
                out.push(b'\\');
                out.push(quote);
            }
            0x20..=0x7E | 0xA1..=0xAC | 0xAE..=0xFF => out.push(b),
            _ => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                out.push(b'\\');
                out.push(b'x');
                out.push(HEX[(b >> 4) as usize]);
                out.push(HEX[(b & 0x0F) as usize]);
            }
        }
    }
    out.push(quote);
    out
}

fn format_dir_entry(key: &[u8], pos: u64, siz: u64) -> Vec<u8> {
    #[cfg(windows)]
    const EOL: &str = "\r\n";
    #[cfg(not(windows))]
    const EOL: &str = "\n";
    let mut line = format_latin1_repr_bytes(key);
    line.extend_from_slice(format!(", ({pos}, {siz}){EOL}").as_bytes());
    line
}

fn hex_val(b: u8) -> Option<u32> {
    match b {
        b'0'..=b'9' => Some((b - b'0') as u32),
        b'a'..=b'f' => Some((b - b'a' + 10) as u32),
        b'A'..=b'F' => Some((b - b'A' + 10) as u32),
        _ => None,
    }
}

/// Parse one non-empty `.dir` line of the form `"'key', (pos, siz)"` (Latin-1
/// encoded) into `(key_bytes, (pos, siz))`.
fn parse_dir_line(line: &[u8]) -> Result<(Vec<u8>, (u64, u64))> {
    let mut i = 0usize;
    while i < line.len() && line[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= line.len() || (line[i] != b'\'' && line[i] != b'"') {
        return Err(PyError::named(
            "SyntaxError",
            "invalid syntax in dbm.dumb .dir file",
        ));
    }
    let quote = line[i];
    i += 1;
    let mut key = Vec::new();
    let mut closed = false;
    while i < line.len() {
        let b = line[i];
        if b == quote {
            i += 1;
            closed = true;
            break;
        }
        if b == b'\\' {
            i += 1;
            if i >= line.len() {
                return Err(PyError::named(
                    "SyntaxError",
                    "unterminated string literal in dbm.dumb .dir file",
                ));
            }
            let esc = line[i];
            i += 1;
            match esc {
                b'\\' => key.push(b'\\'),
                b'\'' => key.push(b'\''),
                b'"' => key.push(b'"'),
                b'a' => key.push(0x07),
                b'b' => key.push(0x08),
                b'f' => key.push(0x0C),
                b'n' => key.push(b'\n'),
                b'r' => key.push(b'\r'),
                b't' => key.push(b'\t'),
                b'v' => key.push(0x0B),
                b'x' => {
                    if i + 2 > line.len() {
                        return Err(PyError::named("SyntaxError", "invalid \\x escape"));
                    }
                    let hi = hex_val(line[i])
                        .ok_or_else(|| PyError::named("SyntaxError", "invalid \\x escape"))?;
                    let lo = hex_val(line[i + 1])
                        .ok_or_else(|| PyError::named("SyntaxError", "invalid \\x escape"))?;
                    i += 2;
                    key.push(((hi << 4) | lo) as u8);
                }
                b'u' | b'U' => {
                    let count = if esc == b'u' { 4 } else { 8 };
                    if i + count > line.len() {
                        return Err(PyError::named("SyntaxError", "invalid unicode escape"));
                    }
                    let mut val = 0u32;
                    for &d in &line[i..i + count] {
                        let hv = hex_val(d)
                            .ok_or_else(|| PyError::named("SyntaxError", "invalid hex digit"))?;
                        val = (val << 4) | hv;
                    }
                    i += count;
                    if val > 0xFF {
                        return Err(PyError::named(
                            "UnicodeEncodeError",
                            "key in dbm.dumb .dir is not Latin-1",
                        ));
                    }
                    key.push(val as u8);
                }
                b'0'..=b'7' => {
                    let mut val = (esc - b'0') as u32;
                    for _ in 0..2 {
                        if i < line.len() && (b'0'..=b'7').contains(&line[i]) {
                            val = (val << 3) | ((line[i] - b'0') as u32);
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    key.push((val & 0xFF) as u8);
                }
                other => {
                    key.push(b'\\');
                    key.push(other);
                }
            }
        } else {
            key.push(b);
            i += 1;
        }
    }
    if !closed {
        return Err(PyError::named(
            "SyntaxError",
            "unterminated string literal in dbm.dumb .dir file",
        ));
    }

    let rest = std::str::from_utf8(&line[i..])
        .map_err(|_| PyError::named("SyntaxError", "invalid trailing bytes in .dir file"))?
        .trim();
    let rest = rest
        .strip_prefix(',')
        .ok_or_else(|| PyError::named("SyntaxError", "expected ',' in dbm.dumb .dir file"))?
        .trim();
    let inner = rest
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| PyError::named("SyntaxError", "expected tuple in dbm.dumb .dir file"))?;
    let mut parts = inner.split(',');
    let pos_str = parts
        .next()
        .ok_or_else(|| PyError::named("ValueError", "missing pos in dbm.dumb .dir file"))?
        .trim();
    let siz_str = parts
        .next()
        .ok_or_else(|| PyError::named("ValueError", "missing siz in dbm.dumb .dir file"))?
        .trim();
    if parts.next().is_some() {
        return Err(PyError::named(
            "ValueError",
            "too many elements in dbm.dumb .dir tuple",
        ));
    }
    let pos: u64 = pos_str
        .parse()
        .map_err(|_| PyError::named("ValueError", "invalid pos in dbm.dumb .dir file"))?;
    let siz: u64 = siz_str
        .parse()
        .map_err(|_| PyError::named("ValueError", "invalid siz in dbm.dumb .dir file"))?;
    Ok((key, (pos, siz)))
}

fn coerce_key_for_write(v: &Value) -> Result<Vec<u8>> {
    if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(v) {
        return Ok(snap);
    }
    match v.kind() {
        ValueKind::Str(s) => Ok(s.as_bytes().to_vec()),
        ValueKind::Bytes(b) => Ok(b.as_slice().to_vec()),
        _ => Err(PyError::named("TypeError", "keys must be bytes or strings")),
    }
}

fn coerce_val_for_write(v: &Value) -> Result<Vec<u8>> {
    if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(v) {
        return Ok(snap);
    }
    match v.kind() {
        ValueKind::Str(s) => Ok(s.as_bytes().to_vec()),
        ValueKind::Bytes(b) => Ok(b.as_slice().to_vec()),
        _ => Err(PyError::named(
            "TypeError",
            "values must be bytes or strings",
        )),
    }
}

enum LookupKey {
    Bytes(Vec<u8>),
    Unhashable(&'static str),
    Missing,
}

fn coerce_key_for_lookup(v: &Value) -> LookupKey {
    if pyrust_builtins::bytearray::as_bytearray_snapshot(v).is_some() {
        return LookupKey::Unhashable("bytearray");
    }
    match v.kind() {
        ValueKind::Str(s) => LookupKey::Bytes(s.as_bytes().to_vec()),
        ValueKind::Bytes(b) => LookupKey::Bytes(b.as_slice().to_vec()),
        ValueKind::List(_) => LookupKey::Unhashable("list"),
        ValueKind::Dict(_) => LookupKey::Unhashable("dict"),
        ValueKind::Set(_) => LookupKey::Unhashable("set"),
        _ => LookupKey::Missing,
    }
}

struct DumbDbState {
    dirfile: PathBuf,
    datfile: PathBuf,
    bakfile: PathBuf,
    mode: u32,
    readonly: bool,
    modified: bool,
    closed: bool,
    index: IndexMap<Vec<u8>, (u64, u64)>,
}

impl DumbDbState {
    fn open(base: &Path, mode: u32, flag: &str) -> Result<Self> {
        let dirfile = append_suffix(base, ".dir");
        let datfile = append_suffix(base, ".dat");
        let bakfile = append_suffix(base, ".bak");
        let readonly = flag == "r";
        let can_create = matches!(flag, "c" | "n");

        if flag == "n" {
            for p in [&datfile, &bakfile, &dirfile] {
                let _ = fs::remove_file(p);
            }
        }

        // _create: ensure .dat exists or create if flag in ('c', 'n')
        match File::open(&datfile) {
            Ok(_) => {}
            Err(e) => {
                if !can_create {
                    return Err(PyError::from_io_error(&e, Some(&datfile.to_string_lossy())));
                }
                File::create(&datfile)
                    .map_err(|ce| PyError::from_io_error(&ce, Some(&datfile.to_string_lossy())))?;
                chmod_file(&datfile, mode);
            }
        }

        // _update: read .dir into index or create empty .dir if flag in ('c', 'n')
        let mut index = IndexMap::new();
        match fs::read(&dirfile) {
            Ok(bytes) => {
                for raw_line in bytes.split(|&b| b == b'\n') {
                    let trimmed = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
                    if trimmed.iter().all(|b| b.is_ascii_whitespace()) {
                        continue;
                    }
                    let (key, pair) = parse_dir_line(trimmed)?;
                    index.insert(key, pair);
                }
            }
            Err(e) => {
                if !can_create {
                    return Err(PyError::from_io_error(&e, Some(&dirfile.to_string_lossy())));
                }
                File::create(&dirfile)
                    .map_err(|ce| PyError::from_io_error(&ce, Some(&dirfile.to_string_lossy())))?;
                chmod_file(&dirfile, mode);
            }
        }

        Ok(Self {
            dirfile,
            datfile,
            bakfile,
            mode,
            readonly,
            modified: false,
            closed: false,
            index,
        })
    }

    fn verify_open(&self) -> Result<()> {
        if self.closed {
            return Err(raise_dbm_os_error("DBM object has already been closed"));
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<()> {
        if self.closed || !self.modified {
            return Ok(());
        }
        let _ = fs::remove_file(&self.bakfile);
        let _ = fs::rename(&self.dirfile, &self.bakfile);

        let mut f = File::create(&self.dirfile)
            .map_err(|e| PyError::from_io_error(&e, Some(&self.dirfile.to_string_lossy())))?;
        chmod_file(&self.dirfile, self.mode);
        for (key, &(pos, siz)) in &self.index {
            let line = format_dir_entry(key, pos, siz);
            f.write_all(&line)
                .map_err(|e| PyError::from_io_error(&e, Some(&self.dirfile.to_string_lossy())))?;
        }
        f.flush()
            .map_err(|e| PyError::from_io_error(&e, Some(&self.dirfile.to_string_lossy())))?;
        self.modified = false;
        Ok(())
    }

    fn addval(&self, val: &[u8]) -> Result<(u64, u64)> {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.datfile)
            .map_err(|e| PyError::from_io_error(&e, Some(&self.datfile.to_string_lossy())))?;
        let pos = f
            .seek(SeekFrom::End(0))
            .map_err(|e| PyError::from_io_error(&e, Some(&self.datfile.to_string_lossy())))?;
        let npos = pos.div_ceil(BLOCKSIZE) * BLOCKSIZE;
        if npos > pos {
            let pad = vec![0u8; (npos - pos) as usize];
            f.write_all(&pad)
                .map_err(|e| PyError::from_io_error(&e, Some(&self.datfile.to_string_lossy())))?;
        }
        f.write_all(val)
            .map_err(|e| PyError::from_io_error(&e, Some(&self.datfile.to_string_lossy())))?;
        Ok((npos, val.len() as u64))
    }

    fn setval(&self, pos: u64, val: &[u8]) -> Result<(u64, u64)> {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.datfile)
            .map_err(|e| PyError::from_io_error(&e, Some(&self.datfile.to_string_lossy())))?;
        f.seek(SeekFrom::Start(pos))
            .map_err(|e| PyError::from_io_error(&e, Some(&self.datfile.to_string_lossy())))?;
        f.write_all(val)
            .map_err(|e| PyError::from_io_error(&e, Some(&self.datfile.to_string_lossy())))?;
        Ok((pos, val.len() as u64))
    }

    fn read_val(&self, pos: u64, siz: u64) -> Result<Vec<u8>> {
        let mut f = File::open(&self.datfile)
            .map_err(|e| PyError::from_io_error(&e, Some(&self.datfile.to_string_lossy())))?;
        f.seek(SeekFrom::Start(pos))
            .map_err(|e| PyError::from_io_error(&e, Some(&self.datfile.to_string_lossy())))?;
        let mut buf = vec![0u8; siz as usize];
        let mut read_total = 0usize;
        while read_total < buf.len() {
            let n = f
                .read(&mut buf[read_total..])
                .map_err(|e| PyError::from_io_error(&e, Some(&self.datfile.to_string_lossy())))?;
            if n == 0 {
                buf.truncate(read_total);
                break;
            }
            read_total += n;
        }
        Ok(buf)
    }

    fn addkey(&mut self, key: Vec<u8>, pair: (u64, u64)) -> Result<()> {
        let line = format_dir_entry(&key, pair.0, pair.1);
        self.index.insert(key, pair);
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.dirfile)
            .map_err(|e| PyError::from_io_error(&e, Some(&self.dirfile.to_string_lossy())))?;
        chmod_file(&self.dirfile, self.mode);
        f.write_all(&line)
            .map_err(|e| PyError::from_io_error(&e, Some(&self.dirfile.to_string_lossy())))?;
        Ok(())
    }

    fn close_internal(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        let res = self.commit();
        self.closed = true;
        self.index.clear();
        res
    }
}

impl Drop for DumbDbState {
    fn drop(&mut self) {
        let _ = self.close_internal();
    }
}

/// Wire `_Database` to inherit from `collections.abc.MutableMapping` on module
/// load so `isinstance(db, collections.abc.MutableMapping)` is `True` and all
/// `MutableMapping` mixin methods (`pop`, `popitem`, `clear`, `update`,
/// `values`, etc.) work seamlessly.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };
    if let Ok(abc_mod) = interp.load_module("collections.abc")
        && let Ok(mm_val) = interp.get_attr(&abc_mod, "MutableMapping")
        && let ValueKind::PyClass(mm_cls) = mm_val.kind()
        && let Some(db_val) = mod_rc.borrow().attrs.get("_Database").cloned()
        && let ValueKind::PyClass(db_cls) = db_val.kind()
    {
        crate::builtin_modules::macros::link_class_bases(db_cls, vec![Rc::clone(mm_cls)]);
    }
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![Value::string("error"), Value::string("open")]),
        "_BLOCKSIZE" => Value::int(BLOCKSIZE as i64),
        "error" => Value::py_class(
            lookup_exc_class("OSError").expect("OSError must exist"),
        ),
    }

    #[native_state(DumbDbState)]
    class _Database {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            filebasename: PyPath,
            mode: PyIndex,
            #[default("c".into())] flag: PyStr<'_>,
        ) -> Result<Value> {
            let mode_u32 = mode.expect_i64(FN_NAME, "mode")? as u32;
            let state = DumbDbState::open(filebasename.as_path(), mode_u32, &flag)?;
            set_state(self_val.0, state)?;
            Ok(Value::none())
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
        ) -> Result<Value> {
            let key_bytes = match coerce_key_for_lookup(&key.0) {
                LookupKey::Bytes(b) => b,
                LookupKey::Unhashable(ty) => {
                    return Err(PyError::named("TypeError", format!("unhashable type: '{ty}'")));
                }
                LookupKey::Missing => {
                    with_state(self_val.0, |st| st.verify_open())?;
                    return Err(raise_key_error(_interp, key.0));
                }
            };
            let lookup_val = Value::bytes(key_bytes.clone());
            let pair = with_state(self_val.0, |st| {
                st.verify_open()?;
                Ok(st.index.get(&key_bytes).copied())
            })?;
            let Some((pos, siz)) = pair else {
                return Err(raise_key_error(_interp, lookup_val));
            };
            let data = with_state(self_val.0, |st| st.read_val(pos, siz))?;
            Ok(Value::bytes(data))
        }

        fn __setitem__(
            #[state_mut] st: &mut DumbDbState,
            key: PyValue,
            val: PyValue,
        ) -> Result<Value> {
            if st.readonly {
                return Err(raise_dbm_os_error(
                    "The database is opened for reading only",
                ));
            }
            let key_bytes = coerce_key_for_write(&key.0)?;
            let val_bytes = coerce_val_for_write(&val.0)?;
            st.verify_open()?;
            st.modified = true;
            if let Some(&(pos, siz)) = st.index.get(&key_bytes) {
                let oldblocks = siz.div_ceil(BLOCKSIZE);
                let newblocks = (val_bytes.len() as u64).div_ceil(BLOCKSIZE);
                let new_pair = if newblocks <= oldblocks {
                    st.setval(pos, &val_bytes)?
                } else {
                    st.addval(&val_bytes)?
                };
                st.index.insert(key_bytes, new_pair);
            } else {
                let pair = st.addval(&val_bytes)?;
                st.addkey(key_bytes, pair)?;
            }
            Ok(Value::none())
        }

        fn __delitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
        ) -> Result<Value> {
            let key_bytes = match coerce_key_for_lookup(&key.0) {
                LookupKey::Bytes(b) => b,
                LookupKey::Unhashable(ty) => {
                    return Err(PyError::named("TypeError", format!("unhashable type: '{ty}'")));
                }
                LookupKey::Missing => {
                    with_state(self_val.0, |st| {
                        if st.readonly {
                            return Err(raise_dbm_os_error(
                                "The database is opened for reading only",
                            ));
                        }
                        st.verify_open()
                    })?;
                    return Err(raise_key_error(_interp, key.0));
                }
            };
            let lookup_val = Value::bytes(key_bytes.clone());
            let removed = with_state_mut(self_val.0, |st| {
                if st.readonly {
                    return Err(raise_dbm_os_error(
                        "The database is opened for reading only",
                    ));
                }
                st.verify_open()?;
                st.modified = true;
                if st.index.shift_remove(&key_bytes).is_none() {
                    return Ok(false);
                }
                st.commit()?;
                Ok(true)
            })?;
            if !removed {
                return Err(raise_key_error(_interp, lookup_val));
            }
            Ok(Value::none())
        }

        fn __contains__(
            #[state] st: &DumbDbState,
            key: PyValue,
        ) -> Result<Value> {
            st.verify_open()?;
            match coerce_key_for_lookup(&key.0) {
                LookupKey::Bytes(b) => Ok(Value::bool_(st.index.contains_key(&b))),
                LookupKey::Unhashable(ty) => {
                    Err(PyError::named("TypeError", format!("unhashable type: '{ty}'")))
                }
                LookupKey::Missing => Ok(Value::bool_(false)),
            }
        }

        fn __len__(#[state] st: &DumbDbState) -> Result<Value> {
            st.verify_open()?;
            Ok(Value::int(st.index.len() as i64))
        }

        fn iterkeys(#[state] st: &DumbDbState) -> Result<Value> {
            st.verify_open()?;
            let keys: Vec<Value> = st
                .index
                .keys()
                .map(|k| Value::bytes(k.clone()))
                .collect();
            Ok(Value::generator(Box::new(NativeIterFrame::generator(keys))))
        }

        fn __iter__(#[state] st: &DumbDbState) -> Result<Value> {
            st.verify_open()?;
            let keys: Vec<Value> = st
                .index
                .keys()
                .map(|k| Value::bytes(k.clone()))
                .collect();
            Ok(Value::generator(Box::new(NativeIterFrame::generator(keys))))
        }

        fn keys(#[state] st: &DumbDbState) -> Result<Value> {
            st.verify_open()?;
            let keys: Vec<Value> = st
                .index
                .keys()
                .map(|k| Value::bytes(k.clone()))
                .collect();
            Ok(Value::list(keys))
        }

        fn items(#[state] st: &DumbDbState) -> Result<Value> {
            st.verify_open()?;
            let mut out = Vec::with_capacity(st.index.len());
            for (k, &(pos, siz)) in &st.index {
                let data = st.read_val(pos, siz)?;
                out.push(Value::tuple(vec![
                    Value::bytes(k.clone()),
                    Value::bytes(data),
                ]));
            }
            Ok(Value::list(out))
        }

        fn get(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
            #[default(OptionalArg::Omitted)] default: OptValueRef<'_>,
        ) -> Result<Value> {
            let def = default.as_value().cloned().unwrap_or_else(Value::none);
            let key_bytes = match coerce_key_for_lookup(&key.0) {
                LookupKey::Bytes(b) => b,
                LookupKey::Unhashable(ty) => {
                    return Err(PyError::named("TypeError", format!("unhashable type: '{ty}'")));
                }
                LookupKey::Missing => {
                    with_state(self_val.0, |st| st.verify_open())?;
                    return Ok(def);
                }
            };
            with_state(self_val.0, |st| {
                st.verify_open()?;
                if let Some(&(pos, siz)) = st.index.get(&key_bytes) {
                    Ok(Value::bytes(st.read_val(pos, siz)?))
                } else {
                    Ok(def)
                }
            })
        }

        fn setdefault(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
            #[default(OptionalArg::Omitted)] default: OptValueRef<'_>,
        ) -> Result<Value> {
            let def = default.as_value().cloned().unwrap_or_else(Value::none);
            let existing = match coerce_key_for_lookup(&key.0) {
                LookupKey::Bytes(b) => with_state(self_val.0, |st| {
                    st.verify_open()?;
                    if let Some(&(pos, siz)) = st.index.get(&b) {
                        Ok(Some(Value::bytes(st.read_val(pos, siz)?)))
                    } else {
                        Ok(None)
                    }
                })?,
                LookupKey::Unhashable(ty) => {
                    return Err(PyError::named("TypeError", format!("unhashable type: '{ty}'")));
                }
                LookupKey::Missing => None,
            };
            if let Some(val) = existing {
                return Ok(val);
            }
            let setitem = _interp.get_attr(self_val.0, "__setitem__")?;
            call_pos(_interp, setitem, &[key.0.clone(), def.clone()])?;
            let getitem = _interp.get_attr(self_val.0, "__getitem__")?;
            call_pos(_interp, getitem, &[key.0]).or(Ok(def))
        }

        fn sync(#[state_mut] st: &mut DumbDbState) -> Result<Value> {
            st.commit()?;
            Ok(Value::none())
        }

        fn _commit(#[state_mut] st: &mut DumbDbState) -> Result<Value> {
            st.commit()?;
            Ok(Value::none())
        }

        fn close(#[state_mut] st: &mut DumbDbState) -> Result<Value> {
            st.close_internal()?;
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            let close_fn = _interp.get_attr(self_val.0, "close")?;
            call_pos(_interp, close_fn, &[])?;
            Ok(Value::none())
        }
    }

    fn open(
        file: PyPath,
        #[default("c".into())] flag: PyStr<'_>,
        #[default(0o666i64.into())] mode: PyIndex,
    ) -> Result<Value> {
        if !matches!(&*flag, "r" | "w" | "c" | "n") {
            return Err(PyError::named(
                "ValueError",
                "Flag must be one of 'r', 'w', 'c', or 'n'",
            ));
        }
        let raw_mode = mode.expect_i64(FN_NAME, "mode")? as u32;
        let effective_mode = apply_umask(raw_mode);
        let dumb_mod = _interp.load_module("dbm.dumb")?;
        let db_cls = _interp.get_attr(&dumb_mod, "_Database")?;
        let path_val = if file.is_bytes() {
            Value::bytes(file.as_path().as_os_str().as_encoded_bytes().to_vec())
        } else {
            Value::string(file.as_path().to_string_lossy())
        };
        call_pos(
            _interp,
            db_cls,
            &[
                path_val,
                Value::int(effective_mode as i64),
                Value::string(&*flag),
            ],
        )
    }
}
