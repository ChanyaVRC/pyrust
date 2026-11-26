// `sys.version_info` and `sys.flags` class singletons, builders, and
// rich-comparison / sequence method implementations.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, value_type_name_str};
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value, ValueKind};

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
            ));
        }
    };
    let borrow = inst.borrow();
    let get_int = |name: &str| -> Result<i64> {
        match borrow.attrs.get(name).map(|v| v.kind()) {
            Some(ValueKind::Int(n)) => Ok(n),
            _ => Err(PyError::runtime(format!(
                "version_info missing integer field '{name}'"
            ))),
        }
    };
    let get_str = |name: &str| -> Result<String> {
        match borrow.attrs.get(name).map(|v| v.kind()) {
            Some(ValueKind::Str(s)) => Ok(s.to_string()),
            _ => Err(PyError::runtime(format!(
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
        return Err(PyError::runtime(
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
fn vi_cmp_order(lhs: &[Value], rhs: &[Value]) -> Result<Ordering> {
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
                ));
            }
        };
        if ord != Ordering::Equal {
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

pub(super) fn version_info_cmp(
    args: &[ExpandedCallArg],
    predicate: impl FnOnce(Ordering) -> bool,
) -> Result<Value> {
    match vi_cmp_args(args)? {
        None => Ok(Value::not_implemented()),
        Some((lhs, rhs)) => Ok(Value::bool_(predicate(vi_cmp_order(&lhs, &rhs)?))),
    }
}

pub(super) fn version_info_repr(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.is_empty() {
        return Err(PyError::runtime(
            "version_info_repr() missing self".to_string(),
        ));
    }
    let fields = vi_as_fields(&args[0].value)?;
    Ok(Value::string(format!(
        "sys.version_info(major={}, minor={}, micro={}, releaselevel='{}', serial={})",
        fields.major, fields.minor, fields.micro, fields.releaselevel, fields.serial,
    )))
}

pub(super) fn version_info_getitem(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() < 2 {
        return Err(PyError::runtime(
            "version_info_getitem() missing index".to_string(),
        ));
    }
    let fields = vi_as_fields(&args[0].value)?;
    let as_tuple = fields.as_tuple();
    // Slice keys return a plain tuple of the selected fields, matching
    // CPython's structseq (tuple subtype) behaviour.
    if let Some((start, stop, step)) = pyrust_builtins::slice::slice_fields(&args[1].value) {
        let (lo, hi, st) =
            pyrust_builtins::slice::compute_indices(as_tuple.len() as i64, &start, &stop, &step)?;
        let mut out = Vec::new();
        let mut i = lo;
        if st > 0 {
            while i < hi {
                out.push(as_tuple[i as usize].clone());
                i += st;
            }
        } else {
            while i > hi {
                out.push(as_tuple[i as usize].clone());
                i += st;
            }
        }
        return Ok(Value::tuple(out));
    }
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

pub(super) fn version_info_len(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.is_empty() {
        return Err(PyError::runtime(
            "version_info_len() missing self".to_string(),
        ));
    }
    Ok(Value::int(5))
}

pyrust_struct_seq! {
    pub(crate) flags in "sys" {
        fields: [
            debug,
            inspect,
            interactive,
            optimize,
            dont_write_bytecode,
            no_user_site,
            no_site,
            ignore_environment,
            verbose,
            bytes_warning,
            quiet,
            hash_randomization,
            isolated,
            dev_mode,
            utf8_mode,
            warn_default_encoding,
            safe_path,
            int_max_str_digits,
        ],
    }
}

// Thread-local justification:
// (a) Why `thread_local!`: `PyClass` and its attribute `Value`s are
//     `!Send + !Sync`, so per-thread singletons for `sys.version_info`
//     cannot live in a process-global `OnceLock`.
// (b) Multi-interpreter / multi-threaded impact: Multiple interpreters on one
//     thread share the `sys.version_info` class object (each module gets fresh
//     `PyInstance` objects); separate threads build independent class objects.
pyrust_core::gil_local! {
    static VERSION_INFO_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
        attrs.insert("__module__".to_string(), Value::string("sys"));
        // Rich-comparison dunders — registered as `"sys.version_info.__*__"` in
        // the `pyrust_module!` facade.
        for (dunder, reg_name) in [
            ("__ge__", "sys.version_info.__ge__"),
            ("__gt__", "sys.version_info.__gt__"),
            ("__le__", "sys.version_info.__le__"),
            ("__lt__", "sys.version_info.__lt__"),
            ("__eq__", "sys.version_info.__eq__"),
            ("__ne__", "sys.version_info.__ne__"),
            ("__repr__", "sys.version_info.__repr__"),
            ("__getitem__", "sys.version_info.__getitem__"),
            ("__len__", "sys.version_info.__len__"),
        ] {
            attrs.insert(dunder.to_string(), Value::builtin_function(reg_name));
        }
        Rc::new(RefCell::new(PyClass::new(
            "version_info",
            "version_info",
            None,
            attrs,
        )))
    };
}

/// The 18 `sys.flags` fields of `cfg`, in struct-sequence order.
///
/// Every field is an immediate (`int` / `bool`), so capturing them detaches
/// the eventual struct-sequence from `RuntimeConfig` without allocating.
fn flag_fields(cfg: &crate::interpreter::RuntimeConfig) -> [Value; 18] {
    [
        Value::int(cfg.debug),
        Value::int(cfg.inspect),
        Value::int(cfg.interactive),
        Value::int(cfg.optimize),
        Value::int(cfg.dont_write_bytecode),
        Value::int(cfg.no_user_site),
        Value::int(cfg.no_site),
        Value::int(cfg.ignore_environment),
        Value::int(cfg.verbose),
        Value::int(cfg.bytes_warning),
        Value::int(cfg.quiet),
        Value::int(cfg.hash_randomization),
        Value::int(cfg.isolated),
        Value::bool_(cfg.dev_mode),
        Value::int(cfg.utf8_mode),
        Value::int(cfg.warn_default_encoding),
        Value::bool_(cfg.safe_path),
        Value::int(cfg.int_max_str_digits),
    ]
}

/// Build the `sys.flags` structseq value populated from `cfg` (matching
/// CPython 3.12's 18-field `sys.flags` tuple subtype).
pub(crate) fn make_flags_from_config(cfg: &crate::interpreter::RuntimeConfig) -> Value {
    flags::make(flag_fields(cfg))
}

/// A builder for `cfg`'s `sys.flags` that allocates nothing until it runs.
///
/// `sys.flags` is a `tuple` subclass instance, so building it forces the
/// primitive class objects into existence; an interpreter that never reads
/// `sys.flags` should not pay for that at startup (issue #3293).
pub(crate) fn deferred_flags(
    cfg: &crate::interpreter::RuntimeConfig,
) -> crate::value::LazyModuleAttrInit {
    let fields = flag_fields(cfg);
    Box::new(move || flags::make(fields))
}

/// Build the default `sys.flags` singleton value (no CLI options).
pub(super) fn make_flags() -> Value {
    make_flags_from_config(&crate::interpreter::RuntimeConfig::default())
}

/// Build the `sys.version_info` singleton value.  Called once per
/// `sys.module()` invocation (which the interpreter memoises in the module
/// cache).  Returns a `PyInstance` of `VERSION_INFO_CLASS` with the five
/// standard fields pre-set.
pub(super) fn make_version_info() -> Value {
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
