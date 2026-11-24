// `collections.abc` module — abstract base class stubs and structural hooks.
//
// This file is the thin Python-registration facade. Canonical ABC singletons,
// structural `__instancecheck__` / `__subclasshook__` / `__subclasscheck__`
// dispatch, and hierarchy/mixin registration live in submodules under
// `bodies/collections_abc/`.
//
// Reference: <https://docs.python.org/3/library/collections.abc.html>

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;
use pyrust_derive::pyrust_module;

#[path = "collections_abc/definitions.rs"]
mod definitions;
#[path = "collections_abc/hooks.rs"]
mod hooks;
#[path = "collections_abc/registration.rs"]
mod registration;

use self::definitions::Abc;
pub(crate) use self::registration::{
    inject_python_members, register_virtual_mapping, register_virtual_mutable_sequence,
};

pyrust_module! {
    constants {
        "Container"        => registration::make_abc_module(),
        "Hashable"         => Value::py_class(Abc::Hashable.class()),
        "Iterable"         => Value::py_class(Abc::Iterable.class()),
        "Iterator"         => Value::py_class(Abc::Iterator.class()),
        "Reversible"       => Value::py_class(Abc::Reversible.class()),
        "Generator"        => Value::py_class(Abc::Generator.class()),
        "Sized"            => Value::py_class(Abc::Sized.class()),
        "Collection"       => Value::py_class(Abc::Collection.class()),
        "Callable"         => Value::py_class(Abc::Callable.class()),
        "Sequence"         => Value::py_class(Abc::Sequence.class()),
        "MutableSequence"  => Value::py_class(Abc::MutableSequence.class()),
        "Set"              => Value::py_class(Abc::Set.class()),
        "MutableSet"       => Value::py_class(Abc::MutableSet.class()),
        "Mapping"          => Value::py_class(Abc::Mapping.class()),
        "MutableMapping"   => Value::py_class(Abc::MutableMapping.class()),
        "MappingView"      => Value::py_class(Abc::MappingView.class()),
        "KeysView"         => Value::py_class(Abc::KeysView.class()),
        "ItemsView"        => Value::py_class(Abc::ItemsView.class()),
        "ValuesView"       => Value::py_class(Abc::ValuesView.class()),
        "Awaitable"        => Value::py_class(Abc::Awaitable.class()),
        "Coroutine"        => Value::py_class(Abc::Coroutine.class()),
        "AsyncIterable"    => Value::py_class(Abc::AsyncIterable.class()),
        "AsyncIterator"    => Value::py_class(Abc::AsyncIterator.class()),
        "AsyncGenerator"   => Value::py_class(Abc::AsyncGenerator.class()),
        "Buffer"           => Value::py_class(Abc::Buffer.class())
    }

    #[py_name = "__instancecheck__"]
    fn abc_instancecheck(args) -> Result<Value> {
        let _ = _interp;
        hooks::instancecheck(args, FN_NAME)
    }

    #[py_name = "__subclasshook__"]
    fn abc_subclasshook_method(args) -> Result<Value> {
        let _ = _interp;
        hooks::subclasshook_method(args, FN_NAME)
    }

    #[py_name = "__subclasscheck__"]
    fn abc_subclasscheck(args) -> Result<Value> {
        let _ = _interp;
        hooks::subclasscheck(args, FN_NAME)
    }
}
