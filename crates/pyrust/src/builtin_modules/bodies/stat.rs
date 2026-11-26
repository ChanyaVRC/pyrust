// `stat` module — 100% native Rust implementation of CPython 3.12's `stat`
// (issues #3209, #3355).
//
// CPython's `Lib/stat.py` is a thin Python layer over the C accelerator
// `_stat`; this file provides the combined surface with no Python-source
// injection: the `ST_*` indices, `S_IF*` / permission bit / `UF_*` / `SF_*` /
// `FILE_ATTRIBUTE_*` / `IO_REPARSE_TAG_*` constants, `_filemode_table`, the
// mode helpers (`S_IMODE`, `S_IFMT`, `S_IS*`) and `filemode()`.
//
// The helpers mirror `Modules/_stat.c`: the argument must be an `int` (or
// `bool` / `int` subclass; `__index__` is *not* consulted), it is converted
// the way `PyLong_AsUnsignedLong` + a `mode_t` range check would, so negative
// values and out-of-range values raise `OverflowError` and non-integers raise
// `TypeError`.  `mode_t` is 16 bits wide (and `unsigned long` 32 bits) on
// Windows and 32 bits wide (with a 64-bit `unsigned long`) elsewhere.
//
// Reference: <https://docs.python.org/3.12/library/stat.html>

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::PyValue;
use crate::value::{PyBigIntSign, PyToPrimitive, Value, ValueKind};
use pyrust_derive::pyrust_module;

const S_IFDIR: u64 = 0o040000;
const S_IFCHR: u64 = 0o020000;
const S_IFBLK: u64 = 0o060000;
const S_IFREG: u64 = 0o100000;
const S_IFIFO: u64 = 0o010000;
const S_IFLNK: u64 = 0o120000;
const S_IFSOCK: u64 = 0o140000;
const S_IFMT_MASK: u64 = 0o170000;

const S_ISUID: u64 = 0o4000;
const S_ISGID: u64 = 0o2000;
const S_ISVTX: u64 = 0o1000;
const S_IRUSR: u64 = 0o0400;
const S_IWUSR: u64 = 0o0200;
const S_IXUSR: u64 = 0o0100;
const S_IRGRP: u64 = 0o0040;
const S_IWGRP: u64 = 0o0020;
const S_IXGRP: u64 = 0o0010;
const S_IROTH: u64 = 0o0004;
const S_IWOTH: u64 = 0o0002;
const S_IXOTH: u64 = 0o0001;

/// Largest value that fits the platform's C `unsigned long`.
#[cfg(windows)]
const ULONG_MAX: u64 = u32::MAX as u64;
#[cfg(not(windows))]
const ULONG_MAX: u64 = u64::MAX;

/// Largest value that fits the platform's C `mode_t`.
#[cfg(windows)]
const MODE_MAX: u64 = u16::MAX as u64;
#[cfg(not(windows))]
const MODE_MAX: u64 = u32::MAX as u64;

const STAT_DOC: &str = "Constants/functions for interpreting results of os.stat() and os.lstat().

Suggested usage: from stat import *
";

fn overflow(msg: &str) -> PyError {
    PyError::named("OverflowError", msg.to_string())
}

/// `_PyLong_AsMode_t`: `PyLong_AsUnsignedLong` followed by a `mode_t` range
/// check.
fn mode_arg(arg: &Value) -> Result<u64> {
    let backing;
    let value = match arg.kind() {
        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => arg,
        _ => match crate::interpreter::coerce_subclass_backing(arg, &[]) {
            Some(b)
                if matches!(
                    b.kind(),
                    ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                ) =>
            {
                backing = b;
                &backing
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "an integer is required".to_string(),
                ));
            }
        },
    };
    let n: u64 = match value.kind() {
        ValueKind::Bool(b) => u64::from(b),
        ValueKind::Int(n) => u64::try_from(n)
            .map_err(|_| overflow("can't convert negative value to unsigned int"))?,
        ValueKind::BigInt(b) => {
            if b.sign() == PyBigIntSign::Minus {
                return Err(overflow("can't convert negative value to unsigned int"));
            }
            b.to_u64()
                .ok_or_else(|| overflow("Python int too large to convert to C unsigned long"))?
        }
        _ => unreachable!(),
    };
    if n > ULONG_MAX {
        return Err(overflow(
            "Python int too large to convert to C unsigned long",
        ));
    }
    if n > MODE_MAX {
        return Err(overflow("mode out of range"));
    }
    Ok(n)
}

fn int(n: u64) -> Value {
    Value::int(n as i64)
}

/// Whether the file-type bits of `mode` equal `kind`.
fn is_type(arg: &Value, kind: u64) -> Result<Value> {
    Ok(Value::bool_(mode_arg(arg)? & S_IFMT_MASK == kind))
}

/// `stat.filemode`'s `_filemode_table`, as a tuple of tuples of
/// `(bit, char)` pairs.
fn filemode_table() -> Value {
    let row = |entries: &[(u64, &str)]| {
        Value::tuple(
            entries
                .iter()
                .map(|&(bit, ch)| Value::tuple(vec![int(bit), Value::string(ch)]))
                .collect(),
        )
    };
    Value::tuple(vec![
        row(&[
            (S_IFLNK, "l"),
            (S_IFSOCK, "s"),
            (S_IFREG, "-"),
            (S_IFBLK, "b"),
            (S_IFDIR, "d"),
            (S_IFCHR, "c"),
            (S_IFIFO, "p"),
        ]),
        row(&[(S_IRUSR, "r")]),
        row(&[(S_IWUSR, "w")]),
        row(&[(S_IXUSR | S_ISUID, "s"), (S_ISUID, "S"), (S_IXUSR, "x")]),
        row(&[(S_IRGRP, "r")]),
        row(&[(S_IWGRP, "w")]),
        row(&[(S_IXGRP | S_ISGID, "s"), (S_ISGID, "S"), (S_IXGRP, "x")]),
        row(&[(S_IROTH, "r")]),
        row(&[(S_IWOTH, "w")]),
        row(&[(S_IXOTH | S_ISVTX, "t"), (S_ISVTX, "T"), (S_IXOTH, "x")]),
    ])
}

/// `_stat.filemode`: the `-rwxrwxrwx` rendering of `mode`.
fn render_filemode(mode: u64) -> String {
    let mut out = String::with_capacity(10);
    out.push(match mode & S_IFMT_MASK {
        S_IFREG => '-',
        S_IFDIR => 'd',
        S_IFLNK => 'l',
        S_IFBLK => 'b',
        S_IFCHR => 'c',
        S_IFIFO => 'p',
        S_IFSOCK => 's',
        _ => '?',
    });
    let has = |bit: u64| mode & bit != 0;
    out.push(if has(S_IRUSR) { 'r' } else { '-' });
    out.push(if has(S_IWUSR) { 'w' } else { '-' });
    out.push(match (has(S_ISUID), has(S_IXUSR)) {
        (true, true) => 's',
        (true, false) => 'S',
        (false, true) => 'x',
        (false, false) => '-',
    });
    out.push(if has(S_IRGRP) { 'r' } else { '-' });
    out.push(if has(S_IWGRP) { 'w' } else { '-' });
    out.push(match (has(S_ISGID), has(S_IXGRP)) {
        (true, true) => 's',
        (true, false) => 'S',
        (false, true) => 'x',
        (false, false) => '-',
    });
    out.push(if has(S_IROTH) { 'r' } else { '-' });
    out.push(if has(S_IWOTH) { 'w' } else { '-' });
    out.push(match (has(S_ISVTX), has(S_IXOTH)) {
        (true, true) => 't',
        (true, false) => 'T',
        (false, true) => 'x',
        (false, false) => '-',
    });
    out
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(STAT_DOC),

        // Indices for stat struct members in the tuple returned by os.stat().
        "ST_MODE" => Value::int(0),
        "ST_INO" => Value::int(1),
        "ST_DEV" => Value::int(2),
        "ST_NLINK" => Value::int(3),
        "ST_UID" => Value::int(4),
        "ST_GID" => Value::int(5),
        "ST_SIZE" => Value::int(6),
        "ST_ATIME" => Value::int(7),
        "ST_MTIME" => Value::int(8),
        "ST_CTIME" => Value::int(9),

        // File types (`S_IFMT()` values).  Door / event port / whiteout do not
        // exist on the platforms CPython builds are tested on here: 0.
        "S_IFDIR" => int(S_IFDIR),
        "S_IFCHR" => int(S_IFCHR),
        "S_IFBLK" => int(S_IFBLK),
        "S_IFREG" => int(S_IFREG),
        "S_IFIFO" => int(S_IFIFO),
        "S_IFLNK" => int(S_IFLNK),
        "S_IFSOCK" => int(S_IFSOCK),
        "S_IFDOOR" => Value::int(0),
        "S_IFPORT" => Value::int(0),
        "S_IFWHT" => Value::int(0),

        // Permission bits.
        "S_ISUID" => int(S_ISUID),
        "S_ISGID" => int(S_ISGID),
        "S_ENFMT" => int(S_ISGID),
        "S_ISVTX" => int(S_ISVTX),
        "S_IREAD" => int(S_IRUSR),
        "S_IWRITE" => int(S_IWUSR),
        "S_IEXEC" => int(S_IXUSR),
        "S_IRWXU" => int(0o0700),
        "S_IRUSR" => int(S_IRUSR),
        "S_IWUSR" => int(S_IWUSR),
        "S_IXUSR" => int(S_IXUSR),
        "S_IRWXG" => int(0o0070),
        "S_IRGRP" => int(S_IRGRP),
        "S_IWGRP" => int(S_IWGRP),
        "S_IXGRP" => int(S_IXGRP),
        "S_IRWXO" => int(0o0007),
        "S_IROTH" => int(S_IROTH),
        "S_IWOTH" => int(S_IWOTH),
        "S_IXOTH" => int(S_IXOTH),

        // File flags.
        "UF_NODUMP" => Value::int(0x0000_0001),
        "UF_IMMUTABLE" => Value::int(0x0000_0002),
        "UF_APPEND" => Value::int(0x0000_0004),
        "UF_OPAQUE" => Value::int(0x0000_0008),
        "UF_NOUNLINK" => Value::int(0x0000_0010),
        "UF_COMPRESSED" => Value::int(0x0000_0020),
        "UF_HIDDEN" => Value::int(0x0000_8000),
        "SF_ARCHIVED" => Value::int(0x0001_0000),
        "SF_IMMUTABLE" => Value::int(0x0002_0000),
        "SF_APPEND" => Value::int(0x0004_0000),
        "SF_NOUNLINK" => Value::int(0x0010_0000),
        "SF_SNAPSHOT" => Value::int(0x0020_0000),

        "_filemode_table" => filemode_table(),

        // Windows FILE_ATTRIBUTE constants for `st_file_attributes`.
        "FILE_ATTRIBUTE_ARCHIVE" => Value::int(32),
        "FILE_ATTRIBUTE_COMPRESSED" => Value::int(2048),
        "FILE_ATTRIBUTE_DEVICE" => Value::int(64),
        "FILE_ATTRIBUTE_DIRECTORY" => Value::int(16),
        "FILE_ATTRIBUTE_ENCRYPTED" => Value::int(16384),
        "FILE_ATTRIBUTE_HIDDEN" => Value::int(2),
        "FILE_ATTRIBUTE_INTEGRITY_STREAM" => Value::int(32768),
        "FILE_ATTRIBUTE_NORMAL" => Value::int(128),
        "FILE_ATTRIBUTE_NOT_CONTENT_INDEXED" => Value::int(8192),
        "FILE_ATTRIBUTE_NO_SCRUB_DATA" => Value::int(131072),
        "FILE_ATTRIBUTE_OFFLINE" => Value::int(4096),
        "FILE_ATTRIBUTE_READONLY" => Value::int(1),
        "FILE_ATTRIBUTE_REPARSE_POINT" => Value::int(1024),
        "FILE_ATTRIBUTE_SPARSE_FILE" => Value::int(512),
        "FILE_ATTRIBUTE_SYSTEM" => Value::int(4),
        "FILE_ATTRIBUTE_TEMPORARY" => Value::int(256),
        "FILE_ATTRIBUTE_VIRTUAL" => Value::int(65536),

        "IO_REPARSE_TAG_SYMLINK" => Value::int(0xA000_000C),
        "IO_REPARSE_TAG_MOUNT_POINT" => Value::int(0xA000_0003),
        "IO_REPARSE_TAG_APPEXECLINK" => Value::int(0x8000_001B),
    }

    /// Return the portion of the file's mode that can be set by os.chmod().
    fn S_IMODE(#[positional_only] mode: PyValue) -> Result<Value> {
        Ok(int(mode_arg(&mode.0)? & 0o7777))
    }

    /// Return the portion of the file's mode that describes the file type.
    fn S_IFMT(#[positional_only] mode: PyValue) -> Result<Value> {
        Ok(int(mode_arg(&mode.0)? & S_IFMT_MASK))
    }

    /// Return True if mode is from a directory.
    fn S_ISDIR(#[positional_only] mode: PyValue) -> Result<Value> {
        is_type(&mode.0, S_IFDIR)
    }

    /// Return True if mode is from a character special device file.
    fn S_ISCHR(#[positional_only] mode: PyValue) -> Result<Value> {
        is_type(&mode.0, S_IFCHR)
    }

    /// Return True if mode is from a block special device file.
    fn S_ISBLK(#[positional_only] mode: PyValue) -> Result<Value> {
        is_type(&mode.0, S_IFBLK)
    }

    /// Return True if mode is from a regular file.
    fn S_ISREG(#[positional_only] mode: PyValue) -> Result<Value> {
        is_type(&mode.0, S_IFREG)
    }

    /// Return True if mode is from a FIFO (named pipe).
    fn S_ISFIFO(#[positional_only] mode: PyValue) -> Result<Value> {
        is_type(&mode.0, S_IFIFO)
    }

    /// Return True if mode is from a symbolic link.
    fn S_ISLNK(#[positional_only] mode: PyValue) -> Result<Value> {
        is_type(&mode.0, S_IFLNK)
    }

    /// Return True if mode is from a socket.
    fn S_ISSOCK(#[positional_only] mode: PyValue) -> Result<Value> {
        is_type(&mode.0, S_IFSOCK)
    }

    /// Return True if mode is from a door.
    fn S_ISDOOR(#[positional_only] mode: PyValue) -> Result<Value> {
        mode_arg(&mode.0)?;
        Ok(Value::bool_(false))
    }

    /// Return True if mode is from an event port.
    fn S_ISPORT(#[positional_only] mode: PyValue) -> Result<Value> {
        mode_arg(&mode.0)?;
        Ok(Value::bool_(false))
    }

    /// Return True if mode is from a whiteout.
    fn S_ISWHT(#[positional_only] mode: PyValue) -> Result<Value> {
        mode_arg(&mode.0)?;
        Ok(Value::bool_(false))
    }

    /// Convert a file's mode to a string of the form '-rwxrwxrwx'
    fn filemode(#[positional_only] mode: PyValue) -> Result<Value> {
        Ok(Value::string(render_filemode(mode_arg(&mode.0)?)))
    }
}
