//! Shared runtime helpers and declarative macros for native-Rust stdlib modules.
//!
//! Re-exported across all `builtin_modules/bodies/*.rs` modules:
//! - [`pyrust_struct_seq!`] — CPython-compatible `structseq` named tuple types.
//! - [`pyrust_int_enum!`] / [`pyrust_int_flag!`] — genuine Python `enum.IntEnum`
//!   and `enum.IntFlag` subclasses from Rust constant tables.
//! - [`native_state`] — typed per-instance Rust state storage backing
//!   `#[native_state(MyState)] class ...` in `pyrust_module!`.
//! - [`IntoClassBase`] / [`link_class_bases`] / [`resolve_exception_base`] —
//!   base-class resolution helpers used by `pyrust_module!` codegen.

#![allow(dead_code)]

use std::any::Any;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::builtin_registry::{BuiltinCallableMetadata, BuiltinReg};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{InstanceAttrs, ModuleAttrs, PyClass, PyInstance, PyKey, Value, ValueKind};

// ─── Module Identity ─────────────────────────────────────────────────────────

/// Declare the identity a `pyrust_module!` body reads from its enclosing
/// module: `MODULE_NAME`, `FN_PREFIX`, and the `__pyrust_qualname!` macro that
/// turns a callable's short name into its registry name.
///
/// `__pyrust_qualname!` is a `concat!`, so every registry name is a string
/// literal in read-only data rather than a string built and leaked at run
/// time.  A plain `const` prefix cannot do this: `concat!` only accepts
/// literals.
///
/// - `pyrust_module_identity!("os.path")` — callables register as
///   `"os.path.<name>"`.
/// - `pyrust_module_identity!(@flat "builtins")` — callables register under
///   their short name only.
macro_rules! pyrust_module_identity {
    (@flat $name:expr) => {
        /// Python-level name of this built-in module.  Used for
        /// `PyModule.name` and as the key for `import`.
        pub(crate) const MODULE_NAME: &str = $name;
        /// Empty prefix: this module's functions live in the global
        /// namespace (`abs`, `len`, …).
        #[allow(dead_code)]
        pub(crate) const FN_PREFIX: &str = "";

        #[allow(unused_macros)]
        macro_rules! __pyrust_qualname {
            ($suffix:expr) => {
                $suffix
            };
        }
    };
    ($name:expr) => {
        /// Python-level name of this built-in module.  Used for
        /// `PyModule.name` and as the key for `import`.
        pub(crate) const MODULE_NAME: &str = $name;
        /// Prefix prepended to every function's short name to form its
        /// Python-level registration name.
        #[allow(dead_code)]
        pub(crate) const FN_PREFIX: &str = concat!($name, ".");

        #[allow(unused_macros)]
        macro_rules! __pyrust_qualname {
            ($suffix:expr) => {
                concat!($name, ".", $suffix)
            };
        }
    };
}

// ─── Class & Exception Base Resolution ───────────────────────────────────────

/// Trait implemented for values accepted as a base class in `pyrust_module!`
/// `class Sub(base)` / `class Sub : base` and `exception MyErr(base)`.
pub(crate) trait IntoClassBase {
    fn into_class_base(self, module_attrs: &ModuleAttrs) -> Rc<RefCell<PyClass>>;
}

impl IntoClassBase for Rc<RefCell<PyClass>> {
    fn into_class_base(self, _module_attrs: &ModuleAttrs) -> Rc<RefCell<PyClass>> {
        self
    }
}

impl IntoClassBase for &Rc<RefCell<PyClass>> {
    fn into_class_base(self, _module_attrs: &ModuleAttrs) -> Rc<RefCell<PyClass>> {
        Rc::clone(self)
    }
}

impl IntoClassBase for Option<Rc<RefCell<PyClass>>> {
    fn into_class_base(self, _module_attrs: &ModuleAttrs) -> Rc<RefCell<PyClass>> {
        self.expect("pyrust_module! class base resolved to None")
    }
}

impl IntoClassBase for Value {
    fn into_class_base(self, _module_attrs: &ModuleAttrs) -> Rc<RefCell<PyClass>> {
        match self.kind() {
            ValueKind::PyClass(cls) => Rc::clone(cls),
            _ => panic!(
                "pyrust_module! class base must be a PyClass, got {}",
                pyrust_core::builtin_type_name(&self)
            ),
        }
    }
}

impl IntoClassBase for &Value {
    fn into_class_base(self, _module_attrs: &ModuleAttrs) -> Rc<RefCell<PyClass>> {
        match self.kind() {
            ValueKind::PyClass(cls) => Rc::clone(cls),
            _ => panic!(
                "pyrust_module! class base must be a PyClass, got {}",
                pyrust_core::builtin_type_name(self)
            ),
        }
    }
}

impl IntoClassBase for &str {
    fn into_class_base(self, module_attrs: &ModuleAttrs) -> Rc<RefCell<PyClass>> {
        resolve_named_class_base(module_attrs, self)
    }
}

/// Resolve a class base specified by Python name: checks the current module's
/// `attrs` first (allowing inheritance from classes/exceptions declared earlier
/// in the same `pyrust_module!`), then canonical primitive classes (`dict`,
/// `tuple`, `int`, `object`, `type`, …), then built-in exception classes.
pub(crate) fn resolve_named_class_base(
    module_attrs: &ModuleAttrs,
    name: &str,
) -> Rc<RefCell<PyClass>> {
    if let Some(val) = module_attrs.get(name)
        && let ValueKind::PyClass(cls) = val.kind()
    {
        return Rc::clone(cls);
    }
    if name == "object" {
        return crate::interpreter::object_class_singleton();
    }
    if name == "type" {
        return crate::interpreter::type_class_singleton();
    }
    if let Some(cls) = crate::interpreter::primitive_class_by_name(name) {
        return cls;
    }
    if let Some(cls) = crate::interpreter::lookup_exc_class(name) {
        return cls;
    }
    panic!("pyrust_module! could not resolve base class `{name}`");
}

/// Resolve an exception base name (checks current module's `attrs` first, then
/// built-in exception classes via `lookup_exc_class`).
pub(crate) fn resolve_exception_base(
    module_attrs: &ModuleAttrs,
    name: &str,
) -> Rc<RefCell<PyClass>> {
    if let Some(val) = module_attrs.get(name)
        && let ValueKind::PyClass(cls) = val.kind()
    {
        return Rc::clone(cls);
    }
    if let Some(cls) = crate::interpreter::lookup_exc_class(name) {
        return cls;
    }
    panic!("pyrust_module! could not resolve exception base `{name}`");
}

/// Attach `bases` (`[primary, extra...]`) to `cls` and register `cls` in each
/// base's weakref `subclasses` list.
pub(crate) fn link_class_bases(cls: &Rc<RefCell<PyClass>>, bases: Vec<Rc<RefCell<PyClass>>>) {
    let mut iter = bases.into_iter();
    let Some(primary) = iter.next() else {
        return;
    };
    let extras: Vec<Rc<RefCell<PyClass>>> = iter.collect();
    {
        let mut borrowed = cls.borrow_mut();
        borrowed.base = Some(Rc::clone(&primary));
        borrowed.extra_bases = extras.clone();
    }
    primary
        .borrow()
        .subclasses
        .borrow_mut()
        .push(Rc::downgrade(cls));
    for extra in &extras {
        extra
            .borrow()
            .subclasses
            .borrow_mut()
            .push(Rc::downgrade(cls));
    }
}

// ─── Typed Native Class State (`#[native_state(T)]`) ─────────────────────────

/// Runtime storage helpers for `#[native_state(MyState)] class MyClass { … }`.
///
/// Each instance stores its `Box<dyn Any>` inside an invisible native slot
/// (`InstanceAttrs::insert_slot`), completely isolated from Python's `__dict__`
/// so user code cannot inspect or corrupt the Rust payload.
pub(crate) mod native_state {
    use super::*;
    use pyrust_core::{BuiltinState, BuiltinTypeOps, builtin_ops_is};

    /// Invisible native-slot key on `InstanceAttrs`.
    pub(crate) const SLOT_NAME: &str = "__pyrust_native_state__";

    struct NativeStateOps;
    static NATIVE_STATE_OPS: NativeStateOps = NativeStateOps;

    impl BuiltinTypeOps for NativeStateOps {
        fn type_name(&self) -> &'static str {
            "_native_state"
        }

        fn repr(&self, _state: &BuiltinState) -> String {
            "<native state>".to_string()
        }

        fn truthy(&self, _state: &BuiltinState) -> bool {
            true
        }
    }

    fn extract_state_handle(self_val: &Value, type_name: &str) -> Result<BuiltinState> {
        let ValueKind::PyInstance(inst_rc) = self_val.kind() else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "descriptor requires a '{}' object but received a '{}'",
                    type_name,
                    value_type_name_str(self_val),
                ),
            ));
        };
        let inst = inst_rc.borrow();
        let Some(slot_val) = inst.attrs.get_slot(SLOT_NAME) else {
            return Err(PyError::named(
                "RuntimeError",
                format!("'{type_name}' object is not initialized"),
            ));
        };
        let ValueKind::BuiltinObject { ops, state } = slot_val.kind() else {
            return Err(PyError::named(
                "RuntimeError",
                format!("'{type_name}' object has invalid internal state"),
            ));
        };
        if !builtin_ops_is::<NativeStateOps>(ops) {
            return Err(PyError::named(
                "RuntimeError",
                format!("'{type_name}' object has invalid internal state"),
            ));
        }
        // Clone the Rc<RefCell<Box<dyn Any>>> and drop `inst` borrow before
        // running user closures so callbacks can freely inspect/mutate `self`.
        Ok(Rc::clone(state))
    }

    /// Attach (or replace) typed Rust state `T` on a `PyInstance`.
    pub(crate) fn set_native_state<T: 'static>(self_val: &Value, state: T) -> Result<()> {
        let ValueKind::PyInstance(inst_rc) = self_val.kind() else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "expected an instance object, got '{}'",
                    value_type_name_str(self_val)
                ),
            ));
        };
        let payload = Value::builtin_object(&NATIVE_STATE_OPS, Box::new(state));
        inst_rc.borrow_mut().attrs.insert_slot(SLOT_NAME, payload);
        Ok(())
    }

    /// Return `true` iff `self_val` is a `PyInstance` holding state of type `T`.
    #[allow(dead_code)]
    pub(crate) fn has_native_state<T: 'static>(self_val: &Value) -> bool {
        let ValueKind::PyInstance(inst_rc) = self_val.kind() else {
            return false;
        };
        let inst = inst_rc.borrow();
        let Some(slot_val) = inst.attrs.get_slot(SLOT_NAME) else {
            return false;
        };
        let ValueKind::BuiltinObject { ops, state } = slot_val.kind() else {
            return false;
        };
        if !builtin_ops_is::<NativeStateOps>(ops) {
            return false;
        }
        state.try_borrow().is_ok_and(|b| b.is::<T>())
    }

    /// Borrow typed Rust state `&T` stored on `self_val` and run `f`.
    pub(crate) fn with_native_state<T: 'static, R>(
        self_val: &Value,
        type_name: &str,
        f: impl FnOnce(&T) -> Result<R>,
    ) -> Result<R> {
        let handle = extract_state_handle(self_val, type_name)?;
        let borrow = handle.try_borrow().map_err(|_| {
            PyError::named(
                "RuntimeError",
                format!("reentrant borrow of '{type_name}' internal state"),
            )
        })?;
        let typed = borrow.downcast_ref::<T>().ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "descriptor requires a '{}' object but received a '{}'",
                    type_name,
                    value_type_name_str(self_val),
                ),
            )
        })?;
        f(typed)
    }

    /// Mutably borrow typed Rust state `&mut T` stored on `self_val` and run `f`.
    pub(crate) fn with_native_state_mut<T: 'static, R>(
        self_val: &Value,
        type_name: &str,
        f: impl FnOnce(&mut T) -> Result<R>,
    ) -> Result<R> {
        let handle = extract_state_handle(self_val, type_name)?;
        let mut borrow = handle.try_borrow_mut().map_err(|_| {
            PyError::named(
                "RuntimeError",
                format!("reentrant mutable borrow of '{type_name}' internal state"),
            )
        })?;
        let typed = borrow.downcast_mut::<T>().ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "descriptor requires a '{}' object but received a '{}'",
                    type_name,
                    value_type_name_str(self_val),
                ),
            )
        })?;
        f(typed)
    }

    /// Remove and return typed Rust state `T` from `self_val`.
    #[allow(dead_code)]
    pub(crate) fn take_native_state<T: 'static>(self_val: &Value, type_name: &str) -> Result<T> {
        let ValueKind::PyInstance(inst_rc) = self_val.kind() else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "descriptor requires a '{}' object but received a '{}'",
                    type_name,
                    value_type_name_str(self_val),
                ),
            ));
        };
        let removed = inst_rc.borrow_mut().attrs.shift_remove_slot(SLOT_NAME);
        let Some(slot_val) = removed else {
            return Err(PyError::named(
                "RuntimeError",
                format!("'{type_name}' object is not initialized"),
            ));
        };
        let ValueKind::BuiltinObject { ops, state } = slot_val.kind() else {
            return Err(PyError::named(
                "RuntimeError",
                format!("'{type_name}' object has invalid internal state"),
            ));
        };
        if !builtin_ops_is::<NativeStateOps>(ops) {
            return Err(PyError::named(
                "RuntimeError",
                format!("'{type_name}' object has invalid internal state"),
            ));
        }
        let state = Rc::clone(state);
        drop(slot_val);
        let boxed: Box<dyn Any> = match Rc::try_unwrap(state) {
            Ok(cell) => cell.into_inner(),
            Err(_) => {
                return Err(PyError::named(
                    "RuntimeError",
                    format!("'{type_name}' internal state is currently borrowed"),
                ));
            }
        };
        boxed.downcast::<T>().map(|b| *b).map_err(|_| {
            PyError::named(
                "TypeError",
                format!(
                    "descriptor requires a '{}' object but received a '{}'",
                    type_name,
                    value_type_name_str(self_val),
                ),
            )
        })
    }
}

// ─── Struct-Sequence (`pyrust_struct_seq!`) Runtime Support ──────────────────

/// Static descriptor for a CPython-compatible `structseq` named tuple type.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StructSeqSpec {
    pub(crate) module_name: &'static str,
    pub(crate) type_name: &'static str,
    pub(crate) qualname: &'static str,
    pub(crate) sequence_fields: &'static [&'static str],
    pub(crate) extra_fields: &'static [&'static str],
    pub(crate) unnamed_fields: usize,
}

impl StructSeqSpec {
    #[inline]
    pub(crate) const fn n_sequence_fields(&self) -> usize {
        self.sequence_fields.len()
    }

    #[inline]
    pub(crate) const fn n_fields(&self) -> usize {
        self.sequence_fields.len() + self.extra_fields.len()
    }

    #[inline]
    pub(crate) const fn n_unnamed_fields(&self) -> usize {
        self.unnamed_fields
    }
}

// Each name's bytes live in a `static`, so the registry entry and the
// function objects built from the name share one pointer (see
// `crate::builtin_registry::name_str`).
static STRUCTSEQ_NEW_NAME_BYTES: [u8; 18] =
    crate::builtin_registry::name_bytes("_structseq.__new__");
static STRUCTSEQ_REPR_NAME_BYTES: [u8; 19] =
    crate::builtin_registry::name_bytes("_structseq.__repr__");
const STRUCTSEQ_NEW_NAME: &str = crate::builtin_registry::name_str(&STRUCTSEQ_NEW_NAME_BYTES);
const STRUCTSEQ_REPR_NAME: &str = crate::builtin_registry::name_str(&STRUCTSEQ_REPR_NAME_BYTES);

// Thread-local justification:
// (a) Why `thread_local!`: Maps `!Send` weak `PyClass` pointers created by
//     `pyrust_struct_seq!` back to their `&'static StructSeqSpec` without
//     polluting the class's Python-visible `__dict__`.
// (b) Multi-interpreter / multi-threaded impact: Entries are weak pointers that
//     are pruned on insertion; each thread maintains its own table.
type StructSeqEntry = (Weak<RefCell<PyClass>>, &'static StructSeqSpec);

pyrust_core::gil_local! {
    static STRUCT_SEQ_SPECS: RefCell<Vec<StructSeqEntry>> =
        const { RefCell::new(Vec::new()) };
}

fn register_struct_seq_spec(cls: &Rc<RefCell<PyClass>>, spec: &'static StructSeqSpec) {
    STRUCT_SEQ_SPECS.with(|cell| {
        let mut entries = cell.borrow_mut();
        entries.retain(|(weak, _)| weak.strong_count() > 0);
        entries.push((Rc::downgrade(cls), spec));
    });
}

fn lookup_struct_seq_spec(cls: &Rc<RefCell<PyClass>>) -> Option<&'static StructSeqSpec> {
    let target = Rc::as_ptr(cls);
    STRUCT_SEQ_SPECS.with(|cell| {
        cell.borrow().iter().find_map(|(weak, spec)| {
            weak.upgrade()
                .is_some_and(|rc| std::ptr::eq(Rc::as_ptr(&rc), target))
                .then_some(*spec)
        })
    })
}

/// Build a CPython-compatible `structseq` `PyClass` inheriting from `tuple`.
pub(crate) fn build_struct_seq_class(spec: &'static StructSeqSpec) -> Rc<RefCell<PyClass>> {
    let tuple_base = crate::interpreter::primitive_class_by_name("tuple")
        .expect("canonical tuple class must exist before structseq creation");

    let mut attrs = indexmap::IndexMap::new();
    attrs.insert("__module__".to_string(), Value::string(spec.module_name));
    attrs.insert(
        "__new__".to_string(),
        Value::fresh_builtin_function(STRUCTSEQ_NEW_NAME),
    );
    attrs.insert(
        "__repr__".to_string(),
        Value::fresh_builtin_function(STRUCTSEQ_REPR_NAME),
    );
    attrs.insert(
        "n_sequence_fields".to_string(),
        Value::int(spec.n_sequence_fields() as i64),
    );
    attrs.insert("n_fields".to_string(), Value::int(spec.n_fields() as i64));
    attrs.insert(
        "n_unnamed_fields".to_string(),
        Value::int(spec.n_unnamed_fields() as i64),
    );

    let match_count = spec
        .sequence_fields
        .len()
        .saturating_sub(spec.unnamed_fields);
    let match_args: Vec<Value> = spec.sequence_fields[..match_count]
        .iter()
        .map(|name| Value::string(*name))
        .collect();
    attrs.insert("__match_args__".to_string(), Value::tuple(match_args));

    let mut class = PyClass::new(
        spec.type_name,
        spec.type_name,
        Some(Rc::clone(&tuple_base)),
        attrs,
    );
    class.non_subclassable_name = Some(spec.qualname);
    // CPython's `tp_name` is the dotted qualname, so error messages say
    // `'time.struct_time' object ...`.
    class.error_name = Some(spec.qualname);
    let cls_rc = Rc::new(RefCell::new(class));
    tuple_base
        .borrow()
        .subclasses
        .borrow_mut()
        .push(Rc::downgrade(&cls_rc));
    register_struct_seq_spec(&cls_rc, spec);
    cls_rc
}

/// Construct a `structseq` instance directly from Rust values.
pub(crate) fn make_struct_seq_instance(
    class: Rc<RefCell<PyClass>>,
    spec: &'static StructSeqSpec,
    seq_values: Vec<Value>,
    extra_values: Vec<Value>,
) -> Value {
    debug_assert_eq!(
        seq_values.len(),
        spec.sequence_fields.len(),
        "structseq `{}` expected {} sequence values, got {}",
        spec.qualname,
        spec.sequence_fields.len(),
        seq_values.len()
    );
    let mut attrs = InstanceAttrs::new();
    for (field, val) in spec.sequence_fields.iter().zip(seq_values.iter()) {
        attrs.insert(*field, val.clone());
    }
    for (idx, field) in spec.extra_fields.iter().enumerate() {
        let val = extra_values.get(idx).cloned().unwrap_or_else(Value::none);
        attrs.insert(*field, val);
    }
    attrs.set_builtin_data(Value::tuple(seq_values));
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

fn structseq_new(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let class = match args.first().map(|a| a.value.kind()) {
        Some(ValueKind::PyClass(cls)) => Rc::clone(cls),
        _ => {
            return Err(PyError::runtime(
                "internal: _structseq.__new__ first argument must be a class".to_string(),
            ));
        }
    };
    let spec = lookup_struct_seq_spec(&class).ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!("type '{}' is not a structseq type", class.borrow().name),
        )
    })?;

    let supplied = &args[1..];
    let positional: Vec<&ExpandedCallArg> = supplied.iter().filter(|a| a.name.is_none()).collect();
    if positional.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{}() takes a dict as second arg, if any", spec.qualname),
        ));
    }

    let mut sequence = positional.first().map(|a| a.value.clone());
    let mut overrides = positional.get(1).map(|a| a.value.clone());
    let mut invalid_kw: Option<String> = None;
    for arg in supplied.iter().filter(|a| a.name.is_some()) {
        match arg.name.as_deref().unwrap_or_default() {
            "sequence" => {
                if sequence.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        "argument for structseq() given by name ('sequence') and position (1)"
                            .to_string(),
                    ));
                }
                sequence = Some(arg.value.clone());
            }
            "dict" => {
                if overrides.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        "argument for structseq() given by name ('dict') and position (2)"
                            .to_string(),
                    ));
                }
                overrides = Some(arg.value.clone());
            }
            other => {
                invalid_kw = Some(other.to_string());
            }
        }
    }

    let sequence = sequence.ok_or_else(|| {
        PyError::named(
            "TypeError",
            "structseq() missing required argument 'sequence' (pos 1)".to_string(),
        )
    })?;
    if let Some(other) = invalid_kw {
        return Err(PyError::named(
            "TypeError",
            format!("'{other}' is an invalid keyword argument for structseq()"),
        ));
    }
    if overrides
        .as_ref()
        .is_some_and(|v| !matches!(v.kind(), ValueKind::Dict(_)))
    {
        return Err(PyError::named(
            "TypeError",
            format!("{}() takes a dict as second arg, if any", spec.qualname),
        ));
    }

    // CPython's `PySequence_Fast(arg, "constructor requires a sequence")`
    // replaces the generic "not iterable" failure for non-iterable input.
    let values = interp.collect_iterable(&sequence).map_err(|e| {
        if e.class_name_is("TypeError") && e.to_string().ends_with("object is not iterable") {
            PyError::named("TypeError", "constructor requires a sequence".to_string())
        } else {
            e
        }
    })?;
    let min_len = spec.n_sequence_fields();
    let max_len = spec.n_fields();
    if min_len == max_len {
        if values.len() != min_len {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{}() takes a {min_len}-sequence ({}-sequence given)",
                    spec.qualname,
                    values.len(),
                ),
            ));
        }
    } else if values.len() < min_len {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{}() takes an at least {min_len}-sequence ({}-sequence given)",
                spec.qualname,
                values.len(),
            ),
        ));
    } else if values.len() > max_len {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{}() takes an at most {max_len}-sequence ({}-sequence given)",
                spec.qualname,
                values.len(),
            ),
        ));
    }

    let seq_values = values[..min_len].to_vec();
    let mut extra_values = Vec::with_capacity(spec.extra_fields.len());
    for (idx, field) in spec.extra_fields.iter().enumerate() {
        let from_dict = overrides.as_ref().and_then(|d| {
            let ValueKind::Dict(dict) = d.kind() else {
                return None;
            };
            dict.get(&PyKey::str_from(field)).cloned()
        });
        let val = from_dict
            .or_else(|| values.get(min_len + idx).cloned())
            .unwrap_or_else(Value::none);
        extra_values.push(val);
    }

    Ok(make_struct_seq_instance(
        class,
        spec,
        seq_values,
        extra_values,
    ))
}

fn structseq_repr(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    if args.iter().any(|a| a.name.is_some()) {
        return Err(PyError::named(
            "TypeError",
            "__repr__() takes no keyword arguments".to_string(),
        ));
    }
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "__repr__() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    }
    let ValueKind::PyInstance(inst_rc) = args[0].value.kind() else {
        return Err(PyError::named(
            "TypeError",
            "__repr__() expected a structseq instance".to_string(),
        ));
    };
    let inst = inst_rc.borrow();
    let spec = lookup_struct_seq_spec(&inst.class).ok_or_else(|| {
        PyError::named(
            "TypeError",
            "__repr__() expected a structseq instance".to_string(),
        )
    })?;
    let tuple_vals = inst.attrs.builtin_data().and_then(|v| match v.kind() {
        ValueKind::Tuple(t) => Some(t),
        _ => None,
    });
    let mut out = String::with_capacity(spec.qualname.len() + 32);
    out.push_str(spec.qualname);
    out.push('(');
    for (idx, field) in spec.sequence_fields.iter().enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        out.push_str(field);
        out.push('=');
        let repr = tuple_vals
            .and_then(|t| t.get(idx))
            .or_else(|| inst.attrs.get(field))
            .map_or_else(|| "None".to_string(), Value::repr_raw);
        out.push_str(&repr);
    }
    out.push(')');
    Ok(Value::string(out))
}

/// Builtin registrations for the shared `_structseq` runtime hooks.  Part of
/// the registry's always-available tier: a struct sequence can be built by
/// `sys` before any module is imported.
pub(crate) static STRUCTSEQ_REGS: &[BuiltinReg] = &[
    BuiltinReg {
        name: STRUCTSEQ_NEW_NAME,
        dispatch: structseq_new,
        metadata: BuiltinCallableMetadata::method_descriptor("builtins", STRUCTSEQ_NEW_NAME),
        fast: None,
        min_arity: 0,
        max_arity: 0,
    },
    BuiltinReg {
        name: STRUCTSEQ_REPR_NAME,
        dispatch: structseq_repr,
        metadata: BuiltinCallableMetadata::method_descriptor("builtins", STRUCTSEQ_REPR_NAME),
        fast: None,
        min_arity: 0,
        max_arity: 0,
    },
];

// ─── IntEnum & IntFlag Runtime Builders ──────────────────────────────────────

/// Build a genuine Python `enum.IntEnum` or `enum.IntFlag` subclass via the
/// interpreter's `enum` module.
fn build_enum_subclass(
    interp: &mut Interpreter,
    base_name: &str,
    module_name: &str,
    class_name: &str,
    members: &[(&str, i64)],
) -> Result<Value> {
    let enum_mod = interp.load_module("enum")?;
    let base_cls = interp.get_attr(&enum_mod, base_name)?;
    let member_pairs: Vec<Value> = members
        .iter()
        .map(|(k, v)| Value::tuple(vec![Value::string(*k), Value::int(*v)]))
        .collect();
    let call_args = [
        ExpandedCallArg {
            name: None,
            value: Value::string(class_name),
        },
        ExpandedCallArg {
            name: None,
            value: Value::list(member_pairs),
        },
        ExpandedCallArg {
            name: Some("module".to_string()),
            value: Value::string(module_name),
        },
    ];
    interp.call_function_expanded(base_cls, &call_args)
}

/// Build a genuine Python `enum.Enum` subclass with `__module__ = module_name`.
pub(crate) fn build_enum(
    interp: &mut Interpreter,
    module_name: &str,
    class_name: &str,
    members: &[(&str, i64)],
) -> Result<Value> {
    build_enum_subclass(interp, "Enum", module_name, class_name, members)
}

/// Build a genuine Python `enum.IntEnum` subclass with `__module__ = module_name`.
pub(crate) fn build_int_enum(
    interp: &mut Interpreter,
    module_name: &str,
    class_name: &str,
    members: &[(&str, i64)],
) -> Result<Value> {
    build_enum_subclass(interp, "IntEnum", module_name, class_name, members)
}

/// Build a genuine Python `enum.IntFlag` subclass with `__module__ = module_name`.
pub(crate) fn build_int_flag(
    interp: &mut Interpreter,
    module_name: &str,
    class_name: &str,
    members: &[(&str, i64)],
) -> Result<Value> {
    build_enum_subclass(interp, "IntFlag", module_name, class_name, members)
}

/// Install an already-built `IntEnum` / `IntFlag` class onto `module`,
/// optionally also exporting every enum member as a module-level constant.
pub(crate) fn install_enum_on_module(
    interp: &mut Interpreter,
    module: &Value,
    class_name: &str,
    enum_cls: Value,
    export_globals: bool,
    members: &[(&str, i64)],
) -> Result<Value> {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return Err(PyError::runtime(
            "install_enum_on_module expected a PyModule".to_string(),
        ));
    };
    mod_rc
        .borrow_mut()
        .insert_attr(class_name.to_string(), enum_cls.clone());
    if export_globals {
        for (member_name, _) in members {
            let member_val = interp.get_attr(&enum_cls, member_name)?;
            mod_rc
                .borrow_mut()
                .insert_attr((*member_name).to_string(), member_val);
        }
    }
    Ok(enum_cls)
}

// ─── Declarative Macros ──────────────────────────────────────────────────────

/// Declare a CPython-compatible `structseq` named-tuple type.
///
/// Generates a Rust module `$name` exposing:
/// - `SPEC: StructSeqSpec`
/// - `N_SEQUENCE_FIELDS: usize`
/// - `class() -> Value` (thread-local singleton `PyClass`, subclass of `tuple`)
/// - `class_rc() -> Rc<RefCell<PyClass>>`
/// - `make(values: [Value; N_SEQUENCE_FIELDS]) -> Value`
/// - `make_from_vec(values: Vec<Value>) -> Value`
/// - `make_with_extra(seq_values: Vec<Value>, extra_values: Vec<Value>) -> Value`
///
/// # Example
/// ```ignore
/// pyrust_struct_seq! {
///     pub(crate) struct_passwd in "pwd" {
///         fields: [pw_name, pw_passwd, pw_uid, pw_gid, pw_gecos, pw_dir, pw_shell],
///     }
/// }
/// ```
#[macro_export]
macro_rules! pyrust_struct_seq {
    (
        $vis:vis $name:ident in $module:literal {
            fields: [$($field:ident),* $(,)?]
            $(, extra_fields: [$($extra:ident),* $(,)?])?
            $(, unnamed_fields: $unnamed:expr)?
            $(,)?
        }
    ) => {
        #[allow(non_snake_case, dead_code)]
        $vis mod $name {
            use std::cell::RefCell;
            use std::rc::Rc;

            pub(crate) const SEQUENCE_FIELDS: &[&str] = &[$(stringify!($field)),*];
            pub(crate) const EXTRA_FIELDS: &[&str] = &[$($(stringify!($extra)),*)?];
            pub(crate) const N_SEQUENCE_FIELDS: usize = SEQUENCE_FIELDS.len();
            pub(crate) const N_UNNAMED_FIELDS: usize = {
                let _v: usize = 0;
                $(let _v: usize = $unnamed;)?
                _v
            };

            pub(crate) const SPEC: $crate::builtin_modules::macros::StructSeqSpec =
                $crate::builtin_modules::macros::StructSeqSpec {
                    module_name: $module,
                    type_name: stringify!($name),
                    qualname: concat!($module, ".", stringify!($name)),
                    sequence_fields: SEQUENCE_FIELDS,
                    extra_fields: EXTRA_FIELDS,
                    unnamed_fields: N_UNNAMED_FIELDS,
                };

            pyrust_core::gil_local! {
                static CLASS: Rc<RefCell<$crate::value::PyClass>> =
                    $crate::builtin_modules::macros::build_struct_seq_class(&SPEC);
            }

            pub(crate) fn class_rc() -> Rc<RefCell<$crate::value::PyClass>> {
                CLASS.with(Rc::clone)
            }

            pub(crate) fn class() -> $crate::value::Value {
                $crate::value::Value::py_class(class_rc())
            }

            pub(crate) fn make(
                values: [$crate::value::Value; N_SEQUENCE_FIELDS],
            ) -> $crate::value::Value {
                $crate::builtin_modules::macros::make_struct_seq_instance(
                    class_rc(),
                    &SPEC,
                    values.into(),
                    Vec::new(),
                )
            }

            pub(crate) fn make_from_vec(
                values: Vec<$crate::value::Value>,
            ) -> $crate::value::Value {
                $crate::builtin_modules::macros::make_struct_seq_instance(
                    class_rc(),
                    &SPEC,
                    values,
                    Vec::new(),
                )
            }

            pub(crate) fn make_with_extra(
                seq_values: Vec<$crate::value::Value>,
                extra_values: Vec<$crate::value::Value>,
            ) -> $crate::value::Value {
                $crate::builtin_modules::macros::make_struct_seq_instance(
                    class_rc(),
                    &SPEC,
                    seq_values,
                    extra_values,
                )
            }
        }
    };
}

/// Declare a genuine Python `enum.IntEnum` subclass from a Rust constant table.
///
/// Individual variants support `#[cfg(...)]` attributes. Generates a module
/// `$name` exposing `NAME`, `MODULE`, `members()`, `build(interp)`,
/// `install(interp, module)`, and `install_with_globals(interp, module)`.
///
/// # Example
/// ```ignore
/// pyrust_int_enum! {
///     pub(crate) VerifyMode in "ssl" {
///         CERT_NONE = 0,
///         CERT_OPTIONAL = 1,
///         CERT_REQUIRED = 2,
///     }
/// }
/// ```
#[macro_export]
macro_rules! pyrust_int_enum {
    (
        $vis:vis $name:ident in $module:literal {
            $(
                $(#[cfg($($cfg:tt)*)])*
                $variant:ident = $val:expr
            ),* $(,)?
        }
    ) => {
        #[allow(non_snake_case, dead_code)]
        $vis mod $name {
            pub(crate) const NAME: &str = stringify!($name);
            pub(crate) const MODULE: &str = $module;

            pub(crate) fn members() -> Vec<(&'static str, i64)> {
                let mut out: Vec<(&'static str, i64)> = Vec::new();
                $(
                    $(#[cfg($($cfg)*)])*
                    {
                        out.push((stringify!($variant), $val as i64));
                    }
                )*
                out
            }

            pub(crate) fn build(
                interp: &mut $crate::interpreter::Interpreter,
            ) -> $crate::error::Result<$crate::value::Value> {
                let entries = members();
                $crate::builtin_modules::macros::build_int_enum(interp, MODULE, NAME, &entries)
            }

            pub(crate) fn install(
                interp: &mut $crate::interpreter::Interpreter,
                module: &$crate::value::Value,
            ) -> $crate::error::Result<$crate::value::Value> {
                let entries = members();
                let cls =
                    $crate::builtin_modules::macros::build_int_enum(interp, MODULE, NAME, &entries)?;
                $crate::builtin_modules::macros::install_enum_on_module(
                    interp, module, NAME, cls, false, &entries,
                )
            }

            pub(crate) fn install_with_globals(
                interp: &mut $crate::interpreter::Interpreter,
                module: &$crate::value::Value,
            ) -> $crate::error::Result<$crate::value::Value> {
                let entries = members();
                let cls =
                    $crate::builtin_modules::macros::build_int_enum(interp, MODULE, NAME, &entries)?;
                $crate::builtin_modules::macros::install_enum_on_module(
                    interp, module, NAME, cls, true, &entries,
                )
            }
        }
    };
}

/// Declare a genuine Python `enum.IntFlag` subclass from a Rust constant table.
///
/// Individual variants support `#[cfg(...)]` attributes. Generates a module
/// `$name` exposing `NAME`, `MODULE`, `members()`, `build(interp)`,
/// `install(interp, module)`, and `install_with_globals(interp, module)`.
///
/// # Example
/// ```ignore
/// pyrust_int_flag! {
///     pub(crate) Options in "ssl" {
///         OP_ALL = 0x80000BFF,
///         OP_NO_SSLv2 = 0x01000000,
///         OP_NO_SSLv3 = 0x02000000,
///     }
/// }
/// ```
#[macro_export]
macro_rules! pyrust_int_flag {
    (
        $vis:vis $name:ident in $module:literal {
            $(
                $(#[cfg($($cfg:tt)*)])*
                $variant:ident = $val:expr
            ),* $(,)?
        }
    ) => {
        #[allow(non_snake_case, dead_code)]
        $vis mod $name {
            pub(crate) const NAME: &str = stringify!($name);
            pub(crate) const MODULE: &str = $module;

            pub(crate) fn members() -> Vec<(&'static str, i64)> {
                let mut out: Vec<(&'static str, i64)> = Vec::new();
                $(
                    $(#[cfg($($cfg)*)])*
                    {
                        out.push((stringify!($variant), $val as i64));
                    }
                )*
                out
            }

            pub(crate) fn build(
                interp: &mut $crate::interpreter::Interpreter,
            ) -> $crate::error::Result<$crate::value::Value> {
                let entries = members();
                $crate::builtin_modules::macros::build_int_flag(interp, MODULE, NAME, &entries)
            }

            pub(crate) fn install(
                interp: &mut $crate::interpreter::Interpreter,
                module: &$crate::value::Value,
            ) -> $crate::error::Result<$crate::value::Value> {
                let entries = members();
                let cls =
                    $crate::builtin_modules::macros::build_int_flag(interp, MODULE, NAME, &entries)?;
                $crate::builtin_modules::macros::install_enum_on_module(
                    interp, module, NAME, cls, false, &entries,
                )
            }

            pub(crate) fn install_with_globals(
                interp: &mut $crate::interpreter::Interpreter,
                module: &$crate::value::Value,
            ) -> $crate::error::Result<$crate::value::Value> {
                let entries = members();
                let cls =
                    $crate::builtin_modules::macros::build_int_flag(interp, MODULE, NAME, &entries)?;
                $crate::builtin_modules::macros::install_enum_on_module(
                    interp, module, NAME, cls, true, &entries,
                )
            }
        }
    };
}

#[allow(unused_imports)]
pub(crate) use {pyrust_int_enum, pyrust_int_flag, pyrust_struct_seq};
