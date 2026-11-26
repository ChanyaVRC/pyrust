//! `AbstractContextManager` / `AbstractAsyncContextManager`: the structural
//! `__subclasshook__`, `__class_getitem__`, and the `abc.ABC` wiring applied
//! to every class of the module that derives from either one.

use std::cell::RefCell;
use std::rc::Rc;

use crate::builtin_modules::macros::link_class_bases;
use crate::error::Result;
use crate::interpreter::{ExpandedCallArg, Interpreter, class_is_subclass_of};
use crate::value::{InstanceAttrs, PyClass, PyInstance, PyKey, PySet, Value, ValueKind};

use super::coroutine;
use super::support::{expect_self, instance_attr};

type ClassRc = Rc<RefCell<PyClass>>;

/// `AbstractContextManager.__class_getitem__(cls, item)`.
pub(super) fn class_getitem(cls: &Value, item: &Value) -> Value {
    let args = match item.kind() {
        ValueKind::Tuple(_) => item.clone(),
        _ => Value::tuple(vec![item.clone()]),
    };
    pyrust_builtins::generic_alias::generic_alias(cls.clone(), args)
}

/// `AbstractAsyncContextManager.__aenter__`: `async def` returning `self`.
pub(super) fn async_enter(receiver: Value) -> Result<Value> {
    coroutine::ready(receiver)
}

/// `AbstractAsyncContextManager.__aexit__`: `async def` returning `None`.
pub(super) fn async_exit() -> Result<Value> {
    coroutine::ready(Value::none())
}

/// The shared `__subclasshook__` body: `cls` must be the exact ABC (`class_name`
/// declares `hook_name` itself); any other class defers to the normal ABC
/// machinery by returning `NotImplemented`.
pub(super) fn subclasshook(
    interp: &mut Interpreter,
    cls: &Value,
    subclass: &Value,
    class_name: &str,
    methods: [&str; 2],
) -> Result<Value> {
    let ValueKind::PyClass(class) = cls.kind() else {
        return Ok(Value::not_implemented());
    };
    let is_the_abc = {
        let class = class.borrow();
        class.name == class_name && class.attrs.contains_key("__subclasshook__")
    };
    if !is_the_abc {
        return Ok(Value::not_implemented());
    }
    check_methods(interp, subclass, &methods)
}

/// CPython's private `contextlib._check_methods(C, *methods)`.
fn check_methods(interp: &mut Interpreter, subclass: &Value, methods: &[&str]) -> Result<Value> {
    let mro = interp.get_attr(subclass, "__mro__")?;
    let mro = interp.collect_iterable(&mro)?;
    for method in methods {
        let mut found = false;
        for base in &mro {
            let ValueKind::PyClass(base) = base.kind() else {
                continue;
            };
            let entry = base.borrow().attrs.get(*method).cloned();
            if let Some(entry) = entry {
                if entry.is_none() {
                    return Ok(Value::not_implemented());
                }
                found = true;
                break;
            }
        }
        if !found {
            return Ok(Value::not_implemented());
        }
    }
    Ok(Value::bool_(true))
}

fn frozen_names(names: &[&str]) -> Value {
    let mut set = PySet::default();
    for name in names {
        set.insert(PyKey::str_from(name));
    }
    pyrust_builtins::frozenset::frozenset(set)
}

/// `abc.abstractmethod` for a native method: replace `class.<name>` with an
/// `_AbstractMethod` instance (a callable carrying `__isabstractmethod__`),
/// since native method descriptors cannot hold attributes.
fn mark_abstract_method(marker: &ClassRc, class: &ClassRc, name: &str, is_async: bool) {
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_async", Value::bool_(is_async));
    let method = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(marker),
        attrs,
    })));
    class.borrow_mut().attrs.insert(name.to_string(), method);
}

/// `_AbstractMethod.__call__`: the body of the abstract `__exit__` /
/// `__aexit__`, returning `None` (or an already-completed coroutine).
pub(super) fn abstract_method_call(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let is_async = instance_attr(&inst, "_async")
        .is_some_and(|flag| matches!(flag.kind(), ValueKind::Bool(true)));
    if is_async {
        coroutine::ready(Value::none())
    } else {
        Ok(Value::none())
    }
}

fn module_classes(module: &Value) -> Vec<(String, ClassRc)> {
    let ValueKind::PyModule(module) = module.kind() else {
        return Vec::new();
    };
    module
        .borrow()
        .attrs
        .iter()
        .filter_map(|(name, value)| match value.kind() {
            ValueKind::PyClass(class) => Some((name.clone(), Rc::clone(class))),
            _ => None,
        })
        .collect()
}

/// Give the two ABCs `abc.ABC` as their base and `abc.ABCMeta` as their
/// metaclass, then do the same for every class of the module that derives from
/// either of them, mirroring what `ABCMeta.__new__` does for a `class`
/// statement.
pub(super) fn wire_abc_classes(interp: &mut Interpreter, module: &Value) {
    let Ok(abc_module) = interp.load_module("abc") else {
        return;
    };
    let (Ok(meta_value), Ok(base_value)) = (
        interp.get_attr(&abc_module, "ABCMeta"),
        interp.get_attr(&abc_module, "ABC"),
    ) else {
        return;
    };
    let (ValueKind::PyClass(meta), ValueKind::PyClass(abc_base)) =
        (meta_value.kind(), base_value.kind())
    else {
        return;
    };
    let classes = module_classes(module);
    let find = |wanted: &str| {
        classes
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, class)| Rc::clone(class))
    };
    let (Some(sync_abc), Some(async_abc)) = (
        find("AbstractContextManager"),
        find("AbstractAsyncContextManager"),
    ) else {
        return;
    };
    let marker = find("_AbstractMethod");
    for (abc_class, abstract_method, is_async) in [
        (&sync_abc, "__exit__", false),
        (&async_abc, "__aexit__", true),
    ] {
        link_class_bases(abc_class, vec![Rc::clone(abc_base)]);
        if let Some(marker) = &marker {
            mark_abstract_method(marker, abc_class, abstract_method, is_async);
        }
    }
    for (_, class) in &classes {
        let derived_from_sync = class_is_subclass_of(class, &sync_abc);
        let derived_from_async = class_is_subclass_of(class, &async_abc);
        if !derived_from_sync && !derived_from_async {
            continue;
        }
        let abstract_names: &[&str] = if Rc::ptr_eq(class, &sync_abc) {
            &["__exit__"]
        } else if Rc::ptr_eq(class, &async_abc) {
            &["__aexit__"]
        } else {
            &[]
        };
        let mut class = class.borrow_mut();
        class.metatype = Some(Rc::clone(meta));
        class.attrs.insert(
            "__abstractmethods__".to_string(),
            frozen_names(abstract_names),
        );
        class.attrs.insert(
            "_abc_impl".to_string(),
            crate::builtin_modules::abc::new_abc_impl(),
        );
        class.bump_mutation_version();
    }
}
