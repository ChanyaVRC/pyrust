// `collections` registration facade — included into `pub mod collections { … }`
// declared by `pyrust_builtin_modules!` in `builtin_modules/mod.rs`.
//
// Python-visible classes and callables stay declared here while focused files
// under `bodies/collections/` implement each domain:
//
// - `backing`: shared dict-subclass backing, views, and mapping protocol;
// - `canonical_identity`: generation-aware weak identities for native classes;
// - `module_setup`: Python-source injection (`OrderedDict`, `namedtuple`, …) & tagging;
// - `counter` / `counter_algebra` / `most_common`: `Counter` methods and multiset ops;
// - `defaultdict`: `defaultdict` methods;
// - `deque` / `deque_methods` / `native_iterators`: `deque` storage, iterators, methods;
// - `support`: shared `expect_self` receiver helper.
//
// Reference: <https://docs.python.org/3/library/collections.html>

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{
    Interpreter, IterSrcBuf, MapIter, NativeIterFrame, NativeIteratorClass, class_is_subclass_of,
    coerce_subclass_backing, invoke_class_method, lookup_class_attr, value_from_bigint,
    value_type_name_str,
};
use crate::value::{
    InstanceAttrs, PyBigInt, PyClass, PyDict, PyInstance, PyKey, Value, ValueKind, key_repr,
};
use pyrust_derive::pyrust_module;

#[path = "collections/most_common.rs"]
mod most_common;
#[path = "collections/native_iterators.rs"]
mod native_iterators;

include!("collections/backing.rs");
include!("collections/canonical_identity.rs");
include!("collections/counter.rs");
include!("collections/counter_algebra.rs");
include!("collections/defaultdict.rs");
include!("collections/deque.rs");
include!("collections/deque_methods.rs");
include!("collections/deque_ops.rs");
include!("collections/module_setup.rs");
include!("collections/support.rs");

pyrust_module! {
    #[py_name = "_deque_iterator.__new__"]
    fn deque_iterator_new(args) -> Result<Value> {
        native_deque_iterator_new(_interp, args, NativeIteratorClass::Deque)
    }

    #[py_name = "_deque_iterator.__getattribute__"]
    fn deque_iterator_getattribute(args) -> Result<Value> {
        _interp.call_native_iterator_unbound(
            args,
            NativeIteratorClass::Deque,
            "__getattribute__",
        )
    }

    #[py_name = "_deque_iterator.__iter__"]
    fn deque_iterator_iter(args) -> Result<Value> {
        _interp.call_native_iterator_unbound(args, NativeIteratorClass::Deque, "__iter__")
    }

    #[py_name = "_deque_iterator.__next__"]
    fn deque_iterator_next(args) -> Result<Value> {
        _interp.call_native_iterator_unbound(args, NativeIteratorClass::Deque, "__next__")
    }

    #[py_name = "_deque_iterator.__length_hint__"]
    fn deque_iterator_length_hint(args) -> Result<Value> {
        _interp.call_native_iterator_unbound(
            args,
            NativeIteratorClass::Deque,
            "__length_hint__",
        )
    }

    #[py_name = "_deque_iterator.__reduce__"]
    fn deque_iterator_reduce(args) -> Result<Value> {
        _interp.call_native_iterator_unbound(args, NativeIteratorClass::Deque, "__reduce__")
    }

    #[py_name = "_deque_reverse_iterator.__new__"]
    fn deque_reverse_iterator_new(args) -> Result<Value> {
        native_deque_iterator_new(_interp, args, NativeIteratorClass::DequeReverse)
    }

    #[py_name = "_deque_reverse_iterator.__getattribute__"]
    fn deque_reverse_iterator_getattribute(args) -> Result<Value> {
        _interp.call_native_iterator_unbound(
            args,
            NativeIteratorClass::DequeReverse,
            "__getattribute__",
        )
    }

    #[py_name = "_deque_reverse_iterator.__iter__"]
    fn deque_reverse_iterator_iter(args) -> Result<Value> {
        _interp.call_native_iterator_unbound(args, NativeIteratorClass::DequeReverse, "__iter__")
    }

    #[py_name = "_deque_reverse_iterator.__next__"]
    fn deque_reverse_iterator_next(args) -> Result<Value> {
        _interp.call_native_iterator_unbound(args, NativeIteratorClass::DequeReverse, "__next__")
    }

    #[py_name = "_deque_reverse_iterator.__length_hint__"]
    fn deque_reverse_iterator_length_hint(args) -> Result<Value> {
        _interp.call_native_iterator_unbound(
            args,
            NativeIteratorClass::DequeReverse,
            "__length_hint__",
        )
    }

    #[py_name = "_deque_reverse_iterator.__reduce__"]
    fn deque_reverse_iterator_reduce(args) -> Result<Value> {
        _interp.call_native_iterator_unbound(
            args,
            NativeIteratorClass::DequeReverse,
            "__reduce__",
        )
    }

    fn _counter_repeat_entry(args) -> Result<Value> {
        counter_repeat_entry(_interp, args, FN_NAME)
    }

    fn _deque_readonly_maxlen(_args) -> Result<Value> {
        Err(PyError::named(
            "AttributeError",
            "attribute 'maxlen' of 'collections.deque' objects is not writable".to_string(),
        ))
    }

    fn _counter_fromkeys(args) -> Result<Value> {
        counter_fromkeys(args)
    }

    fn _ordered_dict_move_to_end(args) -> Result<Value> {
        ordered_dict_move_to_end(_interp, args)
    }

    class Counter {
        fn __init__(args) -> Result<Value> {
            counter_init(_interp, args, FN_NAME)
        }

        fn __getitem__(args) -> Result<Value> {
            counter_getitem(_interp, args, FN_NAME)
        }

        fn __setitem__(args) -> Result<Value> {
            dict_subclass_setitem(_interp, args, FN_NAME, "Counter")
        }

        fn __delitem__(args) -> Result<Value> {
            counter_delitem(_interp, args, FN_NAME)
        }

        fn __contains__(args) -> Result<Value> {
            dict_subclass_contains(_interp, args, FN_NAME, "Counter")
        }

        fn __len__(args) -> Result<Value> {
            dict_subclass_len(args, FN_NAME, "Counter")
        }

        fn __iter__(args) -> Result<Value> {
            dict_subclass_iter(args, FN_NAME, "Counter")
        }

        fn __repr__(args) -> Result<Value> {
            counter_repr(_interp, args, FN_NAME)
        }

        fn most_common(args) -> Result<Value> {
            counter_most_common(_interp, args, FN_NAME)
        }

        fn elements(args) -> Result<Value> {
            counter_elements(_interp, args, FN_NAME)
        }

        fn update(args) -> Result<Value> {
            apply_delta::<false>(_interp, args, FN_NAME)
        }

        fn subtract(args) -> Result<Value> {
            apply_delta::<true>(_interp, args, FN_NAME)
        }

        fn total(args) -> Result<Value> {
            counter_total(_interp, args, FN_NAME)
        }

        fn copy(args) -> Result<Value> {
            counter_copy(args, FN_NAME)
        }

        fn get(args) -> Result<Value> {
            dict_subclass_get(_interp, args, FN_NAME, "Counter")
        }

        fn keys(args) -> Result<Value> {
            dict_subclass_view(args, FN_NAME, "Counter", "keys")
        }

        fn values(args) -> Result<Value> {
            dict_subclass_view(args, FN_NAME, "Counter", "values")
        }

        fn items(args) -> Result<Value> {
            dict_subclass_view(args, FN_NAME, "Counter", "items")
        }

        fn __add__(args) -> Result<Value> {
            counter_binop(_interp, args, CounterOp::Add)
        }

        fn __sub__(args) -> Result<Value> {
            counter_binop(_interp, args, CounterOp::Sub)
        }

        fn __and__(args) -> Result<Value> {
            counter_binop(_interp, args, CounterOp::And)
        }

        fn __or__(args) -> Result<Value> {
            counter_binop(_interp, args, CounterOp::Or)
        }

        fn __pos__(args) -> Result<Value> {
            counter_unary(_interp, args, CounterUnaryOp::Positive)
        }

        fn __neg__(args) -> Result<Value> {
            counter_unary(_interp, args, CounterUnaryOp::Negative)
        }

        fn __eq__(args) -> Result<Value> {
            counter_compare(_interp, args, CounterCompareOp::Eq)
        }

        fn __ne__(args) -> Result<Value> {
            counter_compare(_interp, args, CounterCompareOp::Ne)
        }

        fn __le__(args) -> Result<Value> {
            counter_compare(_interp, args, CounterCompareOp::Le)
        }

        fn __lt__(args) -> Result<Value> {
            counter_compare(_interp, args, CounterCompareOp::Lt)
        }

        fn __ge__(args) -> Result<Value> {
            counter_compare(_interp, args, CounterCompareOp::Ge)
        }

        fn __gt__(args) -> Result<Value> {
            counter_compare(_interp, args, CounterCompareOp::Gt)
        }

        fn __iadd__(args) -> Result<Value> {
            counter_inplace_op(_interp, args, CounterOp::Add)
        }

        fn __isub__(args) -> Result<Value> {
            counter_inplace_op(_interp, args, CounterOp::Sub)
        }

        fn __iand__(args) -> Result<Value> {
            counter_inplace_op(_interp, args, CounterOp::And)
        }

        fn __ior__(args) -> Result<Value> {
            counter_inplace_op(_interp, args, CounterOp::Or)
        }
    }

    class defaultdict {
        fn __init__(args) -> Result<Value> {
            defaultdict_init(_interp, args, FN_NAME)
        }

        fn __getitem__(args) -> Result<Value> {
            defaultdict_getitem(_interp, args, FN_NAME)
        }

        fn __missing__(args) -> Result<Value> {
            defaultdict_missing(_interp, args, FN_NAME)
        }

        fn __setitem__(args) -> Result<Value> {
            dict_subclass_setitem(_interp, args, FN_NAME, "defaultdict")
        }

        fn __contains__(args) -> Result<Value> {
            dict_subclass_contains(_interp, args, FN_NAME, "defaultdict")
        }

        fn __len__(args) -> Result<Value> {
            dict_subclass_len(args, FN_NAME, "defaultdict")
        }

        fn __iter__(args) -> Result<Value> {
            dict_subclass_iter(args, FN_NAME, "defaultdict")
        }

        fn __repr__(args) -> Result<Value> {
            defaultdict_repr(args, FN_NAME)
        }

        fn get(args) -> Result<Value> {
            dict_subclass_get(_interp, args, FN_NAME, "defaultdict")
        }

        fn keys(args) -> Result<Value> {
            dict_subclass_view(args, FN_NAME, "defaultdict", "keys")
        }

        fn values(args) -> Result<Value> {
            dict_subclass_view(args, FN_NAME, "defaultdict", "values")
        }

        fn items(args) -> Result<Value> {
            dict_subclass_view(args, FN_NAME, "defaultdict", "items")
        }

        fn copy(args) -> Result<Value> {
            defaultdict_copy(args, FN_NAME)
        }

        fn __copy__(args) -> Result<Value> {
            defaultdict_dunder_copy(args, FN_NAME)
        }

        fn __reduce__(args) -> Result<Value> {
            defaultdict_reduce(_interp, args, FN_NAME)
        }
    }

    class deque {
        fn __init__(args) -> Result<Value> {
            deque_init(_interp, args, FN_NAME)
        }

        fn append(args) -> Result<Value> {
            deque_append_end(args, FN_NAME, false)
        }

        fn appendleft(args) -> Result<Value> {
            deque_append_end(args, FN_NAME, true)
        }

        fn pop(args) -> Result<Value> {
            deque_pop_end(args, FN_NAME, false)
        }

        fn popleft(args) -> Result<Value> {
            deque_pop_end(args, FN_NAME, true)
        }

        fn extend(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            deque_extend_iterable(_interp, &inst, arg, false)
        }

        fn extendleft(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            deque_extend_iterable(_interp, &inst, arg, true)
        }

        fn rotate(args) -> Result<Value> {
            deque_rotate(_interp, args, FN_NAME)
        }

        fn clear(args) -> Result<Value> {
            deque_clear(args, FN_NAME)
        }

        fn copy(args) -> Result<Value> {
            let inst = expect_self_no_args(args, FN_NAME)?;
            deque_shallow_copy_instance(&inst)
        }

        fn __copy__(args) -> Result<Value> {
            let inst = validate_collection_no_arg_method(
                args,
                CanonicalCollectionKind::Deque,
                "__copy__",
                "deque",
                "collections.deque",
            )?;
            deque_shallow_copy_instance(&inst)
        }

        fn __reduce__(args) -> Result<Value> {
            deque_reduce(_interp, args)
        }

        fn count(args) -> Result<Value> {
            deque_count(_interp, args, FN_NAME)
        }

        fn remove(args) -> Result<Value> {
            deque_remove(_interp, args, FN_NAME)
        }

        fn reverse(args) -> Result<Value> {
            let inst = expect_self_no_args(args, FN_NAME)?;
            let items = deque_items_data(&inst)?;
            items.borrow_mut().make_contiguous().reverse();
            Ok(Value::none())
        }

        fn index(args) -> Result<Value> {
            deque_index(_interp, args, FN_NAME)
        }

        fn insert(args) -> Result<Value> {
            deque_insert(_interp, args, FN_NAME)
        }

        fn __len__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            Ok(Value::int(deque_items_data(&inst)?.borrow().len() as i64))
        }

        fn __getitem__(args) -> Result<Value> {
            deque_getitem(_interp, args, FN_NAME)
        }

        fn __setitem__(args) -> Result<Value> {
            deque_setitem(_interp, args, FN_NAME)
        }

        fn __delitem__(args) -> Result<Value> {
            deque_delitem(_interp, args, FN_NAME)
        }

        fn __contains__(args) -> Result<Value> {
            deque_contains(_interp, args, FN_NAME)
        }

        fn __iter__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            deque_iterator(&inst, false)
        }

        fn __reversed__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            deque_iterator(&inst, true)
        }

        fn __repr__(args) -> Result<Value> {
            deque_repr(_interp, args, FN_NAME)
        }

        fn __eq__(args) -> Result<Value> {
            deque_eq(_interp, args, FN_NAME)
        }

        fn __lt__(args) -> Result<Value> {
            deque_ordering_compare(_interp, args, FN_NAME, BinaryOp::Lt)
        }

        fn __le__(args) -> Result<Value> {
            deque_ordering_compare(_interp, args, FN_NAME, BinaryOp::Le)
        }

        fn __gt__(args) -> Result<Value> {
            deque_ordering_compare(_interp, args, FN_NAME, BinaryOp::Gt)
        }

        fn __ge__(args) -> Result<Value> {
            deque_ordering_compare(_interp, args, FN_NAME, BinaryOp::Ge)
        }

        fn __add__(args) -> Result<Value> {
            let _inst = expect_self(args, FN_NAME)?;
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            deque_concat_values(&args[0].value, &args[1].value)
        }

        fn __iadd__(args) -> Result<Value> {
            let (inst, other) = expect_self_one_arg(args, FN_NAME)?;
            deque_extend_iterable(_interp, &inst, other, false)?;
            Ok(args[0].value.clone())
        }

        fn __mul__(args) -> Result<Value> {
            deque_repeat(_interp, args, FN_NAME)
        }

        fn __rmul__(args) -> Result<Value> {
            deque_repeat(_interp, args, FN_NAME)
        }

        fn __imul__(args) -> Result<Value> {
            deque_imul(_interp, args, FN_NAME)
        }
    }
}
