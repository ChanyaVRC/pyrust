// `warnings` module — minimal implementation of Python's `warnings` module.
//
// Provides the standard `warnings.warn()` API plus filter management
// (`filterwarnings`, `simplefilter`, `resetwarnings`) and the
// `catch_warnings` context manager.
//
// ## Design
//
// ### Filter list
//
// CPython stores `warnings.filters` as interpreter/module state.  Here the
// native filter and recording state is owned by `Interpreter`: independent
// root interpreters do not leak policy through the host thread, while the
// short-lived child used to execute an imported Python module shares its
// parent's state because both represent one Python interpreter.
//
// The public `"filters"` value remains a snapshot stub. Mutations through
// that Python list are not reflected back into the canonical state.
//
// ### warn() output
//
// Warnings are written to `sys.stderr` via Rust's `eprintln!`.  Stack
// introspection (to find the caller's filename/lineno) is not implemented;
// we emit `"<unknown>:0: <Category>: <message>\n"`.
//
// ### catch_warnings
//
// The context manager saves and restores the filter list around a `with`
// block.  When `record=True`, collected `WarningMessage` objects are
// returned from `__enter__` and `warn()` appends to them instead of
// printing to stderr.
//
// Reference: <https://docs.python.org/3/library/warnings.html>

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, class_is_subclass_of, lookup_exc_class, render_instance_str,
};
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value, ValueKind};
use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

// ── filter list ───────────────────────────────────────────────────────────────

/// A parsed warnings filter entry.
#[derive(Clone)]
struct Filter {
    /// "default", "ignore", "always", "error", "once", "module".
    action: String,
    /// The exact class object supplied to the filter API.
    ///
    /// `simplefilter` intentionally accepts non-class values and defers the
    /// resulting `issubclass` TypeError until a warning reaches the filter, as
    /// CPython 3.12 does.  Valid categories retain their Rc identity.
    category: FilterCategory,
}

impl Filter {
    fn new(action: &str, category: FilterCategory) -> Self {
        Filter {
            action: action.to_string(),
            category,
        }
    }
}

#[derive(Clone)]
enum FilterCategory {
    Class(Rc<RefCell<PyClass>>),
    Invalid(Value),
}

impl FilterCategory {
    fn unchecked(value: Value) -> Self {
        let class = match value.kind() {
            ValueKind::PyClass(class) => Some(Rc::clone(class)),
            _ => None,
        };
        match class {
            Some(class) => Self::Class(class),
            None => Self::Invalid(value),
        }
    }

    fn as_value(&self) -> Value {
        match self {
            Self::Class(class) => Value::py_class(Rc::clone(class)),
            Self::Invalid(value) => value.clone(),
        }
    }
}

struct RecordSink {
    log: Value,
    warning_message_class: Rc<RefCell<PyClass>>,
}

/// Mutable state of the `warnings` module for one Python interpreter.
///
/// `Interpreter` owns this behind an `Rc`: roots get fresh state, while the
/// implementation-only child used for a filesystem import clones the handle.
/// Keeping the concrete filters and sinks private to this module preserves the
/// built-in's responsibility boundary.
#[derive(Default)]
pub(crate) struct WarningsState {
    /// Active filters — most-recently-added is checked first (prepend).
    filters: RefCell<Vec<Filter>>,
    /// Active `catch_warnings(record=True)` sinks in nesting order.
    ///
    /// Leaving an inner recording context must restore the outer context,
    /// rather than disabling recording entirely. Non-recording nested
    /// contexts do not push and therefore keep using the nearest recording
    /// ancestor, matching CPython.
    record_sinks: RefCell<Vec<RecordSink>>,
}

/// Push a filter entry to the front of the list (highest priority).
fn push_filter(interpreter: &crate::Interpreter, filter: Filter) {
    interpreter
        .warnings_state
        .filters
        .borrow_mut()
        .insert(0, filter);
}

/// Determine the action for a warning with the given category class.
/// Returns "default" if no filter matches.
fn matched_action(
    interpreter: &crate::Interpreter,
    category: &Rc<RefCell<PyClass>>,
) -> Result<String> {
    for filter in interpreter.warnings_state.filters.borrow().iter() {
        let matches = match &filter.category {
            FilterCategory::Class(expected) => class_is_subclass_of(category, expected),
            FilterCategory::Invalid(_) => {
                return Err(PyError::named(
                    "TypeError",
                    "issubclass() arg 2 must be a class, a tuple of classes, or a union",
                ));
            }
        };
        if matches {
            return Ok(filter.action.clone());
        }
    }
    Ok("default".to_string())
}

// ── warnings-owned classes ───────────────────────────────────────────────────

type WeakPyClass = Weak<RefCell<PyClass>>;

struct WarningClassGeneration {
    /// A live catch_warnings class owns the lifetime of its paired
    /// WarningMessage generation through this registry entry.
    warning_message: Rc<RefCell<PyClass>>,
    catch_warnings: WeakPyClass,
}

thread_local! {
    /// Tracks the class pair created for every still-live catch_warnings
    /// generation.
    ///
    /// `del sys.modules["warnings"]` followed by another import executes the
    /// module again and must create fresh classes. A retained catch_warnings
    /// class must still be sufficient to construct a context after its module
    /// and WarningMessage binding are collected. The catch side stays weak to
    /// avoid making every import immortal; the paired WarningMessage stays
    /// strong until that catch side dies and the registry is next pruned.
    static WARNING_CLASS_GENERATIONS: RefCell<Vec<WarningClassGeneration>> =
        const { RefCell::new(Vec::new()) };
}

fn new_warning_message_class() -> Value {
    Value::py_class(Rc::new(RefCell::new(PyClass::new(
        "WarningMessage",
        "WarningMessage",
        None,
        IndexMap::new(),
    ))))
}

fn new_catch_warnings_class() -> Value {
    let mut attrs: IndexMap<String, Value> = IndexMap::new();
    attrs.insert(
        "__init__".to_string(),
        Value::builtin_function("warnings.CatchWarnings.__init__"),
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

/// Finalize one imported warnings generation: publish this interpreter's
/// filter snapshot and register the module's concrete class pair. Called by
/// the generic built-in module finalization hook before the module becomes
/// observable through `sys.modules`.
pub(crate) fn prepare_module(interpreter: &crate::Interpreter, module: &Value) {
    let ValueKind::PyModule(module) = module.kind() else {
        return;
    };
    module
        .borrow_mut()
        .attrs
        .insert("filters".to_string(), snapshot_filters(interpreter));
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

/// Resolve and retain the WarningMessage class paired with this context's
/// generation. The cached instance attribute keeps the class alive even if the
/// old module is removed and no other Python reference to it remains.
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
        PyError::Runtime(format!(
            "internal: {fn_name}() cannot resolve its warnings module generation",
        ))
    })?;
    inst.borrow_mut().attrs.insert(
        "_warning_message_class",
        Value::py_class(Rc::clone(&warning_message)),
    );
    Ok(warning_message)
}

/// Build a `WarningMessage` instance without reconstructing its category from
/// a mutable Python-visible class name.
fn make_warning_message(
    warning_message_class: &Rc<RefCell<PyClass>>,
    message: Value,
    category: &Rc<RefCell<PyClass>>,
    filename: &str,
    lineno: i64,
) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert("message", message);
    attrs.insert("category", Value::py_class(Rc::clone(category)));
    attrs.insert("filename", Value::string(filename));
    attrs.insert("lineno", Value::int(lineno));
    attrs.insert("source", Value::none());
    Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(warning_message_class),
        attrs,
    })))
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn expect_self(args: &[ExpandedCallArg], fn_name: &str) -> Result<Rc<RefCell<PyInstance>>> {
    match args.first().map(|a| a.value.kind()) {
        Some(ValueKind::PyInstance(rc)) => Ok(Rc::clone(rc)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

fn built_in_warning_class(name: &str) -> Result<Rc<RefCell<PyClass>>> {
    lookup_exc_class(name)
        .ok_or_else(|| PyError::Runtime(format!("built-in exception '{name}' is not defined")))
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

fn validated_filter_category(value: &Value) -> Result<FilterCategory> {
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
    Ok(FilterCategory::Class(Rc::clone(category)))
}

/// Normalise `warn(message, category)` to the actual Warning instance and its
/// concrete category.  A Warning instance wins over an explicitly supplied
/// category; otherwise the requested class is invoked so user `__init__`
/// semantics and instance identity are preserved.
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

/// Snapshot the current filter list while retaining category object identity.
fn snapshot_filters(interpreter: &crate::Interpreter) -> Value {
    let items: Vec<Value> = interpreter
        .warnings_state
        .filters
        .borrow()
        .iter()
        .map(|filter| {
            Value::tuple(vec![
                Value::string(&filter.action),
                filter.category.as_value(),
            ])
        })
        .collect();
    Value::list(items)
}

/// Restore the filter list from a snapshot produced by `snapshot_filters`.
///
/// The snapshot is ordered identically to the internal filter vec (index 0 =
/// highest priority), so we iterate forward and push to keep that order.
fn restore_filters(interpreter: &crate::Interpreter, snapshot: &Value) {
    let mut filters = interpreter.warnings_state.filters.borrow_mut();
    filters.clear();
    if let ValueKind::List(items) = snapshot.kind() {
        for item in items.iter() {
            if let ValueKind::Tuple(pair) = item.kind()
                && pair.len() == 2
            {
                let action = match pair[0].kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => continue,
                };
                filters.push(Filter {
                    action,
                    category: FilterCategory::unchecked(pair[1].clone()),
                });
            }
        }
    }
}

// ── module ────────────────────────────────────────────────────────────────────

pyrust_module! {
    constants {
        // Warning category classes live in builtins, NOT in the warnings module.
        // CPython 3.12: `hasattr(warnings, 'UserWarning')` is False.
        // `warnings.warn("msg", UserWarning)` still works because UserWarning
        // is resolved from the caller's builtins namespace, not from here.

        // Replaced with this interpreter's snapshot by the import finalizer.
        "filters"                   => Value::list(Vec::new()),

        // These classes are defined by warnings.py, so each fresh module import
        // receives a fresh pair just as CPython does.
        "catch_warnings"            => new_catch_warnings_class(),

        "WarningMessage"            => new_warning_message_class(),
    }

    /// `warnings.warn(message, category=UserWarning, stacklevel=1, source=None)`
    ///
    /// Issue a warning.  If a filter with action "ignore" matches, the warning
    /// is suppressed.  If action "error" matches, raises the category as an
    /// exception.  Otherwise, the warning is written to stderr (or appended
    /// to the recording list if inside a `catch_warnings(record=True)` block).
    fn warn(args) -> Result<Value> {
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() requires at least 1 argument (0 given)"),
            ));
        }
        let msg_val = args[0].value.clone();
        let category = positional_or_keyword(args, 1, "category");
        // stacklevel and source are accepted but ignored.
        let (warning_instance, category) =
            normalize_warning(_interp, msg_val, category)?;
        let category_name = category.borrow().name.clone();
        let message_text = render_instance_str(_interp, &warning_instance)?;
        let action = matched_action(_interp, &category)?;

        match action.as_str() {
            "ignore" => {
                // Suppressed — do nothing.
            }
            "error" => {
                // The warning was already constructed above.  Raising that
                // exact object preserves user-class identity, custom
                // __init__, and (when passed in) object identity.
                return Err(PyError::Raised(warning_instance));
            }
            _ => {
                // "default", "always", "once", "module" — emit the warning.
                let warn_line = format!("<unknown>:0: {category_name}: {message_text}");
                // If inside a catch_warnings(record=True) block, push
                // directly into the shared Python list so the caller's `w`
                // binding sees the item immediately.
                let recorded = if let Some(record_sink) =
                    _interp.warnings_state.record_sinks.borrow().last()
                {
                    let wmsg = make_warning_message(
                        &record_sink.warning_message_class,
                        warning_instance.clone(),
                        &category,
                        "<unknown>",
                        0,
                    );
                    let _ = record_sink.log.list_push(wmsg);
                    true
                } else {
                    false
                };
                if !recorded {
                    eprintln!("{warn_line}");
                }
            }
        }
        Ok(Value::none())
    }

    /// `warnings.filterwarnings(action, message='', category=Warning,
    ///     module='', lineno=0, append=False)`
    ///
    /// Add a filter entry.  Only `action` and `category` are acted on;
    /// `message`, `module`, `lineno`, and `append` are accepted but ignored.
    fn filterwarnings(args) -> Result<Value> {
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() requires at least 1 argument (0 given)"),
            ));
        }
        let action = match args[0].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() action must be a string"),
                ))
            }
        };
        // category is the 3rd positional arg (index 2) or keyword "category".
        // Search both positional slot and any keyword arg named "category".
        let cat_val = args
            .iter()
            .find(|a| a.name.as_deref() == Some("category"))
            .map(|a| a.value.clone())
            .or_else(|| positional_or_keyword(args, 2, "category"))
            .unwrap_or(Value::py_class(built_in_warning_class("Warning")?));
        // Unlike simplefilter, filterwarnings validates eagerly.
        let category = validated_filter_category(&cat_val)?;
        push_filter(_interp, Filter::new(&action, category));
        Ok(Value::none())
    }

    /// `warnings.simplefilter(action, category=Warning, lineno=0, append=False)`
    ///
    /// Add a simple filter that matches all messages of the given category.
    fn simplefilter(args) -> Result<Value> {
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() requires at least 1 argument (0 given)"),
            ));
        }
        let action = match args[0].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() action must be a string"),
                ))
            }
        };
        // CPython deliberately does not validate this category here.  A class
        // participates in normal subclass matching; a non-class is retained
        // and raises TypeError only if warning dispatch reaches this filter.
        let category = positional_or_keyword(args, 1, "category")
            .unwrap_or(Value::py_class(built_in_warning_class("Warning")?));
        push_filter(
            _interp,
            Filter::new(&action, FilterCategory::unchecked(category)),
        );
        Ok(Value::none())
    }

    /// `warnings.resetwarnings()` — clear all warning filters.
    fn resetwarnings(args) -> Result<Value> {
        let _ = args;
        _interp.warnings_state.filters.borrow_mut().clear();
        Ok(Value::none())
    }

    // ── catch_warnings dispatch ───────────────────────────────────────────────

    /// `catch_warnings(*, record=False)` — constructor.
    ///
    /// `record=True` causes `__enter__` to install a "always" filter and
    /// return a list that `warn()` appends `WarningMessage` objects to.
    #[py_name = "CatchWarnings.__init__"]
    fn catch_warnings_init(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        // Resolve the paired class by immutable class identity while this
        // generation is registered, then retain it on the context object.
        let _ = context_warning_message_class(&inst, FN_NAME)?;
        // Parse keyword args for `record`.
        let mut record = false;
        for arg in &args[1..] {
            if arg.name.as_deref() == Some("record") {
                match arg.value.kind() {
                    ValueKind::Bool(b) => record = b,
                    ValueKind::Int(n) => record = n != 0,
                    _ => {}
                }
            }
        }
        let _ = _interp;
        let mut instance = inst.borrow_mut();
        instance.attrs.insert("_record", Value::bool_(record));
        instance.attrs.insert("_entered", Value::bool_(false));
        instance.attrs.insert("_sink_active", Value::bool_(false));
        Ok(Value::none())
    }

    /// `catch_warnings.__enter__()` — save filter state; return list if recording.
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
            return Err(PyError::named(
                "RuntimeError",
                if record {
                    "Cannot enter catch_warnings(record=True) twice"
                } else {
                    "Cannot enter catch_warnings() twice"
                },
            ));
        }
        inst.borrow_mut()
            .attrs
            .insert("_entered", Value::bool_(true));
        // Save current filters.
        let snap = snapshot_filters(_interp);
        inst.borrow_mut()
            .attrs
            .insert("_saved_filters", snap);
        // If record=True, install an "always" filter and set up the sink.
        if record {
            let warning_message_class = context_warning_message_class(&inst, FN_NAME)?;
            push_filter(
                _interp,
                Filter::new(
                    "always",
                    FilterCategory::Class(built_in_warning_class("Warning")?),
                ),
            );
            // Create the Python list that both the recording state and the
            // caller's `w` binding share via Rc<ListInner>. warn() pushes
            // directly into this list so items are visible immediately.
            let list_val = Value::list(Vec::new());
            _interp
                .warnings_state
                .record_sinks
                .borrow_mut()
                .push(RecordSink {
                    log: list_val.clone(),
                    warning_message_class,
                });
            inst.borrow_mut()
                .attrs
                .insert("_log", list_val.clone());
            inst.borrow_mut()
                .attrs
                .insert("_sink_active", Value::bool_(true));
            Ok(list_val)
        } else {
            Ok(Value::none())
        }
    }

    /// `catch_warnings.__exit__(exc_type, exc_val, exc_tb)` — restore filter state.
    #[py_name = "CatchWarnings.__exit__"]
    fn catch_warnings_exit(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        // Flush recorded warnings into the list object.
        let record = matches!(
            inst.borrow().attrs.get("_record").map(|v| v.kind()),
            Some(ValueKind::Bool(true))
        );
        let entered = matches!(
            inst.borrow().attrs.get("_entered").map(|v| v.kind()),
            Some(ValueKind::Bool(true))
        );
        if !entered {
            return Err(PyError::named(
                "RuntimeError",
                if record {
                    "Cannot exit catch_warnings(record=True) without entering first"
                } else {
                    "Cannot exit catch_warnings() without entering first"
                },
            ));
        }
        let sink_active = matches!(
            inst.borrow().attrs.get("_sink_active").map(|v| v.kind()),
            Some(ValueKind::Bool(true))
        );
        if record && sink_active {
            // Pop only this context so an outer recording context becomes
            // active again.
            _interp.warnings_state.record_sinks.borrow_mut().pop();
            inst.borrow_mut()
                .attrs
                .insert("_sink_active", Value::bool_(false));
        }
        // Restore saved filters.
        let snap = inst.borrow().attrs.get("_saved_filters").cloned();
        if let Some(snap) = snap {
            restore_filters(_interp, &snap);
        }
        // Context-manager suppression treats both None and False as falsey,
        // but the direct Python-visible return value is observable.  CPython's
        // catch_warnings.__exit__ returns None, including on repeated exits.
        Ok(Value::none())
    }
}
