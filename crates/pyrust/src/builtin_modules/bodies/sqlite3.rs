// `sqlite3` module — body for the `sqlite3 @inject` entry in
// `pyrust_builtin_modules!` (issue #3225).
//
// Native Rust layer backed by bundled SQLite (`rusqlite::ffi`) providing
// connection/statement/blob lifecycle management, parameter binding, lazy
// row stepping, extended error code mapping, user-defined functions,
// aggregates, collations, authorizer/progress/trace hooks, online backup,
// and `complete_statement`.
//
// The public Python 3.12 DB-API 2.0 surface (`Connection`, `Cursor`, `Row`,
// `Blob`, `connect`, exception hierarchy, `adapters`, `converters`,
// `register_adapter`, `register_converter`, `adapt`, `iterdump`, `Date`,
// `Time`, `Timestamp`, `Binary`, ...) lives in `sqlite3_py.py` and is
// injected by `inject_python_members`.
//
// Reference: <https://docs.python.org/3/library/sqlite3.html>

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::rc::{Rc, Weak};
use std::slice;
use std::thread;
use std::time::Duration;

use pyrust_core::{BuiltinTypeOps, PyErrorKind, builtin_ops_is};
use pyrust_derive::pyrust_module;
use rusqlite::ffi;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{PyDict, PyKey, Value, ValueKind};

#[inline]
fn pos_arg(value: Value) -> ExpandedCallArg {
    ExpandedCallArg { name: None, value }
}

/// Python-source definitions for the `sqlite3` module.
const SQLITE3_PY_SOURCE: &str = include_str!("sqlite3_py.py");

/// Private native helpers moved into the exec namespace and hidden from
/// `dir(sqlite3)` so the module surface matches CPython 3.12.
const SQLITE3_NATIVE_HELPERS: &[&str] = &[
    "_sqlite_version",
    "_threadsafety",
    "_sqlite_constants",
    "_complete_statement",
    "_enable_callback_tracebacks",
    "_conn_open",
    "_conn_close",
    "_conn_in_transaction",
    "_conn_total_changes",
    "_conn_interrupt",
    "_conn_exec_simple",
    "_conn_executescript",
    "_conn_create_function",
    "_conn_create_aggregate",
    "_conn_create_collation",
    "_conn_set_authorizer",
    "_conn_set_progress_handler",
    "_conn_set_trace_callback",
    "_conn_backup",
    "_conn_getlimit",
    "_conn_setlimit",
    "_conn_getconfig",
    "_conn_setconfig",
    "_conn_serialize",
    "_conn_deserialize",
    "_stmt_prepare",
    "_stmt_bind_and_step",
    "_stmt_step",
    "_stmt_close",
    "_blob_open",
    "_blob_close",
    "_blob_len",
    "_blob_read",
    "_blob_write",
];

/// Public attributes exported from `SQLITE3_PY_SOURCE` onto the `sqlite3` module.
const SQLITE3_PY_EXPORTS: &[&str] = &[
    "__package__",
    "apilevel",
    "threadsafety",
    "paramstyle",
    "sqlite_version",
    "sqlite_version_info",
    "PARSE_DECLTYPES",
    "PARSE_COLNAMES",
    "LEGACY_TRANSACTION_CONTROL",
    "Warning",
    "Error",
    "InterfaceError",
    "DatabaseError",
    "DataError",
    "OperationalError",
    "IntegrityError",
    "InternalError",
    "ProgrammingError",
    "NotSupportedError",
    "PrepareProtocol",
    "Connection",
    "Cursor",
    "Row",
    "Blob",
    "connect",
    "complete_statement",
    "enable_callback_tracebacks",
    "register_adapter",
    "register_converter",
    "adapt",
    "adapters",
    "converters",
    "Date",
    "Time",
    "Timestamp",
    "DateFromTicks",
    "TimeFromTicks",
    "TimestampFromTicks",
    "Binary",
];

// ── Thread-local Interpreter bridge for SQLite C callbacks ───────────────────

thread_local! {
    static ACTIVE_INTERP: Cell<*mut Interpreter> = const { Cell::new(ptr::null_mut()) };
    static CALLBACK_ERROR: RefCell<Option<PyError>> = const { RefCell::new(None) };
    static CALLBACK_TRACEBACKS: Cell<bool> = const { Cell::new(false) };
}

struct InterpGuard {
    prev: *mut Interpreter,
}

impl InterpGuard {
    fn new(interp: &mut Interpreter) -> Self {
        let prev = ACTIVE_INTERP.with(|cell| cell.replace(interp as *mut Interpreter));
        Self { prev }
    }
}

impl Drop for InterpGuard {
    fn drop(&mut self) {
        ACTIVE_INTERP.with(|cell| cell.set(self.prev));
    }
}

fn with_active_interp<R>(f: impl FnOnce(&mut Interpreter) -> R) -> Option<R> {
    let ptr = ACTIVE_INTERP.with(Cell::get);
    if ptr.is_null() {
        return None;
    }
    // SAFETY: `InterpGuard` guarantees `ptr` points to the live `Interpreter`
    // currently executing a SQLite call on this thread.
    Some(f(unsafe { &mut *ptr }))
}

fn take_callback_error() -> Option<PyError> {
    CALLBACK_ERROR.with(|cell| cell.borrow_mut().take())
}

fn stash_callback_error(err: PyError) {
    CALLBACK_ERROR.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(err);
        }
    });
}

fn maybe_print_callback_error(err: &PyError) {
    if CALLBACK_TRACEBACKS.with(Cell::get) {
        eprintln!("Traceback (most recent call last):");
        eprintln!("  {err}");
    }
}

// ── SQLite error code table & exception raising ──────────────────────────────

/// Return the standard SQLite result / extended result code constant name.
fn sqlite_error_name(code: c_int) -> &'static str {
    match code {
        ffi::SQLITE_OK => "SQLITE_OK",
        ffi::SQLITE_ERROR => "SQLITE_ERROR",
        ffi::SQLITE_INTERNAL => "SQLITE_INTERNAL",
        ffi::SQLITE_PERM => "SQLITE_PERM",
        ffi::SQLITE_ABORT => "SQLITE_ABORT",
        ffi::SQLITE_BUSY => "SQLITE_BUSY",
        ffi::SQLITE_LOCKED => "SQLITE_LOCKED",
        ffi::SQLITE_NOMEM => "SQLITE_NOMEM",
        ffi::SQLITE_READONLY => "SQLITE_READONLY",
        ffi::SQLITE_INTERRUPT => "SQLITE_INTERRUPT",
        ffi::SQLITE_IOERR => "SQLITE_IOERR",
        ffi::SQLITE_CORRUPT => "SQLITE_CORRUPT",
        ffi::SQLITE_NOTFOUND => "SQLITE_NOTFOUND",
        ffi::SQLITE_FULL => "SQLITE_FULL",
        ffi::SQLITE_CANTOPEN => "SQLITE_CANTOPEN",
        ffi::SQLITE_PROTOCOL => "SQLITE_PROTOCOL",
        ffi::SQLITE_EMPTY => "SQLITE_EMPTY",
        ffi::SQLITE_SCHEMA => "SQLITE_SCHEMA",
        ffi::SQLITE_TOOBIG => "SQLITE_TOOBIG",
        ffi::SQLITE_CONSTRAINT => "SQLITE_CONSTRAINT",
        ffi::SQLITE_MISMATCH => "SQLITE_MISMATCH",
        ffi::SQLITE_MISUSE => "SQLITE_MISUSE",
        ffi::SQLITE_NOLFS => "SQLITE_NOLFS",
        ffi::SQLITE_AUTH => "SQLITE_AUTH",
        ffi::SQLITE_FORMAT => "SQLITE_FORMAT",
        ffi::SQLITE_RANGE => "SQLITE_RANGE",
        ffi::SQLITE_NOTADB => "SQLITE_NOTADB",
        ffi::SQLITE_NOTICE => "SQLITE_NOTICE",
        ffi::SQLITE_WARNING => "SQLITE_WARNING",
        ffi::SQLITE_ROW => "SQLITE_ROW",
        ffi::SQLITE_DONE => "SQLITE_DONE",
        ffi::SQLITE_ERROR_MISSING_COLLSEQ => "SQLITE_ERROR_MISSING_COLLSEQ",
        ffi::SQLITE_ERROR_RETRY => "SQLITE_ERROR_RETRY",
        ffi::SQLITE_ERROR_SNAPSHOT => "SQLITE_ERROR_SNAPSHOT",
        ffi::SQLITE_IOERR_READ => "SQLITE_IOERR_READ",
        ffi::SQLITE_IOERR_SHORT_READ => "SQLITE_IOERR_SHORT_READ",
        ffi::SQLITE_IOERR_WRITE => "SQLITE_IOERR_WRITE",
        ffi::SQLITE_IOERR_FSYNC => "SQLITE_IOERR_FSYNC",
        ffi::SQLITE_IOERR_DIR_FSYNC => "SQLITE_IOERR_DIR_FSYNC",
        ffi::SQLITE_IOERR_TRUNCATE => "SQLITE_IOERR_TRUNCATE",
        ffi::SQLITE_IOERR_FSTAT => "SQLITE_IOERR_FSTAT",
        ffi::SQLITE_IOERR_UNLOCK => "SQLITE_IOERR_UNLOCK",
        ffi::SQLITE_IOERR_RDLOCK => "SQLITE_IOERR_RDLOCK",
        ffi::SQLITE_IOERR_DELETE => "SQLITE_IOERR_DELETE",
        ffi::SQLITE_IOERR_BLOCKED => "SQLITE_IOERR_BLOCKED",
        ffi::SQLITE_IOERR_NOMEM => "SQLITE_IOERR_NOMEM",
        ffi::SQLITE_IOERR_ACCESS => "SQLITE_IOERR_ACCESS",
        ffi::SQLITE_IOERR_CHECKRESERVEDLOCK => "SQLITE_IOERR_CHECKRESERVEDLOCK",
        ffi::SQLITE_IOERR_LOCK => "SQLITE_IOERR_LOCK",
        ffi::SQLITE_IOERR_CLOSE => "SQLITE_IOERR_CLOSE",
        ffi::SQLITE_IOERR_DIR_CLOSE => "SQLITE_IOERR_DIR_CLOSE",
        ffi::SQLITE_IOERR_SHMOPEN => "SQLITE_IOERR_SHMOPEN",
        ffi::SQLITE_IOERR_SHMSIZE => "SQLITE_IOERR_SHMSIZE",
        ffi::SQLITE_IOERR_SHMLOCK => "SQLITE_IOERR_SHMLOCK",
        ffi::SQLITE_IOERR_SHMMAP => "SQLITE_IOERR_SHMMAP",
        ffi::SQLITE_IOERR_SEEK => "SQLITE_IOERR_SEEK",
        ffi::SQLITE_IOERR_DELETE_NOENT => "SQLITE_IOERR_DELETE_NOENT",
        ffi::SQLITE_IOERR_MMAP => "SQLITE_IOERR_MMAP",
        ffi::SQLITE_IOERR_GETTEMPPATH => "SQLITE_IOERR_GETTEMPPATH",
        ffi::SQLITE_IOERR_CONVPATH => "SQLITE_IOERR_CONVPATH",
        ffi::SQLITE_IOERR_VNODE => "SQLITE_IOERR_VNODE",
        ffi::SQLITE_IOERR_AUTH => "SQLITE_IOERR_AUTH",
        ffi::SQLITE_IOERR_BEGIN_ATOMIC => "SQLITE_IOERR_BEGIN_ATOMIC",
        ffi::SQLITE_IOERR_COMMIT_ATOMIC => "SQLITE_IOERR_COMMIT_ATOMIC",
        ffi::SQLITE_IOERR_ROLLBACK_ATOMIC => "SQLITE_IOERR_ROLLBACK_ATOMIC",
        ffi::SQLITE_IOERR_DATA => "SQLITE_IOERR_DATA",
        ffi::SQLITE_IOERR_CORRUPTFS => "SQLITE_IOERR_CORRUPTFS",
        ffi::SQLITE_LOCKED_SHAREDCACHE => "SQLITE_LOCKED_SHAREDCACHE",
        ffi::SQLITE_LOCKED_VTAB => "SQLITE_LOCKED_VTAB",
        ffi::SQLITE_BUSY_RECOVERY => "SQLITE_BUSY_RECOVERY",
        ffi::SQLITE_BUSY_SNAPSHOT => "SQLITE_BUSY_SNAPSHOT",
        ffi::SQLITE_BUSY_TIMEOUT => "SQLITE_BUSY_TIMEOUT",
        ffi::SQLITE_CANTOPEN_NOTEMPDIR => "SQLITE_CANTOPEN_NOTEMPDIR",
        ffi::SQLITE_CANTOPEN_ISDIR => "SQLITE_CANTOPEN_ISDIR",
        ffi::SQLITE_CANTOPEN_FULLPATH => "SQLITE_CANTOPEN_FULLPATH",
        ffi::SQLITE_CANTOPEN_CONVPATH => "SQLITE_CANTOPEN_CONVPATH",
        ffi::SQLITE_CANTOPEN_DIRTYWAL => "SQLITE_CANTOPEN_DIRTYWAL",
        ffi::SQLITE_CANTOPEN_SYMLINK => "SQLITE_CANTOPEN_SYMLINK",
        ffi::SQLITE_CORRUPT_VTAB => "SQLITE_CORRUPT_VTAB",
        ffi::SQLITE_CORRUPT_SEQUENCE => "SQLITE_CORRUPT_SEQUENCE",
        ffi::SQLITE_CORRUPT_INDEX => "SQLITE_CORRUPT_INDEX",
        ffi::SQLITE_READONLY_RECOVERY => "SQLITE_READONLY_RECOVERY",
        ffi::SQLITE_READONLY_CANTLOCK => "SQLITE_READONLY_CANTLOCK",
        ffi::SQLITE_READONLY_ROLLBACK => "SQLITE_READONLY_ROLLBACK",
        ffi::SQLITE_READONLY_DBMOVED => "SQLITE_READONLY_DBMOVED",
        ffi::SQLITE_READONLY_CANTINIT => "SQLITE_READONLY_CANTINIT",
        ffi::SQLITE_READONLY_DIRECTORY => "SQLITE_READONLY_DIRECTORY",
        ffi::SQLITE_ABORT_ROLLBACK => "SQLITE_ABORT_ROLLBACK",
        ffi::SQLITE_CONSTRAINT_CHECK => "SQLITE_CONSTRAINT_CHECK",
        ffi::SQLITE_CONSTRAINT_COMMITHOOK => "SQLITE_CONSTRAINT_COMMITHOOK",
        ffi::SQLITE_CONSTRAINT_FOREIGNKEY => "SQLITE_CONSTRAINT_FOREIGNKEY",
        ffi::SQLITE_CONSTRAINT_FUNCTION => "SQLITE_CONSTRAINT_FUNCTION",
        ffi::SQLITE_CONSTRAINT_NOTNULL => "SQLITE_CONSTRAINT_NOTNULL",
        ffi::SQLITE_CONSTRAINT_PRIMARYKEY => "SQLITE_CONSTRAINT_PRIMARYKEY",
        ffi::SQLITE_CONSTRAINT_TRIGGER => "SQLITE_CONSTRAINT_TRIGGER",
        ffi::SQLITE_CONSTRAINT_UNIQUE => "SQLITE_CONSTRAINT_UNIQUE",
        ffi::SQLITE_CONSTRAINT_VTAB => "SQLITE_CONSTRAINT_VTAB",
        ffi::SQLITE_CONSTRAINT_ROWID => "SQLITE_CONSTRAINT_ROWID",
        ffi::SQLITE_CONSTRAINT_PINNED => "SQLITE_CONSTRAINT_PINNED",
        ffi::SQLITE_NOTICE_RECOVER_WAL => "SQLITE_NOTICE_RECOVER_WAL",
        ffi::SQLITE_NOTICE_RECOVER_ROLLBACK => "SQLITE_NOTICE_RECOVER_ROLLBACK",
        ffi::SQLITE_WARNING_AUTOINDEX => "SQLITE_WARNING_AUTOINDEX",
        ffi::SQLITE_AUTH_USER => "SQLITE_AUTH_USER",
        ffi::SQLITE_OK_LOAD_PERMANENTLY => "SQLITE_OK_LOAD_PERMANENTLY",
        ffi::SQLITE_OK_SYMLINK => "SQLITE_OK_SYMLINK",
        _ => "UNKNOWN",
    }
}

fn sqlite_error_class_name(primary_code: c_int) -> &'static str {
    match primary_code {
        ffi::SQLITE_INTERNAL | ffi::SQLITE_NOTFOUND => "InternalError",
        ffi::SQLITE_ERROR
        | ffi::SQLITE_PERM
        | ffi::SQLITE_ABORT
        | ffi::SQLITE_BUSY
        | ffi::SQLITE_LOCKED
        | ffi::SQLITE_READONLY
        | ffi::SQLITE_INTERRUPT
        | ffi::SQLITE_IOERR
        | ffi::SQLITE_FULL
        | ffi::SQLITE_CANTOPEN
        | ffi::SQLITE_PROTOCOL
        | ffi::SQLITE_EMPTY
        | ffi::SQLITE_SCHEMA
        | ffi::SQLITE_AUTH => "OperationalError",
        ffi::SQLITE_CORRUPT | ffi::SQLITE_NOTADB | ffi::SQLITE_FORMAT => "DatabaseError",
        ffi::SQLITE_TOOBIG => "DataError",
        ffi::SQLITE_CONSTRAINT | ffi::SQLITE_MISMATCH => "IntegrityError",
        ffi::SQLITE_MISUSE => "ProgrammingError",
        ffi::SQLITE_RANGE => "InterfaceError",
        _ => "DatabaseError",
    }
}

fn raise_sqlite_class(
    interp: &mut Interpreter,
    class_name: &str,
    msg: String,
    errcode: Option<c_int>,
) -> PyError {
    if let Ok(module_val) = interp.load_module("sqlite3")
        && let ValueKind::PyModule(mod_rc) = module_val.kind()
    {
        let cls_opt = mod_rc.borrow().attrs.get(class_name).cloned();
        if let Some(cls) = cls_opt
            && let Ok(inst) = interp.call_function_expanded(cls, &[pos_arg(Value::string(&msg))])
        {
            if let Some(code) = errcode
                && let ValueKind::PyInstance(inst_rc) = inst.kind()
            {
                let mut b = inst_rc.borrow_mut();
                b.attrs
                    .insert("sqlite_errorcode", Value::int(i64::from(code)));
                b.attrs
                    .insert("sqlite_errorname", Value::string(sqlite_error_name(code)));
            }
            return PyError::new(PyErrorKind::Raised(inst));
        }
    }
    PyError::runtime(msg)
}

fn raise_db_error(
    interp: &mut Interpreter,
    db: *mut ffi::sqlite3,
    fallback_code: c_int,
) -> PyError {
    if let Some(cb_err) = take_callback_error() {
        return cb_err;
    }
    let (ext_code, msg) = if db.is_null() {
        let s = unsafe {
            let p = ffi::sqlite3_errstr(fallback_code);
            if p.is_null() {
                "SQLite error".to_string()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        };
        (fallback_code, s)
    } else {
        let ext = unsafe { ffi::sqlite3_extended_errcode(db) };
        let code = if ext != ffi::SQLITE_OK {
            ext
        } else {
            fallback_code
        };
        let s = unsafe {
            let p = ffi::sqlite3_errmsg(db);
            if p.is_null() {
                "SQLite error".to_string()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        };
        (code, s)
    };
    let primary = ext_code & 0xFF;
    if primary == ffi::SQLITE_NOMEM {
        return PyError::named("MemoryError", msg);
    }
    let cls_name = sqlite_error_class_name(primary);
    raise_sqlite_class(interp, cls_name, msg, Some(ext_code))
}

fn closed_db_error(interp: &mut Interpreter) -> PyError {
    raise_sqlite_class(
        interp,
        "ProgrammingError",
        "Cannot operate on a closed database.".to_string(),
        None,
    )
}

fn closed_cursor_error(interp: &mut Interpreter) -> PyError {
    raise_sqlite_class(
        interp,
        "ProgrammingError",
        "Cannot operate on a closed cursor.".to_string(),
        None,
    )
}

fn closed_blob_error(interp: &mut Interpreter) -> PyError {
    raise_sqlite_class(
        interp,
        "ProgrammingError",
        "Cannot operate on a closed blob.".to_string(),
        None,
    )
}

// ── SQLite C constants exported to Python ────────────────────────────────────

const SQLITE_CONSTANTS: &[(&str, c_int)] = &[
    ("SQLITE_OK", ffi::SQLITE_OK),
    ("SQLITE_ERROR", ffi::SQLITE_ERROR),
    ("SQLITE_INTERNAL", ffi::SQLITE_INTERNAL),
    ("SQLITE_PERM", ffi::SQLITE_PERM),
    ("SQLITE_ABORT", ffi::SQLITE_ABORT),
    ("SQLITE_BUSY", ffi::SQLITE_BUSY),
    ("SQLITE_LOCKED", ffi::SQLITE_LOCKED),
    ("SQLITE_NOMEM", ffi::SQLITE_NOMEM),
    ("SQLITE_READONLY", ffi::SQLITE_READONLY),
    ("SQLITE_INTERRUPT", ffi::SQLITE_INTERRUPT),
    ("SQLITE_IOERR", ffi::SQLITE_IOERR),
    ("SQLITE_CORRUPT", ffi::SQLITE_CORRUPT),
    ("SQLITE_NOTFOUND", ffi::SQLITE_NOTFOUND),
    ("SQLITE_FULL", ffi::SQLITE_FULL),
    ("SQLITE_CANTOPEN", ffi::SQLITE_CANTOPEN),
    ("SQLITE_PROTOCOL", ffi::SQLITE_PROTOCOL),
    ("SQLITE_EMPTY", ffi::SQLITE_EMPTY),
    ("SQLITE_SCHEMA", ffi::SQLITE_SCHEMA),
    ("SQLITE_TOOBIG", ffi::SQLITE_TOOBIG),
    ("SQLITE_CONSTRAINT", ffi::SQLITE_CONSTRAINT),
    ("SQLITE_MISMATCH", ffi::SQLITE_MISMATCH),
    ("SQLITE_MISUSE", ffi::SQLITE_MISUSE),
    ("SQLITE_NOLFS", ffi::SQLITE_NOLFS),
    ("SQLITE_AUTH", ffi::SQLITE_AUTH),
    ("SQLITE_FORMAT", ffi::SQLITE_FORMAT),
    ("SQLITE_RANGE", ffi::SQLITE_RANGE),
    ("SQLITE_NOTADB", ffi::SQLITE_NOTADB),
    ("SQLITE_NOTICE", ffi::SQLITE_NOTICE),
    ("SQLITE_WARNING", ffi::SQLITE_WARNING),
    ("SQLITE_ROW", ffi::SQLITE_ROW),
    ("SQLITE_DONE", ffi::SQLITE_DONE),
    ("SQLITE_DENY", ffi::SQLITE_DENY),
    ("SQLITE_IGNORE", ffi::SQLITE_IGNORE),
    // Extended result codes
    ("SQLITE_ABORT_ROLLBACK", ffi::SQLITE_ABORT_ROLLBACK),
    ("SQLITE_AUTH_USER", ffi::SQLITE_AUTH_USER),
    ("SQLITE_BUSY_RECOVERY", ffi::SQLITE_BUSY_RECOVERY),
    ("SQLITE_BUSY_SNAPSHOT", ffi::SQLITE_BUSY_SNAPSHOT),
    ("SQLITE_BUSY_TIMEOUT", ffi::SQLITE_BUSY_TIMEOUT),
    ("SQLITE_CANTOPEN_CONVPATH", ffi::SQLITE_CANTOPEN_CONVPATH),
    ("SQLITE_CANTOPEN_DIRTYWAL", ffi::SQLITE_CANTOPEN_DIRTYWAL),
    ("SQLITE_CANTOPEN_FULLPATH", ffi::SQLITE_CANTOPEN_FULLPATH),
    ("SQLITE_CANTOPEN_ISDIR", ffi::SQLITE_CANTOPEN_ISDIR),
    ("SQLITE_CANTOPEN_NOTEMPDIR", ffi::SQLITE_CANTOPEN_NOTEMPDIR),
    ("SQLITE_CANTOPEN_SYMLINK", ffi::SQLITE_CANTOPEN_SYMLINK),
    ("SQLITE_CONSTRAINT_CHECK", ffi::SQLITE_CONSTRAINT_CHECK),
    (
        "SQLITE_CONSTRAINT_COMMITHOOK",
        ffi::SQLITE_CONSTRAINT_COMMITHOOK,
    ),
    (
        "SQLITE_CONSTRAINT_FOREIGNKEY",
        ffi::SQLITE_CONSTRAINT_FOREIGNKEY,
    ),
    (
        "SQLITE_CONSTRAINT_FUNCTION",
        ffi::SQLITE_CONSTRAINT_FUNCTION,
    ),
    ("SQLITE_CONSTRAINT_NOTNULL", ffi::SQLITE_CONSTRAINT_NOTNULL),
    ("SQLITE_CONSTRAINT_PINNED", ffi::SQLITE_CONSTRAINT_PINNED),
    (
        "SQLITE_CONSTRAINT_PRIMARYKEY",
        ffi::SQLITE_CONSTRAINT_PRIMARYKEY,
    ),
    ("SQLITE_CONSTRAINT_ROWID", ffi::SQLITE_CONSTRAINT_ROWID),
    ("SQLITE_CONSTRAINT_TRIGGER", ffi::SQLITE_CONSTRAINT_TRIGGER),
    ("SQLITE_CONSTRAINT_UNIQUE", ffi::SQLITE_CONSTRAINT_UNIQUE),
    ("SQLITE_CONSTRAINT_VTAB", ffi::SQLITE_CONSTRAINT_VTAB),
    ("SQLITE_CORRUPT_INDEX", ffi::SQLITE_CORRUPT_INDEX),
    ("SQLITE_CORRUPT_SEQUENCE", ffi::SQLITE_CORRUPT_SEQUENCE),
    ("SQLITE_CORRUPT_VTAB", ffi::SQLITE_CORRUPT_VTAB),
    (
        "SQLITE_ERROR_MISSING_COLLSEQ",
        ffi::SQLITE_ERROR_MISSING_COLLSEQ,
    ),
    ("SQLITE_ERROR_RETRY", ffi::SQLITE_ERROR_RETRY),
    ("SQLITE_ERROR_SNAPSHOT", ffi::SQLITE_ERROR_SNAPSHOT),
    ("SQLITE_IOERR_ACCESS", ffi::SQLITE_IOERR_ACCESS),
    ("SQLITE_IOERR_AUTH", ffi::SQLITE_IOERR_AUTH),
    ("SQLITE_IOERR_BEGIN_ATOMIC", ffi::SQLITE_IOERR_BEGIN_ATOMIC),
    ("SQLITE_IOERR_BLOCKED", ffi::SQLITE_IOERR_BLOCKED),
    (
        "SQLITE_IOERR_CHECKRESERVEDLOCK",
        ffi::SQLITE_IOERR_CHECKRESERVEDLOCK,
    ),
    ("SQLITE_IOERR_CLOSE", ffi::SQLITE_IOERR_CLOSE),
    (
        "SQLITE_IOERR_COMMIT_ATOMIC",
        ffi::SQLITE_IOERR_COMMIT_ATOMIC,
    ),
    ("SQLITE_IOERR_CONVPATH", ffi::SQLITE_IOERR_CONVPATH),
    ("SQLITE_IOERR_CORRUPTFS", ffi::SQLITE_IOERR_CORRUPTFS),
    ("SQLITE_IOERR_DATA", ffi::SQLITE_IOERR_DATA),
    ("SQLITE_IOERR_DELETE", ffi::SQLITE_IOERR_DELETE),
    ("SQLITE_IOERR_DELETE_NOENT", ffi::SQLITE_IOERR_DELETE_NOENT),
    ("SQLITE_IOERR_DIR_CLOSE", ffi::SQLITE_IOERR_DIR_CLOSE),
    ("SQLITE_IOERR_DIR_FSYNC", ffi::SQLITE_IOERR_DIR_FSYNC),
    ("SQLITE_IOERR_FSTAT", ffi::SQLITE_IOERR_FSTAT),
    ("SQLITE_IOERR_FSYNC", ffi::SQLITE_IOERR_FSYNC),
    ("SQLITE_IOERR_GETTEMPPATH", ffi::SQLITE_IOERR_GETTEMPPATH),
    ("SQLITE_IOERR_LOCK", ffi::SQLITE_IOERR_LOCK),
    ("SQLITE_IOERR_MMAP", ffi::SQLITE_IOERR_MMAP),
    ("SQLITE_IOERR_NOMEM", ffi::SQLITE_IOERR_NOMEM),
    ("SQLITE_IOERR_RDLOCK", ffi::SQLITE_IOERR_RDLOCK),
    ("SQLITE_IOERR_READ", ffi::SQLITE_IOERR_READ),
    (
        "SQLITE_IOERR_ROLLBACK_ATOMIC",
        ffi::SQLITE_IOERR_ROLLBACK_ATOMIC,
    ),
    ("SQLITE_IOERR_SEEK", ffi::SQLITE_IOERR_SEEK),
    ("SQLITE_IOERR_SHMLOCK", ffi::SQLITE_IOERR_SHMLOCK),
    ("SQLITE_IOERR_SHMMAP", ffi::SQLITE_IOERR_SHMMAP),
    ("SQLITE_IOERR_SHMOPEN", ffi::SQLITE_IOERR_SHMOPEN),
    ("SQLITE_IOERR_SHMSIZE", ffi::SQLITE_IOERR_SHMSIZE),
    ("SQLITE_IOERR_SHORT_READ", ffi::SQLITE_IOERR_SHORT_READ),
    ("SQLITE_IOERR_TRUNCATE", ffi::SQLITE_IOERR_TRUNCATE),
    ("SQLITE_IOERR_UNLOCK", ffi::SQLITE_IOERR_UNLOCK),
    ("SQLITE_IOERR_VNODE", ffi::SQLITE_IOERR_VNODE),
    ("SQLITE_IOERR_WRITE", ffi::SQLITE_IOERR_WRITE),
    ("SQLITE_LOCKED_SHAREDCACHE", ffi::SQLITE_LOCKED_SHAREDCACHE),
    ("SQLITE_LOCKED_VTAB", ffi::SQLITE_LOCKED_VTAB),
    (
        "SQLITE_NOTICE_RECOVER_ROLLBACK",
        ffi::SQLITE_NOTICE_RECOVER_ROLLBACK,
    ),
    ("SQLITE_NOTICE_RECOVER_WAL", ffi::SQLITE_NOTICE_RECOVER_WAL),
    (
        "SQLITE_OK_LOAD_PERMANENTLY",
        ffi::SQLITE_OK_LOAD_PERMANENTLY,
    ),
    ("SQLITE_OK_SYMLINK", ffi::SQLITE_OK_SYMLINK),
    ("SQLITE_READONLY_CANTINIT", ffi::SQLITE_READONLY_CANTINIT),
    ("SQLITE_READONLY_CANTLOCK", ffi::SQLITE_READONLY_CANTLOCK),
    ("SQLITE_READONLY_DBMOVED", ffi::SQLITE_READONLY_DBMOVED),
    ("SQLITE_READONLY_DIRECTORY", ffi::SQLITE_READONLY_DIRECTORY),
    ("SQLITE_READONLY_RECOVERY", ffi::SQLITE_READONLY_RECOVERY),
    ("SQLITE_READONLY_ROLLBACK", ffi::SQLITE_READONLY_ROLLBACK),
    ("SQLITE_WARNING_AUTOINDEX", ffi::SQLITE_WARNING_AUTOINDEX),
    // Authorizer action codes
    ("SQLITE_CREATE_INDEX", ffi::SQLITE_CREATE_INDEX),
    ("SQLITE_CREATE_TABLE", ffi::SQLITE_CREATE_TABLE),
    ("SQLITE_CREATE_TEMP_INDEX", ffi::SQLITE_CREATE_TEMP_INDEX),
    ("SQLITE_CREATE_TEMP_TABLE", ffi::SQLITE_CREATE_TEMP_TABLE),
    (
        "SQLITE_CREATE_TEMP_TRIGGER",
        ffi::SQLITE_CREATE_TEMP_TRIGGER,
    ),
    ("SQLITE_CREATE_TEMP_VIEW", ffi::SQLITE_CREATE_TEMP_VIEW),
    ("SQLITE_CREATE_TRIGGER", ffi::SQLITE_CREATE_TRIGGER),
    ("SQLITE_CREATE_VIEW", ffi::SQLITE_CREATE_VIEW),
    ("SQLITE_DELETE", ffi::SQLITE_DELETE),
    ("SQLITE_DROP_INDEX", ffi::SQLITE_DROP_INDEX),
    ("SQLITE_DROP_TABLE", ffi::SQLITE_DROP_TABLE),
    ("SQLITE_DROP_TEMP_INDEX", ffi::SQLITE_DROP_TEMP_INDEX),
    ("SQLITE_DROP_TEMP_TABLE", ffi::SQLITE_DROP_TEMP_TABLE),
    ("SQLITE_DROP_TEMP_TRIGGER", ffi::SQLITE_DROP_TEMP_TRIGGER),
    ("SQLITE_DROP_TEMP_VIEW", ffi::SQLITE_DROP_TEMP_VIEW),
    ("SQLITE_DROP_TRIGGER", ffi::SQLITE_DROP_TRIGGER),
    ("SQLITE_DROP_VIEW", ffi::SQLITE_DROP_VIEW),
    ("SQLITE_INSERT", ffi::SQLITE_INSERT),
    ("SQLITE_PRAGMA", ffi::SQLITE_PRAGMA),
    ("SQLITE_READ", ffi::SQLITE_READ),
    ("SQLITE_SELECT", ffi::SQLITE_SELECT),
    ("SQLITE_TRANSACTION", ffi::SQLITE_TRANSACTION),
    ("SQLITE_UPDATE", ffi::SQLITE_UPDATE),
    ("SQLITE_ATTACH", ffi::SQLITE_ATTACH),
    ("SQLITE_DETACH", ffi::SQLITE_DETACH),
    ("SQLITE_ALTER_TABLE", ffi::SQLITE_ALTER_TABLE),
    ("SQLITE_REINDEX", ffi::SQLITE_REINDEX),
    ("SQLITE_ANALYZE", ffi::SQLITE_ANALYZE),
    ("SQLITE_CREATE_VTABLE", ffi::SQLITE_CREATE_VTABLE),
    ("SQLITE_DROP_VTABLE", ffi::SQLITE_DROP_VTABLE),
    ("SQLITE_FUNCTION", ffi::SQLITE_FUNCTION),
    ("SQLITE_SAVEPOINT", ffi::SQLITE_SAVEPOINT),
    ("SQLITE_RECURSIVE", ffi::SQLITE_RECURSIVE),
    // Limit categories
    ("SQLITE_LIMIT_LENGTH", ffi::SQLITE_LIMIT_LENGTH),
    ("SQLITE_LIMIT_SQL_LENGTH", ffi::SQLITE_LIMIT_SQL_LENGTH),
    ("SQLITE_LIMIT_COLUMN", ffi::SQLITE_LIMIT_COLUMN),
    ("SQLITE_LIMIT_EXPR_DEPTH", ffi::SQLITE_LIMIT_EXPR_DEPTH),
    (
        "SQLITE_LIMIT_COMPOUND_SELECT",
        ffi::SQLITE_LIMIT_COMPOUND_SELECT,
    ),
    ("SQLITE_LIMIT_VDBE_OP", ffi::SQLITE_LIMIT_VDBE_OP),
    ("SQLITE_LIMIT_FUNCTION_ARG", ffi::SQLITE_LIMIT_FUNCTION_ARG),
    ("SQLITE_LIMIT_ATTACHED", ffi::SQLITE_LIMIT_ATTACHED),
    (
        "SQLITE_LIMIT_LIKE_PATTERN_LENGTH",
        ffi::SQLITE_LIMIT_LIKE_PATTERN_LENGTH,
    ),
    (
        "SQLITE_LIMIT_VARIABLE_NUMBER",
        ffi::SQLITE_LIMIT_VARIABLE_NUMBER,
    ),
    (
        "SQLITE_LIMIT_TRIGGER_DEPTH",
        ffi::SQLITE_LIMIT_TRIGGER_DEPTH,
    ),
    (
        "SQLITE_LIMIT_WORKER_THREADS",
        ffi::SQLITE_LIMIT_WORKER_THREADS,
    ),
    // DB config options
    ("SQLITE_DBCONFIG_DEFENSIVE", ffi::SQLITE_DBCONFIG_DEFENSIVE),
    ("SQLITE_DBCONFIG_DQS_DDL", ffi::SQLITE_DBCONFIG_DQS_DDL),
    ("SQLITE_DBCONFIG_DQS_DML", ffi::SQLITE_DBCONFIG_DQS_DML),
    (
        "SQLITE_DBCONFIG_ENABLE_FKEY",
        ffi::SQLITE_DBCONFIG_ENABLE_FKEY,
    ),
    (
        "SQLITE_DBCONFIG_ENABLE_FTS3_TOKENIZER",
        ffi::SQLITE_DBCONFIG_ENABLE_FTS3_TOKENIZER,
    ),
    (
        "SQLITE_DBCONFIG_ENABLE_LOAD_EXTENSION",
        ffi::SQLITE_DBCONFIG_ENABLE_LOAD_EXTENSION,
    ),
    (
        "SQLITE_DBCONFIG_ENABLE_QPSG",
        ffi::SQLITE_DBCONFIG_ENABLE_QPSG,
    ),
    (
        "SQLITE_DBCONFIG_ENABLE_TRIGGER",
        ffi::SQLITE_DBCONFIG_ENABLE_TRIGGER,
    ),
    (
        "SQLITE_DBCONFIG_ENABLE_VIEW",
        ffi::SQLITE_DBCONFIG_ENABLE_VIEW,
    ),
    (
        "SQLITE_DBCONFIG_LEGACY_ALTER_TABLE",
        ffi::SQLITE_DBCONFIG_LEGACY_ALTER_TABLE,
    ),
    (
        "SQLITE_DBCONFIG_LEGACY_FILE_FORMAT",
        ffi::SQLITE_DBCONFIG_LEGACY_FILE_FORMAT,
    ),
    (
        "SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE",
        ffi::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
    ),
    (
        "SQLITE_DBCONFIG_RESET_DATABASE",
        ffi::SQLITE_DBCONFIG_RESET_DATABASE,
    ),
    (
        "SQLITE_DBCONFIG_TRIGGER_EQP",
        ffi::SQLITE_DBCONFIG_TRIGGER_EQP,
    ),
    (
        "SQLITE_DBCONFIG_TRUSTED_SCHEMA",
        ffi::SQLITE_DBCONFIG_TRUSTED_SCHEMA,
    ),
    (
        "SQLITE_DBCONFIG_WRITABLE_SCHEMA",
        ffi::SQLITE_DBCONFIG_WRITABLE_SCHEMA,
    ),
];

// ── Native RAII handles ──────────────────────────────────────────────────────

struct SqliteConnInner {
    db: Cell<*mut ffi::sqlite3>,
    open_stmts: RefCell<Vec<Weak<Cell<*mut ffi::sqlite3_stmt>>>>,
    open_blobs: RefCell<Vec<Weak<Cell<*mut ffi::sqlite3_blob>>>>,
    authorizer: RefCell<Option<Value>>,
    progress_handler: RefCell<Option<Value>>,
    trace_callback: RefCell<Option<Value>>,
}

impl SqliteConnInner {
    fn new(db: *mut ffi::sqlite3) -> Rc<Self> {
        Rc::new(Self {
            db: Cell::new(db),
            open_stmts: RefCell::new(Vec::new()),
            open_blobs: RefCell::new(Vec::new()),
            authorizer: RefCell::new(None),
            progress_handler: RefCell::new(None),
            trace_callback: RefCell::new(None),
        })
    }

    fn close(&self) {
        let db = self.db.replace(ptr::null_mut());
        if db.is_null() {
            return;
        }
        // Finalize all live statements before closing the DB.
        {
            let mut stmts = self.open_stmts.borrow_mut();
            for weak in stmts.drain(..) {
                if let Some(slot) = weak.upgrade() {
                    let stmt = slot.replace(ptr::null_mut());
                    if !stmt.is_null() {
                        unsafe {
                            ffi::sqlite3_finalize(stmt);
                        }
                    }
                }
            }
        }
        // Close all live blobs before closing the DB.
        {
            let mut blobs = self.open_blobs.borrow_mut();
            for weak in blobs.drain(..) {
                if let Some(slot) = weak.upgrade() {
                    let blob = slot.replace(ptr::null_mut());
                    if !blob.is_null() {
                        unsafe {
                            ffi::sqlite3_blob_close(blob);
                        }
                    }
                }
            }
        }
        // Clear connection-level hooks before closing.
        unsafe {
            ffi::sqlite3_set_authorizer(db, None, ptr::null_mut());
            ffi::sqlite3_progress_handler(db, 0, None, ptr::null_mut());
            ffi::sqlite3_trace_v2(db, 0, None, ptr::null_mut());
        }
        *self.authorizer.borrow_mut() = None;
        *self.progress_handler.borrow_mut() = None;
        *self.trace_callback.borrow_mut() = None;

        // Close connection (this also invokes xDestroy on registered UDFs/collations).
        unsafe {
            ffi::sqlite3_close(db);
        }
    }

    fn register_stmt(&self, slot: &Rc<Cell<*mut ffi::sqlite3_stmt>>) {
        let mut stmts = self.open_stmts.borrow_mut();
        if stmts.len() >= 64 {
            stmts.retain(|w| w.strong_count() > 0);
        }
        stmts.push(Rc::downgrade(slot));
    }

    fn register_blob(&self, slot: &Rc<Cell<*mut ffi::sqlite3_blob>>) {
        let mut blobs = self.open_blobs.borrow_mut();
        if blobs.len() >= 32 {
            blobs.retain(|w| w.strong_count() > 0);
        }
        blobs.push(Rc::downgrade(slot));
    }
}

impl Drop for SqliteConnInner {
    fn drop(&mut self) {
        self.close();
    }
}

struct SqliteConnHandle(Rc<SqliteConnInner>);

struct SqliteConnOps;
const SQLITE_CONN_OPS: &SqliteConnOps = &SqliteConnOps;

impl BuiltinTypeOps for SqliteConnOps {
    fn type_name(&self) -> &'static str {
        "_sqlite_conn"
    }

    fn is_internal_storage(&self) -> bool {
        true
    }
}

fn wrap_conn(inner: Rc<SqliteConnInner>) -> Value {
    let boxed: Box<dyn Any> = Box::new(SqliteConnHandle(inner));
    Value::builtin_object(SQLITE_CONN_OPS, boxed)
}

fn clone_conn_inner(val: &Value) -> Result<Rc<SqliteConnInner>> {
    let ValueKind::BuiltinObject { ops, state } = val.kind() else {
        return Err(PyError::named(
            "TypeError",
            "expected internal sqlite3 connection handle".to_string(),
        ));
    };
    if !builtin_ops_is::<SqliteConnOps>(ops) {
        return Err(PyError::named(
            "TypeError",
            "expected internal sqlite3 connection handle".to_string(),
        ));
    }
    let borrowed = state.borrow();
    let handle = borrowed.downcast_ref::<SqliteConnHandle>().ok_or_else(|| {
        PyError::runtime("corrupt internal sqlite3 connection handle".to_string())
    })?;
    Ok(Rc::clone(&handle.0))
}

struct SqliteStmtHandle {
    conn: Rc<SqliteConnInner>,
    stmt: Rc<Cell<*mut ffi::sqlite3_stmt>>,
    col_names: Vec<String>,
}

impl SqliteStmtHandle {
    fn close(&self) {
        let stmt = self.stmt.replace(ptr::null_mut());
        if !stmt.is_null() {
            unsafe {
                ffi::sqlite3_finalize(stmt);
            }
        }
    }
}

impl Drop for SqliteStmtHandle {
    fn drop(&mut self) {
        self.close();
    }
}

struct SqliteStmtOps;
const SQLITE_STMT_OPS: &SqliteStmtOps = &SqliteStmtOps;

impl BuiltinTypeOps for SqliteStmtOps {
    fn type_name(&self) -> &'static str {
        "_sqlite_stmt"
    }

    fn is_internal_storage(&self) -> bool {
        true
    }
}

fn wrap_stmt(handle: SqliteStmtHandle) -> Value {
    let boxed: Box<dyn Any> = Box::new(handle);
    Value::builtin_object(SQLITE_STMT_OPS, boxed)
}

type StmtSlot = Rc<Cell<*mut ffi::sqlite3_stmt>>;
type StmtParts = (Rc<SqliteConnInner>, StmtSlot, Vec<String>);

fn get_stmt_parts(val: &Value) -> Result<StmtParts> {
    let ValueKind::BuiltinObject { ops, state } = val.kind() else {
        return Err(PyError::named(
            "TypeError",
            "expected internal sqlite3 statement handle".to_string(),
        ));
    };
    if !builtin_ops_is::<SqliteStmtOps>(ops) {
        return Err(PyError::named(
            "TypeError",
            "expected internal sqlite3 statement handle".to_string(),
        ));
    }
    let borrowed = state.borrow();
    let handle = borrowed
        .downcast_ref::<SqliteStmtHandle>()
        .ok_or_else(|| PyError::runtime("corrupt internal sqlite3 statement handle".to_string()))?;
    Ok((
        Rc::clone(&handle.conn),
        Rc::clone(&handle.stmt),
        handle.col_names.clone(),
    ))
}

struct SqliteBlobHandle {
    conn: Rc<SqliteConnInner>,
    blob: Rc<Cell<*mut ffi::sqlite3_blob>>,
}

impl SqliteBlobHandle {
    fn close(&self) {
        let blob = self.blob.replace(ptr::null_mut());
        if !blob.is_null() {
            unsafe {
                ffi::sqlite3_blob_close(blob);
            }
        }
    }
}

impl Drop for SqliteBlobHandle {
    fn drop(&mut self) {
        self.close();
    }
}

struct SqliteBlobOps;
const SQLITE_BLOB_OPS: &SqliteBlobOps = &SqliteBlobOps;

impl BuiltinTypeOps for SqliteBlobOps {
    fn type_name(&self) -> &'static str {
        "_sqlite_blob"
    }

    fn is_internal_storage(&self) -> bool {
        true
    }
}

fn wrap_blob(handle: SqliteBlobHandle) -> Value {
    let boxed: Box<dyn Any> = Box::new(handle);
    Value::builtin_object(SQLITE_BLOB_OPS, boxed)
}

fn get_blob_parts(val: &Value) -> Result<(Rc<SqliteConnInner>, Rc<Cell<*mut ffi::sqlite3_blob>>)> {
    let ValueKind::BuiltinObject { ops, state } = val.kind() else {
        return Err(PyError::named(
            "TypeError",
            "expected internal sqlite3 blob handle".to_string(),
        ));
    };
    if !builtin_ops_is::<SqliteBlobOps>(ops) {
        return Err(PyError::named(
            "TypeError",
            "expected internal sqlite3 blob handle".to_string(),
        ));
    }
    let borrowed = state.borrow();
    let handle = borrowed
        .downcast_ref::<SqliteBlobHandle>()
        .ok_or_else(|| PyError::runtime("corrupt internal sqlite3 blob handle".to_string()))?;
    Ok((Rc::clone(&handle.conn), Rc::clone(&handle.blob)))
}

// ── Value conversion helpers ─────────────────────────────────────────────────

fn extract_str(val: &Value, what: &str) -> Result<String> {
    match val.kind() {
        ValueKind::Str(s) => Ok(s.to_string()),
        _ => Err(PyError::named(
            "TypeError",
            format!("{what} must be str, not {}", value_type_name_str(val)),
        )),
    }
}

fn extract_i64(val: &Value, what: &str) -> Result<i64> {
    match val.kind() {
        ValueKind::Int(n) => Ok(n),
        ValueKind::Bool(b) => Ok(i64::from(b)),
        _ => Err(PyError::named(
            "TypeError",
            format!("{what} must be int, not {}", value_type_name_str(val)),
        )),
    }
}

fn extract_f64(val: &Value, what: &str) -> Result<f64> {
    match val.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(n) => Ok(n as f64),
        ValueKind::Bool(b) => Ok(f64::from(u8::from(b))),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{what} must be a real number, not {}",
                value_type_name_str(val)
            ),
        )),
    }
}

fn extract_bytes_like(val: &Value) -> Option<Vec<u8>> {
    if let ValueKind::Bytes(rc) = val.kind() {
        return Some(rc.as_ref().clone());
    }
    if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(val) {
        return Some(snap);
    }
    if pyrust_builtins::memoryview::is_memoryview(val) {
        return pyrust_builtins::memoryview::to_bytes_checked(val).ok();
    }
    None
}

fn to_cstring(s: &str, what: &str) -> Result<CString> {
    CString::new(s).map_err(|_| {
        PyError::named(
            "ValueError",
            format!("{what} must not contain embedded null characters"),
        )
    })
}

/// Bind a single Python value at 1-based parameter `index` on `stmt`.
fn bind_value(
    interp: &mut Interpreter,
    db: *mut ffi::sqlite3,
    stmt: *mut ffi::sqlite3_stmt,
    index: c_int,
    val: &Value,
) -> Result<()> {
    let rc = match val.kind() {
        ValueKind::None => unsafe { ffi::sqlite3_bind_null(stmt, index) },
        ValueKind::Bool(b) => unsafe { ffi::sqlite3_bind_int64(stmt, index, i64::from(b)) },
        ValueKind::Int(i) => unsafe { ffi::sqlite3_bind_int64(stmt, index, i) },
        ValueKind::BigInt(bi) => {
            let Ok(i) = i64::try_from(bi) else {
                return Err(PyError::named(
                    "OverflowError",
                    "Python int too large to convert to SQLite INTEGER".to_string(),
                ));
            };
            unsafe { ffi::sqlite3_bind_int64(stmt, index, i) }
        }
        ValueKind::Float(f) => unsafe { ffi::sqlite3_bind_double(stmt, index, f) },
        ValueKind::Str(s) => {
            let bytes = s.as_bytes();
            let Ok(len) = c_int::try_from(bytes.len()) else {
                return Err(raise_sqlite_class(
                    interp,
                    "DataError",
                    "string or blob too big".to_string(),
                    Some(ffi::SQLITE_TOOBIG),
                ));
            };
            let ptr = if bytes.is_empty() {
                c"".as_ptr()
            } else {
                bytes.as_ptr().cast::<c_char>()
            };
            unsafe { ffi::sqlite3_bind_text(stmt, index, ptr, len, ffi::SQLITE_TRANSIENT()) }
        }
        _ => {
            if let Some(bytes) = extract_bytes_like(val) {
                let Ok(len) = c_int::try_from(bytes.len()) else {
                    return Err(raise_sqlite_class(
                        interp,
                        "DataError",
                        "string or blob too big".to_string(),
                        Some(ffi::SQLITE_TOOBIG),
                    ));
                };
                let ptr = if bytes.is_empty() {
                    ptr::NonNull::<u8>::dangling().as_ptr().cast::<c_void>()
                } else {
                    bytes.as_ptr().cast::<c_void>()
                };
                unsafe { ffi::sqlite3_bind_blob(stmt, index, ptr, len, ffi::SQLITE_TRANSIENT()) }
            } else {
                return Err(raise_sqlite_class(
                    interp,
                    "ProgrammingError",
                    format!(
                        "Error binding parameter {}: type '{}' is not supported",
                        index,
                        value_type_name_str(val)
                    ),
                    None,
                ));
            }
        }
    };
    if rc != ffi::SQLITE_OK {
        return Err(raise_db_error(interp, db, rc));
    }
    Ok(())
}

/// Convert a `sqlite3_value*` (from a UDF or aggregate argument) to a Python `Value`.
unsafe fn sqlite_value_to_py(val: *mut ffi::sqlite3_value) -> Value {
    let ty = unsafe { ffi::sqlite3_value_type(val) };
    match ty {
        ffi::SQLITE_NULL => Value::none(),
        ffi::SQLITE_INTEGER => Value::int(unsafe { ffi::sqlite3_value_int64(val) }),
        ffi::SQLITE_FLOAT => Value::float(unsafe { ffi::sqlite3_value_double(val) }),
        ffi::SQLITE_TEXT => {
            let nbytes = unsafe { ffi::sqlite3_value_bytes(val) } as usize;
            if nbytes == 0 {
                return Value::string("");
            }
            let ptr = unsafe { ffi::sqlite3_value_text(val) };
            if ptr.is_null() {
                return Value::string("");
            }
            let bytes = unsafe { slice::from_raw_parts(ptr, nbytes) };
            match std::str::from_utf8(bytes) {
                Ok(s) => Value::string(s),
                Err(_) => Value::bytes(bytes.to_vec()),
            }
        }
        ffi::SQLITE_BLOB => {
            let nbytes = unsafe { ffi::sqlite3_value_bytes(val) } as usize;
            if nbytes == 0 {
                return Value::bytes(Vec::new());
            }
            let ptr = unsafe { ffi::sqlite3_value_blob(val) }.cast::<u8>();
            if ptr.is_null() {
                return Value::bytes(Vec::new());
            }
            let bytes = unsafe { slice::from_raw_parts(ptr, nbytes) };
            Value::bytes(bytes.to_vec())
        }
        _ => Value::none(),
    }
}

/// Set a SQLite function/aggregate return value from a Python `Value`.
unsafe fn set_sqlite_result(ctx: *mut ffi::sqlite3_context, val: &Value) {
    match val.kind() {
        ValueKind::None => unsafe { ffi::sqlite3_result_null(ctx) },
        ValueKind::Bool(b) => unsafe { ffi::sqlite3_result_int64(ctx, i64::from(b)) },
        ValueKind::Int(i) => unsafe { ffi::sqlite3_result_int64(ctx, i) },
        ValueKind::BigInt(bi) => {
            if let Ok(i) = i64::try_from(bi) {
                unsafe { ffi::sqlite3_result_int64(ctx, i) };
            } else {
                let msg = c"string or blob too big";
                unsafe { ffi::sqlite3_result_error_toobig(ctx) };
                let _ = msg;
            }
        }
        ValueKind::Float(f) => unsafe { ffi::sqlite3_result_double(ctx, f) },
        ValueKind::Str(s) => {
            let bytes = s.as_bytes();
            let Ok(len) = c_int::try_from(bytes.len()) else {
                unsafe { ffi::sqlite3_result_error_toobig(ctx) };
                return;
            };
            let ptr = if bytes.is_empty() {
                c"".as_ptr()
            } else {
                bytes.as_ptr().cast::<c_char>()
            };
            unsafe { ffi::sqlite3_result_text(ctx, ptr, len, ffi::SQLITE_TRANSIENT()) };
        }
        _ => {
            if let Some(bytes) = extract_bytes_like(val) {
                let Ok(len) = c_int::try_from(bytes.len()) else {
                    unsafe { ffi::sqlite3_result_error_toobig(ctx) };
                    return;
                };
                let ptr = if bytes.is_empty() {
                    ptr::NonNull::<u8>::dangling().as_ptr().cast::<c_void>()
                } else {
                    bytes.as_ptr().cast::<c_void>()
                };
                unsafe { ffi::sqlite3_result_blob(ctx, ptr, len, ffi::SQLITE_TRANSIENT()) };
            } else {
                let msg = c"user-defined function raised exception";
                unsafe { ffi::sqlite3_result_error(ctx, msg.as_ptr(), -1) };
            }
        }
    }
}

/// Read the current row from `stmt` as a Python tuple, applying column
/// converters (`PARSE_DECLTYPES` / `PARSE_COLNAMES`) and `text_factory`.
fn read_current_row(
    interp: &mut Interpreter,
    stmt: *mut ffi::sqlite3_stmt,
    col_names: &[String],
    text_factory: &Value,
    col_converters: &[Option<Value>],
) -> Result<Value> {
    let col_count = unsafe { ffi::sqlite3_column_count(stmt) } as usize;
    let mut row = Vec::with_capacity(col_count);

    for i in 0..col_count {
        let idx = i as c_int;
        let col_type = unsafe { ffi::sqlite3_column_type(stmt, idx) };
        if col_type == ffi::SQLITE_NULL {
            row.push(Value::none());
            continue;
        }
        // If a converter is configured for this column, pass raw column bytes.
        if let Some(Some(converter)) = col_converters.get(i) {
            let nbytes = unsafe { ffi::sqlite3_column_bytes(stmt, idx) } as usize;
            let raw = if nbytes == 0 {
                Vec::new()
            } else {
                let ptr = unsafe { ffi::sqlite3_column_blob(stmt, idx) }.cast::<u8>();
                if ptr.is_null() {
                    Vec::new()
                } else {
                    unsafe { slice::from_raw_parts(ptr, nbytes) }.to_vec()
                }
            };
            let converted =
                interp.call_function_expanded(converter.clone(), &[pos_arg(Value::bytes(raw))])?;
            row.push(converted);
            continue;
        }

        let cell = match col_type {
            ffi::SQLITE_INTEGER => Value::int(unsafe { ffi::sqlite3_column_int64(stmt, idx) }),
            ffi::SQLITE_FLOAT => Value::float(unsafe { ffi::sqlite3_column_double(stmt, idx) }),
            ffi::SQLITE_BLOB => {
                let nbytes = unsafe { ffi::sqlite3_column_bytes(stmt, idx) } as usize;
                if nbytes == 0 {
                    Value::bytes(Vec::new())
                } else {
                    let ptr = unsafe { ffi::sqlite3_column_blob(stmt, idx) }.cast::<u8>();
                    if ptr.is_null() {
                        Value::bytes(Vec::new())
                    } else {
                        Value::bytes(unsafe { slice::from_raw_parts(ptr, nbytes) }.to_vec())
                    }
                }
            }
            ffi::SQLITE_TEXT => {
                let nbytes = unsafe { ffi::sqlite3_column_bytes(stmt, idx) } as usize;
                let bytes: &[u8] = if nbytes == 0 {
                    &[]
                } else {
                    let ptr = unsafe { ffi::sqlite3_column_text(stmt, idx) };
                    if ptr.is_null() {
                        &[]
                    } else {
                        unsafe { slice::from_raw_parts(ptr, nbytes) }
                    }
                };
                match text_factory.kind() {
                    // None => default UTF-8 str fast path
                    ValueKind::None => match std::str::from_utf8(bytes) {
                        Ok(s) => Value::string(s),
                        Err(_) => {
                            let col_name = col_names.get(i).map_or("", String::as_str);
                            let lossy = String::from_utf8_lossy(bytes);
                            return Err(raise_sqlite_class(
                                interp,
                                "OperationalError",
                                format!(
                                    "Could not decode to UTF-8 column '{col_name}' with text '{lossy}'"
                                ),
                                None,
                            ));
                        }
                    },
                    // True => bytes fast path
                    ValueKind::Bool(true) => Value::bytes(bytes.to_vec()),
                    // Custom callable text_factory
                    _ => interp.call_function_expanded(
                        text_factory.clone(),
                        &[pos_arg(Value::bytes(bytes.to_vec()))],
                    )?,
                }
            }
            _ => Value::none(),
        };
        row.push(cell);
    }

    Ok(Value::tuple(row))
}

// ── SQLite statement inspection helpers ──────────────────────────────────────

/// Return `true` if the SQL text has any non-whitespace, non-comment tokens.
fn has_remaining_sql(tail: &str) -> bool {
    let bytes = tail.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Line comment: -- ...\n
        if b == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'-' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Block comment: /* ... */
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        return true;
    }
    false
}

/// Extract the leading SQL keyword (lowercased) after skipping whitespace and comments.
fn leading_sql_keyword(sql: &str) -> String {
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'-' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        break;
    }
    let start = i;
    while i < bytes.len() && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
        i += 1;
    }
    sql[start..i].to_ascii_lowercase()
}

fn is_dml_statement(sql: &str) -> bool {
    matches!(
        leading_sql_keyword(sql).as_str(),
        "insert" | "update" | "delete" | "replace"
    )
}

// ── SQLite C callback trampolines ────────────────────────────────────────────

struct FuncCallback {
    func: Value,
}

unsafe extern "C" fn destroy_func_callback(ptr: *mut c_void) {
    if !ptr.is_null() {
        unsafe {
            drop(Box::from_raw(ptr.cast::<FuncCallback>()));
        }
    }
}

unsafe extern "C" fn scalar_func_trampoline(
    ctx: *mut ffi::sqlite3_context,
    argc: c_int,
    argv: *mut *mut ffi::sqlite3_value,
) {
    let user_data = unsafe { ffi::sqlite3_user_data(ctx) };
    if user_data.is_null() {
        unsafe {
            ffi::sqlite3_result_error(ctx, c"user-defined function raised exception".as_ptr(), -1);
        }
        return;
    }
    let cb = unsafe { &*user_data.cast::<FuncCallback>() };
    let func = cb.func.clone();

    let mut py_args = Vec::with_capacity(argc.max(0) as usize);
    for i in 0..argc {
        let arg_ptr = unsafe { *argv.add(i as usize) };
        py_args.push(pos_arg(unsafe { sqlite_value_to_py(arg_ptr) }));
    }

    let call_res = with_active_interp(|interp| interp.call_function_expanded(func, &py_args));
    match call_res {
        Some(Ok(val)) => unsafe { set_sqlite_result(ctx, &val) },
        Some(Err(err)) => {
            maybe_print_callback_error(&err);
            unsafe {
                ffi::sqlite3_result_error(
                    ctx,
                    c"user-defined function raised exception".as_ptr(),
                    -1,
                );
            }
        }
        None => unsafe {
            ffi::sqlite3_result_error(ctx, c"user-defined function raised exception".as_ptr(), -1);
        },
    }
}

struct AggCallback {
    cls: Value,
}

#[repr(C)]
struct AggStateSlot {
    inst: *mut Value,
}

unsafe extern "C" fn destroy_agg_callback(ptr: *mut c_void) {
    if !ptr.is_null() {
        unsafe {
            drop(Box::from_raw(ptr.cast::<AggCallback>()));
        }
    }
}

unsafe extern "C" fn agg_step_trampoline(
    ctx: *mut ffi::sqlite3_context,
    argc: c_int,
    argv: *mut *mut ffi::sqlite3_value,
) {
    let slot_ptr = unsafe {
        ffi::sqlite3_aggregate_context(ctx, std::mem::size_of::<AggStateSlot>() as c_int)
    }
    .cast::<AggStateSlot>();
    if slot_ptr.is_null() {
        return;
    }
    let slot = unsafe { &mut *slot_ptr };

    // Instantiate aggregate class on first step.
    if slot.inst.is_null() {
        let user_data = unsafe { ffi::sqlite3_user_data(ctx) };
        if user_data.is_null() {
            unsafe {
                ffi::sqlite3_result_error(
                    ctx,
                    c"user-defined aggregate's '__init__' method raised error".as_ptr(),
                    -1,
                );
            }
            return;
        }
        let cb = unsafe { &*user_data.cast::<AggCallback>() };
        let cls = cb.cls.clone();
        match with_active_interp(|interp| interp.call_function_expanded(cls, &[])) {
            Some(Ok(inst)) => {
                slot.inst = Box::into_raw(Box::new(inst));
            }
            Some(Err(err)) => {
                maybe_print_callback_error(&err);
                unsafe {
                    ffi::sqlite3_result_error(
                        ctx,
                        c"user-defined aggregate's '__init__' method raised error".as_ptr(),
                        -1,
                    );
                }
                return;
            }
            None => {
                unsafe {
                    ffi::sqlite3_result_error(
                        ctx,
                        c"user-defined aggregate's '__init__' method raised error".as_ptr(),
                        -1,
                    );
                }
                return;
            }
        }
    }

    let inst = unsafe { (*slot.inst).clone() };
    let mut py_args = Vec::with_capacity(argc.max(0) as usize);
    for i in 0..argc {
        let arg_ptr = unsafe { *argv.add(i as usize) };
        py_args.push(pos_arg(unsafe { sqlite_value_to_py(arg_ptr) }));
    }

    let step_res = with_active_interp(|interp| {
        let step_method = interp.get_attr(&inst, "step")?;
        interp.call_function_expanded(step_method, &py_args)
    });
    match step_res {
        Some(Ok(_)) => {}
        Some(Err(err)) => {
            maybe_print_callback_error(&err);
            unsafe {
                ffi::sqlite3_result_error(
                    ctx,
                    c"user-defined aggregate's 'step' method raised error".as_ptr(),
                    -1,
                );
            }
        }
        None => unsafe {
            ffi::sqlite3_result_error(
                ctx,
                c"user-defined aggregate's 'step' method raised error".as_ptr(),
                -1,
            );
        },
    }
}

unsafe extern "C" fn agg_final_trampoline(ctx: *mut ffi::sqlite3_context) {
    let slot_ptr = unsafe { ffi::sqlite3_aggregate_context(ctx, 0) }.cast::<AggStateSlot>();
    if slot_ptr.is_null() {
        // No rows were processed: SQLite returns NULL by default.
        return;
    }
    let slot = unsafe { &mut *slot_ptr };
    if slot.inst.is_null() {
        return;
    }
    let inst_box = unsafe { Box::from_raw(slot.inst) };
    slot.inst = ptr::null_mut();

    let fin_res = with_active_interp(|interp| {
        let fin_method = interp.get_attr(&inst_box, "finalize")?;
        interp.call_function_expanded(fin_method, &[])
    });
    match fin_res {
        Some(Ok(val)) => unsafe { set_sqlite_result(ctx, &val) },
        Some(Err(err)) => {
            maybe_print_callback_error(&err);
            unsafe {
                ffi::sqlite3_result_error(
                    ctx,
                    c"user-defined aggregate's 'finalize' method raised error".as_ptr(),
                    -1,
                );
            }
        }
        None => unsafe {
            ffi::sqlite3_result_error(
                ctx,
                c"user-defined aggregate's 'finalize' method raised error".as_ptr(),
                -1,
            );
        },
    }
}

unsafe extern "C" fn collation_trampoline(
    user_data: *mut c_void,
    len1: c_int,
    ptr1: *const c_void,
    len2: c_int,
    ptr2: *const c_void,
) -> c_int {
    if user_data.is_null() {
        return 0;
    }
    // If an earlier collation call already failed, short-circuit.
    if CALLBACK_ERROR.with(|c| c.borrow().is_some()) {
        return 0;
    }
    let cb = unsafe { &*user_data.cast::<FuncCallback>() };
    let func = cb.func.clone();

    let decode_arg = |ptr: *const c_void, len: c_int| -> Value {
        if len <= 0 || ptr.is_null() {
            return Value::string("");
        }
        let bytes = unsafe { slice::from_raw_parts(ptr.cast::<u8>(), len as usize) };
        match std::str::from_utf8(bytes) {
            Ok(s) => Value::string(s),
            Err(_) => Value::bytes(bytes.to_vec()),
        }
    };
    let s1 = decode_arg(ptr1, len1);
    let s2 = decode_arg(ptr2, len2);

    let res = with_active_interp(|interp| {
        interp.call_function_expanded(func, &[pos_arg(s1), pos_arg(s2)])
    });
    match res {
        Some(Ok(val)) => match val.kind() {
            ValueKind::Int(n) => {
                if n > 0 {
                    1
                } else if n < 0 {
                    -1
                } else {
                    0
                }
            }
            ValueKind::Bool(b) => c_int::from(b),
            ValueKind::BigInt(bi) => {
                use num_traits::Signed;
                if bi.is_positive() {
                    1
                } else if bi.is_negative() {
                    -1
                } else {
                    0
                }
            }
            _ => {
                stash_callback_error(PyError::named(
                    "TypeError",
                    format!(
                        "collation callback must return int, not {}",
                        value_type_name_str(&val)
                    ),
                ));
                0
            }
        },
        Some(Err(err)) => {
            maybe_print_callback_error(&err);
            stash_callback_error(err);
            0
        }
        None => 0,
    }
}

unsafe extern "C" fn authorizer_trampoline(
    user_data: *mut c_void,
    action: c_int,
    arg1: *const c_char,
    arg2: *const c_char,
    dbname: *const c_char,
    source: *const c_char,
) -> c_int {
    if user_data.is_null() {
        return ffi::SQLITE_OK;
    }
    let conn = unsafe { &*user_data.cast::<SqliteConnInner>() };
    let func_opt = conn.authorizer.borrow().clone();
    let Some(func) = func_opt else {
        return ffi::SQLITE_OK;
    };
    let opt_str = |p: *const c_char| -> Value {
        if p.is_null() {
            Value::none()
        } else {
            Value::string(unsafe { CStr::from_ptr(p) }.to_string_lossy())
        }
    };
    let args = [
        pos_arg(Value::int(i64::from(action))),
        pos_arg(opt_str(arg1)),
        pos_arg(opt_str(arg2)),
        pos_arg(opt_str(dbname)),
        pos_arg(opt_str(source)),
    ];
    match with_active_interp(|interp| interp.call_function_expanded(func, &args)) {
        Some(Ok(val)) => match val.kind() {
            ValueKind::Int(n) => n as c_int,
            ValueKind::Bool(b) => c_int::from(b),
            _ => ffi::SQLITE_DENY,
        },
        Some(Err(err)) => {
            maybe_print_callback_error(&err);
            ffi::SQLITE_DENY
        }
        None => ffi::SQLITE_DENY,
    }
}

unsafe extern "C" fn progress_trampoline(user_data: *mut c_void) -> c_int {
    if user_data.is_null() {
        return 0;
    }
    let conn = unsafe { &*user_data.cast::<SqliteConnInner>() };
    let func_opt = conn.progress_handler.borrow().clone();
    let Some(func) = func_opt else {
        return 0;
    };
    match with_active_interp(|interp| {
        let v = interp.call_function_expanded(func, &[])?;
        interp.truthy_value(&v)
    }) {
        Some(Ok(true)) => 1,
        Some(Ok(false)) => 0,
        Some(Err(err)) => {
            maybe_print_callback_error(&err);
            1
        }
        None => 0,
    }
}

unsafe extern "C" fn trace_trampoline(
    _mask: u32,
    user_data: *mut c_void,
    p: *mut c_void,
    _x: *mut c_void,
) -> c_int {
    if user_data.is_null() || p.is_null() {
        return 0;
    }
    let conn = unsafe { &*user_data.cast::<SqliteConnInner>() };
    let func_opt = conn.trace_callback.borrow().clone();
    let Some(func) = func_opt else {
        return 0;
    };
    let expanded = unsafe { ffi::sqlite3_expanded_sql(p.cast::<ffi::sqlite3_stmt>()) };
    let sql_str = if !expanded.is_null() {
        let s = unsafe { CStr::from_ptr(expanded) }
            .to_string_lossy()
            .into_owned();
        unsafe { ffi::sqlite3_free(expanded.cast::<c_void>()) };
        s
    } else {
        let raw = unsafe { ffi::sqlite3_sql(p.cast::<ffi::sqlite3_stmt>()) };
        if raw.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(raw) }
                .to_string_lossy()
                .into_owned()
        }
    };
    let res = with_active_interp(|interp| {
        interp.call_function_expanded(func, &[pos_arg(Value::string(&sql_str))])
    });
    if let Some(Err(err)) = res {
        maybe_print_callback_error(&err);
    }
    0
}

// ── Native helper implementations ────────────────────────────────────────────

fn conn_open_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let path = extract_str(&args[0].value, "database")?;
    let timeout_ms = extract_i64(&args[1].value, "timeout_ms")? as c_int;
    let uri = interp.truthy_value(&args[2].value)?;

    let c_path = to_cstring(&path, "database")?;
    let mut flags = ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE;
    if uri {
        flags |= ffi::SQLITE_OPEN_URI;
    }

    let mut db: *mut ffi::sqlite3 = ptr::null_mut();
    let rc = unsafe { ffi::sqlite3_open_v2(c_path.as_ptr(), &mut db, flags, ptr::null()) };
    if rc != ffi::SQLITE_OK {
        let err = raise_db_error(interp, db, rc);
        if !db.is_null() {
            unsafe {
                ffi::sqlite3_close(db);
            }
        }
        return Err(err);
    }

    unsafe {
        ffi::sqlite3_extended_result_codes(db, 1);
        ffi::sqlite3_busy_timeout(db, timeout_ms.max(0));
    }

    Ok(wrap_conn(SqliteConnInner::new(db)))
}

fn conn_exec_simple_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let conn = clone_conn_inner(&args[0].value)?;
    let sql = extract_str(&args[1].value, "sql")?;
    let db = conn.db.get();
    if db.is_null() {
        return Err(closed_db_error(interp));
    }
    let c_sql = to_cstring(&sql, "sql")?;
    let _guard = InterpGuard::new(interp);
    let _ = take_callback_error();
    let rc =
        unsafe { ffi::sqlite3_exec(db, c_sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut()) };
    drop(_guard);
    if rc != ffi::SQLITE_OK {
        return Err(raise_db_error(interp, db, rc));
    }
    if let Some(cb_err) = take_callback_error() {
        return Err(cb_err);
    }
    Ok(Value::none())
}

fn conn_executescript_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let conn = clone_conn_inner(&args[0].value)?;
    let sql = extract_str(&args[1].value, "sql_script")?;
    let db = conn.db.get();
    if db.is_null() {
        return Err(closed_db_error(interp));
    }
    let c_sql = to_cstring(&sql, "sql_script")?;
    let _guard = InterpGuard::new(interp);
    let _ = take_callback_error();

    let mut cur_ptr = c_sql.as_ptr();
    loop {
        if unsafe { *cur_ptr } == 0 {
            break;
        }
        let mut stmt: *mut ffi::sqlite3_stmt = ptr::null_mut();
        let mut tail: *const c_char = ptr::null();
        let rc = unsafe { ffi::sqlite3_prepare_v2(db, cur_ptr, -1, &mut stmt, &mut tail) };
        if rc != ffi::SQLITE_OK {
            drop(_guard);
            return Err(raise_db_error(interp, db, rc));
        }
        if !stmt.is_null() {
            loop {
                let step_rc = unsafe { ffi::sqlite3_step(stmt) };
                if step_rc == ffi::SQLITE_ROW {
                    continue;
                }
                if step_rc == ffi::SQLITE_DONE {
                    break;
                }
                unsafe {
                    ffi::sqlite3_finalize(stmt);
                }
                drop(_guard);
                return Err(raise_db_error(interp, db, step_rc));
            }
            let fin_rc = unsafe { ffi::sqlite3_finalize(stmt) };
            if fin_rc != ffi::SQLITE_OK {
                drop(_guard);
                return Err(raise_db_error(interp, db, fin_rc));
            }
        }
        if tail.is_null() || tail == cur_ptr {
            break;
        }
        cur_ptr = tail;
    }

    drop(_guard);
    if let Some(cb_err) = take_callback_error() {
        return Err(cb_err);
    }
    Ok(Value::none())
}

fn conn_create_function_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let conn = clone_conn_inner(&args[0].value)?;
    let name = extract_str(&args[1].value, "name")?;
    let narg = extract_i64(&args[2].value, "narg")? as c_int;
    let func = args[3].value.clone();
    let deterministic = interp.truthy_value(&args[4].value)?;

    let db = conn.db.get();
    if db.is_null() {
        return Err(closed_db_error(interp));
    }
    let c_name = to_cstring(&name, "name")?;
    let mut flags = ffi::SQLITE_UTF8;
    if deterministic {
        flags |= ffi::SQLITE_DETERMINISTIC;
    }

    let rc = if matches!(func.kind(), ValueKind::None) {
        unsafe {
            ffi::sqlite3_create_function_v2(
                db,
                c_name.as_ptr(),
                narg,
                flags,
                ptr::null_mut(),
                None,
                None,
                None,
                None,
            )
        }
    } else {
        let user_data = Box::into_raw(Box::new(FuncCallback { func })).cast::<c_void>();
        unsafe {
            ffi::sqlite3_create_function_v2(
                db,
                c_name.as_ptr(),
                narg,
                flags,
                user_data,
                Some(scalar_func_trampoline),
                None,
                None,
                Some(destroy_func_callback),
            )
        }
    };

    if rc != ffi::SQLITE_OK {
        return Err(raise_db_error(interp, db, rc));
    }
    Ok(Value::none())
}

fn conn_create_aggregate_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let conn = clone_conn_inner(&args[0].value)?;
    let name = extract_str(&args[1].value, "name")?;
    let narg = extract_i64(&args[2].value, "narg")? as c_int;
    let cls = args[3].value.clone();

    let db = conn.db.get();
    if db.is_null() {
        return Err(closed_db_error(interp));
    }
    let c_name = to_cstring(&name, "name")?;

    let rc = if matches!(cls.kind(), ValueKind::None) {
        unsafe {
            ffi::sqlite3_create_function_v2(
                db,
                c_name.as_ptr(),
                narg,
                ffi::SQLITE_UTF8,
                ptr::null_mut(),
                None,
                None,
                None,
                None,
            )
        }
    } else {
        let user_data = Box::into_raw(Box::new(AggCallback { cls })).cast::<c_void>();
        unsafe {
            ffi::sqlite3_create_function_v2(
                db,
                c_name.as_ptr(),
                narg,
                ffi::SQLITE_UTF8,
                user_data,
                None,
                Some(agg_step_trampoline),
                Some(agg_final_trampoline),
                Some(destroy_agg_callback),
            )
        }
    };

    if rc != ffi::SQLITE_OK {
        return Err(raise_db_error(interp, db, rc));
    }
    Ok(Value::none())
}

fn conn_create_collation_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let conn = clone_conn_inner(&args[0].value)?;
    let name = extract_str(&args[1].value, "name")?;
    let func = args[2].value.clone();

    let db = conn.db.get();
    if db.is_null() {
        return Err(closed_db_error(interp));
    }
    let c_name = to_cstring(&name, "name")?;

    let rc = if matches!(func.kind(), ValueKind::None) {
        unsafe {
            ffi::sqlite3_create_collation_v2(
                db,
                c_name.as_ptr(),
                ffi::SQLITE_UTF8,
                ptr::null_mut(),
                None,
                None,
            )
        }
    } else {
        let user_data = Box::into_raw(Box::new(FuncCallback { func })).cast::<c_void>();
        unsafe {
            ffi::sqlite3_create_collation_v2(
                db,
                c_name.as_ptr(),
                ffi::SQLITE_UTF8,
                user_data,
                Some(collation_trampoline),
                Some(destroy_func_callback),
            )
        }
    };

    if rc != ffi::SQLITE_OK {
        return Err(raise_db_error(interp, db, rc));
    }
    Ok(Value::none())
}

fn conn_set_authorizer_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let conn = clone_conn_inner(&args[0].value)?;
    let func = args[1].value.clone();
    let db = conn.db.get();
    if db.is_null() {
        return Err(closed_db_error(interp));
    }
    let rc = if matches!(func.kind(), ValueKind::None) {
        *conn.authorizer.borrow_mut() = None;
        unsafe { ffi::sqlite3_set_authorizer(db, None, ptr::null_mut()) }
    } else {
        *conn.authorizer.borrow_mut() = Some(func);
        let ptr = Rc::as_ptr(&conn) as *mut c_void;
        unsafe { ffi::sqlite3_set_authorizer(db, Some(authorizer_trampoline), ptr) }
    };
    if rc != ffi::SQLITE_OK {
        return Err(raise_db_error(interp, db, rc));
    }
    Ok(Value::none())
}

fn conn_set_progress_handler_impl(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let conn = clone_conn_inner(&args[0].value)?;
    let func = args[1].value.clone();
    let n = extract_i64(&args[2].value, "n")? as c_int;
    let db = conn.db.get();
    if db.is_null() {
        return Err(closed_db_error(interp));
    }
    if matches!(func.kind(), ValueKind::None) || n <= 0 {
        *conn.progress_handler.borrow_mut() = None;
        unsafe {
            ffi::sqlite3_progress_handler(db, 0, None, ptr::null_mut());
        }
    } else {
        *conn.progress_handler.borrow_mut() = Some(func);
        let ptr = Rc::as_ptr(&conn) as *mut c_void;
        unsafe {
            ffi::sqlite3_progress_handler(db, n, Some(progress_trampoline), ptr);
        }
    }
    Ok(Value::none())
}

fn conn_set_trace_callback_impl(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let conn = clone_conn_inner(&args[0].value)?;
    let func = args[1].value.clone();
    let db = conn.db.get();
    if db.is_null() {
        return Err(closed_db_error(interp));
    }
    if matches!(func.kind(), ValueKind::None) {
        *conn.trace_callback.borrow_mut() = None;
        unsafe {
            ffi::sqlite3_trace_v2(db, 0, None, ptr::null_mut());
        }
    } else {
        *conn.trace_callback.borrow_mut() = Some(func);
        let ptr = Rc::as_ptr(&conn) as *mut c_void;
        unsafe {
            ffi::sqlite3_trace_v2(db, ffi::SQLITE_TRACE_STMT, Some(trace_trampoline), ptr);
        }
    }
    Ok(Value::none())
}

fn conn_backup_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let src_conn = clone_conn_inner(&args[0].value)?;
    let dst_conn = clone_conn_inner(&args[1].value)?;
    let pages = extract_i64(&args[2].value, "pages")? as c_int;
    let progress = args[3].value.clone();
    let name = extract_str(&args[4].value, "name")?;
    let sleep_secs = extract_f64(&args[5].value, "sleep")?;

    let src_db = src_conn.db.get();
    if src_db.is_null() {
        return Err(closed_db_error(interp));
    }
    let dst_db = dst_conn.db.get();
    if dst_db.is_null() {
        return Err(closed_db_error(interp));
    }
    if src_db == dst_db {
        return Err(PyError::named(
            "ValueError",
            "target cannot be the same connection instance".to_string(),
        ));
    }

    let c_name = to_cstring(&name, "name")?;
    let c_main = c"main";
    let backup =
        unsafe { ffi::sqlite3_backup_init(dst_db, c_main.as_ptr(), src_db, c_name.as_ptr()) };
    if backup.is_null() {
        return Err(raise_db_error(interp, dst_db, ffi::SQLITE_ERROR));
    }

    let step_pages = if pages <= 0 { -1 } else { pages };
    let sleep_dur = if sleep_secs > 0.0 && sleep_secs.is_finite() {
        Duration::from_secs_f64(sleep_secs)
    } else {
        Duration::ZERO
    };

    let mut progress_err: Option<PyError> = None;
    loop {
        let rc = unsafe { ffi::sqlite3_backup_step(backup, step_pages) };
        if !matches!(progress.kind(), ValueKind::None) {
            let remaining = unsafe { ffi::sqlite3_backup_remaining(backup) };
            let pagecount = unsafe { ffi::sqlite3_backup_pagecount(backup) };
            if let Err(err) = interp.call_function_expanded(
                progress.clone(),
                &[
                    pos_arg(Value::int(i64::from(rc))),
                    pos_arg(Value::int(i64::from(remaining))),
                    pos_arg(Value::int(i64::from(pagecount))),
                ],
            ) {
                progress_err = Some(err);
                break;
            }
        }
        if rc == ffi::SQLITE_BUSY || rc == ffi::SQLITE_LOCKED {
            if !sleep_dur.is_zero() {
                thread::sleep(sleep_dur);
            }
            continue;
        }
        if rc != ffi::SQLITE_OK {
            break;
        }
    }

    let fin_rc = unsafe { ffi::sqlite3_backup_finish(backup) };
    if let Some(err) = progress_err {
        return Err(err);
    }
    if fin_rc != ffi::SQLITE_OK {
        return Err(raise_db_error(interp, dst_db, fin_rc));
    }
    Ok(Value::none())
}

fn conn_serialize_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let conn = clone_conn_inner(&args[0].value)?;
    let name = extract_str(&args[1].value, "name")?;
    let db = conn.db.get();
    if db.is_null() {
        return Err(closed_db_error(interp));
    }
    let c_name = to_cstring(&name, "name")?;
    let mut size: ffi::sqlite3_int64 = 0;
    let ptr = unsafe { ffi::sqlite3_serialize(db, c_name.as_ptr(), &mut size, 0) };
    if ptr.is_null() {
        return Err(raise_sqlite_class(
            interp,
            "OperationalError",
            format!("unable to serialize database '{name}'"),
            None,
        ));
    }
    let bytes = if size <= 0 {
        Vec::new()
    } else {
        unsafe { slice::from_raw_parts(ptr, size as usize) }.to_vec()
    };
    unsafe {
        ffi::sqlite3_free(ptr.cast::<c_void>());
    }
    Ok(Value::bytes(bytes))
}

fn conn_deserialize_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let conn = clone_conn_inner(&args[0].value)?;
    let Some(data) = extract_bytes_like(&args[1].value) else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "data must be bytes-like, not {}",
                value_type_name_str(&args[1].value)
            ),
        ));
    };
    let name = extract_str(&args[2].value, "name")?;
    let db = conn.db.get();
    if db.is_null() {
        return Err(closed_db_error(interp));
    }
    let c_name = to_cstring(&name, "name")?;
    let len = data.len();
    let buf = unsafe { ffi::sqlite3_malloc64(len.max(1) as u64) }.cast::<u8>();
    if buf.is_null() {
        return Err(PyError::named("MemoryError", "out of memory".to_string()));
    }
    if len > 0 {
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), buf, len);
        }
    }
    let flags = ffi::SQLITE_DESERIALIZE_FREEONCLOSE | ffi::SQLITE_DESERIALIZE_RESIZEABLE;
    let rc = unsafe {
        ffi::sqlite3_deserialize(db, c_name.as_ptr(), buf, len as i64, len as i64, flags)
    };
    if rc != ffi::SQLITE_OK {
        unsafe {
            ffi::sqlite3_free(buf.cast::<c_void>());
        }
        return Err(raise_db_error(interp, db, rc));
    }
    Ok(Value::none())
}

fn stmt_prepare_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let conn = clone_conn_inner(&args[0].value)?;
    let sql = extract_str(&args[1].value, "sql")?;
    let db = conn.db.get();
    if db.is_null() {
        return Err(closed_db_error(interp));
    }
    let c_sql = to_cstring(&sql, "sql")?;
    let mut stmt: *mut ffi::sqlite3_stmt = ptr::null_mut();
    let mut tail: *const c_char = ptr::null();

    let _guard = InterpGuard::new(interp);
    let _ = take_callback_error();
    let rc = unsafe { ffi::sqlite3_prepare_v2(db, c_sql.as_ptr(), -1, &mut stmt, &mut tail) };
    drop(_guard);

    if rc != ffi::SQLITE_OK {
        if !stmt.is_null() {
            unsafe {
                ffi::sqlite3_finalize(stmt);
            }
        }
        return Err(raise_db_error(interp, db, rc));
    }

    // Check trailing SQL statements (CPython rejects multiple statements in execute()).
    if !tail.is_null() {
        let tail_str = unsafe { CStr::from_ptr(tail) }.to_string_lossy();
        if has_remaining_sql(&tail_str) {
            if !stmt.is_null() {
                unsafe {
                    ffi::sqlite3_finalize(stmt);
                }
            }
            return Err(raise_sqlite_class(
                interp,
                "ProgrammingError",
                "You can only execute one statement at a time.".to_string(),
                None,
            ));
        }
    }

    // Empty or comment-only SQL.
    if stmt.is_null() {
        return Ok(Value::tuple(vec![
            Value::none(),
            Value::bool_(false),
            Value::int(0),
            Value::list(Vec::new()),
            Value::int(0),
            Value::list(Vec::new()),
        ]));
    }

    let is_dml = is_dml_statement(&sql);
    let col_count = unsafe { ffi::sqlite3_column_count(stmt) };
    let mut col_names = Vec::with_capacity(col_count.max(0) as usize);
    let mut col_meta = Vec::with_capacity(col_count.max(0) as usize);
    for i in 0..col_count {
        let name_ptr = unsafe { ffi::sqlite3_column_name(stmt, i) };
        let col_name = if name_ptr.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(name_ptr) }
                .to_string_lossy()
                .into_owned()
        };
        let decl_ptr = unsafe { ffi::sqlite3_column_decltype(stmt, i) };
        let decl_val = if decl_ptr.is_null() {
            Value::none()
        } else {
            Value::string(unsafe { CStr::from_ptr(decl_ptr) }.to_string_lossy())
        };
        col_names.push(col_name.clone());
        col_meta.push(Value::tuple(vec![Value::string(&col_name), decl_val]));
    }

    let param_count = unsafe { ffi::sqlite3_bind_parameter_count(stmt) };
    let mut param_names = Vec::with_capacity(param_count.max(0) as usize);
    for i in 1..=param_count {
        let name_ptr = unsafe { ffi::sqlite3_bind_parameter_name(stmt, i) };
        if name_ptr.is_null() {
            param_names.push(Value::none());
        } else {
            param_names.push(Value::string(
                unsafe { CStr::from_ptr(name_ptr) }.to_string_lossy(),
            ));
        }
    }

    let slot = Rc::new(Cell::new(stmt));
    conn.register_stmt(&slot);
    let handle = SqliteStmtHandle {
        conn,
        stmt: slot,
        col_names,
    };

    Ok(Value::tuple(vec![
        wrap_stmt(handle),
        Value::bool_(is_dml),
        Value::int(i64::from(col_count)),
        Value::list(col_meta),
        Value::int(i64::from(param_count)),
        Value::list(param_names),
    ]))
}

fn extract_converters_list(val: &Value) -> Vec<Option<Value>> {
    match val.kind() {
        ValueKind::List(list_ref) => list_ref
            .iter()
            .map(|v| {
                if matches!(v.kind(), ValueKind::None) {
                    None
                } else {
                    Some(v.clone())
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn stmt_bind_and_step_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (conn, stmt_slot, col_names) = get_stmt_parts(&args[0].value)?;
    let params = &args[1].value;
    let text_factory = &args[2].value;
    let col_converters = extract_converters_list(&args[3].value);

    let db = conn.db.get();
    if db.is_null() {
        return Err(closed_db_error(interp));
    }
    let stmt = stmt_slot.get();
    if stmt.is_null() {
        return Err(closed_cursor_error(interp));
    }

    // Reset & clear previous bindings so the statement can be re-executed (e.g. executemany).
    unsafe {
        ffi::sqlite3_reset(stmt);
        ffi::sqlite3_clear_bindings(stmt);
    }

    // Bind positional list of adapted values (length already verified in Python).
    if let ValueKind::List(list_ref) = params.kind() {
        let items: Vec<Value> = list_ref.clone();
        drop(list_ref);
        for (idx, val) in items.iter().enumerate() {
            bind_value(interp, db, stmt, (idx + 1) as c_int, val)?;
        }
    }

    let _guard = InterpGuard::new(interp);
    let _ = take_callback_error();
    let step_rc = unsafe { ffi::sqlite3_step(stmt) };
    drop(_guard);

    if let Some(cb_err) = take_callback_error() {
        unsafe {
            ffi::sqlite3_reset(stmt);
        }
        return Err(cb_err);
    }

    match step_rc {
        ffi::SQLITE_ROW => {
            let row = read_current_row(interp, stmt, &col_names, text_factory, &col_converters)?;
            Ok(Value::tuple(vec![
                Value::bool_(true),
                row,
                Value::int(0),
                Value::int(unsafe { ffi::sqlite3_last_insert_rowid(db) }),
            ]))
        }
        ffi::SQLITE_DONE => {
            let changes = unsafe { ffi::sqlite3_changes(db) };
            let lastrowid = unsafe { ffi::sqlite3_last_insert_rowid(db) };
            unsafe {
                ffi::sqlite3_reset(stmt);
            }
            Ok(Value::tuple(vec![
                Value::bool_(false),
                Value::none(),
                Value::int(i64::from(changes)),
                Value::int(lastrowid),
            ]))
        }
        _ => {
            let err = raise_db_error(interp, db, step_rc);
            unsafe {
                ffi::sqlite3_reset(stmt);
            }
            Err(err)
        }
    }
}

fn stmt_step_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (conn, stmt_slot, col_names) = get_stmt_parts(&args[0].value)?;
    let text_factory = &args[1].value;
    let col_converters = extract_converters_list(&args[2].value);

    let db = conn.db.get();
    if db.is_null() {
        return Err(closed_db_error(interp));
    }
    let stmt = stmt_slot.get();
    if stmt.is_null() {
        return Err(closed_cursor_error(interp));
    }

    let _guard = InterpGuard::new(interp);
    let _ = take_callback_error();
    let step_rc = unsafe { ffi::sqlite3_step(stmt) };
    drop(_guard);

    if let Some(cb_err) = take_callback_error() {
        unsafe {
            ffi::sqlite3_reset(stmt);
        }
        return Err(cb_err);
    }

    match step_rc {
        ffi::SQLITE_ROW => {
            let row = read_current_row(interp, stmt, &col_names, text_factory, &col_converters)?;
            Ok(Value::tuple(vec![Value::bool_(true), row]))
        }
        ffi::SQLITE_DONE => {
            unsafe {
                ffi::sqlite3_reset(stmt);
            }
            Ok(Value::tuple(vec![Value::bool_(false), Value::none()]))
        }
        _ => {
            let err = raise_db_error(interp, db, step_rc);
            unsafe {
                ffi::sqlite3_reset(stmt);
            }
            Err(err)
        }
    }
}

fn blob_open_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let conn = clone_conn_inner(&args[0].value)?;
    let db_name = extract_str(&args[1].value, "name")?;
    let table = extract_str(&args[2].value, "table")?;
    let column = extract_str(&args[3].value, "column")?;
    let rowid = extract_i64(&args[4].value, "row")?;
    let readonly = interp.truthy_value(&args[5].value)?;

    let db = conn.db.get();
    if db.is_null() {
        return Err(closed_db_error(interp));
    }
    let c_db = to_cstring(&db_name, "name")?;
    let c_table = to_cstring(&table, "table")?;
    let c_col = to_cstring(&column, "column")?;
    let flags = if readonly { 0 } else { 1 };

    let mut blob: *mut ffi::sqlite3_blob = ptr::null_mut();
    let rc = unsafe {
        ffi::sqlite3_blob_open(
            db,
            c_db.as_ptr(),
            c_table.as_ptr(),
            c_col.as_ptr(),
            rowid,
            flags,
            &mut blob,
        )
    };
    if rc != ffi::SQLITE_OK {
        return Err(raise_db_error(interp, db, rc));
    }
    let slot = Rc::new(Cell::new(blob));
    conn.register_blob(&slot);
    Ok(wrap_blob(SqliteBlobHandle { conn, blob: slot }))
}

fn blob_len_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (conn, slot) = get_blob_parts(&args[0].value)?;
    if conn.db.get().is_null() {
        return Err(closed_db_error(interp));
    }
    let blob = slot.get();
    if blob.is_null() {
        return Err(closed_blob_error(interp));
    }
    let len = unsafe { ffi::sqlite3_blob_bytes(blob) };
    Ok(Value::int(i64::from(len)))
}

fn blob_read_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (conn, slot) = get_blob_parts(&args[0].value)?;
    let offset = extract_i64(&args[1].value, "offset")?;
    let length = extract_i64(&args[2].value, "length")?;
    let db = conn.db.get();
    if db.is_null() {
        return Err(closed_db_error(interp));
    }
    let blob = slot.get();
    if blob.is_null() {
        return Err(closed_blob_error(interp));
    }
    if length <= 0 {
        return Ok(Value::bytes(Vec::new()));
    }
    let mut buf = vec![0u8; length as usize];
    let rc = unsafe {
        ffi::sqlite3_blob_read(
            blob,
            buf.as_mut_ptr().cast::<c_void>(),
            length as c_int,
            offset as c_int,
        )
    };
    if rc != ffi::SQLITE_OK {
        return Err(raise_db_error(interp, db, rc));
    }
    Ok(Value::bytes(buf))
}

fn blob_write_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (conn, slot) = get_blob_parts(&args[0].value)?;
    let offset = extract_i64(&args[1].value, "offset")?;
    let Some(data) = extract_bytes_like(&args[2].value) else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "a bytes-like object is required, not '{}'",
                value_type_name_str(&args[2].value)
            ),
        ));
    };
    let db = conn.db.get();
    if db.is_null() {
        return Err(closed_db_error(interp));
    }
    let blob = slot.get();
    if blob.is_null() {
        return Err(closed_blob_error(interp));
    }
    if data.is_empty() {
        return Ok(Value::none());
    }
    let rc = unsafe {
        ffi::sqlite3_blob_write(
            blob,
            data.as_ptr().cast::<c_void>(),
            data.len() as c_int,
            offset as c_int,
        )
    };
    if rc != ffi::SQLITE_OK {
        return Err(raise_db_error(interp, db, rc));
    }
    Ok(Value::none())
}

// ── Module injection ─────────────────────────────────────────────────────────

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for helper in SQLITE3_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(*helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source_with_filename(
        SQLITE3_PY_SOURCE,
        "<frozen sqlite3>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("sqlite3: exec namespace not a dict".into()))?;
    for &name in SQLITE3_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("sqlite3"));
                class.error_name = match name {
                    "Warning" => Some("sqlite3.Warning"),
                    "Error" => Some("sqlite3.Error"),
                    "InterfaceError" => Some("sqlite3.InterfaceError"),
                    "DatabaseError" => Some("sqlite3.DatabaseError"),
                    "DataError" => Some("sqlite3.DataError"),
                    "OperationalError" => Some("sqlite3.OperationalError"),
                    "IntegrityError" => Some("sqlite3.IntegrityError"),
                    "InternalError" => Some("sqlite3.InternalError"),
                    "ProgrammingError" => Some("sqlite3.ProgrammingError"),
                    "NotSupportedError" => Some("sqlite3.NotSupportedError"),
                    _ => None,
                };
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    // Export all SQLITE_* constants directly onto the module.
    {
        let mut mod_ref = module.borrow_mut();
        for &(cname, cval) in SQLITE_CONSTANTS {
            mod_ref
                .attrs
                .insert(cname.to_string(), Value::int(i64::from(cval)));
        }
    }
    Ok(Some(ns.clone()))
}

// ── Native helper registration ───────────────────────────────────────────────

pyrust_module! {
    fn _sqlite_version(args) -> Result<Value> {
        let _ = (_interp, args);
        let ver = unsafe { CStr::from_ptr(ffi::sqlite3_libversion()) }
            .to_string_lossy()
            .into_owned();
        Ok(Value::string(&ver))
    }

    fn _threadsafety(args) -> Result<Value> {
        let _ = (_interp, args);
        let ts = unsafe { ffi::sqlite3_threadsafe() };
        let mapped = match ts {
            0 => 0,
            1 => 3,
            _ => 1,
        };
        Ok(Value::int(mapped))
    }

    fn _sqlite_constants(args) -> Result<Value> {
        let _ = (_interp, args);
        let d = Value::dict(PyDict::default());
        for &(name, val) in SQLITE_CONSTANTS {
            d.dict_insert(PyKey::str_from(name), Value::int(i64::from(val)))?;
        }
        Ok(d)
    }

    fn _complete_statement(args) -> Result<Value> {
        let _ = _interp;
        let sql = extract_str(&args[0].value, "statement")?;
        let c_sql = to_cstring(&sql, "statement")?;
        let res = unsafe { ffi::sqlite3_complete(c_sql.as_ptr()) };
        Ok(Value::bool_(res != 0))
    }

    fn _enable_callback_tracebacks(args) -> Result<Value> {
        let flag = _interp.truthy_value(&args[0].value)?;
        CALLBACK_TRACEBACKS.with(|c| c.set(flag));
        Ok(Value::none())
    }

    fn _conn_open(args) -> Result<Value> {
        conn_open_impl(_interp, args)
    }

    fn _conn_close(args) -> Result<Value> {
        let _ = _interp;
        let conn = clone_conn_inner(&args[0].value)?;
        conn.close();
        Ok(Value::none())
    }

    fn _conn_in_transaction(args) -> Result<Value> {
        let conn = clone_conn_inner(&args[0].value)?;
        let db = conn.db.get();
        if db.is_null() {
            return Ok(Value::bool_(false));
        }
        let ac = unsafe { ffi::sqlite3_get_autocommit(db) };
        Ok(Value::bool_(ac == 0))
    }

    fn _conn_total_changes(args) -> Result<Value> {
        let conn = clone_conn_inner(&args[0].value)?;
        let db = conn.db.get();
        if db.is_null() {
            return Err(closed_db_error(_interp));
        }
        let n = unsafe { ffi::sqlite3_total_changes(db) };
        Ok(Value::int(i64::from(n)))
    }

    fn _conn_interrupt(args) -> Result<Value> {
        let conn = clone_conn_inner(&args[0].value)?;
        let db = conn.db.get();
        if db.is_null() {
            return Err(closed_db_error(_interp));
        }
        unsafe {
            ffi::sqlite3_interrupt(db);
        }
        Ok(Value::none())
    }

    fn _conn_exec_simple(args) -> Result<Value> {
        conn_exec_simple_impl(_interp, args)
    }

    fn _conn_executescript(args) -> Result<Value> {
        conn_executescript_impl(_interp, args)
    }

    fn _conn_create_function(args) -> Result<Value> {
        conn_create_function_impl(_interp, args)
    }

    fn _conn_create_aggregate(args) -> Result<Value> {
        conn_create_aggregate_impl(_interp, args)
    }

    fn _conn_create_collation(args) -> Result<Value> {
        conn_create_collation_impl(_interp, args)
    }

    fn _conn_set_authorizer(args) -> Result<Value> {
        conn_set_authorizer_impl(_interp, args)
    }

    fn _conn_set_progress_handler(args) -> Result<Value> {
        conn_set_progress_handler_impl(_interp, args)
    }

    fn _conn_set_trace_callback(args) -> Result<Value> {
        conn_set_trace_callback_impl(_interp, args)
    }

    fn _conn_backup(args) -> Result<Value> {
        conn_backup_impl(_interp, args)
    }

    fn _conn_getlimit(args) -> Result<Value> {
        let conn = clone_conn_inner(&args[0].value)?;
        let cat = extract_i64(&args[1].value, "category")? as c_int;
        let db = conn.db.get();
        if db.is_null() {
            return Err(closed_db_error(_interp));
        }
        let prev = unsafe { ffi::sqlite3_limit(db, cat, -1) };
        if prev < 0 {
            return Err(raise_sqlite_class(
                _interp,
                "ProgrammingError",
                "'category' is out of bounds".to_string(),
                None,
            ));
        }
        Ok(Value::int(i64::from(prev)))
    }

    fn _conn_setlimit(args) -> Result<Value> {
        let conn = clone_conn_inner(&args[0].value)?;
        let cat = extract_i64(&args[1].value, "category")? as c_int;
        let limit = extract_i64(&args[2].value, "limit")? as c_int;
        let db = conn.db.get();
        if db.is_null() {
            return Err(closed_db_error(_interp));
        }
        let prev = unsafe { ffi::sqlite3_limit(db, cat, limit) };
        if prev < 0 {
            return Err(raise_sqlite_class(
                _interp,
                "ProgrammingError",
                "'category' is out of bounds".to_string(),
                None,
            ));
        }
        Ok(Value::int(i64::from(prev)))
    }

    fn _conn_getconfig(args) -> Result<Value> {
        let conn = clone_conn_inner(&args[0].value)?;
        let op = extract_i64(&args[1].value, "op")? as c_int;
        let db = conn.db.get();
        if db.is_null() {
            return Err(closed_db_error(_interp));
        }
        let mut out: c_int = 0;
        let rc = unsafe { ffi::sqlite3_db_config(db, op, -1 as c_int, &mut out as *mut c_int) };
        if rc != ffi::SQLITE_OK {
            return Err(raise_db_error(_interp, db, rc));
        }
        Ok(Value::bool_(out != 0))
    }

    fn _conn_setconfig(args) -> Result<Value> {
        let conn = clone_conn_inner(&args[0].value)?;
        let op = extract_i64(&args[1].value, "op")? as c_int;
        let enable = _interp.truthy_value(&args[2].value)?;
        let db = conn.db.get();
        if db.is_null() {
            return Err(closed_db_error(_interp));
        }
        let mut out: c_int = 0;
        let rc = unsafe {
            ffi::sqlite3_db_config(db, op, c_int::from(enable), &mut out as *mut c_int)
        };
        if rc != ffi::SQLITE_OK {
            return Err(raise_db_error(_interp, db, rc));
        }
        Ok(Value::bool_(out != 0))
    }

    fn _conn_serialize(args) -> Result<Value> {
        conn_serialize_impl(_interp, args)
    }

    fn _conn_deserialize(args) -> Result<Value> {
        conn_deserialize_impl(_interp, args)
    }

    fn _stmt_prepare(args) -> Result<Value> {
        stmt_prepare_impl(_interp, args)
    }

    fn _stmt_bind_and_step(args) -> Result<Value> {
        stmt_bind_and_step_impl(_interp, args)
    }

    fn _stmt_step(args) -> Result<Value> {
        stmt_step_impl(_interp, args)
    }

    fn _stmt_close(args) -> Result<Value> {
        let _ = _interp;
        let (_, slot, _) = get_stmt_parts(&args[0].value)?;
        let stmt = slot.replace(ptr::null_mut());
        if !stmt.is_null() {
            unsafe {
                ffi::sqlite3_finalize(stmt);
            }
        }
        Ok(Value::none())
    }

    fn _blob_open(args) -> Result<Value> {
        blob_open_impl(_interp, args)
    }

    fn _blob_close(args) -> Result<Value> {
        let _ = _interp;
        let (_, slot) = get_blob_parts(&args[0].value)?;
        let blob = slot.replace(ptr::null_mut());
        if !blob.is_null() {
            unsafe {
                ffi::sqlite3_blob_close(blob);
            }
        }
        Ok(Value::none())
    }

    fn _blob_len(args) -> Result<Value> {
        blob_len_impl(_interp, args)
    }

    fn _blob_read(args) -> Result<Value> {
        blob_read_impl(_interp, args)
    }

    fn _blob_write(args) -> Result<Value> {
        blob_write_impl(_interp, args)
    }
}
