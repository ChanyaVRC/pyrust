//! `@property` descriptor.
//!
//! Eliminated from `pyrust-core`'s Tier 1 (#294): three-callable descriptor
//! with no compile-time-specializable payload.  Lives here as a
//! `BuiltinObject` whose ops dispatch the descriptor-protocol calls.

use std::any::Any;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::{BuiltinState, BuiltinTypeOps, Result, Value, ValueKind, builtin_ops_is};

/// Property descriptor state.  Each slot is either a callable `Value` or
/// `Value::none()` (meaning "not set").
pub struct PropertyState {
    pub fget: Rc<Value>,
    pub fset: Rc<Value>,
    pub fdel: Rc<Value>,
    /// If `Some(slot)`, this descriptor is the intermediate result of
    /// `prop.setter` / `prop.deleter` / `prop.getter`: calling it with a
    /// function returns a new Property with that slot replaced.
    /// `slot` is 0=fget, 1=fset, 2=fdel.
    pub partial_slot: Option<u8>,
    /// Attribute name recorded via `__set_name__` when the property is bound in
    /// a class body (issue #1846). CPython stores the object without requiring
    /// it to be a string, so direct calls such as `p.__set_name__(C, 1)` are
    /// valid and later accessor errors render that object with `repr`.
    ///
    /// `None` is reserved for properties that have never received
    /// `__set_name__`; their errors use the unnamed `property of '...'` form.
    pub name: Option<Value>,
    /// Explicit `doc=` argument passed to `property(...)`.  `None` means no
    /// explicit doc was given, in which case `property.__doc__` falls back to
    /// the getter's docstring (issue #1961).  Stored as a `Value` so any object
    /// can be supplied as the doc, matching CPython.
    pub doc: Option<Value>,
}

pub struct PropertyOps;
pub const PROPERTY_OPS: &PropertyOps = &PropertyOps;
pub const TYPE_NAME: &str = "property";

impl BuiltinTypeOps for PropertyOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        if borrow_state(state).is_some_and(|s| s.partial_slot.is_some()) {
            "<property accessor partial>".to_string()
        } else {
            "<property object>".to_string()
        }
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn has_method(&self, name: &str) -> bool {
        name == "__set_name__"
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        if name != "__set_name__" {
            return Err(pyrust_core::PyError::attribute_error(
                format!("'property' object has no attribute '{name}'"),
                Some(name.to_string()),
                None,
            ));
        }
        if !kwargs.is_empty() {
            return Err(pyrust_core::type_err!(
                "property.__set_name__() takes no keyword arguments"
            ));
        }
        if args.len() != 2 {
            return Err(pyrust_core::type_err!(
                "property.__set_name__() takes exactly 2 arguments ({} given)",
                args.len()
            ));
        }
        let mut borrow = state.borrow_mut();
        let property = borrow
            .downcast_mut::<PropertyState>()
            .ok_or_else(|| pyrust_core::PyError::Runtime("invalid property state".to_string()))?;
        property.name = Some(args[1].clone());
        Ok(Value::none())
    }
}

/// Construct a new `@property` value.  Pass `Value::none()` for any
/// accessor that is not set.  `__doc__` falls back to the getter's docstring.
pub fn property(fget: Value, fset: Value, fdel: Value) -> Value {
    property_with_doc(fget, fset, fdel, None)
}

/// Construct a `@property` with an explicit `doc=` value (or `None` to fall
/// back to the getter's docstring).  See [`property`].
pub fn property_with_doc(fget: Value, fset: Value, fdel: Value, doc: Option<Value>) -> Value {
    let state: Box<dyn Any> = Box::new(PropertyState {
        fget: Rc::new(fget),
        fset: Rc::new(fset),
        fdel: Rc::new(fdel),
        partial_slot: None,
        name: None,
        doc,
    });
    Value::builtin_object(PROPERTY_OPS, state)
}

/// Returned by `prop.setter(fn)` — creates a new Property with fset replaced.
pub fn property_setter_partial(fget: Value, fdel: Value) -> Value {
    accessor_partial(1, fget, Value::none(), fdel)
}

/// Returned by `prop.deleter(fn)` — creates a new Property with fdel replaced.
pub fn property_deleter_partial(fget: Value, fset: Value) -> Value {
    accessor_partial(2, fget, fset, Value::none())
}

/// Returned by `prop.getter(fn)` — creates a new Property with fget replaced.
pub fn property_getter_partial(fset: Value, fdel: Value) -> Value {
    accessor_partial(0, Value::none(), fset, fdel)
}

fn accessor_partial(slot: u8, fget: Value, fset: Value, fdel: Value) -> Value {
    let state: Box<dyn Any> = Box::new(PropertyState {
        fget: Rc::new(fget),
        fset: Rc::new(fset),
        fdel: Rc::new(fdel),
        partial_slot: Some(slot),
        name: None,
        doc: None,
    });
    Value::builtin_object(PROPERTY_OPS, state)
}

/// Property slots returned by [`as_property`]: `(fget, fset, fdel, partial_slot)`.
pub type PropertyFields = (Rc<Value>, Rc<Value>, Rc<Value>, Option<u8>);

/// Extract the property fields (fget, fset, fdel, partial_slot) from a value.
/// Returns None if the value isn't a property.
///
/// Note: clones all three `Rc<Value>` slots. Prefer [`with_property`] when
/// only a subset of slots is needed — see #304.
pub fn as_property(value: &Value) -> Option<PropertyFields> {
    with_property(value, |s| {
        (
            Rc::clone(&s.fget),
            Rc::clone(&s.fset),
            Rc::clone(&s.fdel),
            s.partial_slot,
        )
    })
}

/// Run `f` with a borrow of the underlying [`PropertyState`].  Returns
/// `Some(f(state))` if `value` is a `property`, or `None` otherwise.
///
/// Callers should clone only the slots they need inside the closure — this
/// avoids the unconditional 3 × `Rc::clone` cost of [`as_property`].
pub fn with_property<R>(value: &Value, f: impl FnOnce(&PropertyState) -> R) -> Option<R> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<PropertyOps>(ops) {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<PropertyState>()?;
    Some(f(s))
}

/// Compatibility helper for the former class-construction shortcut.
///
/// New code should invoke the descriptor's `__set_name__` protocol so custom
/// descriptors receive the same treatment. This wrapper preserves the old
/// string-only helper for downstream `pyrust-builtins` callers.
#[deprecated(
    since = "0.1.0",
    note = "invoke the descriptor __set_name__ protocol instead"
)]
pub fn set_property_name(value: &Value, name: &str) {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return;
    };
    if !builtin_ops_is::<PropertyOps>(ops) {
        return;
    }
    if let Some(property) = state.borrow_mut().downcast_mut::<PropertyState>() {
        property.name = Some(Value::string(name));
    }
}

/// Return `Some(partial_slot)` if `value` is a `property`, or `None`.
///
/// Cheaper than [`as_property`] when only the partial-slot flag is needed:
/// no `Rc::clone` calls on the accessor slots.
pub fn property_partial_slot(value: &Value) -> Option<Option<u8>> {
    with_property(value, |s| s.partial_slot)
}

fn borrow_state(state: &BuiltinState) -> Option<std::cell::Ref<'_, PropertyState>> {
    std::cell::Ref::filter_map(state.borrow(), |b| b.downcast_ref::<PropertyState>()).ok()
}

/// Which descriptor-protocol dunder a bound [`property`] method targets.
/// Slot ordering matches elsewhere: 0=`__get__`, 1=`__set__`, 2=`__delete__`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PropertyMethodKind {
    Get,
    Set,
    Delete,
}

impl PropertyMethodKind {
    fn dunder(self) -> &'static str {
        match self {
            PropertyMethodKind::Get => "__get__",
            PropertyMethodKind::Set => "__set__",
            PropertyMethodKind::Delete => "__delete__",
        }
    }

    /// Accessor label used by CPython's missing-accessor error messages.
    pub const fn accessor_name(self) -> &'static str {
        match self {
            PropertyMethodKind::Get => "getter",
            PropertyMethodKind::Set => "setter",
            PropertyMethodKind::Delete => "deleter",
        }
    }
}

/// State for a property descriptor dunder accessed as a value, e.g.
/// `f = p.__get__; f(obj, owner)` or `hasattr(p, "__get__")`.  Holds the
/// originating property and which dunder is bound.
pub struct PropertyMethodState {
    pub prop: Value,
    pub kind: PropertyMethodKind,
}

pub struct PropertyMethodOps;
pub const PROPERTY_METHOD_OPS: &PropertyMethodOps = &PropertyMethodOps;
pub const METHOD_TYPE_NAME: &str = "property-method";

impl BuiltinTypeOps for PropertyMethodOps {
    fn type_name(&self) -> &'static str {
        METHOD_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let name =
            std::cell::Ref::filter_map(state.borrow(), |b| b.downcast_ref::<PropertyMethodState>())
                .ok()
                .map(|s| s.kind.dunder())
                .unwrap_or("__get__");
        format!("<method-wrapper '{name}' of property object>")
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }
}

/// Construct a bound descriptor-method value for `prop`.
pub fn property_method(prop: Value, kind: PropertyMethodKind) -> Value {
    let state: Box<dyn Any> = Box::new(PropertyMethodState { prop, kind });
    Value::builtin_object(PROPERTY_METHOD_OPS, state)
}

/// Return `(prop, kind)` if `value` is a bound property descriptor-method.
pub fn as_property_method(value: &Value) -> Option<(Value, PropertyMethodKind)> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<PropertyMethodOps>(ops) {
        return None;
    }
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<PropertyMethodState>()?;
    Some((s.prop.clone(), s.kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_property_extracts_fget_only() {
        let getter = Value::builtin_function("len");
        let prop = property(getter.clone(), Value::none(), Value::none());
        let fget = with_property(&prop, |s| Rc::clone(&s.fget)).expect("is property");
        // fget should be exactly the getter we put in.  Both refer to the
        // same interned built-in function via `Value::builtin_function`'s
        // thread-local cache, so a content-eq is sufficient (and would also
        // catch a slot mix-up where fget came back as Value::none()).
        assert_eq!(*fget, getter);
        // The slots we didn't touch should still be `none`.
        let (fset_is_none, fdel_is_none) =
            with_property(&prop, |s| (s.fset.is_none(), s.fdel.is_none())).expect("is property");
        assert!(fset_is_none, "fset should remain none for getter-only");
        assert!(fdel_is_none, "fdel should remain none for getter-only");
        assert_eq!(
            with_property(&prop, |s| s.partial_slot),
            Some(None),
            "plain property has no partial_slot",
        );
    }

    #[test]
    fn with_property_returns_none_for_non_property() {
        let not_a_property = Value::int(42);
        assert!(with_property(&not_a_property, |_| ()).is_none());
        assert!(property_partial_slot(&not_a_property).is_none());
    }

    #[test]
    fn property_partial_slot_distinguishes_kinds() {
        let plain = property(Value::none(), Value::none(), Value::none());
        assert_eq!(property_partial_slot(&plain), Some(None));

        let setter_partial = property_setter_partial(Value::none(), Value::none());
        assert_eq!(property_partial_slot(&setter_partial), Some(Some(1)));

        let getter_partial = property_getter_partial(Value::none(), Value::none());
        assert_eq!(property_partial_slot(&getter_partial), Some(Some(0)));

        let deleter_partial = property_deleter_partial(Value::none(), Value::none());
        assert_eq!(property_partial_slot(&deleter_partial), Some(Some(2)));
    }

    #[test]
    fn as_property_still_returns_all_four() {
        let getter = Value::builtin_function("len");
        let setter = Value::builtin_function("id");
        let prop = property(getter, setter, Value::none());
        let (fget, fset, fdel, slot) = as_property(&prop).expect("is property");
        assert!(!fget.is_none());
        assert!(!fset.is_none());
        assert!(fdel.is_none());
        assert_eq!(slot, None);
    }
}
