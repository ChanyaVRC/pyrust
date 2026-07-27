//! `classmethod` and `staticmethod` descriptor wrappers.
//!
//! This module provides two sets of `BuiltinObject` types:
//!
//! ## Any-value wrappers (for non-`UserFunction` arguments)
//!
//! CPython 3.12 accepts any object as the argument to `classmethod()` or
//! `staticmethod()`.  pyrust's existing path uses `Value::class_method` /
//! `Value::static_method` which require `Rc<UserFunction>`.  When the
//! argument is not a `UserFunction`, we wrap it in one of these
//! `BuiltinObject` types instead.
//!
//! - `ClassMethodAny` — descriptor wrapping an arbitrary value.  Exposes
//!   `__func__` and `__get__(instance, owner)`.
//! - `StaticMethodAny` — same for `staticmethod`.
//!
//! ## `__get__` binder objects (for `UserFunction` classmethods)
//!
//! When the user explicitly calls `cm.__get__(instance, owner)` where `cm` is
//! a `UserFunction` classmethod or staticmethod, the attribute lookup returns a
//! `ClassMethodGetBinder` / `StaticMethodGetBinder`.  The interpreter's
//! `call_function_expanded` recognises these and applies the descriptor
//! binding.

use std::any::Any;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

use indexmap::IndexMap;
use pyrust_core::{
    BuiltinState, BuiltinTypeOps, CanonicalClassTag, PyClass, PyError, UserFunction, Value,
    ValueKind, builtin_ops_is,
};

// ─── staticmethod any ────────────────────────────────────────────────────────

/// State for a `staticmethod` wrapping an arbitrary (non-`UserFunction`) value.
pub struct StaticMethodAnyState {
    pub wrapped: Value,
}

pub struct StaticMethodAnyOps;
pub const STATIC_METHOD_ANY_OPS: &StaticMethodAnyOps = &StaticMethodAnyOps;
pub const STATIC_TYPE_NAME: &str = "staticmethod";

impl BuiltinTypeOps for StaticMethodAnyOps {
    fn type_name(&self) -> &'static str {
        STATIC_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<StaticMethodAnyState>()
            .expect("StaticMethodAnyState");
        format!("<staticmethod({})>", s.wrapped.repr_raw())
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        builtin_state_identity_eq::<StaticMethodAnyOps>(state, other)
    }

    fn hash(&self, state: &BuiltinState) -> Option<u64> {
        Some(builtin_state_identity_hash(state))
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        if name == "__func__" {
            let borrow = state.borrow();
            let s = borrow.downcast_ref::<StaticMethodAnyState>()?;
            return Some(s.wrapped.clone());
        }
        None
    }

    fn has_method(&self, name: &str) -> bool {
        name == "__get__"
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value, PyError> {
        if name != "__get__" {
            return Err(PyError::named(
                "AttributeError",
                format!("'staticmethod' object has no attribute '{name}'"),
            ));
        }
        // CPython 3.12: __get__(None, None) is invalid — instance and owner
        // cannot both be None.
        let instance = args.first().cloned().unwrap_or_else(Value::none);
        let owner = args.get(1).cloned().unwrap_or_else(Value::none);
        if matches!(instance.kind(), ValueKind::None) && matches!(owner.kind(), ValueKind::None) {
            return Err(PyError::named(
                "TypeError",
                "__get__(None, None) is invalid".to_string(),
            ));
        }
        // staticmethod.__get__(instance, owner) — returns the wrapped value
        // directly, ignoring both arguments (CPython Data Model §3.3.2).
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<StaticMethodAnyState>()
            .expect("StaticMethodAnyState");
        Ok(s.wrapped.clone())
    }
}

/// Construct a `staticmethod` descriptor wrapping an arbitrary `Value`.
pub fn static_method_any(wrapped: Value) -> Value {
    let state: Box<dyn Any> = Box::new(StaticMethodAnyState { wrapped });
    Value::builtin_object(STATIC_METHOD_ANY_OPS, state)
}

/// Return `Some(wrapped)` if `value` is a non-function staticmethod wrapper,
/// cloning the inner value.  Returns `None` for all other value kinds.
pub fn as_static_method_any(value: &Value) -> Option<Value> {
    with_static_method_any(value, |s| s.wrapped.clone())
}

/// Run `f` with a borrow of the underlying [`StaticMethodAnyState`].
pub fn with_static_method_any<R>(
    value: &Value,
    f: impl FnOnce(&StaticMethodAnyState) -> R,
) -> Option<R> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<StaticMethodAnyOps>(ops) {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<StaticMethodAnyState>()?;
    Some(f(s))
}

// ─── classmethod any ─────────────────────────────────────────────────────────

/// State for a `classmethod` wrapping an arbitrary (non-`UserFunction`) value.
pub struct ClassMethodAnyState {
    pub wrapped: Value,
    binding_style: ClassMethodBindingStyle,
    native_metadata: Option<NativeClassMethodMetadata>,
}

#[derive(Clone)]
struct NativeClassMethodMetadata {
    owner: Weak<RefCell<PyClass>>,
    owner_name: Rc<String>,
    name: Rc<String>,
    qualname: Rc<String>,
}

/// Binding policy is an intrinsic property of the descriptor provider.
///
/// A Python-created `classmethod(value)` always produces a normal Python
/// bound-method object, even when `value` happens to be a built-in function.
/// Interpreter-owned C-style class methods use the native adapter instead.
/// Keeping that distinction here prevents generic attribute lookup from
/// guessing semantics from the wrapped callable's spelling or `ValueKind`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum ClassMethodBindingStyle {
    Python,
    NativeBuiltin,
}

/// Typed binding specification returned by [`as_class_method_any`].
///
/// Consumers deliberately cannot inspect the policy; they can only pass the
/// specification to [`bind_wrapped_class_method`].  That keeps the
/// Python-vs-native descriptor decision at construction time.
#[derive(Clone)]
pub struct ClassMethodBindingSpec {
    wrapped: Value,
    style: ClassMethodBindingStyle,
    native_metadata: Option<NativeClassMethodMetadata>,
}

/// Target-specific, cache-safe plan for an interpreter-owned native
/// `classmethod` descriptor.
///
/// `wrapped` is admitted only when it is a registry `BuiltinFunction` with no
/// Python bytecode. It therefore cannot point back to the `FnCode` cache that
/// retains this plan; ordinary `UserFunction` descriptors stay on the generic
/// path.
///
/// The descriptor provider resolves and validates its owner exactly once when
/// an attribute cache is filled.  The bytecode cache retains this opaque plan
/// and may only ask this module to bind it or produce its direct-call payload.
/// A weak target reference avoids introducing a `FnCode -> class -> function
/// -> FnCode` ownership cycle.
#[derive(Clone)]
pub struct NativeClassMethodCachePlan {
    wrapped: Value,
    target: Weak<RefCell<PyClass>>,
    name: Rc<String>,
    qualname: Rc<String>,
}

pub struct ClassMethodAnyOps;
pub const CLASS_METHOD_ANY_OPS: &ClassMethodAnyOps = &ClassMethodAnyOps;
pub const CLASS_TYPE_NAME: &str = "classmethod";

pub struct NativeClassMethodDescriptorOps;
pub const NATIVE_CLASS_METHOD_DESCRIPTOR_OPS: &NativeClassMethodDescriptorOps =
    &NativeClassMethodDescriptorOps;
pub const NATIVE_CLASS_METHOD_DESCRIPTOR_TYPE_NAME: &str = "classmethod_descriptor";

impl BuiltinTypeOps for ClassMethodAnyOps {
    fn type_name(&self) -> &'static str {
        CLASS_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<ClassMethodAnyState>()
            .expect("ClassMethodAnyState");
        format!("<classmethod({})>", s.wrapped.repr_raw())
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        builtin_state_identity_eq::<ClassMethodAnyOps>(state, other)
    }

    fn hash(&self, state: &BuiltinState) -> Option<u64> {
        Some(builtin_state_identity_hash(state))
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        if name == "__func__" {
            let borrow = state.borrow();
            let s = borrow.downcast_ref::<ClassMethodAnyState>()?;
            return Some(s.wrapped.clone());
        }
        None
    }

    fn has_method(&self, name: &str) -> bool {
        name == "__get__"
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value, PyError> {
        call_class_method_descriptor_get(state, name, args, CLASS_TYPE_NAME)
    }
}

impl BuiltinTypeOps for NativeClassMethodDescriptorOps {
    fn type_name(&self) -> &'static str {
        NATIVE_CLASS_METHOD_DESCRIPTOR_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let descriptor = borrow
            .downcast_ref::<ClassMethodAnyState>()
            .expect("ClassMethodAnyState");
        let metadata = descriptor
            .native_metadata
            .as_ref()
            .expect("native classmethod metadata");
        format!(
            "<method '{}' of '{}' objects>",
            metadata.name, metadata.owner_name
        )
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        builtin_state_identity_eq::<NativeClassMethodDescriptorOps>(state, other)
    }

    fn hash(&self, state: &BuiltinState) -> Option<u64> {
        Some(builtin_state_identity_hash(state))
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let descriptor = borrow.downcast_ref::<ClassMethodAnyState>()?;
        let metadata = descriptor.native_metadata.as_ref()?;
        match name {
            "__name__" => Some(Value::string(metadata.name.as_str())),
            "__qualname__" => Some(Value::string(metadata.qualname.as_str())),
            "__objclass__" => metadata.owner.upgrade().map(Value::py_class),
            "__doc__" => Some(Value::none()),
            _ => None,
        }
    }

    fn has_method(&self, name: &str) -> bool {
        name == "__get__"
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value, PyError> {
        call_class_method_descriptor_get(
            state,
            name,
            args,
            NATIVE_CLASS_METHOD_DESCRIPTOR_TYPE_NAME,
        )
    }
}

fn call_class_method_descriptor_get(
    state: &BuiltinState,
    name: &str,
    args: Vec<Value>,
    descriptor_type: &str,
) -> Result<Value, PyError> {
    if name != "__get__" {
        return Err(PyError::named(
            "AttributeError",
            format!("'{descriptor_type}' object has no attribute '{name}'"),
        ));
    }
    // classmethod.__get__(instance, owner)
    // args[0] = instance (None or PyInstance — ignored for classmethod)
    // args[1] = owner (the owning class)
    //
    // CPython 3.12: __get__(None, None) is invalid — instance and owner
    // cannot both be None.
    let instance = args.first().cloned().unwrap_or_else(Value::none);
    let owner = args.get(1).cloned().unwrap_or_else(Value::none);
    if matches!(instance.kind(), ValueKind::None) && matches!(owner.kind(), ValueKind::None) {
        return Err(PyError::named(
            "TypeError",
            "__get__(None, None) is invalid".to_string(),
        ));
    }

    let borrow = state.borrow();
    let descriptor = borrow
        .downcast_ref::<ClassMethodAnyState>()
        .expect("ClassMethodAnyState");
    let binding = ClassMethodBindingSpec {
        wrapped: descriptor.wrapped.clone(),
        style: descriptor.binding_style,
        native_metadata: descriptor.native_metadata.clone(),
    };
    drop(borrow);
    let class_rc = match owner.kind() {
        ValueKind::PyClass(class) => Some(Rc::clone(class)),
        _ => match instance.kind() {
            ValueKind::PyInstance(object) => Some(Rc::clone(&object.borrow().class)),
            _ => None,
        },
    };

    if let Some(class_rc) = class_rc {
        return bind_wrapped_class_method(binding, class_rc);
    }

    // No recognisable owner class. Automatic attribute access always supplies
    // one; preserve the wrapped value for this explicit malformed descriptor
    // invocation rather than guessing a class.
    Ok(binding.wrapped)
}

/// Construct a `classmethod` descriptor wrapping an arbitrary `Value`.
pub fn class_method_any(wrapped: Value) -> Value {
    let state: Box<dyn Any> = Box::new(ClassMethodAnyState {
        wrapped,
        binding_style: ClassMethodBindingStyle::Python,
        native_metadata: None,
    });
    Value::builtin_object(CLASS_METHOD_ANY_OPS, state)
}

/// Construct a native classmethod descriptor owned by a concrete built-in
/// class. The owner is weakly referenced so installing the descriptor in the
/// class dictionary does not create an `Rc` cycle.
pub fn native_class_method_descriptor(
    wrapped: Value,
    owner: &Rc<RefCell<PyClass>>,
    name: impl Into<String>,
) -> Value {
    let name = Rc::new(name.into());
    let (owner_name, qualname) = {
        let owner = owner.borrow();
        (
            Rc::new(owner.name.clone()),
            Rc::new(format!("{}.{}", owner.qualname, name)),
        )
    };
    let state: Box<dyn Any> = Box::new(ClassMethodAnyState {
        wrapped,
        binding_style: ClassMethodBindingStyle::NativeBuiltin,
        native_metadata: Some(NativeClassMethodMetadata {
            owner: Rc::downgrade(owner),
            owner_name,
            name,
            qualname,
        }),
    });
    Value::builtin_object(NATIVE_CLASS_METHOD_DESCRIPTOR_OPS, state)
}

/// Bind the value stored in a `classmethod` descriptor to `class`.
///
/// Builtin registry functions need the same implicit-class adapter used by
/// `super()`; user functions have a native `ClassBoundMethod` representation.
/// Keeping this decision on the descriptor removes concrete Python method-name
/// knowledge from the interpreter's generic attribute lookup.
pub fn bind_wrapped_class_method(
    binding: ClassMethodBindingSpec,
    class: Rc<RefCell<PyClass>>,
) -> Result<Value, PyError> {
    validate_native_class_method_owner(&binding, &class)?;

    enum WrappedKind {
        UserFunction(Rc<UserFunction>),
        BuiltinFunction,
        Other,
    }

    let kind = match binding.wrapped.kind() {
        ValueKind::UserFunction(function) => WrappedKind::UserFunction(Rc::clone(function)),
        ValueKind::BuiltinFunction(_) => WrappedKind::BuiltinFunction,
        _ => WrappedKind::Other,
    };
    match kind {
        WrappedKind::UserFunction(function) => Ok(Value::class_bound_method(function, class)),
        WrappedKind::BuiltinFunction if binding.style == ClassMethodBindingStyle::NativeBuiltin => {
            let metadata = binding
                .native_metadata
                .expect("native classmethod descriptors carry provider metadata");
            let defining_owner = metadata
                .owner
                .upgrade()
                .expect("native classmethod owner validated before binding");
            let qualname = if Rc::ptr_eq(&class, &defining_owner) {
                Rc::clone(&metadata.qualname)
            } else {
                Rc::new(format!("{}.{}", class.borrow().qualname, metadata.name))
            };
            Ok(crate::native_builtin_callable::native_class_builtin(
                binding.wrapped,
                Value::py_class(class),
                Rc::clone(&metadata.name),
                qualname,
            ))
        }
        WrappedKind::BuiltinFunction | WrappedKind::Other => {
            Ok(class_bound_any(binding.wrapped, class))
        }
    }
}

/// Resolve an interpreter-owned native classmethod descriptor into a
/// target-specific cache plan.
///
/// Python-created `classmethod(...)` wrappers and native descriptors wrapping
/// anything other than a registry builtin remain on the generic descriptor
/// path.  Owner validation is performed here, before a plan can enter a
/// bytecode cache, so aliasing a descriptor onto an unrelated class retains
/// the normal `TypeError`.
pub fn native_class_method_cache_plan(
    descriptor: &Value,
    target: &Rc<RefCell<PyClass>>,
) -> Option<NativeClassMethodCachePlan> {
    let binding = as_class_method_any(descriptor)?;
    if binding.style != ClassMethodBindingStyle::NativeBuiltin
        || !matches!(binding.wrapped.kind(), ValueKind::BuiltinFunction(_))
        || binding.wrapped.as_function_rc().is_none_or(|function| {
            function.precompiled_code.is_some() || function.wrapped_func.is_some()
        })
    {
        return None;
    }
    validate_native_class_method_owner(&binding, target).ok()?;
    let metadata = binding
        .native_metadata
        .expect("native classmethod descriptors carry provider metadata");
    let defining_owner = metadata
        .owner
        .upgrade()
        .expect("native classmethod owner validated before cache planning");
    let qualname = if Rc::ptr_eq(target, &defining_owner) {
        Rc::clone(&metadata.qualname)
    } else {
        Rc::new(format!(
            "{}.{}",
            target.borrow().qualname,
            metadata.name.as_str()
        ))
    };
    Some(NativeClassMethodCachePlan {
        wrapped: binding.wrapped,
        target: Rc::downgrade(target),
        name: metadata.name,
        qualname,
    })
}

fn cached_native_class_method_target_matches(
    plan: &NativeClassMethodCachePlan,
    target: &Rc<RefCell<PyClass>>,
) -> bool {
    plan.target
        .upgrade()
        .is_some_and(|cached| Rc::ptr_eq(&cached, target))
}

/// Materialise a fresh Python-facing bound builtin from a validated cache plan.
///
/// Each attribute read still creates a distinct wrapper object, preserving
/// normal descriptor object identity.  Only lookup, classification, owner
/// validation, and qualname construction are cached.
pub fn bind_cached_native_class_method(
    plan: &NativeClassMethodCachePlan,
    target: Rc<RefCell<PyClass>>,
) -> Option<Value> {
    cached_native_class_method_target_matches(plan, &target).then(|| {
        crate::native_builtin_callable::native_class_builtin(
            plan.wrapped.clone(),
            Value::py_class(target),
            Rc::clone(&plan.name),
            Rc::clone(&plan.qualname),
        )
    })
}

/// Return the direct-call payload for a validated native classmethod plan.
///
/// Fused `CallMethod` bytecode may prepend the target class and invoke the
/// wrapped registry builtin without allocating the otherwise unobservable
/// bound-method wrapper.  Attribute reads use
/// [`bind_cached_native_class_method`] instead.
pub fn cached_native_class_method_call(
    plan: &NativeClassMethodCachePlan,
    target: &Rc<RefCell<PyClass>>,
) -> Option<(Value, Value)> {
    cached_native_class_method_target_matches(plan, target)
        .then(|| (plan.wrapped.clone(), Value::py_class(Rc::clone(target))))
}

fn validate_native_class_method_owner(
    binding: &ClassMethodBindingSpec,
    class: &Rc<RefCell<PyClass>>,
) -> Result<(), PyError> {
    if binding.style != ClassMethodBindingStyle::NativeBuiltin {
        return Ok(());
    }
    let metadata = binding
        .native_metadata
        .as_ref()
        .expect("native classmethod descriptors carry provider metadata");
    let Some(owner) = metadata.owner.upgrade() else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "descriptor '{}' requires a subtype of '{}' but received '{}'",
                metadata.name,
                metadata.owner_name,
                class.borrow().name
            ),
        ));
    };
    if native_class_is_subclass_of(class, &owner) {
        return Ok(());
    }
    Err(PyError::named(
        "TypeError",
        format!(
            "descriptor '{}' requires a subtype of '{}' but received '{}'",
            metadata.name,
            metadata.owner_name,
            class.borrow().name
        ),
    ))
}

fn native_class_is_subclass_of(
    class: &Rc<RefCell<PyClass>>,
    expected: &Rc<RefCell<PyClass>>,
) -> bool {
    if Rc::ptr_eq(class, expected) {
        return true;
    }
    if expected.borrow().canonical_tag == Some(CanonicalClassTag::Object) {
        return true;
    }
    let (base, extra_bases) = {
        let class = class.borrow();
        (class.base.clone(), class.extra_bases.clone())
    };
    base.is_some_and(|base| native_class_is_subclass_of(&base, expected))
        || extra_bases
            .iter()
            .any(|base| native_class_is_subclass_of(base, expected))
}

fn builtin_state_identity_eq<T: BuiltinTypeOps>(state: &BuiltinState, other: &Value) -> bool {
    let ValueKind::BuiltinObject {
        ops,
        state: other_state,
    } = other.kind()
    else {
        return false;
    };
    builtin_ops_is::<T>(ops) && Rc::ptr_eq(state, other_state)
}

fn builtin_state_identity_hash(state: &BuiltinState) -> u64 {
    let hash = Rc::as_ptr(state) as usize as u64;
    if hash == u64::MAX { u64::MAX - 1 } else { hash }
}

/// Return a typed binding specification when `value` is a non-function
/// classmethod wrapper. Returns `None` for all other value kinds.
pub fn as_class_method_any(value: &Value) -> Option<ClassMethodBindingSpec> {
    with_class_method_any(value, |s| ClassMethodBindingSpec {
        wrapped: s.wrapped.clone(),
        style: s.binding_style,
        native_metadata: s.native_metadata.clone(),
    })
}

/// Run `f` with a borrow of the underlying [`ClassMethodAnyState`].
pub fn with_class_method_any<R>(
    value: &Value,
    f: impl FnOnce(&ClassMethodAnyState) -> R,
) -> Option<R> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<ClassMethodAnyOps>(ops)
        && !builtin_ops_is::<NativeClassMethodDescriptorOps>(ops)
    {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<ClassMethodAnyState>()?;
    Some(f(s))
}

// ─── bound arbitrary classmethod value ──────────────────────────────────────

/// Bound result of a `classmethod` whose wrapped callable has no specialised
/// `ValueKind` method representation (for example another class object).
pub struct ClassBoundAnyState {
    pub wrapped: Value,
    pub class: Rc<RefCell<PyClass>>,
}

pub struct ClassBoundAnyOps;
pub const CLASS_BOUND_ANY_OPS: &ClassBoundAnyOps = &ClassBoundAnyOps;
pub const CLASS_BOUND_ANY_TYPE_NAME: &str = "method";

impl BuiltinTypeOps for ClassBoundAnyOps {
    fn type_name(&self) -> &'static str {
        CLASS_BOUND_ANY_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let bound = borrow
            .downcast_ref::<ClassBoundAnyState>()
            .expect("ClassBoundAnyState");
        let class = bound.class.borrow();
        let module = class
            .attrs
            .get("__module__")
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| "__main__".to_string());
        format!("<bound method ? of <class '{module}.{}'>>", class.qualname)
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let bound = borrow.downcast_ref::<ClassBoundAnyState>()?;
        match name {
            "__func__" => Some(bound.wrapped.clone()),
            "__self__" => Some(Value::py_class(Rc::clone(&bound.class))),
            _ => None,
        }
    }
}

fn class_bound_any(wrapped: Value, class: Rc<RefCell<PyClass>>) -> Value {
    let state: Box<dyn Any> = Box::new(ClassBoundAnyState { wrapped, class });
    Value::builtin_object(CLASS_BOUND_ANY_OPS, state)
}

/// Extract the callable and bound class from an arbitrary classmethod result.
pub fn as_class_bound_any(value: &Value) -> Option<(Value, Rc<RefCell<PyClass>>)> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<ClassBoundAnyOps>(ops) {
        return None;
    }
    let borrow = state.borrow();
    let bound = borrow.downcast_ref::<ClassBoundAnyState>()?;
    Some((bound.wrapped.clone(), Rc::clone(&bound.class)))
}

// ─── __get__ binders for UserFunction classmethod / staticmethod ──────────────

/// Returned by `classmethod.__get__` when the descriptor wraps a
/// `UserFunction`.  Calling this value (via the interpreter's
/// `call_function_expanded` guard arm) creates a `ClassBoundMethod`.
pub struct ClassMethodGetBinder {
    pub func: Rc<UserFunction>,
}

pub struct ClassMethodGetBinderOps;
pub const CLASS_METHOD_GET_BINDER_OPS: &ClassMethodGetBinderOps = &ClassMethodGetBinderOps;
pub const CLASS_BINDER_TYPE_NAME: &str = "classmethod_get_binder";

impl BuiltinTypeOps for ClassMethodGetBinderOps {
    fn type_name(&self) -> &'static str {
        CLASS_BINDER_TYPE_NAME
    }

    fn repr(&self, _state: &BuiltinState) -> String {
        "<classmethod.__get__ binder>".to_string()
    }
}

/// Construct a `classmethod.__get__` binder wrapping a `UserFunction`.
pub fn class_method_get_binder(func: Rc<UserFunction>) -> Value {
    let state: Box<dyn Any> = Box::new(ClassMethodGetBinder { func });
    Value::builtin_object(CLASS_METHOD_GET_BINDER_OPS, state)
}

/// Extract the `Rc<UserFunction>` from a `ClassMethodGetBinder` value, or
/// return `None` if the value is not one.
pub fn as_class_method_get_binder(value: &Value) -> Option<Rc<UserFunction>> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<ClassMethodGetBinderOps>(ops) {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<ClassMethodGetBinder>()?;
    Some(Rc::clone(&s.func))
}

/// Returned by `staticmethod.__get__` when the descriptor wraps a
/// `UserFunction`.  Calling this value returns the underlying plain function.
pub struct StaticMethodGetBinder {
    pub func: Rc<UserFunction>,
}

pub struct StaticMethodGetBinderOps;
pub const STATIC_METHOD_GET_BINDER_OPS: &StaticMethodGetBinderOps = &StaticMethodGetBinderOps;
pub const STATIC_BINDER_TYPE_NAME: &str = "staticmethod_get_binder";

impl BuiltinTypeOps for StaticMethodGetBinderOps {
    fn type_name(&self) -> &'static str {
        STATIC_BINDER_TYPE_NAME
    }

    fn repr(&self, _state: &BuiltinState) -> String {
        "<staticmethod.__get__ binder>".to_string()
    }
}

/// Construct a `staticmethod.__get__` binder wrapping a `UserFunction`.
pub fn static_method_get_binder(func: Rc<UserFunction>) -> Value {
    let state: Box<dyn Any> = Box::new(StaticMethodGetBinder { func });
    Value::builtin_object(STATIC_METHOD_GET_BINDER_OPS, state)
}

/// Extract the `Rc<UserFunction>` from a `StaticMethodGetBinder` value, or
/// return `None` if the value is not one.
pub fn as_static_method_get_binder(value: &Value) -> Option<Rc<UserFunction>> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<StaticMethodGetBinderOps>(ops) {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<StaticMethodGetBinder>()?;
    Some(Rc::clone(&s.func))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner_class() -> Rc<RefCell<PyClass>> {
        Rc::new(RefCell::new(PyClass::new(
            "Owner",
            "Owner",
            None,
            IndexMap::new(),
        )))
    }

    #[test]
    fn native_cache_plan_admits_only_bytecode_free_builtin_functions() {
        let owner = owner_class();
        let builtin = Value::builtin_function("__native_cache_plan_test");
        let descriptor = native_class_method_descriptor(builtin, &owner, "native_cache_plan_test");
        let plan = native_class_method_cache_plan(&descriptor, &owner)
            .expect("registry builtin should produce a native cache plan");
        let (wrapped, receiver) = cached_native_class_method_call(&plan, &owner)
            .expect("validated plan must match its owner");
        assert!(matches!(
            wrapped.kind(),
            ValueKind::BuiltinFunction("__native_cache_plan_test")
        ));
        assert!(
            wrapped
                .as_function_rc()
                .is_some_and(|function| function.precompiled_code.is_none()),
            "strong native plans must never retain Python bytecode"
        );
        assert!(matches!(receiver.kind(), ValueKind::PyClass(class) if Rc::ptr_eq(class, &owner)));

        let template = Value::fresh_builtin_function("__native_cache_plan_regular_test");
        let regular = Value::with_function_kind(
            Rc::clone(template.as_function_rc().expect("builtin function backing")),
            pyrust_core::UserFunctionKind::Regular,
        );
        let descriptor =
            native_class_method_descriptor(regular, &owner, "native_cache_plan_regular_test");
        assert!(
            native_class_method_cache_plan(&descriptor, &owner).is_none(),
            "regular UserFunction values may own FnCode and must stay out of the strong plan"
        );
    }
}
