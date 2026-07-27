//! Native wrappers for `sys.stdin`, `sys.stdout`, and `sys.stderr`.
//!
//! These objects bridge Python stream methods to the process standard handles.
//! Regular path-backed file state and buffering remain owned by the parent
//! `file` module.

use indexmap::IndexMap;
use pyrust_core::{
    BuiltinState, BuiltinTypeOps, PyError, Result, Value, ValueKind, builtin_ops_is,
};

/// Which standard I/O channel this object wraps.
#[derive(Clone, Copy, Debug)]
pub enum StdioKind {
    Stdin,
    Stdout,
    Stderr,
}

/// State carried in the `BuiltinObject` for a stdio stream.
pub struct StdioState {
    pub kind: StdioKind,
}

pub struct StdioOps;

pub const STDIO_OPS: &StdioOps = &StdioOps;
pub const STDIO_TYPE_NAME: &str = "_io.TextIOWrapper";

impl BuiltinTypeOps for StdioOps {
    fn type_name(&self) -> &'static str {
        STDIO_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<StdioState>()
            .expect("StdioOps: bad state");
        match s.kind {
            StdioKind::Stdin => "<_io.TextIOWrapper name='<stdin>' mode='r' encoding='utf-8'>",
            StdioKind::Stdout => "<_io.TextIOWrapper name='<stdout>' mode='w' encoding='utf-8'>",
            StdioKind::Stderr => "<_io.TextIOWrapper name='<stderr>' mode='w' encoding='utf-8'>",
        }
        .to_string()
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn has_method(&self, name: &str) -> bool {
        stdio_has_method(name)
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        call_stdio_method(state, name, &args)
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<StdioState>()
            .expect("StdioOps: bad state");
        match name {
            "name" => Some(Value::string(match s.kind {
                StdioKind::Stdin => "<stdin>",
                StdioKind::Stdout => "<stdout>",
                StdioKind::Stderr => "<stderr>",
            })),
            "mode" => Some(Value::string(match s.kind {
                StdioKind::Stdin => "r",
                StdioKind::Stdout | StdioKind::Stderr => "w",
            })),
            "encoding" => Some(Value::string("utf-8")),
            "closed" => Some(Value::bool_(false)),
            _ => None,
        }
    }
}

/// Create a `sys.stdout` value.
pub fn make_stdout() -> Value {
    Value::builtin_object(
        STDIO_OPS,
        Box::new(StdioState {
            kind: StdioKind::Stdout,
        }),
    )
}

/// Create a `sys.stderr` value.
pub fn make_stderr() -> Value {
    Value::builtin_object(
        STDIO_OPS,
        Box::new(StdioState {
            kind: StdioKind::Stderr,
        }),
    )
}

/// Create a `sys.stdin` value.
pub fn make_stdin() -> Value {
    Value::builtin_object(
        STDIO_OPS,
        Box::new(StdioState {
            kind: StdioKind::Stdin,
        }),
    )
}

/// If `value` is one of the *default* stdio stream wrappers (the objects
/// returned by [`make_stdout`] / [`make_stderr`] / [`make_stdin`]), report which
/// channel it wraps; otherwise `None`.  Used by `print()` to decide whether the
/// current `sys.stdout` is still the native console (fast path) or has been
/// redirected to some other writable object (`contextlib.redirect_stdout`,
/// `io.StringIO`, a user file, …), in which case output must route through that
/// object's `write()` method instead of the native handle.
pub fn default_stdio_kind(value: &Value) -> Option<StdioKind> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    // Match by concrete ops type: `StdioOps` is a zero-sized type, so address
    // identity is not stable across codegen units.
    if !builtin_ops_is::<StdioOps>(ops) {
        return None;
    }
    let borrow = state.borrow();
    borrow.downcast_ref::<StdioState>().map(|s| s.kind)
}

pub fn stdio_has_method(name: &str) -> bool {
    matches!(
        name,
        "write" | "flush" | "fileno" | "read" | "readline" | "readlines"
    )
}

fn call_stdio_method(state: &BuiltinState, method: &str, args: &[Value]) -> Result<Value> {
    let kind = {
        let borrow = state.borrow();
        borrow
            .downcast_ref::<StdioState>()
            .expect("StdioOps: bad state")
            .kind
    };
    match method {
        "write" => {
            let first = args.first();
            let s = first
                .and_then(|v| match v.kind() {
                    ValueKind::Str(s) => Some(s.to_string()),
                    _ => None,
                })
                .ok_or_else(|| {
                    // CPython's C implementation uses tp_name, which for NoneType
                    // is "NoneType" yet the error prints "None".  Mirror that by
                    // mapping NoneType→None in the error message.
                    let type_name = first
                        .map(|v| match v.kind() {
                            ValueKind::None => "None".to_string(),
                            _ => pyrust_core::builtin_type_name(v).into_owned(),
                        })
                        .unwrap_or_else(|| "str".to_string());
                    PyError::named(
                        "TypeError",
                        format!("write() argument must be str, not {type_name}"),
                    )
                })?;
            let n = s.chars().count() as i64;
            match kind {
                StdioKind::Stdout => {
                    use std::io::Write;
                    print!("{s}");
                    let _ = std::io::stdout().flush();
                }
                StdioKind::Stderr => {
                    use std::io::Write;
                    eprint!("{s}");
                    let _ = std::io::stderr().flush();
                }
                StdioKind::Stdin => {
                    return Err(PyError::named(
                        "io.UnsupportedOperation",
                        "not writable".to_string(),
                    ));
                }
            }
            Ok(Value::int(n))
        }
        "flush" => {
            use std::io::Write;
            match kind {
                StdioKind::Stdout => {
                    let _ = std::io::stdout().flush();
                }
                StdioKind::Stderr => {
                    let _ = std::io::stderr().flush();
                }
                StdioKind::Stdin => {}
            }
            Ok(Value::none())
        }
        "fileno" => {
            let fd = match kind {
                StdioKind::Stdin => 0i64,
                StdioKind::Stdout => 1,
                StdioKind::Stderr => 2,
            };
            Ok(Value::int(fd))
        }
        "read" => {
            if !matches!(kind, StdioKind::Stdin) {
                return Err(PyError::named(
                    "io.UnsupportedOperation",
                    "not readable".to_string(),
                ));
            }
            use std::io::Read;
            let size: Option<usize> = args.first().and_then(|v| match v.kind() {
                ValueKind::Int(n) if n >= 0 => Some(n as usize),
                ValueKind::None => None,
                ValueKind::Int(_) => None,
                _ => None,
            });
            let mut buf = String::new();
            match size {
                None => {
                    std::io::stdin().read_to_string(&mut buf).ok();
                }
                Some(n) => {
                    let mut tmp = vec![0u8; n];
                    let got = std::io::stdin().read(&mut tmp).unwrap_or(0);
                    buf = String::from_utf8_lossy(&tmp[..got]).into_owned();
                }
            }
            Ok(Value::string(buf))
        }
        "readline" => {
            if !matches!(kind, StdioKind::Stdin) {
                return Err(PyError::named(
                    "io.UnsupportedOperation",
                    "not readable".to_string(),
                ));
            }
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).ok();
            Ok(Value::string(line))
        }
        "readlines" => {
            if !matches!(kind, StdioKind::Stdin) {
                return Err(PyError::named(
                    "io.UnsupportedOperation",
                    "not readable".to_string(),
                ));
            }
            use std::io::BufRead;
            let lines: Vec<Value> = std::io::stdin()
                .lock()
                .lines()
                .map_while(|l| l.ok())
                .map(|l| Value::string(l + "\n"))
                .collect();
            Ok(Value::list(lines))
        }
        _ => Err(PyError::named(
            "AttributeError",
            format!("'{}' object has no attribute '{method}'", STDIO_TYPE_NAME),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OtherZeroSizedOps;

    impl BuiltinTypeOps for OtherZeroSizedOps {
        fn type_name(&self) -> &'static str {
            "other-zst"
        }
    }

    const OTHER_ZERO_SIZED_OPS: &OtherZeroSizedOps = &OtherZeroSizedOps;

    #[test]
    fn default_streams_retain_their_channel_identity() {
        assert!(matches!(
            default_stdio_kind(&make_stdin()),
            Some(StdioKind::Stdin)
        ));
        assert!(matches!(
            default_stdio_kind(&make_stdout()),
            Some(StdioKind::Stdout)
        ));
        assert!(matches!(
            default_stdio_kind(&make_stderr()),
            Some(StdioKind::Stderr)
        ));
        assert!(default_stdio_kind(&Value::string("redirected")).is_none());
        let unrelated = Value::builtin_object(OTHER_ZERO_SIZED_OPS, Box::new(()));
        assert!(default_stdio_kind(&unrelated).is_none());
    }

    #[test]
    fn stdio_method_surface_is_separate_from_regular_file_methods() {
        for method in ["write", "flush", "fileno", "read", "readline", "readlines"] {
            assert!(stdio_has_method(method), "{method}");
        }
        for method in ["close", "seek", "tell", "writelines"] {
            assert!(!stdio_has_method(method), "{method}");
        }
    }

    #[test]
    fn stdio_dispatch_still_reaches_the_native_wrapper() {
        let stdout = make_stdout();
        let ValueKind::BuiltinObject { ops, state } = stdout.kind() else {
            panic!("stdout must remain a builtin object");
        };

        let fd = ops
            .call_method(state, "fileno", Vec::new(), &IndexMap::new())
            .expect("stdout.fileno()");
        assert!(matches!(fd.kind(), ValueKind::Int(1)));
        let name = ops.getattr(state, "name").expect("stdout.name");
        assert!(matches!(
            name.kind(),
            ValueKind::Str(name) if name == "<stdout>"
        ));
    }
}
