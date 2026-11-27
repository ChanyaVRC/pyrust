// `_imp` internal module — 100% native-Rust Python 3.12 import machinery
// primitives (issue #3250).
//
// Exposes `is_builtin`, `is_frozen`, `is_frozen_package`, `create_builtin`,
// `exec_builtin`, `extension_suffixes`, `acquire_lock`, `release_lock`,
// `lock_held`, `source_hash`, `_frozen_module_names`, and
// `check_hash_based_pycs`.
//
// Reference: <https://docs.python.org/3.12/library/importlib.html>

use std::cell::Cell;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{PyBytesLike, PyIndex, PyStr, PyValueRef};
use crate::value::{Value, ValueKind};

// Thread-local justification:
// (a) Why `thread_local!`: PyRust interpreters run on a single thread; tracking
//     re-entrant `_imp.acquire_lock()` / `_imp.release_lock()` depth per thread
//     matches CPython's per-thread import lock ownership check without needing
//     global mutex state.
// (b) Multi-interpreter / multi-threaded impact: Each OS thread has an
//     independent lock depth starting at 0.
thread_local! {
    static IMPORT_LOCK_DEPTH: Cell<usize> = const { Cell::new(0) };
}

pyrust_module! {
    constants {
        "check_hash_based_pycs" => Value::string("default"),
    }

    fn is_builtin(#[positional_only] name: PyStr<'_>) -> Result<Value> {
        let _ = _interp;
        let s: &str = &name;
        let code = if matches!(s, "sys" | "builtins") {
            -1
        } else if crate::builtin_modules::is_builtin_module(s) {
            1
        } else {
            0
        };
        Ok(Value::int(code))
    }

    fn is_frozen(#[positional_only] _name: PyStr<'_>) -> Result<Value> {
        let _ = _interp;
        Ok(Value::bool_(false))
    }

    fn is_frozen_package(#[positional_only] _name: PyStr<'_>) -> Result<Value> {
        let _ = _interp;
        Ok(Value::bool_(false))
    }

    fn _frozen_module_names() -> Result<Value> {
        let _ = _interp;
        Ok(Value::list(Vec::new()))
    }

    fn create_builtin(#[positional_only] spec: PyValueRef<'_>) -> Result<Value> {
        let name_val = _interp.get_attr(spec.0, "name")?;
        let Some(name_str) = name_val.as_str() else {
            return Err(PyError::named("TypeError", "spec.name must be a str"));
        };
        if !crate::builtin_modules::is_builtin_module(name_str) {
            return Ok(Value::none());
        }
        _interp.load_module(name_str)
    }

    fn exec_builtin(#[positional_only] mod_val: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        if !matches!(mod_val.0.kind(), ValueKind::PyModule(_)) {
            return Err(PyError::named("TypeError", "expected a module object"));
        }
        Ok(Value::int(0))
    }

    fn extension_suffixes() -> Result<Value> {
        let _ = _interp;
        #[cfg(windows)]
        let suffixes = vec![Value::string(".pyd")];
        #[cfg(not(windows))]
        let suffixes = vec![Value::string(".so")];
        Ok(Value::list(suffixes))
    }

    fn acquire_lock() -> Result<Value> {
        let _ = _interp;
        IMPORT_LOCK_DEPTH.with(|d| d.set(d.get().saturating_add(1)));
        Ok(Value::none())
    }

    fn release_lock() -> Result<Value> {
        let _ = _interp;
        let held = IMPORT_LOCK_DEPTH.with(|d| {
            let cur = d.get();
            if cur == 0 {
                false
            } else {
                d.set(cur - 1);
                true
            }
        });
        if !held {
            return Err(PyError::named(
                "RuntimeError",
                "not holding the import lock",
            ));
        }
        Ok(Value::none())
    }

    fn lock_held() -> Result<Value> {
        let _ = _interp;
        let held = IMPORT_LOCK_DEPTH.with(|d| d.get() > 0);
        Ok(Value::bool_(held))
    }

    /// Return the SipHash-1-3 of `source` keyed with `key` (PEP 552), as
    /// 8 little-endian bytes.
    fn source_hash(key: PyIndex, source: PyBytesLike<'_>) -> Result<Value> {
        let _ = _interp;
        let k = key.expect_i64(FN_NAME, "key")? as u64;
        let digest = crate::builtin_modules::py_compile::siphash13(k, source.as_bytes());
        Ok(Value::bytes(digest.to_le_bytes().to_vec()))
    }

    /// Override the multi-interpreter extension check.  CPython 3.12 only
    /// allows this from a subinterpreter, and this runtime only has the main
    /// interpreter, so it always raises (`importlib.util`'s
    /// `_incompatible_extension_module_restrictions` therefore does too).
    fn _override_multi_interp_extensions_check(
        #[positional_only] override_: PyIndex,
    ) -> Result<Value> {
        let _ = _interp;
        override_.expect_i64(FN_NAME, "override")?;
        Err(PyError::named(
            "RuntimeError",
            "_imp._override_multi_interp_extensions_check() cannot be used in the main interpreter",
        ))
    }
}
