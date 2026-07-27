//! Generation-aware `Path`/`PosixPath` class construction and instance storage.
//!
//! A `pathlib` module generation owns one exact base/concrete class pair.
//! Weak registry entries let old imported classes keep their identity without
//! retaining otherwise unreachable module generations forever.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use indexmap::IndexMap;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, class_is_subclass_of};
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value, ValueKind};

/// (method-short, registry-name) pairs for regular `Path` methods.
///
/// These names mirror the `#[py_name = "Path.<method>"]` declarations in the
/// owner module. Keeping class construction here avoids coupling the registry
/// to the implementation module that happens to own a method body.
const PATH_METHODS: &[(&str, &str)] = &[
    ("__new__", "pathlib.Path.__new__"),
    ("__init__", "pathlib.Path.__init__"),
    ("__str__", "pathlib.Path.__str__"),
    ("__repr__", "pathlib.Path.__repr__"),
    ("__truediv__", "pathlib.Path.__truediv__"),
    ("__eq__", "pathlib.Path.__eq__"),
    ("__hash__", "pathlib.Path.__hash__"),
    ("__fspath__", "pathlib.Path.__fspath__"),
    ("joinpath", "pathlib.Path.joinpath"),
    ("exists", "pathlib.Path.exists"),
    ("is_file", "pathlib.Path.is_file"),
    ("is_dir", "pathlib.Path.is_dir"),
    ("is_absolute", "pathlib.Path.is_absolute"),
    ("resolve", "pathlib.Path.resolve"),
    ("read_text", "pathlib.Path.read_text"),
    ("write_text", "pathlib.Path.write_text"),
    ("read_bytes", "pathlib.Path.read_bytes"),
    ("write_bytes", "pathlib.Path.write_bytes"),
    ("open", "pathlib.Path.open"),
    ("mkdir", "pathlib.Path.mkdir"),
    ("unlink", "pathlib.Path.unlink"),
    ("iterdir", "pathlib.Path.iterdir"),
    ("glob", "pathlib.Path.glob"),
    ("with_name", "pathlib.Path.with_name"),
    ("with_stem", "pathlib.Path.with_stem"),
    ("with_suffix", "pathlib.Path.with_suffix"),
];

/// Class methods use an explicit descriptor so generic attribute lookup does
/// not depend on pathlib's mutable Python-visible method names.
const PATH_CLASS_METHODS: &[(&str, &str)] =
    &[("cwd", "pathlib.Path.cwd"), ("home", "pathlib.Path.home")];

/// Read-only properties installed on the `Path` class.
const PATH_PROPERTIES: &[(&str, &str)] = &[
    ("name", "pathlib.Path.name"),
    ("parent", "pathlib.Path.parent"),
    ("stem", "pathlib.Path.stem"),
    ("suffix", "pathlib.Path.suffix"),
    ("parts", "pathlib.Path.parts"),
];

/// Weakly tracks the synthetic classes created for one `pathlib` module
/// generation. `PosixPath` must stay paired with the exact `Path` object that
/// was exported alongside it, including after a later module re-import.
struct PathClassGeneration {
    path: Weak<RefCell<PyClass>>,
    posix: Weak<RefCell<PyClass>>,
}

thread_local! {
    static PATH_CLASS_GENERATIONS: RefCell<Vec<PathClassGeneration>> =
        const { RefCell::new(Vec::new()) };
}

pub(super) fn expect_self(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<RefCell<PyInstance>>> {
    match args.first().map(|arg| arg.value.kind()) {
        Some(ValueKind::PyInstance(instance)) => Ok(Rc::clone(instance)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

pub(super) fn get_path(inst: &Rc<RefCell<PyInstance>>, fn_name: &str) -> Result<String> {
    match inst.borrow().attrs.get("_path").map(|value| value.kind()) {
        Some(ValueKind::Str(path)) => Ok(path.to_string()),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}: Path._path has been overwritten with a non-str; \
                 don't assign to internal attributes",
            ),
        )),
    }
}

/// Return whether an instance belongs to any still-live canonical `Path`
/// hierarchy. Python-visible names are intentionally not involved.
pub(super) fn is_path_instance(instance: &Rc<RefCell<PyInstance>>) -> bool {
    let class = Rc::clone(&instance.borrow().class);
    is_path_class_or_subclass(&class)
}

pub(super) fn make_path_instance_for_class(class: Rc<RefCell<PyClass>>, path: &str) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_path", Value::string(path));
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Derived path operations preserve the receiver's concrete subclass.
pub(super) fn make_path_result(inst: &Rc<RefCell<PyInstance>>, path: &str) -> Value {
    make_path_instance_for_class(Rc::clone(&inst.borrow().class), path)
}

/// Construct a result through the class bound to `Path.cwd` / `Path.home`.
///
/// Calling the class preserves subclasses and lets the abstract `Path`
/// factory select the matching generation's `PosixPath`.
pub(super) fn make_path_classmethod_result(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    path: String,
) -> Result<Value> {
    let class = args
        .first()
        .map(|arg| arg.value.clone())
        .filter(|value| matches!(value.kind(), ValueKind::PyClass(_)))
        .ok_or_else(|| {
            PyError::Runtime("internal: pathlib classmethod missing class receiver".to_string())
        })?;
    interp.call_function_expanded(
        class,
        &[ExpandedCallArg {
            name: None,
            value: Value::string(path),
        }],
    )
}

/// Allocate the bare instance for `Path.__new__`.
///
/// Calling the abstract generation's exact `Path` selects its paired
/// `PosixPath`; calling a concrete subclass preserves that subclass.
pub(super) fn path_new(args: &[ExpandedCallArg]) -> Result<Value> {
    let requested = match args.first().map(|arg| arg.value.kind()) {
        Some(ValueKind::PyClass(class)) => Rc::clone(class),
        _ => {
            return Err(PyError::Runtime(
                "internal: pathlib.Path.__new__ missing class receiver".to_string(),
            ));
        }
    };
    let concrete = concrete_path_class(requested);
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: concrete,
        attrs: InstanceAttrs::new(),
    }))))
}

fn build_path_class() -> Rc<RefCell<PyClass>> {
    let mut attrs: IndexMap<String, Value> = IndexMap::new();
    for (short, py_full) in PATH_METHODS {
        attrs.insert((*short).to_string(), Value::builtin_function(py_full));
    }
    for (short, py_full) in PATH_CLASS_METHODS {
        attrs.insert(
            (*short).to_string(),
            pyrust_builtins::classmethod::class_method_any(Value::builtin_function(py_full)),
        );
    }
    for (short, py_full) in PATH_PROPERTIES {
        let getter = Value::builtin_function(py_full);
        attrs.insert(
            (*short).to_string(),
            pyrust_builtins::property::property(getter, Value::none(), Value::none()),
        );
    }
    attrs.insert("__module__".to_string(), Value::string("pathlib"));
    Rc::new(RefCell::new(PyClass::new("Path", "Path", None, attrs)))
}

fn build_posix_path_class(path: &Rc<RefCell<PyClass>>) -> Rc<RefCell<PyClass>> {
    let mut attrs = IndexMap::new();
    attrs.insert("__module__".to_string(), Value::string("pathlib"));
    let posix = Rc::new(RefCell::new(PyClass::new(
        "PosixPath",
        "PosixPath",
        Some(Rc::clone(path)),
        attrs,
    )));
    path.borrow()
        .subclasses
        .borrow_mut()
        .push(Rc::downgrade(&posix));
    posix
}

fn register_path_generation(path: &Rc<RefCell<PyClass>>) {
    PATH_CLASS_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        generations.retain(|generation| generation.path.upgrade().is_some());
        generations.push(PathClassGeneration {
            path: Rc::downgrade(path),
            posix: Weak::new(),
        });
    });
}

fn is_path_class_or_subclass(class: &Rc<RefCell<PyClass>>) -> bool {
    PATH_CLASS_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        generations.retain(|generation| generation.path.upgrade().is_some());
        generations
            .iter()
            .filter_map(|generation| generation.path.upgrade())
            .any(|path| class_is_subclass_of(class, &path))
    })
}

fn latest_path_class() -> Rc<RefCell<PyClass>> {
    if let Some(path) = PATH_CLASS_GENERATIONS.with(|generations| {
        generations
            .borrow()
            .iter()
            .rev()
            .find_map(|generation| generation.path.upgrade())
    }) {
        return path;
    }

    let path = build_path_class();
    register_path_generation(&path);
    path
}

fn posix_class_for_path(path: &Rc<RefCell<PyClass>>) -> Rc<RefCell<PyClass>> {
    if let Some(posix) = PATH_CLASS_GENERATIONS.with(|generations| {
        generations.borrow().iter().rev().find_map(|generation| {
            let generation_path = generation.path.upgrade()?;
            if Rc::ptr_eq(&generation_path, path) {
                generation.posix.upgrade()
            } else {
                None
            }
        })
    }) {
        return posix;
    }

    let posix = build_posix_path_class(path);
    PATH_CLASS_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        if let Some(generation) = generations.iter_mut().rev().find(|generation| {
            generation
                .path
                .upgrade()
                .is_some_and(|generation_path| Rc::ptr_eq(&generation_path, path))
        }) {
            generation.posix = Rc::downgrade(&posix);
        } else {
            generations.push(PathClassGeneration {
                path: Rc::downgrade(path),
                posix: Rc::downgrade(&posix),
            });
        }
    });
    posix
}

fn current_posix_path_class() -> Rc<RefCell<PyClass>> {
    let path = latest_path_class();
    posix_class_for_path(&path)
}

fn concrete_path_class(requested: Rc<RefCell<PyClass>>) -> Rc<RefCell<PyClass>> {
    let matching_path = PATH_CLASS_GENERATIONS.with(|generations| {
        generations
            .borrow()
            .iter()
            .filter_map(|generation| generation.path.upgrade())
            .find(|path| Rc::ptr_eq(path, &requested))
    });
    matching_path
        .map(|path| posix_class_for_path(&path))
        .unwrap_or(requested)
}

/// Start a fresh class generation for a newly-created `pathlib` module.
pub(super) fn new_path_class_value() -> Value {
    let path = build_path_class();
    register_path_generation(&path);
    Value::py_class(path)
}

/// Return the concrete class paired with the current module's `Path`.
pub(super) fn current_posix_path_class_value() -> Value {
    Value::py_class(current_posix_path_class())
}
