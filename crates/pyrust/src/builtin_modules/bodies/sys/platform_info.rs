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

/// Path to the currently running executable for `sys.executable`.
pub(super) fn sys_executable() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
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

/// Build `sys.stdlib_module_names` — CPython 3.12.3's exact 300-entry
/// `frozenset` of standard library module names (from CPython v3.12.3
/// `Python/stdlib_module_names.h`, issues #2970 / #3242).
pub(super) fn make_stdlib_module_names() -> Value {
    const NAMES: &[&str] = &[
        "__future__",
        "_abc",
        "_aix_support",
        "_ast",
        "_asyncio",
        "_bisect",
        "_blake2",
        "_bz2",
        "_codecs",
        "_codecs_cn",
        "_codecs_hk",
        "_codecs_iso2022",
        "_codecs_jp",
        "_codecs_kr",
        "_codecs_tw",
        "_collections",
        "_collections_abc",
        "_compat_pickle",
        "_compression",
        "_contextvars",
        "_crypt",
        "_csv",
        "_ctypes",
        "_curses",
        "_curses_panel",
        "_datetime",
        "_dbm",
        "_decimal",
        "_elementtree",
        "_frozen_importlib",
        "_frozen_importlib_external",
        "_functools",
        "_gdbm",
        "_hashlib",
        "_heapq",
        "_imp",
        "_io",
        "_json",
        "_locale",
        "_lsprof",
        "_lzma",
        "_markupbase",
        "_md5",
        "_msi",
        "_multibytecodec",
        "_multiprocessing",
        "_opcode",
        "_operator",
        "_osx_support",
        "_overlapped",
        "_pickle",
        "_posixshmem",
        "_posixsubprocess",
        "_py_abc",
        "_pydatetime",
        "_pydecimal",
        "_pyio",
        "_pylong",
        "_queue",
        "_random",
        "_scproxy",
        "_sha1",
        "_sha2",
        "_sha3",
        "_signal",
        "_sitebuiltins",
        "_socket",
        "_sqlite3",
        "_sre",
        "_ssl",
        "_stat",
        "_statistics",
        "_string",
        "_strptime",
        "_struct",
        "_symtable",
        "_thread",
        "_threading_local",
        "_tkinter",
        "_tokenize",
        "_tracemalloc",
        "_typing",
        "_uuid",
        "_warnings",
        "_weakref",
        "_weakrefset",
        "_winapi",
        "_zoneinfo",
        "abc",
        "aifc",
        "antigravity",
        "argparse",
        "array",
        "ast",
        "asyncio",
        "atexit",
        "audioop",
        "base64",
        "bdb",
        "binascii",
        "bisect",
        "builtins",
        "bz2",
        "cProfile",
        "calendar",
        "cgi",
        "cgitb",
        "chunk",
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
        "crypt",
        "csv",
        "ctypes",
        "curses",
        "dataclasses",
        "datetime",
        "dbm",
        "decimal",
        "difflib",
        "dis",
        "doctest",
        "email",
        "encodings",
        "ensurepip",
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
        "imghdr",
        "importlib",
        "inspect",
        "io",
        "ipaddress",
        "itertools",
        "json",
        "keyword",
        "lib2to3",
        "linecache",
        "locale",
        "logging",
        "lzma",
        "mailbox",
        "mailcap",
        "marshal",
        "math",
        "mimetypes",
        "mmap",
        "modulefinder",
        "msilib",
        "msvcrt",
        "multiprocessing",
        "netrc",
        "nis",
        "nntplib",
        "nt",
        "ntpath",
        "nturl2path",
        "numbers",
        "opcode",
        "operator",
        "optparse",
        "os",
        "ossaudiodev",
        "pathlib",
        "pdb",
        "pickle",
        "pickletools",
        "pipes",
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
        "pydoc_data",
        "pyexpat",
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
        "sndhdr",
        "socket",
        "socketserver",
        "spwd",
        "sqlite3",
        "sre_compile",
        "sre_constants",
        "sre_parse",
        "ssl",
        "stat",
        "statistics",
        "string",
        "stringprep",
        "struct",
        "subprocess",
        "sunau",
        "symtable",
        "sys",
        "sysconfig",
        "syslog",
        "tabnanny",
        "tarfile",
        "telnetlib",
        "tempfile",
        "termios",
        "textwrap",
        "this",
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
        "turtledemo",
        "types",
        "typing",
        "unicodedata",
        "unittest",
        "urllib",
        "uu",
        "uuid",
        "venv",
        "warnings",
        "wave",
        "weakref",
        "webbrowser",
        "winreg",
        "winsound",
        "wsgiref",
        "xdrlib",
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

#[cfg(test)]
mod stdlib_module_names_tests {
    use super::make_stdlib_module_names;
    use crate::value::ValueKind;

    #[test]
    fn stdlib_module_names_has_exact_cpython_3_12_3_count() {
        let val = make_stdlib_module_names();
        let count = match val.kind() {
            ValueKind::BuiltinObject { ops, state } => ops.len(state).expect("frozenset len"),
            _ => panic!("expected frozenset"),
        };
        assert_eq!(
            count, 300,
            "CPython 3.12.3 stdlib_module_names has 300 entries"
        );
    }
}
