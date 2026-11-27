// `xml.etree.ElementPath` module — 100% native Rust implementation of
// CPython 3.12.3's `Lib/xml/etree/ElementPath.py` (issues #3210, #3410).
//
// Included into `pub mod xml_etree_elementpath { … }` declared by the
// `pyrust_builtin_modules!` invocation in `builtin_modules/mod.rs`.
//
// * `xpath_tokenizer` is a lazy generator over a native scanner that yields
//   exactly what `xpath_tokenizer_re.findall()` returns, then resolves
//   namespace prefixes like CPython (errors surface token by token).
// * `prepare_child` / `prepare_star` / … / `prepare_predicate` compile one
//   step each into a `select` callable (a hidden native class standing in for
//   CPython's nested `select` functions).  Calling a select returns a lazy
//   `generator` that walks elements through the same duck-typed protocol
//   (`iter(elem)`, `.tag`, `.iter()`, `.get()`, `.find()`, `.findall()`,
//   `.iterfind()`, `.itertext()`), in the same order.
// * `iterfind` follows CPython step for step: the selector cache is the
//   module's real `_cache` dict (same keys, same 100-entry reset), the step
//   compilers are looked up in the module's `ops` dict (so replacing an entry
//   takes effect), and `find` / `findall` / `findtext` wrap it as CPython does.
//
// Reference: <https://docs.python.org/3/library/xml.etree.elementtree.html#xpath-support>

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::macros::native_state::{
    has_native_state, set_native_state, with_native_state,
};
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, instantiate_exception,
    is_stop_iteration_error, lookup_exc_class, render_value_repr, value_type_name_str,
};
use crate::value::{InstanceAttrs, PyClass, PyDict, PyInstance, PyModule, Value, ValueKind};

#[path = "xml_etree_elementpath/select.rs"]
mod select;
#[path = "xml_etree_elementpath/tokenizer.rs"]
mod tokenizer;

use select::{Feed, Step};

type ClassRc = Rc<RefCell<PyClass>>;

/// `xpath_tokenizer_re.pattern`, byte for byte.
const XPATH_TOKENIZER_PATTERN: &str = concat!(
    r#"('[^']*'|\"[^\"]*\"|"#,
    r#"::|"#,
    r#"//?|"#,
    r#"\.\.|"#,
    r#"\(\)|"#,
    r#"!=|"#,
    r#"[/.*:\[\]\(\)@=])|"#,
    r#"((?:\{[^}]+\})?[^/\[\]\(\)@!=\s]+)|"#,
    r#"\s+"#,
);

/// The hidden class standing in for CPython's nested `select` functions.
const SELECT_CLASS_NAME: &str = "_Select";

/// The module namespace in CPython's definition order (after the dunders).
const NAMESPACE_ORDER: [&str; 19] = [
    "re",
    "xpath_tokenizer_re",
    "xpath_tokenizer",
    "get_parent_map",
    "_is_wildcard_tag",
    "_prepare_tag",
    "prepare_child",
    "prepare_star",
    "prepare_self",
    "prepare_descendant",
    "prepare_parent",
    "prepare_predicate",
    "ops",
    "_cache",
    "_SelectorContext",
    "iterfind",
    "find",
    "findall",
    "findtext",
];

/// `ops`: path operator -> step compiler.
const OPS: [(&str, NativePrepare); 6] = [
    ("", NativePrepare::Child),
    ("*", NativePrepare::Star),
    (".", NativePrepare::SelfStep),
    ("..", NativePrepare::Parent),
    ("//", NativePrepare::Descendant),
    ("[", NativePrepare::Predicate),
];

/// The native `prepare_*` functions.
#[derive(Clone, Copy)]
enum NativePrepare {
    Child,
    Star,
    SelfStep,
    Parent,
    Descendant,
    Predicate,
}

impl NativePrepare {
    fn py_name(self) -> &'static str {
        match self {
            Self::Child => "prepare_child",
            Self::Star => "prepare_star",
            Self::SelfStep => "prepare_self",
            Self::Parent => "prepare_parent",
            Self::Descendant => "prepare_descendant",
            Self::Predicate => "prepare_predicate",
        }
    }

    fn registered_name(self) -> &'static str {
        match self {
            Self::Child => __pyrust_qualname!("prepare_child"),
            Self::Star => __pyrust_qualname!("prepare_star"),
            Self::SelfStep => __pyrust_qualname!("prepare_self"),
            Self::Parent => __pyrust_qualname!("prepare_parent"),
            Self::Descendant => __pyrust_qualname!("prepare_descendant"),
            Self::Predicate => __pyrust_qualname!("prepare_predicate"),
        }
    }

    /// The native compiler `value` is, if it is one of this module's.
    fn of(value: &Value) -> Option<Self> {
        let ValueKind::BuiltinFunction(name) = value.kind() else {
            return None;
        };
        OPS.iter()
            .map(|&(_, prepare)| prepare)
            .find(|prepare| prepare.registered_name() == name)
    }

    /// `prepare(next, token)`.
    fn run(self, interp: &mut Interpreter, feed: &Feed, token: &Value) -> Result<Value> {
        let step = match self {
            Self::Child => Some(select::prepare_child(interp, token)?),
            Self::Star => Some(Step::Star),
            Self::SelfStep => Some(Step::Current),
            Self::Parent => Some(Step::Parent),
            Self::Descendant => select::prepare_descendant(interp, feed)?,
            Self::Predicate => select::prepare_predicate(interp, feed)?,
        };
        match step {
            Some(step) => new_select(step),
            None => Ok(Value::none()),
        }
    }
}

// ── Module-local registry ────────────────────────────────────────────────────

/// What the native functions need from the most recently imported module.
struct ModuleState {
    /// The module, for its live `_cache` and `ops` globals.
    module: Weak<RefCell<PyModule>>,
    /// The hidden `_Select` class.
    select_class: ClassRc,
    /// `_SelectorContext`.
    context_class: ClassRc,
}

// Thread-local justification:
// (a) Why `thread_local!`-style storage: `iterfind` must reach the module's
//     live `_cache` / `ops` globals and the classes it instantiates, but
//     native module functions receive no module handle; the registry holds
//     `!Send` `Rc` class handles and a `Weak` module reference.
// (b) Multi-interpreter / multi-threaded impact: the registry is GIL-local,
//     so separate interpreters and threads are isolated; a re-import replaces
//     the entry, and selects created earlier keep working because each one
//     owns its compiled step.
pyrust_core::gil_local! {
    static STATE: RefCell<Option<ModuleState>> = const { RefCell::new(None) };
}

fn select_class() -> Result<ClassRc> {
    STATE.with(|state| {
        state
            .borrow()
            .as_ref()
            .map(|st| Rc::clone(&st.select_class))
            .ok_or_else(|| PyError::runtime("xml.etree.ElementPath is not initialised".into()))
    })
}

fn context_class() -> Result<ClassRc> {
    STATE.with(|state| {
        state
            .borrow()
            .as_ref()
            .map(|st| Rc::clone(&st.context_class))
            .ok_or_else(|| PyError::runtime("xml.etree.ElementPath is not initialised".into()))
    })
}

/// The module's current `_cache` and `ops` globals.
fn module_tables() -> (Option<Value>, Option<Value>) {
    STATE.with(|state| {
        let state = state.borrow();
        let Some(module) = state.as_ref().and_then(|st| st.module.upgrade()) else {
            return (None, None);
        };
        // Through `get_attr_value`: once `vars(module)` has been taken, the
        // live namespace dict (not `attrs`) holds the module's globals.
        let module = module.borrow();
        (
            module.get_attr_value("_cache"),
            module.get_attr_value("ops"),
        )
    })
}

// ── Generic helpers ──────────────────────────────────────────────────────────

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn syntax_error(msg: impl Into<String>) -> PyError {
    PyError::named("SyntaxError", msg.into())
}

/// `KeyError(key)`.
fn key_error(key: Value) -> PyError {
    match lookup_exc_class("KeyError") {
        Some(class) => PyError::raised(instantiate_exception(class, vec![key])),
        None => PyError::named("KeyError", String::new()),
    }
}

/// CPython's `Py_TYPE(obj)->tp_name`.
fn type_name(value: &Value) -> String {
    value_type_name_str(value).into_owned()
}

/// The text of an exact `str` or a `str` subclass instance.
fn str_text(value: &Value) -> Option<Cow<'_, str>> {
    if let Some(text) = value.as_str() {
        return Some(Cow::Borrowed(text));
    }
    let backing = builtin_data_backing(value)?;
    backing.as_str().map(|text| Cow::Owned(text.to_owned()))
}

/// `isinstance(value, str)`.
fn is_str_instance(value: &Value) -> bool {
    str_text(value).is_some()
}

fn call(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    args: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call(interp, callee, args)
}

fn builtin(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let builtins = interp.load_module("builtins")?;
    interp.get_attr(&builtins, name)
}

/// `bool(value)`.
fn truthy(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    if let Some(text) = value.as_str() {
        return Ok(!text.is_empty());
    }
    if value.is_none() {
        return Ok(false);
    }
    interp.truthy_value(value)
}

/// `bool(a == b)`.
fn py_eq(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<bool> {
    if let (Some(x), Some(y)) = (a.as_str(), b.as_str()) {
        return Ok(x == y);
    }
    let result = interp.eval_binary(a.clone(), BinaryOp::Eq, b.clone())?;
    truthy(interp, &result)
}

/// `bool(a != b)`.
fn py_ne(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<bool> {
    if let (Some(x), Some(y)) = (a.as_str(), b.as_str()) {
        return Ok(x != y);
    }
    let result = interp.eval_binary(a.clone(), BinaryOp::Ne, b.clone())?;
    truthy(interp, &result)
}

/// `str(value)`.
fn value_str(interp: &mut Interpreter, value: &Value) -> Result<String> {
    if let Some(text) = value.as_str() {
        return Ok(text.to_string());
    }
    interp.render_value_as_str(value)
}

/// `repr(value)`.
fn render_repr(interp: &mut Interpreter, value: &Value) -> Result<String> {
    render_value_repr(interp, value)
}

/// Whether `err` is an instance of the built-in exception `name` (or of a
/// subclass of it).
fn error_is(err: &PyError, name: &str) -> bool {
    use crate::interpreter::class_is_builtin_exception_subclass as is_subclass;
    match err.kind() {
        pyrust_core::PyErrorKind::Named(class, _) => class.as_ref() == name,
        pyrust_core::PyErrorKind::KeyError(_) => name == "KeyError",
        pyrust_core::PyErrorKind::AttributeError { .. } => name == "AttributeError",
        pyrust_core::PyErrorKind::Class(class, _) => is_subclass(class, name),
        pyrust_core::PyErrorKind::Raised(exc) => match exc.kind() {
            ValueKind::PyInstance(inst) => is_subclass(&inst.borrow().class, name),
            ValueKind::PyClass(class) => is_subclass(class, name),
            _ => false,
        },
        _ => false,
    }
}

/// The next item of `iterator`, or `None` once it is exhausted.
fn next_item(interp: &mut Interpreter, iterator: &Value) -> Result<Option<Value>> {
    match interp.call_next(iterator, None) {
        Ok(item) => Ok(Some(item)),
        Err(err) if is_stop_iteration_error(&err) => Ok(None),
        Err(err) => Err(err),
    }
}

/// `obj[key]`.
fn getitem(interp: &mut Interpreter, obj: &Value, key: Value) -> Result<Value> {
    if obj.is_dict() {
        // An exact dict (subclass instances, e.g. with `__missing__`, take
        // the full subscription path below).
        return match interp.dict_lookup_value(obj, &key)? {
            Some((_, value)) => Ok(value),
            None => Err(key_error(key)),
        };
    }
    crate::builtin_modules::operator::getitem_value(interp, obj.clone(), key)
}

/// `obj[start:stop]`.
fn slice(
    interp: &mut Interpreter,
    obj: &Value,
    start: Option<i64>,
    stop: Option<i64>,
) -> Result<Value> {
    let bounds =
        crate::interpreter::make_slice_value(start.map(Value::int), stop.map(Value::int), None);
    getitem(interp, obj, bounds)
}

/// `bool(obj[start:stop] == text)`.
fn slice_eq(
    interp: &mut Interpreter,
    obj: &Value,
    start: Option<i64>,
    stop: Option<i64>,
    text: &str,
) -> Result<bool> {
    let part = slice(interp, obj, start, stop)?;
    py_eq(interp, &part, &Value::string(text))
}

/// `int(value)`.
///
/// Decimal digits of any script are mapped to ASCII first, as CPython's own
/// `int()` parser does (`_PyUnicode_TransformDecimalAndSpaceToASCII`).
fn int_value(interp: &mut Interpreter, value: &Value) -> Result<Value> {
    let int_type = builtin(interp, "int")?;
    if let Some(text) = value.as_str()
        && !text.is_ascii()
    {
        let ascii: String = text
            .chars()
            .map(|c| {
                crate::builtin_modules::unicodedata::decimal_value(c as u32)
                    .and_then(|digit| char::from_digit(digit, 10))
                    .unwrap_or(c)
            })
            .collect();
        if ascii.is_ascii() {
            return call(interp, int_type, &[Value::string(ascii)]);
        }
    }
    call(interp, int_type, std::slice::from_ref(value))
}

fn instance_of(class: &ClassRc) -> Value {
    Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(class),
        attrs: InstanceAttrs::new(),
    })))
}

// ── Argument binding ─────────────────────────────────────────────────────────

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// `'a'`, `'a' and 'b'`, `'a', 'b', and 'c'` (CPython's `format_missing`).
fn format_names(names: &[&str]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
    match quoted.len() {
        0 => String::new(),
        1 => quoted[0].clone(),
        2 => format!("{} and {}", quoted[0], quoted[1]),
        n => format!("{}, and {}", quoted[..n - 1].join(", "), quoted[n - 1]),
    }
}

/// Bind call arguments the way CPython binds a `def` whose parameters are all
/// positional-or-keyword (the last `defaults` have defaults), with its exact
/// `TypeError` wordings and check order.
fn bind<const N: usize>(
    qualname: &str,
    params: [&str; N],
    defaults: usize,
    args: &[ExpandedCallArg],
) -> Result<[Option<Value>; N]> {
    let mut slots: [Option<Value>; N] = std::array::from_fn(|_| None);
    let mut positional = 0;
    for arg in args {
        if arg.name.is_none() {
            if positional < N {
                slots[positional] = Some(arg.value.clone());
            }
            positional += 1;
        }
    }
    for arg in args {
        let Some(name) = &arg.name else {
            continue;
        };
        match params.iter().position(|param| param == name) {
            Some(i) if slots[i].is_some() => {
                return Err(type_error(format!(
                    "{qualname}() got multiple values for argument '{name}'"
                )));
            }
            Some(i) => slots[i] = Some(arg.value.clone()),
            None => {
                return Err(type_error(format!(
                    "{qualname}() got an unexpected keyword argument '{name}'"
                )));
            }
        }
    }
    if positional > N {
        let (takes, plural) = if defaults > 0 {
            (format!("from {} to {N}", N - defaults), "s")
        } else {
            (N.to_string(), plural(N))
        };
        return Err(type_error(format!(
            "{qualname}() takes {takes} positional argument{plural} but {positional} {} given",
            if positional == 1 { "was" } else { "were" }
        )));
    }
    let missing: Vec<&str> = (0..N - defaults)
        .filter(|&i| slots[i].is_none())
        .map(|i| params[i])
        .collect();
    if !missing.is_empty() {
        return Err(type_error(format!(
            "{qualname}() missing {} required positional argument{}: {}",
            missing.len(),
            plural(missing.len()),
            format_names(&missing)
        )));
    }
    Ok(slots)
}

/// A bound required argument.
fn required(slot: Option<Value>) -> Value {
    slot.unwrap_or_else(Value::none)
}

// ── Selects and contexts ─────────────────────────────────────────────────────

/// Native payload of a `_Select` instance.
struct SelectState(Rc<Step>);

/// Wrap a compiled step as a callable `select(context, result)`.
fn new_select(step: Step) -> Result<Value> {
    let select = instance_of(&select_class()?);
    set_native_state(&select, SelectState(Rc::new(step)))?;
    Ok(select)
}

/// The compiled step of a native select, if `value` is one.
fn select_step(value: &Value) -> Option<Rc<Step>> {
    if !has_native_state::<SelectState>(value) {
        return None;
    }
    with_native_state(value, SELECT_CLASS_NAME, |st: &SelectState| {
        Ok(Rc::clone(&st.0))
    })
    .ok()
}

/// `_SelectorContext(root)`.
fn new_context(interp: &mut Interpreter, root: &Value) -> Result<Value> {
    let context = instance_of(&context_class()?);
    interp.assign_attr(context.clone(), "root", root.clone())?;
    Ok(context)
}

/// `get_parent_map(context)`.
fn get_parent_map_impl(interp: &mut Interpreter, context: &Value) -> Result<Value> {
    let parent_map = interp.get_attr(context, "parent_map")?;
    if !parent_map.is_none() {
        return Ok(parent_map);
    }
    let parent_map = Value::dict(PyDict::default());
    interp.assign_attr(context.clone(), "parent_map", parent_map.clone())?;
    let root = interp.get_attr(context, "root")?;
    let walk = call_method(interp, &root, "iter", &[])?;
    let walk = crate::interpreter::make_iterator(interp, &walk)?;
    while let Some(p) = next_item(interp, &walk)? {
        let children = crate::interpreter::make_iterator(interp, &p)?;
        while let Some(e) = next_item(interp, &children)? {
            let key = interp.value_to_pykey(&e)?;
            interp.dict_insert_value(&parent_map, key, p.clone())?;
        }
    }
    Ok(parent_map)
}

// ── iterfind / find / findall / findtext ─────────────────────────────────────

/// The step compiler registered for a path operator.
enum Prepare {
    Native(NativePrepare),
    Python(Value),
}

/// `ops[op]`.
fn lookup_op(interp: &mut Interpreter, ops: Option<&Value>, op: &Value) -> Result<Prepare> {
    let entry = match ops {
        Some(ops) => getitem(interp, ops, op.clone())?,
        None => {
            let native = str_text(op).and_then(|text| {
                OPS.iter()
                    .find(|(name, _)| *name == text)
                    .map(|&(_, prepare)| prepare)
            });
            return native
                .map(Prepare::Native)
                .ok_or_else(|| key_error(op.clone()));
        }
    };
    Ok(match NativePrepare::of(&entry) {
        Some(native) => Prepare::Native(native),
        None => Prepare::Python(entry),
    })
}

/// Compile `path` into a selector list; `None` for a path without tokens.
fn compile(
    interp: &mut Interpreter,
    path: &Value,
    namespaces: &Value,
    ops: Option<&Value>,
) -> Result<Option<Value>> {
    let tokens = tokenizer::make_tokenizer(path.clone(), namespaces.clone());
    let feed = Feed::Iter(tokens.clone());
    let mut next_callable: Option<Value> = None;
    let Some(mut token) = feed.pull(interp)? else {
        return Ok(None);
    };
    let mut selector = Vec::new();
    loop {
        // selector.append(ops[token[0]](next, token))
        let produced = (|| -> Result<Value> {
            let op = select::token_item(interp, &token, 0)?;
            match lookup_op(interp, ops, &op)? {
                Prepare::Native(native) => native.run(interp, &feed, &token),
                Prepare::Python(prepare) => {
                    let next = match &next_callable {
                        Some(next) => next.clone(),
                        None => {
                            let next = interp.get_attr(&tokens, "__next__")?;
                            next_callable = Some(next.clone());
                            next
                        }
                    };
                    call(interp, prepare, &[next, token.clone()])
                }
            }
        })();
        match produced {
            Ok(select) => selector.push(select),
            Err(err) if is_stop_iteration_error(&err) => {
                return Err(syntax_error("invalid path"));
            }
            Err(err) => return Err(err),
        }
        let Some(next_token) = feed.pull(interp)? else {
            break;
        };
        token = next_token;
        let op = select::token_item(interp, &token, 0)?;
        if py_eq(interp, &op, &Value::string("/"))? {
            let Some(next_token) = feed.pull(interp)? else {
                break;
            };
            token = next_token;
        }
    }
    Ok(Some(Value::list(selector)))
}

/// Run a selector list over `elem`.
fn execute(interp: &mut Interpreter, elem: &Value, selector: &Value) -> Result<Value> {
    let selects: Vec<Value> = match selector.as_list() {
        Some(items) => items.clone(),
        None => interp.collect_iterable(selector)?,
    };
    let mut result = Value::list(vec![elem.clone()]);
    if selects.is_empty() {
        return Ok(result);
    }
    let context = new_context(interp, elem)?;
    for select in selects {
        result = match select_step(&select) {
            Some(step) => select::make_select_iter(step, context.clone(), result),
            None => call(interp, select, &[context.clone(), result])?,
        };
    }
    Ok(result)
}

/// `iterfind(elem, path, namespaces)`.
fn iterfind_impl(
    interp: &mut Interpreter,
    elem: &Value,
    path: &Value,
    namespaces: &Value,
) -> Result<Value> {
    // if path[-1:] == "/": path = path + "*"  # implicit all
    let path = match str_text(path) {
        Some(text) if text.ends_with('/') => Value::string(format!("{text}*")),
        Some(_) => path.clone(),
        None if slice_eq(interp, path, Some(-1), None, "/")? => {
            interp.eval_binary(path.clone(), BinaryOp::Add, Value::string("*"))?
        }
        None => path.clone(),
    };
    // cache_key = (path,) + tuple(sorted(namespaces.items()))
    let mut key_items = vec![path.clone()];
    if truthy(interp, namespaces)? {
        let items = call_method(interp, namespaces, "items", &[])?;
        let sorted = builtin(interp, "sorted")?;
        let sorted = call(interp, sorted, &[items])?;
        key_items.extend(interp.collect_iterable(&sorted)?);
    }
    let cache_key = Value::tuple(key_items);
    let (cache, ops) = module_tables();
    let cache = cache.filter(Value::is_dict);
    let cached = match &cache {
        Some(cache) => interp
            .dict_lookup_value(cache, &cache_key)?
            .map(|(_, selector)| selector),
        None => {
            interp.value_to_pykey(&cache_key)?;
            None
        }
    };
    let selector = match cached {
        Some(selector) => selector,
        None => {
            if let Some(cache) = &cache
                && cache.as_dict().is_some_and(|dict| dict.len() > 100)
            {
                call_method(interp, cache, "clear", &[])?;
            }
            let absolute = match str_text(&path) {
                Some(text) => text.starts_with('/'),
                None => slice_eq(interp, &path, None, Some(1), "/")?,
            };
            if absolute {
                return Err(syntax_error("cannot use absolute path on element"));
            }
            let Some(selector) = compile(interp, &path, namespaces, ops.as_ref())? else {
                return Ok(Value::none());
            };
            if let Some(cache) = &cache {
                let key = interp.value_to_pykey(&cache_key)?;
                interp.dict_insert_value(cache, key, selector.clone())?;
            }
            selector
        }
    };
    execute(interp, elem, &selector)
}

/// `findtext(elem, path, default, namespaces)`.
fn findtext_impl(
    interp: &mut Interpreter,
    elem: &Value,
    path: &Value,
    default: Value,
    namespaces: &Value,
) -> Result<Value> {
    let found = iterfind_impl(interp, elem, path, namespaces)
        .and_then(|matches| interp.call_next(&matches, None))
        .and_then(|found| interp.get_attr(&found, "text"));
    match found {
        Ok(text) if text.is_none() => Ok(Value::string("")),
        Ok(text) => Ok(text),
        Err(err) if is_stop_iteration_error(&err) => Ok(default),
        Err(err) => Err(err),
    }
}

// ── Hidden class methods ─────────────────────────────────────────────────────

/// `select(context, result)`.
fn select_call(args: &[ExpandedCallArg]) -> Result<Value> {
    let Some((this, rest)) = args.split_first() else {
        return Err(type_error(
            "select() missing 2 required positional arguments",
        ));
    };
    let Some(step) = select_step(&this.value) else {
        return Err(type_error(
            "select() requires a compiled ElementPath select",
        ));
    };
    let [context, result] = bind(step.qualname(), ["context", "result"], 0, rest)?;
    Ok(select::make_select_iter(
        step,
        required(context),
        required(result),
    ))
}

/// `<function prepare_child.<locals>.select at 0x…>`.
fn select_repr(args: &[ExpandedCallArg]) -> Result<Value> {
    let Some(this) = args.first() else {
        return Err(type_error(
            "__repr__() missing 1 required positional argument",
        ));
    };
    let qualname = select_step(&this.value).map_or("select", |step| step.qualname());
    let addr = this.value.value_id().unwrap_or(0);
    Ok(Value::string(format!(
        "<function {qualname} at 0x{addr:x}>"
    )))
}

// ── Module wiring ────────────────────────────────────────────────────────────

/// Finish the module after it is built: CPython's `import re`,
/// `xpath_tokenizer_re`, `ops`, `_cache`, the hidden select class, and the
/// namespace order.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let re_mod = interp.load_module("re")?;
    let compile_fn = interp.get_attr(&re_mod, "compile")?;
    let tokenizer_re = call(
        interp,
        compile_fn,
        &[Value::string(XPATH_TOKENIZER_PATTERN)],
    )?;

    let (select_class, context_class, prepares) = {
        let mut m = module.borrow_mut();
        let class_of = |value: Option<Value>, name: &str| match value.as_ref().map(Value::kind) {
            Some(ValueKind::PyClass(class)) => Ok(Rc::clone(class)),
            _ => Err(PyError::runtime(format!(
                "xml.etree.ElementPath: missing class {name}"
            ))),
        };
        let select_class = class_of(m.attrs.shift_remove(SELECT_CLASS_NAME), SELECT_CLASS_NAME)?;
        let context_class = class_of(m.attrs.get("_SelectorContext").cloned(), "_SelectorContext")?;
        let prepares: Vec<(&str, Option<Value>)> = OPS
            .iter()
            .map(|&(op, prepare)| (op, m.attrs.get(prepare.py_name()).cloned()))
            .collect();
        (select_class, context_class, prepares)
    };
    STATE.with(|state| {
        *state.borrow_mut() = Some(ModuleState {
            module: Rc::downgrade(module),
            select_class,
            context_class,
        });
    });

    let ops = Value::dict(PyDict::default());
    for (op, prepare) in prepares {
        let prepare = prepare.ok_or_else(|| {
            PyError::runtime("xml.etree.ElementPath: missing prepare function".into())
        })?;
        interp.dict_set_value(&ops, Value::string(op), prepare)?;
    }

    let mut m = module.borrow_mut();
    m.attrs.insert("re".to_string(), re_mod);
    m.attrs
        .insert("xpath_tokenizer_re".to_string(), tokenizer_re);
    m.attrs.insert("ops".to_string(), ops);
    m.attrs
        .insert("_cache".to_string(), Value::dict(PyDict::default()));
    for name in NAMESPACE_ORDER {
        if let Some(value) = m.attrs.shift_remove(name) {
            m.attrs.insert(name.to_string(), value);
        }
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__doc__" => Value::none(),
        "__package__" => Value::string("xml.etree"),
    }

    fn xpath_tokenizer(args) -> Result<Value> {
        let [pattern, namespaces] = bind("xpath_tokenizer", ["pattern", "namespaces"], 1, args)?;
        Ok(tokenizer::make_tokenizer(
            required(pattern),
            namespaces.unwrap_or_else(Value::none),
        ))
    }

    fn get_parent_map(args) -> Result<Value> {
        let [context] = bind("get_parent_map", ["context"], 0, args)?;
        get_parent_map_impl(_interp, &required(context))
    }

    fn _is_wildcard_tag(args) -> Result<Value> {
        let [tag] = bind("_is_wildcard_tag", ["tag"], 0, args)?;
        Ok(Value::bool_(select::is_wildcard_tag(_interp, &required(tag))?))
    }

    fn _prepare_tag(args) -> Result<Value> {
        let [tag] = bind("_prepare_tag", ["tag"], 0, args)?;
        let filter = select::prepare_tag(_interp, &required(tag))?;
        new_select(Step::Filter(filter))
    }

    fn prepare_child(args) -> Result<Value> {
        let [next, token] = bind("prepare_child", ["next", "token"], 0, args)?;
        let feed = Feed::Call(required(next));
        NativePrepare::Child.run(_interp, &feed, &required(token))
    }

    fn prepare_star(args) -> Result<Value> {
        let [next, token] = bind("prepare_star", ["next", "token"], 0, args)?;
        let feed = Feed::Call(required(next));
        NativePrepare::Star.run(_interp, &feed, &required(token))
    }

    fn prepare_self(args) -> Result<Value> {
        let [next, token] = bind("prepare_self", ["next", "token"], 0, args)?;
        let feed = Feed::Call(required(next));
        NativePrepare::SelfStep.run(_interp, &feed, &required(token))
    }

    fn prepare_descendant(args) -> Result<Value> {
        let [next, token] = bind("prepare_descendant", ["next", "token"], 0, args)?;
        let feed = Feed::Call(required(next));
        NativePrepare::Descendant.run(_interp, &feed, &required(token))
    }

    fn prepare_parent(args) -> Result<Value> {
        let [next, token] = bind("prepare_parent", ["next", "token"], 0, args)?;
        let feed = Feed::Call(required(next));
        NativePrepare::Parent.run(_interp, &feed, &required(token))
    }

    fn prepare_predicate(args) -> Result<Value> {
        let [next, token] = bind("prepare_predicate", ["next", "token"], 0, args)?;
        let feed = Feed::Call(required(next));
        NativePrepare::Predicate.run(_interp, &feed, &required(token))
    }

    fn iterfind(args) -> Result<Value> {
        let [elem, path, namespaces] = bind("iterfind", ["elem", "path", "namespaces"], 1, args)?;
        let namespaces = namespaces.unwrap_or_else(Value::none);
        iterfind_impl(_interp, &required(elem), &required(path), &namespaces)
    }

    fn find(args) -> Result<Value> {
        let [elem, path, namespaces] = bind("find", ["elem", "path", "namespaces"], 1, args)?;
        let namespaces = namespaces.unwrap_or_else(Value::none);
        let matches = iterfind_impl(_interp, &required(elem), &required(path), &namespaces)?;
        _interp.call_next(&matches, Some(Value::none()))
    }

    fn findall(args) -> Result<Value> {
        let [elem, path, namespaces] = bind("findall", ["elem", "path", "namespaces"], 1, args)?;
        let namespaces = namespaces.unwrap_or_else(Value::none);
        let matches = iterfind_impl(_interp, &required(elem), &required(path), &namespaces)?;
        Ok(Value::list(_interp.collect_iterable(&matches)?))
    }

    fn findtext(args) -> Result<Value> {
        let [elem, path, default, namespaces] = bind(
            "findtext",
            ["elem", "path", "default", "namespaces"],
            2,
            args,
        )?;
        findtext_impl(
            _interp,
            &required(elem),
            &required(path),
            default.unwrap_or_else(Value::none),
            &namespaces.unwrap_or_else(Value::none),
        )
    }

    class _SelectorContext {
        constants {
            "parent_map" => Value::none(),
        }

        fn __init__(args) -> Result<Value> {
            let [this, root] = bind("_SelectorContext.__init__", ["self", "root"], 0, args)?;
            _interp.assign_attr(required(this), "root", required(root))?;
            Ok(Value::none())
        }
    }

    #[native_state(SelectState)]
    class _Select {
        fn __call__(args) -> Result<Value> {
            select_call(args)
        }

        fn __repr__(args) -> Result<Value> {
            select_repr(args)
        }

        #[getter("__name__")]
        fn name(#[state] st: &SelectState) -> Result<Value> {
            let qualname = st.0.qualname();
            Ok(Value::string(qualname.rsplit('.').next().unwrap_or(qualname)))
        }

        #[getter("__qualname__")]
        fn qualname(#[state] st: &SelectState) -> Result<Value> {
            Ok(Value::string(st.0.qualname()))
        }
    }
}
