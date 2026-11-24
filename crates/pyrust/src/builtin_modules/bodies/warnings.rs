// `warnings` module — CPython 3.12-compatible implementation of Python's
// `warnings` module.
//
// Provides `warn()`, `warn_explicit()`, `filterwarnings()`, `simplefilter()`,
// `resetwarnings()`, `formatwarning()`, `showwarning()`, `catch_warnings`,
// `WarningMessage`, and live `filters` / `onceregistry` / `defaultaction`.
//
// ## Design
//
// ### Live filter list & interpreter isolation
//
// `WarningsState` is owned per root `Interpreter` behind an `Rc` (shared with
// filesystem-import child interpreters because both represent one Python
// interpreter). Its `filters` field holds the live Python `list` exposed as
// `warnings.filters`, and `onceregistry` holds the live `dict` exposed as
// `warnings.onceregistry`. Mutations to `warnings.filters` (in-place or by
// rebinding `warnings.filters = [...]` on the module) take effect immediately.
//
// ### Caller frame resolution
//
// `warn()` resolves `(filename, lineno, module, globals)` from PyRust's live
// frame views / call-site line stacks using `stacklevel` (default 1), falling
// back to `("sys", 1, "sys")` when `stacklevel` exceeds the active call stack.
//
// ### Monkey-patching & catch_warnings
//
// `warn_explicit()` consults the live `warnings.showwarning` attribute on the
// `warnings` module; `showwarning()` consults `warnings.formatwarning`. Inside
// `catch_warnings(record=True)`, `showwarning` is temporarily reset to the
// default hook and warnings append `WarningMessage` instances to the active
// recording sink unless user code replaces `warnings.showwarning` inside the
// `with` block.
//
// Reference: <https://docs.python.org/3/library/warnings.html>

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, class_is_subclass_of, is_builtin_callable_adapter, lookup_class_attr,
    lookup_exc_class, render_instance_str, render_value_repr,
};
use crate::value::{InstanceAttrs, PyClass, PyDict, PyInstance, PyKey, Value, ValueKind};
use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

// ── interpreter-scoped state ──────────────────────────────────────────────────

struct RecordSink {
    log: Value,
    warning_message_class: Rc<RefCell<PyClass>>,
}

/// Mutable state of the `warnings` module for one Python interpreter.
///
/// `Interpreter` owns this behind an `Rc`: roots get fresh state, while the
/// implementation-only child used for a filesystem import clones the handle.
pub(crate) struct WarningsState {
    /// Live Python `list` of 5-tuple filter entries:
    /// `(action, message_pattern, category, module_pattern, lineno)`.
    filters: RefCell<Value>,
    /// Live Python `dict` backing the `"once"` filter action.
    onceregistry: RefCell<Value>,
    /// Monotonic version counter bumped whenever filters are mutated via the
    /// warnings API, invalidating per-module `__warningregistry__` caches.
    filters_version: Cell<u64>,
    /// Active `catch_warnings(record=True)` sinks in nesting order.
    record_sinks: RefCell<Vec<RecordSink>>,
}

impl Default for WarningsState {
    fn default() -> Self {
        Self {
            filters: RefCell::new(Value::list(Vec::new())),
            onceregistry: RefCell::new(Value::dict(PyDict::default())),
            filters_version: Cell::new(0),
            record_sinks: RefCell::new(Vec::new()),
        }
    }
}

impl WarningsState {
    fn bump_version(&self) {
        self.filters_version
            .set(self.filters_version.get().wrapping_add(1));
    }

    fn version(&self) -> u64 {
        self.filters_version.get()
    }
}

// ── live module accessors ─────────────────────────────────────────────────────

fn dict_get_str(dict_val: &Value, key: &str) -> Option<Value> {
    dict_val
        .as_dict()
        .and_then(|d| d.get(&PyKey::str_from(key)).cloned())
}

fn value_is_callable(value: &Value) -> bool {
    match value.kind() {
        ValueKind::UserFunction(_)
        | ValueKind::BuiltinFunction(_)
        | ValueKind::BoundMethod { .. }
        | ValueKind::ClassBoundMethod { .. }
        | ValueKind::PyClass(_) => true,
        ValueKind::BuiltinObject { .. } => is_builtin_callable_adapter(value),
        ValueKind::PyInstance(inst) => {
            let class = Rc::clone(&inst.borrow().class);
            lookup_class_attr(&class, "__call__").is_some()
        }
        _ => false,
    }
}

fn warnings_module_rc(interp: &crate::Interpreter) -> Option<Rc<RefCell<crate::value::PyModule>>> {
    let modules_dict = interp.import_module_registry().ok()?;
    let warn_mod = dict_get_str(&modules_dict, "warnings")?;
    match warn_mod.kind() {
        ValueKind::PyModule(rc) => Some(Rc::clone(rc)),
        _ => None,
    }
}

fn active_filters_value(interp: &crate::Interpreter) -> Value {
    if let Some(module) = warnings_module_rc(interp)
        && let Some(filters) = module.borrow().attrs.get("filters").cloned()
    {
        return filters;
    }
    interp.warnings_state.filters.borrow().clone()
}

fn set_active_filters_value(interp: &crate::Interpreter, new_filters: Value) {
    *interp.warnings_state.filters.borrow_mut() = new_filters.clone();
    if let Some(module) = warnings_module_rc(interp) {
        module
            .borrow_mut()
            .attrs
            .insert("filters".to_string(), new_filters);
    }
}

fn active_onceregistry(interp: &crate::Interpreter) -> Value {
    if let Some(module) = warnings_module_rc(interp)
        && let Some(reg) = module.borrow().attrs.get("onceregistry").cloned()
    {
        return reg;
    }
    interp.warnings_state.onceregistry.borrow().clone()
}

fn active_defaultaction(interp: &crate::Interpreter) -> String {
    if let Some(module) = warnings_module_rc(interp)
        && let Some(val) = module.borrow().attrs.get("defaultaction")
        && let Some(s) = val.as_str()
    {
        return s.to_string();
    }
    "default".to_string()
}

fn active_module_attr(interp: &crate::Interpreter, name: &str) -> Option<Value> {
    let module = warnings_module_rc(interp)?;
    module.borrow().attrs.get(name).cloned()
}

fn set_module_attr(interp: &crate::Interpreter, name: &str, value: Value) {
    if let Some(module) = warnings_module_rc(interp) {
        module.borrow_mut().attrs.insert(name.to_string(), value);
    }
}

// ── warnings-owned classes ───────────────────────────────────────────────────

type WeakPyClass = Weak<RefCell<PyClass>>;

struct WarningClassGeneration {
    warning_message: Rc<RefCell<PyClass>>,
    catch_warnings: WeakPyClass,
}

// Thread-local justification:
// (a) Why `thread_local!`: Holds `!Send` `Rc`/`Weak<RefCell<PyClass>>`
//     generation pairs linking each `warnings.catch_warnings` class to its
//     sibling `warnings.WarningMessage` class.
// (b) Multi-interpreter / multi-threaded impact: Weak `catch_warnings` keys
//     isolate distinct `warnings` imports across interpreters on one thread
//     and prune dead generations; separate threads are isolated.
thread_local! {
    static WARNING_CLASS_GENERATIONS: RefCell<Vec<WarningClassGeneration>> =
        const { RefCell::new(Vec::new()) };
}

fn new_warning_message_class() -> Value {
    let mut attrs: IndexMap<String, Value> = IndexMap::new();
    attrs.insert(
        "__init__".to_string(),
        Value::builtin_function("warnings.WarningMessage.__init__"),
    );
    attrs.insert(
        "__str__".to_string(),
        Value::builtin_function("warnings.WarningMessage.__str__"),
    );
    Value::py_class(Rc::new(RefCell::new(PyClass::new(
        "WarningMessage",
        "WarningMessage",
        None,
        attrs,
    ))))
}

fn new_catch_warnings_class() -> Value {
    let mut attrs: IndexMap<String, Value> = IndexMap::new();
    attrs.insert(
        "__init__".to_string(),
        Value::builtin_function("warnings.CatchWarnings.__init__"),
    );
    attrs.insert(
        "__repr__".to_string(),
        Value::builtin_function("warnings.CatchWarnings.__repr__"),
    );
    attrs.insert(
        "__enter__".to_string(),
        Value::builtin_function("warnings.CatchWarnings.__enter__"),
    );
    attrs.insert(
        "__exit__".to_string(),
        Value::builtin_function("warnings.CatchWarnings.__exit__"),
    );
    Value::py_class(Rc::new(RefCell::new(PyClass::new(
        "catch_warnings",
        "catch_warnings",
        None,
        attrs,
    ))))
}

/// Finalize one imported warnings generation: attach this interpreter's live
/// `filters` list / `onceregistry` dict and register the module's class pair.
pub(crate) fn prepare_module(interpreter: &crate::Interpreter, module: &Value) {
    let ValueKind::PyModule(module) = module.kind() else {
        return;
    };
    {
        let mut m = module.borrow_mut();
        m.attrs.insert(
            "filters".to_string(),
            interpreter.warnings_state.filters.borrow().clone(),
        );
        m.attrs.insert(
            "onceregistry".to_string(),
            interpreter.warnings_state.onceregistry.borrow().clone(),
        );
    }
    let (warning_message, catch_warnings) = {
        let module = module.borrow();
        let warning_message =
            module
                .attrs
                .get("WarningMessage")
                .and_then(|value| match value.kind() {
                    ValueKind::PyClass(class) => Some(Rc::clone(class)),
                    _ => None,
                });
        let catch_warnings =
            module
                .attrs
                .get("catch_warnings")
                .and_then(|value| match value.kind() {
                    ValueKind::PyClass(class) => Some(Rc::clone(class)),
                    _ => None,
                });
        (warning_message, catch_warnings)
    };
    let (Some(warning_message), Some(catch_warnings)) = (warning_message, catch_warnings) else {
        return;
    };

    for class in [&warning_message, &catch_warnings] {
        class
            .borrow_mut()
            .attrs
            .insert("__module__".to_string(), Value::string("warnings"));
    }

    WARNING_CLASS_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        generations.retain(|generation| generation.catch_warnings.strong_count() > 0);
        if generations.iter().any(|generation| {
            Rc::ptr_eq(&generation.warning_message, &warning_message)
                && generation.catch_warnings.as_ptr() == Rc::as_ptr(&catch_warnings)
        }) {
            return;
        }
        generations.push(WarningClassGeneration {
            warning_message,
            catch_warnings: Rc::downgrade(&catch_warnings),
        });
    });
}

fn warning_message_class_for_catch(class: &Rc<RefCell<PyClass>>) -> Option<Rc<RefCell<PyClass>>> {
    WARNING_CLASS_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        generations.retain(|generation| generation.catch_warnings.strong_count() > 0);
        generations.iter().rev().find_map(|generation| {
            let catch_warnings = generation.catch_warnings.upgrade()?;
            class_is_subclass_of(class, &catch_warnings)
                .then(|| Rc::clone(&generation.warning_message))
        })
    })
}

fn context_warning_message_class(
    inst: &Rc<RefCell<PyInstance>>,
    fn_name: &str,
) -> Result<Rc<RefCell<PyClass>>> {
    let cached = inst.borrow().attrs.get("_warning_message_class").cloned();
    if let Some(value) = cached
        && let ValueKind::PyClass(class) = value.kind()
    {
        return Ok(Rc::clone(class));
    }

    let context_class = Rc::clone(&inst.borrow().class);
    let warning_message = warning_message_class_for_catch(&context_class).ok_or_else(|| {
        PyError::runtime(format!(
            "internal: {fn_name}() cannot resolve its warnings module generation",
        ))
    })?;
    inst.borrow_mut().attrs.insert(
        "_warning_message_class",
        Value::py_class(Rc::clone(&warning_message)),
    );
    Ok(warning_message)
}

#[allow(clippy::too_many_arguments)]
fn make_warning_message(
    warning_message_class: &Rc<RefCell<PyClass>>,
    message: Value,
    category: &Rc<RefCell<PyClass>>,
    filename: &str,
    lineno: i64,
    file: Value,
    line: Value,
    source: Value,
) -> Value {
    let category_name = category.borrow().name.clone();
    let mut attrs = InstanceAttrs::new();
    attrs.insert("message", message);
    attrs.insert("category", Value::py_class(Rc::clone(category)));
    attrs.insert("filename", Value::string(filename));
    attrs.insert("lineno", Value::int(lineno));
    attrs.insert("file", file);
    attrs.insert("line", line);
    attrs.insert("source", source);
    attrs.insert("_category_name", Value::string(category_name));
    Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(warning_message_class),
        attrs,
    })))
}

// ── argument & validation helpers ─────────────────────────────────────────────

const VALID_ACTIONS: [&str; 6] = ["error", "ignore", "always", "default", "module", "once"];

fn expect_self(args: &[ExpandedCallArg], fn_name: &str) -> Result<Rc<RefCell<PyInstance>>> {
    match args.first().map(|a| a.value.kind()) {
        Some(ValueKind::PyInstance(rc)) => Ok(Rc::clone(rc)),
        _ => Err(PyError::runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

fn built_in_warning_class(name: &str) -> Result<Rc<RefCell<PyClass>>> {
    lookup_exc_class(name)
        .ok_or_else(|| PyError::runtime(format!("built-in exception '{name}' is not defined")))
}

fn positional_or_keyword(
    args: &[ExpandedCallArg],
    position: usize,
    keyword: &str,
) -> Option<Value> {
    args.iter()
        .find(|argument| argument.name.as_deref() == Some(keyword))
        .map(|argument| argument.value.clone())
        .or_else(|| {
            args.iter()
                .filter(|argument| argument.name.is_none())
                .nth(position)
                .map(|argument| argument.value.clone())
        })
}

fn validate_action(interp: &mut crate::Interpreter, val: &Value) -> Result<String> {
    if let Some(s) = val.as_str()
        && VALID_ACTIONS.contains(&s)
    {
        return Ok(s.to_string());
    }
    let repr = render_value_repr(interp, val)?;
    Err(PyError::named(
        "AssertionError",
        format!("invalid action: {repr}"),
    ))
}

fn validate_lineno_nonneg(val: &Value) -> Result<i64> {
    let n = match val.kind() {
        ValueKind::Int(i) => i,
        ValueKind::Bool(b) => i64::from(b),
        _ => {
            return Err(PyError::named(
                "AssertionError",
                "lineno must be an int >= 0",
            ));
        }
    };
    if n < 0 {
        return Err(PyError::named(
            "AssertionError",
            "lineno must be an int >= 0",
        ));
    }
    Ok(n)
}

fn validate_filter_category(value: &Value) -> Result<Value> {
    let ValueKind::PyClass(category) = value.kind() else {
        return Err(PyError::named("AssertionError", "category must be a class"));
    };
    let warning = built_in_warning_class("Warning")?;
    if !class_is_subclass_of(category, &warning) {
        return Err(PyError::named(
            "AssertionError",
            "category must be a Warning subclass",
        ));
    }
    Ok(value.clone())
}

fn compile_regex(
    interp: &mut crate::Interpreter,
    pattern: &str,
    ignorecase: bool,
) -> Result<Value> {
    let re_mod = interp.load_module("re")?;
    let compile_fn = interp.get_attr(&re_mod, "compile")?;
    let mut call_args = vec![ExpandedCallArg {
        name: None,
        value: Value::string(pattern),
    }];
    if ignorecase {
        let flag_i = interp.get_attr(&re_mod, "IGNORECASE")?;
        call_args.push(ExpandedCallArg {
            name: None,
            value: flag_i,
        });
    }
    interp.call_function_expanded(compile_fn, &call_args)
}

fn add_filter_entry(interp: &mut crate::Interpreter, item: Value, append: bool) -> Result<()> {
    let filters = active_filters_value(interp);
    if !append {
        // Remove an equal duplicate if present, then insert at index 0.
        let remove_fn = interp.get_attr(&filters, "remove")?;
        let _ = interp.call_function_expanded(
            remove_fn,
            &[ExpandedCallArg {
                name: None,
                value: item.clone(),
            }],
        );
        let insert_fn = interp.get_attr(&filters, "insert")?;
        interp.call_function_expanded(
            insert_fn,
            &[
                ExpandedCallArg {
                    name: None,
                    value: Value::int(0),
                },
                ExpandedCallArg {
                    name: None,
                    value: item,
                },
            ],
        )?;
    } else {
        let in_val = interp.eval_in(filters.clone(), item.clone())?;
        let already_in = interp.truthy_value(&in_val)?;
        if !already_in {
            let append_fn = interp.get_attr(&filters, "append")?;
            interp.call_function_expanded(
                append_fn,
                &[ExpandedCallArg {
                    name: None,
                    value: item,
                }],
            )?;
        }
    }
    interp.warnings_state.bump_version();
    Ok(())
}

// ── warning normalization & matching ──────────────────────────────────────────

fn normalize_warning(
    interp: &mut crate::Interpreter,
    message: Value,
    category: Option<Value>,
) -> Result<(Value, Rc<RefCell<PyClass>>)> {
    let warning = built_in_warning_class("Warning")?;
    let message_class = match message.kind() {
        ValueKind::PyInstance(instance) => Some(Rc::clone(&instance.borrow().class)),
        _ => None,
    };
    if let Some(message_class) = message_class
        && class_is_subclass_of(&message_class, &warning)
    {
        return Ok((message, message_class));
    }

    let category = match category {
        None => Value::py_class(built_in_warning_class("UserWarning")?),
        Some(value) if value.is_none() => Value::py_class(built_in_warning_class("UserWarning")?),
        Some(value) => value,
    };
    let ValueKind::PyClass(category_class) = category.kind() else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "category must be a Warning subclass, not '{}'",
                pyrust_core::builtin_type_name(&category)
            ),
        ));
    };
    let category_class = Rc::clone(category_class);
    if !class_is_subclass_of(&category_class, &warning) {
        return Err(PyError::named(
            "TypeError",
            "category must be a Warning subclass, not 'type'",
        ));
    }
    let warning_instance = interp.call_function_expanded(
        Value::py_class(Rc::clone(&category_class)),
        &[ExpandedCallArg {
            name: None,
            value: message,
        }],
    )?;
    Ok((warning_instance, category_class))
}

fn category_matches_filter(category: &Rc<RefCell<PyClass>>, filter_cat: &Value) -> Result<bool> {
    match filter_cat.kind() {
        ValueKind::PyClass(expected) => Ok(class_is_subclass_of(category, expected)),
        ValueKind::Tuple(items) => {
            for item in items.iter() {
                if category_matches_filter(category, item)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => Err(PyError::named(
            "TypeError",
            "issubclass() arg 2 must be a class, a tuple of classes, or a union",
        )),
    }
}

fn regex_field_matches(
    interp: &mut crate::Interpreter,
    matcher: &Value,
    target: &str,
) -> Result<bool> {
    if matcher.is_none() {
        return Ok(true);
    }
    let match_fn = interp.get_attr(matcher, "match")?;
    let res = interp.call_function_expanded(
        match_fn,
        &[ExpandedCallArg {
            name: None,
            value: Value::string(target),
        }],
    )?;
    Ok(!res.is_none())
}

/// Evaluate the live `warnings.filters` list and return `(action, matched_item)`.
fn match_live_filters(
    interp: &mut crate::Interpreter,
    text: &str,
    category: &Rc<RefCell<PyClass>>,
    module: &str,
    lineno: i64,
) -> Result<(String, Option<Value>)> {
    let filters = active_filters_value(interp);
    // Snapshot entries so re-entrant regex calls cannot alias the list borrow.
    let entries: Vec<Value> = match filters.kind() {
        ValueKind::List(items) => items.iter().cloned().collect(),
        ValueKind::Tuple(items) => items.to_vec(),
        _ => interp.collect_iterable(&filters)?,
    };

    for item in entries {
        let parts: Vec<Value> = match item.kind() {
            ValueKind::Tuple(t) => t.to_vec(),
            ValueKind::List(l) => l.iter().cloned().collect(),
            _ => {
                return Err(PyError::named(
                    "ValueError",
                    "not enough values to unpack (expected 5)",
                ));
            }
        };
        if parts.len() != 5 {
            return Err(PyError::named(
                "ValueError",
                format!(
                    "not enough values to unpack (expected 5, got {})",
                    parts.len()
                ),
            ));
        }
        let action_val = &parts[0];
        let msg_val = &parts[1];
        let cat_val = &parts[2];
        let mod_val = &parts[3];
        let ln_val = &parts[4];

        if !regex_field_matches(interp, msg_val, text)? {
            continue;
        }
        if !category_matches_filter(category, cat_val)? {
            continue;
        }
        if !regex_field_matches(interp, mod_val, module)? {
            continue;
        }
        let filter_ln = match ln_val.kind() {
            ValueKind::Int(n) => n,
            ValueKind::Bool(b) => i64::from(b),
            _ => -1,
        };
        if filter_ln != 0 && filter_ln != lineno {
            continue;
        }
        let action = match action_val.as_str() {
            Some(s) => s.to_string(),
            None => render_value_repr(interp, action_val)?,
        };
        return Ok((action, Some(item)));
    }
    Ok((active_defaultaction(interp), None))
}

// ── formatting & emission ─────────────────────────────────────────────────────

fn read_source_line(filename: &str, lineno: i64) -> Option<String> {
    if filename == "sys" || filename.starts_with('<') || lineno < 1 {
        return None;
    }
    let contents = std::fs::read_to_string(filename).ok()?;
    let line = contents.lines().nth((lineno - 1) as usize)?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn formatwarning_impl(
    interp: &mut crate::Interpreter,
    message: &Value,
    category: &Value,
    filename: &str,
    lineno: i64,
    line: &Value,
) -> Result<String> {
    let category_name = match category.kind() {
        ValueKind::PyClass(cls) => cls.borrow().name.clone(),
        _ => {
            let attr = interp.get_attr(category, "__name__")?;
            attr.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| "<unknown>".to_string())
        }
    };
    let message_text = render_instance_str(interp, message)?;
    let mut out = format!("{filename}:{lineno}: {category_name}: {message_text}\n");

    let source_line = if line.is_none() {
        read_source_line(filename, lineno)
    } else if let Some(s) = line.as_str() {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    } else {
        let s = render_instance_str(interp, line)?;
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };

    if let Some(src) = source_line {
        out.push_str("  ");
        out.push_str(&src);
        out.push('\n');
    }
    Ok(out)
}

fn format_warning_dispatch(
    interp: &mut crate::Interpreter,
    message: &Value,
    category: &Value,
    filename: &str,
    lineno: i64,
    line: &Value,
) -> Result<String> {
    if let Some(fw) = active_module_attr(interp, "formatwarning")
        && !matches!(
            fw.kind(),
            ValueKind::BuiltinFunction(name) if name == "warnings.formatwarning"
        )
    {
        let formatted = interp.call_function_expanded(
            fw,
            &[
                ExpandedCallArg {
                    name: None,
                    value: message.clone(),
                },
                ExpandedCallArg {
                    name: None,
                    value: category.clone(),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::string(filename),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::int(lineno),
                },
                ExpandedCallArg {
                    name: None,
                    value: line.clone(),
                },
            ],
        )?;
        return match formatted.as_str() {
            Some(s) => Ok(s.to_string()),
            None => render_instance_str(interp, &formatted),
        };
    }
    formatwarning_impl(interp, message, category, filename, lineno, line)
}

fn showwarning_impl(
    interp: &mut crate::Interpreter,
    message: &Value,
    category: &Value,
    filename: &str,
    lineno: i64,
    file: Option<Value>,
    line: Value,
) -> Result<()> {
    let target_file = match file {
        Some(f) if !f.is_none() => Some(f),
        _ => {
            let sys_mod = interp.load_module("sys")?;
            let stderr = interp.get_attr(&sys_mod, "stderr")?;
            if stderr.is_none() { None } else { Some(stderr) }
        }
    };
    let Some(target_file) = target_file else {
        return Ok(());
    };
    let text = format_warning_dispatch(interp, message, category, filename, lineno, &line)?;
    let write_fn = interp.get_attr(&target_file, "write")?;
    let res = interp.call_function_expanded(
        write_fn,
        &[ExpandedCallArg {
            name: None,
            value: Value::string(text),
        }],
    );
    if let Err(err) = res {
        // CPython ignores OSError when writing warnings to an invalid stream.
        if !err.class_name_is("OSError") {
            return Err(err);
        }
    }
    Ok(())
}

fn dispatch_show_warning(
    interp: &mut crate::Interpreter,
    warning_instance: Value,
    category: &Rc<RefCell<PyClass>>,
    filename: &str,
    lineno: i64,
    source: Value,
) -> Result<()> {
    // Respect monkey-patched `warnings.showwarning`.
    if let Some(sw) = active_module_attr(interp, "showwarning")
        && !matches!(
            sw.kind(),
            ValueKind::BuiltinFunction(name) if name == "warnings.showwarning"
        )
    {
        if !value_is_callable(&sw) {
            return Err(PyError::named(
                "TypeError",
                "warnings.showwarning() must be set to a function or method",
            ));
        }
        interp.call_function_expanded(
            sw,
            &[
                ExpandedCallArg {
                    name: None,
                    value: warning_instance,
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::py_class(Rc::clone(category)),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::string(filename),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::int(lineno),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::none(),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::none(),
                },
            ],
        )?;
        return Ok(());
    }

    // If inside an active `catch_warnings(record=True)` block, append directly
    // to the innermost recording sink.
    let recorded = if let Some(record_sink) = interp.warnings_state.record_sinks.borrow().last() {
        let wmsg = make_warning_message(
            &record_sink.warning_message_class,
            warning_instance.clone(),
            category,
            filename,
            lineno,
            Value::none(),
            Value::none(),
            source,
        );
        let _ = record_sink.log.list_push(wmsg);
        true
    } else {
        false
    };
    if !recorded {
        let cat_val = Value::py_class(Rc::clone(category));
        showwarning_impl(
            interp,
            &warning_instance,
            &cat_val,
            filename,
            lineno,
            None,
            Value::none(),
        )?;
    }
    Ok(())
}

fn registry_lookup(interp: &mut crate::Interpreter, registry: &Value, key: &Value) -> Result<bool> {
    if !matches!(registry.kind(), ValueKind::Dict(_)) {
        return Ok(false);
    }
    let get_fn = interp.get_attr(registry, "get")?;
    let val = interp.call_function_expanded(
        get_fn,
        &[ExpandedCallArg {
            name: None,
            value: key.clone(),
        }],
    )?;
    interp.truthy_value(&val)
}

fn registry_set(
    interp: &mut crate::Interpreter,
    registry: &Value,
    key: Value,
    value: Value,
) -> Result<()> {
    if !matches!(registry.kind(), ValueKind::Dict(_)) {
        return Ok(());
    }
    let setitem_fn = interp.get_attr(registry, "__setitem__")?;
    interp.call_function_expanded(
        setitem_fn,
        &[
            ExpandedCallArg {
                name: None,
                value: key,
            },
            ExpandedCallArg { name: None, value },
        ],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn warn_explicit_impl(
    interp: &mut crate::Interpreter,
    message: Value,
    category: Option<Value>,
    filename: &str,
    lineno: i64,
    module: Option<String>,
    registry: Option<Value>,
    source: Value,
) -> Result<Value> {
    let module_name = match module {
        Some(m) => m,
        None => {
            let raw = if filename.is_empty() {
                "<unknown>"
            } else {
                filename
            };
            if raw.len() >= 3 && raw[raw.len() - 3..].eq_ignore_ascii_case(".py") {
                raw[..raw.len() - 3].to_string()
            } else {
                raw.to_string()
            }
        }
    };

    let registry = match registry {
        Some(r) if !r.is_none() => r,
        _ => Value::dict(PyDict::default()),
    };

    // Invalidate per-module warning cache whenever filters were mutated.
    if matches!(registry.kind(), ValueKind::Dict(_)) {
        let cur_ver = interp.warnings_state.version() as i64;
        let stored_ver = dict_get_str(&registry, "version").and_then(|v| match v.kind() {
            ValueKind::Int(n) => Some(n),
            _ => None,
        });
        if stored_ver != Some(cur_ver) {
            let _ = registry.dict_clear();
            let _ = registry.dict_insert(PyKey::str_from("version"), Value::int(cur_ver));
        }
    }

    let (warning_instance, category_class) = normalize_warning(interp, message, category)?;
    let message_text = render_instance_str(interp, &warning_instance)?;
    let cat_val = Value::py_class(Rc::clone(&category_class));

    let key = Value::tuple(vec![
        Value::string(&message_text),
        cat_val.clone(),
        Value::int(lineno),
    ]);
    if registry_lookup(interp, &registry, &key)? {
        return Ok(Value::none());
    }

    let (action, matched_item) =
        match_live_filters(interp, &message_text, &category_class, &module_name, lineno)?;

    match action.as_str() {
        "ignore" => return Ok(Value::none()),
        "error" => return Err(PyError::raised(warning_instance)),
        "once" => {
            registry_set(interp, &registry, key, Value::int(1))?;
            let once_reg = active_onceregistry(interp);
            let once_key = Value::tuple(vec![Value::string(&message_text), cat_val]);
            if registry_lookup(interp, &once_reg, &once_key)? {
                return Ok(Value::none());
            }
            registry_set(interp, &once_reg, once_key, Value::int(1))?;
        }
        "always" => {}
        "module" => {
            registry_set(interp, &registry, key, Value::int(1))?;
            let alt_key = Value::tuple(vec![Value::string(&message_text), cat_val, Value::int(0)]);
            if registry_lookup(interp, &registry, &alt_key)? {
                return Ok(Value::none());
            }
            registry_set(interp, &registry, alt_key, Value::int(1))?;
        }
        "default" => {
            registry_set(interp, &registry, key, Value::int(1))?;
        }
        _ => {
            let item_repr = match matched_item {
                Some(ref item) => render_value_repr(interp, item)?,
                None => "None".to_string(),
            };
            return Err(PyError::named(
                "RuntimeError",
                format!("Unrecognized action ({action:?}) in warnings.filters:\n {item_repr}"),
            ));
        }
    }

    dispatch_show_warning(
        interp,
        warning_instance,
        &category_class,
        filename,
        lineno,
        source,
    )?;
    Ok(Value::none())
}

// ── module ────────────────────────────────────────────────────────────────────

pyrust_module! {
    constants {
        // Warning category classes live in builtins, NOT in the warnings module.
        // CPython 3.12: `hasattr(warnings, 'UserWarning')` is False.

        // Replaced with this interpreter's live list/dict by `prepare_module`.
        "filters"                   => Value::list(Vec::new()),
        "onceregistry"              => Value::dict(PyDict::default()),
        "defaultaction"             => Value::string("default"),

        // Fresh classes per import generation, matching CPython.
        "catch_warnings"            => new_catch_warnings_class(),
        "WarningMessage"            => new_warning_message_class(),
    }

    /// `warnings.warn(message, category=None, stacklevel=1, source=None, *, skip_file_prefixes=())`
    fn warn(args) -> Result<Value> {
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() requires at least 1 argument (0 given)"),
            ));
        }
        let msg_val = positional_or_keyword(args, 0, "message")
            .unwrap_or_else(|| args[0].value.clone());
        let category = positional_or_keyword(args, 1, "category");
        let stacklevel_val = positional_or_keyword(args, 2, "stacklevel");
        let source = positional_or_keyword(args, 3, "source").unwrap_or_else(Value::none);

        if let Some(prefixes) = args
            .iter()
            .find(|a| a.name.as_deref() == Some("skip_file_prefixes"))
            .map(|a| &a.value)
            && !matches!(prefixes.kind(), ValueKind::Tuple(_))
        {
            return Err(PyError::named(
                "TypeError",
                "skip_file_prefixes must be a tuple of strs.",
            ));
        }

        let stacklevel: isize = match stacklevel_val {
            None => 1,
            Some(v) => _interp
                .value_to_isize(&v, "Python int too large to convert to C ssize_t")?
                as isize,
        };

        // Resolve caller frame using PyRust's active frame stack.
        let (filename, lineno, module, registry) =
            if let Some((filename, lineno, module, globals)) =
                _interp.resolve_warning_caller(stacklevel)
            {
                let reg = if matches!(globals.kind(), ValueKind::Dict(_)) {
                    let existing = dict_get_str(&globals, "__warningregistry__");
                    match existing {
                        Some(r) => Some(r),
                        None => {
                            let fresh = Value::dict(PyDict::default());
                            let _ = globals.dict_insert(
                                PyKey::str_from("__warningregistry__"),
                                fresh.clone(),
                            );
                            Some(fresh)
                        }
                    }
                } else {
                    None
                };
                (filename, lineno, Some(module), reg)
            } else {
                ("sys".to_string(), 1i64, Some("sys".to_string()), None)
            };

        warn_explicit_impl(
            _interp,
            msg_val,
            category,
            &filename,
            lineno,
            module,
            registry,
            source,
        )
    }

    /// `warnings.warn_explicit(message, category, filename, lineno, module=None, registry=None, module_globals=None, source=None)`
    fn warn_explicit(args) -> Result<Value> {
        let message = positional_or_keyword(args, 0, "message").ok_or_else(|| {
            PyError::named("TypeError", "warn_explicit() missing required argument: 'message'")
        })?;
        let category = positional_or_keyword(args, 1, "category").ok_or_else(|| {
            PyError::named("TypeError", "warn_explicit() missing required argument: 'category'")
        })?;
        let filename_val = positional_or_keyword(args, 2, "filename").ok_or_else(|| {
            PyError::named("TypeError", "warn_explicit() missing required argument: 'filename'")
        })?;
        let lineno_val = positional_or_keyword(args, 3, "lineno").ok_or_else(|| {
            PyError::named("TypeError", "warn_explicit() missing required argument: 'lineno'")
        })?;
        let filename = match filename_val.as_str() {
            Some(s) => s.to_string(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "warn_explicit() argument 'filename' must be str, not {}",
                        pyrust_core::builtin_type_name(&filename_val)
                    ),
                ));
            }
        };
        let lineno = _interp.value_to_isize(&lineno_val, "Python int too large to convert to C int")?;
        let module = match positional_or_keyword(args, 4, "module") {
            None => None,
            Some(v) if v.is_none() => None,
            Some(v) => match v.as_str() {
                Some(s) => Some(s.to_string()),
                None => Some(render_instance_str(_interp, &v)?),
            },
        };
        let registry = positional_or_keyword(args, 5, "registry");
        // module_globals (pos 6) is accepted for CPython signature parity.
        let _module_globals = positional_or_keyword(args, 6, "module_globals");
        let source = positional_or_keyword(args, 7, "source").unwrap_or_else(Value::none);

        warn_explicit_impl(
            _interp,
            message,
            Some(category),
            &filename,
            lineno,
            module,
            registry,
            source,
        )
    }

    /// `warnings.formatwarning(message, category, filename, lineno, line=None)`
    fn formatwarning(args) -> Result<Value> {
        let message = positional_or_keyword(args, 0, "message").ok_or_else(|| {
            PyError::named("TypeError", "formatwarning() missing required argument: 'message'")
        })?;
        let category = positional_or_keyword(args, 1, "category").ok_or_else(|| {
            PyError::named("TypeError", "formatwarning() missing required argument: 'category'")
        })?;
        let filename_val = positional_or_keyword(args, 2, "filename").ok_or_else(|| {
            PyError::named("TypeError", "formatwarning() missing required argument: 'filename'")
        })?;
        let lineno_val = positional_or_keyword(args, 3, "lineno").ok_or_else(|| {
            PyError::named("TypeError", "formatwarning() missing required argument: 'lineno'")
        })?;
        let line = positional_or_keyword(args, 4, "line").unwrap_or_else(Value::none);
        let filename = match filename_val.as_str() {
            Some(s) => s.to_string(),
            None => render_instance_str(_interp, &filename_val)?,
        };
        let lineno = _interp.value_to_isize(&lineno_val, "Python int too large to convert to C int")?;
        let formatted = formatwarning_impl(_interp, &message, &category, &filename, lineno, &line)?;
        Ok(Value::string(formatted))
    }

    /// `warnings.showwarning(message, category, filename, lineno, file=None, line=None)`
    fn showwarning(args) -> Result<Value> {
        let message = positional_or_keyword(args, 0, "message").ok_or_else(|| {
            PyError::named("TypeError", "showwarning() missing required argument: 'message'")
        })?;
        let category = positional_or_keyword(args, 1, "category").ok_or_else(|| {
            PyError::named("TypeError", "showwarning() missing required argument: 'category'")
        })?;
        let filename_val = positional_or_keyword(args, 2, "filename").ok_or_else(|| {
            PyError::named("TypeError", "showwarning() missing required argument: 'filename'")
        })?;
        let lineno_val = positional_or_keyword(args, 3, "lineno").ok_or_else(|| {
            PyError::named("TypeError", "showwarning() missing required argument: 'lineno'")
        })?;
        let file = positional_or_keyword(args, 4, "file");
        let line = positional_or_keyword(args, 5, "line").unwrap_or_else(Value::none);
        let filename = match filename_val.as_str() {
            Some(s) => s.to_string(),
            None => render_instance_str(_interp, &filename_val)?,
        };
        let lineno = _interp.value_to_isize(&lineno_val, "Python int too large to convert to C int")?;
        showwarning_impl(_interp, &message, &category, &filename, lineno, file, line)?;
        Ok(Value::none())
    }

    /// `warnings.filterwarnings(action, message='', category=Warning, module='', lineno=0, append=False)`
    fn filterwarnings(args) -> Result<Value> {
        let action_val = positional_or_keyword(args, 0, "action").ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!("{FN_NAME}() requires at least 1 argument (0 given)"),
            )
        })?;
        let action = validate_action(_interp, &action_val)?;

        let message_val =
            positional_or_keyword(args, 1, "message").unwrap_or_else(|| Value::string(""));
        let Some(message_str) = message_val.as_str() else {
            return Err(PyError::named(
                "AssertionError",
                "message must be a string",
            ));
        };
        let message_str = message_str.to_string();

        let cat_val = positional_or_keyword(args, 2, "category")
            .unwrap_or(Value::py_class(built_in_warning_class("Warning")?));
        let category = validate_filter_category(&cat_val)?;

        let module_val =
            positional_or_keyword(args, 3, "module").unwrap_or_else(|| Value::string(""));
        let Some(module_str) = module_val.as_str() else {
            return Err(PyError::named(
                "AssertionError",
                "module must be a string",
            ));
        };
        let module_str = module_str.to_string();

        let lineno_val =
            positional_or_keyword(args, 4, "lineno").unwrap_or_else(|| Value::int(0));
        let lineno = validate_lineno_nonneg(&lineno_val)?;

        let append_val =
            positional_or_keyword(args, 5, "append").unwrap_or_else(|| Value::bool_(false));
        let append = _interp.truthy_value(&append_val)?;

        let msg_pat = if message_str.is_empty() {
            Value::none()
        } else {
            compile_regex(_interp, &message_str, true)?
        };
        let mod_pat = if module_str.is_empty() {
            Value::none()
        } else {
            compile_regex(_interp, &module_str, false)?
        };

        let item = Value::tuple(vec![
            Value::string(action),
            msg_pat,
            category,
            mod_pat,
            Value::int(lineno),
        ]);
        add_filter_entry(_interp, item, append)?;
        Ok(Value::none())
    }

    /// `warnings.simplefilter(action, category=Warning, lineno=0, append=False)`
    fn simplefilter(args) -> Result<Value> {
        let action_val = positional_or_keyword(args, 0, "action").ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!("{FN_NAME}() requires at least 1 argument (0 given)"),
            )
        })?;
        let action = validate_action(_interp, &action_val)?;

        // CPython deliberately does not validate `category` here.
        let category = positional_or_keyword(args, 1, "category")
            .unwrap_or(Value::py_class(built_in_warning_class("Warning")?));

        let lineno_val =
            positional_or_keyword(args, 2, "lineno").unwrap_or_else(|| Value::int(0));
        let lineno = validate_lineno_nonneg(&lineno_val)?;

        let append_val =
            positional_or_keyword(args, 3, "append").unwrap_or_else(|| Value::bool_(false));
        let append = _interp.truthy_value(&append_val)?;

        let item = Value::tuple(vec![
            Value::string(action),
            Value::none(),
            category,
            Value::none(),
            Value::int(lineno),
        ]);
        add_filter_entry(_interp, item, append)?;
        Ok(Value::none())
    }

    /// `warnings.resetwarnings()` — clear the live warning filters list in place.
    fn resetwarnings(args) -> Result<Value> {
        let _ = args;
        let filters = active_filters_value(_interp);
        if matches!(filters.kind(), ValueKind::List(_)) {
            let clear_fn = _interp.get_attr(&filters, "clear")?;
            _interp.call_function_expanded(clear_fn, &[])?;
        } else {
            set_active_filters_value(_interp, Value::list(Vec::new()));
        }
        _interp.warnings_state.bump_version();
        Ok(Value::none())
    }

    // ── WarningMessage methods ────────────────────────────────────────────────

    #[py_name = "WarningMessage.__init__"]
    fn warning_message_init(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let rest = &args[1..];
        let message = positional_or_keyword(rest, 0, "message").ok_or_else(|| {
            PyError::named("TypeError", "WarningMessage.__init__() missing required argument: 'message'")
        })?;
        let category = positional_or_keyword(rest, 1, "category").ok_or_else(|| {
            PyError::named("TypeError", "WarningMessage.__init__() missing required argument: 'category'")
        })?;
        let filename = positional_or_keyword(rest, 2, "filename").ok_or_else(|| {
            PyError::named("TypeError", "WarningMessage.__init__() missing required argument: 'filename'")
        })?;
        let lineno = positional_or_keyword(rest, 3, "lineno").ok_or_else(|| {
            PyError::named("TypeError", "WarningMessage.__init__() missing required argument: 'lineno'")
        })?;
        let file = positional_or_keyword(rest, 4, "file").unwrap_or_else(Value::none);
        let line = positional_or_keyword(rest, 5, "line").unwrap_or_else(Value::none);
        let source = positional_or_keyword(rest, 6, "source").unwrap_or_else(Value::none);

        let category_name = if category.is_none() {
            Value::none()
        } else if let ValueKind::PyClass(cls) = category.kind() {
            Value::string(cls.borrow().name.clone())
        } else {
            _interp.get_attr(&category, "__name__").unwrap_or_else(|_| Value::none())
        };

        let mut instance = inst.borrow_mut();
        instance.attrs.insert("message", message);
        instance.attrs.insert("category", category);
        instance.attrs.insert("filename", filename);
        instance.attrs.insert("lineno", lineno);
        instance.attrs.insert("file", file);
        instance.attrs.insert("line", line);
        instance.attrs.insert("source", source);
        instance.attrs.insert("_category_name", category_name);
        Ok(Value::none())
    }

    #[py_name = "WarningMessage.__str__"]
    fn warning_message_str(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let (message, cat_name, filename, lineno, line) = {
            let b = inst.borrow();
            (
                b.attrs.get("message").cloned().unwrap_or_else(Value::none),
                b.attrs
                    .get("_category_name")
                    .cloned()
                    .unwrap_or_else(Value::none),
                b.attrs.get("filename").cloned().unwrap_or_else(Value::none),
                b.attrs.get("lineno").cloned().unwrap_or_else(Value::none),
                b.attrs.get("line").cloned().unwrap_or_else(Value::none),
            )
        };
        let msg_r = render_value_repr(_interp, &message)?;
        let cat_r = render_value_repr(_interp, &cat_name)?;
        let file_r = render_value_repr(_interp, &filename)?;
        let lineno_s = render_instance_str(_interp, &lineno)?;
        let line_r = render_value_repr(_interp, &line)?;
        Ok(Value::string(format!(
            "{{message : {msg_r}, category : {cat_r}, filename : {file_r}, lineno : {lineno_s}, line : {line_r}}}"
        )))
    }

    // ── catch_warnings dispatch ───────────────────────────────────────────────

    /// `catch_warnings(*, record=False, module=None, action=None, category=Warning, lineno=0, append=False)`
    #[py_name = "CatchWarnings.__init__"]
    fn catch_warnings_init(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = context_warning_message_class(&inst, FN_NAME)?;

        let mut record = false;
        let mut action: Option<Value> = None;
        let mut category: Option<Value> = None;
        let mut lineno = Value::int(0);
        let mut append = Value::bool_(false);

        for arg in &args[1..] {
            match arg.name.as_deref() {
                Some("record") => {
                    record = _interp.truthy_value(&arg.value)?;
                }
                Some("module") => {}
                Some("action") => {
                    if !arg.value.is_none() {
                        action = Some(arg.value.clone());
                    }
                }
                Some("category") => {
                    category = Some(arg.value.clone());
                }
                Some("lineno") => {
                    lineno = arg.value.clone();
                }
                Some("append") => {
                    append = arg.value.clone();
                }
                _ => {}
            }
        }

        let filter_spec = if let Some(act) = action {
            let cat = category.unwrap_or(Value::py_class(built_in_warning_class("Warning")?));
            Value::tuple(vec![act, cat, lineno, append])
        } else {
            Value::none()
        };

        let mut instance = inst.borrow_mut();
        instance.attrs.insert("_record", Value::bool_(record));
        instance.attrs.insert("_entered", Value::bool_(false));
        instance.attrs.insert("_sink_active", Value::bool_(false));
        instance.attrs.insert("_filter", filter_spec);
        Ok(Value::none())
    }

    #[py_name = "CatchWarnings.__repr__"]
    fn catch_warnings_repr(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let record = matches!(
            inst.borrow().attrs.get("_record").map(|v| v.kind()),
            Some(ValueKind::Bool(true))
        );
        let class_name = inst.borrow().class.borrow().name.clone();
        if record {
            Ok(Value::string(format!("{class_name}(record=True)")))
        } else {
            Ok(Value::string(format!("{class_name}()")))
        }
    }

    /// `catch_warnings.__enter__()` — copy live filter list; return list if recording.
    #[py_name = "CatchWarnings.__enter__"]
    fn catch_warnings_enter(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let record = matches!(
            inst.borrow().attrs.get("_record").map(|v| v.kind()),
            Some(ValueKind::Bool(true))
        );
        let entered = matches!(
            inst.borrow().attrs.get("_entered").map(|v| v.kind()),
            Some(ValueKind::Bool(true))
        );
        if entered {
            let class_name = inst.borrow().class.borrow().name.clone();
            return Err(PyError::named(
                "RuntimeError",
                if record {
                    format!("Cannot enter {class_name}(record=True) twice")
                } else {
                    format!("Cannot enter {class_name}() twice")
                },
            ));
        }
        inst.borrow_mut()
            .attrs
            .insert("_entered", Value::bool_(true));

        // Save the current live filters list object and install a shallow copy.
        let saved_filters = active_filters_value(_interp);
        let copied_items: Vec<Value> = match saved_filters.kind() {
            ValueKind::List(items) => items.iter().cloned().collect(),
            ValueKind::Tuple(items) => items.to_vec(),
            _ => Vec::new(),
        };
        let filters_copy = Value::list(copied_items);
        let saved_showwarning = active_module_attr(_interp, "showwarning")
            .unwrap_or_else(|| Value::builtin_function("warnings.showwarning"));

        {
            let mut b = inst.borrow_mut();
            b.attrs.insert("_saved_filters", saved_filters);
            b.attrs.insert("_saved_showwarning", saved_showwarning);
        }
        set_active_filters_value(_interp, filters_copy);
        _interp.warnings_state.bump_version();

        // If `action=...` was supplied to catch_warnings(), apply simplefilter now.
        let filter_spec = inst.borrow().attrs.get("_filter").cloned();
        if let Some(spec) = filter_spec
            && let ValueKind::Tuple(parts) = spec.kind()
            && parts.len() == 4
        {
            let action = validate_action(_interp, &parts[0])?;
            let category = parts[1].clone();
            let lineno = validate_lineno_nonneg(&parts[2])?;
            let append = _interp.truthy_value(&parts[3])?;
            let item = Value::tuple(vec![
                Value::string(action),
                Value::none(),
                category,
                Value::none(),
                Value::int(lineno),
            ]);
            add_filter_entry(_interp, item, append)?;
        }

        if record {
            let warning_message_class = context_warning_message_class(&inst, FN_NAME)?;
            // Reset showwarning to the default hook so the recording sink runs
            // (matching CPython 3.12); restored on __exit__.
            set_module_attr(
                _interp,
                "showwarning",
                Value::builtin_function("warnings.showwarning"),
            );
            let list_val = Value::list(Vec::new());
            _interp
                .warnings_state
                .record_sinks
                .borrow_mut()
                .push(RecordSink {
                    log: list_val.clone(),
                    warning_message_class,
                });
            let mut b = inst.borrow_mut();
            b.attrs.insert("_log", list_val.clone());
            b.attrs.insert("_sink_active", Value::bool_(true));
            Ok(list_val)
        } else {
            Ok(Value::none())
        }
    }

    /// `catch_warnings.__exit__(exc_type, exc_val, exc_tb)` — restore filter & showwarning state.
    #[py_name = "CatchWarnings.__exit__"]
    fn catch_warnings_exit(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let record = matches!(
            inst.borrow().attrs.get("_record").map(|v| v.kind()),
            Some(ValueKind::Bool(true))
        );
        let entered = matches!(
            inst.borrow().attrs.get("_entered").map(|v| v.kind()),
            Some(ValueKind::Bool(true))
        );
        if !entered {
            let class_name = inst.borrow().class.borrow().name.clone();
            return Err(PyError::named(
                "RuntimeError",
                if record {
                    format!("Cannot exit {class_name}(record=True) without entering first")
                } else {
                    format!("Cannot exit {class_name}() without entering first")
                },
            ));
        }
        let sink_active = matches!(
            inst.borrow().attrs.get("_sink_active").map(|v| v.kind()),
            Some(ValueKind::Bool(true))
        );
        if record && sink_active {
            _interp.warnings_state.record_sinks.borrow_mut().pop();
            inst.borrow_mut()
                .attrs
                .insert("_sink_active", Value::bool_(false));
        }
        let (saved_filters, saved_showwarning) = {
            let b = inst.borrow();
            (
                b.attrs.get("_saved_filters").cloned(),
                b.attrs.get("_saved_showwarning").cloned(),
            )
        };
        if let Some(saved_filters) = saved_filters {
            set_active_filters_value(_interp, saved_filters);
        }
        if let Some(saved_showwarning) = saved_showwarning {
            set_module_attr(_interp, "showwarning", saved_showwarning);
        }
        _interp.warnings_state.bump_version();
        Ok(Value::none())
    }
}
