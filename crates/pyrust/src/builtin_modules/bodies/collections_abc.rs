// `collections.abc` / `_collections_abc` — 100% native-Rust implementation of
// CPython 3.12's `Lib/_collections_abc.py` (issue #3480); there is no
// Python-source injection.
//
// `pyrust_module!` below declares every ABC with its bases, its structural
// `__subclasshook__`, its `__class_getitem__`, its abstract-method bodies, and
// every mixin method. The first build of these classes becomes the canonical
// set shared by `collections.abc`, `_collections_abc`, and the native modules
// that register virtual subclasses (`array`, `collections`, `weakref`). The
// one-time wiring in `collections_abc/registration.rs` then turns them into
// genuine ABCs: `abc.ABCMeta` metaclass (`_DeprecateByteStringMeta` for
// `ByteString`), `__slots__`, `_abc_impl`, `@abstractmethod` descriptors and
// `__abstractmethods__`, and CPython's `register()` calls.
//
// Reference: <https://docs.python.org/3/library/collections.abc.html>

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValue, PyValueRef};
use crate::value::Value;

#[path = "collections_abc/abstracts.rs"]
mod abstracts;
#[path = "collections_abc/callable_alias.rs"]
mod callable_alias;
#[path = "collections_abc/definitions.rs"]
mod definitions;
#[path = "collections_abc/hooks.rs"]
mod hooks;
#[path = "collections_abc/mixins.rs"]
mod mixins;
#[path = "collections_abc/registration.rs"]
mod registration;
#[path = "collections_abc/support.rs"]
mod support;

use self::abstracts::{Body, Shape, call_abstract};

pub(crate) use self::registration::{
    populate_private_module, post_load_init, register_virtual_mutable_sequence,
    register_virtual_mutable_set,
};

/// `cls.__class_getitem__(item)` = `GenericAlias(cls, item)`.
fn class_getitem(cls: &Value, item: &Value) -> Value {
    let args = match item.kind() {
        crate::value::ValueKind::Tuple(_) => item.clone(),
        _ => Value::tuple(vec![item.clone()]),
    };
    pyrust_builtins::generic_alias::generic_alias(cls.clone(), args)
}

const SELF: &[&str] = &["self"];
const SELF_VALUE: &[&str] = &["self", "value"];
const SELF_KEY: &[&str] = &["self", "key"];
const SELF_INDEX: &[&str] = &["self", "index"];
const SELF_KEY_VALUE: &[&str] = &["self", "key", "value"];
const SELF_INDEX_VALUE: &[&str] = &["self", "index", "value"];

pyrust_module! {
    /// `_check_methods(C, *methods)`.
    #[py_name = "_check_methods"]
    fn check_methods(
        #[positional_only] subclass: PyValueRef<'_>,
        #[varargs] methods: &[Value],
    ) -> Result<Value> {
        let names: Vec<String> = methods
            .iter()
            .map(|method| method.as_str().map(str::to_string).unwrap_or_default())
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        hooks::check_methods(_interp, subclass.0, &names)
    }

    /// `_is_param_expr(obj)`.
    #[py_name = "_is_param_expr"]
    fn is_param_expr(#[positional_only] obj: PyValueRef<'_>) -> Result<Value> {
        Ok(Value::bool_(callable_alias::is_param_expr(_interp, obj.0)?))
    }

    /// `_type_repr(obj)`.
    #[py_name = "_type_repr"]
    fn type_repr(#[positional_only] obj: PyValueRef<'_>) -> Result<Value> {
        callable_alias::type_repr(_interp, obj.0)
    }

    // ── One-trick ponies ─────────────────────────────────────────────────────

    class Hashable {
        fn __hash__(args) -> Result<Value> {
            call_abstract(_interp, args, "Hashable.__hash__", Shape::Params(SELF), Body::ReturnZero)
        }

        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            hooks::subclasshook(_interp, cls.0, subclass.0)
        }
    }

    class Awaitable {
        fn __await__(args) -> Result<Value> {
            call_abstract(_interp, args, "Awaitable.__await__", Shape::Params(SELF), Body::YieldOnce)
        }

        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            hooks::subclasshook(_interp, cls.0, subclass.0)
        }

        #[classmethod]
        fn __class_getitem__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] item: PyValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(class_getitem(cls.0, item.0))
        }
    }

    class Coroutine("Awaitable") {
        fn send(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "Coroutine.send",
                Shape::Params(SELF_VALUE),
                Body::Raise("StopIteration"),
            )
        }

        fn throw(args) -> Result<Value> {
            call_abstract(_interp, args, "Coroutine.throw", Shape::Throw, Body::Throw)
        }

        fn close(args) -> Result<Value> {
            mixins::close(_interp, args, "Coroutine.close", "coroutine")
        }

        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            hooks::subclasshook(_interp, cls.0, subclass.0)
        }
    }

    class AsyncIterable {
        fn __aiter__(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "AsyncIterable.__aiter__",
                Shape::Params(SELF),
                Body::NewAsyncIterator,
            )
        }

        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            hooks::subclasshook(_interp, cls.0, subclass.0)
        }

        #[classmethod]
        fn __class_getitem__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] item: PyValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(class_getitem(cls.0, item.0))
        }
    }

    class AsyncIterator("AsyncIterable") {
        fn __anext__(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "AsyncIterator.__anext__",
                Shape::Params(SELF),
                Body::AsyncRaise("StopAsyncIteration"),
            )
        }

        fn __aiter__(args) -> Result<Value> {
            let _ = _interp;
            mixins::return_self("AsyncIterator.__aiter__", args)
        }

        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            hooks::subclasshook(_interp, cls.0, subclass.0)
        }
    }

    class AsyncGenerator("AsyncIterator") {
        fn __anext__(args) -> Result<Value> {
            abstracts::async_generator_anext(_interp, args)
        }

        fn asend(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "AsyncGenerator.asend",
                Shape::Params(SELF_VALUE),
                Body::AsyncRaise("StopAsyncIteration"),
            )
        }

        fn athrow(args) -> Result<Value> {
            call_abstract(_interp, args, "AsyncGenerator.athrow", Shape::Throw, Body::AsyncThrow)
        }

        fn aclose(args) -> Result<Value> {
            abstracts::async_generator_aclose(_interp, args)
        }

        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            hooks::subclasshook(_interp, cls.0, subclass.0)
        }
    }

    class Iterable {
        fn __iter__(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "Iterable.__iter__",
                Shape::Params(SELF),
                Body::EmptyGenerator,
            )
        }

        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            hooks::subclasshook(_interp, cls.0, subclass.0)
        }

        #[classmethod]
        fn __class_getitem__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] item: PyValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(class_getitem(cls.0, item.0))
        }
    }

    class Iterator("Iterable") {
        fn __next__(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "Iterator.__next__",
                Shape::Params(SELF),
                Body::Raise("StopIteration"),
            )
        }

        fn __iter__(args) -> Result<Value> {
            let _ = _interp;
            mixins::return_self("Iterator.__iter__", args)
        }

        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            hooks::subclasshook(_interp, cls.0, subclass.0)
        }
    }

    class Reversible("Iterable") {
        fn __reversed__(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "Reversible.__reversed__",
                Shape::Params(SELF),
                Body::EmptyGenerator,
            )
        }

        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            hooks::subclasshook(_interp, cls.0, subclass.0)
        }
    }

    class Generator("Iterator") {
        fn __next__(args) -> Result<Value> {
            mixins::generator_next(_interp, args)
        }

        fn send(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "Generator.send",
                Shape::Params(SELF_VALUE),
                Body::Raise("StopIteration"),
            )
        }

        fn throw(args) -> Result<Value> {
            call_abstract(_interp, args, "Generator.throw", Shape::Throw, Body::Throw)
        }

        fn close(args) -> Result<Value> {
            mixins::close(_interp, args, "Generator.close", "generator")
        }

        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            hooks::subclasshook(_interp, cls.0, subclass.0)
        }
    }

    class Sized {
        fn __len__(args) -> Result<Value> {
            call_abstract(_interp, args, "Sized.__len__", Shape::Params(SELF), Body::ReturnZero)
        }

        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            hooks::subclasshook(_interp, cls.0, subclass.0)
        }
    }

    class Container {
        fn __contains__(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "Container.__contains__",
                Shape::Params(&["self", "x"]),
                Body::ReturnFalse,
            )
        }

        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            hooks::subclasshook(_interp, cls.0, subclass.0)
        }

        #[classmethod]
        fn __class_getitem__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] item: PyValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(class_getitem(cls.0, item.0))
        }
    }

    class Collection("Sized", "Iterable", "Container") {
        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            hooks::subclasshook(_interp, cls.0, subclass.0)
        }
    }

    class Buffer {
        fn __buffer__(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "Buffer.__buffer__",
                Shape::PositionalOne,
                Body::Raise("NotImplementedError"),
            )
        }

        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            hooks::subclasshook(_interp, cls.0, subclass.0)
        }
    }

    class _CallableGenericAlias {
        constants {
            "__doc__" => Value::string(callable_alias::CALLABLE_ALIAS_DOC),
            "__slots__" => Value::tuple(Vec::new()),
        }

        fn __new__(
            #[positional_only] _cls: PyValueRef<'_>,
            origin: PyValue,
            args: PyValue,
        ) -> Result<Value> {
            callable_alias::new_alias(_interp, &origin.0, &args.0)
        }
    }

    class Callable {
        fn __call__(args) -> Result<Value> {
            call_abstract(_interp, args, "Callable.__call__", Shape::Any, Body::ReturnFalse)
        }

        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            hooks::subclasshook(_interp, cls.0, subclass.0)
        }

        #[classmethod]
        fn __class_getitem__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] item: PyValueRef<'_>,
        ) -> Result<Value> {
            callable_alias::new_alias(_interp, cls.0, item.0)
        }
    }

    // ── Sets ─────────────────────────────────────────────────────────────────

    class Set("Collection") {
        fn __le__(args) -> Result<Value> {
            mixins::set_le(_interp, args)
        }

        fn __lt__(args) -> Result<Value> {
            mixins::set_lt(_interp, args)
        }

        fn __gt__(args) -> Result<Value> {
            mixins::set_gt(_interp, args)
        }

        fn __ge__(args) -> Result<Value> {
            mixins::set_ge(_interp, args)
        }

        fn __eq__(args) -> Result<Value> {
            mixins::set_eq(_interp, args)
        }

        fn __and__(args) -> Result<Value> {
            mixins::set_and(_interp, args, "Set.__and__")
        }

        fn isdisjoint(args) -> Result<Value> {
            mixins::set_isdisjoint(_interp, args)
        }

        fn __or__(args) -> Result<Value> {
            mixins::set_or(_interp, args, "Set.__or__")
        }

        fn __sub__(args) -> Result<Value> {
            mixins::set_sub(_interp, args)
        }

        fn __rsub__(args) -> Result<Value> {
            mixins::set_rsub(_interp, args)
        }

        fn __xor__(args) -> Result<Value> {
            mixins::set_xor(_interp, args, "Set.__xor__")
        }

        fn _hash(args) -> Result<Value> {
            mixins::set_hash(_interp, args)
        }

        #[classmethod]
        fn _from_iterable(
            #[positional_only] cls: PyValueRef<'_>,
            it: PyValueRef<'_>,
        ) -> Result<Value> {
            mixins::set_from_iterable(_interp, cls.0, it.0)
        }
    }

    class MutableSet("Set") {
        fn add(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "MutableSet.add",
                Shape::Params(SELF_VALUE),
                Body::Raise("NotImplementedError"),
            )
        }

        fn discard(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "MutableSet.discard",
                Shape::Params(SELF_VALUE),
                Body::Raise("NotImplementedError"),
            )
        }

        fn remove(args) -> Result<Value> {
            mixins::mutable_set_remove(_interp, args)
        }

        fn pop(args) -> Result<Value> {
            mixins::mutable_set_pop(_interp, args)
        }

        fn clear(args) -> Result<Value> {
            mixins::mutable_set_clear(_interp, args)
        }

        fn __ior__(args) -> Result<Value> {
            mixins::mutable_set_ior(_interp, args)
        }

        fn __iand__(args) -> Result<Value> {
            mixins::mutable_set_iand(_interp, args)
        }

        fn __ixor__(args) -> Result<Value> {
            mixins::mutable_set_ixor(_interp, args)
        }

        fn __isub__(args) -> Result<Value> {
            mixins::mutable_set_isub(_interp, args)
        }
    }

    // ── Mappings ─────────────────────────────────────────────────────────────

    class Mapping("Collection") {
        constants {
            "__reversed__" => Value::none(),
        }

        fn __getitem__(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "Mapping.__getitem__",
                Shape::Params(SELF_KEY),
                Body::Raise("KeyError"),
            )
        }

        fn get(args) -> Result<Value> {
            mixins::mapping_get(_interp, args)
        }

        fn __contains__(args) -> Result<Value> {
            mixins::mapping_contains(_interp, args)
        }

        fn keys(args) -> Result<Value> {
            mixins::mapping_view(_interp, args, "Mapping.keys", definitions::Abc::KeysView)
        }

        fn items(args) -> Result<Value> {
            mixins::mapping_view(_interp, args, "Mapping.items", definitions::Abc::ItemsView)
        }

        fn values(args) -> Result<Value> {
            mixins::mapping_view(_interp, args, "Mapping.values", definitions::Abc::ValuesView)
        }

        fn __eq__(args) -> Result<Value> {
            mixins::mapping_eq(_interp, args)
        }
    }

    class MappingView("Sized") {
        fn __init__(args) -> Result<Value> {
            mixins::mapping_view_init(_interp, args)
        }

        fn __len__(args) -> Result<Value> {
            mixins::mapping_view_len(_interp, args)
        }

        fn __repr__(args) -> Result<Value> {
            mixins::mapping_view_repr(_interp, args)
        }

        #[classmethod]
        fn __class_getitem__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] item: PyValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(class_getitem(cls.0, item.0))
        }
    }

    class KeysView("MappingView", "Set") {
        fn __contains__(args) -> Result<Value> {
            mixins::keys_view_contains(_interp, args)
        }

        fn __iter__(args) -> Result<Value> {
            mixins::keys_view_iter(_interp, args)
        }

        #[classmethod]
        fn _from_iterable(
            #[positional_only] cls: PyValueRef<'_>,
            it: PyValueRef<'_>,
        ) -> Result<Value> {
            let _ = cls;
            mixins::view_from_iterable(_interp, it.0)
        }
    }

    class ItemsView("MappingView", "Set") {
        fn __contains__(args) -> Result<Value> {
            mixins::items_view_contains(_interp, args)
        }

        fn __iter__(args) -> Result<Value> {
            mixins::items_view_iter(_interp, args)
        }

        #[classmethod]
        fn _from_iterable(
            #[positional_only] cls: PyValueRef<'_>,
            it: PyValueRef<'_>,
        ) -> Result<Value> {
            let _ = cls;
            mixins::view_from_iterable(_interp, it.0)
        }
    }

    class ValuesView("MappingView", "Collection") {
        fn __contains__(args) -> Result<Value> {
            mixins::values_view_contains(_interp, args)
        }

        fn __iter__(args) -> Result<Value> {
            mixins::values_view_iter(_interp, args)
        }
    }

    class MutableMapping("Mapping") {
        fn __setitem__(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "MutableMapping.__setitem__",
                Shape::Params(SELF_KEY_VALUE),
                Body::Raise("KeyError"),
            )
        }

        fn __delitem__(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "MutableMapping.__delitem__",
                Shape::Params(SELF_KEY),
                Body::Raise("KeyError"),
            )
        }

        fn pop(args) -> Result<Value> {
            mixins::mutable_mapping_pop(_interp, args)
        }

        fn popitem(args) -> Result<Value> {
            mixins::mutable_mapping_popitem(_interp, args)
        }

        fn clear(args) -> Result<Value> {
            mixins::mutable_mapping_clear(_interp, args)
        }

        fn update(args) -> Result<Value> {
            mixins::mutable_mapping_update(_interp, args)
        }

        fn setdefault(args) -> Result<Value> {
            mixins::mutable_mapping_setdefault(_interp, args)
        }
    }

    // ── Sequences ────────────────────────────────────────────────────────────

    class Sequence("Reversible", "Collection") {
        fn __getitem__(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "Sequence.__getitem__",
                Shape::Params(SELF_INDEX),
                Body::Raise("IndexError"),
            )
        }

        fn __iter__(args) -> Result<Value> {
            mixins::sequence_iter(_interp, args)
        }

        fn __contains__(args) -> Result<Value> {
            mixins::sequence_contains(_interp, args)
        }

        fn __reversed__(args) -> Result<Value> {
            mixins::sequence_reversed(_interp, args)
        }

        fn index(args) -> Result<Value> {
            mixins::sequence_index(_interp, args)
        }

        fn count(args) -> Result<Value> {
            mixins::sequence_count(_interp, args)
        }
    }

    class _DeprecateByteStringMeta {
        fn __new__(
            #[positional_only] mcls: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            registration::deprecate_meta_new(_interp, mcls.0, args, kwargs)
        }

        fn __instancecheck__(
            #[positional_only] cls: PyValueRef<'_>,
            instance: PyValue,
        ) -> Result<Value> {
            registration::deprecate_meta_instancecheck(_interp, cls.0, &instance.0)
        }
    }

    class ByteString("Sequence") {
    }

    class MutableSequence("Sequence") {
        fn __setitem__(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "MutableSequence.__setitem__",
                Shape::Params(SELF_INDEX_VALUE),
                Body::Raise("IndexError"),
            )
        }

        fn __delitem__(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "MutableSequence.__delitem__",
                Shape::Params(SELF_INDEX),
                Body::Raise("IndexError"),
            )
        }

        fn insert(args) -> Result<Value> {
            call_abstract(
                _interp,
                args,
                "MutableSequence.insert",
                Shape::Params(SELF_INDEX_VALUE),
                Body::Raise("IndexError"),
            )
        }

        fn append(args) -> Result<Value> {
            mixins::mutable_sequence_append(_interp, args)
        }

        fn clear(args) -> Result<Value> {
            mixins::mutable_sequence_clear(_interp, args)
        }

        fn reverse(args) -> Result<Value> {
            mixins::mutable_sequence_reverse(_interp, args)
        }

        fn extend(args) -> Result<Value> {
            mixins::mutable_sequence_extend(_interp, args)
        }

        fn pop(args) -> Result<Value> {
            mixins::mutable_sequence_pop(_interp, args)
        }

        fn remove(args) -> Result<Value> {
            mixins::mutable_sequence_remove(_interp, args)
        }

        fn __iadd__(args) -> Result<Value> {
            mixins::mutable_sequence_iadd(_interp, args)
        }
    }

    // ── `@abstractmethod` descriptors (hidden) ───────────────────────────────

    #[native_state(registration::AbstractMethodState)]
    class _AbstractMethod {
        constants {
            "__isabstractmethod__" => Value::bool_(true),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValue,
            name: PyValue,
            qualname: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                registration::AbstractMethodState {
                    func: func.0,
                    name: name.0.as_str().unwrap_or_default().to_string(),
                    qualname: qualname.0.as_str().unwrap_or_default().to_string(),
                },
            )?;
            Ok(Value::none())
        }

        fn __get__(
            #[positional_only] self_val: PyValueRef<'_>,
            obj: PyValue,
            #[default(OptionalArg::Omitted)] _objtype: OptValueRef<'_>,
        ) -> Result<Value> {
            if obj.0.is_none() {
                return Ok(self_val.0.clone());
            }
            let func = with_state(self_val.0, |st| Ok(st.func.clone()))?;
            registration::bind_abstract_method(_interp, func, &obj.0)
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let func = with_state(self_val.0, |st| Ok(st.func.clone()))?;
            registration::call_with(_interp, func, None, args, kwargs)
        }

        #[getter]
        fn __name__(#[state] st: &registration::AbstractMethodState) -> Result<Value> {
            Ok(Value::string(&st.name))
        }

        #[getter]
        fn __qualname__(#[state] st: &registration::AbstractMethodState) -> Result<Value> {
            Ok(Value::string(&st.qualname))
        }
    }

    #[native_state(registration::BoundAbstractMethodState)]
    class _BoundAbstractMethod {
        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let (func, instance) =
                with_state(self_val.0, |st| Ok((st.func.clone(), st.instance.clone())))?;
            registration::call_with(_interp, func, Some(instance), args, kwargs)
        }

        #[getter]
        fn __self__(#[state] st: &registration::BoundAbstractMethodState) -> Result<Value> {
            Ok(st.instance.clone())
        }

        #[getter]
        fn __func__(#[state] st: &registration::BoundAbstractMethodState) -> Result<Value> {
            Ok(st.func.clone())
        }
    }
}
