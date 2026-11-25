// `mailbox` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// Read/write support for `Maildir`, `mbox`, `MH`, `Babyl`, and `MMDF` mailboxes
// along with `Message`, `MaildirMessage`, `mboxMessage`, `MHMessage`,
// `BabylMessage`, `MMDFMessage`, and the `mailbox.Error` exception hierarchy.
//
// Reference: <https://docs.python.org/3/library/mailbox.html>

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::builtin_modules::macros::native_state::with_native_state_mut;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, OptValueRef, OptionalArg, PyBool, PyBytesLike, PyIndex, PyPath, PyStr, PyValue,
    PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

#[cfg(windows)]
const OS_LINESEP: &[u8] = b"\r\n";
#[cfg(not(windows))]
const OS_LINESEP: &[u8] = b"\n";

static MAILDIR_COUNTER: AtomicU64 = AtomicU64::new(1);
static TMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(1);

const BABYL_SPECIAL_LABELS: &[&str] = &[
    "unseen",
    "deleted",
    "filed",
    "answered",
    "forwarded",
    "edited",
    "resent",
];

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

fn value_as_bytes(v: &Value) -> Option<&[u8]> {
    match v.kind() {
        ValueKind::Bytes(b) => Some(b.as_slice()),
        _ => None,
    }
}

fn with_singlefile_mut<R>(
    self_val: &Value,
    f: impl FnOnce(&mut SingleFileState) -> Result<R>,
) -> Result<R> {
    with_native_state_mut::<SingleFileState, R>(self_val, "_singlefileMailbox", f)
}

fn raise_mb_error(interp: &mut Interpreter, exc_name: &'static str, message: &str) -> PyError {
    if let Ok(m) = interp.load_module("mailbox")
        && let Ok(cls) = interp.get_attr(&m, exc_name)
        && let Ok(inst) = call_pos(interp, cls, &[Value::string(message)])
    {
        return PyError::raised(inst);
    }
    PyError::named(exc_name, message.to_string())
}

fn is_instance_of_mod_cls(
    interp: &mut Interpreter,
    val: &Value,
    mod_name: &str,
    cls_name: &str,
) -> bool {
    let Ok(m) = interp.load_module(mod_name) else {
        return false;
    };
    let Ok(cls) = interp.get_attr(&m, cls_name) else {
        return false;
    };
    call_pos(
        interp,
        Value::builtin_function("isinstance"),
        &[val.clone(), cls],
    )
    .and_then(|v| interp.truthy_value(&v))
    .unwrap_or(false)
}

fn replace_bytes(data: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    if from.is_empty() || from == to {
        return data.to_vec();
    }
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

fn normalize_lf_to_os(data: &[u8]) -> Vec<u8> {
    if OS_LINESEP == b"\n" {
        data.to_vec()
    } else {
        replace_bytes(data, b"\n", OS_LINESEP)
    }
}

fn normalize_os_to_lf(data: &[u8]) -> Vec<u8> {
    if OS_LINESEP == b"\n" {
        data.to_vec()
    } else {
        replace_bytes(data, OS_LINESEP, b"\n")
    }
}

fn mangle_from_bytes(data: &[u8]) -> Vec<u8> {
    replace_bytes(data, b"\nFrom ", b"\n>From ")
}

fn now_secs_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn format_time_tuple_asctime(interp: &mut Interpreter, tt: &Value) -> Result<String> {
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let items = interp.collect_iterable(tt)?;
    if items.len() < 7 {
        return Err(PyError::named(
            "TypeError",
            "Tuple or struct_time argument required".to_string(),
        ));
    }
    let year = items[0].as_int().unwrap_or(1970);
    let mon = items[1].as_int().unwrap_or(1).clamp(1, 12) as usize;
    let mday = items[2].as_int().unwrap_or(1);
    let hour = items[3].as_int().unwrap_or(0);
    let min = items[4].as_int().unwrap_or(0);
    let sec = items[5].as_int().unwrap_or(0);
    let wday = items[6].as_int().unwrap_or(0).clamp(0, 6) as usize;
    Ok(format!(
        "{} {} {:2} {:02}:{:02}:{:02} {:04}",
        DAYS[wday],
        MONTHS[mon - 1],
        mday,
        hour,
        min,
        sec,
        year
    ))
}

fn gmtime_asctime(interp: &mut Interpreter) -> Result<String> {
    let time_mod = interp.load_module("time")?;
    let gmtime = interp.get_attr(&time_mod, "gmtime")?;
    let gm = call_pos(interp, gmtime, &[])?;
    format_time_tuple_asctime(interp, &gm)
}

fn resolve_mailbox_path(interp: &mut Interpreter, path: &Value) -> Result<String> {
    let p = PyPath::try_from_value_with_interp(interp, path, "Mailbox", "path")?;
    let os_path = interp.load_module("os.path")?;
    let expanduser = interp.get_attr(&os_path, "expanduser")?;
    let abspath = interp.get_attr(&os_path, "abspath")?;
    let raw_str = Value::string(p.as_path().to_string_lossy().as_ref());
    let exp = call_pos(interp, expanduser, &[raw_str])?;
    let abs = call_pos(interp, abspath, &[exp])?;
    Ok(abs.as_str().unwrap_or("").to_string())
}

fn serialize_message_to_bytes(
    interp: &mut Interpreter,
    message: &Value,
    mangle_from: bool,
    append_newline: bool,
) -> Result<Vec<u8>> {
    let mut data: Vec<u8> = if is_instance_of_mod_cls(interp, message, "email.message", "Message") {
        let io_mod = interp.load_module("io")?;
        let bio_cls = interp.get_attr(&io_mod, "BytesIO")?;
        let buf = call_pos(interp, bio_cls, &[])?;
        let gen_mod = interp.load_module("email.generator")?;
        let bg_cls = interp.get_attr(&gen_mod, "BytesGenerator")?;
        let generator = call_pos(
            interp,
            bg_cls,
            &[buf.clone(), Value::bool_(mangle_from), Value::int(0)],
        )?;
        let flat = interp.get_attr(&generator, "flatten")?;
        call_pos(interp, flat, std::slice::from_ref(message))?;
        let getval = interp.get_attr(&buf, "getvalue")?;
        let raw_v = call_pos(interp, getval, &[])?;
        let bl =
            PyBytesLike::try_from_value_with_interp(interp, &raw_v, "_dump_message", "message")?;
        normalize_lf_to_os(bl.as_bytes())
    } else if let Some(s) = message.as_str() {
        if !s.is_ascii() {
            return Err(PyError::named(
                "ValueError",
                "String input must be ASCII-only; use bytes or a Message instead".to_string(),
            ));
        }
        let mut raw = s.as_bytes().to_vec();
        if mangle_from {
            raw = mangle_from_bytes(&raw);
        }
        normalize_lf_to_os(&raw)
    } else if let Ok(bl) =
        PyBytesLike::try_from_value_with_interp(interp, message, "_dump_message", "message")
    {
        let mut raw = bl.as_bytes().to_vec();
        if mangle_from {
            raw = mangle_from_bytes(&raw);
        }
        normalize_lf_to_os(&raw)
    } else if is_instance_of_mod_cls(interp, message, "io", "StringIO") {
        let gv = interp.get_attr(message, "getvalue")?;
        let s_val = call_pos(interp, gv, &[])?;
        let s = s_val.as_str().unwrap_or("");
        if !s.is_ascii() {
            return Err(PyError::named(
                "ValueError",
                "String input must be ASCII-only; use bytes or a Message instead".to_string(),
            ));
        }
        let mut raw = s.as_bytes().to_vec();
        if mangle_from {
            raw = mangle_from_bytes(&raw);
        }
        normalize_lf_to_os(&raw)
    } else if interp.get_attr(message, "read").is_ok() {
        let reader = if let Ok(buf) = interp.get_attr(message, "buffer") {
            buf
        } else {
            message.clone()
        };
        let readline_fn = interp.get_attr(&reader, "readline")?;
        let mut out = Vec::new();
        loop {
            let line_v = call_pos(interp, readline_fn.clone(), &[])?;
            let bl =
                PyBytesLike::try_from_value_with_interp(interp, &line_v, "_dump_message", "line")?;
            let raw_line = bl.as_bytes();
            if raw_line.is_empty() {
                break;
            }
            let mut line = if let Some(stripped) = raw_line.strip_suffix(b"\r\n") {
                let mut v = stripped.to_vec();
                v.push(b'\n');
                v
            } else if let Some(stripped) = raw_line.strip_suffix(b"\r") {
                let mut v = stripped.to_vec();
                v.push(b'\n');
                v
            } else {
                raw_line.to_vec()
            };
            if mangle_from && line.starts_with(b"From ") {
                let mut mangled = b">From ".to_vec();
                mangled.extend_from_slice(&line[5..]);
                line = mangled;
            }
            out.extend_from_slice(&normalize_lf_to_os(&line));
        }
        out
    } else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "Invalid message type: {}",
                crate::interpreter::value_type_name_str(message)
            ),
        ));
    };

    if append_newline && !data.is_empty() && !data.ends_with(OS_LINESEP) {
        data.extend_from_slice(OS_LINESEP);
    }
    Ok(data)
}

fn email_from_bytes(interp: &mut Interpreter, data: &[u8]) -> Result<Value> {
    let email_mod = interp.load_module("email")?;
    let mfb = interp.get_attr(&email_mod, "message_from_bytes")?;
    call_pos(interp, mfb, &[Value::bytes(data.to_vec())])
}

fn email_from_string(interp: &mut Interpreter, s: &str) -> Result<Value> {
    let email_mod = interp.load_module("email")?;
    let mfs = interp.get_attr(&email_mod, "message_from_string")?;
    call_pos(interp, mfs, &[Value::string(s)])
}

fn make_bytes_io(interp: &mut Interpreter, data: Vec<u8>) -> Result<Value> {
    let io_mod = interp.load_module("io")?;
    let bio_cls = interp.get_attr(&io_mod, "BytesIO")?;
    call_pos(interp, bio_cls, &[Value::bytes(data)])
}

fn create_dotlock(interp: &mut Interpreter, base_path: &Path) -> Result<()> {
    let lock_path = PathBuf::from(format!("{}.lock", base_path.display()));
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(raise_mb_error(
            interp,
            "ExternalClashError",
            &format!("dot lock unavailable: {}", base_path.display()),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Ok(()),
        Err(e) => Err(PyError::from_io_error(
            &e,
            Some(&lock_path.to_string_lossy()),
        )),
    }
}

fn remove_dotlock(base_path: &Path) {
    let lock_path = PathBuf::from(format!("{}.lock", base_path.display()));
    let _ = fs::remove_file(lock_path);
}

fn make_temp_sibling_path(base_path: &Path) -> PathBuf {
    let seq = TMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let ts = now_secs_f64() as u64;
    PathBuf::from(format!("{}.{}.{}.{}", base_path.display(), ts, pid, seq))
}

fn split_lines_with_offsets(data: &[u8]) -> Vec<(usize, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let start = i;
        while i < data.len() && data[i] != b'\n' {
            i += 1;
        }
        if i < data.len() {
            i += 1;
        }
        out.push((start, &data[start..i]));
    }
    out
}

fn scan_mbox_toc(data: &[u8]) -> BTreeMap<i64, (usize, usize)> {
    let mut starts: Vec<usize> = Vec::new();
    let mut stops: Vec<usize> = Vec::new();
    let mut last_was_empty = false;
    for (pos, line) in split_lines_with_offsets(data) {
        if line.starts_with(b"From ") {
            if stops.len() < starts.len() {
                if last_was_empty {
                    stops.push(pos.saturating_sub(OS_LINESEP.len()));
                } else {
                    stops.push(pos);
                }
            }
            starts.push(pos);
            last_was_empty = false;
        } else {
            last_was_empty = line == OS_LINESEP || line == b"\n" || line == b"\r\n";
        }
    }
    if stops.len() < starts.len() {
        if last_was_empty {
            stops.push(data.len().saturating_sub(OS_LINESEP.len()));
        } else {
            stops.push(data.len());
        }
    }
    let mut toc = BTreeMap::new();
    for (idx, (s, e)) in starts.into_iter().zip(stops).enumerate() {
        toc.insert(idx as i64, (s, e));
    }
    toc
}

fn scan_mmdf_toc(data: &[u8]) -> BTreeMap<i64, (usize, usize)> {
    let mut starts: Vec<usize> = Vec::new();
    let mut stops: Vec<usize> = Vec::new();
    let lines = split_lines_with_offsets(data);
    let mut i = 0;
    while i < lines.len() {
        let (pos, line) = lines[i];
        let next_pos = pos + line.len();
        if line.starts_with(b"\x01\x01\x01\x01")
            && (line == b"\x01\x01\x01\x01\n" || line == b"\x01\x01\x01\x01\r\n")
        {
            starts.push(next_pos);
            i += 1;
            let mut found_end = false;
            while i < lines.len() {
                let (epos, eline) = lines[i];
                if eline == b"\x01\x01\x01\x01\n" || eline == b"\x01\x01\x01\x01\r\n" {
                    let sep_len = if epos >= 2 && &data[epos - 2..epos] == b"\r\n" {
                        2
                    } else if epos >= 1 && data[epos - 1] == b'\n' {
                        1
                    } else {
                        0
                    };
                    stops.push(epos - sep_len);
                    found_end = true;
                    i += 1;
                    break;
                }
                i += 1;
            }
            if !found_end {
                stops.push(data.len());
            }
        } else {
            i += 1;
        }
    }
    let mut toc = BTreeMap::new();
    for (idx, (s, e)) in starts.into_iter().zip(stops).enumerate() {
        toc.insert(idx as i64, (s, e));
    }
    toc
}

type TocMap = BTreeMap<i64, (usize, usize)>;
type LabelsMap = BTreeMap<i64, Vec<String>>;

fn scan_babyl_toc(data: &[u8]) -> (TocMap, LabelsMap) {
    let mut starts: Vec<usize> = Vec::new();
    let mut stops: Vec<usize> = Vec::new();
    let mut label_lists: Vec<Vec<String>> = Vec::new();
    let lines = split_lines_with_offsets(data);
    let mut i = 0;
    while i < lines.len() {
        let (pos, line) = lines[i];
        let next_pos = pos + line.len();
        let is_msg_start = line == b"\x1f\x0c\n" || line == b"\x1f\x0c\r\n";
        let is_delim = line == b"\x1f" || line == b"\x1f\n" || line == b"\x1f\r\n";
        if is_msg_start {
            if stops.len() < starts.len() {
                let sep_len = if pos >= 2 && &data[pos - 2..pos] == b"\r\n" {
                    2
                } else if pos >= 1 && data[pos - 1] == b'\n' {
                    1
                } else {
                    0
                };
                stops.push(pos - sep_len);
            }
            starts.push(next_pos);
            let labels = if i + 1 < lines.len() {
                let lline = lines[i + 1].1;
                i += 1;
                let rest = if !lline.is_empty() { &lline[1..] } else { b"" };
                rest.split(|&b| b == b',')
                    .filter_map(|part| {
                        let s = String::from_utf8_lossy(part).trim().to_string();
                        if s.is_empty() { None } else { Some(s) }
                    })
                    .collect()
            } else {
                Vec::new()
            };
            label_lists.push(labels);
        } else if is_delim && stops.len() < starts.len() {
            let sep_len = if pos >= 2 && &data[pos - 2..pos] == b"\r\n" {
                2
            } else if pos >= 1 && data[pos - 1] == b'\n' {
                1
            } else {
                0
            };
            stops.push(pos - sep_len);
        }
        i += 1;
    }
    if stops.len() < starts.len() {
        stops.push(data.len());
    }
    let mut toc = BTreeMap::new();
    let mut labels_map = BTreeMap::new();
    for (idx, (s, e)) in starts.into_iter().zip(stops).enumerate() {
        toc.insert(idx as i64, (s, e));
        if let Some(ll) = label_lists.get(idx) {
            labels_map.insert(idx as i64, ll.clone());
        }
    }
    (toc, labels_map)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SingleFileKind {
    Mbox,
    Mmdf,
    Babyl,
}

struct SingleFileState {
    kind: SingleFileKind,
    path: PathBuf,
    toc: Option<BTreeMap<i64, (usize, usize)>>,
    labels: BTreeMap<i64, Vec<String>>,
    next_key: i64,
    pending: bool,
    pending_sync: bool,
    locked: bool,
    closed: bool,
    file_length: Option<u64>,
}

impl SingleFileState {
    fn ensure_toc(&mut self) -> Result<()> {
        if self.toc.is_some() {
            return Ok(());
        }
        let data = fs::read(&self.path)
            .map_err(|e| PyError::from_io_error(&e, Some(&self.path.to_string_lossy())))?;
        self.file_length = Some(data.len() as u64);
        match self.kind {
            SingleFileKind::Mbox => {
                let toc = scan_mbox_toc(&data);
                self.next_key = toc.len() as i64;
                self.toc = Some(toc);
            }
            SingleFileKind::Mmdf => {
                let toc = scan_mmdf_toc(&data);
                self.next_key = toc.len() as i64;
                self.toc = Some(toc);
            }
            SingleFileKind::Babyl => {
                let (toc, labels) = scan_babyl_toc(&data);
                self.next_key = toc.len() as i64;
                self.toc = Some(toc);
                self.labels = labels;
            }
        }
        Ok(())
    }

    fn lookup_key(&mut self, key: i64) -> Result<(usize, usize)> {
        self.ensure_toc()?;
        self.toc
            .as_ref()
            .and_then(|t| t.get(&key).copied())
            .ok_or_else(|| PyError::named("KeyError", format!("No message with key: {key}")))
    }

    fn read_slice(&self, start: usize, stop: usize) -> Result<Vec<u8>> {
        let mut f = fs::File::open(&self.path)
            .map_err(|e| PyError::from_io_error(&e, Some(&self.path.to_string_lossy())))?;
        f.seek(SeekFrom::Start(start as u64))
            .map_err(|e| PyError::from_io_error(&e, Some(&self.path.to_string_lossy())))?;
        let len = stop.saturating_sub(start);
        let mut buf = vec![0u8; len];
        f.read_exact(&mut buf)
            .map_err(|e| PyError::from_io_error(&e, Some(&self.path.to_string_lossy())))?;
        Ok(buf)
    }

    fn user_babyl_labels(&mut self) -> Result<Vec<String>> {
        self.ensure_toc()?;
        let mut out = BTreeSet::new();
        for ll in self.labels.values() {
            for l in ll {
                if !BABYL_SPECIAL_LABELS.contains(&l.as_str()) {
                    out.insert(l.clone());
                }
            }
        }
        Ok(out.into_iter().collect())
    }

    fn pre_mailbox_bytes(&mut self) -> Result<Vec<u8>> {
        if self.kind != SingleFileKind::Babyl {
            return Ok(Vec::new());
        }
        let labels = self.user_babyl_labels()?;
        let mut out = Vec::new();
        out.extend_from_slice(b"BABYL OPTIONS:");
        out.extend_from_slice(OS_LINESEP);
        out.extend_from_slice(b"Version: 5");
        out.extend_from_slice(OS_LINESEP);
        out.extend_from_slice(b"Labels:");
        out.extend_from_slice(labels.join(",").as_bytes());
        out.extend_from_slice(OS_LINESEP);
        out.push(0x1f);
        Ok(out)
    }

    fn pre_message_bytes(&self) -> Vec<u8> {
        match self.kind {
            SingleFileKind::Mbox => Vec::new(),
            SingleFileKind::Mmdf => {
                let mut v = b"\x01\x01\x01\x01".to_vec();
                v.extend_from_slice(OS_LINESEP);
                v
            }
            SingleFileKind::Babyl => {
                let mut v = vec![0x0c];
                v.extend_from_slice(OS_LINESEP);
                v
            }
        }
    }

    fn post_message_bytes(&self) -> Vec<u8> {
        match self.kind {
            SingleFileKind::Mbox => OS_LINESEP.to_vec(),
            SingleFileKind::Mmdf => {
                let mut v = OS_LINESEP.to_vec();
                v.extend_from_slice(b"\x01\x01\x01\x01");
                v.extend_from_slice(OS_LINESEP);
                v
            }
            SingleFileKind::Babyl => {
                let mut v = OS_LINESEP.to_vec();
                v.push(0x1f);
                v
            }
        }
    }

    fn flush_disk(&mut self, interp: &mut Interpreter) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        if !self.pending {
            if self.pending_sync {
                if let Ok(f) = OpenOptions::new().read(true).write(true).open(&self.path) {
                    let _ = f.sync_all();
                }
                self.pending_sync = false;
            }
            return Ok(());
        }
        let toc = self.toc.clone().unwrap_or_default();
        let cur_len = fs::metadata(&self.path)
            .map_err(|e| PyError::from_io_error(&e, Some(&self.path.to_string_lossy())))?
            .len();
        if let Some(expected) = self.file_length
            && cur_len != expected
        {
            return Err(raise_mb_error(
                interp,
                "ExternalClashError",
                &format!("Size of mailbox file changed (expected {expected}, found {cur_len})"),
            ));
        }
        let tmp_path = make_temp_sibling_path(&self.path);
        let pre_mb = self.pre_mailbox_bytes()?;
        let pre_msg = self.pre_message_bytes();
        let post_msg = self.post_message_bytes();
        let mut new_toc = BTreeMap::new();
        {
            let mut new_file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_path)
                .map_err(|e| PyError::from_io_error(&e, Some(&tmp_path.to_string_lossy())))?;
            let write_res = (|| -> Result<u64> {
                new_file
                    .write_all(&pre_mb)
                    .map_err(|e| PyError::from_io_error(&e, Some(&tmp_path.to_string_lossy())))?;
                for (key, (start, stop)) in toc {
                    new_file.write_all(&pre_msg).map_err(|e| {
                        PyError::from_io_error(&e, Some(&tmp_path.to_string_lossy()))
                    })?;
                    let new_start = new_file.stream_position().map_err(|e| {
                        PyError::from_io_error(&e, Some(&tmp_path.to_string_lossy()))
                    })? as usize;
                    let chunk = self.read_slice(start, stop)?;
                    new_file.write_all(&chunk).map_err(|e| {
                        PyError::from_io_error(&e, Some(&tmp_path.to_string_lossy()))
                    })?;
                    let new_stop = new_file.stream_position().map_err(|e| {
                        PyError::from_io_error(&e, Some(&tmp_path.to_string_lossy()))
                    })? as usize;
                    new_toc.insert(key, (new_start, new_stop));
                    new_file.write_all(&post_msg).map_err(|e| {
                        PyError::from_io_error(&e, Some(&tmp_path.to_string_lossy()))
                    })?;
                }
                let end_len = new_file
                    .stream_position()
                    .map_err(|e| PyError::from_io_error(&e, Some(&tmp_path.to_string_lossy())))?;
                new_file
                    .sync_all()
                    .map_err(|e| PyError::from_io_error(&e, Some(&tmp_path.to_string_lossy())))?;
                Ok(end_len)
            })();
            match write_res {
                Ok(end_len) => {
                    self.file_length = Some(end_len);
                }
                Err(err) => {
                    drop(new_file);
                    let _ = fs::remove_file(&tmp_path);
                    return Err(err);
                }
            }
        }
        // Replace target file (Windows requires removing target if rename hits AlreadyExists).
        if let Err(_e) = fs::rename(&tmp_path, &self.path) {
            let _ = fs::remove_file(&self.path);
            fs::rename(&tmp_path, &self.path)
                .map_err(|e| PyError::from_io_error(&e, Some(&self.path.to_string_lossy())))?;
        }
        self.toc = Some(new_toc);
        self.pending = false;
        self.pending_sync = false;
        Ok(())
    }
}

fn format_mbox_mmdf_record(
    interp: &mut Interpreter,
    message: &Value,
    mangle_from: bool,
    append_newline: bool,
) -> Result<Vec<u8>> {
    let mut from_line: Option<Vec<u8>> = None;
    let mut body_msg = message.clone();

    if let Some(s) = message.as_str() {
        if !s.is_ascii() {
            return Err(PyError::named(
                "ValueError",
                "String input must be ASCII-only; use bytes or a Message instead".to_string(),
            ));
        }
        body_msg = Value::bytes(s.as_bytes().to_vec());
    }

    if let Some(raw) = value_as_bytes(&body_msg)
        && raw.starts_with(b"From ")
    {
        if let Some(nl) = raw.iter().position(|&b| b == b'\n') {
            from_line = Some(raw[..nl].strip_suffix(b"\r").unwrap_or(&raw[..nl]).to_vec());
            body_msg = Value::bytes(raw[nl + 1..].to_vec());
        } else {
            from_line = Some(raw.to_vec());
            body_msg = Value::bytes(Vec::new());
        }
    } else if is_instance_of_mod_cls(interp, &body_msg, "mailbox", "_mboxMMDFMessage") {
        let gf = interp.get_attr(&body_msg, "get_from")?;
        let author = call_pos(interp, gf, &[])?;
        let mut fl = b"From ".to_vec();
        fl.extend_from_slice(author.as_str().unwrap_or("MAILER-DAEMON").as_bytes());
        from_line = Some(fl);
    } else if is_instance_of_mod_cls(interp, &body_msg, "email.message", "Message") {
        let guf = interp.get_attr(&body_msg, "get_unixfrom")?;
        let uf = call_pos(interp, guf, &[])?;
        if let Some(s) = uf.as_str() {
            from_line = Some(s.as_bytes().to_vec());
        }
    }

    let fl = match from_line {
        Some(v) => v,
        None => {
            let ts = gmtime_asctime(interp)?;
            format!("From MAILER-DAEMON {ts}").into_bytes()
        }
    };

    let mut record = fl;
    record.extend_from_slice(OS_LINESEP);
    let dumped = serialize_message_to_bytes(interp, &body_msg, mangle_from, append_newline)?;
    record.extend_from_slice(&dumped);
    Ok(record)
}

fn format_babyl_record(
    interp: &mut Interpreter,
    message: &Value,
) -> Result<(Vec<u8>, Option<Vec<String>>)> {
    let mut record = Vec::new();
    let mut msg_labels: Option<Vec<String>> = None;
    if is_instance_of_mod_cls(interp, message, "mailbox", "BabylMessage") {
        let gl = interp.get_attr(message, "get_labels")?;
        let lval = call_pos(interp, gl, &[])?;
        let items = interp.collect_iterable(&lval)?;
        let mut special = Vec::new();
        let mut user = Vec::new();
        let mut all_l = Vec::new();
        for item in items {
            if let Some(s) = item.as_str() {
                all_l.push(s.to_string());
                if BABYL_SPECIAL_LABELS.contains(&s) {
                    special.push(s.to_string());
                } else {
                    user.push(s.to_string());
                }
            }
        }
        record.push(b'1');
        for s in special {
            record.extend_from_slice(b", ");
            record.extend_from_slice(s.as_bytes());
        }
        record.extend_from_slice(b",,");
        for u in user {
            record.push(b' ');
            record.extend_from_slice(u.as_bytes());
            record.push(b',');
        }
        record.extend_from_slice(OS_LINESEP);
        msg_labels = Some(all_l);
    } else {
        record.extend_from_slice(b"1,,");
        record.extend_from_slice(OS_LINESEP);
    }

    let raw = normalize_os_to_lf(&serialize_message_to_bytes(interp, message, false, false)?);
    // Split headers vs body at first blank line `\n\n`.
    if let Some(pos) = raw.windows(2).position(|w| w == b"\n\n") {
        let orig_hdrs = &raw[..pos + 2];
        let body = &raw[pos + 2..];
        record.extend_from_slice(&normalize_lf_to_os(orig_hdrs));
        record.extend_from_slice(b"*** EOOH ***");
        record.extend_from_slice(OS_LINESEP);
        if is_instance_of_mod_cls(interp, message, "mailbox", "BabylMessage") {
            let gv = interp.get_attr(message, "get_visible")?;
            let vis_msg = call_pos(interp, gv, &[])?;
            let vis_raw =
                normalize_os_to_lf(&serialize_message_to_bytes(interp, &vis_msg, false, false)?);
            let vis_hdrs = if let Some(vp) = vis_raw.windows(2).position(|w| w == b"\n\n") {
                &vis_raw[..vp + 2]
            } else {
                vis_raw.as_slice()
            };
            record.extend_from_slice(&normalize_lf_to_os(vis_hdrs));
            if !vis_hdrs.ends_with(b"\n\n") {
                if !vis_hdrs.ends_with(b"\n") && !vis_hdrs.is_empty() {
                    record.extend_from_slice(OS_LINESEP);
                }
                record.extend_from_slice(OS_LINESEP);
            }
        } else {
            record.extend_from_slice(&normalize_lf_to_os(orig_hdrs));
        }
        record.extend_from_slice(&normalize_lf_to_os(body));
    } else {
        record.extend_from_slice(b"*** EOOH ***");
        record.extend_from_slice(OS_LINESEP);
        record.extend_from_slice(OS_LINESEP);
        record.extend_from_slice(&normalize_lf_to_os(&raw));
    }

    Ok((record, msg_labels))
}

fn append_record_to_singlefile(st: &mut SingleFileState, record: &[u8]) -> Result<(usize, usize)> {
    st.ensure_toc()?;
    let need_pre_mb = st.toc.as_ref().is_none_or(|t| t.is_empty()) && !st.pending;
    let pre_mb = if need_pre_mb {
        st.pre_mailbox_bytes()?
    } else {
        Vec::new()
    };
    let pre_msg = st.pre_message_bytes();
    let post_msg = st.post_message_bytes();

    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&st.path)
        .map_err(|e| PyError::from_io_error(&e, Some(&st.path.to_string_lossy())))?;
    let before = f
        .seek(SeekFrom::End(0))
        .map_err(|e| PyError::from_io_error(&e, Some(&st.path.to_string_lossy())))?;

    let write_res = (|| -> Result<(usize, usize, u64)> {
        if !pre_mb.is_empty() {
            f.write_all(&pre_mb)
                .map_err(|e| PyError::from_io_error(&e, Some(&st.path.to_string_lossy())))?;
        }
        if !pre_msg.is_empty() {
            f.write_all(&pre_msg)
                .map_err(|e| PyError::from_io_error(&e, Some(&st.path.to_string_lossy())))?;
        }
        let start = f
            .stream_position()
            .map_err(|e| PyError::from_io_error(&e, Some(&st.path.to_string_lossy())))?
            as usize;
        f.write_all(record)
            .map_err(|e| PyError::from_io_error(&e, Some(&st.path.to_string_lossy())))?;
        let stop = f
            .stream_position()
            .map_err(|e| PyError::from_io_error(&e, Some(&st.path.to_string_lossy())))?
            as usize;
        if !post_msg.is_empty() {
            f.write_all(&post_msg)
                .map_err(|e| PyError::from_io_error(&e, Some(&st.path.to_string_lossy())))?;
        }
        f.flush()
            .map_err(|e| PyError::from_io_error(&e, Some(&st.path.to_string_lossy())))?;
        let end = f
            .stream_position()
            .map_err(|e| PyError::from_io_error(&e, Some(&st.path.to_string_lossy())))?;
        Ok((start, stop, end))
    })();

    match write_res {
        Ok((start, stop, end)) => {
            st.file_length = Some(end);
            Ok((start, stop))
        }
        Err(err) => {
            let _ = f.set_len(before);
            Err(err)
        }
    }
}

struct MaildirState {
    path: PathBuf,
    toc: IndexMap<String, String>,
    last_read: f64,
}

impl MaildirState {
    fn refresh(&mut self, colon: &str) -> Result<()> {
        self.toc.clear();
        for subdir in ["cur", "new"] {
            let dir = self.path.join(subdir);
            let entries = match fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            let mut names: Vec<String> = Vec::new();
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    continue;
                }
                if let Some(name) = entry.file_name().to_str() {
                    names.push(name.to_string());
                }
            }
            names.sort();
            for entry in names {
                let uniq = entry.split(colon).next().unwrap_or(&entry).to_string();
                self.toc.insert(uniq, format!("{subdir}/{entry}"));
            }
        }
        self.last_read = now_secs_f64();
        Ok(())
    }

    fn lookup(&mut self, key: &str, colon: &str) -> Result<String> {
        if let Some(sub) = self.toc.get(key)
            && self.path.join(sub).exists()
        {
            return Ok(sub.clone());
        }
        self.refresh(colon)?;
        self.toc
            .get(key)
            .cloned()
            .ok_or_else(|| PyError::named("KeyError", format!("No message with key: {key}")))
    }
}

struct MhState {
    path: PathBuf,
    locked: bool,
}

impl MhState {
    fn list_numeric_keys(&self) -> Result<Vec<i64>> {
        let mut keys = Vec::new();
        let entries = fs::read_dir(&self.path)
            .map_err(|e| PyError::from_io_error(&e, Some(&self.path.to_string_lossy())))?;
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && !name.is_empty()
                && name.bytes().all(|b| b.is_ascii_digit())
                && let Ok(n) = name.parse::<i64>()
            {
                keys.push(n);
            }
        }
        keys.sort_unstable();
        Ok(keys)
    }

    fn read_sequences(&self, interp: &mut Interpreter) -> Result<IndexMap<String, Vec<i64>>> {
        let seq_path = self.path.join(".mh_sequences");
        let content = fs::read_to_string(&seq_path)
            .map_err(|e| PyError::from_io_error(&e, Some(&seq_path.to_string_lossy())))?;
        let all_keys: HashSet<i64> = self.list_numeric_keys()?.into_iter().collect();
        let mut results: IndexMap<String, Vec<i64>> = IndexMap::new();
        for line in content.lines() {
            let Some((name, contents)) = line.split_once(':') else {
                return Err(raise_mb_error(
                    interp,
                    "FormatError",
                    &format!("Invalid sequence specification: {}", line.trim_end()),
                ));
            };
            let mut keys = BTreeSet::new();
            for spec in contents.split_whitespace() {
                if spec.bytes().all(|b| b.is_ascii_digit()) {
                    let n = spec.parse::<i64>().map_err(|_| {
                        raise_mb_error(
                            interp,
                            "FormatError",
                            &format!("Invalid sequence specification: {}", line.trim_end()),
                        )
                    })?;
                    keys.insert(n);
                } else if let Some((a, b)) = spec.split_once('-') {
                    let start = a.parse::<i64>().map_err(|_| {
                        raise_mb_error(
                            interp,
                            "FormatError",
                            &format!("Invalid sequence specification: {}", line.trim_end()),
                        )
                    })?;
                    let stop = b.parse::<i64>().map_err(|_| {
                        raise_mb_error(
                            interp,
                            "FormatError",
                            &format!("Invalid sequence specification: {}", line.trim_end()),
                        )
                    })?;
                    for n in start..=stop {
                        keys.insert(n);
                    }
                } else {
                    return Err(raise_mb_error(
                        interp,
                        "FormatError",
                        &format!("Invalid sequence specification: {}", line.trim_end()),
                    ));
                }
            }
            let filtered: Vec<i64> = keys.into_iter().filter(|k| all_keys.contains(k)).collect();
            if !filtered.is_empty() {
                results.insert(name.to_string(), filtered);
            }
        }
        Ok(results)
    }

    fn write_sequences(&self, sequences: &IndexMap<String, Vec<i64>>) -> Result<()> {
        let seq_path = self.path.join(".mh_sequences");
        let mut out = String::new();
        for (name, keys) in sequences {
            if keys.is_empty() {
                continue;
            }
            let sorted: Vec<i64> = keys
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            out.push_str(name);
            out.push(':');
            let mut prev: Option<i64> = None;
            let mut completing = false;
            for key in sorted {
                if prev == Some(key - 1) {
                    if !completing {
                        completing = true;
                        out.push('-');
                    }
                } else if completing {
                    completing = false;
                    out.push_str(&format!("{} {}", prev.unwrap(), key));
                } else {
                    out.push_str(&format!(" {key}"));
                }
                prev = Some(key);
            }
            if completing {
                out.push_str(&format!("{}\n", prev.unwrap()));
            } else {
                out.push('\n');
            }
        }
        fs::write(&seq_path, out.as_bytes())
            .map_err(|e| PyError::from_io_error(&e, Some(&seq_path.to_string_lossy())))?;
        Ok(())
    }
}

struct ValueIterState {
    items: VecDeque<Value>,
}

fn make_value_iter(interp: &mut Interpreter, items: Vec<Value>) -> Result<Value> {
    let m = interp.load_module("mailbox")?;
    let cls = interp.get_attr(&m, "_MailboxIterator")?;
    call_pos(interp, cls, &[Value::list(items)])
}

fn copy_non_specific_attrs(interp: &mut Interpreter, dst: &Value, src: &Value) -> Result<()> {
    let mut skip: HashSet<String> = HashSet::new();
    if let Ok(tsa) = interp.get_attr(src, "_type_specific_attributes")
        && let Ok(items) = interp.collect_iterable(&tsa)
    {
        for it in items {
            if let Some(s) = it.as_str() {
                skip.insert(s.to_string());
            }
        }
    }
    let src_dict = interp.get_attr(src, "__dict__")?;
    let items_fn = interp.get_attr(&src_dict, "items")?;
    let items_val = call_pos(interp, items_fn, &[])?;
    let pairs = interp.collect_iterable(&items_val)?;
    for pair in pairs {
        let kv = interp.collect_iterable(&pair)?;
        if kv.len() >= 2
            && let Some(k) = kv[0].as_str()
            && !skip.contains(k)
        {
            interp.assign_attr(dst.clone(), k, kv[1].clone())?;
        }
    }
    Ok(())
}

fn init_mailbox_message(
    interp: &mut Interpreter,
    self_val: &Value,
    message: Option<&Value>,
) -> Result<()> {
    match message {
        None => {
            let em = interp.load_module("email.message")?;
            let msg_cls = interp.get_attr(&em, "Message")?;
            let init_fn = interp.get_attr(&msg_cls, "__init__")?;
            call_pos(interp, init_fn, std::slice::from_ref(self_val))?;
        }
        Some(v) if v.is_none() => {
            let em = interp.load_module("email.message")?;
            let msg_cls = interp.get_attr(&em, "Message")?;
            let init_fn = interp.get_attr(&msg_cls, "__init__")?;
            call_pos(interp, init_fn, std::slice::from_ref(self_val))?;
        }
        Some(v) => {
            if is_instance_of_mod_cls(interp, v, "email.message", "Message") {
                let copy_mod = interp.load_module("copy")?;
                let deepcopy = interp.get_attr(&copy_mod, "deepcopy")?;
                let cloned = call_pos(interp, deepcopy, std::slice::from_ref(v))?;
                copy_non_specific_attrs(interp, self_val, &cloned)?;
                if is_instance_of_mod_cls(interp, v, "mailbox", "Message") {
                    let explain = interp.get_attr(v, "_explain_to")?;
                    call_pos(interp, explain, std::slice::from_ref(self_val))?;
                }
            } else if let Some(b) = value_as_bytes(v) {
                let parsed = email_from_bytes(interp, b)?;
                copy_non_specific_attrs(interp, self_val, &parsed)?;
            } else if let Some(s) = v.as_str() {
                let parsed = email_from_string(interp, s)?;
                copy_non_specific_attrs(interp, self_val, &parsed)?;
            } else if is_instance_of_mod_cls(interp, v, "io", "TextIOWrapper") {
                let email_mod = interp.load_module("email")?;
                let mff = interp.get_attr(&email_mod, "message_from_file")?;
                let parsed = call_pos(interp, mff, std::slice::from_ref(v))?;
                copy_non_specific_attrs(interp, self_val, &parsed)?;
            } else if interp.get_attr(v, "read").is_ok() {
                let email_mod = interp.load_module("email")?;
                let mfbf = interp.get_attr(&email_mod, "message_from_binary_file")?;
                let parsed = call_pos(interp, mfbf, std::slice::from_ref(v))?;
                copy_non_specific_attrs(interp, self_val, &parsed)?;
            } else {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "Invalid message type: {}",
                        crate::interpreter::value_type_name_str(v)
                    ),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let email_msg_mod = interp.load_module("email.message")?;
    let email_msg_cls_val = interp.get_attr(&email_msg_mod, "Message")?;
    let ValueKind::PyClass(email_msg_cls) = email_msg_cls_val.kind() else {
        return Err(PyError::runtime(
            "email.message.Message is not a class".to_string(),
        ));
    };
    let cls_val = module.borrow().attrs.get("Message").cloned();
    if let Some(val) = cls_val
        && let ValueKind::PyClass(cls_rc) = val.kind()
    {
        link_class_bases(cls_rc, vec![Rc::clone(email_msg_cls)]);
    }
    Ok(())
}

fn all_list() -> Value {
    Value::list(vec![
        Value::string("Mailbox"),
        Value::string("Maildir"),
        Value::string("mbox"),
        Value::string("MH"),
        Value::string("Babyl"),
        Value::string("MMDF"),
        Value::string("Message"),
        Value::string("MaildirMessage"),
        Value::string("mboxMessage"),
        Value::string("MHMessage"),
        Value::string("BabylMessage"),
        Value::string("MMDFMessage"),
        Value::string("Error"),
        Value::string("NoSuchMailboxError"),
        Value::string("NotEmptyError"),
        Value::string("ExternalClashError"),
        Value::string("FormatError"),
    ])
}

pyrust_module! {
    constants {
        "__all__" => all_list(),
        "linesep" => Value::bytes(OS_LINESEP.to_vec()),
    }

    exception Error(Exception);
    exception NoSuchMailboxError(Error);
    exception NotEmptyError(Error);
    exception ExternalClashError(Error);
    exception FormatError(Error);

    #[native_state(ValueIterState)]
    class _MailboxIterator {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            items: PyValue,
        ) -> Result<Value> {
            let collected = _interp.collect_iterable(&items.0)?;
            set_state(
                self_val.0,
                ValueIterState {
                    items: VecDeque::from(collected),
                },
            )?;
            Ok(Value::none())
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __next__(#[state_mut] st: &mut ValueIterState) -> Result<Value> {
            match st.items.pop_front() {
                Some(v) => Ok(v),
                None => Err(PyError::named("StopIteration", String::new())),
            }
        }
    }

    class _ProxyFile {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            f: PyValue,
            #[default(OptionalArg::Omitted)] pos: OptValueRef<'_>,
        ) -> Result<Value> {
            let p = match pos.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => {
                    let tell_fn = _interp.get_attr(&f.0, "tell")?;
                    call_pos(_interp, tell_fn, &[])?
                }
            };
            _interp.assign_attr(self_val.0.clone(), "_file", f.0)?;
            _interp.assign_attr(self_val.0.clone(), "_pos", p)?;
            Ok(Value::none())
        }

        fn _read(
            #[positional_only] self_val: PyValueRef<'_>,
            size: PyValue,
            read_method: PyValue,
        ) -> Result<Value> {
            let sz = if size.0.is_none() {
                Value::int(-1)
            } else {
                size.0
            };
            let f = _interp.get_attr(self_val.0, "_file")?;
            let pos = _interp.get_attr(self_val.0, "_pos")?;
            let seek_fn = _interp.get_attr(&f, "seek")?;
            call_pos(_interp, seek_fn, &[pos])?;
            let res = call_pos(_interp, read_method.0, &[sz])?;
            let tell_fn = _interp.get_attr(&f, "tell")?;
            let new_pos = call_pos(_interp, tell_fn, &[])?;
            _interp.assign_attr(self_val.0.clone(), "_pos", new_pos)?;
            Ok(res)
        }

        fn read(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] size: OptValueRef<'_>,
        ) -> Result<Value> {
            let f = _interp.get_attr(self_val.0, "_file")?;
            let rm = _interp.get_attr(&f, "read")?;
            let _read = _interp.get_attr(self_val.0, "_read")?;
            call_pos(
                _interp,
                _read,
                &[size.as_value().cloned().unwrap_or_else(Value::none), rm],
            )
        }

        fn read1(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] size: OptValueRef<'_>,
        ) -> Result<Value> {
            let f = _interp.get_attr(self_val.0, "_file")?;
            let rm = _interp
                .get_attr(&f, "read1")
                .or_else(|_| _interp.get_attr(&f, "read"))?;
            let _read = _interp.get_attr(self_val.0, "_read")?;
            call_pos(
                _interp,
                _read,
                &[size.as_value().cloned().unwrap_or_else(Value::none), rm],
            )
        }

        fn readline(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] size: OptValueRef<'_>,
        ) -> Result<Value> {
            let f = _interp.get_attr(self_val.0, "_file")?;
            let rm = _interp.get_attr(&f, "readline")?;
            let _read = _interp.get_attr(self_val.0, "_read")?;
            call_pos(
                _interp,
                _read,
                &[size.as_value().cloned().unwrap_or_else(Value::none), rm],
            )
        }

        fn readlines(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] sizehint: OptValueRef<'_>,
        ) -> Result<Value> {
            let mut rem = sizehint.as_value().and_then(|v| v.as_int());
            let rl = _interp.get_attr(self_val.0, "readline")?;
            let mut lines = Vec::new();
            loop {
                let line = call_pos(_interp, rl.clone(), &[])?;
                let len = value_as_bytes(&line).map(|b| b.len() as i64).unwrap_or(0);
                if len == 0 {
                    break;
                }
                lines.push(line);
                if let Some(ref mut r) = rem {
                    *r -= len;
                    if *r <= 0 {
                        break;
                    }
                }
            }
            Ok(Value::list(lines))
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __next__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let rl = _interp.get_attr(self_val.0, "readline")?;
            let line = call_pos(_interp, rl, &[])?;
            if !_interp.truthy_value(&line)? {
                return Err(PyError::named("StopIteration", String::new()));
            }
            Ok(line)
        }

        fn tell(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_pos")
        }

        fn seek(
            #[positional_only] self_val: PyValueRef<'_>,
            offset: PyIndex,
            #[default(0i64.into())] whence: PyIndex,
        ) -> Result<Value> {
            let off = offset.expect_i64(FN_NAME, "offset")?;
            let wh = whence.expect_i64(FN_NAME, "whence")?;
            let f = _interp.get_attr(self_val.0, "_file")?;
            let seek_fn = _interp.get_attr(&f, "seek")?;
            if wh == 1 {
                let pos = _interp.get_attr(self_val.0, "_pos")?;
                call_pos(_interp, seek_fn.clone(), &[pos])?;
            }
            call_pos(_interp, seek_fn, &[Value::int(off), Value::int(wh)])?;
            let tell_fn = _interp.get_attr(&f, "tell")?;
            let new_pos = call_pos(_interp, tell_fn, &[])?;
            _interp.assign_attr(self_val.0.clone(), "_pos", new_pos)?;
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            if let Ok(f) = _interp.get_attr(self_val.0, "_file")
                && !f.is_none()
            {
                if let Ok(c) = _interp.get_attr(&f, "close") {
                    let _ = call_pos(_interp, c, &[]);
                }
                _interp.assign_attr(self_val.0.clone(), "_file", Value::none())?;
            }
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _exc: &[Value],
        ) -> Result<Value> {
            let c = _interp.get_attr(self_val.0, "close")?;
            call_pos(_interp, c, &[])?;
            Ok(Value::none())
        }

        fn readable(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let f = _interp.get_attr(self_val.0, "_file")?;
            let m = _interp.get_attr(&f, "readable")?;
            call_pos(_interp, m, &[])
        }

        fn writable(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let f = _interp.get_attr(self_val.0, "_file")?;
            let m = _interp.get_attr(&f, "writable")?;
            call_pos(_interp, m, &[])
        }

        fn seekable(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let f = _interp.get_attr(self_val.0, "_file")?;
            let m = _interp.get_attr(&f, "seekable")?;
            call_pos(_interp, m, &[])
        }

        fn flush(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let f = _interp.get_attr(self_val.0, "_file")?;
            let m = _interp.get_attr(&f, "flush")?;
            call_pos(_interp, m, &[])
        }

        #[getter]
        fn closed(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let Ok(f) = _interp.get_attr(self_val.0, "_file") else {
                return Ok(Value::bool_(true));
            };
            if f.is_none() {
                return Ok(Value::bool_(true));
            }
            match _interp.get_attr(&f, "closed") {
                Ok(v) => Ok(v),
                Err(_) => Ok(Value::bool_(false)),
            }
        }
    }

    class _PartialFile("_ProxyFile") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            f: PyValue,
            #[default(OptionalArg::Omitted)] start: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] stop: OptValueRef<'_>,
        ) -> Result<Value> {
            let st = start.as_value().cloned().unwrap_or_else(|| Value::int(0));
            let sp = stop.as_value().cloned().unwrap_or_else(|| Value::int(0));
            let m = _interp.load_module("mailbox")?;
            let pf = _interp.get_attr(&m, "_ProxyFile")?;
            let init_fn = _interp.get_attr(&pf, "__init__")?;
            call_pos(_interp, init_fn, &[self_val.0.clone(), f.0, st.clone()])?;
            _interp.assign_attr(self_val.0.clone(), "_start", st)?;
            _interp.assign_attr(self_val.0.clone(), "_stop", sp)?;
            Ok(Value::none())
        }

        fn tell(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let pos = _interp.get_attr(self_val.0, "_pos")?.as_int().unwrap_or(0);
            let start = _interp.get_attr(self_val.0, "_start")?.as_int().unwrap_or(0);
            Ok(Value::int(pos - start))
        }

        fn seek(
            #[positional_only] self_val: PyValueRef<'_>,
            offset: PyIndex,
            #[default(0i64.into())] whence: PyIndex,
        ) -> Result<Value> {
            let off = offset.expect_i64(FN_NAME, "offset")?;
            let mut wh = whence.expect_i64(FN_NAME, "whence")?;
            if wh == 0 {
                let start = _interp.get_attr(self_val.0, "_start")?;
                _interp.assign_attr(self_val.0.clone(), "_pos", start)?;
                wh = 1;
            } else if wh == 2 {
                let stop = _interp.get_attr(self_val.0, "_stop")?;
                _interp.assign_attr(self_val.0.clone(), "_pos", stop)?;
                wh = 1;
            }
            let m = _interp.load_module("mailbox")?;
            let pf = _interp.get_attr(&m, "_ProxyFile")?;
            let seek_fn = _interp.get_attr(&pf, "seek")?;
            call_pos(
                _interp,
                seek_fn,
                &[self_val.0.clone(), Value::int(off), Value::int(wh)],
            )
        }

        fn _read(
            #[positional_only] self_val: PyValueRef<'_>,
            size: PyValue,
            read_method: PyValue,
        ) -> Result<Value> {
            let stop = _interp.get_attr(self_val.0, "_stop")?.as_int().unwrap_or(0);
            let pos = _interp.get_attr(self_val.0, "_pos")?.as_int().unwrap_or(0);
            let remaining = stop - pos;
            if remaining <= 0 {
                return Ok(Value::bytes(Vec::new()));
            }
            let sz = match size.0.as_int() {
                Some(n) if n >= 0 && n <= remaining => n,
                _ => remaining,
            };
            let m = _interp.load_module("mailbox")?;
            let pf = _interp.get_attr(&m, "_ProxyFile")?;
            let r_fn = _interp.get_attr(&pf, "_read")?;
            call_pos(
                _interp,
                r_fn,
                &[self_val.0.clone(), Value::int(sz), read_method.0],
            )
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_file", Value::none())?;
            Ok(Value::none())
        }
    }

    // ── Base Mailbox ─────────────────────────────────────────────────────────

    class Mailbox {
        constants {
            "_append_newline" => Value::bool_(false),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            path: PyValue,
            #[default(OptionalArg::Omitted)] factory: OptValueRef<'_>,
            #[default(true.into())] _create: PyBool,
        ) -> Result<Value> {
            let resolved = resolve_mailbox_path(_interp, &path.0)?;
            let fac = factory.as_value().cloned().unwrap_or_else(Value::none);
            _interp.assign_attr(self_val.0.clone(), "_path", Value::string(resolved))?;
            _interp.assign_attr(self_val.0.clone(), "_factory", fac)?;
            Ok(Value::none())
        }

        fn add(#[positional_only] _self_val: PyValueRef<'_>, _message: PyValue) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "Method must be implemented by subclass".to_string(),
            ))
        }

        fn remove(#[positional_only] _self_val: PyValueRef<'_>, _key: PyValue) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "Method must be implemented by subclass".to_string(),
            ))
        }

        fn __delitem__(#[positional_only] self_val: PyValueRef<'_>, key: PyValue) -> Result<Value> {
            let rem = _interp.get_attr(self_val.0, "remove")?;
            call_pos(_interp, rem, &[key.0])
        }

        fn discard(#[positional_only] self_val: PyValueRef<'_>, key: PyValue) -> Result<Value> {
            let rem = _interp.get_attr(self_val.0, "remove")?;
            match call_pos(_interp, rem, &[key.0]) {
                Ok(_) => Ok(Value::none()),
                Err(e) if e.class_name_is("KeyError") => Ok(Value::none()),
                Err(e) => Err(e),
            }
        }

        fn __setitem__(
            #[positional_only] _self_val: PyValueRef<'_>,
            _key: PyValue,
            _message: PyValue,
        ) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "Method must be implemented by subclass".to_string(),
            ))
        }

        fn get(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
            #[default(OptionalArg::Omitted)] default: OptValueRef<'_>,
        ) -> Result<Value> {
            let gi = _interp.get_attr(self_val.0, "__getitem__")?;
            match call_pos(_interp, gi, &[key.0]) {
                Ok(v) => Ok(v),
                Err(e) if e.class_name_is("KeyError") => {
                    Ok(default.as_value().cloned().unwrap_or_else(Value::none))
                }
                Err(e) => Err(e),
            }
        }

        fn __getitem__(#[positional_only] self_val: PyValueRef<'_>, key: PyValue) -> Result<Value> {
            let fac = _interp.get_attr(self_val.0, "_factory")?;
            if !_interp.truthy_value(&fac)? {
                let gm = _interp.get_attr(self_val.0, "get_message")?;
                call_pos(_interp, gm, &[key.0])
            } else {
                let gf = _interp.get_attr(self_val.0, "get_file")?;
                let file_obj = call_pos(_interp, gf, &[key.0])?;
                let res = call_pos(_interp, fac, std::slice::from_ref(&file_obj));
                if let Ok(close_fn) = _interp.get_attr(&file_obj, "close") {
                    let _ = call_pos(_interp, close_fn, &[]);
                }
                res
            }
        }

        fn get_message(#[positional_only] _self_val: PyValueRef<'_>, _key: PyValue) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "Method must be implemented by subclass".to_string(),
            ))
        }

        fn get_string(#[positional_only] self_val: PyValueRef<'_>, key: PyValue) -> Result<Value> {
            let gb = _interp.get_attr(self_val.0, "get_bytes")?;
            let raw = call_pos(_interp, gb, &[key.0])?;
            let bl = PyBytesLike::try_from_value_with_interp(_interp, &raw, FN_NAME, "bytes")?;
            let msg = email_from_bytes(_interp, bl.as_bytes())?;
            let as_str = _interp.get_attr(&msg, "as_string")?;
            call_pos(_interp, as_str, &[])
        }

        fn get_bytes(#[positional_only] _self_val: PyValueRef<'_>, _key: PyValue) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "Method must be implemented by subclass".to_string(),
            ))
        }

        fn get_file(#[positional_only] _self_val: PyValueRef<'_>, _key: PyValue) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "Method must be implemented by subclass".to_string(),
            ))
        }

        fn iterkeys(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "Method must be implemented by subclass".to_string(),
            ))
        }

        fn keys(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ik = _interp.get_attr(self_val.0, "iterkeys")?;
            let it = call_pos(_interp, ik, &[])?;
            Ok(Value::list(_interp.collect_iterable(&it)?))
        }

        fn itervalues(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ik = _interp.get_attr(self_val.0, "iterkeys")?;
            let keys_it = call_pos(_interp, ik, &[])?;
            let keys = _interp.collect_iterable(&keys_it)?;
            let gi = _interp.get_attr(self_val.0, "__getitem__")?;
            let mut vals = Vec::with_capacity(keys.len());
            for k in keys {
                match call_pos(_interp, gi.clone(), &[k]) {
                    Ok(v) => vals.push(v),
                    Err(e) if e.class_name_is("KeyError") => continue,
                    Err(e) => return Err(e),
                }
            }
            make_value_iter(_interp, vals)
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let iv = _interp.get_attr(self_val.0, "itervalues")?;
            call_pos(_interp, iv, &[])
        }

        fn values(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let iv = _interp.get_attr(self_val.0, "itervalues")?;
            let it = call_pos(_interp, iv, &[])?;
            Ok(Value::list(_interp.collect_iterable(&it)?))
        }

        fn iteritems(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ik = _interp.get_attr(self_val.0, "iterkeys")?;
            let keys_it = call_pos(_interp, ik, &[])?;
            let keys = _interp.collect_iterable(&keys_it)?;
            let gi = _interp.get_attr(self_val.0, "__getitem__")?;
            let mut pairs = Vec::with_capacity(keys.len());
            for k in keys {
                match call_pos(_interp, gi.clone(), std::slice::from_ref(&k)) {
                    Ok(v) => pairs.push(Value::tuple(vec![k, v])),
                    Err(e) if e.class_name_is("KeyError") => continue,
                    Err(e) => return Err(e),
                }
            }
            make_value_iter(_interp, pairs)
        }

        fn items(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ii = _interp.get_attr(self_val.0, "iteritems")?;
            let it = call_pos(_interp, ii, &[])?;
            Ok(Value::list(_interp.collect_iterable(&it)?))
        }

        fn __contains__(#[positional_only] _self_val: PyValueRef<'_>, _key: PyValue) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "Method must be implemented by subclass".to_string(),
            ))
        }

        fn __len__(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "Method must be implemented by subclass".to_string(),
            ))
        }

        fn clear(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let keys_fn = _interp.get_attr(self_val.0, "keys")?;
            let keys_val = call_pos(_interp, keys_fn, &[])?;
            let keys = _interp.collect_iterable(&keys_val)?;
            let discard_fn = _interp.get_attr(self_val.0, "discard")?;
            for k in keys {
                call_pos(_interp, discard_fn.clone(), &[k])?;
            }
            Ok(Value::none())
        }

        fn pop(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
            #[default(OptionalArg::Omitted)] default: OptValueRef<'_>,
        ) -> Result<Value> {
            let gi = _interp.get_attr(self_val.0, "__getitem__")?;
            let result = match call_pos(_interp, gi, std::slice::from_ref(&key.0)) {
                Ok(v) => v,
                Err(e) if e.class_name_is("KeyError") => {
                    return Ok(default.as_value().cloned().unwrap_or_else(Value::none));
                }
                Err(e) => return Err(e),
            };
            let disc = _interp.get_attr(self_val.0, "discard")?;
            call_pos(_interp, disc, &[key.0])?;
            Ok(result)
        }

        fn popitem(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ik = _interp.get_attr(self_val.0, "iterkeys")?;
            let keys_it = call_pos(_interp, ik, &[])?;
            let keys = _interp.collect_iterable(&keys_it)?;
            if let Some(first_key) = keys.into_iter().next() {
                let pop_fn = _interp.get_attr(self_val.0, "pop")?;
                let val = call_pos(_interp, pop_fn, std::slice::from_ref(&first_key))?;
                Ok(Value::tuple(vec![first_key, val]))
            } else {
                Err(PyError::named(
                    "KeyError",
                    "No messages in mailbox".to_string(),
                ))
            }
        }

        fn update(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] arg: OptValueRef<'_>,
        ) -> Result<Value> {
            let Some(src_arg) = arg.as_value() else {
                return Ok(Value::none());
            };
            let source_val = if let Ok(ii) = _interp.get_attr(src_arg, "iteritems") {
                call_pos(_interp, ii, &[])?
            } else if let Ok(it) = _interp.get_attr(src_arg, "items") {
                call_pos(_interp, it, &[])?
            } else {
                src_arg.clone()
            };
            let pairs = _interp.collect_iterable(&source_val)?;
            let setitem = _interp.get_attr(self_val.0, "__setitem__")?;
            let mut bad_key = false;
            for pair in pairs {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() >= 2 {
                    match call_pos(_interp, setitem.clone(), &[kv[0].clone(), kv[1].clone()]) {
                        Ok(_) => {}
                        Err(e) if e.class_name_is("KeyError") => bad_key = true,
                        Err(e) => return Err(e),
                    }
                }
            }
            if bad_key {
                return Err(PyError::named(
                    "KeyError",
                    "No message with key(s)".to_string(),
                ));
            }
            Ok(Value::none())
        }

        fn flush(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "Method must be implemented by subclass".to_string(),
            ))
        }

        fn lock(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "Method must be implemented by subclass".to_string(),
            ))
        }

        fn unlock(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "Method must be implemented by subclass".to_string(),
            ))
        }

        fn close(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "Method must be implemented by subclass".to_string(),
            ))
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _exc: &[Value],
        ) -> Result<Value> {
            let close_fn = _interp.get_attr(self_val.0, "close")?;
            call_pos(_interp, close_fn, &[])?;
            Ok(Value::none())
        }

        #[classmethod]
        fn __class_getitem__(
            #[positional_only] cls: PyValueRef<'_>,
            item: PyValue,
        ) -> Result<Value> {
            let types_mod = _interp.load_module("types")?;
            let ga = _interp.get_attr(&types_mod, "GenericAlias")?;
            call_pos(_interp, ga, &[cls.0.clone(), item.0])
        }
    }

    // ── Maildir ──────────────────────────────────────────────────────────────

    #[native_state(MaildirState)]
    class Maildir("Mailbox") {
        constants {
            "colon" => Value::string(":"),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            dirname: PyValue,
            #[default(OptionalArg::Omitted)] factory: OptValueRef<'_>,
            #[default(true.into())] create: PyBool,
        ) -> Result<Value> {
            let resolved = resolve_mailbox_path(_interp, &dirname.0)?;
            let path = PathBuf::from(&resolved);
            let fac = factory.as_value().cloned().unwrap_or_else(Value::none);
            _interp.assign_attr(self_val.0.clone(), "_path", Value::string(&resolved))?;
            _interp.assign_attr(self_val.0.clone(), "_factory", fac)?;

            if !path.exists() {
                if create.0 {
                    fs::create_dir(&path)
                        .map_err(|e| PyError::from_io_error(&e, Some(&resolved)))?;
                    for sub in ["tmp", "new", "cur"] {
                        let sp = path.join(sub);
                        fs::create_dir(&sp)
                            .map_err(|e| PyError::from_io_error(&e, Some(&sp.to_string_lossy())))?;
                    }
                } else {
                    return Err(raise_mb_error(_interp, "NoSuchMailboxError", &resolved));
                }
            }

            let mut paths_dict = PyDict::default();
            for sub in ["tmp", "new", "cur"] {
                paths_dict.insert(
                    PyKey::str_from(sub),
                    Value::string(path.join(sub).to_string_lossy().as_ref()),
                );
            }
            _interp.assign_attr(self_val.0.clone(), "_paths", Value::dict(paths_dict))?;
            _interp.assign_attr(self_val.0.clone(), "_skewfactor", Value::float(0.1))?;
            set_state(
                self_val.0,
                MaildirState {
                    path,
                    toc: IndexMap::new(),
                    last_read: 0.0,
                },
            )?;
            Ok(Value::none())
        }

        fn add(#[positional_only] self_val: PyValueRef<'_>, message: PyValue) -> Result<Value> {
            let colon = _interp
                .get_attr(self_val.0, "colon")?
                .as_str()
                .unwrap_or(":")
                .to_string();
            let base_path = with_state(self_val.0, |st| Ok(st.path.clone()))?;
            let data = serialize_message_to_bytes(_interp, &message.0, false, false)?;

            let now = now_secs_f64();
            let secs = now as u64;
            let micros = ((now - (secs as f64)) * 1e6) as u64;
            let pid = std::process::id();
            let seq = MAILDIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let sock_mod = _interp.load_module("socket")?;
            let ghn = _interp.get_attr(&sock_mod, "gethostname")?;
            let host_v = call_pos(_interp, ghn, &[])?;
            let host = host_v
                .as_str()
                .unwrap_or("localhost")
                .replace('/', "\\057")
                .replace(':', "\\072");
            let uniq = format!("{secs}.M{micros}P{pid}Q{seq}.{host}");
            let tmp_path = base_path.join("tmp").join(&uniq);

            {
                let mut f = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&tmp_path)
                    .map_err(|e| {
                        if e.kind() == std::io::ErrorKind::AlreadyExists {
                            raise_mb_error(
                                _interp,
                                "ExternalClashError",
                                &format!("Name clash prevented file creation: {}", tmp_path.display()),
                            )
                        } else {
                            PyError::from_io_error(&e, Some(&tmp_path.to_string_lossy()))
                        }
                    })?;
                if let Err(e) = f.write_all(&data).and_then(|_| f.sync_all()) {
                    drop(f);
                    let _ = fs::remove_file(&tmp_path);
                    return Err(PyError::from_io_error(&e, Some(&tmp_path.to_string_lossy())));
                }
            }

            let (subdir, suffix) =
                if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "MaildirMessage") {
                    let gs = _interp.get_attr(&message.0, "get_subdir")?;
                    let sd = call_pos(_interp, gs, &[])?
                        .as_str()
                        .unwrap_or("new")
                        .to_string();
                    let gi = _interp.get_attr(&message.0, "get_info")?;
                    let info = call_pos(_interp, gi, &[])?
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    let suf = if info.is_empty() {
                        String::new()
                    } else {
                        format!("{colon}{info}")
                    };
                    let gd = _interp.get_attr(&message.0, "get_date")?;
                    let date_v = call_pos(_interp, gd, &[])?;
                    if let Ok(os_mod) = _interp.load_module("os")
                        && let Ok(utime) = _interp.get_attr(&os_mod, "utime")
                    {
                        let p_str = Value::string(tmp_path.to_string_lossy().as_ref());
                        let _ = call_pos(
                            _interp,
                            utime,
                            &[p_str, Value::tuple(vec![date_v.clone(), date_v])],
                        );
                    }
                    (sd, suf)
                } else {
                    ("new".to_string(), String::new())
                };

            let dest = base_path.join(&subdir).join(format!("{uniq}{suffix}"));
            if dest.exists() {
                let _ = fs::remove_file(&tmp_path);
                return Err(raise_mb_error(
                    _interp,
                    "ExternalClashError",
                    &format!("Name clash with existing message: {}", dest.display()),
                ));
            }
            fs::rename(&tmp_path, &dest).map_err(|e| {
                let _ = fs::remove_file(&tmp_path);
                PyError::from_io_error(&e, Some(&dest.to_string_lossy()))
            })?;

            with_state_mut(self_val.0, |st| {
                st.toc.insert(uniq.clone(), format!("{subdir}/{uniq}{suffix}"));
                Ok(())
            })?;
            Ok(Value::string(uniq))
        }

        fn remove(#[positional_only] self_val: PyValueRef<'_>, key: PyStr<'_>) -> Result<Value> {
            let colon = _interp
                .get_attr(self_val.0, "colon")?
                .as_str()
                .unwrap_or(":")
                .to_string();
            let (base_path, sub) = with_state_mut(self_val.0, |st| {
                let sub = st.lookup(key.0.as_ref(), &colon)?;
                st.toc.shift_remove(key.0.as_ref());
                Ok((st.path.clone(), sub))
            })?;
            let full = base_path.join(&sub);
            fs::remove_file(&full)
                .map_err(|e| PyError::from_io_error(&e, Some(&full.to_string_lossy())))?;
            Ok(Value::none())
        }

        fn discard(#[positional_only] self_val: PyValueRef<'_>, key: PyValue) -> Result<Value> {
            let rem = _interp.get_attr(self_val.0, "remove")?;
            match call_pos(_interp, rem, &[key.0]) {
                Ok(_) => Ok(Value::none()),
                Err(e) if e.class_name_is("KeyError") || e.class_name_is("FileNotFoundError") => {
                    Ok(Value::none())
                }
                Err(e) => Err(e),
            }
        }

        fn __setitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyStr<'_>,
            message: PyValue,
        ) -> Result<Value> {
            let colon = _interp
                .get_attr(self_val.0, "colon")?
                .as_str()
                .unwrap_or(":")
                .to_string();
            let old_sub = with_state_mut(self_val.0, |st| st.lookup(key.0.as_ref(), &colon))?;
            let add_fn = _interp.get_attr(self_val.0, "add")?;
            let temp_key_v = call_pos(_interp, add_fn, std::slice::from_ref(&message.0))?;
            let temp_key = temp_key_v.as_str().unwrap_or("").to_string();
            let (base_path, temp_sub) = with_state_mut(self_val.0, |st| {
                let sub = st.lookup(&temp_key, &colon)?;
                Ok((st.path.clone(), sub))
            })?;

            let is_md_msg = is_instance_of_mod_cls(_interp, &message.0, "mailbox", "MaildirMessage");
            let dominant = if is_md_msg { &temp_sub } else { &old_sub };
            let subdir = Path::new(dominant)
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or("new")
                .to_string();
            let suffix = if let Some((_, tail)) = dominant.rsplit_once(&colon) {
                format!("{colon}{tail}")
            } else {
                String::new()
            };

            let disc = _interp.get_attr(self_val.0, "discard")?;
            call_pos(_interp, disc, &[Value::string(key.0.as_ref())])?;

            let tmp_full = base_path.join(&temp_sub);
            let new_name = format!("{}{}", key.0.as_ref(), suffix);
            let new_full = base_path.join(&subdir).join(&new_name);
            fs::rename(&tmp_full, &new_full)
                .map_err(|e| PyError::from_io_error(&e, Some(&new_full.to_string_lossy())))?;

            with_state_mut(self_val.0, |st| {
                st.toc.shift_remove(&temp_key);
                st.toc
                    .insert(key.0.to_string(), format!("{subdir}/{new_name}"));
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn get_message(#[positional_only] self_val: PyValueRef<'_>, key: PyStr<'_>) -> Result<Value> {
            let colon = _interp
                .get_attr(self_val.0, "colon")?
                .as_str()
                .unwrap_or(":")
                .to_string();
            let (base_path, sub) = with_state_mut(self_val.0, |st| {
                let sub = st.lookup(key.0.as_ref(), &colon)?;
                Ok((st.path.clone(), sub))
            })?;
            let full = base_path.join(&sub);
            let raw = fs::read(&full)
                .map_err(|e| PyError::from_io_error(&e, Some(&full.to_string_lossy())))?;
            let bio = make_bytes_io(_interp, raw)?;
            let fac = _interp.get_attr(self_val.0, "_factory")?;
            let msg = if _interp.truthy_value(&fac)? {
                call_pos(_interp, fac, &[bio])?
            } else {
                let mb_mod = _interp.load_module("mailbox")?;
                let md_cls = _interp.get_attr(&mb_mod, "MaildirMessage")?;
                call_pos(_interp, md_cls, &[bio])?
            };
            let subdir = Path::new(&sub)
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or("new");
            let name = Path::new(&sub)
                .file_name()
                .and_then(|p| p.to_str())
                .unwrap_or("");
            if let Ok(set_sd) = _interp.get_attr(&msg, "set_subdir") {
                call_pos(_interp, set_sd, &[Value::string(subdir)])?;
            }
            if let Some((_, info)) = name.rsplit_once(&colon)
                && let Ok(set_info) = _interp.get_attr(&msg, "set_info")
            {
                call_pos(_interp, set_info, &[Value::string(info)])?;
            }
            if let Ok(meta) = fs::metadata(&full)
                && let Ok(modified) = meta.modified()
                && let Ok(dur) = modified.duration_since(UNIX_EPOCH)
                && let Ok(set_date) = _interp.get_attr(&msg, "set_date")
            {
                call_pos(_interp, set_date, &[Value::float(dur.as_secs_f64())])?;
            }
            Ok(msg)
        }

        fn get_bytes(#[positional_only] self_val: PyValueRef<'_>, key: PyStr<'_>) -> Result<Value> {
            let colon = _interp
                .get_attr(self_val.0, "colon")?
                .as_str()
                .unwrap_or(":")
                .to_string();
            let (base_path, sub) = with_state_mut(self_val.0, |st| {
                let sub = st.lookup(key.0.as_ref(), &colon)?;
                Ok((st.path.clone(), sub))
            })?;
            let full = base_path.join(&sub);
            let raw = fs::read(&full)
                .map_err(|e| PyError::from_io_error(&e, Some(&full.to_string_lossy())))?;
            Ok(Value::bytes(normalize_os_to_lf(&raw)))
        }

        fn get_file(#[positional_only] self_val: PyValueRef<'_>, key: PyStr<'_>) -> Result<Value> {
            let colon = _interp
                .get_attr(self_val.0, "colon")?
                .as_str()
                .unwrap_or(":")
                .to_string();
            let (base_path, sub) = with_state_mut(self_val.0, |st| {
                let sub = st.lookup(key.0.as_ref(), &colon)?;
                Ok((st.path.clone(), sub))
            })?;
            let full = base_path.join(&sub);
            let raw = fs::read(&full)
                .map_err(|e| PyError::from_io_error(&e, Some(&full.to_string_lossy())))?;
            let bio = make_bytes_io(_interp, raw)?;
            let m = _interp.load_module("mailbox")?;
            let pf = _interp.get_attr(&m, "_ProxyFile")?;
            call_pos(_interp, pf, &[bio])
        }

        fn iterkeys(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let colon = _interp
                .get_attr(self_val.0, "colon")?
                .as_str()
                .unwrap_or(":")
                .to_string();
            let keys: Vec<Value> = with_state_mut(self_val.0, |st| {
                st.refresh(&colon)?;
                Ok(st.toc.keys().map(Value::string).collect())
            })?;
            make_value_iter(_interp, keys)
        }

        fn __contains__(#[positional_only] self_val: PyValueRef<'_>, key: PyValue) -> Result<Value> {
            let Some(k) = key.0.as_str() else {
                return Ok(Value::bool_(false));
            };
            let colon = _interp
                .get_attr(self_val.0, "colon")?
                .as_str()
                .unwrap_or(":")
                .to_string();
            let has = with_state_mut(self_val.0, |st| {
                st.refresh(&colon)?;
                Ok(st.toc.contains_key(k))
            })?;
            Ok(Value::bool_(has))
        }

        fn __len__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let colon = _interp
                .get_attr(self_val.0, "colon")?
                .as_str()
                .unwrap_or(":")
                .to_string();
            let len = with_state_mut(self_val.0, |st| {
                st.refresh(&colon)?;
                Ok(st.toc.len())
            })?;
            Ok(Value::int(len as i64))
        }

        fn flush(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        fn lock(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        fn unlock(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        fn close(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        fn list_folders(#[state] st: &MaildirState) -> Result<Value> {
            let mut result = Vec::new();
            let entries = fs::read_dir(&st.path)
                .map_err(|e| PyError::from_io_error(&e, Some(&st.path.to_string_lossy())))?;
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str()
                    && name.len() > 1
                    && name.starts_with('.')
                    && entry.path().is_dir()
                {
                    result.push(Value::string(&name[1..]));
                }
            }
            Ok(Value::list(result))
        }

        fn get_folder(#[positional_only] self_val: PyValueRef<'_>, folder: PyStr<'_>) -> Result<Value> {
            let base = with_state(self_val.0, |st| Ok(st.path.clone()))?;
            let fac = _interp.get_attr(self_val.0, "_factory")?;
            let folder_path = base.join(format!(".{}", folder.0.as_ref()));
            let m = _interp.load_module("mailbox")?;
            let cls = _interp.get_attr(&m, "Maildir")?;
            call_kw(
                _interp,
                cls,
                &[Value::string(folder_path.to_string_lossy().as_ref())],
                &[("factory", fac), ("create", Value::bool_(false))],
            )
        }

        fn add_folder(#[positional_only] self_val: PyValueRef<'_>, folder: PyStr<'_>) -> Result<Value> {
            let base = with_state(self_val.0, |st| Ok(st.path.clone()))?;
            let fac = _interp.get_attr(self_val.0, "_factory")?;
            let folder_path = base.join(format!(".{}", folder.0.as_ref()));
            let m = _interp.load_module("mailbox")?;
            let cls = _interp.get_attr(&m, "Maildir")?;
            let result = call_kw(
                _interp,
                cls,
                &[Value::string(folder_path.to_string_lossy().as_ref())],
                &[("factory", fac)],
            )?;
            let marker = folder_path.join("maildirfolder");
            if !marker.exists() {
                let _ = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(false)
                    .open(&marker);
            }
            Ok(result)
        }

        fn remove_folder(
            #[positional_only] self_val: PyValueRef<'_>,
            folder: PyStr<'_>,
        ) -> Result<Value> {
            let base = with_state(self_val.0, |st| Ok(st.path.clone()))?;
            let path = base.join(format!(".{}", folder.0.as_ref()));
            for sub in ["new", "cur"] {
                let sp = path.join(sub);
                if let Ok(entries) = fs::read_dir(&sp) {
                    for entry in entries.flatten() {
                        if let Some(name) = entry.file_name().to_str()
                            && !name.starts_with('.')
                        {
                            return Err(raise_mb_error(
                                _interp,
                                "NotEmptyError",
                                &format!("Folder contains message(s): {}", folder.0.as_ref()),
                            ));
                        }
                    }
                }
            }
            let entries = fs::read_dir(&path)
                .map_err(|e| PyError::from_io_error(&e, Some(&path.to_string_lossy())))?;
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str()
                    && !matches!(name, "new" | "cur" | "tmp")
                    && entry.path().is_dir()
                {
                    return Err(raise_mb_error(
                        _interp,
                        "NotEmptyError",
                        &format!(
                            "Folder contains subdirectory '{}': {}",
                            folder.0.as_ref(),
                            name
                        ),
                    ));
                }
            }
            fs::remove_dir_all(&path)
                .map_err(|e| PyError::from_io_error(&e, Some(&path.to_string_lossy())))?;
            Ok(Value::none())
        }

        fn clean(#[state] st: &MaildirState) -> Result<Value> {
            let now = SystemTime::now();
            let tmp_dir = st.path.join("tmp");
            if let Ok(entries) = fs::read_dir(&tmp_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if let Ok(meta) = fs::metadata(&p)
                        && let Ok(atime) = meta.accessed()
                        && let Ok(age) = now.duration_since(atime)
                        && age.as_secs() > 129_600
                    {
                        let _ = fs::remove_file(&p);
                    }
                }
            }
            Ok(Value::none())
        }
    }

    // ── _singlefileMailbox, _mboxMMDF, mbox, MMDF, Babyl ─────────────────────

    #[native_state(SingleFileState)]
    class _singlefileMailbox("Mailbox") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            path: PyValue,
            #[default(OptionalArg::Omitted)] factory: OptValueRef<'_>,
            #[default(true.into())] create: PyBool,
        ) -> Result<Value> {
            let resolved = resolve_mailbox_path(_interp, &path.0)?;
            let pb = PathBuf::from(&resolved);
            let fac = factory.as_value().cloned().unwrap_or_else(Value::none);
            _interp.assign_attr(self_val.0.clone(), "_path", Value::string(&resolved))?;
            _interp.assign_attr(self_val.0.clone(), "_factory", fac)?;

            if !pb.exists() {
                if create.0 {
                    OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create(true)
                        .truncate(true)
                        .open(&pb)
                        .map_err(|e| PyError::from_io_error(&e, Some(&resolved)))?;
                } else {
                    return Err(raise_mb_error(_interp, "NoSuchMailboxError", &resolved));
                }
            }

            let kind = if is_instance_of_mod_cls(_interp, self_val.0, "mailbox", "Babyl") {
                SingleFileKind::Babyl
            } else if is_instance_of_mod_cls(_interp, self_val.0, "mailbox", "MMDF") {
                SingleFileKind::Mmdf
            } else {
                SingleFileKind::Mbox
            };

            set_state(
                self_val.0,
                SingleFileState {
                    kind,
                    path: pb,
                    toc: None,
                    labels: BTreeMap::new(),
                    next_key: 0,
                    pending: false,
                    pending_sync: false,
                    locked: false,
                    closed: false,
                    file_length: None,
                },
            )?;
            Ok(Value::none())
        }

        fn add(#[positional_only] self_val: PyValueRef<'_>, message: PyValue) -> Result<Value> {
            let kind = with_state(self_val.0, |st| Ok(st.kind))?;
            let (record, babyl_labels) = match kind {
                SingleFileKind::Mbox => (format_mbox_mmdf_record(_interp, &message.0, true, true)?, None),
                SingleFileKind::Mmdf => (format_mbox_mmdf_record(_interp, &message.0, true, false)?, None),
                SingleFileKind::Babyl => format_babyl_record(_interp, &message.0)?,
            };
            let assigned = with_state_mut(self_val.0, |st| {
                st.ensure_toc()?;
                let offsets = append_record_to_singlefile(st, &record)?;
                let key = st.next_key;
                st.toc.as_mut().unwrap().insert(key, offsets);
                if let Some(ll) = babyl_labels {
                    st.labels.insert(key, ll);
                }
                st.next_key += 1;
                st.pending_sync = true;
                Ok(key)
            })?;
            Ok(Value::int(assigned))
        }

        fn remove(#[positional_only] self_val: PyValueRef<'_>, key: PyIndex) -> Result<Value> {
            let k = key.expect_i64(FN_NAME, "key")?;
            with_state_mut(self_val.0, |st| {
                st.lookup_key(k)?;
                st.toc.as_mut().unwrap().remove(&k);
                st.labels.remove(&k);
                st.pending = true;
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn __setitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyIndex,
            message: PyValue,
        ) -> Result<Value> {
            let k = key.expect_i64(FN_NAME, "key")?;
            let kind = with_state_mut(self_val.0, |st| {
                st.lookup_key(k)?;
                Ok(st.kind)
            })?;
            let (record, babyl_labels) = match kind {
                SingleFileKind::Mbox => (format_mbox_mmdf_record(_interp, &message.0, true, true)?, None),
                SingleFileKind::Mmdf => (format_mbox_mmdf_record(_interp, &message.0, true, false)?, None),
                SingleFileKind::Babyl => format_babyl_record(_interp, &message.0)?,
            };
            with_state_mut(self_val.0, |st| {
                let offsets = append_record_to_singlefile(st, &record)?;
                st.toc.as_mut().unwrap().insert(k, offsets);
                if let Some(ll) = babyl_labels {
                    st.labels.insert(k, ll);
                }
                st.pending = true;
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn iterkeys(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let keys: Vec<Value> = with_state_mut(self_val.0, |st| {
                st.ensure_toc()?;
                Ok(st
                    .toc
                    .as_ref()
                    .unwrap()
                    .keys()
                    .map(|&k| Value::int(k))
                    .collect())
            })?;
            make_value_iter(_interp, keys)
        }

        fn __contains__(#[positional_only] self_val: PyValueRef<'_>, key: PyValue) -> Result<Value> {
            let Some(k) = key.0.as_int() else {
                return Ok(Value::bool_(false));
            };
            let has = with_state_mut(self_val.0, |st| {
                st.ensure_toc()?;
                Ok(st.toc.as_ref().unwrap().contains_key(&k))
            })?;
            Ok(Value::bool_(has))
        }

        fn __len__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let len = with_state_mut(self_val.0, |st| {
                st.ensure_toc()?;
                Ok(st.toc.as_ref().unwrap().len())
            })?;
            Ok(Value::int(len as i64))
        }

        fn lock(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (already, path) = with_state(self_val.0, |st| Ok((st.locked, st.path.clone())))?;
            if !already {
                create_dotlock(_interp, &path)?;
                with_state_mut(self_val.0, |st| {
                    st.locked = true;
                    Ok(())
                })?;
            }
            Ok(Value::none())
        }

        fn unlock(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (locked, path) = with_state(self_val.0, |st| Ok((st.locked, st.path.clone())))?;
            if locked {
                remove_dotlock(&path);
                with_state_mut(self_val.0, |st| {
                    st.locked = false;
                    Ok(())
                })?;
            }
            Ok(Value::none())
        }

        fn flush(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_state_mut(self_val.0, |st| st.flush_disk(_interp))?;
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let flush_fn = _interp.get_attr(self_val.0, "flush")?;
            let flush_res = call_pos(_interp, flush_fn, &[]);
            let unlock_fn = _interp.get_attr(self_val.0, "unlock")?;
            let _ = call_pos(_interp, unlock_fn, &[]);
            let _ = with_state_mut(self_val.0, |st| {
                st.closed = true;
                Ok(())
            });
            flush_res?;
            Ok(Value::none())
        }
    }

    class _mboxMMDF("_singlefileMailbox") {
        constants {
            "_mangle_from_" => Value::bool_(true),
        }

        fn get_message(#[positional_only] self_val: PyValueRef<'_>, key: PyIndex) -> Result<Value> {
            let k = key.expect_i64(FN_NAME, "key")?;
            let (raw, kind) = with_singlefile_mut(self_val.0, |st| {
                let (start, stop) = st.lookup_key(k)?;
                Ok((st.read_slice(start, stop)?, st.kind))
            })?;
            let (from_line_bytes, rest) = if let Some(nl) = raw.iter().position(|&b| b == b'\n') {
                let fl = raw[..nl].strip_suffix(b"\r").unwrap_or(&raw[..nl]);
                (fl, &raw[nl + 1..])
            } else {
                (raw.as_slice(), &[][..])
            };
            let from_line = String::from_utf8_lossy(from_line_bytes).into_owned();
            let body = normalize_os_to_lf(rest);
            let m = _interp.load_module("mailbox")?;
            let msg_cls_name = if kind == SingleFileKind::Mmdf {
                "MMDFMessage"
            } else {
                "mboxMessage"
            };
            let msg_cls = _interp.get_attr(&m, msg_cls_name)?;
            let msg = call_pos(_interp, msg_cls, &[Value::bytes(body)])?;
            let set_uf = _interp.get_attr(&msg, "set_unixfrom")?;
            call_pos(_interp, set_uf, &[Value::string(&from_line)])?;
            let set_f = _interp.get_attr(&msg, "set_from")?;
            let from_arg = from_line.strip_prefix("From ").unwrap_or(&from_line);
            call_pos(_interp, set_f, &[Value::string(from_arg)])?;
            Ok(msg)
        }

        fn get_string(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
            #[default(false.into())] from_: PyBool,
        ) -> Result<Value> {
            let gb = _interp.get_attr(self_val.0, "get_bytes")?;
            let raw = call_pos(_interp, gb, &[key.0, Value::bool_(from_.0)])?;
            let bl = PyBytesLike::try_from_value_with_interp(_interp, &raw, FN_NAME, "bytes")?;
            let msg = email_from_bytes(_interp, bl.as_bytes())?;
            let as_str = _interp.get_attr(&msg, "as_string")?;
            call_kw(_interp, as_str, &[], &[("unixfrom", Value::bool_(from_.0))])
        }

        fn get_bytes(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyIndex,
            #[default(false.into())] from_: PyBool,
        ) -> Result<Value> {
            let k = key.expect_i64(FN_NAME, "key")?;
            let raw = with_singlefile_mut(self_val.0, |st| {
                let (start, stop) = st.lookup_key(k)?;
                st.read_slice(start, stop)
            })?;
            let slice = if from_.0 {
                raw.as_slice()
            } else if let Some(nl) = raw.iter().position(|&b| b == b'\n') {
                &raw[nl + 1..]
            } else {
                &[]
            };
            Ok(Value::bytes(normalize_os_to_lf(slice)))
        }

        fn get_file(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyIndex,
            #[default(false.into())] from_: PyBool,
        ) -> Result<Value> {
            let k = key.expect_i64(FN_NAME, "key")?;
            let raw = with_singlefile_mut(self_val.0, |st| {
                let (start, stop) = st.lookup_key(k)?;
                st.read_slice(start, stop)
            })?;
            let slice = if from_.0 {
                raw.as_slice()
            } else if let Some(nl) = raw.iter().position(|&b| b == b'\n') {
                &raw[nl + 1..]
            } else {
                &[]
            };
            let len = slice.len() as i64;
            let bio = make_bytes_io(_interp, slice.to_vec())?;
            let m = _interp.load_module("mailbox")?;
            let pf = _interp.get_attr(&m, "_PartialFile")?;
            call_pos(_interp, pf, &[bio, Value::int(0), Value::int(len)])
        }
    }

    class mbox("_mboxMMDF") {
        constants {
            "_mangle_from_" => Value::bool_(true),
            "_append_newline" => Value::bool_(true),
        }
    }

    class MMDF("_mboxMMDF") {}

    class Babyl("_singlefileMailbox") {
        fn get_message(#[positional_only] self_val: PyValueRef<'_>, key: PyIndex) -> Result<Value> {
            let k = key.expect_i64(FN_NAME, "key")?;
            let (raw, labels_opt) = with_singlefile_mut(self_val.0, |st| {
                let (start, stop) = st.lookup_key(k)?;
                let data = st.read_slice(start, stop)?;
                let labels = st.labels.get(&k).cloned();
                Ok((data, labels))
            })?;
            let norm = normalize_os_to_lf(&raw);
            let lines = split_lines_with_offsets(&norm);
            let mut idx = 1usize; // skip '1,...' line
            let mut orig_hdrs = Vec::new();
            while idx < lines.len() {
                let line = lines[idx].1;
                idx += 1;
                if line == b"*** EOOH ***\n" || line == b"*** EOOH ***" {
                    break;
                }
                orig_hdrs.extend_from_slice(line);
            }
            let mut vis_hdrs = Vec::new();
            while idx < lines.len() {
                let line = lines[idx].1;
                idx += 1;
                if line == b"\n" {
                    break;
                }
                vis_hdrs.extend_from_slice(line);
            }
            let mut body = Vec::new();
            while idx < lines.len() {
                body.extend_from_slice(lines[idx].1);
                idx += 1;
            }
            let mut full = orig_hdrs;
            if !full.ends_with(b"\n\n") && !full.is_empty() {
                if !full.ends_with(b"\n") {
                    full.push(b'\n');
                }
                full.push(b'\n');
            }
            full.extend_from_slice(&body);
            let m = _interp.load_module("mailbox")?;
            let bm_cls = _interp.get_attr(&m, "BabylMessage")?;
            let msg = call_pos(_interp, bm_cls, &[Value::bytes(full)])?;
            let set_vis = _interp.get_attr(&msg, "set_visible")?;
            call_pos(_interp, set_vis, &[Value::bytes(vis_hdrs)])?;
            if let Some(ll) = labels_opt {
                let set_l = _interp.get_attr(&msg, "set_labels")?;
                call_pos(
                    _interp,
                    set_l,
                    &[Value::list(ll.into_iter().map(Value::string).collect())],
                )?;
            }
            Ok(msg)
        }

        fn get_bytes(#[positional_only] self_val: PyValueRef<'_>, key: PyIndex) -> Result<Value> {
            let k = key.expect_i64(FN_NAME, "key")?;
            let raw = with_singlefile_mut(self_val.0, |st| {
                let (start, stop) = st.lookup_key(k)?;
                st.read_slice(start, stop)
            })?;
            let norm = normalize_os_to_lf(&raw);
            let lines = split_lines_with_offsets(&norm);
            let mut idx = 1usize;
            let mut orig_hdrs = Vec::new();
            while idx < lines.len() {
                let line = lines[idx].1;
                idx += 1;
                if line == b"*** EOOH ***\n" || line == b"*** EOOH ***" {
                    break;
                }
                orig_hdrs.extend_from_slice(line);
            }
            while idx < lines.len() {
                let line = lines[idx].1;
                idx += 1;
                if line == b"\n" {
                    break;
                }
            }
            let mut body = Vec::new();
            while idx < lines.len() {
                body.extend_from_slice(lines[idx].1);
                idx += 1;
            }
            let mut out = orig_hdrs;
            out.extend_from_slice(&body);
            Ok(Value::bytes(out))
        }

        fn get_file(#[positional_only] self_val: PyValueRef<'_>, key: PyValue) -> Result<Value> {
            let gb = _interp.get_attr(self_val.0, "get_bytes")?;
            let raw = call_pos(_interp, gb, &[key.0])?;
            let bl = PyBytesLike::try_from_value_with_interp(_interp, &raw, FN_NAME, "bytes")?;
            make_bytes_io(_interp, normalize_lf_to_os(bl.as_bytes()))
        }

        fn get_labels(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let labels = with_singlefile_mut(self_val.0, |st| st.user_babyl_labels())?;
            Ok(Value::list(labels.into_iter().map(Value::string).collect()))
        }
    }

    // ── MH ───────────────────────────────────────────────────────────────────

    #[native_state(MhState)]
    class MH("Mailbox") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            path: PyValue,
            #[default(OptionalArg::Omitted)] factory: OptValueRef<'_>,
            #[default(true.into())] create: PyBool,
        ) -> Result<Value> {
            let resolved = resolve_mailbox_path(_interp, &path.0)?;
            let pb = PathBuf::from(&resolved);
            let fac = factory.as_value().cloned().unwrap_or_else(Value::none);
            _interp.assign_attr(self_val.0.clone(), "_path", Value::string(&resolved))?;
            _interp.assign_attr(self_val.0.clone(), "_factory", fac)?;

            if !pb.exists() {
                if create.0 {
                    fs::create_dir(&pb)
                        .map_err(|e| PyError::from_io_error(&e, Some(&resolved)))?;
                    let seq = pb.join(".mh_sequences");
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&seq)
                        .map_err(|e| PyError::from_io_error(&e, Some(&seq.to_string_lossy())))?;
                } else {
                    return Err(raise_mb_error(_interp, "NoSuchMailboxError", &resolved));
                }
            }

            set_state(
                self_val.0,
                MhState {
                    path: pb,
                    locked: false,
                },
            )?;
            Ok(Value::none())
        }

        fn add(#[positional_only] self_val: PyValueRef<'_>, message: PyValue) -> Result<Value> {
            let (base, new_key) = with_state(self_val.0, |st| {
                let keys = st.list_numeric_keys()?;
                let nk = keys.last().map(|&k| k + 1).unwrap_or(1);
                Ok((st.path.clone(), nk))
            })?;
            let data = serialize_message_to_bytes(_interp, &message.0, false, false)?;
            let new_path = base.join(new_key.to_string());
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&new_path)
                .map_err(|e| PyError::from_io_error(&e, Some(&new_path.to_string_lossy())))?;
            if let Err(e) = f.write_all(&data).and_then(|_| f.sync_all()) {
                drop(f);
                let _ = fs::remove_file(&new_path);
                return Err(PyError::from_io_error(&e, Some(&new_path.to_string_lossy())));
            }
            if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "MHMessage") {
                let gs = _interp.get_attr(&message.0, "get_sequences")?;
                let seqs_v = call_pos(_interp, gs, &[])?;
                let pending: Vec<String> = _interp
                    .collect_iterable(&seqs_v)?
                    .into_iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
                with_state(self_val.0, |st| {
                    let mut all_seq = st.read_sequences(_interp)?;
                    for (name, key_list) in &mut all_seq {
                        if pending.contains(name) {
                            key_list.push(new_key);
                        } else {
                            key_list.retain(|&x| x != new_key);
                        }
                    }
                    for seq in pending {
                        all_seq.entry(seq).or_insert_with(|| vec![new_key]);
                    }
                    st.write_sequences(&all_seq)
                })?;
            }
            Ok(Value::int(new_key))
        }

        fn remove(#[positional_only] self_val: PyValueRef<'_>, key: PyIndex) -> Result<Value> {
            let k = key.expect_i64(FN_NAME, "key")?;
            let base = with_state(self_val.0, |st| Ok(st.path.clone()))?;
            let p = base.join(k.to_string());
            if !p.exists() {
                return Err(PyError::named("KeyError", format!("No message with key: {k}")));
            }
            fs::remove_file(&p)
                .map_err(|e| PyError::from_io_error(&e, Some(&p.to_string_lossy())))?;
            Ok(Value::none())
        }

        fn __setitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyIndex,
            message: PyValue,
        ) -> Result<Value> {
            let k = key.expect_i64(FN_NAME, "key")?;
            let base = with_state(self_val.0, |st| Ok(st.path.clone()))?;
            let p = base.join(k.to_string());
            if !p.exists() {
                return Err(PyError::named("KeyError", format!("No message with key: {k}")));
            }
            let data = serialize_message_to_bytes(_interp, &message.0, false, false)?;
            fs::write(&p, &data)
                .map_err(|e| PyError::from_io_error(&e, Some(&p.to_string_lossy())))?;
            if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "MHMessage") {
                let gs = _interp.get_attr(&message.0, "get_sequences")?;
                let seqs_v = call_pos(_interp, gs, &[])?;
                let pending: Vec<String> = _interp
                    .collect_iterable(&seqs_v)?
                    .into_iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
                with_state(self_val.0, |st| {
                    let mut all_seq = st.read_sequences(_interp)?;
                    for (name, key_list) in &mut all_seq {
                        if pending.contains(name) {
                            if !key_list.contains(&k) {
                                key_list.push(k);
                            }
                        } else {
                            key_list.retain(|&x| x != k);
                        }
                    }
                    for seq in pending {
                        all_seq.entry(seq).or_insert_with(|| vec![k]);
                    }
                    st.write_sequences(&all_seq)
                })?;
            }
            Ok(Value::none())
        }

        fn get_message(#[positional_only] self_val: PyValueRef<'_>, key: PyIndex) -> Result<Value> {
            let k = key.expect_i64(FN_NAME, "key")?;
            let base = with_state(self_val.0, |st| Ok(st.path.clone()))?;
            let p = base.join(k.to_string());
            if !p.exists() {
                return Err(PyError::named("KeyError", format!("No message with key: {k}")));
            }
            let raw = fs::read(&p)
                .map_err(|e| PyError::from_io_error(&e, Some(&p.to_string_lossy())))?;
            let bio = make_bytes_io(_interp, raw)?;
            let m = _interp.load_module("mailbox")?;
            let cls = _interp.get_attr(&m, "MHMessage")?;
            let msg = call_pos(_interp, cls, &[bio])?;
            let seqs = with_state(self_val.0, |st| st.read_sequences(_interp))?;
            let add_seq = _interp.get_attr(&msg, "add_sequence")?;
            for (name, key_list) in seqs {
                if key_list.contains(&k) {
                    call_pos(_interp, add_seq.clone(), &[Value::string(name)])?;
                }
            }
            Ok(msg)
        }

        fn get_bytes(#[positional_only] self_val: PyValueRef<'_>, key: PyIndex) -> Result<Value> {
            let k = key.expect_i64(FN_NAME, "key")?;
            let base = with_state(self_val.0, |st| Ok(st.path.clone()))?;
            let p = base.join(k.to_string());
            if !p.exists() {
                return Err(PyError::named("KeyError", format!("No message with key: {k}")));
            }
            let raw = fs::read(&p)
                .map_err(|e| PyError::from_io_error(&e, Some(&p.to_string_lossy())))?;
            Ok(Value::bytes(normalize_os_to_lf(&raw)))
        }

        fn get_file(#[positional_only] self_val: PyValueRef<'_>, key: PyIndex) -> Result<Value> {
            let k = key.expect_i64(FN_NAME, "key")?;
            let base = with_state(self_val.0, |st| Ok(st.path.clone()))?;
            let p = base.join(k.to_string());
            if !p.exists() {
                return Err(PyError::named("KeyError", format!("No message with key: {k}")));
            }
            let raw = fs::read(&p)
                .map_err(|e| PyError::from_io_error(&e, Some(&p.to_string_lossy())))?;
            let bio = make_bytes_io(_interp, raw)?;
            let m = _interp.load_module("mailbox")?;
            let pf = _interp.get_attr(&m, "_ProxyFile")?;
            call_pos(_interp, pf, &[bio])
        }

        fn iterkeys(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let keys: Vec<Value> = with_state(self_val.0, |st| {
                Ok(st.list_numeric_keys()?.into_iter().map(Value::int).collect())
            })?;
            make_value_iter(_interp, keys)
        }

        fn __contains__(#[positional_only] self_val: PyValueRef<'_>, key: PyValue) -> Result<Value> {
            let Some(k) = key.0.as_int() else {
                return Ok(Value::bool_(false));
            };
            let base = with_state(self_val.0, |st| Ok(st.path.clone()))?;
            Ok(Value::bool_(base.join(k.to_string()).exists()))
        }

        fn __len__(#[state] st: &MhState) -> Result<Value> {
            Ok(Value::int(st.list_numeric_keys()?.len() as i64))
        }

        fn lock(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (locked, seq_path) =
                with_state(self_val.0, |st| Ok((st.locked, st.path.join(".mh_sequences"))))?;
            if !locked {
                create_dotlock(_interp, &seq_path)?;
                with_state_mut(self_val.0, |st| {
                    st.locked = true;
                    Ok(())
                })?;
            }
            Ok(Value::none())
        }

        fn unlock(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (locked, seq_path) =
                with_state(self_val.0, |st| Ok((st.locked, st.path.join(".mh_sequences"))))?;
            if locked {
                remove_dotlock(&seq_path);
                with_state_mut(self_val.0, |st| {
                    st.locked = false;
                    Ok(())
                })?;
            }
            Ok(Value::none())
        }

        fn flush(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let unlock_fn = _interp.get_attr(self_val.0, "unlock")?;
            call_pos(_interp, unlock_fn, &[])?;
            Ok(Value::none())
        }

        fn list_folders(#[state] st: &MhState) -> Result<Value> {
            let mut out = Vec::new();
            let entries = fs::read_dir(&st.path)
                .map_err(|e| PyError::from_io_error(&e, Some(&st.path.to_string_lossy())))?;
            for entry in entries.flatten() {
                if entry.path().is_dir()
                    && let Some(name) = entry.file_name().to_str()
                {
                    out.push(Value::string(name));
                }
            }
            Ok(Value::list(out))
        }

        fn get_folder(#[positional_only] self_val: PyValueRef<'_>, folder: PyStr<'_>) -> Result<Value> {
            let base = with_state(self_val.0, |st| Ok(st.path.clone()))?;
            let fac = _interp.get_attr(self_val.0, "_factory")?;
            let m = _interp.load_module("mailbox")?;
            let cls = _interp.get_attr(&m, "MH")?;
            call_kw(
                _interp,
                cls,
                &[Value::string(base.join(folder.0.as_ref()).to_string_lossy().as_ref())],
                &[("factory", fac), ("create", Value::bool_(false))],
            )
        }

        fn add_folder(#[positional_only] self_val: PyValueRef<'_>, folder: PyStr<'_>) -> Result<Value> {
            let base = with_state(self_val.0, |st| Ok(st.path.clone()))?;
            let fac = _interp.get_attr(self_val.0, "_factory")?;
            let m = _interp.load_module("mailbox")?;
            let cls = _interp.get_attr(&m, "MH")?;
            call_kw(
                _interp,
                cls,
                &[Value::string(base.join(folder.0.as_ref()).to_string_lossy().as_ref())],
                &[("factory", fac)],
            )
        }

        fn remove_folder(
            #[positional_only] self_val: PyValueRef<'_>,
            folder: PyStr<'_>,
        ) -> Result<Value> {
            let base = with_state(self_val.0, |st| Ok(st.path.clone()))?;
            let p = base.join(folder.0.as_ref());
            let entries: Vec<String> = fs::read_dir(&p)
                .map_err(|e| PyError::from_io_error(&e, Some(&p.to_string_lossy())))?
                .flatten()
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .collect();
            if entries.len() == 1 && entries[0] == ".mh_sequences" {
                let _ = fs::remove_file(p.join(".mh_sequences"));
            } else if !entries.is_empty() {
                return Err(raise_mb_error(
                    _interp,
                    "NotEmptyError",
                    &format!("Folder not empty: {}", base.display()),
                ));
            }
            fs::remove_dir(&p)
                .map_err(|e| PyError::from_io_error(&e, Some(&p.to_string_lossy())))?;
            Ok(Value::none())
        }

        fn get_sequences(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let seqs = with_state(self_val.0, |st| st.read_sequences(_interp))?;
            let mut d = PyDict::default();
            for (name, keys) in seqs {
                d.insert(
                    PyKey::str_from(&name),
                    Value::list(keys.into_iter().map(Value::int).collect()),
                );
            }
            Ok(Value::dict(d))
        }

        fn set_sequences(
            #[positional_only] self_val: PyValueRef<'_>,
            sequences: PyValue,
        ) -> Result<Value> {
            let items_fn = _interp.get_attr(&sequences.0, "items")?;
            let items_val = call_pos(_interp, items_fn, &[])?;
            let pairs = _interp.collect_iterable(&items_val)?;
            let mut map: IndexMap<String, Vec<i64>> = IndexMap::new();
            for pair in pairs {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() >= 2
                    && let Some(name) = kv[0].as_str()
                {
                    let keys: Vec<i64> = _interp
                        .collect_iterable(&kv[1])?
                        .into_iter()
                        .filter_map(|v| v.as_int())
                        .collect();
                    map.insert(name.to_string(), keys);
                }
            }
            with_state(self_val.0, |st| st.write_sequences(&map))?;
            Ok(Value::none())
        }

        fn pack(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_state(self_val.0, |st| {
                let mut seqs = st.read_sequences(_interp)?;
                let keys = st.list_numeric_keys()?;
                let mut changes: Vec<(i64, i64)> = Vec::new();
                for (prev_idx, key) in keys.into_iter().enumerate() {
                    let prev = prev_idx as i64;
                    if key - 1 != prev {
                        let new_k = prev + 1;
                        changes.push((key, new_k));
                        let old_p = st.path.join(key.to_string());
                        let new_p = st.path.join(new_k.to_string());
                        fs::rename(&old_p, &new_p)
                            .map_err(|e| PyError::from_io_error(&e, Some(&new_p.to_string_lossy())))?;
                    }
                }
                if !changes.is_empty() {
                    for key_list in seqs.values_mut() {
                        for &(old, new) in &changes {
                            if let Some(slot) = key_list.iter_mut().find(|x| **x == old) {
                                *slot = new;
                            }
                        }
                    }
                    st.write_sequences(&seqs)?;
                }
                Ok(())
            })?;
            Ok(Value::none())
        }
    }

    // ── Message classes (all inherit email.message.Message) ──────────────────

    class Message {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] message: OptValueRef<'_>,
        ) -> Result<Value> {
            init_mailbox_message(_interp, self_val.0, message.as_value())?;
            Ok(Value::none())
        }

        fn _become_message(
            #[positional_only] self_val: PyValueRef<'_>,
            message: PyValue,
        ) -> Result<Value> {
            copy_non_specific_attrs(_interp, self_val.0, &message.0)?;
            Ok(Value::none())
        }

        fn _explain_to(
            #[positional_only] _self_val: PyValueRef<'_>,
            message: PyValue,
        ) -> Result<Value> {
            if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "Message") {
                Ok(Value::none())
            } else {
                Err(PyError::named(
                    "TypeError",
                    "Cannot convert to specified type".to_string(),
                ))
            }
        }
    }

    class MaildirMessage("Message") {
        constants {
            "_type_specific_attributes" => Value::list(vec![
                Value::string("_subdir"),
                Value::string("_info"),
                Value::string("_date"),
            ]),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] message: OptValueRef<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_subdir", Value::string("new"))?;
            _interp.assign_attr(self_val.0.clone(), "_info", Value::string(""))?;
            _interp.assign_attr(self_val.0.clone(), "_date", Value::float(now_secs_f64()))?;
            init_mailbox_message(_interp, self_val.0, message.as_value())?;
            Ok(Value::none())
        }

        fn get_subdir(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_subdir")
        }

        fn set_subdir(
            #[positional_only] self_val: PyValueRef<'_>,
            subdir: PyStr<'_>,
        ) -> Result<Value> {
            let s = subdir.0.as_ref();
            if s == "new" || s == "cur" {
                _interp.assign_attr(self_val.0.clone(), "_subdir", Value::string(s))?;
                Ok(Value::none())
            } else {
                Err(PyError::named(
                    "ValueError",
                    format!("subdir must be 'new' or 'cur': {s}"),
                ))
            }
        }

        fn get_flags(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let info = _interp.get_attr(self_val.0, "_info")?;
            let s = info.as_str().unwrap_or("");
            if let Some(rest) = s.strip_prefix("2,") {
                Ok(Value::string(rest))
            } else {
                Ok(Value::string(""))
            }
        }

        fn set_flags(
            #[positional_only] self_val: PyValueRef<'_>,
            flags: PyStr<'_>,
        ) -> Result<Value> {
            let mut chars: Vec<char> = flags.0.chars().collect();
            chars.sort_unstable();
            let sorted: String = chars.into_iter().collect();
            _interp.assign_attr(
                self_val.0.clone(),
                "_info",
                Value::string(format!("2,{sorted}")),
            )?;
            Ok(Value::none())
        }

        fn add_flag(
            #[positional_only] self_val: PyValueRef<'_>,
            flag: PyStr<'_>,
        ) -> Result<Value> {
            let gf = _interp.get_attr(self_val.0, "get_flags")?;
            let cur = call_pos(_interp, gf, &[])?;
            let mut set: BTreeSet<char> = cur.as_str().unwrap_or("").chars().collect();
            for ch in flag.0.chars() {
                set.insert(ch);
            }
            let combined: String = set.into_iter().collect();
            let sf = _interp.get_attr(self_val.0, "set_flags")?;
            call_pos(_interp, sf, &[Value::string(combined)])
        }

        fn remove_flag(
            #[positional_only] self_val: PyValueRef<'_>,
            flag: PyStr<'_>,
        ) -> Result<Value> {
            let gf = _interp.get_attr(self_val.0, "get_flags")?;
            let cur = call_pos(_interp, gf, &[])?;
            let cur_s = cur.as_str().unwrap_or("");
            if !cur_s.is_empty() {
                let mut set: BTreeSet<char> = cur_s.chars().collect();
                for ch in flag.0.chars() {
                    set.remove(&ch);
                }
                let combined: String = set.into_iter().collect();
                let sf = _interp.get_attr(self_val.0, "set_flags")?;
                call_pos(_interp, sf, &[Value::string(combined)])?;
            }
            Ok(Value::none())
        }

        fn get_date(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_date")
        }

        fn set_date(#[positional_only] self_val: PyValueRef<'_>, date: PyValue) -> Result<Value> {
            let float_cls = Value::py_class(
                crate::interpreter::primitive_class_by_name("float")
                    .ok_or_else(|| PyError::runtime("float class not found".to_string()))?,
            );
            let fval = match call_pos(_interp, float_cls, std::slice::from_ref(&date.0)) {
                Ok(v) => v,
                Err(e) if e.class_name_is("ValueError") => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "can't convert to float: {}",
                            super::email_errors::py_str(_interp, &date.0)?
                        ),
                    ));
                }
                Err(e) => return Err(e),
            };
            _interp.assign_attr(self_val.0.clone(), "_date", fval)?;
            Ok(Value::none())
        }

        fn get_info(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_info")
        }

        fn set_info(#[positional_only] self_val: PyValueRef<'_>, info: PyValue) -> Result<Value> {
            if info.0.as_str().is_none() {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "info must be a string: {}",
                        crate::interpreter::value_type_name_str(&info.0)
                    ),
                ));
            }
            _interp.assign_attr(self_val.0.clone(), "_info", info.0)?;
            Ok(Value::none())
        }

        fn _explain_to(
            #[positional_only] self_val: PyValueRef<'_>,
            message: PyValue,
        ) -> Result<Value> {
            let gf = _interp.get_attr(self_val.0, "get_flags")?;
            let flags_s = call_pos(_interp, gf, &[])?
                .as_str()
                .unwrap_or("")
                .to_string();
            let flags: HashSet<char> = flags_s.chars().collect();
            let gs = _interp.get_attr(self_val.0, "get_subdir")?;
            let subdir = call_pos(_interp, gs, &[])?
                .as_str()
                .unwrap_or("new")
                .to_string();
            let gd = _interp.get_attr(self_val.0, "get_date")?;
            let date_v = call_pos(_interp, gd, &[])?;

            if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "MaildirMessage") {
                let sf = _interp.get_attr(&message.0, "set_flags")?;
                call_pos(_interp, sf, &[Value::string(&flags_s)])?;
                let ss = _interp.get_attr(&message.0, "set_subdir")?;
                call_pos(_interp, ss, &[Value::string(&subdir)])?;
                let sd = _interp.get_attr(&message.0, "set_date")?;
                call_pos(_interp, sd, &[date_v])?;
            } else if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "_mboxMMDFMessage") {
                let af = _interp.get_attr(&message.0, "add_flag")?;
                if flags.contains(&'S') {
                    call_pos(_interp, af.clone(), &[Value::string("R")])?;
                }
                if subdir == "cur" {
                    call_pos(_interp, af.clone(), &[Value::string("O")])?;
                }
                if flags.contains(&'T') {
                    call_pos(_interp, af.clone(), &[Value::string("D")])?;
                }
                if flags.contains(&'F') {
                    call_pos(_interp, af.clone(), &[Value::string("F")])?;
                }
                if flags.contains(&'R') {
                    call_pos(_interp, af, &[Value::string("A")])?;
                }
                let time_mod = _interp.load_module("time")?;
                let gmtime = _interp.get_attr(&time_mod, "gmtime")?;
                let gm = call_pos(_interp, gmtime, &[date_v])?;
                let set_from = _interp.get_attr(&message.0, "set_from")?;
                call_pos(_interp, set_from, &[Value::string("MAILER-DAEMON"), gm])?;
            } else if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "MHMessage") {
                let add_seq = _interp.get_attr(&message.0, "add_sequence")?;
                if !flags.contains(&'S') {
                    call_pos(_interp, add_seq.clone(), &[Value::string("unseen")])?;
                }
                if flags.contains(&'R') {
                    call_pos(_interp, add_seq.clone(), &[Value::string("replied")])?;
                }
                if flags.contains(&'F') {
                    call_pos(_interp, add_seq, &[Value::string("flagged")])?;
                }
            } else if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "BabylMessage") {
                let add_lbl = _interp.get_attr(&message.0, "add_label")?;
                if !flags.contains(&'S') {
                    call_pos(_interp, add_lbl.clone(), &[Value::string("unseen")])?;
                }
                if flags.contains(&'T') {
                    call_pos(_interp, add_lbl.clone(), &[Value::string("deleted")])?;
                }
                if flags.contains(&'R') {
                    call_pos(_interp, add_lbl.clone(), &[Value::string("answered")])?;
                }
                if flags.contains(&'P') {
                    call_pos(_interp, add_lbl, &[Value::string("forwarded")])?;
                }
            } else if !is_instance_of_mod_cls(_interp, &message.0, "mailbox", "Message") {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "Cannot convert to specified type: {}",
                        crate::interpreter::value_type_name_str(&message.0)
                    ),
                ));
            }
            Ok(Value::none())
        }
    }

    class _mboxMMDFMessage("Message") {
        constants {
            "_type_specific_attributes" => Value::list(vec![Value::string("_from")]),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] message: OptValueRef<'_>,
        ) -> Result<Value> {
            let set_from = _interp.get_attr(self_val.0, "set_from")?;
            call_pos(
                _interp,
                set_from.clone(),
                &[Value::string("MAILER-DAEMON"), Value::bool_(true)],
            )?;
            if let Some(m) = message.as_value()
                && is_instance_of_mod_cls(_interp, m, "email.message", "Message")
            {
                let guf = _interp.get_attr(m, "get_unixfrom")?;
                let uf = call_pos(_interp, guf, &[])?;
                if let Some(s) = uf.as_str()
                    && let Some(rest) = s.strip_prefix("From ")
                {
                    call_pos(_interp, set_from, &[Value::string(rest)])?;
                }
            }
            init_mailbox_message(_interp, self_val.0, message.as_value())?;
            Ok(Value::none())
        }

        fn get_from(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_from")
        }

        fn set_from(
            #[positional_only] self_val: PyValueRef<'_>,
            from_: PyStr<'_>,
            #[default(OptionalArg::Omitted)] time_: OptValueRef<'_>,
        ) -> Result<Value> {
            let mut val = from_.0.to_string();
            if let Some(t) = time_.as_value()
                && !t.is_none()
            {
                let tt = if matches!(t.kind(), ValueKind::Bool(true)) {
                    let time_mod = _interp.load_module("time")?;
                    let gmtime = _interp.get_attr(&time_mod, "gmtime")?;
                    call_pos(_interp, gmtime, &[])?
                } else {
                    t.clone()
                };
                let ts = format_time_tuple_asctime(_interp, &tt)?;
                val.push(' ');
                val.push_str(&ts);
            }
            _interp.assign_attr(self_val.0.clone(), "_from", Value::string(val))?;
            Ok(Value::none())
        }

        fn get_flags(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let get_fn = _interp.get_attr(self_val.0, "get")?;
            let s1 = call_pos(_interp, get_fn.clone(), &[Value::string("Status"), Value::string("")])?;
            let s2 = call_pos(_interp, get_fn, &[Value::string("X-Status"), Value::string("")])?;
            Ok(Value::string(format!(
                "{}{}",
                s1.as_str().unwrap_or(""),
                s2.as_str().unwrap_or("")
            )))
        }

        fn set_flags(
            #[positional_only] self_val: PyValueRef<'_>,
            flags: PyStr<'_>,
        ) -> Result<Value> {
            let mut set: BTreeSet<char> = flags.0.chars().collect();
            let mut status_flags = String::new();
            for ch in ['R', 'O'] {
                if set.remove(&ch) {
                    status_flags.push(ch);
                }
            }
            let mut xstatus_flags = String::new();
            for ch in ['D', 'F', 'A'] {
                if set.remove(&ch) {
                    xstatus_flags.push(ch);
                }
            }
            for ch in set {
                xstatus_flags.push(ch);
            }
            let rep = _interp.get_attr(self_val.0, "replace_header")?;
            let add = _interp.get_attr(self_val.0, "add_header")?;
            if call_pos(
                _interp,
                rep.clone(),
                &[Value::string("Status"), Value::string(&status_flags)],
            )
            .is_err()
            {
                call_pos(
                    _interp,
                    add.clone(),
                    &[Value::string("Status"), Value::string(&status_flags)],
                )?;
            }
            if call_pos(
                _interp,
                rep,
                &[Value::string("X-Status"), Value::string(&xstatus_flags)],
            )
            .is_err()
            {
                call_pos(
                    _interp,
                    add,
                    &[Value::string("X-Status"), Value::string(&xstatus_flags)],
                )?;
            }
            Ok(Value::none())
        }

        fn add_flag(
            #[positional_only] self_val: PyValueRef<'_>,
            flag: PyStr<'_>,
        ) -> Result<Value> {
            let gf = _interp.get_attr(self_val.0, "get_flags")?;
            let cur = call_pos(_interp, gf, &[])?;
            let mut set: BTreeSet<char> = cur.as_str().unwrap_or("").chars().collect();
            for ch in flag.0.chars() {
                set.insert(ch);
            }
            let combined: String = set.into_iter().collect();
            let sf = _interp.get_attr(self_val.0, "set_flags")?;
            call_pos(_interp, sf, &[Value::string(combined)])
        }

        fn remove_flag(
            #[positional_only] self_val: PyValueRef<'_>,
            flag: PyStr<'_>,
        ) -> Result<Value> {
            let contains = _interp.get_attr(self_val.0, "__contains__")?;
            let has_s_v = call_pos(_interp, contains.clone(), &[Value::string("Status")])?;
            let has_s = _interp.truthy_value(&has_s_v)?;
            let has_xs_v = call_pos(_interp, contains, &[Value::string("X-Status")])?;
            let has_xs = _interp.truthy_value(&has_xs_v)?;
            if has_s || has_xs {
                let gf = _interp.get_attr(self_val.0, "get_flags")?;
                let cur = call_pos(_interp, gf, &[])?;
                let mut set: BTreeSet<char> = cur.as_str().unwrap_or("").chars().collect();
                for ch in flag.0.chars() {
                    set.remove(&ch);
                }
                let combined: String = set.into_iter().collect();
                let sf = _interp.get_attr(self_val.0, "set_flags")?;
                call_pos(_interp, sf, &[Value::string(combined)])?;
            }
            Ok(Value::none())
        }

        fn _explain_to(
            #[positional_only] self_val: PyValueRef<'_>,
            message: PyValue,
        ) -> Result<Value> {
            let gf = _interp.get_attr(self_val.0, "get_flags")?;
            let flags_s = call_pos(_interp, gf, &[])?
                .as_str()
                .unwrap_or("")
                .to_string();
            let flags: HashSet<char> = flags_s.chars().collect();
            let get_from = _interp.get_attr(self_val.0, "get_from")?;
            let from_s = call_pos(_interp, get_from, &[])?
                .as_str()
                .unwrap_or("")
                .to_string();

            if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "MaildirMessage") {
                if flags.contains(&'O') {
                    let ss = _interp.get_attr(&message.0, "set_subdir")?;
                    call_pos(_interp, ss, &[Value::string("cur")])?;
                }
                let af = _interp.get_attr(&message.0, "add_flag")?;
                if flags.contains(&'F') {
                    call_pos(_interp, af.clone(), &[Value::string("F")])?;
                }
                if flags.contains(&'A') {
                    call_pos(_interp, af.clone(), &[Value::string("R")])?;
                }
                if flags.contains(&'R') {
                    call_pos(_interp, af.clone(), &[Value::string("S")])?;
                }
                if flags.contains(&'D') {
                    call_pos(_interp, af, &[Value::string("T")])?;
                }
                let delitem = _interp.get_attr(&message.0, "__delitem__")?;
                let _ = call_pos(_interp, delitem.clone(), &[Value::string("status")]);
                let _ = call_pos(_interp, delitem, &[Value::string("x-status")]);
            } else if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "_mboxMMDFMessage") {
                let sf = _interp.get_attr(&message.0, "set_flags")?;
                call_pos(_interp, sf, &[Value::string(&flags_s)])?;
                let sfrom = _interp.get_attr(&message.0, "set_from")?;
                call_pos(_interp, sfrom, &[Value::string(&from_s)])?;
            } else if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "MHMessage") {
                let add_seq = _interp.get_attr(&message.0, "add_sequence")?;
                if !flags.contains(&'R') {
                    call_pos(_interp, add_seq.clone(), &[Value::string("unseen")])?;
                }
                if flags.contains(&'A') {
                    call_pos(_interp, add_seq.clone(), &[Value::string("replied")])?;
                }
                if flags.contains(&'F') {
                    call_pos(_interp, add_seq, &[Value::string("flagged")])?;
                }
                let delitem = _interp.get_attr(&message.0, "__delitem__")?;
                let _ = call_pos(_interp, delitem.clone(), &[Value::string("status")]);
                let _ = call_pos(_interp, delitem, &[Value::string("x-status")]);
            } else if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "BabylMessage") {
                let add_lbl = _interp.get_attr(&message.0, "add_label")?;
                if !flags.contains(&'R') {
                    call_pos(_interp, add_lbl.clone(), &[Value::string("unseen")])?;
                }
                if flags.contains(&'D') {
                    call_pos(_interp, add_lbl.clone(), &[Value::string("deleted")])?;
                }
                if flags.contains(&'A') {
                    call_pos(_interp, add_lbl, &[Value::string("answered")])?;
                }
                let delitem = _interp.get_attr(&message.0, "__delitem__")?;
                let _ = call_pos(_interp, delitem.clone(), &[Value::string("status")]);
                let _ = call_pos(_interp, delitem, &[Value::string("x-status")]);
            } else if !is_instance_of_mod_cls(_interp, &message.0, "mailbox", "Message") {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "Cannot convert to specified type: {}",
                        crate::interpreter::value_type_name_str(&message.0)
                    ),
                ));
            }
            Ok(Value::none())
        }
    }

    class mboxMessage("_mboxMMDFMessage") {}
    class MMDFMessage("_mboxMMDFMessage") {}

    class MHMessage("Message") {
        constants {
            "_type_specific_attributes" => Value::list(vec![Value::string("_sequences")]),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] message: OptValueRef<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_sequences", Value::list(Vec::new()))?;
            init_mailbox_message(_interp, self_val.0, message.as_value())?;
            Ok(Value::none())
        }

        fn get_sequences(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let seqs = _interp.get_attr(self_val.0, "_sequences")?;
            Ok(Value::list(_interp.collect_iterable(&seqs)?))
        }

        fn set_sequences(
            #[positional_only] self_val: PyValueRef<'_>,
            sequences: PyValue,
        ) -> Result<Value> {
            let items = _interp.collect_iterable(&sequences.0)?;
            _interp.assign_attr(self_val.0.clone(), "_sequences", Value::list(items))?;
            Ok(Value::none())
        }

        fn add_sequence(
            #[positional_only] self_val: PyValueRef<'_>,
            sequence: PyValue,
        ) -> Result<Value> {
            if sequence.0.as_str().is_none() {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "sequence type must be str: {}",
                        crate::interpreter::value_type_name_str(&sequence.0)
                    ),
                ));
            }
            let seqs = _interp.get_attr(self_val.0, "_sequences")?;
            let items = _interp.collect_iterable(&seqs)?;
            if !items.iter().any(|x| x.as_str() == sequence.0.as_str()) {
                let app = _interp.get_attr(&seqs, "append")?;
                call_pos(_interp, app, &[sequence.0])?;
            }
            Ok(Value::none())
        }

        fn remove_sequence(
            #[positional_only] self_val: PyValueRef<'_>,
            sequence: PyValue,
        ) -> Result<Value> {
            let seqs = _interp.get_attr(self_val.0, "_sequences")?;
            let rem = _interp.get_attr(&seqs, "remove")?;
            let _ = call_pos(_interp, rem, &[sequence.0]);
            Ok(Value::none())
        }

        fn _explain_to(
            #[positional_only] self_val: PyValueRef<'_>,
            message: PyValue,
        ) -> Result<Value> {
            let gs = _interp.get_attr(self_val.0, "get_sequences")?;
            let seqs_v = call_pos(_interp, gs, &[])?;
            let seqs_vec: Vec<String> = _interp
                .collect_iterable(&seqs_v)?
                .into_iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            let seqs: HashSet<&str> = seqs_vec.iter().map(String::as_str).collect();

            if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "MaildirMessage") {
                let ss = _interp.get_attr(&message.0, "set_subdir")?;
                call_pos(_interp, ss, &[Value::string("cur")])?;
                let af = _interp.get_attr(&message.0, "add_flag")?;
                if !seqs.contains("unseen") {
                    call_pos(_interp, af.clone(), &[Value::string("S")])?;
                }
                if seqs.contains("flagged") {
                    call_pos(_interp, af.clone(), &[Value::string("F")])?;
                }
                if seqs.contains("replied") {
                    call_pos(_interp, af, &[Value::string("R")])?;
                }
            } else if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "_mboxMMDFMessage") {
                let af = _interp.get_attr(&message.0, "add_flag")?;
                if !seqs.contains("unseen") {
                    call_pos(_interp, af.clone(), &[Value::string("RO")])?;
                } else {
                    call_pos(_interp, af.clone(), &[Value::string("O")])?;
                }
                if seqs.contains("flagged") {
                    call_pos(_interp, af.clone(), &[Value::string("F")])?;
                }
                if seqs.contains("replied") {
                    call_pos(_interp, af, &[Value::string("A")])?;
                }
            } else if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "MHMessage") {
                let add_seq = _interp.get_attr(&message.0, "add_sequence")?;
                for s in &seqs_vec {
                    call_pos(_interp, add_seq.clone(), &[Value::string(s)])?;
                }
            } else if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "BabylMessage") {
                let add_lbl = _interp.get_attr(&message.0, "add_label")?;
                if seqs.contains("unseen") {
                    call_pos(_interp, add_lbl.clone(), &[Value::string("unseen")])?;
                }
                if seqs.contains("replied") {
                    call_pos(_interp, add_lbl, &[Value::string("answered")])?;
                }
            } else if !is_instance_of_mod_cls(_interp, &message.0, "mailbox", "Message") {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "Cannot convert to specified type: {}",
                        crate::interpreter::value_type_name_str(&message.0)
                    ),
                ));
            }
            Ok(Value::none())
        }
    }

    class BabylMessage("Message") {
        constants {
            "_type_specific_attributes" => Value::list(vec![
                Value::string("_labels"),
                Value::string("_visible"),
            ]),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] message: OptValueRef<'_>,
        ) -> Result<Value> {
            let m = _interp.load_module("mailbox")?;
            let msg_cls = _interp.get_attr(&m, "Message")?;
            let vis = call_pos(_interp, msg_cls, &[])?;
            _interp.assign_attr(self_val.0.clone(), "_labels", Value::list(Vec::new()))?;
            _interp.assign_attr(self_val.0.clone(), "_visible", vis)?;
            init_mailbox_message(_interp, self_val.0, message.as_value())?;
            Ok(Value::none())
        }

        fn get_labels(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let lbls = _interp.get_attr(self_val.0, "_labels")?;
            Ok(Value::list(_interp.collect_iterable(&lbls)?))
        }

        fn set_labels(
            #[positional_only] self_val: PyValueRef<'_>,
            labels: PyValue,
        ) -> Result<Value> {
            let items = _interp.collect_iterable(&labels.0)?;
            _interp.assign_attr(self_val.0.clone(), "_labels", Value::list(items))?;
            Ok(Value::none())
        }

        fn add_label(
            #[positional_only] self_val: PyValueRef<'_>,
            label: PyValue,
        ) -> Result<Value> {
            if label.0.as_str().is_none() {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "label must be a string: {}",
                        crate::interpreter::value_type_name_str(&label.0)
                    ),
                ));
            }
            let lbls = _interp.get_attr(self_val.0, "_labels")?;
            let items = _interp.collect_iterable(&lbls)?;
            if !items.iter().any(|x| x.as_str() == label.0.as_str()) {
                let app = _interp.get_attr(&lbls, "append")?;
                call_pos(_interp, app, &[label.0])?;
            }
            Ok(Value::none())
        }

        fn remove_label(
            #[positional_only] self_val: PyValueRef<'_>,
            label: PyValue,
        ) -> Result<Value> {
            let lbls = _interp.get_attr(self_val.0, "_labels")?;
            let rem = _interp.get_attr(&lbls, "remove")?;
            let _ = call_pos(_interp, rem, &[label.0]);
            Ok(Value::none())
        }

        fn get_visible(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let vis = _interp.get_attr(self_val.0, "_visible")?;
            let m = _interp.load_module("mailbox")?;
            let msg_cls = _interp.get_attr(&m, "Message")?;
            call_pos(_interp, msg_cls, &[vis])
        }

        fn set_visible(
            #[positional_only] self_val: PyValueRef<'_>,
            visible: PyValue,
        ) -> Result<Value> {
            let m = _interp.load_module("mailbox")?;
            let msg_cls = _interp.get_attr(&m, "Message")?;
            let vis = call_pos(_interp, msg_cls, &[visible.0])?;
            _interp.assign_attr(self_val.0.clone(), "_visible", vis)?;
            Ok(Value::none())
        }

        fn update_visible(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let vis = _interp.get_attr(self_val.0, "_visible")?;
            let vis_keys_fn = _interp.get_attr(&vis, "keys")?;
            let vis_keys_v = call_pos(_interp, vis_keys_fn, &[])?;
            let vis_keys = _interp.collect_iterable(&vis_keys_v)?;
            let self_contains = _interp.get_attr(self_val.0, "__contains__")?;
            let self_getitem = _interp.get_attr(self_val.0, "__getitem__")?;
            let vis_replace = _interp.get_attr(&vis, "replace_header")?;
            let vis_delitem = _interp.get_attr(&vis, "__delitem__")?;
            for h in vis_keys {
                let present_v =
                    call_pos(_interp, self_contains.clone(), std::slice::from_ref(&h))?;
                let present = _interp.truthy_value(&present_v)?;
                if present {
                    let val = call_pos(_interp, self_getitem.clone(), std::slice::from_ref(&h))?;
                    call_pos(_interp, vis_replace.clone(), &[h, val])?;
                } else {
                    call_pos(_interp, vis_delitem.clone(), &[h])?;
                }
            }
            let vis_contains = _interp.get_attr(&vis, "__contains__")?;
            let vis_setitem = _interp.get_attr(&vis, "__setitem__")?;
            for hdr in ["Date", "From", "Reply-To", "To", "CC", "Subject"] {
                let hv = Value::string(hdr);
                let in_self_v =
                    call_pos(_interp, self_contains.clone(), std::slice::from_ref(&hv))?;
                let in_self = _interp.truthy_value(&in_self_v)?;
                let in_vis_v =
                    call_pos(_interp, vis_contains.clone(), std::slice::from_ref(&hv))?;
                let in_vis = _interp.truthy_value(&in_vis_v)?;
                if in_self && !in_vis {
                    let val = call_pos(_interp, self_getitem.clone(), std::slice::from_ref(&hv))?;
                    call_pos(_interp, vis_setitem.clone(), &[hv, val])?;
                }
            }
            Ok(Value::none())
        }

        fn _explain_to(
            #[positional_only] self_val: PyValueRef<'_>,
            message: PyValue,
        ) -> Result<Value> {
            let gl = _interp.get_attr(self_val.0, "get_labels")?;
            let labels_v = call_pos(_interp, gl, &[])?;
            let labels_vec: Vec<String> = _interp
                .collect_iterable(&labels_v)?
                .into_iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            let labels: HashSet<&str> = labels_vec.iter().map(String::as_str).collect();

            if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "MaildirMessage") {
                let ss = _interp.get_attr(&message.0, "set_subdir")?;
                call_pos(_interp, ss, &[Value::string("cur")])?;
                let af = _interp.get_attr(&message.0, "add_flag")?;
                if !labels.contains("unseen") {
                    call_pos(_interp, af.clone(), &[Value::string("S")])?;
                }
                if labels.contains("forwarded") || labels.contains("resent") {
                    call_pos(_interp, af.clone(), &[Value::string("P")])?;
                }
                if labels.contains("answered") {
                    call_pos(_interp, af.clone(), &[Value::string("R")])?;
                }
                if labels.contains("deleted") {
                    call_pos(_interp, af, &[Value::string("T")])?;
                }
            } else if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "_mboxMMDFMessage") {
                let af = _interp.get_attr(&message.0, "add_flag")?;
                if !labels.contains("unseen") {
                    call_pos(_interp, af.clone(), &[Value::string("RO")])?;
                } else {
                    call_pos(_interp, af.clone(), &[Value::string("O")])?;
                }
                if labels.contains("deleted") {
                    call_pos(_interp, af.clone(), &[Value::string("D")])?;
                }
                if labels.contains("answered") {
                    call_pos(_interp, af, &[Value::string("A")])?;
                }
            } else if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "MHMessage") {
                let add_seq = _interp.get_attr(&message.0, "add_sequence")?;
                if labels.contains("unseen") {
                    call_pos(_interp, add_seq.clone(), &[Value::string("unseen")])?;
                }
                if labels.contains("answered") {
                    call_pos(_interp, add_seq, &[Value::string("replied")])?;
                }
            } else if is_instance_of_mod_cls(_interp, &message.0, "mailbox", "BabylMessage") {
                let gv = _interp.get_attr(self_val.0, "get_visible")?;
                let vis = call_pos(_interp, gv, &[])?;
                let sv = _interp.get_attr(&message.0, "set_visible")?;
                call_pos(_interp, sv, &[vis])?;
                let add_lbl = _interp.get_attr(&message.0, "add_label")?;
                for l in &labels_vec {
                    call_pos(_interp, add_lbl.clone(), &[Value::string(l)])?;
                }
            } else if !is_instance_of_mod_cls(_interp, &message.0, "mailbox", "Message") {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "Cannot convert to specified type: {}",
                        crate::interpreter::value_type_name_str(&message.0)
                    ),
                ));
            }
            Ok(Value::none())
        }
    }
}
