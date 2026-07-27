//! Construction and presentation of `os` result objects.
//!
//! Host operations pass metadata or scalar fields into this module. Python
//! class identity, instance attributes, and repr formatting stay here.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{BUILTIN_DATA_ATTR, ExpandedCallArg, Interpreter};
use crate::value::{InstanceAttrs, PyClass, PyInstance, PyKey, Value, ValueKind};

use super::arguments::require_self;
use super::{
    STAT_RESULT_NEW_REGISTRY, STAT_RESULT_REPR_REGISTRY, TERMINAL_SIZE_NEW_REGISTRY,
    TERMINAL_SIZE_REPR_REGISTRY,
};

thread_local! {
    static TERMINAL_SIZE_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs = indexmap::IndexMap::new();
        attrs.insert("__module__".to_string(), Value::string("os"));
        attrs.insert(
            "__new__".to_string(),
            Value::builtin_function(TERMINAL_SIZE_NEW_REGISTRY),
        );
        attrs.insert(
            "__repr__".to_string(),
            Value::builtin_function(TERMINAL_SIZE_REPR_REGISTRY),
        );
        attrs.insert("n_fields".to_string(), Value::int(2));
        attrs.insert("n_sequence_fields".to_string(), Value::int(2));
        attrs.insert("n_unnamed_fields".to_string(), Value::int(0));
        attrs.insert(
            "__match_args__".to_string(),
            Value::tuple(vec![Value::string("columns"), Value::string("lines")]),
        );
        let mut class = PyClass::new(
            "terminal_size",
            "terminal_size",
            Some(tuple_class()),
            attrs,
        );
        class.non_subclassable_name = Some("os.terminal_size");
        Rc::new(RefCell::new(class))
    };

    static STAT_RESULT_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs = indexmap::IndexMap::new();
        attrs.insert("__module__".to_string(), Value::string("os"));
        attrs.insert(
            "__new__".to_string(),
            Value::builtin_function(STAT_RESULT_NEW_REGISTRY),
        );
        attrs.insert(
            "__repr__".to_string(),
            Value::builtin_function(STAT_RESULT_REPR_REGISTRY),
        );
        attrs.insert("n_fields".to_string(), Value::int(19));
        attrs.insert("n_sequence_fields".to_string(), Value::int(10));
        attrs.insert("n_unnamed_fields".to_string(), Value::int(3));
        attrs.insert(
            "__match_args__".to_string(),
            Value::tuple(
                [
                    "st_mode", "st_ino", "st_dev", "st_nlink", "st_uid", "st_gid",
                    "st_size",
                ]
                .into_iter()
                .map(Value::string)
                .collect(),
            ),
        );
        let mut class = PyClass::new(
            "stat_result",
            "stat_result",
            Some(tuple_class()),
            attrs,
        );
        class.non_subclassable_name = Some("os.stat_result");
        Rc::new(RefCell::new(class))
    };
}

fn tuple_class() -> Rc<RefCell<PyClass>> {
    crate::interpreter::primitive_class_by_name("tuple")
        .expect("the canonical tuple class must exist before os result classes")
}

pub(super) fn terminal_size_class_value() -> Value {
    TERMINAL_SIZE_CLASS.with(|class| Value::py_class(Rc::clone(class)))
}

pub(super) fn stat_result_class_value() -> Value {
    STAT_RESULT_CLASS.with(|class| Value::py_class(Rc::clone(class)))
}

fn make_result_instance(
    class: Rc<RefCell<PyClass>>,
    sequence: Vec<Value>,
    named_attrs: impl IntoIterator<Item = (&'static str, Value)>,
) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert(BUILTIN_DATA_ATTR, Value::tuple(sequence));
    for (name, value) in named_attrs {
        attrs.insert(name, value);
    }
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

fn struct_sequence_args(
    args: &[ExpandedCallArg],
    type_name: &str,
) -> Result<(Rc<RefCell<PyClass>>, Value, Option<Value>)> {
    let class = match args.first().map(|arg| arg.value.kind()) {
        Some(ValueKind::PyClass(class)) => Rc::clone(class),
        _ => {
            return Err(PyError::Runtime(format!(
                "internal: {type_name}.__new__ first argument must be a class"
            )));
        }
    };
    let supplied = &args[1..];
    let positional: Vec<&ExpandedCallArg> =
        supplied.iter().filter(|arg| arg.name.is_none()).collect();
    if positional.len() > 2 {
        return Err(pyrust_core::type_err!(
            "os.{type_name}() takes a dict as second arg, if any"
        ));
    }

    let mut sequence = positional.first().map(|arg| arg.value.clone());
    let mut overrides = positional.get(1).map(|arg| arg.value.clone());
    let mut invalid_keyword = None;
    for arg in supplied.iter().filter(|arg| arg.name.is_some()) {
        match arg.name.as_deref().unwrap_or_default() {
            "sequence" => {
                if sequence.is_some() {
                    return Err(pyrust_core::type_err!(
                        "argument for structseq() given by name ('sequence') and position (1)"
                    ));
                }
                sequence = Some(arg.value.clone());
            }
            "dict" => {
                if overrides.is_some() {
                    return Err(pyrust_core::type_err!(
                        "argument for structseq() given by name ('dict') and position (2)"
                    ));
                }
                overrides = Some(arg.value.clone());
            }
            other => {
                invalid_keyword = Some(other.to_string());
            }
        }
    }
    let sequence = sequence.ok_or_else(|| {
        pyrust_core::type_err!("structseq() missing required argument 'sequence' (pos 1)")
    })?;
    if let Some(other) = invalid_keyword {
        return Err(pyrust_core::type_err!(
            "'{other}' is an invalid keyword argument for structseq()"
        ));
    }
    if overrides
        .as_ref()
        .is_some_and(|value| !matches!(value.kind(), ValueKind::Dict(_)))
    {
        return Err(pyrust_core::type_err!(
            "os.{type_name}() takes a dict as second arg, if any"
        ));
    }
    Ok((class, sequence, overrides))
}

fn dict_value(overrides: Option<&Value>, name: &str) -> Option<Value> {
    let ValueKind::Dict(dict) = overrides?.kind() else {
        return None;
    };
    dict.get(&PyKey::str_from(name)).cloned()
}

pub(super) fn terminal_size_new(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    _fn_name: &str,
) -> Result<Value> {
    let (class, sequence, _overrides) = struct_sequence_args(args, "terminal_size")?;
    let values = interp.collect_iterable(&sequence)?;
    if values.len() != 2 {
        return Err(pyrust_core::type_err!(
            "os.terminal_size() takes a 2-sequence ({}-sequence given)",
            values.len()
        ));
    }
    Ok(make_result_instance(
        class,
        values.clone(),
        [("columns", values[0].clone()), ("lines", values[1].clone())],
    ))
}

pub(super) fn stat_result_new(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    _fn_name: &str,
) -> Result<Value> {
    let (class, sequence, overrides) = struct_sequence_args(args, "stat_result")?;
    let values = interp.collect_iterable(&sequence)?;
    if values.len() < 10 {
        return Err(pyrust_core::type_err!(
            "os.stat_result() takes an at least 10-sequence ({}-sequence given)",
            values.len()
        ));
    }
    if values.len() > 19 {
        return Err(pyrust_core::type_err!(
            "os.stat_result() takes an at most 19-sequence ({}-sequence given)",
            values.len()
        ));
    }

    let overrides = overrides.as_ref();
    let supplemental = |name: &str, index: usize, fallback: Value| {
        dict_value(overrides, name)
            .or_else(|| values.get(index).cloned())
            .unwrap_or(fallback)
    };
    let atime = supplemental("st_atime", 10, values[7].clone());
    let mtime = supplemental("st_mtime", 11, values[8].clone());
    let ctime = supplemental("st_ctime", 12, values[9].clone());
    let optional = |name: &str, index: usize| {
        dict_value(overrides, name)
            .or_else(|| values.get(index).cloned())
            .unwrap_or_else(Value::none)
    };
    let sequence_values = values[..10].to_vec();
    Ok(make_result_instance(
        class,
        sequence_values,
        [
            ("st_mode", values[0].clone()),
            ("st_ino", values[1].clone()),
            ("st_dev", values[2].clone()),
            ("st_nlink", values[3].clone()),
            ("st_uid", values[4].clone()),
            ("st_gid", values[5].clone()),
            ("st_size", values[6].clone()),
            ("st_atime", atime),
            ("st_mtime", mtime),
            ("st_ctime", ctime),
            ("st_atime_ns", optional("st_atime_ns", 13)),
            ("st_mtime_ns", optional("st_mtime_ns", 14)),
            ("st_ctime_ns", optional("st_ctime_ns", 15)),
            ("st_blksize", optional("st_blksize", 16)),
            ("st_blocks", optional("st_blocks", 17)),
            ("st_rdev", optional("st_rdev", 18)),
        ],
    ))
}

pub(super) fn terminal_size_repr(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_self(args, fn_name)?;
    let instance = match args[0].value.kind() {
        ValueKind::PyInstance(instance) => Rc::clone(instance),
        _ => {
            return Err(PyError::Runtime(
                "terminal_size_repr() expected a terminal_size instance".to_string(),
            ));
        }
    };
    let instance = instance.borrow();
    let attr_repr = |name: &str| match instance.attrs.get(name) {
        Some(value) => value.repr_raw(),
        None => "None".to_string(),
    };
    Ok(Value::string(format!(
        "os.terminal_size(columns={}, lines={})",
        attr_repr("columns"),
        attr_repr("lines"),
    )))
}

pub(super) fn stat_result_repr(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_self(args, fn_name)?;
    let instance = match args[0].value.kind() {
        ValueKind::PyInstance(instance) => Rc::clone(instance),
        _ => {
            return Err(PyError::Runtime(
                "stat_result_repr() expected a stat_result instance".to_string(),
            ));
        }
    };
    let instance = instance.borrow();
    // A struct-sequence's repr shows its ten tuple-visible fields.  The
    // optional constructor dictionary may override supplemental named attrs
    // such as `st_atime`, but it does not rewrite tuple slots or their repr.
    let sequence = instance
        .attrs
        .get(BUILTIN_DATA_ATTR)
        .and_then(|value| match value.kind() {
            ValueKind::Tuple(values) => Some(values),
            _ => None,
        });
    let repr = |name: &str, index: usize| {
        sequence
            .and_then(|values| values.get(index))
            .or_else(|| instance.attrs.get(name))
            .map_or_else(|| "None".to_string(), Value::repr_raw)
    };
    Ok(Value::string(format!(
        "os.stat_result(st_mode={}, st_ino={}, st_dev={}, st_nlink={}, \
         st_uid={}, st_gid={}, st_size={}, st_atime={}, st_mtime={}, st_ctime={})",
        repr("st_mode", 0),
        repr("st_ino", 1),
        repr("st_dev", 2),
        repr("st_nlink", 3),
        repr("st_uid", 4),
        repr("st_gid", 5),
        repr("st_size", 6),
        repr("st_atime", 7),
        repr("st_mtime", 8),
        repr("st_ctime", 9),
    )))
}

pub(super) fn make_terminal_size(columns: i64, lines: i64) -> Value {
    TERMINAL_SIZE_CLASS.with(|class| {
        let columns = Value::int(columns);
        let lines = Value::int(lines);
        make_result_instance(
            Rc::clone(class),
            vec![columns.clone(), lines.clone()],
            [("columns", columns), ("lines", lines)],
        )
    })
}

pub(super) fn make_stat_result(metadata: &std::fs::Metadata) -> Value {
    fn seconds_since_epoch(result: std::io::Result<std::time::SystemTime>) -> f64 {
        result
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs_f64())
            .unwrap_or(0.0)
    }

    let modified = seconds_since_epoch(metadata.modified());
    let accessed = seconds_since_epoch(metadata.accessed());
    let created = seconds_since_epoch(metadata.created());

    #[cfg(unix)]
    let (mode, inode, links, user, group, device) = {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.mode() as i64,
            metadata.ino() as i64,
            metadata.nlink() as i64,
            metadata.uid() as i64,
            metadata.gid() as i64,
            metadata.dev() as i64,
        )
    };
    #[cfg(not(unix))]
    let (mode, inode, links, user, group, device) = (
        if metadata.is_dir() {
            0o040000_i64
        } else {
            0o100000_i64
        },
        0_i64,
        0_i64,
        0_i64,
        0_i64,
        0_i64,
    );

    STAT_RESULT_CLASS.with(|class| {
        let visible = vec![
            Value::int(mode),
            Value::int(inode),
            Value::int(device),
            Value::int(links),
            Value::int(user),
            Value::int(group),
            Value::int(metadata.len() as i64),
            Value::float(accessed),
            Value::float(modified),
            Value::float(created),
        ];
        make_result_instance(
            Rc::clone(class),
            visible.clone(),
            [
                ("st_mode", visible[0].clone()),
                ("st_ino", visible[1].clone()),
                ("st_dev", visible[2].clone()),
                ("st_nlink", visible[3].clone()),
                ("st_uid", visible[4].clone()),
                ("st_gid", visible[5].clone()),
                ("st_size", visible[6].clone()),
                ("st_atime", visible[7].clone()),
                ("st_mtime", visible[8].clone()),
                ("st_ctime", visible[9].clone()),
                (
                    "st_atime_ns",
                    Value::int((accessed * 1_000_000_000.0) as i64),
                ),
                (
                    "st_mtime_ns",
                    Value::int((modified * 1_000_000_000.0) as i64),
                ),
                (
                    "st_ctime_ns",
                    Value::int((created * 1_000_000_000.0) as i64),
                ),
                ("st_blksize", Value::none()),
                ("st_blocks", Value::none()),
                ("st_rdev", Value::none()),
            ],
        )
    })
}
