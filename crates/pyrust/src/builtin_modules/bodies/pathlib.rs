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
use crate::interpreter::{ExpandedCallArg, NativeIterFrame};
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value, ValueKind};
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
        let mut attrs = InstanceAttrs::new();
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
                attrs: InstanceAttrs::new(),
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

    /// `path.joinpath(*args)` — join one or more path components to this
    /// path.  Equivalent to repeatedly applying `path / part` for each
    /// argument.  If any argument is absolute it replaces all prior
    /// components (CPython semantics).
    #[py_name = "Path.joinpath"]
    fn path_joinpath(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let mut current = get_path(&inst, FN_NAME)?;
        for a in args.iter().skip(1) {
            let part = match a.value.kind() {
                ValueKind::Str(s) => s.to_string(),
                ValueKind::PyInstance(rc) => {
                    if !is_path_instance(&rc) {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "{FN_NAME}(): argument must be str or Path, not {}",
                                rc.borrow().class.borrow().name,
                            ),
                        ));
                    }
                    get_path(&Rc::clone(&rc), FN_NAME)?
                }
                _ => return Err(PyError::named(
                    "TypeError",
                    format!(
                        "{FN_NAME}(): argument must be str or Path, not {}",
                        pyrust_core::builtin_type_name(&a.value),
                    ),
                )),
            };
            current = if part.starts_with('/') {
                normalize_path(&part)
            } else if part.is_empty() {
                current
            } else {
                normalize_path(&format!("{current}/{part}"))
            };
        }
        Ok(make_path_instance(&current))
    }

    // ── path component accessors (callable methods, not descriptors) ───────

    /// `path.name()` — the final path component (filename + extension).
    ///
    /// Returns `''` for paths that are pure anchors: `"."` (current dir),
    /// `"/"` (root), and `"//"` (POSIX.1 double-slash prefix).  This matches
    /// CPython 3.12 where `Path('.').name == ''` and `Path('/').name == ''`.
    #[py_name = "Path.name"]
    fn path_name(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        // Pure-anchor paths have no file-name component.
        if p == "." || p == "/" || p == "//" {
            return Ok(Value::string(String::new()));
        }
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
    ///
    /// Names composed entirely of dots (`.`, `..`) have no suffix, so stem
    /// equals the name.  For anchor-only paths the stem is `''`.  This
    /// matches CPython 3.12: `Path('..').stem == '..'`,
    /// `Path('.hidden').stem == '.hidden'`, `Path('foo.txt').stem == 'foo'`.
    #[py_name = "Path.stem"]
    fn path_stem(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        // Pure-anchor paths have no name component.
        if p == "." || p == "/" || p == "//" {
            return Ok(Value::string(String::new()));
        }
        let name = p.rsplit('/').next().unwrap_or(&p);
        // A name composed only of dots (e.g. `..`) has no suffix; its stem is
        // the whole name.  Otherwise split at the last `.` that is not the
        // first character (leading-dot hidden files also have no suffix).
        let stem = if name.chars().all(|c| c == '.') {
            name
        } else {
            match name.rfind('.') {
                Some(i) if i > 0 => &name[..i],
                _ => name,
            }
        };
        Ok(Value::string(stem.to_string()))
    }

    /// `path.suffix()` — the final suffix including `.`, or `''` if none.
    ///
    /// Names composed entirely of dots and anchor-only paths have no suffix.
    /// This matches CPython 3.12: `Path('..').suffix == ''`,
    /// `Path('.hidden').suffix == ''`, `Path('foo.txt').suffix == '.txt'`.
    #[py_name = "Path.suffix"]
    fn path_suffix(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        // Pure-anchor paths have no name component.
        if p == "." || p == "/" || p == "//" {
            return Ok(Value::string(String::new()));
        }
        let name = p.rsplit('/').next().unwrap_or(&p);
        // All-dots names (`.`, `..`) have no suffix.
        let suf = if name.chars().all(|c| c == '.') {
            String::new()
        } else {
            match name.rfind('.') {
                Some(i) if i > 0 => name[i..].to_string(),
                _ => String::new(),
            }
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
            Err(e) => Err(PyError::from_io_error(&e, Some(&p))),
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
        std::fs::write(&p, data.as_bytes())
            .map_err(|e| PyError::from_io_error(&e, Some(&p)))?;
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
            Err(e) => Err(PyError::from_io_error(&e, Some(&p))),
        }
    }

    // ── class methods (cwd, home) ─────────────────────────────────────────────

    /// `Path.cwd()` — return a new path representing the current working
    /// directory.  Works both as `Path.cwd()` (class call) and as
    /// `Path('/tmp').cwd()` (instance call) — the receiver is ignored.
    /// <https://docs.python.org/3/library/pathlib.html#pathlib.Path.cwd>
    #[py_name = "Path.cwd"]
    fn path_cwd(_args) -> Result<Value> {
        let _ = _interp;
        let cwd = std::env::current_dir()
            .map_err(|e| PyError::from_io_error(&e, None))?;
        let p = cwd.to_string_lossy();
        Ok(make_path_instance(&p))
    }

    /// `Path.home()` — return a new path representing the user's home
    /// directory.  Works both as `Path.home()` (class call) and as
    /// `Path('/tmp').home()` (instance call) — the receiver is ignored.
    /// <https://docs.python.org/3/library/pathlib.html#pathlib.Path.home>
    #[py_name = "Path.home"]
    fn path_home(_args) -> Result<Value> {
        let _ = _interp;
        // Use the HOME env var on POSIX (same as CPython's fallback path),
        // falling back to `dirs::home_dir` equivalent via `std::env::var`.
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map_err(|_| PyError::named(
                "RuntimeError",
                "Could not determine home directory".to_string(),
            ))?;
        Ok(make_path_instance(&home))
    }

    // ── pure path predicates ──────────────────────────────────────────────────

    /// `path.is_absolute()` — True if the path is absolute (starts with `/`
    /// on POSIX).
    /// <https://docs.python.org/3/library/pathlib.html#pathlib.PurePath.is_absolute>
    #[py_name = "Path.is_absolute"]
    fn path_is_absolute(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        Ok(Value::bool_(p.starts_with('/')))
    }

    /// `path.resolve(strict=False)` — make the path absolute, resolving
    /// symlinks.  Returns a `PosixPath` with the canonicalised absolute path.
    /// When `strict=True` the path must exist; if it does not,
    /// `FileNotFoundError` is raised (CPython 3.6+ parity).
    /// <https://docs.python.org/3/library/pathlib.html#pathlib.Path.resolve>
    #[py_name = "Path.resolve"]
    fn path_resolve(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        // Parse the `strict` kwarg (Python 3.6+, default False).
        let mut strict = false;
        for a in args.iter().skip(1) {
            match a.name.as_deref() {
                Some("strict") => {
                    strict = matches!(a.value.kind(), ValueKind::Bool(true))
                        || matches!(a.value.kind(), ValueKind::Int(n) if n != 0);
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
                    // First positional after self is `strict`.
                    strict = matches!(a.value.kind(), ValueKind::Bool(true))
                        || matches!(a.value.kind(), ValueKind::Int(n) if n != 0);
                }
            }
        }
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        match std::fs::canonicalize(&p) {
            Ok(resolved) => Ok(make_path_instance(&resolved.to_string_lossy())),
            Err(e) => {
                if strict {
                    // strict=True: propagate the OS error as FileNotFoundError.
                    Err(PyError::from_io_error(&e, Some(&p)))
                } else {
                    // strict=False (default): build an absolute path without
                    // resolving symlinks — matching CPython 3.12 behaviour.
                    let raw = std::path::Path::new(&p);
                    let abs = if raw.is_absolute() {
                        raw.to_path_buf()
                    } else {
                        std::env::current_dir()
                            .unwrap_or_else(|_| std::path::PathBuf::from("/"))
                            .join(raw)
                    };
                    Ok(make_path_instance(&abs.to_string_lossy()))
                }
            }
        }
    }

    // ── I/O (bytes) ───────────────────────────────────────────────────────────

    /// `path.read_bytes()` — read the file as raw bytes.
    /// <https://docs.python.org/3/library/pathlib.html#pathlib.Path.read_bytes>
    #[py_name = "Path.read_bytes"]
    fn path_read_bytes(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments"),
            ));
        }
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        match std::fs::read(&p) {
            Ok(bytes) => Ok(Value::bytes(bytes)),
            Err(e) => Err(PyError::from_io_error(&e, Some(&p))),
        }
    }

    /// `path.write_bytes(data)` — write bytes to the file.  Returns the
    /// number of bytes written, matching CPython 3.10+.
    /// <https://docs.python.org/3/library/pathlib.html#pathlib.Path.write_bytes>
    #[py_name = "Path.write_bytes"]
    fn path_write_bytes(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument"),
            ));
        }
        let data = match args[1].value.kind() {
            ValueKind::Bytes(b) => b.to_vec(),
            // Accept bytearray as a bytes-like object, matching CPython.
            ValueKind::BuiltinObject { .. } => {
                match pyrust_builtins::bytearray::as_bytearray_snapshot(&args[1].value) {
                    Some(v) => v,
                    None => return Err(PyError::named(
                        "TypeError",
                        format!(
                            "{FN_NAME}(): data must be bytes-like, not {}",
                            pyrust_core::builtin_type_name(&args[1].value),
                        ),
                    )),
                }
            }
            _ => return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}(): data must be bytes-like, not {}",
                    pyrust_core::builtin_type_name(&args[1].value),
                ),
            )),
        };
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        let n = data.len();
        std::fs::write(&p, &data)
            .map_err(|e| PyError::from_io_error(&e, Some(&p)))?;
        Ok(Value::int(n as i64))
    }

    /// `path.open(mode='r', buffering=-1, encoding=None, errors=None,
    /// newline=None)` — open the file and return a file object.  Thin
    /// wrapper around the built-in `open()` using the path string.
    /// <https://docs.python.org/3/library/pathlib.html#pathlib.Path.open>
    #[py_name = "Path.open"]
    fn path_open(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let p = get_path(&inst, FN_NAME)?;
        // Extract mode and encoding from args (positional or keyword).
        let mut mode = "r".to_string();
        let mut encoding: Option<String> = None;
        for a in args.iter().skip(1) {
            match a.name.as_deref() {
                Some("mode") => {
                    if let ValueKind::Str(s) = a.value.kind() {
                        mode = s.to_string();
                    }
                }
                Some("encoding") => {
                    if let ValueKind::Str(s) = a.value.kind() {
                        encoding = Some(s.to_string());
                    }
                }
                Some("buffering") | Some("errors") | Some("newline") => {
                    // Accepted and ignored, matching the write_text approach.
                }
                Some(other) => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() got an unexpected keyword argument '{other}'"),
                    ));
                }
                None => {
                    // First positional after self is mode.
                    if let ValueKind::Str(s) = a.value.kind() {
                        mode = s.to_string();
                    }
                }
            }
        }
        let _ = _interp;
        pyrust_builtins::file::open(&p, &mode, encoding.as_deref(), true)
    }

    // ── file removal ──────────────────────────────────────────────────────────

    /// `path.unlink(missing_ok=False)` — delete the file.  If
    /// `missing_ok=True` silently ignores the file not existing.
    /// <https://docs.python.org/3/library/pathlib.html#pathlib.Path.unlink>
    #[py_name = "Path.unlink"]
    fn path_unlink(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let mut missing_ok = false;
        for a in args.iter().skip(1) {
            match a.name.as_deref() {
                Some("missing_ok") => {
                    missing_ok = matches!(a.value.kind(), ValueKind::Bool(true))
                        || matches!(a.value.kind(), ValueKind::Int(n) if n != 0);
                }
                Some(other) => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() got an unexpected keyword argument '{other}'"),
                    ));
                }
                None => {
                    // positional missing_ok
                    missing_ok = matches!(a.value.kind(), ValueKind::Bool(true))
                        || matches!(a.value.kind(), ValueKind::Int(n) if n != 0);
                }
            }
        }
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        match std::fs::remove_file(&p) {
            Ok(()) => Ok(Value::none()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && missing_ok => Ok(Value::none()),
            Err(e) => Err(PyError::from_io_error(&e, Some(&p))),
        }
    }

    // ── directory iteration ───────────────────────────────────────────────────

    /// `path.iterdir()` — yield all children of the directory.  Each child
    /// is returned as an absolute `PosixPath`.  Raises `NotADirectoryError`
    /// (via `OSError`) if the path is not a directory.
    /// <https://docs.python.org/3/library/pathlib.html#pathlib.Path.iterdir>
    #[py_name = "Path.iterdir"]
    fn path_iterdir(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments"),
            ));
        }
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        let entries = std::fs::read_dir(&p)
            .map_err(|e| PyError::from_io_error(&e, Some(&p)))?;
        let mut items: Vec<Value> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| PyError::from_io_error(&e, Some(&p)))?;
            let child_path = entry.path().to_string_lossy().into_owned();
            items.push(make_path_instance(&child_path));
        }
        Ok(Value::generator(Box::new(NativeIterFrame {
            items,
            pos: 0,
            type_name: "generator",
        })))
    }

    /// `path.glob(pattern)` — yield all paths matching `pattern` relative to
    /// this directory.  Supports `*` (single directory level) and `**`
    /// (recursive).  Returns an iterator of `PosixPath` instances.
    /// <https://docs.python.org/3/library/pathlib.html#pathlib.Path.glob>
    #[py_name = "Path.glob"]
    fn path_glob(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument"),
            ));
        }
        let pattern = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}(): pattern must be str, not {}",
                    pyrust_core::builtin_type_name(&args[1].value),
                ),
            )),
        };
        let _ = _interp;
        let base = get_path(&inst, FN_NAME)?;
        let items = glob_collect(&base, &pattern)?;
        Ok(Value::generator(Box::new(NativeIterFrame {
            items,
            pos: 0,
            type_name: "generator",
        })))
    }

    // ── path mutation (with_*) ────────────────────────────────────────────────

    /// `path.with_name(name)` — return a new path with the file name
    /// replaced by `name`.  Raises `ValueError` if the path has no name
    /// (e.g. `Path('/')`) or if `name` is empty / contains a slash.
    /// <https://docs.python.org/3/library/pathlib.html#pathlib.PurePath.with_name>
    #[py_name = "Path.with_name"]
    fn path_with_name(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument"),
            ));
        }
        let name = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}(): name must be str, not {}",
                    pyrust_core::builtin_type_name(&args[1].value),
                ),
            )),
        };
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        // Validate: name must be non-empty and contain no slashes.
        if name.is_empty() {
            return Err(PyError::named(
                "ValueError",
                format!("Invalid name '{name}'"),
            ));
        }
        if name.contains('/') {
            return Err(PyError::named(
                "ValueError",
                format!("Invalid name '{name}'"),
            ));
        }
        // The current path must have a non-empty name component.
        let current_name = if p == "." || p == "/" || p == "//" {
            return Err(PyError::named(
                "ValueError",
                format!("{self_repr} has an empty name", self_repr = repr_path(&p)),
            ));
        } else {
            p.rsplit('/').next().unwrap_or(&p)
        };
        let _ = current_name;
        let new_path = match p.rfind('/') {
            Some(i) => format!("{}/{name}", &p[..i]),
            None => name,
        };
        Ok(make_path_instance(&new_path))
    }

    /// `path.with_stem(stem)` — return a new path with the stem (name
    /// without suffix) replaced.  Raises `ValueError` if the path has no
    /// name.
    /// <https://docs.python.org/3/library/pathlib.html#pathlib.PurePath.with_stem>
    #[py_name = "Path.with_stem"]
    fn path_with_stem(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument"),
            ));
        }
        let stem = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}(): stem must be str, not {}",
                    pyrust_core::builtin_type_name(&args[1].value),
                ),
            )),
        };
        let _ = _interp;
        let p = get_path(&inst, FN_NAME)?;
        if p == "." || p == "/" || p == "//" {
            return Err(PyError::named(
                "ValueError",
                format!("{self_repr} has an empty name", self_repr = repr_path(&p)),
            ));
        }
        let name = p.rsplit('/').next().unwrap_or(&p);
        // Compute the current suffix (same logic as path_suffix).
        let suffix = if name.chars().all(|c| c == '.') {
            String::new()
        } else {
            match name.rfind('.') {
                Some(i) if i > 0 => name[i..].to_string(),
                _ => String::new(),
            }
        };
        let new_name = format!("{stem}{suffix}");
        let new_path = match p.rfind('/') {
            Some(i) => format!("{}/{new_name}", &p[..i]),
            None => new_name,
        };
        Ok(make_path_instance(&new_path))
    }

    /// `path.with_suffix(suffix)` — return a new path with the suffix
    /// replaced.  `suffix` must start with `.` unless it is empty (which
    /// removes the suffix).  Raises `ValueError` on invalid suffix.
    /// <https://docs.python.org/3/library/pathlib.html#pathlib.PurePath.with_suffix>
    #[py_name = "Path.with_suffix"]
    fn path_with_suffix(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument"),
            ));
        }
        let suffix = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}(): suffix must be str, not {}",
                    pyrust_core::builtin_type_name(&args[1].value),
                ),
            )),
        };
        let _ = _interp;
        // Validate suffix: empty is allowed (removes suffix); otherwise must
        // start with '.' and not be a bare '.'.  Multiple dots (e.g. '.tar.gz')
        // are allowed (CPython 3.12 parity).
        if !suffix.is_empty() {
            if !suffix.starts_with('.') {
                return Err(PyError::named(
                    "ValueError",
                    format!("Invalid suffix '{suffix}'"),
                ));
            }
            // A bare '.' is not a valid suffix — must have at least one char after.
            if suffix == "." {
                return Err(PyError::named(
                    "ValueError",
                    format!("Invalid suffix '{suffix}'"),
                ));
            }
        }
        let p = get_path(&inst, FN_NAME)?;
        // CPython raises ValueError when the path has no name component (root,
        // ".", or "//").  Compute the name first and guard before proceeding.
        let name = if p == "." || p == "/" || p == "//" {
            return Err(PyError::named(
                "ValueError",
                format!("{self_repr} has an empty name", self_repr = repr_path(&p)),
            ));
        } else {
            p.rsplit('/').next().unwrap_or(&p).to_string()
        };
        // Compute the stem of the current name (same logic as path_stem).
        let stem = if name.chars().all(|c| c == '.') {
            name.clone()
        } else {
            match name.rfind('.') {
                Some(i) if i > 0 => name[..i].to_string(),
                _ => name.clone(),
            }
        };
        let new_name = format!("{stem}{suffix}");
        let new_path = match p.rfind('/') {
            Some(i) => format!("{}/{new_name}", &p[..i]),
            None => new_name,
        };
        Ok(make_path_instance(&new_path))
    }
}

// ── Path and PosixPath class singletons ───────────────────────────────────────
//
// Both classes are built once per thread (matching CPython's identity semantics
// — `import pathlib; pathlib.Path is pathlib.Path` is True).
//
// `FN_PREFIX` resolves to `"pathlib."` (injected by `pyrust_builtin_modules!`),
// so the registered names are `"pathlib.Path.__init__"` etc.

/// (method-short, registry-name) pairs for every `Path` method that should be
/// exposed as a regular callable method.
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
    ("joinpath", "pathlib.Path.joinpath"),
    ("exists", "pathlib.Path.exists"),
    ("is_file", "pathlib.Path.is_file"),
    ("is_dir", "pathlib.Path.is_dir"),
    ("is_absolute", "pathlib.Path.is_absolute"),
    ("resolve", "pathlib.Path.resolve"),
    ("read_text", "pathlib.Path.read_text"),
    ("write_text", "pathlib.Path.write_text"),
    ("read_bytes", "pathlib.Path.read_bytes"),
    ("write_bytes", "pathlib.Path.write_bytes"),
    ("open", "pathlib.Path.open"),
    ("mkdir", "pathlib.Path.mkdir"),
    ("unlink", "pathlib.Path.unlink"),
    ("iterdir", "pathlib.Path.iterdir"),
    ("glob", "pathlib.Path.glob"),
    ("with_name", "pathlib.Path.with_name"),
    ("with_stem", "pathlib.Path.with_stem"),
    ("with_suffix", "pathlib.Path.with_suffix"),
    // cwd and home are classmethods in CPython, but we expose them as
    // regular callables on the class (receiver-agnostic) so both
    // `Path.cwd()` and `Path('/tmp').cwd()` work (matching CPython).
    ("cwd", "pathlib.Path.cwd"),
    ("home", "pathlib.Path.home"),
];

/// (attr-short, registry-name) pairs for `Path` attributes that CPython exposes
/// as read-only properties.  These are stored in the class attrs as `property`
/// descriptors so that `path.name` evaluates to the string value directly,
/// matching CPython's descriptor protocol.  Calling them without `()` is the
/// correct access pattern.
const PATH_PROPERTIES: &[(&str, &str)] = &[
    ("name", "pathlib.Path.name"),
    ("parent", "pathlib.Path.parent"),
    ("stem", "pathlib.Path.stem"),
    ("suffix", "pathlib.Path.suffix"),
    ("parts", "pathlib.Path.parts"),
];

thread_local! {
    /// The `Path` class singleton — shared across all `Path` instances.
    static PATH_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        for (short, py_full) in PATH_METHODS {
            attrs.insert((*short).to_string(), Value::builtin_function(py_full));
        }
        // Property descriptors: stored as read-only property objects so that
        // `path.name` evaluates to the value directly (CPython parity).  The
        // interpreter's `get_attr` dispatches `with_property` before the
        // ordinary class-attr path, calling the getter with `self`.
        for (short, py_full) in PATH_PROPERTIES {
            let getter = Value::builtin_function(py_full);
            attrs.insert(
                (*short).to_string(),
                pyrust_builtins::property::property(getter, Value::none(), Value::none()),
            );
        }
        Rc::new(RefCell::new(PyClass::new("Path", "Path", None, attrs)))
    };

    /// The `PosixPath` class singleton — `base` points to `PATH_CLASS` so
    /// every `Path` method is inherited via `lookup_class_attr`'s chain walk.
    /// `PosixPath` has no methods of its own; it exists purely to give
    /// instances the correct runtime type name (CPython parity on Linux).
    static POSIX_PATH_CLASS: Rc<RefCell<PyClass>> = {
        PATH_CLASS.with(|path_class| {
            let posix = Rc::new(RefCell::new(PyClass::new(
                "PosixPath",
                "PosixPath",
                Some(Rc::clone(path_class)),
                IndexMap::new(),
            )));
            path_class.borrow().subclasses.borrow_mut().push(Rc::downgrade(&posix));
            posix
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

/// Format a path as a `PosixPath('...')` repr for use in `ValueError` messages,
/// matching CPython's `str(path)` → PosixPath repr in error strings.
fn repr_path(p: &str) -> String {
    format!("PosixPath('{p}')")
}

/// Collect all paths under `base` that match `pattern`, returning them as a
/// `Vec<Value>` of `PosixPath` instances.
///
/// Supports:
/// - `*`  — wildcard within a single path component (no directory traversal).
/// - `**` — recursive wildcard (all descendants).
/// - `?`  — single-character wildcard within a component.
/// - `[…]` — character class within a component.
///
/// Non-recursive patterns (no `**`) are resolved in a single directory
/// level, matching `glob.glob(base + '/' + pattern)` semantics.
/// Recursive `**` patterns walk the entire subtree.
fn glob_collect(base: &str, pattern: &str) -> Result<Vec<Value>> {
    // Split pattern into parts on `/`.
    let parts: Vec<&str> = pattern.split('/').collect();
    let base_path = std::path::Path::new(base);

    // Check for `**` in pattern — triggers recursive walk.
    let has_recursive = parts.iter().any(|p| *p == "**");

    let mut results: Vec<std::path::PathBuf> = Vec::new();

    if has_recursive {
        // Collect all descendants via `collect_recursive`, then filter.
        let all = collect_all_descendants(base_path);
        for candidate in &all {
            let rel = candidate
                .strip_prefix(base_path)
                .unwrap_or(candidate);
            let rel_str = rel.to_string_lossy();
            if glob_pattern_matches(pattern, &rel_str) {
                results.push(candidate.clone());
            }
        }
    } else {
        // Non-recursive: resolve step by step.
        let mut current_dirs: Vec<std::path::PathBuf> = vec![base_path.to_path_buf()];
        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() {
                continue;
            }
            let is_last = i == parts.len() - 1;
            let mut next: Vec<std::path::PathBuf> = Vec::new();
            for dir in &current_dirs {
                let entries = match std::fs::read_dir(dir) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if glob_component_matches(part, &name_str) {
                        let child = entry.path();
                        if is_last {
                            results.push(child);
                        } else if child.is_dir() {
                            next.push(child);
                        }
                    }
                }
            }
            if !is_last {
                current_dirs = next;
            }
        }
    }

    Ok(results
        .into_iter()
        .map(|p| make_path_instance(&p.to_string_lossy()))
        .collect())
}

/// Recursively collect all entries under `dir`, including `dir` itself.
fn collect_all_descendants(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let sub = collect_all_descendants(&path);
            out.push(path);
            out.extend(sub);
        } else {
            out.push(path);
        }
    }
    out
}

/// Match a full glob pattern (with `/` separators and `**`) against a relative
/// path string (also `/`-separated).  Used for the recursive `**` case.
fn glob_pattern_matches(pattern: &str, path: &str) -> bool {
    let pat_parts: Vec<&str> = pattern.split('/').collect();
    let path_parts: Vec<&str> = path.split('/').collect();
    glob_parts_match(&pat_parts, &path_parts)
}

/// Recursive pattern matcher over slices of path components.
fn glob_parts_match(pat: &[&str], path: &[&str]) -> bool {
    match (pat.first(), path.first()) {
        (None, None) => true,
        (None, _) => false,
        (Some(&"**"), _) => {
            // `**` matches zero or more path components.
            let rest_pat = &pat[1..];
            // Try matching rest_pat against every suffix of path.
            for skip in 0..=path.len() {
                if glob_parts_match(rest_pat, &path[skip..]) {
                    return true;
                }
            }
            false
        }
        (Some(_p), None) => {
            // Pattern has more parts but path is exhausted.
            // Only matches if remaining pattern parts are all `**`.
            pat.iter().all(|x| *x == "**")
        }
        (Some(p), Some(q)) => {
            glob_component_matches(p, q) && glob_parts_match(&pat[1..], &path[1..])
        }
    }
}

/// Match a single glob component (no `/`) against a single path component.
/// Supports `*`, `?`, and `[…]` within the component.
fn glob_component_matches(pattern: &str, name: &str) -> bool {
    glob_match(pattern.as_bytes(), name.as_bytes())
}

/// Byte-level glob matcher for a single path component.  Handles:
/// - `*`  — any sequence of bytes (but not `/`, which can't appear in a
///           component anyway).
/// - `?`  — exactly one byte.
/// - `[…]` — character class (only simple ranges, no negation for now).
fn glob_match(pat: &[u8], s: &[u8]) -> bool {
    match (pat.first(), s.first()) {
        (None, None) => true,
        (None, _) => false,
        (Some(b'*'), _) => {
            // `*` matches zero or more bytes.
            for skip in 0..=s.len() {
                if glob_match(&pat[1..], &s[skip..]) {
                    return true;
                }
            }
            false
        }
        (Some(b'?'), Some(_)) => glob_match(&pat[1..], &s[1..]),
        (Some(b'?'), None) => false,
        (Some(b'['), _) => {
            // Find closing `]`.
            let close = pat[1..].iter().position(|&b| b == b']');
            let close = match close {
                Some(i) => i + 1, // index in pat
                None => {
                    // Malformed class — treat `[` as literal.
                    if s.first() == Some(&b'[') {
                        return glob_match(&pat[1..], &s[1..]);
                    }
                    return false;
                }
            };
            let class = &pat[1..close];
            let rest_pat = &pat[close + 1..];
            let c = match s.first() {
                Some(&c) => c,
                None => return false,
            };
            let matched = char_class_matches(class, c);
            matched && glob_match(rest_pat, &s[1..])
        }
        (Some(&p), Some(&q)) => p == q && glob_match(&pat[1..], &s[1..]),
        (Some(_), None) => false,
    }
}

/// Check if byte `c` belongs to a character class spec `class` (the bytes
/// between `[` and `]`).  Supports `a-z` ranges, literal characters, and
/// negation via a leading `!` (e.g. `[!a-z]`, `[!abc]`).  This matches
/// CPython's `fnmatch` semantics: only `!` is the negation marker; `^` is
/// treated as a literal character.
fn char_class_matches(class: &[u8], c: u8) -> bool {
    // Detect and strip a leading `!` negation character.  Only treat `!` as
    // the negation marker when the class has at least one more byte after it;
    // a bare `[!]` (class = b"!") is a degenerate case where the only element
    // is the literal `!` — stripping it would leave an empty class that always
    // returns true under negation, breaking CPython parity.
    let (negated, class) = if class.len() > 1 && class[0] == b'!' {
        (true, &class[1..])
    } else {
        (false, class)
    };

    let mut i = 0;
    let mut matched = false;
    while i < class.len() {
        if i + 2 < class.len() && class[i + 1] == b'-' {
            // Range: e.g. `a-z`.
            if c >= class[i] && c <= class[i + 2] {
                matched = true;
            }
            i += 3;
        } else {
            if class[i] == c {
                matched = true;
            }
            i += 1;
        }
    }

    if negated { !matched } else { matched }
}
