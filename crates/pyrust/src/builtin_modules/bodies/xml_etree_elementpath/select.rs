//! The `prepare_*` compilers and the lazy `select` generators they return.
//!
//! Every CPython `select(context, result)` closure becomes one [`Step`]; a
//! running step is a native generator ([`SelectIter`]) that pulls from its
//! `result` iterator exactly as the CPython generator body does, calling the
//! same element methods (`__iter__`, `tag`, `iter`, `get`, `find`, `findall`,
//! `iterfind`, `itertext`) in the same order.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use super::{
    call_method, error_is, get_parent_map_impl, getitem, int_value, is_str_instance, next_item,
    py_eq, py_ne, slice_eq, str_text, syntax_error, truthy, type_error, type_name,
};
use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::{Interpreter, ProviderIterator, is_stop_iteration_error, make_iterator};
use crate::value::{PyDict, Value};

/// The token source handed to `prepare_*` as `next`.
pub(super) enum Feed {
    /// The tokenizer generator itself (`next(gen)`).
    Iter(Value),
    /// Any other zero-argument callable.
    Call(Value),
}

impl Feed {
    /// `next()`, with `StopIteration` reported as `None`.
    pub(super) fn pull(&self, interp: &mut Interpreter) -> Result<Option<Value>> {
        let result = match self {
            Self::Iter(tokens) => interp.call_next(tokens, None),
            Self::Call(callable) => super::call(interp, callable.clone(), &[]),
        };
        match result {
            Ok(token) => Ok(Some(token)),
            Err(err) if is_stop_iteration_error(&err) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

/// `token[index]`.
pub(super) fn token_item(interp: &mut Interpreter, token: &Value, index: usize) -> Result<Value> {
    if let Some(items) = token.as_tuple() {
        return items
            .get(index)
            .cloned()
            .ok_or_else(|| PyError::named("IndexError", "tuple index out of range"));
    }
    getitem(interp, token, Value::int(index as i64))
}

/// `value == text` for a `str` literal on the right.
fn eq_text(interp: &mut Interpreter, value: &Value, text: &str) -> Result<bool> {
    if let Some(s) = value.as_str() {
        return Ok(s == text);
    }
    py_eq(interp, value, &Value::string(text))
}

/// `token == ('', '')`.
fn is_blank_token(interp: &mut Interpreter, token: &Value) -> Result<bool> {
    if let Some(items) = token.as_tuple() {
        if items.len() != 2 {
            return Ok(false);
        }
        let (op, tag) = (items[0].clone(), items[1].clone());
        return Ok(eq_text(interp, &op, "")? && eq_text(interp, &tag, "")?);
    }
    let blank = Value::tuple(vec![Value::string(""), Value::string("")]);
    py_eq(interp, token, &blank)
}

/// Tag filters built by `_prepare_tag` for the wildcard tag forms.
pub(super) enum TagFilter {
    /// `{*}*`: any `str` tag (no comments or processing instructions).
    AnyStr,
    /// `{}*`: any `str` tag outside a namespace.
    NoNamespace,
    /// `{*}name`: `name` in any (or no) namespace.
    AnyNamespace { local: Value, suffix: String },
    /// `{ns}*`: any tag in namespace `ns`.
    InNamespace { prefix: String },
}

impl TagFilter {
    /// `_prepare_tag(tag)`'s dispatch, or `None` for its `RuntimeError` arm.
    fn for_tag(tag: &str) -> Option<Self> {
        if tag == "{*}*" {
            return Some(Self::AnyStr);
        }
        if tag == "{}*" {
            return Some(Self::NoNamespace);
        }
        if let Some(local) = tag.strip_prefix("{*}") {
            return Some(Self::AnyNamespace {
                local: Value::string(local),
                suffix: tag[2..].to_string(),
            });
        }
        tag.strip_suffix('*')
            .filter(|ns| ns.ends_with('}'))
            .map(|ns| Self::InNamespace {
                prefix: ns.to_string(),
            })
    }

    fn matches(&self, interp: &mut Interpreter, el_tag: &Value) -> Result<bool> {
        match self {
            Self::AnyStr => Ok(is_str_instance(el_tag)),
            Self::NoNamespace => match str_text(el_tag) {
                // `el_tag[0] != '{'`
                Some(text) if text.is_empty() => {
                    Err(PyError::named("IndexError", "string index out of range"))
                }
                Some(text) => Ok(!text.starts_with('{')),
                None => Ok(false),
            },
            Self::AnyNamespace { local, suffix } => {
                if py_eq(interp, el_tag, local)? {
                    return Ok(true);
                }
                Ok(str_text(el_tag).is_some_and(|text| text.ends_with(suffix.as_str())))
            }
            Self::InNamespace { prefix } => {
                Ok(str_text(el_tag).is_some_and(|text| text.starts_with(prefix.as_str())))
            }
        }
    }
}

/// One compiled `select` function.
pub(super) enum Step {
    /// `prepare_child`: children whose `tag == tag`.
    Child(Value),
    /// `prepare_child` with a wildcard tag.
    WildChild(TagFilter),
    /// `prepare_star`: every child.
    Star,
    /// `prepare_self`.
    Current,
    /// `prepare_descendant`: `elem.iter(tag)` minus `elem`.
    Descendant(Value),
    /// `prepare_descendant` with a wildcard tag.
    WildDescendant(TagFilter),
    /// `prepare_parent`.
    Parent,
    /// `[@attr]`.
    AttrExists(Value),
    /// `[@attr='value']`.
    AttrEq(Value, Value),
    /// `[@attr!='value']`.
    AttrNe(Value, Value),
    /// `[tag]`.
    TagExists(Value),
    /// `[tag='value']`.
    TagTextEq(Value, String),
    /// `[tag!='value']`.
    TagTextNe(Value, String),
    /// `[.='value']`.
    TextEq(String),
    /// `[.!='value']`.
    TextNe(String),
    /// `[n]`, `[last()]`, `[last()-n]`; `None` is an index no list reaches.
    Position(Option<i64>),
    /// The `select` returned by `_prepare_tag(tag)`.
    Filter(TagFilter),
}

impl Step {
    /// `__qualname__` of the CPython closure this step stands for.
    pub(super) fn qualname(&self) -> &'static str {
        match self {
            Self::Child(_) | Self::WildChild(_) => "prepare_child.<locals>.select",
            Self::Star => "prepare_star.<locals>.select",
            Self::Current => "prepare_self.<locals>.select",
            Self::Descendant(_) | Self::WildDescendant(_) => "prepare_descendant.<locals>.select",
            Self::Parent => "prepare_parent.<locals>.select",
            Self::AttrNe(..) | Self::TagTextNe(..) | Self::TextNe(_) => {
                "prepare_predicate.<locals>.select_negated"
            }
            Self::AttrExists(_)
            | Self::AttrEq(..)
            | Self::TagExists(_)
            | Self::TagTextEq(..)
            | Self::TextEq(_)
            | Self::Position(_) => "prepare_predicate.<locals>.select",
            Self::Filter(_) => "_prepare_tag.<locals>.select",
        }
    }
}

// ── prepare_* ────────────────────────────────────────────────────────────────

/// `_is_wildcard_tag(tag)`: `tag[:3] == '{*}' or tag[-2:] == '}*'`.
pub(super) fn is_wildcard_tag(interp: &mut Interpreter, tag: &Value) -> Result<bool> {
    match str_text(tag) {
        Some(text) => Ok(text.starts_with("{*}") || text.ends_with("}*")),
        None => Ok(slice_eq(interp, tag, None, Some(3), "{*}")?
            || slice_eq(interp, tag, Some(-2), None, "}*")?),
    }
}

/// `_prepare_tag(tag)`.
pub(super) fn prepare_tag(interp: &mut Interpreter, tag: &Value) -> Result<TagFilter> {
    let filter = match str_text(tag) {
        Some(text) => TagFilter::for_tag(&text),
        // A non-`str` tag runs CPython's comparisons (and their errors); the
        // textual filters cannot apply to it.
        None if eq_text(interp, tag, "{*}*")? => Some(TagFilter::AnyStr),
        None if eq_text(interp, tag, "{}*")? => Some(TagFilter::NoNamespace),
        None => {
            slice_eq(interp, tag, None, Some(3), "{*}")?;
            slice_eq(interp, tag, Some(-2), None, "}*")?;
            None
        }
    };
    match filter {
        Some(filter) => Ok(filter),
        None => {
            let shown = super::value_str(interp, tag)?;
            Err(PyError::named(
                "RuntimeError",
                format!("internal parser error, got {shown}"),
            ))
        }
    }
}

/// `'{}tag' == 'tag'`.
fn strip_empty_namespace(tag: Value) -> Value {
    match str_text(&tag) {
        Some(text) if text.starts_with("{}") => Value::string(&text[2..]),
        _ => tag,
    }
}

/// `prepare_child(next, token)`.
pub(super) fn prepare_child(interp: &mut Interpreter, token: &Value) -> Result<Step> {
    let tag = token_item(interp, token, 1)?;
    if is_wildcard_tag(interp, &tag)? {
        return Ok(Step::WildChild(prepare_tag(interp, &tag)?));
    }
    Ok(Step::Child(strip_empty_namespace(tag)))
}

/// `prepare_descendant(next, token)`; `None` when `next()` is exhausted.
pub(super) fn prepare_descendant(interp: &mut Interpreter, feed: &Feed) -> Result<Option<Step>> {
    let Some(token) = feed.pull(interp)? else {
        return Ok(None);
    };
    let op = token_item(interp, &token, 0)?;
    let tag = if eq_text(interp, &op, "*")? {
        Value::string("*")
    } else if !truthy(interp, &op)? {
        token_item(interp, &token, 1)?
    } else {
        return Err(syntax_error("invalid descendant"));
    };
    if is_wildcard_tag(interp, &tag)? {
        return Ok(Some(Step::WildDescendant(prepare_tag(interp, &tag)?)));
    }
    Ok(Some(Step::Descendant(strip_empty_namespace(tag))))
}

/// `re.match(r"\-?\d+$", text)`.
fn is_int_literal(value: &Value) -> Result<bool> {
    let Some(text) = str_text(value) else {
        return Err(type_error(format!(
            "expected string or bytes-like object, got '{}'",
            type_name(value)
        )));
    };
    let body = text.strip_suffix('\n').unwrap_or(&text);
    let digits = body.strip_prefix('-').unwrap_or(body);
    Ok(!digits.is_empty()
        && digits
            .chars()
            .all(|c| crate::builtin_modules::unicodedata::decimal_value(c as u32).is_some()))
}

/// `prepare_predicate(next, token)`; `None` when `next()` runs out before `]`.
pub(super) fn prepare_predicate(interp: &mut Interpreter, feed: &Feed) -> Result<Option<Step>> {
    let mut signature = String::new();
    let mut predicate: Vec<Value> = Vec::new();
    loop {
        let Some(token) = feed.pull(interp)? else {
            return Ok(None);
        };
        let op = token_item(interp, &token, 0)?;
        if eq_text(interp, &op, "]")? {
            break;
        }
        if is_blank_token(interp, &token)? {
            // ignore whitespace
            continue;
        }
        let op_truthy = truthy(interp, &op)?;
        let op_text = if op_truthy {
            match str_text(&op) {
                Some(text) => Some(text.into_owned()),
                None => {
                    return Err(type_error(format!(
                        "'{}' object is not subscriptable",
                        type_name(&op)
                    )));
                }
            }
        } else {
            None
        };
        match op_text {
            Some(text) if text.starts_with(['\'', '"']) => {
                // token = "'", token[0][1:-1]
                let inner: String = {
                    let mut chars = text.chars();
                    chars.next();
                    chars.next_back();
                    chars.collect()
                };
                signature.push('\'');
                predicate.push(Value::string(inner));
            }
            Some(text) => {
                signature.push_str(&text);
                predicate.push(token_item(interp, &token, 1)?);
            }
            None => {
                signature.push('-');
                predicate.push(token_item(interp, &token, 1)?);
            }
        }
    }
    classify_predicate(interp, &signature, predicate).map(Some)
}

/// The text of a `predicate` entry that the signature says is a quoted value.
fn quoted(value: &Value) -> String {
    str_text(value)
        .map(|text| text.into_owned())
        .unwrap_or_default()
}

fn classify_predicate(
    interp: &mut Interpreter,
    signature: &str,
    mut predicate: Vec<Value>,
) -> Result<Step> {
    let negated = signature.contains("!=");
    match signature {
        // [@attribute] predicate
        "@-" => return Ok(Step::AttrExists(predicate.swap_remove(1))),
        // [@attribute='value'] or [@attribute!='value']
        "@-='" | "@-!='" => {
            let value = predicate.pop().unwrap_or_else(Value::none);
            let key = predicate.swap_remove(1);
            return Ok(if negated {
                Step::AttrNe(key, value)
            } else {
                Step::AttrEq(key, value)
            });
        }
        _ => {}
    }
    if signature == "-" && !is_int_literal(&predicate[0])? {
        // [tag]
        return Ok(Step::TagExists(predicate.swap_remove(0)));
    }
    let text_compare = matches!(signature, ".='" | ".!='")
        || (matches!(signature, "-='" | "-!='") && !is_int_literal(&predicate[0])?);
    if text_compare {
        // [.='value'] or [tag='value'] or [.!='value'] or [tag!='value']
        let value = quoted(&predicate[predicate.len() - 1]);
        let tag = predicate.swap_remove(0);
        return Ok(if truthy(interp, &tag)? {
            if negated {
                Step::TagTextNe(tag, value)
            } else {
                Step::TagTextEq(tag, value)
            }
        } else if negated {
            Step::TextNe(value)
        } else {
            Step::TextEq(value)
        });
    }
    if matches!(signature, "-" | "-()" | "-()-") {
        // [index] or [last()] or [last()-index]
        let index = if signature == "-" {
            let position = int_value(interp, &predicate[0])?;
            let index = decrement(interp, position)?;
            if is_negative(interp, &index, 0)? {
                return Err(syntax_error("XPath position >= 1 expected"));
            }
            index
        } else {
            let last = Value::string("last");
            if py_ne(interp, &predicate[0], &last)? {
                return Err(syntax_error("unsupported function"));
            }
            if signature == "-()-" {
                let offset = match int_value(interp, &predicate[2]) {
                    Ok(offset) => offset,
                    Err(err) if error_is(&err, "ValueError") => {
                        return Err(syntax_error("unsupported expression"));
                    }
                    Err(err) => return Err(err),
                };
                let index = decrement(interp, offset)?;
                // index > -2
                if !is_negative(interp, &index, -1)? {
                    return Err(syntax_error("XPath offset from last() must be negative"));
                }
                index
            } else {
                Value::int(-1)
            }
        };
        return Ok(Step::Position(index.as_int()));
    }
    Err(syntax_error("invalid predicate"))
}

/// `value - 1`.
fn decrement(interp: &mut Interpreter, value: Value) -> Result<Value> {
    interp.eval_binary(value, BinaryOp::Sub, Value::int(1))
}

/// `value < bound`.
fn is_negative(interp: &mut Interpreter, value: &Value, bound: i64) -> Result<bool> {
    let lt = interp.eval_binary(value.clone(), BinaryOp::Lt, Value::int(bound))?;
    truthy(interp, &lt)
}

// ── Running selects ──────────────────────────────────────────────────────────

/// One running `select(context, result)` generator.
struct SelectIter {
    step: Rc<Step>,
    context: Value,
    /// `result`, until the generator body starts.
    source: Option<Value>,
    /// `iter(result)`.
    upstream: Option<Value>,
    /// The nested loop's iterator and the element it walks.
    inner: Option<(Value, Value)>,
    /// `get_parent_map(context)` for `..` and positional predicates.
    parent_map: Value,
    /// `prepare_parent`'s `result_map`.
    seen: Value,
}

/// `select(context, result)`: a lazy generator over the step's matches.
pub(super) fn make_select_iter(step: Rc<Step>, context: Value, result: Value) -> Value {
    let state = SelectIter {
        step,
        context,
        source: Some(result),
        upstream: None,
        inner: None,
        parent_map: Value::none(),
        seen: Value::none(),
    };
    Value::generator(Box::new(ProviderIterator::new(
        Box::new(Some(Some(state))),
        advance_select,
        "generator",
        "generator",
        None,
    )))
}

fn advance_select(
    interp: &mut Interpreter,
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<Option<Value>> {
    // The state is moved out while element methods run, so a re-entrant
    // `next()` reports what a running generator would.
    let taken = state_rc
        .borrow_mut()
        .downcast_mut::<Option<Option<SelectIter>>>()
        .map(Option::take);
    let mut st = match taken {
        Some(Some(Some(st))) => st,
        Some(Some(None)) => {
            restore(state_rc, None);
            return Ok(None);
        }
        Some(None) => {
            return Err(PyError::named("ValueError", "generator already executing"));
        }
        None => {
            return Err(PyError::runtime(
                "internal: ElementPath select state corrupted".to_string(),
            ));
        }
    };
    let result = select_next(interp, &mut st);
    let keep = matches!(result, Ok(Some(_)));
    restore(state_rc, keep.then_some(st));
    result.map_err(|err| interp.pep479_wrap_stop_iteration(err))
}

fn restore(state_rc: &Rc<RefCell<Box<dyn Any>>>, st: Option<SelectIter>) {
    if let Some(slot) = state_rc
        .borrow_mut()
        .downcast_mut::<Option<Option<SelectIter>>>()
    {
        *slot = Some(st);
    }
}

fn select_next(interp: &mut Interpreter, st: &mut SelectIter) -> Result<Option<Value>> {
    if let Some(source) = st.source.take() {
        if matches!(*st.step, Step::Parent | Step::Position(_)) {
            st.parent_map = get_parent_map_impl(interp, &st.context)?;
            if matches!(*st.step, Step::Parent) {
                st.seen = Value::dict(PyDict::default());
            }
        }
        st.upstream = Some(make_iterator(interp, &source)?);
    }
    let step = Rc::clone(&st.step);
    loop {
        if let Some((inner, owner)) = st.inner.clone() {
            match next_item(interp, &inner)? {
                Some(e) => {
                    if inner_accepts(interp, &step, &e, &owner)? {
                        return Ok(Some(e));
                    }
                    continue;
                }
                None => st.inner = None,
            }
        }
        let Some(upstream) = st.upstream.clone() else {
            return Ok(None);
        };
        let Some(elem) = next_item(interp, &upstream)? else {
            st.upstream = None;
            return Ok(None);
        };
        match &*step {
            Step::Child(_) | Step::WildChild(_) | Step::Star => {
                st.inner = Some((make_iterator(interp, &elem)?, elem));
            }
            Step::Descendant(tag) => {
                let walk = call_method(interp, &elem, "iter", std::slice::from_ref(tag))?;
                st.inner = Some((make_iterator(interp, &walk)?, elem));
            }
            Step::WildDescendant(_) => {
                let walk = call_method(interp, &elem, "iter", &[])?;
                st.inner = Some((make_iterator(interp, &walk)?, elem));
            }
            Step::Current => return Ok(Some(elem)),
            Step::Parent => {
                let parent_map = st.parent_map.clone();
                if let Some(parent) = map_lookup(interp, &parent_map, &elem)? {
                    let seen = st.seen.clone();
                    if interp.dict_lookup_value(&seen, &parent)?.is_none() {
                        let key = interp.value_to_pykey(&parent)?;
                        interp.dict_insert_value(&seen, key, Value::none())?;
                        return Ok(Some(parent));
                    }
                }
            }
            Step::Position(index) => {
                let parent_map = st.parent_map.clone();
                match position_matches(interp, &parent_map, &elem, *index) {
                    Ok(true) => return Ok(Some(elem)),
                    Ok(false) => {}
                    Err(err) if error_is(&err, "IndexError") || error_is(&err, "KeyError") => {}
                    Err(err) => return Err(err),
                }
            }
            _ => {
                if predicate_accepts(interp, &step, &elem)? {
                    return Ok(Some(elem));
                }
            }
        }
    }
}

/// The nested-loop test of the child / descendant steps.
fn inner_accepts(interp: &mut Interpreter, step: &Step, e: &Value, owner: &Value) -> Result<bool> {
    match step {
        Step::Child(tag) => {
            let el_tag = interp.get_attr(e, "tag")?;
            py_eq(interp, &el_tag, tag)
        }
        Step::WildChild(filter) => {
            let el_tag = interp.get_attr(e, "tag")?;
            filter.matches(interp, &el_tag)
        }
        Step::Descendant(_) => Ok(!e.is_identical_to(owner)),
        Step::WildDescendant(filter) => {
            if e.is_identical_to(owner) {
                return Ok(false);
            }
            let el_tag = interp.get_attr(e, "tag")?;
            filter.matches(interp, &el_tag)
        }
        _ => Ok(true),
    }
}

/// `"".join(elem.itertext())`.
fn itertext_joined(interp: &mut Interpreter, elem: &Value) -> Result<String> {
    let parts = call_method(interp, elem, "itertext", &[])?;
    let items = interp.collect_iterable(&parts)?;
    let mut out = String::new();
    for (i, item) in items.iter().enumerate() {
        match str_text(item) {
            Some(text) => out.push_str(&text),
            None => {
                return Err(type_error(format!(
                    "sequence item {i}: expected str instance, {} found",
                    type_name(item)
                )));
            }
        }
    }
    Ok(out)
}

/// Whether some element of `found` has `"".join(e.itertext())` (un)equal to
/// `value` (the `for e in ...: if ...: yield elem; break` loops).
fn any_text(interp: &mut Interpreter, found: &Value, value: &str, negated: bool) -> Result<bool> {
    let iterator = make_iterator(interp, found)?;
    while let Some(e) = next_item(interp, &iterator)? {
        if (itertext_joined(interp, &e)? == value) != negated {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The per-element test of the attribute / text / sub-element predicates and
/// of `_prepare_tag`'s select.
fn predicate_accepts(interp: &mut Interpreter, step: &Step, elem: &Value) -> Result<bool> {
    match step {
        Step::AttrExists(key) => {
            let value = call_method(interp, elem, "get", std::slice::from_ref(key))?;
            Ok(!value.is_none())
        }
        Step::AttrEq(key, expected) => {
            let value = call_method(interp, elem, "get", std::slice::from_ref(key))?;
            py_eq(interp, &value, expected)
        }
        Step::AttrNe(key, expected) => {
            let value = call_method(interp, elem, "get", std::slice::from_ref(key))?;
            Ok(!value.is_none() && py_ne(interp, &value, expected)?)
        }
        Step::TagExists(tag) => {
            let found = call_method(interp, elem, "find", std::slice::from_ref(tag))?;
            Ok(!found.is_none())
        }
        Step::TagTextEq(tag, value) => {
            let found = call_method(interp, elem, "findall", std::slice::from_ref(tag))?;
            any_text(interp, &found, value, false)
        }
        Step::TagTextNe(tag, value) => {
            let found = call_method(interp, elem, "iterfind", std::slice::from_ref(tag))?;
            any_text(interp, &found, value, true)
        }
        Step::TextEq(value) => Ok(itertext_joined(interp, elem)? == *value),
        Step::TextNe(value) => Ok(itertext_joined(interp, elem)? != *value),
        Step::Filter(filter) => {
            let el_tag = interp.get_attr(elem, "tag")?;
            filter.matches(interp, &el_tag)
        }
        _ => Ok(true),
    }
}

/// `if elem in parent_map: parent = parent_map[elem]`.
fn map_lookup(interp: &mut Interpreter, map: &Value, key: &Value) -> Result<Option<Value>> {
    if map.is_dict() {
        return Ok(interp.dict_lookup_value(map, key)?.map(|(_, value)| value));
    }
    let contained = interp.eval_binary(key.clone(), BinaryOp::In, map.clone())?;
    if !truthy(interp, &contained)? {
        return Ok(None);
    }
    getitem(interp, map, key.clone()).map(Some)
}

/// The body of the positional predicate's `try:` block.
fn position_matches(
    interp: &mut Interpreter,
    parent_map: &Value,
    elem: &Value,
    index: Option<i64>,
) -> Result<bool> {
    // parent = parent_map[elem]
    let parent = if parent_map.is_dict() {
        match interp.dict_lookup_value(parent_map, elem)? {
            Some((_, parent)) => parent,
            None => return Ok(false),
        }
    } else {
        getitem(interp, parent_map, elem.clone())?
    };
    // elems = list(parent.findall(elem.tag))
    let tag = interp.get_attr(elem, "tag")?;
    let found = call_method(interp, &parent, "findall", &[tag])?;
    let elems = interp.collect_iterable(&found)?;
    // if elems[index] is elem
    let Some(index) = index else {
        return Ok(false);
    };
    let len = elems.len() as i64;
    let slot = if index < 0 { index + len } else { index };
    Ok(usize::try_from(slot)
        .ok()
        .and_then(|slot| elems.get(slot))
        .is_some_and(|item| item.is_identical_to(elem)))
}
