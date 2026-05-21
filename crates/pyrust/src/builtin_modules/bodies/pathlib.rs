// `pathlib` module — `pathlib.Path` and `pathlib.PosixPath` classes.
//
// `Path` wraps a string path (stored as `_path` in instance attrs).  State
// is deliberately kept as a plain `str` so that printing and joining are
// zero-cost string operations.
//
// ## CPython parity: PosixPath subclass
//
// On Linux/macOS, `pathlib.Path(...)` returns a `PosixPath` instance, not a
// plain `Path` instance.  `type(Path('/tmp')).__name__` is `'PosixPath'`.
// `PosixPath` is a concrete subclass of `Path` with no additional behaviour —
// it exists purely to match CPython's runtime type semantics.
//
// Implementation: both classes are built manually in thread-local singletons
// (following the `_Environ` pattern in `os.rs`).  The dispatch functions are
// registered under `"pathlib.Path.<method>"` names via `#[py_name]`; the
// `PosixPath` class has `base = Some(Rc::clone(&PATH_CLASS))` so
// `lookup_class_attr` inherits all `Path` methods.  `make_path_instance`
// constructs `PosixPath` instances, not `Path` instances.
//
// ## Design choices
//
// - All "property-like" methods (`name`, `parent`, `stem`, `suffix`, `parts`)
//   are implemented as callable methods rather than descriptors.  The parity
//   test calls them with `()`.  This keeps the implementation in a single
//   `pyrust_module!` block with no descriptor protocol plumbing.
//
// - `__truediv__` (`/` operator) creates a new `Path` instance by building
//   attrs directly rather than going through `__init__`, matching the pattern
//   established in `functools.rs::make_instance`.
//
// - `__hash__` returns FNV-1a of the path string — the same algorithm that
//   `PyKey::Str` uses in `pyrust-core`, so `hash(Path('x')) == hash('x')`
//   is consistent (CPython's pathlib.Path hash equals the hash of its string).
//
// - `read_text` / `write_text` accept `encoding` and `errors` keyword args
//   but do not honour them (both are UTF-8 only, matching `std::fs`).
//
// Reference: <https://docs.python.org/3/library/pathlib.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::value::{PyClass, PyInstance, Value, ValueKind};
use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

// ── helpers ───────────────────────────────────────────────────────────────────

fn expect_self(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<RefCell<PyInstance>>> {
    match args.first().map(|a| a.value.kind()) {
        Some(ValueKind::PyInstance(rc)) => Ok(Rc::clone(&rc)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

fn get_path(inst: &Rc<RefCell<PyInstance>>, fn_name: &str) -> Result<String> {
    match inst.borrow().attrs.get("_path").map(|v| v.kind()) {
        Some(ValueKind::Str(s)) => Ok(s.to_string()),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}: Path._path has been overwritten with a non-str; \
                 don't assign to internal attributes",
            ),
        )),
    }
}

/// Returns true if the instance's class name is "Path" or "PosixPath"
/// (or any subclass thereof that pyrust creates).  Used by `__eq__` and
/// `__truediv__` to accept both plain `Path` and `PosixPath` instances.
fn is_path_instance(rc: &Rc<RefCell<PyInstance>>) -> bool {
    let name = rc.borrow().class.borrow().name.clone();
    name == "Path" || name == "PosixPath"
}

/// Build a new `PosixPath` instance with the given path string, bypassing
/// `__init__`.  Used by `__truediv__`, `parent`, etc.
fn make_path_instance(path: &str) -> Value {
    POSIX_PATH_CLASS.with(|class| {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        attrs.insert("_path".to_string(), Value::string(path));
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(class),
            attrs,
        })))
    })
}

/// FNV-1a hash of a string — matches the algorithm in `PyKey::Str`'s
/// `py_hash_pykey` arm so that `hash(Path('x')) == hash('x')`.
fn fnv1a_hash(s: &str) -> i64 {
    let mut h: u64 = 14695981039346656037u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211u64);
    }
    h as i64
}

/// Normalize a POSIX path string the same way CPython's `pathlib` does:
///
/// - Strip duplicate `/` separators (except the `//` UNC prefix).
/// - Remove `.` components (but NOT `..`).
/// - Remove trailing slashes.
/// - An all-empty relative result becomes `"."`.
///
/// This mirrors `pathlib._from_parsed_string` normalization in 3.12.
fn normalize_path(path: &str) -> String {
    // CPython special-cases exactly two leading slashes (UNC / POSIX.1 allows
    // implementation-defined semantics for `//`).  We preserve them.
    let (prefix, rest) = if path.starts_with("//") && !path.starts_with("///") {
        ("//", &path[2..])
    } else if path.starts_with('/') {
        ("/", &path[1..])
    } else {
        ("", path)
    };

    let parts: Vec<&str> = rest
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();

    if parts.is_empty() {
        if prefix.is_empty() {
            ".".to_string()
        } else {
            prefix.to_string()
        }
    } else {
        format!("{}{}", prefix, parts.join("/"))
    }
}

// ── module ────────────────────────────────────────────────────────────────────
//
// All methods are declared as module-level fns with `#[py_name = "Path.<m>"]`.
// Both `Path` and `PosixPath` share the same dispatch fns — `lookup_class_attr`
// walks `PosixPath.base` → `Path` automatically.  Only `PosixPath` needs its
// own `module()` entry; no duplicate dispatch fn required.

pyrust_module! {
    constants {
        // Both classes are inserted here so `from pathlib import Path, PosixPath`
        // works.  The thread-local singletons ensure `Path is Path` holds
        // across re-imports (same `Rc<RefCell<PyClass>>` object each time).
        "Path" => make_path_class_value(),
        "PosixPath" => make_posix_path_class_value(),
    }

    /// `Path.__new__(cls, *parts)` — allocate a new `PosixPath` instance.
    ///
    /// CPython dispatches `Path(...)` to `PosixPath` on POSIX via `__new__`.
    /// We replicate that here: `__new__` allocates a bare `PosixPath`
    /// instance (no `_path` set yet); `__init__` is called next by
    /// `call_class_expanded` with the same `parts` args to fill `_path`.
    ///
    /// On POSIX the returned object is always a `PosixPath` regardless of
    /// which concrete class (`Path` or `PosixPath`) was called.
    #[py_name = "Path.__new__"]
    fn path_new(_args) -> Result<Value> {
        // _args[0] is the class (PyClass value), _args[1..] are the parts
        // (which we ignore here — __init__ handles them).
        let _ = _interp;
        Ok(POSIX_PATH_CLASS.with(|class| {
            Value::py_instance(Rc::new(RefCell::new(PyInstance {
                class: Rc::clone(class),
                attrs: IndexMap::new(),
            })))
        }))
    }

    /// `Path(*parts)` — join `parts` with `/` to form the path string.
    ///
    /// If any argument is an absolute path (starts with `/`) all preceding
    /// parts are discarded, matching CPython's `os.path.join` semantics
    /// applied element-wise.
    ///
    /// Returns a `PosixPath` instance on POSIX platforms.
    ///
    /// <https://docs.python.org/3/library/pathlib.html#pathlib.PurePath>
    #[py_name = "Path.__init__"]
    fn path_init(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let parts = &args[1..];
        let mut segments: Vec<String> = Vec::new();
        for a in parts {
            let s = match a.value.kind() {
                ValueKind::Str(s) => s.to_string(),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!(
                        "argument should be str, not '{}'",
                        pyrust_core::builtin_type_name(&a.value),
                    ),
                )),
            };
            // CPython: if a component is absolute it replaces prior ones.
            if s.starts_with('/') {
                segments.clear();
            }
            if !s.is_empty() {
                segments.push(s);
            }
        }
        let joined = if segments.is_empty() {
            ".".to_string()
        } else {
            normalize_path(&segments.join("/"))
        };
        let _ = _interp;
        inst.borrow_mut()
            .attrs
            .insert("_path".to_string(), Value::string(&joined));
        Ok(Value::none())
    }

    /// `str(path)` — returns the path string.
    #[py_name = "Path.__str__"]
    fn path_str(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        Ok(Value::string(p))
    }

    /// `repr(path)` — returns `PosixPath('...')` with the path string
    /// escaped the same way CPython does.
    #[py_name = "Path.__repr__"]
    fn path_repr(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        let escaped = p
            .replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('\t', "\\t")
            .replace('\r', "\\r")
            .replace('\'', "\\'");
        // Use the actual runtime class name so repr(Path('/tmp')) says
        // "PosixPath('/tmp')" (matching CPython) while a bare Path subclass
        // that doesn't override __repr__ shows its real class name.
        let class_name = inst.borrow().class.borrow().name.clone();
        Ok(Value::string(format!("{class_name}('{escaped}')")))
    }

    /// `path / other` — join `other` segment to `path`, returning a new
    /// `Path`.  `other` must be a `str` or a `Path`.
    #[py_name = "Path.__truediv__"]
    fn path_truediv(args) -> Result<Value> {
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument"),
            ));
        }
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let lhs = get_path(&inst, FN_NAME)?;
        let rhs = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            ValueKind::PyInstance(rc) => {
                // Accept Path or PosixPath instances (any path-like instance).
                if !is_path_instance(&rc) {
                    return Ok(Value::not_implemented());
                }
                get_path(&Rc::clone(&rc), FN_NAME)?
            }
            _ => return Ok(Value::not_implemented()),
        };
        // Absolute rhs replaces lhs (CPython semantics).
        let raw = if rhs.starts_with('/') {
            rhs
        } else if rhs.is_empty() {
            lhs
        } else {
            format!("{lhs}/{rhs}")
        };
        Ok(make_path_instance(&normalize_path(&raw)))
    }

    /// `path == other` — compare path strings.
    #[py_name = "Path.__eq__"]
    fn path_eq(args) -> Result<Value> {
        if args.len() != 2 {
            return Ok(Value::not_implemented());
        }
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let lhs = get_path(&inst, FN_NAME)?;
        let rhs = match args[1].value.kind() {
            // CPython: Path('x') == 'x' is False — str is not a Path.
            // Return NotImplemented so the runtime falls back to identity.
            ValueKind::Str(_) => return Ok(Value::not_implemented()),
            ValueKind::PyInstance(rc) => {
                // Only compare against Path or PosixPath instances.
                if !is_path_instance(&rc) {
                    return Ok(Value::not_implemented());
                }
                match get_path(&Rc::clone(&rc), FN_NAME) {
                    Ok(s) => s,
                    Err(_) => return Ok(Value::not_implemented()),
                }
            }
            _ => return Ok(Value::not_implemented()),
        };
        Ok(Value::bool_(lhs == rhs))
    }

    /// `hash(path)` — FNV-1a of the path string.
    #[py_name = "Path.__hash__"]
    fn path_hash(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        Ok(Value::int(fnv1a_hash(&p)))
    }

    /// `path.__fspath__()` — `os.fspath` protocol; returns the path string.
    #[py_name = "Path.__fspath__"]
    fn path_fspath(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        Ok(Value::string(p))
    }

    // ── path component accessors (callable methods, not descriptors) ───────

    /// `path.name()` — the final path component (filename + extension).
    #[py_name = "Path.name"]
    fn path_name(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        let name = p.rsplit('/').next().unwrap_or(&p).to_string();
        Ok(Value::string(name))
    }

    /// `path.parent()` — the logical parent of the path.
    #[py_name = "Path.parent"]
    fn path_parent(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        let parent = match p.rfind('/') {
            // Handles root `/` — parent of `/foo` is `/`.
            Some(0) => "/".to_string(),
            Some(i) => p[..i].to_string(),
            // No slash: parent is `.` (current directory).
            None => ".".to_string(),
        };
        Ok(make_path_instance(&parent))
    }

    /// `path.stem()` — the final component without its suffix.
    #[py_name = "Path.stem"]
    fn path_stem(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        let name = p.rsplit('/').next().unwrap_or(&p);
        // Find last `.` that is not the leading dot (hidden files).
        let stem = match name.rfind('.') {
            Some(i) if i > 0 => &name[..i],
            _ => name,
        };
        Ok(Value::string(stem.to_string()))
    }

    /// `path.suffix()` — the final suffix including `.`, or `''` if none.
    #[py_name = "Path.suffix"]
    fn path_suffix(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        let name = p.rsplit('/').next().unwrap_or(&p);
        let suf = match name.rfind('.') {
            Some(i) if i > 0 => name[i..].to_string(),
            _ => String::new(),
        };
        Ok(Value::string(suf))
    }

    /// `path.parts()` — tuple of path components split by `/`.
    ///
    /// For absolute paths the first element is the anchor: `'/'` for a
    /// single leading slash, or `'//'` for the POSIX.1 double-slash UNC
    /// prefix (which `normalize_path` preserves).  For relative paths the
    /// separator is not included as a separate element.
    #[py_name = "Path.parts"]
    fn path_parts(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        let mut components: Vec<Value> = Vec::new();
        if p.starts_with("//") && !p.starts_with("///") {
            // Double-slash UNC prefix — anchor is '//' per POSIX.1.
            components.push(Value::string("//".to_string()));
            for part in p[2..].split('/') {
                if !part.is_empty() {
                    components.push(Value::string(part.to_string()));
                }
            }
        } else if p.starts_with('/') {
            components.push(Value::string("/".to_string()));
            // Drop the leading slash then split.
            for part in p[1..].split('/') {
                if !part.is_empty() {
                    components.push(Value::string(part.to_string()));
                }
            }
        } else {
            for part in p.split('/') {
                if !part.is_empty() {
                    components.push(Value::string(part.to_string()));
                }
            }
        }
        Ok(Value::tuple(components))
    }

    // ── filesystem predicates ─────────────────────────────────────────────

    /// `path.exists()` — True if the path exists on the filesystem.
    #[py_name = "Path.exists"]
    fn path_exists(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        Ok(Value::bool_(std::path::Path::new(&p).exists()))
    }

    /// `path.is_file()` — True if the path points to a regular file.
    #[py_name = "Path.is_file"]
    fn path_is_file(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        Ok(Value::bool_(std::path::Path::new(&p).is_file()))
    }

    /// `path.is_dir()` — True if the path points to a directory.
    #[py_name = "Path.is_dir"]
    fn path_is_dir(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        Ok(Value::bool_(std::path::Path::new(&p).is_dir()))
    }

    // ── I/O ───────────────────────────────────────────────────────────────

    /// `path.read_text(encoding=None, errors=None)` — read the file as
    /// UTF-8 text.  Raises `FileNotFoundError` if the file does not exist,
    /// `OSError` for other I/O failures.
    #[py_name = "Path.read_text"]
    fn path_read_text(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        // Accept up to 2 extra args (encoding, errors) but ignore them.
        if args.len() > 3 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes at most 2 arguments"),
            ));
        }
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        match std::fs::read_to_string(&p) {
            Ok(contents) => Ok(Value::string(contents)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(PyError::named(
                "FileNotFoundError",
                format!("[Errno 2] No such file or directory: '{p}'"),
            )),
            Err(e) => Err(PyError::named(
                "OSError",
                format!("[Errno {}] {}: '{p}'", e.raw_os_error().unwrap_or(0), e),
            )),
        }
    }

    /// `path.write_text(data, encoding=None, errors=None)` — write `data`
    /// to the file as UTF-8.  Returns the number of characters (code
    /// points) written, matching CPython 3.10+.  Raises `TypeError` if
    /// `data` is not a string.
    #[py_name = "Path.write_text"]
    fn path_write_text(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        if args.len() < 2 || args.len() > 4 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes 1 to 3 arguments"),
            ));
        }
        let data = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}(): data must be str, not {}",
                    pyrust_core::builtin_type_name(&args[1].value),
                ),
            )),
        };
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        // CPython 3.10+ write_text() returns the number of characters
        // (code points) written, not None.
        let char_count = data.chars().count();
        std::fs::write(&p, data.as_bytes()).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                PyError::named(
                    "FileNotFoundError",
                    format!("[Errno 2] No such file or directory: '{p}'"),
                )
            } else {
                PyError::named(
                    "OSError",
                    format!("[Errno {}] {}: '{p}'", e.raw_os_error().unwrap_or(0), e),
                )
            }
        })?;
        Ok(Value::int(char_count as i64))
    }

    /// `path.mkdir(mode=0o777, parents=False, exist_ok=False)` — create
    /// the directory.  If `parents=True` creates all intermediate
    /// directories.  If `exist_ok=True` does not raise when the directory
    /// already exists.  All three parameters may be passed positionally or
    /// by keyword.
    #[py_name = "Path.mkdir"]
    fn path_mkdir(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        // Accept mode, parents, exist_ok by position or keyword name.
        let mut parents = false;
        let mut exist_ok = false;
        let mut pos_index: usize = 0; // positional slot counter (0 = mode, 1 = parents, 2 = exist_ok)
        let mut seen_mode_kw = false;
        let mut seen_parents_kw = false;
        let mut seen_exist_ok_kw = false;
        for a in args.iter().skip(1) {
            match a.name.as_deref() {
                Some("mode") => {
                    if seen_mode_kw {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "{FN_NAME}() got multiple values for argument 'mode'",
                            ),
                        ));
                    }
                    seen_mode_kw = true;
                    // mode is accepted but not used (std::fs doesn't expose umask control).
                }
                Some("parents") => {
                    if seen_parents_kw {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "{FN_NAME}() got multiple values for argument 'parents'",
                            ),
                        ));
                    }
                    parents = matches!(a.value.kind(), ValueKind::Bool(true))
                        || matches!(a.value.kind(), ValueKind::Int(n) if n != 0);
                    seen_parents_kw = true;
                }
                Some("exist_ok") => {
                    if seen_exist_ok_kw {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "{FN_NAME}() got multiple values for argument 'exist_ok'",
                            ),
                        ));
                    }
                    exist_ok = matches!(a.value.kind(), ValueKind::Bool(true))
                        || matches!(a.value.kind(), ValueKind::Int(n) if n != 0);
                    seen_exist_ok_kw = true;
                }
                Some(other) => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "{FN_NAME}() got an unexpected keyword argument '{other}'",
                        ),
                    ));
                }
                None => {
                    match pos_index {
                        0 => { /* mode — ignored */ }
                        1 => {
                            parents = matches!(a.value.kind(), ValueKind::Bool(true))
                                || matches!(a.value.kind(), ValueKind::Int(n) if n != 0);
                        }
                        2 => {
                            exist_ok = matches!(a.value.kind(), ValueKind::Bool(true))
                                || matches!(a.value.kind(), ValueKind::Int(n) if n != 0);
                        }
                        _ => {
                            return Err(PyError::named(
                                "TypeError",
                                format!("{FN_NAME}() takes at most 3 arguments"),
                            ));
                        }
                    }
                    pos_index += 1;
                }
            }
        }
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        let result = if parents {
            std::fs::create_dir_all(&p)
        } else {
            std::fs::create_dir(&p)
        };
        match result {
            Ok(()) => Ok(Value::none()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && exist_ok => {
                Ok(Value::none())
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(PyError::named(
                "FileExistsError",
                format!("[Errno 17] File exists: '{p}'"),
            )),
            Err(e) => Err(PyError::named(
                "OSError",
                format!("[Errno {}] {}: '{p}'", e.raw_os_error().unwrap_or(0), e),
            )),
        }
    }
}

// ── Path and PosixPath class singletons ───────────────────────────────────────
//
// Both classes are built once per thread (matching CPython's identity semantics
// — `import pathlib; pathlib.Path is pathlib.Path` is True).
//
// `FN_PREFIX` resolves to `"pathlib."` (injected by `pyrust_builtin_modules!`),
// so the registered names are `"pathlib.Path.__init__"` etc.

/// (method-short, registry-name) pairs for every `Path` method.
///
/// These must match the `#[py_name = "Path.<method>"]` annotations above.
const PATH_METHODS: &[(&str, &str)] = &[
    ("__new__", "pathlib.Path.__new__"),
    ("__init__", "pathlib.Path.__init__"),
    ("__str__", "pathlib.Path.__str__"),
    ("__repr__", "pathlib.Path.__repr__"),
    ("__truediv__", "pathlib.Path.__truediv__"),
    ("__eq__", "pathlib.Path.__eq__"),
    ("__hash__", "pathlib.Path.__hash__"),
    ("__fspath__", "pathlib.Path.__fspath__"),
    ("name", "pathlib.Path.name"),
    ("parent", "pathlib.Path.parent"),
    ("stem", "pathlib.Path.stem"),
    ("suffix", "pathlib.Path.suffix"),
    ("parts", "pathlib.Path.parts"),
    ("exists", "pathlib.Path.exists"),
    ("is_file", "pathlib.Path.is_file"),
    ("is_dir", "pathlib.Path.is_dir"),
    ("read_text", "pathlib.Path.read_text"),
    ("write_text", "pathlib.Path.write_text"),
    ("mkdir", "pathlib.Path.mkdir"),
];

thread_local! {
    /// The `Path` class singleton — shared across all `Path` instances.
    static PATH_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        for (short, py_full) in PATH_METHODS {
            attrs.insert((*short).to_string(), Value::builtin_function(py_full));
        }
        Rc::new(RefCell::new(PyClass {
            name: "Path".to_string(),
            qualname: "Path".to_string(),
            base: None,
            attrs,
        }))
    };

    /// The `PosixPath` class singleton — `base` points to `PATH_CLASS` so
    /// every `Path` method is inherited via `lookup_class_attr`'s chain walk.
    /// `PosixPath` has no methods of its own; it exists purely to give
    /// instances the correct runtime type name (CPython parity on Linux).
    static POSIX_PATH_CLASS: Rc<RefCell<PyClass>> = {
        PATH_CLASS.with(|path_class| {
            Rc::new(RefCell::new(PyClass {
                name: "PosixPath".to_string(),
                qualname: "PosixPath".to_string(),
                base: Some(Rc::clone(path_class)),
                attrs: IndexMap::new(),
            }))
        })
    };
}

/// Return a `Value::py_class` wrapping the thread-local `Path` singleton.
fn make_path_class_value() -> Value {
    PATH_CLASS.with(|c| Value::py_class(Rc::clone(c)))
}

/// Return a `Value::py_class` wrapping the thread-local `PosixPath` singleton.
fn make_posix_path_class_value() -> Value {
    POSIX_PATH_CLASS.with(|c| Value::py_class(Rc::clone(c)))
}
