use std::rc::Rc;

use crate::interpreter::{Interpreter, NativeIteratorClass};
use crate::value::ValueKind;

/// Apply native module-specific class wiring before a built-in module enters
/// the import cache. Exact module names and exported attributes belong here;
/// the generic importer only invokes this hook.
pub(crate) fn prepare_builtin_module(name: &str, interpreter: &mut Interpreter, module: &Value) {
    match name {
        "sys" => interpreter.initialize_system_module(module),
        "builtins" => prepare_builtins_module(module),
        "collections" => prepare_collections_module(module),
        "contextlib" => prepare_contextlib_module(module),
        "functools" => prepare_functools_module(module),
        "itertools" => prepare_itertools_module(module),
        "io" => prepare_io_module(module),
        "warnings" => prepare_warnings_module(interpreter, module),
        _ => {}
    }
}

/// Replace constructor registry tokens with the canonical Python type objects
/// and install the built-in exception hierarchy.
///
/// This is the single finalization path for both the thread-local globals
/// provider and modules loaded through `import builtins`.
pub(crate) fn prepare_builtins_module(module: &Value) {
    let ValueKind::PyModule(module) = module.kind() else {
        return;
    };
    for type_name in [
        "bool",
        "bytearray",
        "bytes",
        "complex",
        "dict",
        "enumerate",
        "filter",
        "float",
        "frozenset",
        "int",
        "list",
        "map",
        "range",
        "reversed",
        "set",
        "slice",
        "str",
        "tuple",
        "zip",
    ] {
        if let Some(class) = crate::interpreter::primitive_class_by_name(type_name) {
            module
                .borrow_mut()
                .insert_attr(type_name.to_string(), Value::py_class(class));
        }
    }
    module.borrow_mut().insert_attr(
        "type".to_string(),
        Value::py_class(crate::interpreter::type_class_singleton()),
    );
    module.borrow_mut().insert_attr(
        "object".to_string(),
        Value::py_class(crate::interpreter::object_class_singleton()),
    );

    let exception_classes = crate::interpreter::build_exc_class_map();
    let module_only_classes = exception_classes
        .iter()
        .filter(|(name, _)| name.contains('.'))
        .map(|(_, class)| Rc::as_ptr(class))
        .collect::<std::collections::HashSet<_>>();
    for (exception_name, exception_class) in &exception_classes {
        if !exception_name.contains('.')
            && !module_only_classes.contains(&Rc::as_ptr(exception_class))
        {
            module.borrow_mut().insert_attr(
                exception_name.to_string(),
                Value::py_class(Rc::clone(exception_class)),
            );
        }
    }
}

fn prepare_collections_module(module: &Value) {
    let ValueKind::PyModule(module) = module.kind() else {
        return;
    };
    // CPython publishes the forward deque iterator class under its private
    // implementation name.  The reverse iterator class remains discoverable
    // only through `type(reversed(deque()))` and is not a module attribute.
    module.borrow_mut().insert_attr(
        "_deque_iterator".to_string(),
        Value::py_class(NativeIteratorClass::Deque.singleton()),
    );
    let Some(dictionary_class) = crate::interpreter::primitive_class_by_name("dict") else {
        return;
    };
    for class_name in ["Counter", "defaultdict"] {
        let class = module.borrow().attrs.get(class_name).cloned();
        let Some(ValueKind::PyClass(class)) = class.as_ref().map(Value::kind) else {
            continue;
        };
        if class.borrow().base.is_none() {
            class.borrow_mut().base = Some(Rc::clone(&dictionary_class));
            dictionary_class
                .borrow()
                .subclasses
                .borrow_mut()
                .push(Rc::downgrade(class));
        }
    }
}

fn prepare_functools_module(module: &Value) {
    functools::prepare_module_classes(module);
}

fn prepare_contextlib_module(module: &Value) {
    contextlib::prepare_module_classes(module);
}

fn prepare_itertools_module(module: &Value) {
    itertools::prepare_module_classes(module);
    let ValueKind::PyModule(module) = module.kind() else {
        return;
    };
    let Some(chain) = module.borrow().attrs.get("chain").cloned() else {
        return;
    };
    let ValueKind::PyClass(chain_class) = chain.kind() else {
        return;
    };

    // Each import creates a fresh chain class. Publish that generation for
    // internal factories, then install the C-style classmethod descriptor so
    // `from_iterable` receives the exact owner (including old generations and
    // subclasses) instead of consulting a process-global display name.
    crate::interpreter::set_itertools_chain_class(Rc::clone(chain_class));
    let from_iterable = chain_class.borrow().attrs.get("from_iterable").cloned();
    if let Some(from_iterable) = from_iterable {
        let descriptor = pyrust_builtins::classmethod::native_class_method_descriptor(
            from_iterable,
            chain_class,
            "from_iterable",
        );
        chain_class
            .borrow_mut()
            .attrs
            .insert("from_iterable".to_string(), descriptor);
    }
}

fn prepare_io_module(module: &Value) {
    let ValueKind::PyModule(module) = module.kind() else {
        return;
    };
    if let Some(exception) = crate::interpreter::build_exc_class_map()
        .get("io.UnsupportedOperation")
        .map(Rc::clone)
    {
        module.borrow_mut().insert_attr(
            "UnsupportedOperation".to_string(),
            Value::py_class(exception),
        );
    }
    for class_name in ["StringIO", "BytesIO"] {
        let class = module.borrow().attrs.get(class_name).cloned();
        let Some(ValueKind::PyClass(class)) = class.as_ref().map(Value::kind) else {
            continue;
        };
        let getter = class.borrow().attrs.get("closed").cloned();
        if let Some(getter) = getter {
            let property =
                pyrust_builtins::property::property(getter, Value::none(), Value::none());
            class
                .borrow_mut()
                .attrs
                .insert("closed".to_string(), property);
        }
    }
}

fn prepare_warnings_module(interpreter: &Interpreter, module: &Value) {
    warnings::prepare_module(interpreter, module);
}
