//! `collections.defaultdict` — dict subclass with a factory-backed
//! missing-key path.
//!
//! CPython's `defaultdict[key]` semantics: if `key` is absent, call
//! `default_factory()` with no args, store the result under `key`, and
//! return it.  We replicate that by overriding two `BuiltinTypeOps`
//! methods:
//!
//! - `get_item` returns a `KeyError` for missing keys (just like a plain
//!   dict would).
//! - `missing_factory` exposes the stored factory `Value`.  The
//!   interpreter calls it via the standard call dispatch when `get_item`
//!   reports `KeyError`, then re-enters `set_item` to store the
//!   freshly-built default.
//!
//! That split keeps `pyrust-core` ignorant of the `Interpreter` type
//! while still letting `defaultdict` evaluate arbitrary callables for
//! its defaults.
//!
//! ## Aliased-mutation caveat
//!
//! pyrust's `Value` deep-copies mutable containers on read (cloning a
//! list Value materialises a new pool slot), so the
//! `d[k].append(...)` chained-mutation pattern doesn't propagate back
//! into the stored entry — but this is a *language-wide* property,
//! not a defaultdict-specific bug.  The same gap applies to plain
//! dicts:
//!
//! ```text
//! d = {'k': [1, 2]}
//! d['k'].append(3)
//! d['k']  # → [1, 2], not [1, 2, 3] — pyrust deep-copies on read
//! ```
//!
//! Patterns that re-bind (`d[k] = d[k] + [x]`, `d[k] += 1`) work fine
//! because the assignment routes back through `set_item`.  Promoting
//! mutable containers to reference semantics is tracked separately.
//!
//! Reference: <https://docs.python.org/3/library/collections.html#collections.defaultdict>

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::{BuiltinState, BuiltinTypeOps, PyError, PyKey, Result, Value, ValueKind};

pub struct DefaultDictState {
    pub factory: RefCell<Value>,
    pub items: Rc<RefCell<IndexMap<PyKey, Value>>>,
}

pub struct DefaultDictOps;

pub const DEFAULTDICT_OPS: &DefaultDictOps = &DefaultDictOps;
pub const TYPE_NAME: &str = "collections.defaultdict";

pub const METHODS: &[&str] = &["copy", "get", "items", "keys", "values"];

impl BuiltinTypeOps for DefaultDictOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let s = match borrow_state(state) {
            Some(s) => s,
            None => return "defaultdict(<bad state>)".to_string(),
        };
        let items = s.items.borrow();
        let factory_repr = s.factory().repr();
        let body: Vec<String> = items
            .iter()
            .map(|(k, v)| format!("{}: {}", key_repr(k), v.repr()))
            .collect();
        format!("defaultdict({factory_repr}, {{{}}})", body.join(", "))
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        borrow_state(state).is_some_and(|s| !s.items.borrow().is_empty())
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        let s = match borrow_state(state) {
            Some(s) => s,
            None => return false,
        };
        let lhs = s.items.borrow();
        // CPython: defaultdict == dict compares only items (factory not
        // part of equality).
        match other.kind() {
            ValueKind::Dict(rhs) => *lhs == *rhs,
            ValueKind::BuiltinObject {
                ops,
                state: other_state,
            } if ops.type_name() == TYPE_NAME => {
                let other_s = match borrow_state(other_state) {
                    Some(s) => s,
                    None => return false,
                };
                *lhs == *other_s.items.borrow()
            }
            _ => false,
        }
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        borrow_state(state).map(|s| s.items.borrow().len())
    }

    fn get_item(&self, state: &BuiltinState, key: &Value) -> Result<Value> {
        let s = borrow_state(state)
            .ok_or_else(|| PyError::Runtime("internal: bad defaultdict state".to_string()))?;
        let pk = key
            .to_key()
            .ok_or_else(|| PyError::named("TypeError", "unhashable type".to_string()))?;
        match s.items.borrow().get(&pk) {
            Some(v) => Ok(v.clone()),
            None => {
                // Signal "missing" to the interpreter, which then consults
                // `missing_factory()` and calls the factory via its normal
                // call dispatch.  Keeping this as a plain `KeyError`
                // means `defaultdict` without a factory (`defaultdict()`)
                // behaves exactly like a plain dict on missing keys, also
                // matching CPython.
                Err(PyError::named("KeyError", key.repr()))
            }
        }
    }

    fn set_item(&self, state: &BuiltinState, key: &Value, value: Value) -> Result<()> {
        let s = borrow_state(state)
            .ok_or_else(|| PyError::Runtime("internal: bad defaultdict state".to_string()))?;
        let pk = key
            .to_key()
            .ok_or_else(|| PyError::named("TypeError", "unhashable type".to_string()))?;
        s.items.borrow_mut().insert(pk, value);
        Ok(())
    }

    fn missing_factory(&self, state: &BuiltinState) -> Option<Value> {
        let s = borrow_state(state)?;
        // `defaultdict(None)` means "no factory" — fall through to
        // KeyError, matching CPython.
        if s.factory().is_none() {
            None
        } else {
            Some(s.factory.clone())
        }
    }

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        let s = borrow_state(state)
            .ok_or_else(|| PyError::Runtime("internal: bad defaultdict state".to_string()))?;
        let pk = item
            .to_key()
            .ok_or_else(|| PyError::named("TypeError", "unhashable type".to_string()))?;
        Ok(s.items.borrow().contains_key(&pk))
    }

    fn has_method(&self, name: &str) -> bool {
        METHODS.contains(&name)
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        method: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        let s = borrow_state(state)
            .ok_or_else(|| PyError::Runtime("internal: bad defaultdict state".to_string()))?;
        match method {
            "copy" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        "copy() takes no arguments".to_string(),
                    ));
                }
                let items = s.items.borrow().clone();
                Ok(defaultdict(s.factory.clone(), items))
            }
            "get" => {
                if args.is_empty() || args.len() > 2 {
                    return Err(PyError::named(
                        "TypeError",
                        "get() takes 1 or 2 arguments".to_string(),
                    ));
                }
                let key = args[0]
                    .to_key()
                    .ok_or_else(|| PyError::named("TypeError", "unhashable type".to_string()))?;
                match s.items.borrow().get(&key) {
                    Some(v) => Ok(v.clone()),
                    None => Ok(args.get(1).cloned().unwrap_or_else(Value::none)),
                }
            }
            "keys" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        "keys() takes no arguments".to_string(),
                    ));
                }
                let out: Vec<Value> = s.items.borrow().keys().cloned().map(key_to_value).collect();
                Ok(Value::list(out))
            }
            "values" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        "values() takes no arguments".to_string(),
                    ));
                }
                Ok(Value::list(s.items.borrow().values().cloned().collect()))
            }
            "items" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        "items() takes no arguments".to_string(),
                    ));
                }
                let out: Vec<Value> = s
                    .items
                    .borrow()
                    .iter()
                    .map(|(k, v)| Value::tuple(vec![key_to_value(k.clone()), v.clone()]))
                    .collect();
                Ok(Value::list(out))
            }
            _ => Err(PyError::named(
                "AttributeError",
                format!("'defaultdict' object has no attribute '{method}'"),
            )),
        }
    }
}

/// Construct a defaultdict Value.
pub fn defaultdict(factory: Value, items: IndexMap<PyKey, Value>) -> Value {
    let state: Box<dyn Any> = Box::new(DefaultDictState {
        factory: RefCell::new(factory),
        items: Rc::new(RefCell::new(items)),
    });
    Value::builtin_object(DEFAULTDICT_OPS, state)
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Hand the caller a cheap-to-pass view of `DefaultDictState`:
/// - `factory`: a clone of the current `Value` (NaN-boxed, 8 bytes).
/// - `items`: `Rc<RefCell<...>>` handle so mutations persist.
///
/// Returning by value lets callers drop the borrow over `state` before
/// they touch the items map, avoiding nested-RefCell-borrow surprises.
struct DefaultDictView {
    factory: Value,
    items: Rc<RefCell<IndexMap<PyKey, Value>>>,
}

impl DefaultDictView {
    fn factory(&self) -> &Value {
        &self.factory
    }
}

fn borrow_state(state: &BuiltinState) -> Option<DefaultDictView> {
    let borrow = state.borrow();
    let s = borrow.downcast_ref::<DefaultDictState>()?;
    Some(DefaultDictView {
        factory: s.factory.borrow().clone(),
        items: Rc::clone(&s.items),
    })
}

fn key_to_value(key: PyKey) -> Value {
    match key {
        PyKey::Int(v) => Value::int(v),
        PyKey::Float(bits) => Value::float(f64::from_bits(bits)),
        PyKey::Str(s) => Value::string(s),
        PyKey::Bool(b) => Value::bool_(b),
        PyKey::None => Value::none(),
        PyKey::FrozenSet(items) => crate::frozenset::frozenset(items.into_iter().collect()),
    }
}

fn key_repr(key: &PyKey) -> String {
    match key {
        PyKey::Int(v) => v.to_string(),
        PyKey::Float(bits) => f64::from_bits(*bits).to_string(),
        PyKey::Str(s) => format!("'{s}'"),
        PyKey::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        PyKey::None => "None".to_string(),
        PyKey::FrozenSet(_) => "frozenset(...)".to_string(),
    }
}
