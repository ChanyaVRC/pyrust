//! The concrete mixin methods of `collections.abc`, transcribed from CPython
//! 3.12's `Lib/_collections_abc.py`.
//!
//! Every method dispatches through the receiver exactly where the Python
//! source does (`self[key]`, `self.discard(value)`, `self._from_iterable(it)`,
//! `len(self)`, …), so subclasses that override a primitive see every derived
//! operation follow suit. Argument binding reproduces the `def` signatures'
//! `TypeError`s.

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, hash_value_with_interp};
use crate::value::Value;

use super::definitions::{Abc, canonical};
use super::support::{
    GenBody, LazyIter, binop, call, call_method, compare, contains, delitem, generator, getitem,
    is_exc, is_or_eq, isinstance_abc, iter, len, next, raise_from_none, raise_new, setitem,
    try_except, try_except_named, unpack_pair,
};

const fn sig(qualname: &'static str, params: &'static [&'static str], defaults: usize) -> Sig {
    Sig {
        qualname,
        params,
        defaults,
        kwonly: &[],
        var_kw: false,
    }
}

/// Bind `args` to `sig` and return the positional-or-keyword slots in order
/// (`None` where a default applies).
fn bind(sig: &Sig, call_args: &[ExpandedCallArg]) -> Result<Vec<Option<Value>>> {
    let mut bound = args::bind(sig, call_args)?;
    Ok((0..sig.params.len()).map(|i| bound.take(i)).collect())
}

/// Bind a method taking only `self`.
fn bind_self(sig: &Sig, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let mut slots = bind(sig, call_args)?;
    Ok(slots[0].take().unwrap_or_else(Value::none))
}

/// Bind a method taking `self` and one required argument.
fn bind_self_arg(sig: &Sig, call_args: &[ExpandedCallArg]) -> Result<(Value, Value)> {
    let mut slots = bind(sig, call_args)?;
    Ok((
        slots[0].take().unwrap_or_else(Value::none),
        slots[1].take().unwrap_or_else(Value::none),
    ))
}

fn not_implemented() -> Result<Value> {
    Ok(Value::not_implemented())
}

// ── Iterator / Generator / Coroutine ─────────────────────────────────────────

/// `Iterator.__iter__` / `AsyncIterator.__aiter__`: `return self`.
pub(super) fn return_self(qualname: &'static str, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let params: &'static [&'static str] = &["self"];
    bind_self(&sig(qualname, params, 0), call_args)
}

/// `Generator.__next__`: `return self.send(None)`.
pub(super) fn generator_next(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let this = bind_self(&sig("Generator.__next__", &["self"], 0), call_args)?;
    call_method(interp, &this, "send", &[Value::none()])
}

/// `Generator.close` / `Coroutine.close`.
pub(super) fn close(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
    qualname: &'static str,
    kind: &str,
) -> Result<Value> {
    let this = bind_self(&sig(qualname, &["self"], 0), call_args)?;
    let generator_exit = super::support::exc_class("GeneratorExit");
    let thrown = try_except(
        |error| is_exc(error, "GeneratorExit") || is_exc(error, "StopIteration"),
        || call_method(interp, &this, "throw", &[generator_exit]),
    )?;
    match thrown {
        Err(_) => Ok(Value::none()),
        Ok(_) => Err(PyError::named(
            "RuntimeError",
            format!("{kind} ignored GeneratorExit"),
        )),
    }
}

// ── Set ──────────────────────────────────────────────────────────────────────

/// `Set.__le__`.
pub(super) fn set_le(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, other) = bind_self_arg(&sig("Set.__le__", &["self", "other"], 0), call_args)?;
    if !isinstance_abc(interp, &other, Abc::Set)? {
        return not_implemented();
    }
    Ok(Value::bool_(set_is_subset(interp, &this, &other)?))
}

fn set_is_subset(interp: &mut Interpreter, this: &Value, other: &Value) -> Result<bool> {
    if len(interp, this)? > len(interp, other)? {
        return Ok(false);
    }
    all_contained(interp, this, other)
}

/// `for elem in source: if elem not in container: return False`.
fn all_contained(interp: &mut Interpreter, source: &Value, container: &Value) -> Result<bool> {
    let iterator = iter(interp, source)?;
    while let Some(elem) = next(interp, &iterator)? {
        if !contains(interp, container, &elem)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `Set.__lt__`: `len(self) < len(other) and self.__le__(other)`.
pub(super) fn set_lt(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, other) = bind_self_arg(&sig("Set.__lt__", &["self", "other"], 0), call_args)?;
    if !isinstance_abc(interp, &other, Abc::Set)? {
        return not_implemented();
    }
    if len(interp, &this)? >= len(interp, &other)? {
        return Ok(Value::bool_(false));
    }
    call_method(interp, &this, "__le__", &[other])
}

/// `Set.__gt__`: `len(self) > len(other) and self.__ge__(other)`.
pub(super) fn set_gt(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, other) = bind_self_arg(&sig("Set.__gt__", &["self", "other"], 0), call_args)?;
    if !isinstance_abc(interp, &other, Abc::Set)? {
        return not_implemented();
    }
    if len(interp, &this)? <= len(interp, &other)? {
        return Ok(Value::bool_(false));
    }
    call_method(interp, &this, "__ge__", &[other])
}

/// `Set.__ge__`.
pub(super) fn set_ge(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, other) = bind_self_arg(&sig("Set.__ge__", &["self", "other"], 0), call_args)?;
    if !isinstance_abc(interp, &other, Abc::Set)? {
        return not_implemented();
    }
    if len(interp, &this)? < len(interp, &other)? {
        return Ok(Value::bool_(false));
    }
    Ok(Value::bool_(all_contained(interp, &other, &this)?))
}

/// `Set.__eq__`: `len(self) == len(other) and self.__le__(other)`.
pub(super) fn set_eq(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, other) = bind_self_arg(&sig("Set.__eq__", &["self", "other"], 0), call_args)?;
    if !isinstance_abc(interp, &other, Abc::Set)? {
        return not_implemented();
    }
    if len(interp, &this)? != len(interp, &other)? {
        return Ok(Value::bool_(false));
    }
    call_method(interp, &this, "__le__", &[other])
}

/// `Set._from_iterable` (classmethod): `return cls(it)`.
pub(super) fn set_from_iterable(
    interp: &mut Interpreter,
    cls: &Value,
    it: &Value,
) -> Result<Value> {
    call(interp, cls.clone(), std::slice::from_ref(it))
}

/// `KeysView._from_iterable` / `ItemsView._from_iterable`: `return set(it)`.
pub(super) fn view_from_iterable(interp: &mut Interpreter, it: &Value) -> Result<Value> {
    call(
        interp,
        Value::builtin_function("set"),
        std::slice::from_ref(it),
    )
}

/// `(value for value in <iterator> if (value in container) == keep)`.
struct MembershipFilter {
    source: LazyIter,
    container: Value,
    keep_members: bool,
}

impl GenBody for MembershipFilter {
    fn step(&mut self, interp: &mut Interpreter) -> Result<Option<Value>> {
        while let Some(value) = self.source.next(interp)? {
            if contains(interp, &self.container, &value)? == self.keep_members {
                return Ok(Some(value));
            }
        }
        Ok(None)
    }
}

/// A generator expression filtering `source` by membership in `container`.
/// Like CPython, the outermost iterable is evaluated immediately.
fn membership_filter(
    interp: &mut Interpreter,
    source: &Value,
    container: &Value,
    keep_members: bool,
) -> Result<Value> {
    let iterator = iter(interp, source)?;
    Ok(generator(MembershipFilter {
        source: LazyIter::started(iterator),
        container: container.clone(),
        keep_members,
    }))
}

/// `Set.__and__` / `Set.__rand__`.
pub(super) fn set_and(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
    qualname: &'static str,
) -> Result<Value> {
    let (this, other) = bind_self_arg(&sig(qualname, &["self", "other"], 0), call_args)?;
    if !isinstance_abc(interp, &other, Abc::Iterable)? {
        return not_implemented();
    }
    let filtered = membership_filter(interp, &other, &this, true)?;
    call_method(interp, &this, "_from_iterable", &[filtered])
}

/// `Set.isdisjoint`.
pub(super) fn set_isdisjoint(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, other) = bind_self_arg(&sig("Set.isdisjoint", &["self", "other"], 0), call_args)?;
    let iterator = iter(interp, &other)?;
    while let Some(value) = next(interp, &iterator)? {
        if contains(interp, &this, &value)? {
            return Ok(Value::bool_(false));
        }
    }
    Ok(Value::bool_(true))
}

/// `(e for s in (first, second) for e in s)`.
struct Chain {
    pending: Vec<Value>,
    current: Option<LazyIter>,
}

impl GenBody for Chain {
    fn step(&mut self, interp: &mut Interpreter) -> Result<Option<Value>> {
        loop {
            if let Some(current) = &mut self.current
                && let Some(value) = current.next(interp)?
            {
                return Ok(Some(value));
            }
            if self.pending.is_empty() {
                return Ok(None);
            }
            self.current = Some(LazyIter::new(self.pending.remove(0)));
        }
    }
}

/// `Set.__or__` / `Set.__ror__`.
pub(super) fn set_or(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
    qualname: &'static str,
) -> Result<Value> {
    let (this, other) = bind_self_arg(&sig(qualname, &["self", "other"], 0), call_args)?;
    if !isinstance_abc(interp, &other, Abc::Iterable)? {
        return not_implemented();
    }
    let chain = generator(Chain {
        pending: vec![this.clone(), other],
        current: None,
    });
    call_method(interp, &this, "_from_iterable", &[chain])
}

/// The `other` operand of `__sub__` / `__rsub__` / `__xor__`: a `Set` as-is,
/// any other iterable through `self._from_iterable`, else `None`
/// (`NotImplemented`).
fn coerce_set_operand(
    interp: &mut Interpreter,
    this: &Value,
    other: Value,
) -> Result<Option<Value>> {
    if isinstance_abc(interp, &other, Abc::Set)? {
        return Ok(Some(other));
    }
    if !isinstance_abc(interp, &other, Abc::Iterable)? {
        return Ok(None);
    }
    call_method(interp, this, "_from_iterable", &[other]).map(Some)
}

/// `Set.__sub__`.
pub(super) fn set_sub(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, other) = bind_self_arg(&sig("Set.__sub__", &["self", "other"], 0), call_args)?;
    let Some(other) = coerce_set_operand(interp, &this, other)? else {
        return not_implemented();
    };
    let filtered = membership_filter(interp, &this, &other, false)?;
    call_method(interp, &this, "_from_iterable", &[filtered])
}

/// `Set.__rsub__`.
pub(super) fn set_rsub(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, other) = bind_self_arg(&sig("Set.__rsub__", &["self", "other"], 0), call_args)?;
    let Some(other) = coerce_set_operand(interp, &this, other)? else {
        return not_implemented();
    };
    let filtered = membership_filter(interp, &other, &this, false)?;
    call_method(interp, &this, "_from_iterable", &[filtered])
}

/// `Set.__xor__` / `Set.__rxor__`: `(self - other) | (other - self)`.
pub(super) fn set_xor(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
    qualname: &'static str,
) -> Result<Value> {
    let (this, other) = bind_self_arg(&sig(qualname, &["self", "other"], 0), call_args)?;
    let Some(other) = coerce_set_operand(interp, &this, other)? else {
        return not_implemented();
    };
    let left = binop(interp, &this, BinaryOp::Sub, &other)?;
    let right = binop(interp, &other, BinaryOp::Sub, &this)?;
    binop(interp, &left, BinaryOp::BitOr, &right)
}

/// `Set._hash`: the `frozenset` hash algorithm over the set's elements.
pub(super) fn set_hash(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let this = bind_self(&sig("Set._hash", &["self"], 0), call_args)?;
    // `MAX = sys.maxsize; MASK = 2 * MAX + 1`: every step reduces modulo
    // 2**64, so wrapping `u64` arithmetic is exact.
    let n = len(interp, &this)? as u64;
    let mut h: u64 = 1_927_868_237_u64.wrapping_mul(n.wrapping_add(1));
    let iterator = iter(interp, &this)?;
    while let Some(x) = next(interp, &iterator)? {
        let hx = hash_value_with_interp(interp, &x)? as u64;
        h ^= (hx ^ (hx << 16) ^ 89_869_747).wrapping_mul(3_644_798_167);
    }
    h ^= (h >> 11) ^ (h >> 25);
    h = h.wrapping_mul(69_069).wrapping_add(907_133_923);
    let mut h = h as i64;
    if h == -1 {
        h = 590_923_713;
    }
    Ok(Value::int(h))
}

// ── MutableSet ───────────────────────────────────────────────────────────────

/// `MutableSet.remove`.
pub(super) fn mutable_set_remove(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, value) = bind_self_arg(&sig("MutableSet.remove", &["self", "value"], 0), call_args)?;
    if !contains(interp, &this, &value)? {
        return Err(raise_new(interp, "KeyError", &[value]));
    }
    call_method(interp, &this, "discard", &[value])?;
    Ok(Value::none())
}

/// `MutableSet.pop`.
pub(super) fn mutable_set_pop(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let this = bind_self(&sig("MutableSet.pop", &["self"], 0), call_args)?;
    mutable_set_pop_value(interp, &this)
}

fn mutable_set_pop_value(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let iterator = iter(interp, this)?;
    let Some(value) = next(interp, &iterator)? else {
        return Err(raise_from_none(interp, "KeyError"));
    };
    call_method(interp, this, "discard", std::slice::from_ref(&value))?;
    Ok(value)
}

/// `MutableSet.clear`: pop until `KeyError`.
pub(super) fn mutable_set_clear(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let this = bind_self(&sig("MutableSet.clear", &["self"], 0), call_args)?;
    while try_except_named("KeyError", || call_method(interp, &this, "pop", &[]))?.is_ok() {}
    Ok(Value::none())
}

/// `MutableSet.__ior__`.
pub(super) fn mutable_set_ior(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, it) = bind_self_arg(&sig("MutableSet.__ior__", &["self", "it"], 0), call_args)?;
    let iterator = iter(interp, &it)?;
    while let Some(value) = next(interp, &iterator)? {
        call_method(interp, &this, "add", &[value])?;
    }
    Ok(this)
}

/// `MutableSet.__iand__`.
pub(super) fn mutable_set_iand(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, it) = bind_self_arg(&sig("MutableSet.__iand__", &["self", "it"], 0), call_args)?;
    let difference = binop(interp, &this, BinaryOp::Sub, &it)?;
    let iterator = iter(interp, &difference)?;
    while let Some(value) = next(interp, &iterator)? {
        call_method(interp, &this, "discard", &[value])?;
    }
    Ok(this)
}

/// `MutableSet.__ixor__`.
pub(super) fn mutable_set_ixor(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, it) = bind_self_arg(&sig("MutableSet.__ixor__", &["self", "it"], 0), call_args)?;
    if it.is_identical_to(&this) {
        call_method(interp, &this, "clear", &[])?;
        return Ok(this);
    }
    let it = if isinstance_abc(interp, &it, Abc::Set)? {
        it
    } else {
        call_method(interp, &this, "_from_iterable", &[it])?
    };
    let iterator = iter(interp, &it)?;
    while let Some(value) = next(interp, &iterator)? {
        if contains(interp, &this, &value)? {
            call_method(interp, &this, "discard", &[value])?;
        } else {
            call_method(interp, &this, "add", &[value])?;
        }
    }
    Ok(this)
}

/// `MutableSet.__isub__`.
pub(super) fn mutable_set_isub(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, it) = bind_self_arg(&sig("MutableSet.__isub__", &["self", "it"], 0), call_args)?;
    if it.is_identical_to(&this) {
        call_method(interp, &this, "clear", &[])?;
        return Ok(this);
    }
    let iterator = iter(interp, &it)?;
    while let Some(value) = next(interp, &iterator)? {
        call_method(interp, &this, "discard", &[value])?;
    }
    Ok(this)
}

// ── Mapping ──────────────────────────────────────────────────────────────────

/// `Mapping.get`.
pub(super) fn mapping_get(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut slots = bind(
        &sig("Mapping.get", &["self", "key", "default"], 1),
        call_args,
    )?;
    let this = slots[0].take().unwrap_or_else(Value::none);
    let key = slots[1].take().unwrap_or_else(Value::none);
    let default = slots[2].take().unwrap_or_else(Value::none);
    Ok(try_except_named("KeyError", || getitem(interp, &this, &key))?.unwrap_or(default))
}

/// `Mapping.__contains__`.
pub(super) fn mapping_contains(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, key) = bind_self_arg(&sig("Mapping.__contains__", &["self", "key"], 0), call_args)?;
    let found = try_except_named("KeyError", || getitem(interp, &this, &key))?;
    Ok(Value::bool_(found.is_ok()))
}

/// `Mapping.keys` / `items` / `values`: `return <View>(self)`.
pub(super) fn mapping_view(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
    qualname: &'static str,
    view: Abc,
) -> Result<Value> {
    let this = bind_self(&sig(qualname, &["self"], 0), call_args)?;
    call(interp, view.value(), &[this])
}

/// `Mapping.__eq__`: `dict(self.items()) == dict(other.items())`.
pub(super) fn mapping_eq(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, other) = bind_self_arg(&sig("Mapping.__eq__", &["self", "other"], 0), call_args)?;
    if !isinstance_abc(interp, &other, Abc::Mapping)? {
        return not_implemented();
    }
    let dict = Value::builtin_function("dict");
    let mine = call_method(interp, &this, "items", &[])?;
    let mine = call(interp, dict.clone(), &[mine])?;
    let theirs = call_method(interp, &other, "items", &[])?;
    let theirs = call(interp, dict, &[theirs])?;
    binop(interp, &mine, BinaryOp::Eq, &theirs)
}

// ── MappingView and the concrete views ───────────────────────────────────────

fn view_mapping(interp: &mut Interpreter, view: &Value) -> Result<Value> {
    interp.get_attr(view, "_mapping")
}

/// `MappingView.__init__`: `self._mapping = mapping`.
pub(super) fn mapping_view_init(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, mapping) = bind_self_arg(
        &sig("MappingView.__init__", &["self", "mapping"], 0),
        call_args,
    )?;
    interp.assign_attr(this, "_mapping", mapping)?;
    Ok(Value::none())
}

/// `MappingView.__len__`.
pub(super) fn mapping_view_len(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let this = bind_self(&sig("MappingView.__len__", &["self"], 0), call_args)?;
    let mapping = view_mapping(interp, &this)?;
    Ok(Value::int(len(interp, &mapping)?))
}

/// `MappingView.__repr__`: `'{0.__class__.__name__}({0._mapping!r})'`.
pub(super) fn mapping_view_repr(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let this = bind_self(&sig("MappingView.__repr__", &["self"], 0), call_args)?;
    let class = interp.get_attr(&this, "__class__")?;
    let name = interp.get_attr(&class, "__name__")?;
    let name = match name.as_str() {
        Some(name) => name.to_string(),
        None => {
            let text = call(interp, Value::builtin_function("str"), &[name])?;
            text.as_str().unwrap_or_default().to_string()
        }
    };
    let mapping = view_mapping(interp, &this)?;
    let mapping = crate::interpreter::render_value_repr(interp, &mapping)?;
    Ok(Value::string(format!("{name}({mapping})")))
}

/// `KeysView.__contains__`: `key in self._mapping`.
pub(super) fn keys_view_contains(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, key) = bind_self_arg(
        &sig("KeysView.__contains__", &["self", "key"], 0),
        call_args,
    )?;
    let mapping = view_mapping(interp, &this)?;
    Ok(Value::bool_(contains(interp, &mapping, &key)?))
}

/// `KeysView.__iter__`: `yield from self._mapping`.
struct KeysIter {
    view: Value,
    keys: Option<LazyIter>,
}

impl GenBody for KeysIter {
    fn step(&mut self, interp: &mut Interpreter) -> Result<Option<Value>> {
        if self.keys.is_none() {
            let mapping = view_mapping(interp, &self.view)?;
            self.keys = Some(LazyIter::new(mapping));
        }
        match &mut self.keys {
            Some(keys) => keys.next(interp),
            None => Ok(None),
        }
    }
}

pub(super) fn keys_view_iter(
    _interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let view = bind_self(&sig("KeysView.__iter__", &["self"], 0), call_args)?;
    Ok(generator(KeysIter { view, keys: None }))
}

/// `ItemsView.__contains__`.
pub(super) fn items_view_contains(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, item) = bind_self_arg(
        &sig("ItemsView.__contains__", &["self", "item"], 0),
        call_args,
    )?;
    let (key, value) = unpack_pair(interp, &item)?;
    let mapping = view_mapping(interp, &this)?;
    match try_except_named("KeyError", || getitem(interp, &mapping, &key))? {
        Ok(found) => Ok(Value::bool_(is_or_eq(interp, &found, &value)?)),
        Err(_) => Ok(Value::bool_(false)),
    }
}

/// `for key in self._mapping: yield (key, self._mapping[key])` (items) or
/// `yield self._mapping[key]` (values).
struct MappingEntries {
    view: Value,
    keys: Option<LazyIter>,
    items: bool,
}

impl GenBody for MappingEntries {
    fn step(&mut self, interp: &mut Interpreter) -> Result<Option<Value>> {
        if self.keys.is_none() {
            let mapping = view_mapping(interp, &self.view)?;
            self.keys = Some(LazyIter::new(mapping));
        }
        let Some(keys) = &mut self.keys else {
            return Ok(None);
        };
        let Some(key) = keys.next(interp)? else {
            return Ok(None);
        };
        let mapping = view_mapping(interp, &self.view)?;
        let value = getitem(interp, &mapping, &key)?;
        Ok(Some(if self.items {
            Value::tuple(vec![key, value])
        } else {
            value
        }))
    }
}

pub(super) fn items_view_iter(
    _interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let view = bind_self(&sig("ItemsView.__iter__", &["self"], 0), call_args)?;
    Ok(generator(MappingEntries {
        view,
        keys: None,
        items: true,
    }))
}

/// `ValuesView.__contains__`.
pub(super) fn values_view_contains(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, value) = bind_self_arg(
        &sig("ValuesView.__contains__", &["self", "value"], 0),
        call_args,
    )?;
    let mapping = view_mapping(interp, &this)?;
    let keys = iter(interp, &mapping)?;
    while let Some(key) = next(interp, &keys)? {
        let mapping = view_mapping(interp, &this)?;
        let found = getitem(interp, &mapping, &key)?;
        if is_or_eq(interp, &found, &value)? {
            return Ok(Value::bool_(true));
        }
    }
    Ok(Value::bool_(false))
}

pub(super) fn values_view_iter(
    _interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let view = bind_self(&sig("ValuesView.__iter__", &["self"], 0), call_args)?;
    Ok(generator(MappingEntries {
        view,
        keys: None,
        items: false,
    }))
}

// ── MutableMapping ───────────────────────────────────────────────────────────

/// `MutableMapping.pop`.
pub(super) fn mutable_mapping_pop(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut slots = bind(
        &sig("MutableMapping.pop", &["self", "key", "default"], 1),
        call_args,
    )?;
    let this = slots[0].take().unwrap_or_else(Value::none);
    let key = slots[1].take().unwrap_or_else(Value::none);
    // `default=__marker`: an explicit marker argument means "no default".
    let marker = canonical().marker.clone();
    let default = slots[2]
        .take()
        .filter(|value| !value.is_identical_to(&marker));
    // Without a default the `KeyError` is re-raised with its traceback.
    let handled = |error: &PyError| default.is_some() && is_exc(error, "KeyError");
    match try_except(handled, || getitem(interp, &this, &key))? {
        Ok(value) => {
            delitem(interp, &this, &key)?;
            Ok(value)
        }
        Err(_) => Ok(default.unwrap_or_else(Value::none)),
    }
}

/// `MutableMapping.popitem`.
pub(super) fn mutable_mapping_popitem(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let this = bind_self(&sig("MutableMapping.popitem", &["self"], 0), call_args)?;
    let keys = iter(interp, &this)?;
    let Some(key) = next(interp, &keys)? else {
        return Err(raise_from_none(interp, "KeyError"));
    };
    let value = getitem(interp, &this, &key)?;
    delitem(interp, &this, &key)?;
    Ok(Value::tuple(vec![key, value]))
}

/// `MutableMapping.clear`: `popitem()` until `KeyError`.
pub(super) fn mutable_mapping_clear(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let this = bind_self(&sig("MutableMapping.clear", &["self"], 0), call_args)?;
    while try_except_named("KeyError", || call_method(interp, &this, "popitem", &[]))?.is_ok() {}
    Ok(Value::none())
}

/// `MutableMapping.update(self, other=(), /, **kwds)`.
pub(super) fn mutable_mapping_update(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    // `self` and `other` are positional-only, so every keyword (even `self=`
    // or `other=`) lands in `**kwds`.
    let positional: Vec<&Value> = call_args
        .iter()
        .filter(|arg| arg.name.is_none())
        .map(|arg| &arg.value)
        .collect();
    let kwds: Vec<(String, Value)> = call_args
        .iter()
        .filter_map(|arg| arg.name.clone().map(|name| (name, arg.value.clone())))
        .collect();
    if positional.len() > 2 {
        return Err(args::type_error(format!(
            "MutableMapping.update() takes from 1 to 2 positional arguments but {} were given",
            positional.len()
        )));
    }
    let Some(this) = positional.first().map(|value| (*value).clone()) else {
        return Err(args::type_error(
            "MutableMapping.update() missing 1 required positional argument: 'self'",
        ));
    };
    let other = positional
        .get(1)
        .map(|value| (*value).clone())
        .unwrap_or_else(|| Value::tuple(Vec::new()));

    if isinstance_abc(interp, &other, Abc::Mapping)? {
        let keys = iter(interp, &other)?;
        while let Some(key) = next(interp, &keys)? {
            let value = getitem(interp, &other, &key)?;
            setitem(interp, &this, &key, &value)?;
        }
    } else if has_attr(interp, &other, "keys")? {
        let key_list = call_method(interp, &other, "keys", &[])?;
        let keys = iter(interp, &key_list)?;
        while let Some(key) = next(interp, &keys)? {
            let value = getitem(interp, &other, &key)?;
            setitem(interp, &this, &key, &value)?;
        }
    } else {
        let items = iter(interp, &other)?;
        while let Some(item) = next(interp, &items)? {
            let (key, value) = unpack_pair(interp, &item)?;
            setitem(interp, &this, &key, &value)?;
        }
    }
    for (key, value) in kwds {
        setitem(interp, &this, &Value::string(key), &value)?;
    }
    Ok(Value::none())
}

/// `hasattr(obj, name)`: only `AttributeError` means "absent".
fn has_attr(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<bool> {
    Ok(try_except_named("AttributeError", || interp.get_attr(obj, name))?.is_ok())
}

/// `MutableMapping.setdefault`.
pub(super) fn mutable_mapping_setdefault(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut slots = bind(
        &sig("MutableMapping.setdefault", &["self", "key", "default"], 1),
        call_args,
    )?;
    let this = slots[0].take().unwrap_or_else(Value::none);
    let key = slots[1].take().unwrap_or_else(Value::none);
    let default = slots[2].take().unwrap_or_else(Value::none);
    match try_except_named("KeyError", || getitem(interp, &this, &key))? {
        Ok(value) => Ok(value),
        Err(_) => {
            setitem(interp, &this, &key, &default)?;
            Ok(default)
        }
    }
}

// ── Sequence ─────────────────────────────────────────────────────────────────

/// `Sequence.__iter__`: `self[0]`, `self[1]`, … until `IndexError`.
struct SequenceIter {
    sequence: Value,
    index: i64,
}

impl GenBody for SequenceIter {
    fn step(&mut self, interp: &mut Interpreter) -> Result<Option<Value>> {
        let item = Value::int(self.index);
        match try_except_named("IndexError", || getitem(interp, &self.sequence, &item))? {
            Ok(value) => {
                self.index += 1;
                Ok(Some(value))
            }
            Err(_) => Ok(None),
        }
    }
}

pub(super) fn sequence_iter(
    _interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let sequence = bind_self(&sig("Sequence.__iter__", &["self"], 0), call_args)?;
    Ok(generator(SequenceIter { sequence, index: 0 }))
}

/// `Sequence.__contains__`.
pub(super) fn sequence_contains(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, value) = bind_self_arg(
        &sig("Sequence.__contains__", &["self", "value"], 0),
        call_args,
    )?;
    let iterator = iter(interp, &this)?;
    while let Some(item) = next(interp, &iterator)? {
        if is_or_eq(interp, &item, &value)? {
            return Ok(Value::bool_(true));
        }
    }
    Ok(Value::bool_(false))
}

/// `Sequence.__reversed__`: `for i in reversed(range(len(self))): yield self[i]`.
struct SequenceReversed {
    sequence: Value,
    /// The next index to yield, once the generator has started.
    next_index: Option<i64>,
}

impl GenBody for SequenceReversed {
    fn step(&mut self, interp: &mut Interpreter) -> Result<Option<Value>> {
        let index = match self.next_index {
            Some(index) => index,
            None => len(interp, &self.sequence)? - 1,
        };
        if index < 0 {
            self.next_index = Some(-1);
            return Ok(None);
        }
        self.next_index = Some(index - 1);
        getitem(interp, &self.sequence, &Value::int(index)).map(Some)
    }
}

pub(super) fn sequence_reversed(
    _interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let sequence = bind_self(&sig("Sequence.__reversed__", &["self"], 0), call_args)?;
    Ok(generator(SequenceReversed {
        sequence,
        next_index: None,
    }))
}

/// `Sequence.index(value, start=0, stop=None)`.
pub(super) fn sequence_index(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut slots = bind(
        &sig("Sequence.index", &["self", "value", "start", "stop"], 2),
        call_args,
    )?;
    let this = slots[0].take().unwrap_or_else(Value::none);
    let value = slots[1].take().unwrap_or_else(Value::none);
    let mut start = slots[2].take().unwrap_or_else(|| Value::int(0));
    let mut stop = slots[3].take().unwrap_or_else(Value::none);
    let zero = Value::int(0);
    if !start.is_none() && compare(interp, &start, BinaryOp::Lt, &zero)? {
        // `start = max(len(self) + start, 0)`.
        let length = Value::int(len(interp, &this)?);
        let shifted = binop(interp, &length, BinaryOp::Add, &start)?;
        start = if compare(interp, &zero, BinaryOp::Gt, &shifted)? {
            zero.clone()
        } else {
            shifted
        };
    }
    if !stop.is_none() && compare(interp, &stop, BinaryOp::Lt, &zero)? {
        let length = Value::int(len(interp, &this)?);
        stop = binop(interp, &stop, BinaryOp::Add, &length)?;
    }
    let one = Value::int(1);
    let mut i = start;
    while stop.is_none() || compare(interp, &i, BinaryOp::Lt, &stop)? {
        let Ok(item) = try_except_named("IndexError", || getitem(interp, &this, &i))? else {
            break;
        };
        if is_or_eq(interp, &item, &value)? {
            return Ok(i);
        }
        i = binop(interp, &i, BinaryOp::Add, &one)?;
    }
    Err(raise_new(interp, "ValueError", &[]))
}

/// `Sequence.count`.
pub(super) fn sequence_count(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, value) = bind_self_arg(&sig("Sequence.count", &["self", "value"], 0), call_args)?;
    let iterator = iter(interp, &this)?;
    let mut count: i64 = 0;
    while let Some(item) = next(interp, &iterator)? {
        if is_or_eq(interp, &item, &value)? {
            count += 1;
        }
    }
    Ok(Value::int(count))
}

// ── MutableSequence ──────────────────────────────────────────────────────────

/// `MutableSequence.append`: `self.insert(len(self), value)`.
pub(super) fn mutable_sequence_append(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, value) = bind_self_arg(
        &sig("MutableSequence.append", &["self", "value"], 0),
        call_args,
    )?;
    let length = Value::int(len(interp, &this)?);
    call_method(interp, &this, "insert", &[length, value])?;
    Ok(Value::none())
}

/// `MutableSequence.clear`: `pop()` until `IndexError`.
pub(super) fn mutable_sequence_clear(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let this = bind_self(&sig("MutableSequence.clear", &["self"], 0), call_args)?;
    while try_except_named("IndexError", || call_method(interp, &this, "pop", &[]))?.is_ok() {}
    Ok(Value::none())
}

/// `MutableSequence.reverse`.
pub(super) fn mutable_sequence_reverse(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let this = bind_self(&sig("MutableSequence.reverse", &["self"], 0), call_args)?;
    let n = len(interp, &this)?;
    for i in 0..n / 2 {
        let low = Value::int(i);
        let high = Value::int(n - i - 1);
        // `self[i], self[n-i-1] = self[n-i-1], self[i]`.
        let from_high = getitem(interp, &this, &high)?;
        let from_low = getitem(interp, &this, &low)?;
        setitem(interp, &this, &low, &from_high)?;
        setitem(interp, &this, &high, &from_low)?;
    }
    Ok(Value::none())
}

/// `MutableSequence.extend`.
pub(super) fn mutable_sequence_extend(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, values) = bind_self_arg(
        &sig("MutableSequence.extend", &["self", "values"], 0),
        call_args,
    )?;
    extend(interp, &this, values)?;
    Ok(Value::none())
}

fn extend(interp: &mut Interpreter, this: &Value, values: Value) -> Result<()> {
    let values = if values.is_identical_to(this) {
        call(interp, Value::builtin_function("list"), &[values])?
    } else {
        values
    };
    let iterator = iter(interp, &values)?;
    while let Some(value) = next(interp, &iterator)? {
        call_method(interp, this, "append", &[value])?;
    }
    Ok(())
}

/// `MutableSequence.pop(index=-1)`.
pub(super) fn mutable_sequence_pop(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut slots = bind(
        &sig("MutableSequence.pop", &["self", "index"], 1),
        call_args,
    )?;
    let this = slots[0].take().unwrap_or_else(Value::none);
    let index = slots[1].take().unwrap_or_else(|| Value::int(-1));
    let value = getitem(interp, &this, &index)?;
    delitem(interp, &this, &index)?;
    Ok(value)
}

/// `MutableSequence.remove`: `del self[self.index(value)]`.
pub(super) fn mutable_sequence_remove(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, value) = bind_self_arg(
        &sig("MutableSequence.remove", &["self", "value"], 0),
        call_args,
    )?;
    let index = call_method(interp, &this, "index", &[value])?;
    delitem(interp, &this, &index)?;
    Ok(Value::none())
}

/// `MutableSequence.__iadd__`.
pub(super) fn mutable_sequence_iadd(
    interp: &mut Interpreter,
    call_args: &[ExpandedCallArg],
) -> Result<Value> {
    let (this, values) = bind_self_arg(
        &sig("MutableSequence.__iadd__", &["self", "values"], 0),
        call_args,
    )?;
    call_method(interp, &this, "extend", &[values])?;
    Ok(this)
}
