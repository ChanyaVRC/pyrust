//! Per-interpreter `struct` registry: the shared `struct.error` class, the
//! hidden `unpack_iterator` class, and the compiled-format cache.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyClass, Value, ValueKind};

use super::format::{CompiledFormat, compile_format};

/// CPython's `MAXCACHE`: the format cache is cleared when it grows past this.
const MAX_CACHED_FORMATS: usize = 100;

#[derive(Default)]
struct StructRegistry {
    /// `struct.error`.  CPython keeps `_struct` alive across `struct.py`
    /// re-imports, so every generation of the module shares one class.
    error: Option<Rc<RefCell<PyClass>>>,
    /// `_struct.unpack_iterator` — the type returned by `iter_unpack`; it is
    /// deliberately not exported from the module.
    unpack_iterator: Option<Rc<RefCell<PyClass>>>,
    /// Compiled formats keyed by their `str` spelling (`Struct` cache).
    formats: HashMap<String, Rc<CompiledFormat>>,
}

// `gil_local!` justification:
// (a) Why: the registry stores `!Send` `Rc` class handles and compiled formats
//     shared by every Python thread of one interpreter under the GIL.
// (b) Isolation: independent interpreters on separate OS threads each get
//     their own registry, so classes and caches never leak between them.
pyrust_core::gil_local! {
    static REGISTRY: RefCell<StructRegistry> = RefCell::new(StructRegistry::default());
}

/// Publish the freshly built module's `error` / `unpack_iterator` classes.
///
/// The first generation seen by this interpreter wins: later generations of
/// the module are re-pointed at it so `struct.error` stays one class.
/// `unpack_iterator` is removed from the module namespace (CPython does not
/// export it) and kept only here.
pub(crate) fn adopt_module_classes(module: &Rc<RefCell<crate::value::PyModule>>) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let mut module = module.borrow_mut();
        match registry.error.clone() {
            Some(shared) => {
                module.insert_attr("error".to_string(), Value::py_class(shared));
            }
            None => {
                if let Some(ValueKind::PyClass(cls)) = module.attrs.get("error").map(Value::kind) {
                    registry.error = Some(Rc::clone(cls));
                }
            }
        }
        if let Some(hidden) = module.attrs.shift_remove("unpack_iterator")
            && registry.unpack_iterator.is_none()
            && let ValueKind::PyClass(cls) = hidden.kind()
        {
            registry.unpack_iterator = Some(Rc::clone(cls));
        }
    });
}

fn registered<T>(pick: impl Fn(&StructRegistry) -> Option<T>) -> Option<T> {
    REGISTRY.with(|registry| pick(&registry.borrow()))
}

/// The shared `struct.error` class, importing `struct` first if needed.
pub(super) fn error_class(interp: &mut Interpreter) -> Result<Value> {
    if let Some(cls) = registered(|r| r.error.clone()) {
        return Ok(Value::py_class(cls));
    }
    interp.load_module("struct")?;
    registered(|r| r.error.clone())
        .map(Value::py_class)
        .ok_or_else(|| PyError::runtime("struct: error class is not initialised".into()))
}

/// The hidden `unpack_iterator` class, importing `struct` first if needed.
pub(super) fn unpack_iterator_class(interp: &mut Interpreter) -> Result<Rc<RefCell<PyClass>>> {
    if let Some(cls) = registered(|r| r.unpack_iterator.clone()) {
        return Ok(cls);
    }
    interp.load_module("struct")?;
    registered(|r| r.unpack_iterator.clone())
        .ok_or_else(|| PyError::runtime("struct: unpack_iterator is not initialised".into()))
}

/// Compile `format`, reusing the per-interpreter cache for `str` formats.
pub(super) fn compile_cached(format: &Value, error_cls: &Value) -> Result<Rc<CompiledFormat>> {
    let key = format.as_str();
    if let Some(key) = key
        && let Some(hit) = registered(|r| r.formats.get(key).cloned())
    {
        return Ok(hit);
    }
    let compiled = Rc::new(compile_format(format, error_cls)?);
    if let Some(key) = key {
        REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            if registry.formats.len() >= MAX_CACHED_FORMATS {
                registry.formats.clear();
            }
            registry
                .formats
                .insert(key.to_string(), Rc::clone(&compiled));
        });
    }
    Ok(compiled)
}

/// `struct._clearcache()`.
pub(super) fn clear_format_cache() {
    REGISTRY.with(|registry| registry.borrow_mut().formats.clear());
}
