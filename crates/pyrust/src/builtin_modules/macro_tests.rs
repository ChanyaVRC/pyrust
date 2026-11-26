//! End-to-end runtime tests for expanded `pyrust_module!`, `pyrust_builtin_modules!`,
//! `pyrust_struct_seq!`, `pyrust_int_enum!`, `pyrust_int_flag!`, `#[native_state(T)]`,
//! variadic `*args`/`**kwargs`, and protocol-driven `FromValue` wrappers (#3268).

use std::collections::HashMap;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    PyBool, PyBytesLike, PyCallable, PyFloat, PyIndex, PyInt, PyNumber, PyPath, PyStr, PyValue,
    PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyKey, Value};

// ─── 1. Struct-sequence and enum fixtures ────────────────────────────────────

pyrust_struct_seq! {
    pub(crate) demo_point in "_macro_test" {
        fields: [x, y, z],
    }
}

pyrust_struct_seq! {
    pub(crate) demo_extended in "_macro_test" {
        fields: [a, b],
        extra_fields: [extra_note],
        unnamed_fields: 1,
    }
}

pyrust_int_enum! {
    pub(crate) DemoMode in "_macro_test" {
        NONE = 0,
        OPTIONAL = 1,
        REQUIRED = 2,
        #[cfg(not(any(unix, windows)))]
        NEVER_PRESENT = 99,
    }
}

pyrust_int_flag! {
    pub(crate) DemoFlags in "_macro_test" {
        READ = 1,
        WRITE = 2,
        EXEC = 4,
        #[cfg(any(unix, windows))]
        ALL = 7,
    }
}

// ─── 2. Native-state Rust payload ────────────────────────────────────────────

#[derive(Debug, Default)]
struct CounterState {
    count: i64,
    label: String,
}

// ─── 3. Full-featured `pyrust_module!` test fixture ──────────────────────────

mod fixture {
    use super::*;

    pyrust_module_identity!("_macro_test");

    pyrust_module! {
        constants {
            "SHARED" => Value::int(42),
            #[cfg(any(unix, windows))]
            "ENABLED_CONST" => Value::string("present"),
            #[cfg(not(any(unix, windows)))]
            "DISABLED_CONST" => Value::string("absent"),
            "demo_point" => demo_point::class(),
            "demo_extended" => demo_extended::class(),
        }

        /// Top-level custom error inheriting from OSError.
        exception DemoOSError(OSError);

        /// Default base (Exception).
        exception DemoSimpleError;

        /// Chained inheritance from an earlier exception in the same module.
        exception DemoSubError(DemoOSError);

        /// Multiple-inheritance exception.
        exception DemoMultiError(DemoOSError, ValueError);

        #[cfg(not(any(unix, windows)))]
        exception DisabledError;

        #[cfg(any(unix, windows))]
        fn platform_tag() -> Result<Value> {
            Ok(Value::string("active"))
        }

        #[cfg(not(any(unix, windows)))]
        fn platform_tag() -> Result<Value> {
            Ok(Value::string("inactive"))
        }

        #[cfg(not(any(unix, windows)))]
        fn disabled_fn() -> Result<Value> {
            Ok(Value::none())
        }

        // CamelCase direct name and #[py_name = "..."] override.
        fn CamelDirect(x: PyInt) -> Result<Value> {
            Ok(Value::int(x.expect_i64(FN_NAME, "x")? + 1))
        }

        #[py_name = "getCamelValue"]
        fn get_camel_value(x: PyInt) -> Result<Value> {
            Ok(Value::int(x.expect_i64(FN_NAME, "x")? * 2))
        }

        // Variadic *args + keyword-only + **kwargs (IndexMap preserves order).
        fn collect_all(
            first: PyStr<'_>,
            #[varargs] rest: &[Value],
            #[default(false.into())] flag: PyBool,
            #[varkwargs] kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            let mut kw_pairs = Vec::with_capacity(kw.len());
            for (k, v) in kw {
                kw_pairs.push(Value::tuple(vec![Value::string(k), v]));
            }
            Ok(Value::tuple(vec![
                Value::string(&*first),
                Value::tuple(rest.to_vec()),
                Value::bool_(*flag),
                Value::tuple(kw_pairs),
            ]))
        }

        // Variadic with HashMap & Vec<Value>.
        fn sum_and_count_kw(
            #[varargs] nums: Vec<Value>,
            #[varkwargs] kw: HashMap<String, Value>,
        ) -> Result<Value> {
            let mut total = 0i64;
            for v in &nums {
                if let Some(n) = v.as_int() {
                    total += n;
                }
            }
            Ok(Value::tuple(vec![Value::int(total), Value::int(kw.len() as i64)]))
        }

        // Protocol-driven FromValue wrappers.
        fn take_bytes_like(buf: PyBytesLike<'_>) -> Result<Value> {
            Ok(Value::bytes(buf.as_bytes().to_vec()))
        }

        fn take_path(p: PyPath) -> Result<Value> {
            Ok(Value::tuple(vec![
                Value::string(p.as_path().to_string_lossy()),
                Value::bool_(p.is_bytes()),
            ]))
        }

        fn take_index(idx: PyIndex) -> Result<Value> {
            Ok(Value::int(idx.expect_i64(FN_NAME, "idx")?))
        }

        fn take_number(num: PyNumber) -> Result<Value> {
            Ok(Value::float(num.as_f64()))
        }

        fn call_with_arg(cb: PyCallable, arg: PyValue) -> Result<Value> {
            _interp.call_function_expanded(
                cb.into_value(),
                &[ExpandedCallArg {
                    name: None,
                    value: arg.0,
                }],
            )
        }

        fn make_point(x: PyInt, y: PyInt, z: PyInt) -> Result<Value> {
            Ok(demo_point::make([
                Value::int(x.expect_i64(FN_NAME, "x")?),
                Value::int(y.expect_i64(FN_NAME, "y")?),
                Value::int(z.expect_i64(FN_NAME, "z")?),
            ]))
        }

        fn make_extended(a: PyInt, b: PyInt, note: PyStr<'_>) -> Result<Value> {
            Ok(demo_extended::make_with_extra(
                vec![
                    Value::int(a.expect_i64(FN_NAME, "a")?),
                    Value::int(b.expect_i64(FN_NAME, "b")?),
                ],
                vec![Value::string(&*note)],
            ))
        }

        // Base class with constants, getters/setters, classmethod, staticmethod,
        // overloaded methods, and #[cfg] members.
        class BaseWidget {
            constants {
                "KIND" => Value::string("widget"),
                #[cfg(any(unix, windows))]
                "ENABLED_ATTR" => Value::int(7),
                #[cfg(not(any(unix, windows)))]
                "DISABLED_ATTR" => Value::int(0),
            }

            fn __init__(#[positional_only] self_val: PyValueRef<'_>, value: PyInt) -> Result<Value> {
                let crate::value::ValueKind::PyInstance(inst) = self_val.0.kind() else {
                    return Err(PyError::named("TypeError", "expected instance"));
                };
                inst.borrow_mut()
                    .attrs
                    .insert("_val", Value::int(value.expect_i64(FN_NAME, "value")?));
                Ok(Value::none())
            }

            #[getter]
            fn value(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
                let crate::value::ValueKind::PyInstance(inst) = self_val.0.kind() else {
                    return Err(PyError::named("TypeError", "expected instance"));
                };
                Ok(inst.borrow().attrs.get("_val").cloned().unwrap_or_else(Value::none))
            }

            #[setter("value")]
            fn set_value(
                #[positional_only] self_val: PyValueRef<'_>,
                new_val: PyInt,
            ) -> Result<Value> {
                let crate::value::ValueKind::PyInstance(inst) = self_val.0.kind() else {
                    return Err(PyError::named("TypeError", "expected instance"));
                };
                inst.borrow_mut()
                    .attrs
                    .insert("_val", Value::int(new_val.expect_i64(FN_NAME, "new_val")?));
                Ok(Value::none())
            }

            #[getter("doubled")]
            fn get_doubled(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
                let crate::value::ValueKind::PyInstance(inst) = self_val.0.kind() else {
                    return Err(PyError::named("TypeError", "expected instance"));
                };
                let n = inst
                    .borrow()
                    .attrs
                    .get("_val")
                    .and_then(Value::as_int)
                    .unwrap_or(0);
                Ok(Value::int(n * 2))
            }

            #[classmethod]
            fn from_double(
                #[positional_only] cls: PyValueRef<'_>,
                n: PyInt,
            ) -> Result<Value> {
                let half = n.expect_i64(FN_NAME, "n")? / 2;
                _interp.call_function_expanded(
                    cls.0.clone(),
                    &[ExpandedCallArg {
                        name: None,
                        value: Value::int(half),
                    }],
                )
            }

            #[staticmethod]
            fn add_two(a: PyInt, b: PyInt) -> Result<Value> {
                Ok(Value::int(
                    a.expect_i64(FN_NAME, "a")? + b.expect_i64(FN_NAME, "b")?,
                ))
            }

            // Overloaded class method!
            fn describe(#[positional_only] _self: PyValueRef<'_>, x: PyInt) -> Result<Value> {
                Ok(Value::string(format!("int:{}", x.expect_i64(FN_NAME, "x")?)))
            }

            fn describe(#[positional_only] _self: PyValueRef<'_>, x: PyFloat) -> Result<Value> {
                Ok(Value::string(format!("float:{:.1}", *x)))
            }

            fn describe(#[positional_only] _self: PyValueRef<'_>, x: PyStr<'_>) -> Result<Value> {
                Ok(Value::string(format!("str:{}", &*x)))
            }
        }

        // Mixin class.
        class TagMixin {
            constants {
                "TAG" => Value::string("tagged"),
            }
            fn get_tag(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
                Ok(Value::string("mixin-tag"))
            }
        }

        // Single inheritance via `: base_expr`.
        class ChildWidget : "BaseWidget" {
            constants {
                "KIND" => Value::string("child"),
            }
        }

        // Multiple inheritance via `(base1, base2)`.
        class TaggedWidget("BaseWidget", "TagMixin") {
            fn combined(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
                let val = _interp.get_attr(self_val.0, "value")?;
                let tag = _interp.get_attr(self_val.0, "TAG")?;
                Ok(Value::tuple(vec![val, tag]))
            }
        }

        // Typed native state class exercising BOTH `set_state`/`with_state`/`with_state_mut`
        // AND typed `#[state]` / `#[state_mut]` receivers.
        #[native_state(CounterState)]
        class NativeCounter {
            fn __init__(
                #[positional_only] self_val: PyValueRef<'_>,
                #[default(0.into())] start: PyInt,
                #[default("default".into())] label: PyStr<'_>,
            ) -> Result<Value> {
                set_state(
                    self_val.0,
                    CounterState {
                        count: start.expect_i64(FN_NAME, "start")?,
                        label: label.to_string(),
                    },
                )?;
                Ok(Value::none())
            }

            #[getter]
            fn count(#[state] st: &CounterState) -> Result<Value> {
                Ok(Value::int(st.count))
            }

            #[setter("count")]
            fn set_count(#[state_mut] st: &mut CounterState, val: PyInt) -> Result<Value> {
                st.count = val.expect_i64(FN_NAME, "val")?;
                Ok(Value::none())
            }

            #[getter]
            fn label(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
                with_state(self_val.0, |st| Ok(Value::string(&st.label)))
            }

            fn increment(
                #[state_mut] st: &mut CounterState,
                #[default(1.into())] delta: PyInt,
            ) -> Result<Value> {
                st.count += delta.expect_i64(FN_NAME, "delta")?;
                Ok(Value::int(st.count))
            }

            fn reset(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
                with_state_mut(self_val.0, |st| {
                    st.count = 0;
                    Ok(Value::none())
                })
            }
        }

        #[cfg(not(any(unix, windows)))]
        class DisabledClass {
            fn nope() -> Result<Value> {
                Ok(Value::none())
            }
        }
    }
}

/// Registry row for the fixture module, so its callables resolve lazily
/// like any other module's.
pub(crate) static MODULE_REGS: &[crate::builtin_registry::ModuleRegs] =
    &[crate::builtin_registry::ModuleRegs {
        name: fixture::MODULE_NAME,
        regs: fixture::REGS,
    }];

/// Build an `Interpreter` and globals dict with `_macro_test` installed in
/// `sys.modules` and bound to `m`, including `DemoMode` (`IntEnum`) and
/// `DemoFlags` (`IntFlag`).
fn run_py(code: &str) {
    let mut interp = Interpreter::default();
    let module = fixture::module();
    DemoMode::install(&mut interp, &module).expect("install DemoMode");
    DemoFlags::install_with_globals(&mut interp, &module).expect("install DemoFlags");

    let sys_mod = interp.load_module("sys").expect("load sys");
    let sys_modules = interp.get_attr(&sys_mod, "modules").expect("sys.modules");
    sys_modules
        .dict_insert(PyKey::str_from("_macro_test"), module.clone())
        .expect("insert _macro_test into sys.modules");

    let globals = Value::dict(crate::value::PyDict::default());
    globals
        .dict_insert(PyKey::str_from("m"), module)
        .expect("insert m into globals");
    interp
        .exec_source(code, Some(globals), None)
        .unwrap_or_else(|e| panic!("Python test failed: {e:?}\n--- source ---\n{code}"));
}

#[test]
fn cfg_gating_in_pyrust_module() {
    run_py(
        r#"
assert m.SHARED == 42
assert m.ENABLED_CONST == "present"
assert not hasattr(m, "DISABLED_CONST")
assert m.platform_tag() == "active"
assert not hasattr(m, "disabled_fn")
assert not hasattr(m, "DisabledError")
assert not hasattr(m, "DisabledClass")
assert m.BaseWidget.ENABLED_ATTR == 7
assert not hasattr(m.BaseWidget, "DISABLED_ATTR")
"#,
    );
}

#[test]
fn cfg_gating_in_pyrust_builtin_modules_macro() {
    // Exercise `pyrust_builtin_modules!` with `#[cfg(...)]` entries directly in
    // an isolated test module.
    mod cfg_mod_table {
        use crate::builtin_registry::BuiltinReg;
        use crate::value::Value;

        #[allow(dead_code)]
        fn publish_exec_dunder_all(
            _: &std::rc::Rc<std::cell::RefCell<crate::value::PyModule>>,
            _: &Value,
        ) -> crate::error::Result<()> {
            Ok(())
        }

        macro_rules! test_mod_def {
            ($(#[cfg($($cfg:tt)*)])* $ident:ident) => {
                $(#[cfg($($cfg)*)])*
                pub mod $ident {
                    pub(crate) fn regs() -> &'static [crate::builtin_registry::BuiltinReg] { &[] }
                    pub(crate) fn module() -> crate::value::Value {
                        crate::value::Value::string(stringify!($ident))
                    }
                }
            };
        }

        macro_rules! test_table {
            ($( $(#[cfg($($cfg:tt)*)])* $ident:ident ),* $(,)?) => {
                $( test_mod_def!($(#[cfg($($cfg)*)])* $ident); )*

                pub(crate) fn is_mod(name: &str) -> bool {
                    $(
                        $(#[cfg($($cfg)*)])*
                        {
                            if name == stringify!($ident) {
                                return true;
                            }
                        }
                    )*
                    false
                }

                pub(crate) fn load_mod(name: &str) -> Option<Value> {
                    $(
                        $(#[cfg($($cfg)*)])*
                        {
                            if name == stringify!($ident) {
                                return Some($ident::module());
                            }
                        }
                    )*
                    None
                }

                pub(crate) fn collect_regs() -> Vec<BuiltinReg> {
                    let mut all = Vec::new();
                    $(
                        $(#[cfg($($cfg)*)])*
                        {
                            all.extend_from_slice($ident::regs());
                        }
                    )*
                    all
                }
            };
        }

        test_table! {
            #[cfg(any(unix, windows))]
            present_mod,
            #[cfg(not(any(unix, windows)))]
            absent_mod,
        }
    }

    assert!(cfg_mod_table::is_mod("present_mod"));
    assert!(!cfg_mod_table::is_mod("absent_mod"));
    assert_eq!(
        cfg_mod_table::load_mod("present_mod").and_then(|v| v.as_str().map(str::to_owned)),
        Some("present_mod".to_string())
    );
    assert!(cfg_mod_table::load_mod("absent_mod").is_none());
    assert!(cfg_mod_table::collect_regs().is_empty());
}

#[test]
fn rich_class_inheritance_getters_setters_classmethod_staticmethod_overloads() {
    run_py(
        r#"
# Automatic __module__
assert m.BaseWidget.__module__ == "_macro_test"
assert m.ChildWidget.__module__ == "_macro_test"
assert m.TaggedWidget.__module__ == "_macro_test"

# Class constants
assert m.BaseWidget.KIND == "widget"
assert m.ChildWidget.KIND == "child"
assert m.TaggedWidget.TAG == "tagged"

# Inheritance (single & multiple)
assert issubclass(m.ChildWidget, m.BaseWidget)
assert issubclass(m.TaggedWidget, m.BaseWidget)
assert issubclass(m.TaggedWidget, m.TagMixin)

w = m.BaseWidget(10)
assert isinstance(w, m.BaseWidget)
assert w.value == 10
assert w.doubled == 20

# Setter
w.value = 25
assert w.value == 25
assert w.doubled == 50

# Read-only property rejects assignment
try:
    w.doubled = 99
    assert False, "expected AttributeError"
except AttributeError:
    pass

# Classmethod on base, instance, and subclass
w2 = m.BaseWidget.from_double(40)
assert type(w2) is m.BaseWidget
assert w2.value == 20
w3 = w.from_double(16)
assert type(w3) is m.BaseWidget
assert w3.value == 8
cw = m.ChildWidget.from_double(30)
assert type(cw) is m.ChildWidget
assert cw.value == 15
assert cw.KIND == "child"

# Staticmethod on class and instance
assert m.BaseWidget.add_two(3, 4) == 7
assert w.add_two(10, 20) == 30
assert m.ChildWidget.add_two(1, 2) == 3

# Multiple inheritance method & property access
tw = m.TaggedWidget(9)
assert tw.value == 9
assert tw.get_tag() == "mixin-tag"
assert tw.combined() == (9, "tagged")

# Type-based method overloads inside class
assert w.describe(5) == "int:5"
assert w.describe(2.5) == "float:2.5"
assert w.describe("hi") == "str:hi"
try:
    w.describe([1, 2])
    assert False, "expected TypeError"
except TypeError:
    pass
"#,
    );
}

#[test]
fn native_exception_declarations() {
    run_py(
        r#"
assert m.DemoOSError.__module__ == "_macro_test"
assert m.DemoSimpleError.__module__ == "_macro_test"
assert m.DemoSubError.__module__ == "_macro_test"
assert m.DemoMultiError.__module__ == "_macro_test"

assert "inheriting from OSError" in m.DemoOSError.__doc__
assert "Default base" in m.DemoSimpleError.__doc__

assert issubclass(m.DemoOSError, OSError)
assert issubclass(m.DemoSimpleError, Exception)
assert issubclass(m.DemoSubError, m.DemoOSError)
assert issubclass(m.DemoSubError, OSError)
assert issubclass(m.DemoMultiError, m.DemoOSError)
assert issubclass(m.DemoMultiError, ValueError)

try:
    raise m.DemoSubError("disk full")
except m.DemoOSError as exc:
    assert isinstance(exc, m.DemoSubError)
    assert isinstance(exc, OSError)
    assert exc.args == ("disk full",)
    assert str(exc) == "disk full"

try:
    raise m.DemoMultiError("bad value")
except ValueError as exc:
    assert isinstance(exc, m.DemoMultiError)
    assert isinstance(exc, m.DemoOSError)
"#,
    );
}

#[test]
fn varargs_varkwargs_and_camel_case_names() {
    run_py(
        r#"
assert m.CamelDirect(9) == 10
assert m.getCamelValue(7) == 14

assert m.collect_all("a") == ("a", (), False, ())
assert m.collect_all("a", 1, 2, 3, flag=True, x=10, y=20) == (
    "a",
    (1, 2, 3),
    True,
    (("x", 10), ("y", 20)),
)
# Positional-only / keyword-only after varargs: passing `flag` positionally goes into *rest
assert m.collect_all("b", True, 9) == ("b", (True, 9), False, ())

assert m.sum_and_count_kw() == (0, 0)
assert m.sum_and_count_kw(1, 2, 3, a=1, b=2) == (6, 2)
"#,
    );
}

#[test]
fn protocol_from_value_wrappers_end_to_end() {
    run_py(
        r#"
import pathlib

# 1. PyBytesLike: bytes, bytearray, memoryview, PEP 688 __buffer__ with release check
assert m.take_bytes_like(b"abc") == b"abc"
assert m.take_bytes_like(bytearray(b"xyz")) == b"xyz"
assert m.take_bytes_like(memoryview(b"hello")[1:4]) == b"ell"

released = []
class CustomBuf:
    def __init__(self, data):
        self._data = data
    def __buffer__(self, flags):
        return memoryview(self._data)
    def __release_buffer__(self, mv):
        released.append(True)
        mv.release()

assert m.take_bytes_like(CustomBuf(b"pep688")) == b"pep688"
assert released == [True], f"expected __release_buffer__ to be called, got {released}"

try:
    m.take_bytes_like("not bytes")
    assert False
except TypeError:
    pass

# 2. PyPath: str, bytes, pathlib.Path, custom os.PathLike
assert m.take_path("/tmp/a.txt") == ("/tmp/a.txt", False)
assert m.take_path(b"/tmp/b.bin") == ("/tmp/b.bin", True)
p = pathlib.Path("foo/bar.py")
res_str, is_b = m.take_path(p)
assert res_str == str(p) and is_b is False

class CustomPath:
    def __fspath__(self):
        return "custom/path"
assert m.take_path(CustomPath()) == ("custom/path", False)

class BadPath:
    def __fspath__(self):
        return 123
try:
    m.take_path(BadPath())
    assert False
except TypeError:
    pass

# 3. PyIndex: int, bool, custom __index__
assert m.take_index(42) == 42
assert m.take_index(True) == 1

class MyIdx:
    def __index__(self):
        return 77
assert m.take_index(MyIdx()) == 77

try:
    m.take_index(3.14)
    assert False
except TypeError:
    pass

# 4. PyNumber: float, int, bool, __float__, __index__ fallback
assert m.take_number(1.5) == 1.5
assert m.take_number(4) == 4.0
assert m.take_number(True) == 1.0

class MyFloat:
    def __float__(self):
        return 6.25
assert m.take_number(MyFloat()) == 6.25
assert m.take_number(MyIdx()) == 77.0

try:
    m.take_number("1.5")
    assert False
except TypeError:
    pass

# 5. PyCallable: lambda, builtin, class, callable instance
assert m.call_with_arg(lambda x: x + 5, 10) == 15
assert m.call_with_arg(len, [1, 2, 3]) == 3

class Adder:
    def __call__(self, x):
        return x * 3
assert m.call_with_arg(Adder(), 4) == 12

try:
    m.call_with_arg(123, 1)
    assert False
except TypeError:
    pass
"#,
    );
}

#[test]
fn native_state_class_helpers_and_typed_receivers() {
    run_py(
        r#"
c = m.NativeCounter(10, "hits")
assert c.count == 10
assert c.label == "hits"
assert c.increment() == 11
assert c.increment(5) == 16
assert c.count == 16
c.count = 100
assert c.count == 100
c.reset()
assert c.count == 0
assert c.label == "hits"

# Native state slot is completely invisible from Python __dict__ / vars()
assert vars(c) == {}
assert not hasattr(c, "__pyrust_native_state__")
c.__pyrust_native_state__ = "corrupted"
assert c.increment(3) == 3

# Uninitialized instance via object.__new__ raises RuntimeError cleanly
raw = object.__new__(m.NativeCounter)
try:
    _ = raw.count
    assert False, "expected RuntimeError on uninitialized native state"
except RuntimeError:
    pass

# Calling descriptor with wrong self type raises TypeError cleanly
try:
    m.NativeCounter.increment(123)
    assert False, "expected TypeError on non-instance receiver"
except TypeError:
    pass
"#,
    );
}

#[test]
fn pyrust_struct_seq_end_to_end() {
    run_py(
        r#"
pt = m.make_point(10, 20, 30)
assert isinstance(pt, m.demo_point)
assert isinstance(pt, tuple)
assert m.demo_point.__module__ == "_macro_test"
assert m.demo_point.n_sequence_fields == 3
assert m.demo_point.n_fields == 3
assert m.demo_point.n_unnamed_fields == 0
assert m.demo_point.__match_args__ == ("x", "y", "z")

# Positional indexing, slicing, unpacking, named field access, equality, repr
assert len(pt) == 3
assert pt[0] == 10 and pt[1] == 20 and pt[-1] == 30
assert pt[1:3] == (20, 30)
a, b, c = pt
assert (a, b, c) == (10, 20, 30)
assert pt == (10, 20, 30)
assert pt.x == 10 and pt.y == 20 and pt.z == 30
assert repr(pt) == "_macro_test.demo_point(x=10, y=20, z=30)"

# Construct from Python via __new__
pt2 = m.demo_point((1, 2, 3))
assert pt2 == (1, 2, 3)
assert pt2.x == 1 and pt2.y == 2 and pt2.z == 3

try:
    m.demo_point((1, 2))
    assert False, "expected TypeError for wrong sequence length"
except TypeError:
    pass

# Extended structseq with extra_fields and unnamed_fields
ext = m.make_extended(5, 6, "hello")
assert m.demo_extended.n_sequence_fields == 2
assert m.demo_extended.n_fields == 3
assert m.demo_extended.n_unnamed_fields == 1
assert m.demo_extended.__match_args__ == ("a",)
assert len(ext) == 2
assert ext[0] == 5 and ext[1] == 6
assert ext.a == 5 and ext.b == 6 and ext.extra_note == "hello"

ext2 = m.demo_extended((7, 8), {"extra_note": "override"})
assert ext2.a == 7 and ext2.b == 8 and ext2.extra_note == "override"
"#,
    );
}

#[test]
fn pyrust_int_enum_and_int_flag_end_to_end() {
    run_py(
        r#"
import enum

assert issubclass(m.DemoMode, enum.IntEnum)
assert m.DemoMode.__module__ == "_macro_test"
assert m.DemoMode.NONE == 0
assert m.DemoMode.OPTIONAL == 1
assert m.DemoMode.REQUIRED == 2
assert not hasattr(m.DemoMode, "NEVER_PRESENT")
assert m.DemoMode(1) is m.DemoMode.OPTIONAL
assert isinstance(m.DemoMode.REQUIRED, int)
assert m.DemoMode.REQUIRED.name == "REQUIRED"
assert m.DemoMode.REQUIRED.value == 2

assert issubclass(m.DemoFlags, enum.IntFlag)
assert m.DemoFlags.__module__ == "_macro_test"
assert m.READ is m.DemoFlags.READ
assert m.WRITE is m.DemoFlags.WRITE
assert m.EXEC is m.DemoFlags.EXEC
assert m.ALL is m.DemoFlags.ALL
rw = m.DemoFlags.READ | m.DemoFlags.WRITE
assert isinstance(rw, m.DemoFlags)
assert int(rw) == 3
assert m.DemoFlags.READ in rw
assert m.DemoFlags.EXEC not in rw
"#,
    );
}
