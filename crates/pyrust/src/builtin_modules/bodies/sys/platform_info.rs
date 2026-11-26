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

/// Default installation prefix derived from an executable path (matching
/// CPython's fallback layout: parent of `bin/` on Unix when installed in
/// `bin/`, else executable's parent directory).
pub(crate) fn default_sys_prefix_for_exe(exe: &std::path::Path) -> String {
    let Some(exe_dir) = exe.parent() else {
        return String::new();
    };
    #[cfg(not(windows))]
    if exe_dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "bin")
        && let Some(parent) = exe_dir.parent()
    {
        return parent.to_string_lossy().into_owned();
    }
    exe_dir.to_string_lossy().into_owned()
}

/// Default installation prefix derived from `sys.executable`.
pub(crate) fn default_sys_prefix() -> String {
    let Ok(exe) = std::env::current_exe() else {
        return String::new();
    };
    default_sys_prefix_for_exe(&exe)
}

/// Startup metadata parsed from PEP 405 `pyvenv.cfg` when the executable lives
/// inside a virtual environment (either alongside `pyvenv.cfg` or in a child
/// directory such as `bin/` or `Scripts/`).
#[derive(Debug, Clone)]
pub(crate) struct VenvStartupInfo {
    pub prefix: String,
    pub base_prefix: String,
    pub executable: String,
    pub base_executable: String,
    pub home: Option<String>,
    pub include_system_site_packages: bool,
}

/// Normalize `.` and `..` components lexically without following symlinks, so
/// `<venv>/bin/python -> /path/to/pyrust` keeps its `<venv>/bin/python` path.
fn clean_lexical_path(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::{Component, PathBuf};
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(
                    out.components().next_back(),
                    None | Some(Component::RootDir | Component::Prefix(_))
                ) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve the invoked executable path from `argv[0]` without following the
/// final symlink (critical on Unix where `<venv>/bin/python` is typically a
/// symlink to the base `pyrust` binary and `std::env::current_exe()` follows
/// symlinks via `/proc/self/exe`).
fn resolve_invoked_executable(arg0: Option<&str>) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};

    if let Some(raw) = arg0
        && !raw.is_empty()
    {
        let p = Path::new(raw);
        let has_sep =
            raw.contains('/') || (cfg!(windows) && (raw.contains('\\') || raw.contains(':')));
        if has_sep {
            let abs = if p.is_absolute() {
                clean_lexical_path(p)
            } else if let Ok(cwd) = std::env::current_dir() {
                clean_lexical_path(&cwd.join(p))
            } else {
                p.to_path_buf()
            };
            if std::fs::symlink_metadata(&abs).is_ok() {
                return Some(abs);
            }
            #[cfg(windows)]
            if abs.extension().is_none() {
                let with_exe = abs.with_extension("exe");
                if std::fs::symlink_metadata(&with_exe).is_ok() {
                    return Some(with_exe);
                }
            }
        } else if let Some(path_var) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path_var) {
                let candidate = clean_lexical_path(&dir.join(p));
                if std::fs::symlink_metadata(&candidate).is_ok() && candidate.is_file() {
                    return Some(candidate);
                }
                #[cfg(windows)]
                if candidate.extension().is_none() {
                    let with_exe = candidate.with_extension("exe");
                    if std::fs::symlink_metadata(&with_exe).is_ok() && with_exe.is_file() {
                        return Some(with_exe);
                    }
                }
            }
        }
    }

    std::env::current_exe().ok().map(|p| PathBuf::from(&p))
}

/// Parse key/value pairs from a PEP 405 `pyvenv.cfg` file.
pub(crate) fn parse_pyvenv_cfg(content: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for line in content.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().to_string();
            if !key.is_empty() {
                map.insert(key, val);
            }
        }
    }
    map
}

/// Check the executable's directory and its parent directory for `pyvenv.cfg`
/// (PEP 405) and return virtual-environment startup metadata when present.
pub(crate) fn detect_pyvenv_startup(arg0: Option<&str>) -> Option<VenvStartupInfo> {
    use std::path::Path;

    let invoked_exe = resolve_invoked_executable(arg0)?;
    let exe_dir = invoked_exe.parent()?;
    let (venv_dir, cfg_path) = {
        let same_dir = exe_dir.join("pyvenv.cfg");
        if same_dir.is_file() {
            (exe_dir.to_path_buf(), same_dir)
        } else {
            let parent_dir = exe_dir.parent()?;
            let parent_cfg = parent_dir.join("pyvenv.cfg");
            if parent_cfg.is_file() {
                (parent_dir.to_path_buf(), parent_cfg)
            } else {
                return None;
            }
        }
    };

    let raw_bytes = std::fs::read(&cfg_path).ok()?;
    let content = String::from_utf8_lossy(&raw_bytes);
    let kv = parse_pyvenv_cfg(&content);

    let home = kv.get("home").cloned().filter(|s| !s.is_empty());
    // CPython's `site.venv()` defaults `include-system-site-packages` to true
    // when omitted; `venv.EnvBuilder` always writes `true` or `false`.
    let include_system_site_packages = kv
        .get("include-system-site-packages")
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(true);

    let real_exe = std::env::current_exe().unwrap_or_else(|_| invoked_exe.clone());
    let base_prefix = if let Some(bp) = kv.get("base-prefix").filter(|s| !s.is_empty()) {
        bp.clone()
    } else if let Some(ref home_dir) = home {
        let home_path = Path::new(home_dir);
        #[cfg(not(windows))]
        if home_path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == "bin")
            && let Some(parent) = home_path.parent()
        {
            parent.to_string_lossy().into_owned()
        } else {
            home_dir.clone()
        }
        #[cfg(windows)]
        {
            let _ = home_path;
            home_dir.clone()
        }
    } else {
        default_sys_prefix_for_exe(&real_exe)
    };

    let base_executable = if let Some(be) = kv.get("base-executable").filter(|s| !s.is_empty()) {
        be.clone()
    } else if let Some(ref home_dir) = home {
        let candidate = invoked_exe
            .file_name()
            .map(|name| Path::new(home_dir).join(name));
        if let Some(c) = candidate
            && c.is_file()
        {
            c.to_string_lossy().into_owned()
        } else if let Some(exe_cfg) = kv.get("executable").filter(|s| !s.is_empty())
            && Path::new(exe_cfg).is_file()
        {
            exe_cfg.clone()
        } else if real_exe != invoked_exe {
            real_exe.to_string_lossy().into_owned()
        } else {
            kv.get("executable")
                .filter(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| invoked_exe.to_string_lossy().into_owned())
        }
    } else {
        real_exe.to_string_lossy().into_owned()
    };

    Some(VenvStartupInfo {
        prefix: venv_dir.to_string_lossy().into_owned(),
        base_prefix,
        executable: invoked_exe.to_string_lossy().into_owned(),
        base_executable,
        home,
        include_system_site_packages,
    })
}

/// The classes of the struct-sequence style `sys` values.
#[derive(Clone, Copy)]
enum InfoClass {
    FloatInfo,
    IntInfo,
    HashInfo,
    Implementation,
    ThreadInfo,
    AsyncgenHooks,
}

impl InfoClass {
    const COUNT: usize = 6;

    const fn display_name(self) -> &'static str {
        match self {
            Self::FloatInfo => "sys.float_info",
            Self::IntInfo => "sys.int_info",
            Self::HashInfo => "sys.hash_info",
            Self::Implementation => "sys.implementation",
            Self::ThreadInfo => "sys.thread_info",
            Self::AsyncgenHooks => "asyncgen_hooks",
        }
    }

    /// Tuple-backed classes support sequence operations and
    /// `isinstance(..., tuple)`, matching CPython's `PyStructSequence`.
    const fn is_tuple_backed(self) -> bool {
        matches!(self, Self::ThreadInfo | Self::AsyncgenHooks)
    }
}

// Thread-local justification:
// (a) Why `thread_local!`: `PyClass` is `!Send + !Sync`, so the class objects
//     of `sys.float_info` and friends cannot live in a process-global
//     `OnceLock`.
// (b) Multi-interpreter / multi-threaded impact: interpreters on one GIL group
//     share these class objects, exactly as they share `sys.version_info`'s
//     class and every built-in type; each `sys` module still owns its own
//     instances.  Each class is built on first use, so a script that never
//     reads the value never allocates its class (issue #3293).
pyrust_core::gil_local! {
    static INFO_CLASSES: [std::cell::OnceCell<Rc<RefCell<PyClass>>>; InfoClass::COUNT] =
        std::array::from_fn(|_| std::cell::OnceCell::new());
}

/// The shared class object for one struct-sequence style `sys` value.
fn info_class(kind: InfoClass) -> Rc<RefCell<PyClass>> {
    INFO_CLASSES.with(|classes| {
        Rc::clone(classes[kind as usize].get_or_init(|| {
            let base = kind
                .is_tuple_backed()
                .then(|| crate::interpreter::primitive_class_by_name("tuple"))
                .flatten();
            Rc::new(RefCell::new(PyClass::new(
                kind.display_name(),
                kind.display_name(),
                base,
                indexmap::IndexMap::new(),
            )))
        }))
    })
}

/// Build a simple named-attribute `PyInstance` of a shared class.  Used for
/// the namespace-like sys members that only need attribute access.
fn make_named_struct(kind: InfoClass, fields: Vec<(&str, Value)>) -> Value {
    let mut attrs = InstanceAttrs::new();
    for (k, v) in fields {
        attrs.insert(k, v);
    }
    let class = info_class(kind);
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Build a tuple-backed struct-sequence `PyInstance` supporting both named
/// attribute access and sequence operations (`len`, indexing, unpacking,
/// tuple equality, `isinstance(..., tuple)`), matching CPython's
/// `PyStructSequence` for `sys.thread_info` and `sys.get_asyncgen_hooks()`.
fn make_struct_seq(kind: InfoClass, fields: Vec<(&str, Value)>) -> Value {
    debug_assert!(kind.is_tuple_backed());
    let mut attrs = InstanceAttrs::new();
    let mut tuple_values = Vec::with_capacity(fields.len());
    for (k, v) in fields {
        tuple_values.push(v.clone());
        attrs.insert(k, v);
    }
    attrs.set_builtin_data(Value::tuple(tuple_values));
    let class = info_class(kind);
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Build `sys.float_info` — IEEE-754 double-precision limits.
pub(super) fn make_float_info() -> Value {
    make_named_struct(
        InfoClass::FloatInfo,
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
        InfoClass::IntInfo,
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
        InfoClass::HashInfo,
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
        InfoClass::Implementation,
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
    let names = [
        "_thread",
        "_tracemalloc",
        "builtins",
        "errno",
        "faulthandler",
        "gc",
        "marshal",
        "sys",
        "time",
    ];
    Value::tuple(names.iter().map(|n| Value::string(*n)).collect())
}

#[cfg(windows)]
crate::pyrust_struct_seq! {
    pub(super) getwindowsversion in "sys" {
        fields: [major, minor, build, platform, service_pack],
        extra_fields: [
            service_pack_major,
            service_pack_minor,
            suite_mask,
            product_type,
            platform_version,
        ],
    }
}

#[cfg(windows)]
pub(super) fn make_windows_version() -> Value {
    use windows_sys::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOEXW};

    // Default fallback if OS query fails.
    let mut major = 10i64;
    let mut minor = 0i64;
    let mut build = 19041i64;
    let mut platform = 2i64;
    let mut service_pack = String::new();
    let mut sp_major = 0i64;
    let mut sp_minor = 0i64;
    let mut suite_mask = 0i64;
    let mut product_type = 1i64;

    // SAFETY: `OSVERSIONINFOEXW` is zero-initialized and `dwOSVersionInfoSize`
    // is set to `size_of::<OSVERSIONINFOEXW>()`, as required by `GetVersionExW`.
    unsafe {
        let mut info: OSVERSIONINFOEXW = std::mem::zeroed();
        info.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOEXW>() as u32;
        if GetVersionExW(&mut info as *mut OSVERSIONINFOEXW as *mut _) != 0 {
            major = info.dwMajorVersion as i64;
            minor = info.dwMinorVersion as i64;
            build = info.dwBuildNumber as i64;
            platform = info.dwPlatformId as i64;
            let end = info
                .szCSDVersion
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(info.szCSDVersion.len());
            service_pack = String::from_utf16_lossy(&info.szCSDVersion[..end]);
            sp_major = info.wServicePackMajor as i64;
            sp_minor = info.wServicePackMinor as i64;
            suite_mask = info.wSuiteMask as i64;
            product_type = info.wProductType as i64;
        }
    }

    let platform_version = Value::tuple(vec![
        Value::int(major),
        Value::int(minor),
        Value::int(build),
    ]);

    getwindowsversion::make_with_extra(
        vec![
            Value::int(major),
            Value::int(minor),
            Value::int(build),
            Value::int(platform),
            Value::string(service_pack),
        ],
        vec![
            Value::int(sp_major),
            Value::int(sp_minor),
            Value::int(suite_mask),
            Value::int(product_type),
            platform_version,
        ],
    )
}

#[cfg(windows)]
pub(super) fn sys_dllhandle() -> i64 {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    // SAFETY: Passing NULL retrieves the HMODULE of the current executable.
    let handle = unsafe { GetModuleHandleW(std::ptr::null()) };
    handle as isize as i64
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
        InfoClass::ThreadInfo,
        vec![("name", name), ("lock", lock), ("version", Value::none())],
    )
}

/// Build `sys.asyncgen_hooks(firstiter, finalizer)`.
pub(super) fn make_asyncgen_hooks(firstiter: Value, finalizer: Value) -> Value {
    make_struct_seq(
        InfoClass::AsyncgenHooks,
        vec![("firstiter", firstiter), ("finalizer", finalizer)],
    )
}

// Thread-local justification:
// (a) Why `thread_local!`: `Value` is `!Send + !Sync`, so the frozenset cannot
//     live in a process-global `OnceLock`.
// (b) Multi-interpreter / multi-threaded impact: the frozenset is immutable,
//     so interpreters on one GIL group can share the one object; it is built
//     (300 interned strings) on the first `sys.stdlib_module_names` read of
//     the group instead of once per `sys` module object (issue #3293).
pyrust_core::gil_local! {
    static STDLIB_MODULE_NAMES: std::cell::OnceCell<Value> = const { std::cell::OnceCell::new() };
}

/// `sys.stdlib_module_names`, shared by every `sys` module of the GIL group.
pub(super) fn stdlib_module_names() -> Value {
    STDLIB_MODULE_NAMES.with(|names| names.get_or_init(make_stdlib_module_names).clone())
}

/// Build `sys.stdlib_module_names` — CPython 3.12.3's exact 300-entry
/// `frozenset` of standard library module names (from CPython v3.12.3
/// `Python/stdlib_module_names.h`, issues #2970 / #3242).
fn make_stdlib_module_names() -> Value {
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
