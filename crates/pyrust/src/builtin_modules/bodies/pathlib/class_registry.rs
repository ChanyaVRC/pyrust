//! Generation-aware `PurePath`/`PurePosixPath`/`PureWindowsPath`/
//! `Path`/`PosixPath`/`WindowsPath` class construction and instance storage.
//!
//! Each `pathlib` module generation owns one cohesive 6-class hierarchy plus
//! its `_PathParents` sequence class. Weak registry entries let old imported
//! classes keep their identity without retaining unreachable generations.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use indexmap::IndexMap;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, class_is_subclass_of};
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value, ValueKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum PathFlavor {
    Posix,
    Windows,
}

impl PathFlavor {
    pub(super) fn host() -> Self {
        if cfg!(windows) {
            PathFlavor::Windows
        } else {
            PathFlavor::Posix
        }
    }
}

/// Lexical methods installed on `PurePath` (inherited by all 6 path classes).
const PURE_PATH_METHODS: &[(&str, &str)] = &[
    ("__new__", "pathlib.PurePath.__new__"),
    ("__init__", "pathlib.PurePath.__init__"),
    ("__str__", "pathlib.PurePath.__str__"),
    ("__repr__", "pathlib.PurePath.__repr__"),
    ("__truediv__", "pathlib.PurePath.__truediv__"),
    ("__rtruediv__", "pathlib.PurePath.__rtruediv__"),
    ("__eq__", "pathlib.PurePath.__eq__"),
    ("__lt__", "pathlib.PurePath.__lt__"),
    ("__le__", "pathlib.PurePath.__le__"),
    ("__gt__", "pathlib.PurePath.__gt__"),
    ("__ge__", "pathlib.PurePath.__ge__"),
    ("__hash__", "pathlib.PurePath.__hash__"),
    ("__fspath__", "pathlib.PurePath.__fspath__"),
    ("as_posix", "pathlib.PurePath.as_posix"),
    ("joinpath", "pathlib.PurePath.joinpath"),
    ("is_absolute", "pathlib.PurePath.is_absolute"),
    ("with_name", "pathlib.PurePath.with_name"),
    ("with_stem", "pathlib.PurePath.with_stem"),
    ("with_suffix", "pathlib.PurePath.with_suffix"),
    ("relative_to", "pathlib.PurePath.relative_to"),
    ("is_relative_to", "pathlib.PurePath.is_relative_to"),
    ("match", "pathlib.PurePath.match"),
];

/// Read-only properties installed on `PurePath`.
const PURE_PATH_PROPERTIES: &[(&str, &str)] = &[
    ("name", "pathlib.PurePath.name"),
    ("parent", "pathlib.PurePath.parent"),
    ("parents", "pathlib.PurePath.parents"),
    ("stem", "pathlib.PurePath.stem"),
    ("suffix", "pathlib.PurePath.suffix"),
    ("suffixes", "pathlib.PurePath.suffixes"),
    ("parts", "pathlib.PurePath.parts"),
    ("drive", "pathlib.PurePath.drive"),
    ("root", "pathlib.PurePath.root"),
    ("anchor", "pathlib.PurePath.anchor"),
];

/// Filesystem methods installed on `Path` (NOT on `PurePath`).
const PATH_METHODS: &[(&str, &str)] = &[
    ("exists", "pathlib.Path.exists"),
    ("is_file", "pathlib.Path.is_file"),
    ("is_dir", "pathlib.Path.is_dir"),
    ("is_symlink", "pathlib.Path.is_symlink"),
    ("resolve", "pathlib.Path.resolve"),
    ("read_text", "pathlib.Path.read_text"),
    ("write_text", "pathlib.Path.write_text"),
    ("read_bytes", "pathlib.Path.read_bytes"),
    ("write_bytes", "pathlib.Path.write_bytes"),
    ("open", "pathlib.Path.open"),
    ("mkdir", "pathlib.Path.mkdir"),
    ("rmdir", "pathlib.Path.rmdir"),
    ("unlink", "pathlib.Path.unlink"),
    ("touch", "pathlib.Path.touch"),
    ("rename", "pathlib.Path.rename"),
    ("replace", "pathlib.Path.replace"),
    ("iterdir", "pathlib.Path.iterdir"),
    ("glob", "pathlib.Path.glob"),
    ("rglob", "pathlib.Path.rglob"),
    ("stat", "pathlib.Path.stat"),
    ("lstat", "pathlib.Path.lstat"),
    ("samefile", "pathlib.Path.samefile"),
    ("readlink", "pathlib.Path.readlink"),
];

/// Class methods installed on `Path`.
const PATH_CLASS_METHODS: &[(&str, &str)] =
    &[("cwd", "pathlib.Path.cwd"), ("home", "pathlib.Path.home")];

/// Methods installed on `_PathParents`.
const PATH_PARENTS_METHODS: &[(&str, &str)] = &[
    ("__len__", "pathlib._PathParents.__len__"),
    ("__getitem__", "pathlib._PathParents.__getitem__"),
    ("__iter__", "pathlib._PathParents.__iter__"),
    ("__contains__", "pathlib._PathParents.__contains__"),
    ("__repr__", "pathlib._PathParents.__repr__"),
];

struct PathClassGeneration {
    pure_path: Weak<RefCell<PyClass>>,
    pure_posix: Weak<RefCell<PyClass>>,
    pure_windows: Weak<RefCell<PyClass>>,
    path: Weak<RefCell<PyClass>>,
    posix: Weak<RefCell<PyClass>>,
    windows: Weak<RefCell<PyClass>>,
}

#[derive(Clone)]
struct LiveGeneration {
    pure_path: Rc<RefCell<PyClass>>,
    pure_posix: Rc<RefCell<PyClass>>,
    pure_windows: Rc<RefCell<PyClass>>,
    path: Rc<RefCell<PyClass>>,
    posix: Rc<RefCell<PyClass>>,
    windows: Rc<RefCell<PyClass>>,
}

impl PathClassGeneration {
    fn upgrade(&self) -> Option<LiveGeneration> {
        Some(LiveGeneration {
            pure_path: self.pure_path.upgrade()?,
            pure_posix: self.pure_posix.upgrade()?,
            pure_windows: self.pure_windows.upgrade()?,
            path: self.path.upgrade()?,
            posix: self.posix.upgrade()?,
            windows: self.windows.upgrade()?,
        })
    }
}

// Thread-local justification:
// (a) Why `thread_local!`: Holds `!Send` `Weak<RefCell<PyClass>>` generation
//     bundles for `pathlib`'s 6-class hierarchy, a short-lived strong holder
//     while module initialization installs the 6 constants of a new module
//     generation, and the internal `_PathParents` sequence class.
// (b) Multi-interpreter / multi-threaded impact: Weak per-import entries keep
//     re-imported `pathlib` modules isolated across interpreters on one thread
//     and prune collected generations; separate threads are isolated.
thread_local! {
    static PATH_CLASS_GENERATIONS: RefCell<Vec<PathClassGeneration>> =
        const { RefCell::new(Vec::new()) };
    static PENDING_GENERATION: RefCell<Option<LiveGeneration>> =
        const { RefCell::new(None) };
    static PATH_PARENTS_CLASS: Rc<RefCell<PyClass>> = build_parents_class();
}

fn build_parents_class() -> Rc<RefCell<PyClass>> {
    let mut parents_attrs = IndexMap::new();
    for (short, py_full) in PATH_PARENTS_METHODS {
        parents_attrs.insert((*short).to_string(), Value::builtin_function(py_full));
    }
    parents_attrs.insert("__module__".to_string(), Value::string("pathlib"));
    Rc::new(RefCell::new(PyClass::new(
        "_PathParents",
        "_PathParents",
        None,
        parents_attrs,
    )))
}

fn build_generation() -> LiveGeneration {
    let mut pure_attrs: IndexMap<String, Value> = IndexMap::new();
    for (short, py_full) in PURE_PATH_METHODS {
        pure_attrs.insert((*short).to_string(), Value::builtin_function(py_full));
    }
    for (short, py_full) in PURE_PATH_PROPERTIES {
        let getter = Value::builtin_function(py_full);
        pure_attrs.insert(
            (*short).to_string(),
            pyrust_builtins::property::property(getter, Value::none(), Value::none()),
        );
    }
    pure_attrs.insert("__module__".to_string(), Value::string("pathlib"));
    let pure_path = Rc::new(RefCell::new(PyClass::new(
        "PurePath", "PurePath", None, pure_attrs,
    )));

    let make_single_sub =
        |name: &str, base: &Rc<RefCell<PyClass>>, mut attrs: IndexMap<String, Value>| {
            attrs.insert("__module__".to_string(), Value::string("pathlib"));
            let cls = Rc::new(RefCell::new(PyClass::new(
                name,
                name,
                Some(Rc::clone(base)),
                attrs,
            )));
            base.borrow()
                .subclasses
                .borrow_mut()
                .push(Rc::downgrade(&cls));
            cls
        };

    let pure_posix = make_single_sub("PurePosixPath", &pure_path, IndexMap::new());
    let pure_windows = make_single_sub("PureWindowsPath", &pure_path, IndexMap::new());

    let mut path_attrs: IndexMap<String, Value> = IndexMap::new();
    for (short, py_full) in PATH_METHODS {
        path_attrs.insert((*short).to_string(), Value::builtin_function(py_full));
    }
    for (short, py_full) in PATH_CLASS_METHODS {
        path_attrs.insert(
            (*short).to_string(),
            pyrust_builtins::classmethod::class_method_any(Value::builtin_function(py_full)),
        );
    }
    let path = make_single_sub("Path", &pure_path, path_attrs);

    // CPython 3.12 multiple inheritance:
    //   PosixPath(Path, PurePosixPath)
    //   WindowsPath(Path, PureWindowsPath)
    // Primary base is `Path` (so `__base__ is Path`), extra base is `Pure*Path`.
    let make_concrete =
        |name: &str, path_base: &Rc<RefCell<PyClass>>, pure_flavor_base: &Rc<RefCell<PyClass>>| {
            let mut attrs = IndexMap::new();
            attrs.insert("__module__".to_string(), Value::string("pathlib"));
            let cls = Rc::new(RefCell::new(PyClass {
                extra_bases: vec![Rc::clone(pure_flavor_base)],
                ..PyClass::new(name, name, Some(Rc::clone(path_base)), attrs)
            }));
            path_base
                .borrow()
                .subclasses
                .borrow_mut()
                .push(Rc::downgrade(&cls));
            pure_flavor_base
                .borrow()
                .subclasses
                .borrow_mut()
                .push(Rc::downgrade(&cls));
            cls
        };

    let posix = make_concrete("PosixPath", &path, &pure_posix);
    let windows = make_concrete("WindowsPath", &path, &pure_windows);

    LiveGeneration {
        pure_path,
        pure_posix,
        pure_windows,
        path,
        posix,
        windows,
    }
}

fn push_generation(live: &LiveGeneration) {
    PATH_CLASS_GENERATIONS.with(|gens| {
        let mut gens = gens.borrow_mut();
        gens.retain(|g| g.pure_path.upgrade().is_some());
        gens.push(PathClassGeneration {
            pure_path: Rc::downgrade(&live.pure_path),
            pure_posix: Rc::downgrade(&live.pure_posix),
            pure_windows: Rc::downgrade(&live.pure_windows),
            path: Rc::downgrade(&live.path),
            posix: Rc::downgrade(&live.posix),
            windows: Rc::downgrade(&live.windows),
        });
    });
}

fn latest_generation() -> LiveGeneration {
    if let Some(live) = PENDING_GENERATION.with(|slot| slot.borrow().clone()) {
        return live;
    }
    if let Some(live) = PATH_CLASS_GENERATIONS.with(|gens| {
        gens.borrow()
            .iter()
            .rev()
            .find_map(PathClassGeneration::upgrade)
    }) {
        return live;
    }
    let live = build_generation();
    push_generation(&live);
    live
}

/// Start a fresh 6-class generation for a newly-created `pathlib` module and
/// return its `PurePath` class.
pub(super) fn new_generation_pure_path_value() -> Value {
    let live = build_generation();
    push_generation(&live);
    let pure_path = Rc::clone(&live.pure_path);
    PENDING_GENERATION.with(|slot| *slot.borrow_mut() = Some(live));
    Value::py_class(pure_path)
}

pub(super) fn current_pure_posix_value() -> Value {
    Value::py_class(latest_generation().pure_posix)
}

pub(super) fn current_pure_windows_value() -> Value {
    Value::py_class(latest_generation().pure_windows)
}

pub(super) fn current_path_value() -> Value {
    Value::py_class(latest_generation().path)
}

pub(super) fn current_posix_value() -> Value {
    Value::py_class(latest_generation().posix)
}

pub(super) fn current_windows_value() -> Value {
    let live = PENDING_GENERATION
        .with(|slot| slot.borrow_mut().take())
        .unwrap_or_else(latest_generation);
    Value::py_class(live.windows)
}

pub(super) fn expect_self(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<RefCell<PyInstance>>> {
    match args.first().map(|arg| arg.value.kind()) {
        Some(ValueKind::PyInstance(instance)) => Ok(Rc::clone(instance)),
        _ => Err(PyError::runtime(format!(
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

/// Return whether an instance belongs to any still-live canonical `PurePath`
/// hierarchy. Python-visible names are intentionally not involved.
pub(super) fn is_path_instance(instance: &Rc<RefCell<PyInstance>>) -> bool {
    let class = Rc::clone(&instance.borrow().class);
    PATH_CLASS_GENERATIONS.with(|gens| {
        let mut gens = gens.borrow_mut();
        gens.retain(|g| g.pure_path.upgrade().is_some());
        gens.iter()
            .filter_map(|g| g.pure_path.upgrade())
            .any(|pure_path| class_is_subclass_of(&class, &pure_path))
    })
}

/// Determine the lexical flavor (POSIX vs Windows) for a path class.
pub(super) fn flavor_for_class(class: &Rc<RefCell<PyClass>>) -> PathFlavor {
    PATH_CLASS_GENERATIONS.with(|gens| {
        for g in gens.borrow().iter().rev() {
            if let Some(pw) = g.pure_windows.upgrade()
                && class_is_subclass_of(class, &pw)
            {
                return PathFlavor::Windows;
            }
            if let Some(pp) = g.pure_posix.upgrade()
                && class_is_subclass_of(class, &pp)
            {
                return PathFlavor::Posix;
            }
        }
        PathFlavor::host()
    })
}

pub(super) fn flavor_for_instance(inst: &Rc<RefCell<PyInstance>>) -> PathFlavor {
    let class = Rc::clone(&inst.borrow().class);
    flavor_for_class(&class)
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

/// Construct a `_PathParents` sequence instance holding precomputed parents.
pub(super) fn make_parents_instance(
    owner_inst: &Rc<RefCell<PyInstance>>,
    parents: Vec<Value>,
) -> Value {
    let owner_name = owner_inst.borrow().class.borrow().name.clone();
    let parents_cls = PATH_PARENTS_CLASS.with(Rc::clone);
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_items", Value::tuple(parents));
    attrs.insert("_owner_name", Value::string(owner_name));
    Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: parents_cls,
        attrs,
    })))
}

/// Construct a result through the class bound to `Path.cwd` / `Path.home`.
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
            PyError::runtime("internal: pathlib classmethod missing class receiver".to_string())
        })?;
    interp.call_function_expanded(
        class,
        &[ExpandedCallArg {
            name: None,
            value: Value::string(path),
        }],
    )
}

/// Allocate the bare instance for `PurePath.__new__` / `Path.__new__`.
///
/// Matches CPython 3.12:
/// - `PurePath(...)` -> `PureWindowsPath` on Windows, `PurePosixPath` on POSIX
/// - `Path(...)` -> `WindowsPath` on Windows, `PosixPath` on POSIX
/// - `PosixPath(...)` on Windows -> `NotImplementedError`
/// - `WindowsPath(...)` on POSIX -> `NotImplementedError`
/// - Concrete subclasses preserve the requested subclass.
pub(super) fn path_new(args: &[ExpandedCallArg]) -> Result<Value> {
    let requested = match args.first().map(|arg| arg.value.kind()) {
        Some(ValueKind::PyClass(class)) => Rc::clone(class),
        _ => {
            return Err(PyError::runtime(
                "internal: pathlib.PurePath.__new__ missing class receiver".to_string(),
            ));
        }
    };
    let concrete = resolve_concrete_class(requested)?;
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: concrete,
        attrs: InstanceAttrs::new(),
    }))))
}

fn resolve_concrete_class(requested: Rc<RefCell<PyClass>>) -> Result<Rc<RefCell<PyClass>>> {
    PATH_CLASS_GENERATIONS.with(|gens| {
        for g in gens.borrow().iter().rev() {
            let Some(live) = g.upgrade() else {
                continue;
            };
            if Rc::ptr_eq(&requested, &live.pure_path) {
                return Ok(if cfg!(windows) {
                    live.pure_windows
                } else {
                    live.pure_posix
                });
            }
            if Rc::ptr_eq(&requested, &live.path) {
                return Ok(if cfg!(windows) {
                    live.windows
                } else {
                    live.posix
                });
            }
            if cfg!(windows) && class_is_subclass_of(&requested, &live.posix) {
                let name = requested.borrow().name.clone();
                return Err(PyError::named(
                    "NotImplementedError",
                    format!("cannot instantiate '{name}' on your system"),
                ));
            }
            if !cfg!(windows) && class_is_subclass_of(&requested, &live.windows) {
                let name = requested.borrow().name.clone();
                return Err(PyError::named(
                    "NotImplementedError",
                    format!("cannot instantiate '{name}' on your system"),
                ));
            }
        }
        Ok(requested)
    })
}
