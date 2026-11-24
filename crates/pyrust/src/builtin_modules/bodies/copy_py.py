from collections.abc import Iterable as _Iterable
from types import AsyncGeneratorType as _AsyncGeneratorType
from types import CoroutineType as _CoroutineType


def _validate_rv_iterable(rv):
    rv_type = type(rv)
    type_name = type.__getattribute__(rv_type, "__name__")
    try:
        mro = type.__getattribute__(rv_type, "__mro__")
    except AttributeError:
        mro = ()
    if not mro:
        if isinstance(rv, (_CoroutineType, _AsyncGeneratorType)) or not isinstance(
            rv, _Iterable
        ):
            raise TypeError(f"Value after * must be an iterable, not {type_name}")
        return
    for cls in mro:
        namespace = type.__getattribute__(cls, "__dict__")
        if "__iter__" in namespace:
            iter_slot = namespace["__iter__"]
            if iter_slot is None or (
                isinstance(iter_slot, staticmethod) and iter_slot.__func__ is None
            ):
                raise TypeError(f"'{type_name}' object is not iterable")
            break
    else:
        for cls in mro:
            namespace = type.__getattribute__(cls, "__dict__")
            if "__getitem__" in namespace:
                break
        else:
            raise TypeError(f"Value after * must be an iterable, not {type_name}")


_active_deepcopy = None


def _reconstruct_shallow(x, rv):
    if isinstance(rv, str):
        return x
    _validate_rv_iterable(rv)
    return _reconstruct(x, None, *rv)


def _reconstruct_deep(x, memo, rv, deepcopy_fn):
    global _active_deepcopy
    if isinstance(rv, str):
        return x
    _validate_rv_iterable(rv)
    prev = _active_deepcopy
    _active_deepcopy = deepcopy_fn
    try:
        return _reconstruct(x, memo, *rv)
    finally:
        _active_deepcopy = prev


def _reconstruct(
    x, memo, func, args, state=None, listiter=None, dictiter=None, *, deepcopy=None
):
    if deepcopy is None:
        deepcopy = _active_deepcopy
    deep = memo is not None
    if deep and args:
        args = (deepcopy(arg, memo) for arg in args)
    y = func(*args)
    if deep:
        memo[id(x)] = y

    if state is not None:
        if deep:
            state = deepcopy(state, memo)
        if hasattr(y, "__setstate__"):
            y.__setstate__(state)
        else:
            if isinstance(state, tuple) and len(state) == 2:
                state, slotstate = state
            else:
                slotstate = None
            if state is not None:
                y.__dict__.update(state)
            if slotstate is not None:
                for key, value in slotstate.items():
                    setattr(y, key, value)

    if listiter is not None:
        if deep:
            for item in listiter:
                item = deepcopy(item, memo)
                y.append(item)
        else:
            for item in listiter:
                y.append(item)
    if dictiter is not None:
        if deep:
            for key, value in dictiter:
                key = deepcopy(key, memo)
                value = deepcopy(value, memo)
                y[key] = value
        else:
            for key, value in dictiter:
                y[key] = value
    return y
