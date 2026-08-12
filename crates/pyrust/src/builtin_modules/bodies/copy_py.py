from collections.abc import Iterable as _Iterable
from types import AsyncGeneratorType as _AsyncGeneratorType
from types import CoroutineType as _CoroutineType


def _reconstruct_shallow(x, rv):
    if isinstance(rv, str):
        return x
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
        return _reconstruct(x, *rv)
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
    return _reconstruct(x, *rv)


def _reconstruct(x, func, args, state=None, listiter=None, dictiter=None):
    y = func(*args)
    if state is not None:
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
        for item in listiter:
            y.append(item)
    if dictiter is not None:
        for key, value in dictiter:
            y[key] = value
    return y
