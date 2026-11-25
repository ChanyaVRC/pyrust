# Native Rust Stdlib Module Macros Guide

This document is the quick-reference for writing native-Rust Python 3.12 stdlib
modules in `crates/pyrust/src/builtin_modules/bodies/*.rs` without `*_py.py`
scripts or manual class/finalization boilerplate.

---

## 1. `#[cfg(...)]` Platform Gating

### Module-level gating (`pyrust_builtin_modules!` in `builtin_modules/mod.rs`)

Gate platform-specific modules directly on their entry line. On non-matching
targets the module is excluded from compilation, `all_regs()`,
`is_builtin_module()`, and `load_builtin_module()` (raising
`ModuleNotFoundError` at import time):

```rust
pyrust_builtin_modules! {
    #[cfg(unix)]
    fcntl,
    #[cfg(unix)]
    pwd,
    #[cfg(windows)]
    winreg,
}
```

### Item-level gating inside `pyrust_module!`

`#[cfg(...)]` is supported on `constants` entries, `fn` items (including
mutually-exclusive platform implementations of the same Python name!), `class`
blocks, class `constants`, methods, `#[getter]`/`#[setter]`, `#[classmethod]`,
`#[staticmethod]`, and `exception` declarations:

```rust
pyrust_module! {
    constants {
        #[cfg(unix)]
        "O_NONBLOCK" => Value::int(libc::O_NONBLOCK as i64),
        #[cfg(windows)]
        "O_BINARY" => Value::int(0x8000),
    }

    #[cfg(unix)]
    fn getuid() -> Result<Value> { /* ... */ }

    #[cfg(unix)]
    fn platform_impl() -> Result<Value> { Ok(Value::string("unix")) }

    #[cfg(windows)]
    fn platform_impl() -> Result<Value> { Ok(Value::string("windows")) }

    #[cfg(unix)]
    exception PosixOnlyError(OSError);

    #[cfg(unix)]
    class PosixHandle {
        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }
    }
}
```

---

## 2. Rich `class { ... }` Syntax inside `pyrust_module!`

Every generated class automatically sets `__module__ = MODULE_NAME`.

- **Inheritance** (single or multiple bases; each base expression may be a
  `&str` naming an earlier class/exception in the module or a builtin/exception
  class like `"dict"` / `"OSError"`, a `Value`, or an `Rc<RefCell<PyClass>>`):
  - `class Sub : "Base" { ... }`
  - `class Sub : [base1, base2] { ... }`
  - `class Sub("Base", "Mixin") { ... }`
- **Class constants**: `constants { #[cfg(...)] "NAME" => expr, ... }`
- **Properties**: `#[getter]`, `#[getter("prop_name")]`, `#[setter]`,
  `#[setter("prop_name")]`
- **Descriptors**: `#[classmethod]` (installs a native classmethod descriptor),
  `#[staticmethod]`
- **Type-based method overloads**: declare multiple typed `fn`s with the same
  name inside `class { ... }`

```rust
pyrust_module! {
    class Base {
        constants {
            "VERSION" => Value::int(1),
        }

        #[getter]
        fn name(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            /* ... */
        }

        #[setter("name")]
        fn set_name(#[positional_only] self_val: PyValueRef<'_>, v: PyStr<'_>) -> Result<Value> {
            /* ... */
        }

        #[classmethod]
        fn from_str(#[positional_only] cls: PyValueRef<'_>, s: PyStr<'_>) -> Result<Value> {
            /* ... */
        }

        #[staticmethod]
        fn helper(x: PyInt) -> Result<Value> {
            Ok(Value::int(x.expect_i64(FN_NAME, "x")? * 2))
        }

        // Overloaded methods
        fn encode(#[positional_only] _self: PyValueRef<'_>, x: PyInt) -> Result<Value> { /* ... */ }
        fn encode(#[positional_only] _self: PyValueRef<'_>, x: PyStr<'_>) -> Result<Value> { /* ... */ }
    }

    class Derived("Base") {
        constants {
            "VERSION" => Value::int(2),
        }
    }
}
```

---

## 3. Native Exceptions, Struct-Sequences, and Enums

### Exceptions inside `pyrust_module!`

```rust
pyrust_module! {
    /// Raised on protocol errors.
    exception SSLError(OSError);

    /// Inherits from `Exception` by default.
    exception GenericError;

    /// Inherits from an earlier exception in the same module.
    exception SSLCertVerificationError(SSLError);

    /// Multiple bases.
    exception CombinedError(SSLError, ValueError);
}
```

### Struct-Sequences (`pyrust_struct_seq!`)

Creates a CPython-compatible `structseq` (`tuple` subclass) with positional
indexing, slicing, unpacking, named field access, `n_sequence_fields`,
`n_fields`, `n_unnamed_fields`, `__match_args__`, `__new__`, and `repr`:

```rust
pyrust_struct_seq! {
    pub(crate) struct_passwd in "pwd" {
        fields: [pw_name, pw_passwd, pw_uid, pw_gid, pw_gecos, pw_dir, pw_shell],
    }
}

// Optional `extra_fields` and `unnamed_fields`:
pyrust_struct_seq! {
    pub(crate) struct_rusage in "resource" {
        fields: [ru_utime, ru_stime, ru_maxrss],
        extra_fields: [ru_extra],
        unnamed_fields: 0,
    }
}

// Export the class in `pyrust_module!`:
// constants { "struct_passwd" => struct_passwd::class(), }
//
// Construct an instance from Rust:
// Ok(struct_passwd::make([v0, v1, v2, v3, v4, v5, v6]))
```

### `IntEnum` & `IntFlag` (`pyrust_int_enum!`, `pyrust_int_flag!`)

Creates genuine Python `enum.IntEnum` / `enum.IntFlag` subclasses from Rust
constant tables (supporting `#[cfg(...)]` on individual variants):

```rust
pyrust_int_enum! {
    pub(crate) VerifyMode in "ssl" {
        CERT_NONE = 0,
        CERT_OPTIONAL = 1,
        CERT_REQUIRED = 2,
    }
}

pyrust_int_flag! {
    pub(crate) Options in "ssl" {
        OP_ALL = 0x8000_0BFF,
        #[cfg(unix)]
        OP_NO_SSLv2 = 0x0100_0000,
    }
}

// Install during `prepare_builtin_module(name, interp, module)`:
// VerifyMode::install(interp, module)?;
// Options::install_with_globals(interp, module)?; // also exports OP_* on the module
```

---

## 4. Variadic `*args` / `**kwargs`, CamelCase Names, and `FromValue` Wrappers

### `#[varargs]` and `#[varkwargs]`

- `#[varargs] args: &[Value]` (or `Vec<Value>`): collects extra positional args.
  Parameters declared after `#[varargs]` are automatically keyword-only.
- `#[varkwargs] kwargs: IndexMap<String, Value>` (or `HashMap<String, Value>`,
  `Vec<(String, Value)>`, `&[(String, Value)]`): collects excess keyword args.
  Must be the last parameter.
- `#[py_name = "CamelCase"]` or direct `fn CamelCase(...)` both work cleanly
  without lint warnings.

```rust
fn format_call(
    fmt: PyStr<'_>,
    #[varargs] args: &[Value],
    #[default(false.into())] strict: PyBool,
    #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
) -> Result<Value> {
    /* ... */
}

#[py_name = "getLogger"]
fn get_logger(#[default(None)] name: Option<PyStr<'_>>) -> Result<Value> {
    /* ... */
}
```

### Protocol-Driven `FromValue` Wrappers (`crate::interpreter::builtin_args`)

All wrappers work directly in typed `pyrust_module!` signatures (including
inside `Option<T>` and `OptionalArg<T>`) and automatically invoke Python dunder
protocols via `_interp`:

| Wrapper | Accepted Python inputs | Primary accessors |
| --- | --- | --- |
| `PyBytesLike<'_>` | `bytes` (zero-copy), `bytearray`, `memoryview`, subclasses, PEP 688 `__buffer__` (auto-releases via `__release_buffer__`) | `.as_bytes() -> &[u8]`, `.into_vec() -> Vec<u8>`, `Deref<Target=[u8]>` |
| `PyPath` | `str`, `bytes`, subclasses, `os.PathLike` (`__fspath__`) | `.as_path() -> &Path`, `.into_path_buf()`, `.as_os_str()`, `.into_os_string()`, `.is_bytes()` |
| `PyIndex` | `int`, `bool`, `BigInt`, subclasses, `__index__` | `.as_i64()`, `.expect_i64(FN_NAME, "x")`, `.as_isize()`, `.expect_isize(...)`, `.to_bigint()` |
| `PyNumber` | `float`, `int`, `bool`, `BigInt`, `__float__`, `__index__` | `.as_f64() -> f64`, `Deref<Target=f64>` |
| `PyCallable` | Any `callable(v) == True` object | `.as_value() -> &Value`, `.into_value() -> Value`, `Deref<Target=Value>` |

---

## 5. Typed Native Class State (`#[native_state(MyRustState)]`)

Attach a typed Rust struct `T: 'static` to instances of a `pyrust_module!`
class. The payload is stored in an invisible native slot isolated from
`__dict__` so Python code cannot see or corrupt it.

Inside any method, getter, or setter of a `#[native_state(T)]` class:
1. Local helpers are injected automatically:
   - `set_state(inst, state)`
   - `with_state(inst, |s: &T| -> Result<R> { ... })`
   - `with_state_mut(inst, |s: &mut T| -> Result<R> { ... })`
   - `has_state(inst) -> bool`
2. Methods, getters, and setters may also take a typed state receiver directly
   as their first parameter (`#[state] state: &T` or `#[state_mut] state: &mut T`):

```rust
struct HasherState {
    bytes_fed: usize,
}

pyrust_module! {
    #[native_state(HasherState)]
    class Hasher {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            set_state(self_val.0, HasherState { bytes_fed: 0 })?;
            Ok(Value::none())
        }

        fn update(#[state_mut] st: &mut HasherState, data: PyBytesLike<'_>) -> Result<Value> {
            st.bytes_fed += data.len();
            Ok(Value::none())
        }

        #[getter]
        fn bytes_fed(#[state] st: &HasherState) -> Result<Value> {
            Ok(Value::int(st.bytes_fed as i64))
        }
    }
}
```
