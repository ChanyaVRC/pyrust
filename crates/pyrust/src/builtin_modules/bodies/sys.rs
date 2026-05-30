// `sys` module — included into `pub mod sys { … }` declared by the
// `pyrust_builtin_modules!` invocation in
// `builtin_modules/mod.rs`.  `MODULE_NAME` is injected from
// the outer scope; no name literal appears in this file.
//
// Reference: <https://docs.python.org/3/library/sys.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{
    get_call_depth, get_recursion_limit, instantiate_exception, lookup_name_in_module,
    reject_keyword_args_expanded, set_recursion_limit, value_type_name_str,
};
use crate::value::{PyClass, PyInstance, Value, ValueKind};
use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "version"      => Value::string("PyRust 0.2"),
        "argv"         => Value::list(Vec::new()),
        // CPython: sys.maxsize — the largest positive integer that fits in
        // Py_ssize_t (equivalent to i64::MAX on 64-bit platforms, which is
        // the only target pyrust supports).
        // <https://docs.python.org/3/library/sys.html#sys.maxsize>
        "maxsize"      => Value::int(i64::MAX),
        // CPython: sys.platform — identifier for the OS.
        // <https://docs.python.org/3/library/sys.html#sys.platform>
        "platform"     => Value::string(sys_platform()),
        // CPython: sys.version_info — named tuple with major/minor/micro/
        // releaselevel/serial.  We return a PyInstance of the version_info
        // class with named attributes + rich-comparison and sequence methods
        // so that both `sys.version_info.major` and
        // `sys.version_info >= (3, 12)` work.
        // <https://docs.python.org/3/library/sys.html#sys.version_info>
        "version_info" => make_version_info(),
        // CPython: sys.stdout / sys.stderr / sys.stdin — TextIOWrapper
        // objects wrapping the standard I/O handles.
        // <https://docs.python.org/3/library/sys.html#sys.stdout>
        "stdout"       => pyrust_builtins::file::make_stdout(),
        "stderr"       => pyrust_builtins::file::make_stderr(),
        "stdin"        => pyrust_builtins::file::make_stdin(),
        // CPython: sys.path — list of strings that specifies the search path
        // for modules.  Initialised to [""] (the empty string represents the
        // current working directory), matching CPython's default when no
        // PYTHONPATH is set and no site customisation runs.
        // <https://docs.python.org/3/library/sys.html#sys.path>
        "path"         => Value::list(vec![Value::string("")]),
        // CPython: sys.modules — dictionary of already-imported modules.
        // Starts empty; keeping it in sync with actual imports is out of
        // scope for this implementation.
        // <https://docs.python.org/3/library/sys.html#sys.modules>
        "modules"      => Value::dict(indexmap::IndexMap::new()),
        // CPython: sys.executable — string giving the absolute path of the
        // Python interpreter binary.  pyrust uses an empty string because
        // there is no single well-defined binary path.
        // <https://docs.python.org/3/library/sys.html#sys.executable>
        "executable"   => Value::string(""),
        // CPython: sys.byteorder — "little" or "big" reflecting native byte
        // order of the running platform.
        // <https://docs.python.org/3/library/sys.html#sys.byteorder>
        "byteorder"    => Value::string(sys_byteorder()),
        // CPython: sys.prefix / exec_prefix / base_prefix / base_exec_prefix
        // — installation directory paths.  pyrust has no installation, so all
        // four are the empty string.
        // <https://docs.python.org/3/library/sys.html#sys.prefix>
        "prefix"         => Value::string(""),
        "exec_prefix"    => Value::string(""),
        "base_prefix"    => Value::string(""),
        "base_exec_prefix" => Value::string(""),
        // CPython: sys.hexversion — version encoded as a single integer.
        // 3.12.0 final → 0x030c00f0
        // (major<<24 | minor<<16 | micro<<8 | 0xf0 for 'final' | serial)
        // <https://docs.python.org/3/library/sys.html#sys.hexversion>
        "hexversion"   => Value::int(0x030c_00f0),
        // CPython: sys.copyright — string containing copyright pertaining to
        // the Python interpreter.  pyrust returns an empty string.
        // <https://docs.python.org/3/library/sys.html#sys.copyright>
        "copyright"    => Value::string(""),
        // CPython: sys.flags — named struct exposing command-line flag status.
        // pyrust uses a PyInstance of the `flags` class with all fields set to
        // their default zero values, matching the normal-run state in CPython.
        // <https://docs.python.org/3/library/sys.html#sys.flags>
        "flags"        => make_flags(),
    }

    /// CPython: sys.exit([arg]) — raises `SystemExit(arg)`.
    /// <https://docs.python.org/3/library/sys.html#sys.exit>
    fn exit(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let arg = if args.is_empty() { Value::int(0) } else { args[0].value.clone() };
        let class = match lookup_name_in_module(&_interp.env, "SystemExit") {
            Some(v) => match v.kind() {
                ValueKind::PyClass(c) => Rc::clone(c),
                _ => return Err(PyError::Runtime(
                    "built-in exception 'SystemExit' is not defined".to_string(),
                )),
            },
            None => return Err(PyError::Runtime(
                "built-in exception 'SystemExit' is not defined".to_string(),
            )),
        };
        let exc = instantiate_exception(class, vec![arg]);
        Err(PyError::Raised(exc))
    }

    /// CPython: sys.getrecursionlimit() — return the current recursion limit.
    /// <https://docs.python.org/3/library/sys.html#sys.getrecursionlimit>
    fn getrecursionlimit(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "sys.getrecursionlimit() takes no arguments ({} given)",
                    args.len()
                ),
            ));
        }
        Ok(Value::int(get_recursion_limit() as i64))
    }

    /// CPython: sys.setrecursionlimit(limit) — set the maximum depth of the
    /// Python interpreter stack.  `limit` must be a positive integer (>= 1).
    /// <https://docs.python.org/3/library/sys.html#sys.setrecursionlimit>
    fn setrecursionlimit(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "sys.setrecursionlimit() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        }
        let n = match args[0].value.kind() {
            ValueKind::Int(i) => i,
            ValueKind::Bool(b) => b as i64,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object cannot be interpreted as an integer",
                        value_type_name_str(&args[0].value)
                    ),
                ));
            }
        };
        if n < 1 {
            return Err(PyError::named(
                "ValueError",
                "recursion limit must be greater or equal than 1".to_string(),
            ));
        }
        let new_limit = n as usize;
        let current_depth = get_call_depth();
        if new_limit <= current_depth {
            return Err(PyError::named(
                "RecursionError",
                format!(
                    "cannot set the recursion limit to {} at the recursion depth {}: the limit is too low",
                    new_limit, current_depth
                ),
            ));
        }
        set_recursion_limit(new_limit);
        Ok(Value::none())
    }

    /// CPython: sys.exc_info() — returns the (type, value, traceback) tuple
    /// for the exception currently being handled, or (None, None, None) when
    /// called outside any active exception handler.
    /// <https://docs.python.org/3/library/sys.html#sys.exc_info>
    fn exc_info(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "sys.exc_info() takes no arguments ({} given)",
                    args.len()
                ),
            ));
        }
        match _interp.active_exception.clone() {
            None => Ok(Value::tuple(vec![
                Value::none(),
                Value::none(),
                Value::none(),
            ])),
            Some(exc_val) => {
                let exc_type = match exc_val.kind() {
                    ValueKind::PyInstance(inst) => {
                        Value::py_class(Rc::clone(&inst.borrow().class))
                    }
                    _ => Value::none(),
                };
                Ok(Value::tuple(vec![exc_type, exc_val, Value::none()]))
            }
        }
    }

    /// CPython: sys.exception() — returns the exception instance currently
    /// being handled, or None when called outside any active exception handler.
    /// Added in Python 3.11.
    /// <https://docs.python.org/3/library/sys.html#sys.exception>
    fn exception(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "sys.exception() takes no arguments ({} given)",
                    args.len()
                ),
            ));
        }
        Ok(_interp.active_exception.clone().unwrap_or_else(Value::none))
    }

    // ── version_info rich-comparison and sequence methods ────────────────────
    //
    // These are registered as `"sys.version_info_*"` builtins; they are
    // stored in `VERSION_INFO_CLASS.attrs` under the standard dunder names
    // so that `try_dunder_binary` resolves them when comparing a
    // version_info instance against a tuple.  `args[0]` is always `self`
    // (the version_info instance); `args[1]` is the right-hand side.
    //
    // Returning `Value::not_implemented()` for non-tuple rhs lets the
    // interpreter's reflected-dunder machinery raise a proper TypeError.

    /// `sys.version_info >= other`
    #[py_name = "version_info_ge"]
    fn version_info_ge(args) -> Result<Value> {
        match vi_cmp_args(args)? {
            None => Ok(Value::not_implemented()),
            Some((lhs, rhs)) => Ok(Value::bool_(
                vi_cmp_order(&lhs, &rhs)? != std::cmp::Ordering::Less,
            )),
        }
    }

    /// `sys.version_info > other`
    #[py_name = "version_info_gt"]
    fn version_info_gt(args) -> Result<Value> {
        match vi_cmp_args(args)? {
            None => Ok(Value::not_implemented()),
            Some((lhs, rhs)) => Ok(Value::bool_(
                vi_cmp_order(&lhs, &rhs)? == std::cmp::Ordering::Greater,
            )),
        }
    }

    /// `sys.version_info <= other`
    #[py_name = "version_info_le"]
    fn version_info_le(args) -> Result<Value> {
        match vi_cmp_args(args)? {
            None => Ok(Value::not_implemented()),
            Some((lhs, rhs)) => Ok(Value::bool_(
                vi_cmp_order(&lhs, &rhs)? != std::cmp::Ordering::Greater,
            )),
        }
    }

    /// `sys.version_info < other`
    #[py_name = "version_info_lt"]
    fn version_info_lt(args) -> Result<Value> {
        match vi_cmp_args(args)? {
            None => Ok(Value::not_implemented()),
            Some((lhs, rhs)) => Ok(Value::bool_(
                vi_cmp_order(&lhs, &rhs)? == std::cmp::Ordering::Less,
            )),
        }
    }

    /// `sys.version_info == other`
    #[py_name = "version_info_eq"]
    fn version_info_eq(args) -> Result<Value> {
        match vi_cmp_args(args)? {
            None => Ok(Value::not_implemented()),
            Some((lhs, rhs)) => Ok(Value::bool_(
                vi_cmp_order(&lhs, &rhs)? == std::cmp::Ordering::Equal,
            )),
        }
    }

    /// `sys.version_info != other`
    #[py_name = "version_info_ne"]
    fn version_info_ne(args) -> Result<Value> {
        match vi_cmp_args(args)? {
            None => Ok(Value::not_implemented()),
            Some((lhs, rhs)) => Ok(Value::bool_(
                vi_cmp_order(&lhs, &rhs)? != std::cmp::Ordering::Equal,
            )),
        }
    }

    /// `repr(sys.version_info)` — named-tuple style representation.
    #[py_name = "version_info_repr"]
    fn version_info_repr(args) -> Result<Value> {
        if args.is_empty() {
            return Err(PyError::Runtime(
                "version_info_repr() missing self".to_string(),
            ));
        }
        let fields = vi_as_fields(&args[0].value)?;
        Ok(Value::string(format!(
            "sys.version_info(major={}, minor={}, micro={}, releaselevel='{}', serial={})",
            fields.major, fields.minor, fields.micro, fields.releaselevel, fields.serial,
        )))
    }

    /// `sys.version_info[i]` — index access: 0→major, 1→minor, 2→micro,
    /// 3→releaselevel, 4→serial.
    #[py_name = "version_info_getitem"]
    fn version_info_getitem(args) -> Result<Value> {
        if args.len() < 2 {
            return Err(PyError::Runtime(
                "version_info_getitem() missing index".to_string(),
            ));
        }
        let fields = vi_as_fields(&args[0].value)?;
        let as_tuple = fields.as_tuple();
        match args[1].value.kind() {
            ValueKind::Int(i) => {
                let idx = normalise_index(i, as_tuple.len())?;
                Ok(as_tuple[idx].clone())
            }
            _ => Err(PyError::named(
                "TypeError",
                "version_info indices must be integers".to_string(),
            )),
        }
    }

    /// `len(sys.version_info)` — always 5.
    #[py_name = "version_info_len"]
    fn version_info_len(args) -> Result<Value> {
        if args.is_empty() {
            return Err(PyError::Runtime(
                "version_info_len() missing self".to_string(),
            ));
        }
        Ok(Value::int(5))
    }
}

/// Return the platform string matching CPython's `sys.platform` for the
/// current compilation target.
const fn sys_platform() -> &'static str {
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
const fn sys_byteorder() -> &'static str {
    #[cfg(target_endian = "little")]
    {
        "little"
    }
    #[cfg(target_endian = "big")]
    {
        "big"
    }
}

// ── version_info helpers ─────────────────────────────────────────────────────

/// The five fields of `sys.version_info`, extracted from a PyInstance.
struct ViFields {
    major: i64,
    minor: i64,
    micro: i64,
    releaselevel: String,
    serial: i64,
}

impl ViFields {
    /// Convert to the equivalent 5-element Value vector (tuple field order).
    fn as_tuple(&self) -> Vec<Value> {
        vec![
            Value::int(self.major),
            Value::int(self.minor),
            Value::int(self.micro),
            Value::string(self.releaselevel.clone()),
            Value::int(self.serial),
        ]
    }
}

/// Extract the five fields from a `version_info` PyInstance.
fn vi_as_fields(val: &Value) -> Result<ViFields> {
    let inst = match val.kind() {
        ValueKind::PyInstance(i) => Rc::clone(i),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "expected a version_info instance".to_string(),
            ))
        }
    };
    let borrow = inst.borrow();
    let get_int = |name: &str| -> Result<i64> {
        match borrow.attrs.get(name).map(|v| v.kind()) {
            Some(ValueKind::Int(n)) => Ok(n),
            _ => Err(PyError::Runtime(format!(
                "version_info missing integer field '{name}'"
            ))),
        }
    };
    let get_str = |name: &str| -> Result<String> {
        match borrow.attrs.get(name).map(|v| v.kind()) {
            Some(ValueKind::Str(s)) => Ok(s.to_string()),
            _ => Err(PyError::Runtime(format!(
                "version_info missing string field '{name}'"
            ))),
        }
    };
    Ok(ViFields {
        major: get_int("major")?,
        minor: get_int("minor")?,
        micro: get_int("micro")?,
        releaselevel: get_str("releaselevel")?,
        serial: get_int("serial")?,
    })
}

/// Extract `(lhs_as_tuple, rhs_as_tuple)` for a comparison call.
/// `args[0]` is `self` (version_info instance), `args[1]` is the other operand.
/// Returns `Ok(None)` when the right-hand side is neither a tuple nor another
/// version_info instance, which the callers convert to `Value::not_implemented()`.
fn vi_cmp_args(args: &[ExpandedCallArg]) -> Result<Option<(Vec<Value>, Vec<Value>)>> {
    if args.len() < 2 {
        return Err(PyError::Runtime(
            "version_info comparison missing argument".to_string(),
        ));
    }
    let fields = vi_as_fields(&args[0].value)?;
    let lhs = fields.as_tuple();
    let rhs = match args[1].value.kind() {
        ValueKind::Tuple(items) => items.to_vec(),
        ValueKind::PyInstance(_) => {
            // Allow version_info op version_info comparisons — CPython's
            // named-tuple semantics permit comparing two version_info objects.
            match vi_as_fields(&args[1].value) {
                Ok(rhs_fields) => rhs_fields.as_tuple(),
                Err(_) => return Ok(None),
            }
        }
        _ => return Ok(None),
    };
    Ok(Some((lhs, rhs)))
}

/// Lexicographic comparison of two value slices (like tuple comparison).
fn vi_cmp_order(lhs: &[Value], rhs: &[Value]) -> Result<std::cmp::Ordering> {
    for (a, b) in lhs.iter().zip(rhs.iter()) {
        let ord = match (a.kind(), b.kind()) {
            (ValueKind::Int(x), ValueKind::Int(y)) => x.cmp(&y),
            (ValueKind::Str(x), ValueKind::Str(y)) => x.cmp(y),
            (ValueKind::Bool(x), ValueKind::Int(y)) => (x as i64).cmp(&y),
            (ValueKind::Int(x), ValueKind::Bool(y)) => x.cmp(&(y as i64)),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "'<' not supported between version_info fields of type '{}' and '{}'",
                        value_type_name_str(a),
                        value_type_name_str(b),
                    ),
                ))
            }
        };
        if ord != std::cmp::Ordering::Equal {
            return Ok(ord);
        }
    }
    Ok(lhs.len().cmp(&rhs.len()))
}

/// Normalise a possibly-negative integer index into `[0, len)`.
fn normalise_index(i: i64, len: usize) -> Result<usize> {
    let len_i = len as i64;
    let idx = if i < 0 { i + len_i } else { i };
    if idx < 0 || idx >= len_i {
        return Err(PyError::named(
            "IndexError",
            format!("version_info index {i} out of range"),
        ));
    }
    Ok(idx as usize)
}

// ── flags class singleton ────────────────────────────────────────────────────
//
// CPython exposes `sys.flags` as a named-tuple-like object with a `flags`
// class.  We build a minimal PyInstance whose fields all hold their default
// values (all-zeros for the integer fields, False for the bool fields).  The
// class carries only a `__repr__` so that `repr(sys.flags)` produces a
// recognisable string.

thread_local! {
    static FLAGS_CLASS: Rc<RefCell<PyClass>> = {
        Rc::new(RefCell::new(PyClass {
            name: "flags".to_string(),
            qualname: "flags".to_string(),
            base: None,
            extra_bases: vec![],
            attrs: indexmap::IndexMap::new(),
            mutation_version: std::cell::Cell::new(0),
            subclasses: std::cell::RefCell::new(vec![]),
            metatype: None,
            slots: None,
        }))
    };
}

/// Build the `sys.flags` singleton value.
fn make_flags() -> Value {
    FLAGS_CLASS.with(|class| {
        let mut attrs: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
        // Fields and their defaults match CPython's sys.flags for a normal run
        // with no command-line options (all optimization / debug flags are 0).
        // <https://docs.python.org/3/library/sys.html#sys.flags>
        attrs.insert("debug".to_string(), Value::int(0));
        attrs.insert("inspect".to_string(), Value::int(0));
        attrs.insert("interactive".to_string(), Value::int(0));
        attrs.insert("optimize".to_string(), Value::int(0));
        attrs.insert("dont_write_bytecode".to_string(), Value::int(0));
        attrs.insert("no_user_site".to_string(), Value::int(0));
        attrs.insert("no_site".to_string(), Value::int(0));
        attrs.insert("ignore_environment".to_string(), Value::int(0));
        attrs.insert("verbose".to_string(), Value::int(0));
        attrs.insert("bytes_warning".to_string(), Value::int(0));
        attrs.insert("quiet".to_string(), Value::int(0));
        attrs.insert("hash_randomization".to_string(), Value::int(0));
        attrs.insert("isolated".to_string(), Value::int(0));
        attrs.insert("dev_mode".to_string(), Value::bool_(false));
        attrs.insert("utf8_mode".to_string(), Value::int(0));
        attrs.insert("warn_default_encoding".to_string(), Value::int(0));
        attrs.insert("safe_path".to_string(), Value::bool_(false));
        attrs.insert("int_max_str_digits".to_string(), Value::int(4300));
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(class),
            attrs,
        })))
    })
}

// ── version_info class singleton ─────────────────────────────────────────────
//
// The class is built once per thread.  `BuiltinFunction` attrs point to the
// functions registered above in `pyrust_module!` so pyrust's existing
// `try_dunder_binary` / `get_attr` / subscript dispatch picks them up.

thread_local! {
    static VERSION_INFO_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
        // Rich-comparison dunders — registered as `"sys.version_info_*"` in
        // the pyrust_module! block above.
        attrs.insert(
            "__ge__".to_string(),
            Value::builtin_function("sys.version_info_ge"),
        );
        attrs.insert(
            "__gt__".to_string(),
            Value::builtin_function("sys.version_info_gt"),
        );
        attrs.insert(
            "__le__".to_string(),
            Value::builtin_function("sys.version_info_le"),
        );
        attrs.insert(
            "__lt__".to_string(),
            Value::builtin_function("sys.version_info_lt"),
        );
        attrs.insert(
            "__eq__".to_string(),
            Value::builtin_function("sys.version_info_eq"),
        );
        attrs.insert(
            "__ne__".to_string(),
            Value::builtin_function("sys.version_info_ne"),
        );
        attrs.insert(
            "__repr__".to_string(),
            Value::builtin_function("sys.version_info_repr"),
        );
        attrs.insert(
            "__getitem__".to_string(),
            Value::builtin_function("sys.version_info_getitem"),
        );
        attrs.insert(
            "__len__".to_string(),
            Value::builtin_function("sys.version_info_len"),
        );
        Rc::new(RefCell::new(PyClass {
            name: "version_info".to_string(),
            qualname: "version_info".to_string(),
            base: None,
            extra_bases: vec![],
            attrs,
            mutation_version: std::cell::Cell::new(0),
            subclasses: std::cell::RefCell::new(vec![]),
            metatype: None,
            slots: None,
        }))
    };
}

/// Build the `sys.version_info` singleton value.  Called once per
/// `sys.module()` invocation (which the interpreter memoises in the module
/// cache).  Returns a `PyInstance` of `VERSION_INFO_CLASS` with the five
/// standard fields pre-set.
fn make_version_info() -> Value {
    VERSION_INFO_CLASS.with(|class| {
        let mut attrs: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
        attrs.insert("major".to_string(), Value::int(3));
        // pyrust emulates Python 3.12 semantics.
        attrs.insert("minor".to_string(), Value::int(12));
        attrs.insert("micro".to_string(), Value::int(0));
        attrs.insert("releaselevel".to_string(), Value::string("final"));
        attrs.insert("serial".to_string(), Value::int(0));
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(class),
            attrs,
        })))
    })
}
