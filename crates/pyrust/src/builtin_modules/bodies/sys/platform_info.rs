// Platform constants, struct-sequence builders (`float_info`, `int_info`,
// `hash_info`, `implementation`, `thread_info`, `asyncgen_hooks`), and
// module-name registries (`builtin_module_names`, `stdlib_module_names`).

use std::cell::RefCell;
use std::rc::Rc;

use super::version_info::make_version_info;
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value};

/// Return the platform string matching CPython's `sys.platform` for the
/// current compilation target.
pub(super) const fn sys_platform() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "linux"
    }
    #[cfg(target_os = "windows")]
    {
        "win32"
    }
    #[cfg(target_os = "macos")]
    {
        "darwin"
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        "unknown"
    }
}

/// Return the byte-order string matching CPython's `sys.byteorder` for the
/// current compilation target.
pub(super) const fn sys_byteorder() -> &'static str {
    #[cfg(target_endian = "little")]
    {
        "little"
    }
    #[cfg(target_endian = "big")]
    {
        "big"
    }
}

/// Platform library directory name matching CPython 3.12 (`"DLLs"` on Windows,
/// `"lib"` on Unix/macOS).
pub(super) const fn sys_platlibdir() -> &'static str {
    if cfg!(windows) { "DLLs" } else { "lib" }
}

/// Build a simple named-attribute PyInstance under a fresh anonymous
/// class with the given class name.  Used for the namespace-like sys
/// members that only need attribute access.
fn make_named_struct(class_name: &str, fields: Vec<(&str, Value)>) -> Value {
    let mut attrs = InstanceAttrs::new();
    for (k, v) in fields {
        attrs.insert(k, v);
    }
    let class = Rc::new(RefCell::new(PyClass::new(
        class_name,
        class_name,
        None,
        indexmap::IndexMap::new(),
    )));
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Build a tuple-backed struct-sequence `PyInstance` supporting both named
/// attribute access and sequence operations (`len`, indexing, unpacking,
/// tuple equality, `isinstance(..., tuple)`), matching CPython's
/// `PyStructSequence` for `sys.thread_info` and `sys.get_asyncgen_hooks()`.
fn make_struct_seq(class_name: &str, fields: Vec<(&str, Value)>) -> Value {
    let mut attrs = InstanceAttrs::new();
    let mut tuple_values = Vec::with_capacity(fields.len());
    for (k, v) in fields {
        tuple_values.push(v.clone());
        attrs.insert(k, v);
    }
    attrs.set_builtin_data(Value::tuple(tuple_values));
    let tuple_base = crate::interpreter::primitive_class_by_name("tuple");
    let class = Rc::new(RefCell::new(PyClass::new(
        class_name,
        class_name,
        tuple_base,
        indexmap::IndexMap::new(),
    )));
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Build `sys.float_info` — IEEE-754 double-precision limits.
pub(super) fn make_float_info() -> Value {
    make_named_struct(
        "sys.float_info",
        vec![
            ("max", Value::float(f64::MAX)),
            ("max_exp", Value::int(1024)),
            ("max_10_exp", Value::int(308)),
            ("min", Value::float(f64::MIN_POSITIVE)),
            ("min_exp", Value::int(-1021)),
            ("min_10_exp", Value::int(-307)),
            ("dig", Value::int(15)),
            ("mant_dig", Value::int(53)),
            ("epsilon", Value::float(f64::EPSILON)),
            ("radix", Value::int(2)),
            ("rounds", Value::int(1)),
        ],
    )
}

/// Build `sys.int_info`.
pub(super) fn make_int_info() -> Value {
    make_named_struct(
        "sys.int_info",
        vec![
            ("bits_per_digit", Value::int(30)),
            ("sizeof_digit", Value::int(4)),
            ("default_max_str_digits", Value::int(4300)),
            ("str_digits_check_threshold", Value::int(640)),
        ],
    )
}

/// Build `sys.hash_info` — 64-bit CPython hashing parameters.  Matches the
/// CPython 3.12 64-bit build (`modulus = 2**61 - 1`, SipHash-1-3).
pub(super) fn make_hash_info() -> Value {
    make_named_struct(
        "sys.hash_info",
        vec![
            ("width", Value::int(64)),
            ("modulus", Value::int(2_305_843_009_213_693_951)),
            ("inf", Value::int(314_159)),
            ("nan", Value::int(0)),
            ("imag", Value::int(1_000_003)),
            ("algorithm", Value::string("siphash13")),
            ("hash_bits", Value::int(64)),
            ("seed_bits", Value::int(128)),
            ("cutoff", Value::int(0)),
        ],
    )
}

/// Build `sys.implementation`.  Reports `cpython` (pyrust emulates the
/// CPython 3.12 reference) so version-sniffing user code behaves.
pub(super) fn make_implementation() -> Value {
    let version = make_version_info();
    make_named_struct(
        "sys.implementation",
        vec![
            ("name", Value::string("cpython")),
            ("version", version),
            ("hexversion", Value::int(0x030c_00f0)),
            ("cache_tag", Value::string("cpython-312")),
        ],
    )
}

/// Build `sys.builtin_module_names`.
pub(super) fn make_builtin_module_names() -> Value {
    let names = ["builtins", "sys", "_thread", "errno", "time", "gc"];
    Value::tuple(names.iter().map(|n| Value::string(*n)).collect())
}

/// Build `sys.thread_info` — struct-sequence describing the thread
/// implementation (`name`, `lock`, `version`).
pub(super) fn make_thread_info() -> Value {
    let (name, lock) = if cfg!(windows) {
        (Value::string("nt"), Value::none())
    } else {
        (Value::string("pthread"), Value::string("semaphore"))
    };
    make_struct_seq(
        "sys.thread_info",
        vec![("name", name), ("lock", lock), ("version", Value::none())],
    )
}

/// Build `sys.asyncgen_hooks(firstiter, finalizer)`.
pub(super) fn make_asyncgen_hooks(firstiter: Value, finalizer: Value) -> Value {
    make_struct_seq(
        "asyncgen_hooks",
        vec![("firstiter", firstiter), ("finalizer", finalizer)],
    )
}

/// Build `sys.stdlib_module_names` — a `frozenset` of Python 3.12 standard
/// library module names (issue #2970).
pub(super) fn make_stdlib_module_names() -> Value {
    const NAMES: &[&str] = &[
        "__future__",
        "_abc",
        "_ast",
        "_bisect",
        "_codecs",
        "_collections",
        "_collections_abc",
        "_csv",
        "_datetime",
        "_decimal",
        "_functools",
        "_heapq",
        "_imp",
        "_io",
        "_json",
        "_operator",
        "_pickle",
        "_random",
        "_signal",
        "_socket",
        "_sre",
        "_stat",
        "_string",
        "_struct",
        "_thread",
        "_threading_local",
        "_tokenize",
        "_tracemalloc",
        "_typing",
        "_warnings",
        "_weakref",
        "abc",
        "argparse",
        "array",
        "ast",
        "asyncio",
        "atexit",
        "base64",
        "bdb",
        "binascii",
        "bisect",
        "builtins",
        "bz2",
        "calendar",
        "cmath",
        "cmd",
        "code",
        "codecs",
        "codeop",
        "collections",
        "colorsys",
        "compileall",
        "concurrent",
        "configparser",
        "contextlib",
        "contextvars",
        "copy",
        "copyreg",
        "cProfile",
        "csv",
        "ctypes",
        "dataclasses",
        "datetime",
        "dbm",
        "decimal",
        "difflib",
        "dis",
        "doctest",
        "email",
        "encodings",
        "enum",
        "errno",
        "faulthandler",
        "fcntl",
        "filecmp",
        "fileinput",
        "fnmatch",
        "fractions",
        "ftplib",
        "functools",
        "gc",
        "genericpath",
        "getopt",
        "getpass",
        "gettext",
        "glob",
        "graphlib",
        "grp",
        "gzip",
        "hashlib",
        "heapq",
        "hmac",
        "html",
        "http",
        "idlelib",
        "imaplib",
        "importlib",
        "inspect",
        "io",
        "ipaddress",
        "itertools",
        "json",
        "keyword",
        "linecache",
        "locale",
        "logging",
        "lzma",
        "mailbox",
        "marshal",
        "math",
        "mimetypes",
        "mmap",
        "modulefinder",
        "multiprocessing",
        "netrc",
        "nt",
        "ntpath",
        "numbers",
        "operator",
        "optparse",
        "os",
        "pathlib",
        "pdb",
        "pickle",
        "pickletools",
        "pkgutil",
        "platform",
        "plistlib",
        "poplib",
        "posix",
        "posixpath",
        "pprint",
        "profile",
        "pstats",
        "pty",
        "pwd",
        "py_compile",
        "pyclbr",
        "pydoc",
        "queue",
        "quopri",
        "random",
        "re",
        "readline",
        "reprlib",
        "resource",
        "rlcompleter",
        "runpy",
        "sched",
        "secrets",
        "select",
        "selectors",
        "shelve",
        "shlex",
        "shutil",
        "signal",
        "site",
        "smtplib",
        "socket",
        "socketserver",
        "sqlite3",
        "ssl",
        "stat",
        "statistics",
        "string",
        "stringprep",
        "struct",
        "subprocess",
        "symtable",
        "sys",
        "sysconfig",
        "syslog",
        "tabnanny",
        "tarfile",
        "tempfile",
        "termios",
        "textwrap",
        "threading",
        "time",
        "timeit",
        "tkinter",
        "token",
        "tokenize",
        "tomllib",
        "trace",
        "traceback",
        "tracemalloc",
        "tty",
        "turtle",
        "types",
        "typing",
        "unicodedata",
        "unittest",
        "urllib",
        "uuid",
        "venv",
        "warnings",
        "wave",
        "weakref",
        "webbrowser",
        "winreg",
        "winsound",
        "wsgiref",
        "xml",
        "xmlrpc",
        "zipapp",
        "zipfile",
        "zipimport",
        "zlib",
        "zoneinfo",
    ];
    let mut set = crate::value::PySet::default();
    for &name in NAMES {
        set.insert(crate::value::PyKey::str_from(name));
    }
    pyrust_builtins::frozenset::frozenset(set)
}
