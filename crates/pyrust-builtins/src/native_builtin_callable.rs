//! Python-facing wrapper for a native static built-in callable.
//!
//! A primitive type's static method is stored in its class dictionary as a
//! real `staticmethod` descriptor.  Descriptor access returns this stable
//! callable object, whose Python type is `builtin_function_or_method`, whose
//! `__self__` is `None`, and whose invocation does not prepend a receiver.
//! The interpreter unwraps it at its builtin-call adapter boundary.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use pyrust_core::{BuiltinState, BuiltinTypeOps, PyClass, Value, ValueKind};

pub struct NativeStaticBuiltinState {
    pub wrapped: Value,
    name: Rc<String>,
    qualname: Rc<String>,
    receiver: Option<Value>,
    /// Generation-bound module callables carry a private receiver used only
    /// by the interpreter adapter.  It must not leak through `__self__`.
    hide_receiver: bool,
    /// Module-generation callables present as ordinary built-in functions;
    /// descriptor-bound class/static methods retain their existing method
    /// presentation.
    display_as_function: bool,
    owner_id: i64,
    /// Per-callable `__module__` slot. Native static/class-bound builtins
    /// expose the same mutable slot as captured built-in methods in CPython.
    module: Value,
}

/// Interpreter-facing call payload for a native builtin wrapper.
pub struct NativeBuiltinCall {
    pub wrapped: Value,
    pub receiver: Option<Value>,
}

pub struct NativeStaticBuiltinOps;
pub const NATIVE_STATIC_BUILTIN_OPS: &NativeStaticBuiltinOps = &NativeStaticBuiltinOps;
pub const TYPE_NAME: &str = "builtin_function_or_method";

#[inline]
fn has_native_builtin_ops(ops: &dyn BuiltinTypeOps) -> bool {
    pyrust_core::builtin_ops_is::<NativeStaticBuiltinOps>(ops)
}

impl BuiltinTypeOps for NativeStaticBuiltinOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let callable = borrow
            .downcast_ref::<NativeStaticBuiltinState>()
            .expect("NativeStaticBuiltinState");
        if callable.display_as_function {
            format!("<built-in function {}>", callable.name)
        } else {
            format!(
                "<built-in method {} of type object at 0x{:x}>",
                callable.name, callable.owner_id
            )
        }
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        let borrow = state.borrow();
        let Some(callable) = borrow.downcast_ref::<NativeStaticBuiltinState>() else {
            return false;
        };
        let ValueKind::BuiltinObject {
            state: other_state, ..
        } = other.kind()
        else {
            return false;
        };
        let other_borrow = other_state.borrow();
        let Some(other) = other_borrow.downcast_ref::<NativeStaticBuiltinState>() else {
            return false;
        };
        callable.wrapped == other.wrapped
            && callable.receiver == other.receiver
            && callable.hide_receiver == other.hide_receiver
            && callable.display_as_function == other.display_as_function
            && callable.owner_id == other.owner_id
    }

    fn hash(&self, state: &BuiltinState) -> Option<u64> {
        let borrow = state.borrow();
        let callable = borrow.downcast_ref::<NativeStaticBuiltinState>()?;
        let mut hash = 14695981039346656037_u64;
        for byte in callable.qualname.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(1099511628211);
        }
        hash ^= callable.owner_id as u64;
        Some(if hash == u64::MAX { u64::MAX - 1 } else { hash })
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let callable = borrow.downcast_ref::<NativeStaticBuiltinState>()?;
        match name {
            "__name__" => Some(Value::string(callable.name.as_str())),
            "__qualname__" => Some(Value::string(callable.qualname.as_str())),
            "__self__" => Some(if callable.hide_receiver {
                Value::none()
            } else {
                callable.receiver.clone().unwrap_or_else(Value::none)
            }),
            "__module__" => Some(callable.module.clone()),
            "__doc__" => Some(Value::none()),
            _ => None,
        }
    }
}

/// Create the stable callable stored inside a native `staticmethod`.
pub fn native_static_builtin(
    wrapped: Value,
    owner: &Rc<RefCell<PyClass>>,
    name: impl Into<String>,
) -> Value {
    let name = Rc::new(name.into());
    let qualname = Rc::new(format!("{}.{}", owner.borrow().qualname, name));
    let owner_id = Rc::as_ptr(owner) as i64;
    let state: Box<dyn Any> = Box::new(NativeStaticBuiltinState {
        wrapped,
        name,
        qualname,
        receiver: None,
        hide_receiver: false,
        display_as_function: false,
        owner_id,
        module: Value::none(),
    });
    Value::builtin_object(NATIVE_STATIC_BUILTIN_OPS, state)
}

/// Create a native class-bound builtin. Its receiver is prepended only at the
/// interpreter adapter boundary, never by generic attribute lookup.
pub fn native_class_builtin(
    wrapped: Value,
    receiver: Value,
    name: Rc<String>,
    qualname: Rc<String>,
) -> Value {
    let owner_id = match receiver.kind() {
        ValueKind::PyClass(owner) => Rc::as_ptr(owner) as i64,
        _ => receiver.value_id().unwrap_or(0),
    };
    let state: Box<dyn Any> = Box::new(NativeStaticBuiltinState {
        wrapped,
        name,
        qualname,
        receiver: Some(receiver),
        hide_receiver: false,
        display_as_function: false,
        owner_id,
        module: Value::none(),
    });
    Value::builtin_object(NATIVE_STATIC_BUILTIN_OPS, state)
}

/// Create a module-generation-bound native callable.
///
/// `generation` is an opaque, non-module owner value.  Keeping it in the
/// callable lets a retained function continue to construct objects from its
/// original import generation without creating a `module -> callable ->
/// module` reference cycle.  The interpreter's existing native-callable
/// adapter prepends the value only at invocation; Python presentation keeps
/// `__self__` hidden and looks like an ordinary built-in function.
pub fn native_generation_builtin(
    wrapped: Value,
    generation: Value,
    name: impl Into<String>,
    module_name: &str,
) -> Value {
    let name = Rc::new(name.into());
    let qualname = Rc::clone(&name);
    let owner_id = generation.value_id().unwrap_or(0);
    let state: Box<dyn Any> = Box::new(NativeStaticBuiltinState {
        wrapped,
        name,
        qualname,
        receiver: Some(generation),
        hide_receiver: true,
        display_as_function: true,
        owner_id,
        module: Value::string(module_name),
    });
    Value::builtin_object(NATIVE_STATIC_BUILTIN_OPS, state)
}

/// Return the typed call payload for a native built-in wrapper.
pub fn as_native_static_builtin(value: &Value) -> Option<NativeBuiltinCall> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !has_native_builtin_ops(ops) {
        return None;
    }
    let borrow = state.borrow();
    let callable = borrow.downcast_ref::<NativeStaticBuiltinState>()?;
    Some(NativeBuiltinCall {
        wrapped: callable.wrapped.clone(),
        receiver: callable.receiver.clone(),
    })
}

/// Assign the per-object `__module__` slot of a native static/class-bound
/// builtin. Returns false for every other `BuiltinObject` category.
pub fn set_module(value: &Value, module: Value) -> bool {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return false;
    };
    if !has_native_builtin_ops(ops) {
        return false;
    }
    let mut borrow = state.borrow_mut();
    let Some(callable) = borrow.downcast_mut::<NativeStaticBuiltinState>() else {
        return false;
    };
    callable.module = module;
    true
}
