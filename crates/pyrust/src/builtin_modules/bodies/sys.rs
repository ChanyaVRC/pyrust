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
use crate::value::{InstanceAttrs, PyClass, PyDict, PyInstance, Value, ValueKind};
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
        "modules"      => Value::dict(PyDict::default()),
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
        // CPython: sys.maxunicode — the largest Unicode code point,
        // 0x10FFFF (1114111) for the wide build that 3.12 always uses.
        // <https://docs.python.org/3/library/sys.html#sys.maxunicode>
        "maxunicode"   => Value::int(1_114_111),
        // CPython: sys.abiflags — ABI flags string; empty on a normal
        // CPython build.
        // <https://docs.python.org/3/library/sys.html#sys.abiflags>
        "abiflags"     => Value::string(""),
        // CPython: sys.float_info — struct-sequence of float limits
        // (IEEE 754 double). Values are platform-stable.
        // <https://docs.python.org/3/library/sys.html#sys.float_info>
        "float_info"   => make_float_info(),
        // CPython: sys.int_info — struct-sequence describing the int
        // implementation.
        // <https://docs.python.org/3/library/sys.html#sys.int_info>
        "int_info"     => make_int_info(),
        // CPython: sys.implementation — namespace describing the running
        // interpreter (name / version / hexversion / cache_tag).
        // <https://docs.python.org/3/library/sys.html#sys.implementation>
        "implementation" => make_implementation(),
        // CPython: sys.builtin_module_names — tuple of module names
        // compiled into the interpreter.
        // <https://docs.python.org/3/library/sys.html#sys.builtin_module_names>
        "builtin_module_names" => make_builtin_module_names(),
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
        // The limit honors the `__index__` protocol (#2022); a non-int raises
        // the canonical TypeError and an overflowing bigint raises
        // `OverflowError: Python int too large to convert to C int`.
        let n = _interp.value_to_isize(
            &args[0].value,
            "Python int too large to convert to C int",
        )?;
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

    /// CPython: sys.get_int_max_str_digits() — return the current integer
    /// string-conversion length limit (0 = unlimited).
    /// <https://docs.python.org/3/library/sys.html#sys.get_int_max_str_digits>
    fn get_int_max_str_digits(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "sys.get_int_max_str_digits() takes no arguments ({} given)",
                    args.len()
                ),
            ));
        }
        Ok(Value::int(pyrust_core::get_int_max_str_digits() as i64))
    }

    /// CPython: sys.set_int_max_str_digits(maxdigits) — set the integer
    /// string-conversion length limit (gh-95778).  `maxdigits` honors the
    /// `__index__` protocol and must be 0 (unlimited) or >= 640.
    /// <https://docs.python.org/3/library/sys.html#sys.set_int_max_str_digits>
    fn set_int_max_str_digits(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "set_int_max_str_digits() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        }
        let n = _interp.value_to_isize(
            &args[0].value,
            "Python int too large to convert to C int",
        )?;
        if n != 0 && n < pyrust_core::INT_MAX_STR_DIGITS_MIN as i64 {
            return Err(PyError::named(
                "ValueError",
                format!(
                    "maxdigits must be 0 or larger than {}",
                    pyrust_core::INT_MAX_STR_DIGITS_MIN
                ),
            ));
        }
        pyrust_core::set_int_max_str_digits(n as usize);
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
                // Issue #2170: the third element is the exception's traceback
                // object (its `__traceback__`), not `None`.
                let tb = match exc_val.kind() {
                    ValueKind::PyInstance(inst) => inst
                        .borrow()
                        .attrs
                        .get("__traceback__")
                        .cloned()
                        .unwrap_or_else(Value::none),
                    _ => Value::none(),
                };
                Ok(Value::tuple(vec![exc_type, exc_val, tb]))
            }
        }
    }

    /// CPython: sys._getframe([depth]) — return a frame object from the call
    /// stack.  `depth` 0 (the default) is the caller's frame; each increment
    /// moves one frame further out (toward the module).  Raises
    /// `ValueError: call stack is not deep enough` when `depth` exceeds the
    /// stack.  pyrust returns a read-only frame snapshot exposing
    /// `f_code`/`f_lineno`/`f_back`/`f_globals`/`f_locals` (issue #2171).
    /// <https://docs.python.org/3/library/sys.html#sys._getframe>
    fn _getframe(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() > 1 {
            return Err(PyError::named(
                "TypeError",
                format!("_getframe() takes at most 1 argument ({} given)", args.len()),
            ));
        }
        let depth = if args.is_empty() {
            0usize
        } else {
            let n = _interp.value_to_isize(
                &args[0].value,
                "Python int too large to convert to C int",
            )?;
            if n < 0 {
                // CPython treats a negative depth as the innermost frame.
                0usize
            } else {
                n as usize
            }
        };
        // The innermost `vm_frame_views` entry is the frame that called
        // `_getframe` (builtins push no view of their own).
        let frame = _interp.build_frame_object(depth, pyrust_core::get_current_vm_line() as i64);
        if frame.is_none() {
            return Err(PyError::named(
                "ValueError",
                "call stack is not deep enough".to_string(),
            ));
        }
        Ok(frame)
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

    /// CPython: sys.intern(string) — enter `string` in the interned-string
    /// table and return the canonical (interned) object.  pyrust does not
    /// maintain a separate intern table, so we return a str equal to the
    /// input, satisfying the contract `sys.intern(s) == s`.
    /// <https://docs.python.org/3/library/sys.html#sys.intern>
    fn intern(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("intern() takes exactly one argument ({} given)", args.len()),
            ));
        }
        match args[0].value.kind() {
            ValueKind::Str(_) => Ok(args[0].value.clone()),
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "intern() argument 1 must be str, not {}",
                    value_type_name_str(&args[0].value),
                ),
            )),
        }
    }

    /// CPython: sys.getsizeof(object[, default]) — size of `object` in
    /// bytes.  Exact sizes are implementation-specific; pyrust returns a
    /// plausible positive size so callers don't hit AttributeError.  The
    /// value is NOT parity-stable and should not be compared against
    /// CPython.
    /// <https://docs.python.org/3/library/sys.html#sys.getsizeof>
    fn getsizeof(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "getsizeof() takes 1 or 2 arguments ({} given)",
                    args.len()
                ),
            ));
        }
        Ok(Value::int(approximate_sizeof(&args[0].value)))
    }

    /// CPython: sys.getdefaultencoding() — the default string encoding,
    /// always 'utf-8' on Python 3.
    /// <https://docs.python.org/3/library/sys.html#sys.getdefaultencoding>
    fn getdefaultencoding(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "getdefaultencoding() takes no arguments ({} given)",
                    args.len()
                ),
            ));
        }
        Ok(Value::string("utf-8"))
    }

    /// CPython: sys.is_finalizing() — True if the interpreter is shutting
    /// down.  pyrust is never observably finalizing from user code, so
    /// this always returns False.
    /// <https://docs.python.org/3/library/sys.html#sys.is_finalizing>
    fn is_finalizing(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "is_finalizing() takes no arguments ({} given)",
                    args.len()
                ),
            ));
        }
        Ok(Value::bool_(false))
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

// ── float_info / int_info / implementation singletons ────────────────────────
//
// These are CPython "struct sequences" (tuple subclasses with named
// fields).  pyrust models them as plain PyInstances exposing the named
// attributes, which is enough for the common `sys.float_info.max` /
// `sys.int_info.bits_per_digit` access patterns.  The numeric values are
// IEEE-754 / build constants and match CPython 3.12 byte-for-byte.

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

/// Build `sys.float_info` — IEEE-754 double-precision limits.
fn make_float_info() -> Value {
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
fn make_int_info() -> Value {
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

/// Build `sys.implementation`.  Reports `cpython` (pyrust emulates the
/// CPython 3.12 reference) so version-sniffing user code behaves.  The
/// name / cache_tag are NOT parity-stable values to assert against.
fn make_implementation() -> Value {
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

/// Build `sys.builtin_module_names` — a tuple of the names of modules
/// compiled into the interpreter.  pyrust reports the core C-level
/// modules CPython 3.12 also lists; this is informational and not
/// parity-asserted against an exact set.
fn make_builtin_module_names() -> Value {
    let names = ["builtins", "sys", "_thread", "errno", "time", "gc"];
    Value::tuple(names.iter().map(|n| Value::string(*n)).collect())
}

/// Rough in-memory size estimate for `sys.getsizeof`.  CPython sizes are
/// implementation-specific; this returns a plausible positive number so
/// callers don't hit AttributeError, but the value must not be compared
/// against CPython in parity tests.
fn approximate_sizeof(value: &Value) -> i64 {
    match value.kind() {
        ValueKind::Str(s) => 49 + s.len() as i64,
        ValueKind::Bytes(b) => 33 + b.len() as i64,
        ValueKind::List(items) => 56 + (items.len() as i64) * 8,
        ValueKind::Tuple(items) => 40 + (items.len() as i64) * 8,
        ValueKind::Int(_) | ValueKind::Bool(_) => 28,
        ValueKind::Float(_) => 24,
        ValueKind::None => 16,
        _ => 16,
    }
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
        Rc::new(RefCell::new(PyClass::new(
            "flags",
            "flags",
            None,
            indexmap::IndexMap::new(),
        )))
    };
}

/// Build the `sys.flags` singleton value.
fn make_flags() -> Value {
    FLAGS_CLASS.with(|class| {
        let mut attrs = InstanceAttrs::new();
        // Fields and their defaults match CPython's sys.flags for a normal run
        // with no command-line options (all optimization / debug flags are 0).
        // <https://docs.python.org/3/library/sys.html#sys.flags>
        attrs.insert("debug", Value::int(0));
        attrs.insert("inspect", Value::int(0));
        attrs.insert("interactive", Value::int(0));
        attrs.insert("optimize", Value::int(0));
        attrs.insert("dont_write_bytecode", Value::int(0));
        attrs.insert("no_user_site", Value::int(0));
        attrs.insert("no_site", Value::int(0));
        attrs.insert("ignore_environment", Value::int(0));
        attrs.insert("verbose", Value::int(0));
        attrs.insert("bytes_warning", Value::int(0));
        attrs.insert("quiet", Value::int(0));
        attrs.insert("hash_randomization", Value::int(0));
        attrs.insert("isolated", Value::int(0));
        attrs.insert("dev_mode", Value::bool_(false));
        attrs.insert("utf8_mode", Value::int(0));
        attrs.insert("warn_default_encoding", Value::int(0));
        attrs.insert("safe_path", Value::bool_(false));
        attrs.insert("int_max_str_digits", Value::int(4300));
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
        Rc::new(RefCell::new(PyClass::new(
            "version_info",
            "version_info",
            None,
            attrs,
        )))
    };
}

/// Build the `sys.version_info` singleton value.  Called once per
/// `sys.module()` invocation (which the interpreter memoises in the module
/// cache).  Returns a `PyInstance` of `VERSION_INFO_CLASS` with the five
/// standard fields pre-set.
fn make_version_info() -> Value {
    VERSION_INFO_CLASS.with(|class| {
        let mut attrs = InstanceAttrs::new();
        attrs.insert("major", Value::int(3));
        // pyrust emulates Python 3.12 semantics.
        attrs.insert("minor", Value::int(12));
        attrs.insert("micro", Value::int(0));
        attrs.insert("releaselevel", Value::string("final"));
        attrs.insert("serial", Value::int(0));
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(class),
            attrs,
        })))
    })
}
