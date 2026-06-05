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
// CPython stores `warnings.filters` as a module-level list of 5-tuples.
// Here we maintain a thread-local `Vec<Filter>` as the canonical state.
// The module constant `"filters"` is a Python list that is rebuilt from
// the canonical state each time `module()` is called; mutations via the
// Python list object are not reflected back into the canonical state, but
// that is acceptable for a stub implementation.
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
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{lookup_exc_class, ExpandedCallArg};
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value, ValueKind};
use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

// ── filter list ───────────────────────────────────────────────────────────────

/// A parsed warnings filter entry.
#[derive(Clone)]
struct Filter {
    /// "default", "ignore", "always", "error", "once", "module", "all".
    action: String,
    /// Category class name to match (empty = match all).
    category_name: String,
}

impl Filter {
    fn new(action: &str, category_name: &str) -> Self {
        Filter {
            action: action.to_string(),
            category_name: category_name.to_string(),
        }
    }
}

thread_local! {
    /// Active filters — most-recently-added is checked first (prepend).
    static FILTERS: RefCell<Vec<Filter>> = const { RefCell::new(Vec::new()) };

    /// When Some, we are inside a `catch_warnings(record=True)` block and
    /// warn() pushes WarningMessage objects directly into this Python list so
    /// the caller's `w` binding sees them immediately (shared Rc<ListInner>).
    static RECORD_SINK: RefCell<Option<Value>> = const { RefCell::new(None) };
}

/// Push a filter entry to the front of the list (highest priority).
fn push_filter(f: Filter) {
    FILTERS.with(|fl| fl.borrow_mut().insert(0, f));
}

/// Determine the action for a warning with the given category name.
/// Returns "default" if no filter matches.
fn matched_action(category_name: &str) -> String {
    FILTERS.with(|fl| {
        for f in fl.borrow().iter() {
            if f.category_name.is_empty() || f.category_name == category_name {
                return f.action.clone();
            }
        }
        "default".to_string()
    })
}

// ── WarningMessage class ──────────────────────────────────────────────────────

thread_local! {
    static WARNING_MESSAGE_CLASS: Rc<RefCell<PyClass>> = {
        Rc::new(RefCell::new(PyClass::new(
            "WarningMessage",
            "WarningMessage",
            None,
            IndexMap::new(),
        )))
    };
}

/// Build a `WarningMessage` instance.
fn make_warning_message(
    message: Value,
    category_name: &str,
    filename: &str,
    lineno: i64,
) -> Value {
    WARNING_MESSAGE_CLASS.with(|class| {
        let mut attrs = InstanceAttrs::new();
        attrs.insert("message", message);
        attrs.insert(
            "category",
            warning_class_by_name(category_name),
        );
        attrs.insert("filename", Value::string(filename));
        attrs.insert("lineno", Value::int(lineno));
        attrs.insert("source", Value::none());
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(class),
            attrs,
        })))
    })
}

// ── catch_warnings class ──────────────────────────────────────────────────────

thread_local! {
    static CATCH_WARNINGS_CLASS: Rc<RefCell<PyClass>> = {
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
        Rc::new(RefCell::new(PyClass::new(
            "catch_warnings",
            "catch_warnings",
            None,
            attrs,
        )))
    };
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

/// Get the name of a class (PyClass or PyInstance via its class).
fn class_name(v: &Value) -> Option<String> {
    match v.kind() {
        ValueKind::PyClass(rc) => Some(rc.borrow().name.clone()),
        _ => None,
    }
}

/// Resolve a warning category class Value to its name string.
fn category_name_for(cat: &Value) -> String {
    match cat.kind() {
        ValueKind::PyClass(rc) => rc.borrow().name.clone(),
        _ => "UserWarning".to_string(),
    }
}

/// Look up a warning category class by name from the exception cache.
fn warning_class_by_name(name: &str) -> Value {
    lookup_exc_class(name)
        .map(Value::py_class)
        .unwrap_or_else(Value::none)
}

/// Extract the text of a warning from its `message` argument.
///
/// CPython accepts a string or a Warning instance.  We accept anything and
/// call `repr()` as a fallback.
fn message_text(v: &Value) -> String {
    match v.kind() {
        ValueKind::Str(s) => s.to_string(),
        ValueKind::PyInstance(rc) => {
            // If the instance has an `args` attribute with at least one
            // element, use that (matches BaseException.__str__ behaviour).
            let borrow = rc.borrow();
            if let Some(args_val) = borrow.attrs.get("args") {
                match args_val.kind() {
                    ValueKind::Tuple(items) if !items.is_empty() => {
                        if let ValueKind::Str(s) = items[0].kind() { return s.to_string() }
                    }
                    _ => {}
                }
            }
            v.repr()
        }
        _ => v.repr(),
    }
}

/// Snapshot the current filter list as a Python list of strings (action names).
/// We store them as `(action, category_name)` tuples so they can be restored.
fn snapshot_filters() -> Value {
    FILTERS.with(|fl| {
        let items: Vec<Value> = fl
            .borrow()
            .iter()
            .map(|f| {
                Value::tuple(vec![
                    Value::string(&f.action),
                    Value::string(&f.category_name),
                ])
            })
            .collect();
        Value::list(items)
    })
}

/// Restore the filter list from a snapshot produced by `snapshot_filters`.
///
/// The snapshot is ordered identically to the internal FILTERS vec (index 0 =
/// highest priority), so we iterate forward and push to keep that order.
fn restore_filters(snapshot: &Value) {
    FILTERS.with(|fl| {
        let mut filters = fl.borrow_mut();
        filters.clear();
        if let ValueKind::List(items) = snapshot.kind() {
            for item in items.iter() {
                if let ValueKind::Tuple(pair) = item.kind()
                    && pair.len() == 2 {
                        let action = match pair[0].kind() {
                            ValueKind::Str(s) => s.to_string(),
                            _ => continue,
                        };
                        let category_name = match pair[1].kind() {
                            ValueKind::Str(s) => s.to_string(),
                            _ => continue,
                        };
                        filters.push(Filter { action, category_name });
                    }
            }
        }
    });
}

// ── module ────────────────────────────────────────────────────────────────────

pyrust_module! {
    constants {
        // Warning category classes live in builtins, NOT in the warnings module.
        // CPython 3.12: `hasattr(warnings, 'UserWarning')` is False.
        // `warnings.warn("msg", UserWarning)` still works because UserWarning
        // is resolved from the caller's builtins namespace, not from here.

        // filters: the active filter list.  Rebuilt at module() time.
        "filters"                   => snapshot_filters(),

        // catch_warnings class value.
        "catch_warnings"            => Value::py_class(CATCH_WARNINGS_CLASS.with(Rc::clone)),

        // WarningMessage class value.
        "WarningMessage"            => Value::py_class(WARNING_MESSAGE_CLASS.with(Rc::clone)),
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
        // category defaults to UserWarning.
        let cat_val = args.get(1).map(|a| a.value.clone()).unwrap_or_else(|| {
            warning_class_by_name("UserWarning")
        });
        // stacklevel and source are accepted but ignored.
        let _ = _interp;

        let cat_name = category_name_for(&cat_val);
        let msg_text = message_text(&msg_val);
        let action = matched_action(&cat_name);

        match action.as_str() {
            "ignore" => {
                // Suppressed — do nothing.
            }
            "error" => {
                // Raise the warning category as an exception.
                let err = match lookup_exc_class(&cat_name) {
                    Some(rc) => PyError::class(rc, msg_text),
                    None => PyError::Named(std::borrow::Cow::Owned(cat_name), msg_text),
                };
                return Err(err);
            }
            _ => {
                // "default", "always", "once", "module", "all" — emit the warning.
                let warn_line =
                    format!("<unknown>:0: {cat_name}: {msg_text}");
                // If inside a catch_warnings(record=True) block, push
                // directly into the shared Python list so the caller's `w`
                // binding sees the item immediately.
                let recorded = RECORD_SINK.with(|sink| {
                    if let Some(ref list_val) = *sink.borrow() {
                        let wmsg = make_warning_message(
                            msg_val.clone(),
                            &cat_name,
                            "<unknown>",
                            0,
                        );
                        let _ = list_val.list_push(wmsg);
                        true
                    } else {
                        false
                    }
                });
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
            .or_else(|| args.get(2).map(|a| a.value.clone()))
            .unwrap_or_else(|| warning_class_by_name("Warning"));
        // Map the category name to the canonical filter string.  An empty
        // string means "match all"; the base class Warning also means match
        // all because every warning is a Warning subclass.
        let cat_name = match cat_val.kind() {
            ValueKind::PyClass(rc) => {
                let name = rc.borrow().name.clone();
                if name == "Warning" {
                    String::new()
                } else {
                    name
                }
            }
            _ => String::new(),
        };
        let _ = _interp;
        push_filter(Filter::new(&action, &cat_name));
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
        // Empty category_name means "match all".  Warning (the base class)
        // also means match all because every warning is a Warning subclass.
        let cat_name = if let Some(cat_arg) = args.get(1) {
            let name = class_name(&cat_arg.value).unwrap_or_default();
            if name == "Warning" {
                String::new()
            } else {
                name
            }
        } else {
            String::new()
        };
        let _ = _interp;
        push_filter(Filter::new(&action, &cat_name));
        Ok(Value::none())
    }

    /// `warnings.resetwarnings()` — clear all warning filters.
    fn resetwarnings(args) -> Result<Value> {
        let _ = (args, _interp);
        FILTERS.with(|fl| fl.borrow_mut().clear());
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
        inst.borrow_mut()
            .attrs
            .insert("_record", Value::bool_(record));
        Ok(Value::none())
    }

    /// `catch_warnings.__enter__()` — save filter state; return list if recording.
    #[py_name = "CatchWarnings.__enter__"]
    fn catch_warnings_enter(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        // Save current filters.
        let snap = snapshot_filters();
        inst.borrow_mut()
            .attrs
            .insert("_saved_filters", snap);
        // If record=True, install an "always" filter and set up the sink.
        let record = matches!(
            inst.borrow().attrs.get("_record").map(|v| v.kind()),
            Some(ValueKind::Bool(true))
        );
        if record {
            push_filter(Filter::new("always", ""));
            // Create the Python list that both RECORD_SINK and the caller's
            // `w` binding will share via Rc<ListInner>.  warn() pushes
            // directly into this list so items are visible immediately.
            let list_val = Value::list(Vec::new());
            RECORD_SINK.with(|sink| {
                *sink.borrow_mut() = Some(list_val.clone());
            });
            inst.borrow_mut()
                .attrs
                .insert("_log", list_val.clone());
            Ok(list_val)
        } else {
            Ok(Value::py_instance(inst))
        }
    }

    /// `catch_warnings.__exit__(exc_type, exc_val, exc_tb)` — restore filter state.
    #[py_name = "CatchWarnings.__exit__"]
    fn catch_warnings_exit(args) -> Result<Value> {
        let inst = expect_self(args, FN_NAME)?;
        let _ = _interp;
        // Flush recorded warnings into the list object.
        let record = matches!(
            inst.borrow().attrs.get("_record").map(|v| v.kind()),
            Some(ValueKind::Bool(true))
        );
        if record {
            // Items were pushed directly into the shared list via RECORD_SINK;
            // just clear the sink so warn() stops recording.
            RECORD_SINK.with(|sink| sink.borrow_mut().take());
        }
        // Restore saved filters.
        let snap = inst.borrow().attrs.get("_saved_filters").cloned();
        if let Some(snap) = snap {
            restore_filters(&snap);
        }
        Ok(Value::bool_(false))
    }
}
