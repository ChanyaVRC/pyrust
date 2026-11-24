"""Weak reference support for Python (CPython 3.12 parity)."""

from collections.abc import MutableMapping, MutableSet, Mapping
import types

__all__ = [
    "ref",
    "proxy",
    "getweakrefcount",
    "getweakrefs",
    "WeakKeyDictionary",
    "ReferenceType",
    "ProxyType",
    "CallableProxyType",
    "ProxyTypes",
    "WeakValueDictionary",
    "WeakSet",
    "WeakMethod",
    "finalize",
]


class ReferenceType:
    """Weak reference to an object."""

    __slots__ = ("_wr_handle", "_wr_hash")

    def __new__(cls, ob, callback=None, /):
        if cls is ReferenceType and callback is None:
            existing = _find_plain_ref(ob, 1)
            if existing is not None:
                return existing
            dedup = 1
        else:
            dedup = 0
        self = object.__new__(cls)
        handle = _create_handle(self, ob, callback, dedup)
        object.__setattr__(self, "_wr_handle", handle)
        object.__setattr__(self, "_wr_hash", None)
        return self

    def __init__(self, ob, callback=None, /):
        pass

    def __call__(self):
        return _deref(object.__getattribute__(self, "_wr_handle"))

    @property
    def __callback__(self):
        return _get_callback(object.__getattribute__(self, "_wr_handle"))

    def __hash__(self):
        cached = object.__getattribute__(self, "_wr_hash")
        if cached is not None:
            return cached
        obj = _deref(object.__getattribute__(self, "_wr_handle"))
        if obj is None:
            raise TypeError("weak object has gone away")
        h = hash(obj)
        object.__setattr__(self, "_wr_hash", h)
        return h

    def __eq__(self, other):
        if not isinstance(other, ReferenceType):
            return NotImplemented
        if self is other:
            return True
        a = _deref(object.__getattribute__(self, "_wr_handle"))
        b = _deref(object.__getattribute__(other, "_wr_handle"))
        if a is None or b is None:
            return False
        return a == b

    def __ne__(self, other):
        if not isinstance(other, ReferenceType):
            return NotImplemented
        if self is other:
            return False
        a = _deref(object.__getattribute__(self, "_wr_handle"))
        b = _deref(object.__getattribute__(other, "_wr_handle"))
        if a is None or b is None:
            return True
        return a != b

    def __repr__(self):
        alive, tname, tid, rname = _handle_repr_info(
            object.__getattribute__(self, "_wr_handle")
        )
        if not alive:
            return f"<weakref at {hex(id(self))}; dead>"
        if rname is not None:
            return f"<weakref at {hex(id(self))}; to '{tname}' at {hex(tid)} ({rname})>"
        return f"<weakref at {hex(id(self))}; to '{tname}' at {hex(tid)}>"

    __class_getitem__ = classmethod(types.GenericAlias)


ref = ReferenceType


def _proxy_target(p):
    obj = _deref(object.__getattribute__(p, "_wr_handle"))
    if obj is None:
        raise ReferenceError("weakly-referenced object no longer exists")
    return obj


def _unwrap_proxy(x):
    if isinstance(x, ProxyTypes):
        return _proxy_target(x)
    return x


class ProxyType:
    """Weak proxy to a non-callable object."""

    __slots__ = ("_wr_handle",)
    __hash__ = None

    def __new__(cls, *args, **kwargs):
        raise TypeError(f"cannot create '{cls.__module__}.{cls.__name__}' instances")

    def __getattribute__(self, name):
        return getattr(_proxy_target(self), name)

    def __setattr__(self, name, value):
        setattr(_proxy_target(self), name, value)

    def __delattr__(self, name):
        delattr(_proxy_target(self), name)

    def __repr__(self):
        alive, tname, tid, _rname = _handle_repr_info(
            object.__getattribute__(self, "_wr_handle")
        )
        if not alive:
            return f"<weakproxy at {hex(id(self))} to NoneType at {hex(id(None))}>"
        return f"<weakproxy at {hex(id(self))} to {tname} at {hex(tid)}>"

    def __str__(self):
        return str(_proxy_target(self))

    def __bytes__(self):
        return bytes(_proxy_target(self))

    def __format__(self, spec):
        return format(_proxy_target(self), spec)

    def __bool__(self):
        return bool(_proxy_target(self))

    def __len__(self):
        return len(_proxy_target(self))

    def __iter__(self):
        return iter(_proxy_target(self))

    def __next__(self):
        return next(_proxy_target(self))

    def __reversed__(self):
        return reversed(_proxy_target(self))

    def __contains__(self, item):
        return _unwrap_proxy(item) in _proxy_target(self)

    def __getitem__(self, key):
        return _proxy_target(self)[_unwrap_proxy(key)]

    def __setitem__(self, key, value):
        _proxy_target(self)[_unwrap_proxy(key)] = value

    def __delitem__(self, key):
        del _proxy_target(self)[_unwrap_proxy(key)]

    def __eq__(self, other):
        return _proxy_target(self) == _unwrap_proxy(other)

    def __ne__(self, other):
        return _proxy_target(self) != _unwrap_proxy(other)

    def __lt__(self, other):
        return _proxy_target(self) < _unwrap_proxy(other)

    def __le__(self, other):
        return _proxy_target(self) <= _unwrap_proxy(other)

    def __gt__(self, other):
        return _proxy_target(self) > _unwrap_proxy(other)

    def __ge__(self, other):
        return _proxy_target(self) >= _unwrap_proxy(other)

    def __add__(self, other):
        return _proxy_target(self) + _unwrap_proxy(other)

    def __radd__(self, other):
        return _unwrap_proxy(other) + _proxy_target(self)

    def __iadd__(self, other):
        target = _proxy_target(self)
        target += _unwrap_proxy(other)
        return self

    def __sub__(self, other):
        return _proxy_target(self) - _unwrap_proxy(other)

    def __rsub__(self, other):
        return _unwrap_proxy(other) - _proxy_target(self)

    def __isub__(self, other):
        target = _proxy_target(self)
        target -= _unwrap_proxy(other)
        return self

    def __mul__(self, other):
        return _proxy_target(self) * _unwrap_proxy(other)

    def __rmul__(self, other):
        return _unwrap_proxy(other) * _proxy_target(self)

    def __imul__(self, other):
        target = _proxy_target(self)
        target *= _unwrap_proxy(other)
        return self

    def __matmul__(self, other):
        return _proxy_target(self) @ _unwrap_proxy(other)

    def __rmatmul__(self, other):
        return _unwrap_proxy(other) @ _proxy_target(self)

    def __truediv__(self, other):
        return _proxy_target(self) / _unwrap_proxy(other)

    def __rtruediv__(self, other):
        return _unwrap_proxy(other) / _proxy_target(self)

    def __floordiv__(self, other):
        return _proxy_target(self) // _unwrap_proxy(other)

    def __rfloordiv__(self, other):
        return _unwrap_proxy(other) // _proxy_target(self)

    def __mod__(self, other):
        return _proxy_target(self) % _unwrap_proxy(other)

    def __rmod__(self, other):
        return _unwrap_proxy(other) % _proxy_target(self)

    def __divmod__(self, other):
        return divmod(_proxy_target(self), _unwrap_proxy(other))

    def __rdivmod__(self, other):
        return divmod(_unwrap_proxy(other), _proxy_target(self))

    def __pow__(self, other, modulo=None):
        if modulo is None:
            return _proxy_target(self) ** _unwrap_proxy(other)
        return pow(_proxy_target(self), _unwrap_proxy(other), _unwrap_proxy(modulo))

    def __rpow__(self, other):
        return _unwrap_proxy(other) ** _proxy_target(self)

    def __lshift__(self, other):
        return _proxy_target(self) << _unwrap_proxy(other)

    def __rlshift__(self, other):
        return _unwrap_proxy(other) << _proxy_target(self)

    def __rshift__(self, other):
        return _proxy_target(self) >> _unwrap_proxy(other)

    def __rrshift__(self, other):
        return _unwrap_proxy(other) >> _proxy_target(self)

    def __and__(self, other):
        return _proxy_target(self) & _unwrap_proxy(other)

    def __rand__(self, other):
        return _unwrap_proxy(other) & _proxy_target(self)

    def __iand__(self, other):
        target = _proxy_target(self)
        target &= _unwrap_proxy(other)
        return self

    def __xor__(self, other):
        return _proxy_target(self) ^ _unwrap_proxy(other)

    def __rxor__(self, other):
        return _unwrap_proxy(other) ^ _proxy_target(self)

    def __ixor__(self, other):
        target = _proxy_target(self)
        target ^= _unwrap_proxy(other)
        return self

    def __or__(self, other):
        return _proxy_target(self) | _unwrap_proxy(other)

    def __ror__(self, other):
        return _unwrap_proxy(other) | _proxy_target(self)

    def __ior__(self, other):
        target = _proxy_target(self)
        target |= _unwrap_proxy(other)
        return self

    def __neg__(self):
        return -_proxy_target(self)

    def __pos__(self):
        return +_proxy_target(self)

    def __abs__(self):
        return abs(_proxy_target(self))

    def __invert__(self):
        return ~_proxy_target(self)

    def __int__(self):
        return int(_proxy_target(self))

    def __float__(self):
        return float(_proxy_target(self))

    def __complex__(self):
        return complex(_proxy_target(self))

    def __index__(self):
        return _proxy_target(self).__index__()


class CallableProxyType(ProxyType):
    """Weak proxy to a callable object."""

    __slots__ = ()

    def __call__(self, *args, **kwargs):
        return _proxy_target(self)(*args, **kwargs)


ProxyTypes = (ProxyType, CallableProxyType)


def proxy(ob, callback=None, /):
    """Create a proxy object that weakly references `ob`."""
    if callback is None:
        existing = _find_plain_ref(ob, 2)
        if existing is not None:
            return existing
        dedup = 2
    else:
        dedup = 0
    cls = CallableProxyType if callable(ob) else ProxyType
    self = object.__new__(cls)
    handle = _create_handle(self, ob, callback, dedup)
    object.__setattr__(self, "_wr_handle", handle)
    return self


class WeakMethod(ref):
    """A custom `weakref.ref` subclass which simulates a weak reference to
    a bound method, working around the lifetime problem of bound methods.
    """

    __slots__ = ("_func_ref", "_meth_type", "_alive", "__weakref__")

    def __new__(cls, meth, callback=None):
        try:
            obj = meth.__self__
            func = meth.__func__
        except AttributeError:
            raise TypeError(
                f"argument should be a bound method, not {type(meth)}"
            ) from None

        def _cb(arg):
            self = self_wr()
            if self is not None and self._alive:
                self._alive = False
                if callback is not None:
                    callback(self)

        self = ref.__new__(cls, obj, _cb)
        self._func_ref = ref(func, _cb)
        self._meth_type = type(meth)
        self._alive = True
        self_wr = ref(self)
        return self

    def __call__(self):
        obj = super().__call__()
        func = self._func_ref()
        if obj is None or func is None:
            return None
        return _bind_method(func, obj)

    def __eq__(self, other):
        if isinstance(other, WeakMethod):
            if not self._alive or not other._alive:
                return self is other
            return ref.__eq__(self, other) and self._func_ref == other._func_ref
        return NotImplemented

    def __ne__(self, other):
        if isinstance(other, WeakMethod):
            if not self._alive or not other._alive:
                return self is not other
            return ref.__ne__(self, other) or self._func_ref != other._func_ref
        return NotImplemented

    __hash__ = ref.__hash__


class KeyedRef(ref):
    """Specialized reference that includes a key corresponding to the value."""

    __slots__ = ("key",)

    def __new__(cls, ob, callback, key):
        self = ref.__new__(cls, ob, callback)
        self.key = key
        return self

    def __init__(self, ob, callback, key):
        super().__init__(ob, callback)


class _IterationGuard:
    def __init__(self, weakcontainer):
        self.weakcontainer = ref(weakcontainer)

    def __enter__(self):
        w = self.weakcontainer()
        if w is not None:
            w._iterating.add(self)
        return self

    def __exit__(self, e, t, b):
        w = self.weakcontainer()
        if w is not None:
            s = w._iterating
            s.discard(self)
            if not s:
                w._commit_removals()


class WeakValueDictionary(MutableMapping):
    """Mapping class that references values weakly."""

    def __init__(self, other=(), /, **kw):
        def remove(wr, selfref=ref(self)):
            self = selfref()
            if self is not None:
                if self._iterating:
                    self._pending_removals.append(wr.key)
                else:
                    try:
                        if self.data.get(wr.key) is wr:
                            del self.data[wr.key]
                    except KeyError:
                        pass

        self._remove = remove
        self._pending_removals = []
        self._iterating = set()
        self.data = {}
        self.update(other, **kw)

    def _commit_removals(self):
        pop = self._pending_removals.pop
        d = self.data
        while True:
            try:
                key = pop()
            except IndexError:
                return
            try:
                wr = d[key]
            except KeyError:
                continue
            if wr() is None:
                del d[key]

    def __getitem__(self, key):
        if self._pending_removals:
            self._commit_removals()
        o = self.data[key]()
        if o is None:
            raise KeyError(key)
        return o

    def __delitem__(self, key):
        if self._pending_removals:
            self._commit_removals()
        del self.data[key]

    def __len__(self):
        if self._pending_removals:
            self._commit_removals()
        return sum(wr() is not None for wr in self.data.values())

    def __contains__(self, key):
        if self._pending_removals:
            self._commit_removals()
        try:
            o = self.data[key]()
        except KeyError:
            return False
        return o is not None

    def __repr__(self):
        return f"<{self.__class__.__name__} at {id(self):#x}>"

    def __setitem__(self, key, value):
        if self._pending_removals:
            self._commit_removals()
        self.data[key] = KeyedRef(value, self._remove, key)

    def copy(self):
        if self._pending_removals:
            self._commit_removals()
        new = WeakValueDictionary()
        with _IterationGuard(self):
            for key, wr in self.data.items():
                o = wr()
                if o is not None:
                    new[key] = o
        return new

    __copy__ = copy

    def __deepcopy__(self, memo):
        import copy as _copy

        if self._pending_removals:
            self._commit_removals()
        new = self.__class__()
        with _IterationGuard(self):
            for key, wr in self.data.items():
                o = wr()
                if o is not None:
                    new[_copy.deepcopy(key, memo)] = o
        return new

    def get(self, key, default=None):
        if self._pending_removals:
            self._commit_removals()
        try:
            wr = self.data[key]
        except KeyError:
            return default
        o = wr()
        if o is None:
            return default
        return o

    def items(self):
        if self._pending_removals:
            self._commit_removals()
        with _IterationGuard(self):
            for k, wr in self.data.items():
                v = wr()
                if v is not None:
                    yield k, v

    def keys(self):
        if self._pending_removals:
            self._commit_removals()
        with _IterationGuard(self):
            for k, wr in self.data.items():
                if wr() is not None:
                    yield k

    __iter__ = keys

    def itervaluerefs(self):
        if self._pending_removals:
            self._commit_removals()
        with _IterationGuard(self):
            yield from self.data.values()

    def values(self):
        if self._pending_removals:
            self._commit_removals()
        with _IterationGuard(self):
            for wr in self.data.values():
                obj = wr()
                if obj is not None:
                    yield obj

    def popitem(self):
        if self._pending_removals:
            self._commit_removals()
        while True:
            key, wr = self.data.popitem()
            o = wr()
            if o is not None:
                return key, o

    def pop(self, key, *args):
        if self._pending_removals:
            self._commit_removals()
        try:
            o = self.data.pop(key)()
        except KeyError:
            o = None
        if o is None:
            if args:
                return args[0]
            raise KeyError(key)
        return o

    def setdefault(self, key, default=None):
        try:
            o = self.data[key]()
        except KeyError:
            o = None
        if o is None:
            if self._pending_removals:
                self._commit_removals()
            self.data[key] = KeyedRef(default, self._remove, key)
            return default
        return o

    def update(self, other=None, /, **kwargs):
        if self._pending_removals:
            self._commit_removals()
        d = self.data
        if other is not None:
            if not hasattr(other, "items"):
                other = dict(other)
            for key, o in other.items():
                d[key] = KeyedRef(o, self._remove, key)
        for key, o in kwargs.items():
            d[key] = KeyedRef(o, self._remove, key)

    def valuerefs(self):
        if self._pending_removals:
            self._commit_removals()
        return list(self.data.values())

    def __ior__(self, other):
        self.update(other)
        return self

    def __or__(self, other):
        if isinstance(other, Mapping):
            c = self.copy()
            c.update(other)
            return c
        return NotImplemented

    def __ror__(self, other):
        if isinstance(other, Mapping):
            c = self.__class__()
            c.update(other)
            c.update(self)
            return c
        return NotImplemented


class WeakKeyDictionary(MutableMapping):
    """Mapping class that references keys weakly."""

    def __init__(self, dict=None):
        self.data = {}

        def remove(k, selfref=ref(self)):
            self = selfref()
            if self is not None:
                if self._iterating:
                    self._pending_removals.append(k)
                else:
                    try:
                        del self.data[k]
                    except KeyError:
                        pass

        self._remove = remove
        self._pending_removals = []
        self._iterating = set()
        self._dirty_len = False
        if dict is not None:
            self.update(dict)

    def _commit_removals(self):
        pop = self._pending_removals.pop
        d = self.data
        while True:
            try:
                key = pop()
            except IndexError:
                return
            try:
                del d[key]
            except KeyError:
                pass

    def _scrub_removals(self):
        d = self.data
        self._pending_removals = [k for k in self._pending_removals if k in d]
        self._dirty_len = False

    def __delitem__(self, key):
        self._dirty_len = True
        del self.data[ref(key)]

    def __getitem__(self, key):
        return self.data[ref(key)]

    def __len__(self):
        if self._dirty_len and self._pending_removals:
            self._scrub_removals()
        return len(self.data) - len(self._pending_removals)

    def __repr__(self):
        return f"<{self.__class__.__name__} at {id(self):#x}>"

    def __setitem__(self, key, value):
        self.data[ref(key, self._remove)] = value

    def copy(self):
        new = WeakKeyDictionary()
        with _IterationGuard(self):
            for key, value in self.data.items():
                o = key()
                if o is not None:
                    new[o] = value
        return new

    __copy__ = copy

    def __deepcopy__(self, memo):
        import copy as _copy

        new = self.__class__()
        with _IterationGuard(self):
            for key, value in self.data.items():
                o = key()
                if o is not None:
                    new[o] = _copy.deepcopy(value, memo)
        return new

    def get(self, key, default=None):
        return self.data.get(ref(key), default)

    def __contains__(self, key):
        try:
            wr = ref(key)
        except TypeError:
            return False
        return wr in self.data

    def items(self):
        with _IterationGuard(self):
            for wr, value in self.data.items():
                key = wr()
                if key is not None:
                    yield key, value

    def keys(self):
        with _IterationGuard(self):
            for wr in self.data:
                obj = wr()
                if obj is not None:
                    yield obj

    __iter__ = keys

    def values(self):
        with _IterationGuard(self):
            for wr, value in self.data.items():
                if wr() is not None:
                    yield value

    def keyrefs(self):
        return list(self.data)

    def popitem(self):
        self._dirty_len = True
        while True:
            key, value = self.data.popitem()
            o = key()
            if o is not None:
                return o, value

    def pop(self, key, *args):
        self._dirty_len = True
        return self.data.pop(ref(key), *args)

    def setdefault(self, key, default=None):
        return self.data.setdefault(ref(key, self._remove), default)

    def update(self, dict=None, /, **kwargs):
        d = self.data
        if dict is not None:
            if not hasattr(dict, "items"):
                dict = type({})(dict)
            for key, value in dict.items():
                d[ref(key, self._remove)] = value
        if len(kwargs):
            self.update(kwargs)

    def __ior__(self, other):
        self.update(other)
        return self

    def __or__(self, other):
        if isinstance(other, Mapping):
            c = self.copy()
            c.update(other)
            return c
        return NotImplemented

    def __ror__(self, other):
        if isinstance(other, Mapping):
            c = self.__class__()
            c.update(other)
            c.update(self)
            return c
        return NotImplemented


class WeakSet:
    """Set class that keeps weak references to its elements."""

    def __init__(self, data=None):
        self.data = set()

        def _remove(item, selfref=ref(self)):
            self = selfref()
            if self is not None:
                if self._iterating:
                    self._pending_removals.append(item)
                else:
                    self.data.discard(item)

        self._remove = _remove
        self._pending_removals = []
        self._iterating = set()
        if data is not None:
            self.update(data)

    def _commit_removals(self):
        pop = self._pending_removals.pop
        discard = self.data.discard
        while True:
            try:
                item = pop()
            except IndexError:
                return
            discard(item)

    def __iter__(self):
        with _IterationGuard(self):
            for itemref in self.data:
                item = itemref()
                if item is not None:
                    yield item

    def __len__(self):
        return len(self.data) - len(self._pending_removals)

    def __contains__(self, item):
        try:
            wr = ref(item)
        except TypeError:
            return False
        return wr in self.data

    def __reduce__(self):
        return (self.__class__, (list(self),), getattr(self, "__dict__", None))

    def add(self, item):
        if self._pending_removals:
            self._commit_removals()
        self.data.add(ref(item, self._remove))

    def clear(self):
        if self._pending_removals:
            self._commit_removals()
        self.data.clear()

    def copy(self):
        return self.__class__(self)

    def pop(self):
        if self._pending_removals:
            self._commit_removals()
        while True:
            try:
                itemref = self.data.pop()
            except KeyError:
                raise KeyError("pop from empty WeakSet") from None
            item = itemref()
            if item is not None:
                return item

    def remove(self, item):
        if self._pending_removals:
            self._commit_removals()
        self.data.remove(ref(item))

    def discard(self, item):
        if self._pending_removals:
            self._commit_removals()
        self.data.discard(ref(item))

    def update(self, other):
        if self._pending_removals:
            self._commit_removals()
        for element in other:
            self.add(element)

    def __ior__(self, other):
        self.update(other)
        return self

    def difference(self, other):
        newset = self.copy()
        newset.difference_update(other)
        return newset

    __sub__ = difference

    def difference_update(self, other):
        self.__isub__(other)

    def __isub__(self, other):
        if self._pending_removals:
            self._commit_removals()
        if self is other:
            self.data.clear()
        else:
            self.data.difference_update(ref(item) for item in other)
        return self

    def intersection(self, other):
        return self.__class__(item for item in other if item in self)

    __and__ = intersection

    def intersection_update(self, other):
        self.__iand__(other)

    def __iand__(self, other):
        if self._pending_removals:
            self._commit_removals()
        self.data.intersection_update(ref(item) for item in other)
        return self

    def issubset(self, other):
        return self.data.issubset(ref(item) for item in other)

    __le__ = issubset

    def __lt__(self, other):
        return self.data < set(map(ref, other))

    def issuperset(self, other):
        return self.data.issuperset(ref(item) for item in other)

    __ge__ = issuperset

    def __gt__(self, other):
        return self.data > set(map(ref, other))

    def __eq__(self, other):
        if not isinstance(other, self.__class__):
            return NotImplemented
        return self.data == set(map(ref, other))

    def symmetric_difference(self, other):
        newset = self.copy()
        newset.symmetric_difference_update(other)
        return newset

    __xor__ = symmetric_difference

    def symmetric_difference_update(self, other):
        self.__ixor__(other)

    def __ixor__(self, other):
        if self._pending_removals:
            self._commit_removals()
        if self is other:
            self.data.clear()
        else:
            self.data.symmetric_difference_update(
                ref(item, self._remove) for item in other
            )
        return self

    def union(self, other):
        return self.__class__(e for s in (self, other) for e in s)

    __or__ = union

    def isdisjoint(self, other):
        return len(self.intersection(other)) == 0

    def __repr__(self):
        return repr(self.data)

    __class_getitem__ = classmethod(types.GenericAlias)


class finalize:
    """Class for finalization of weakrefable objects."""

    __slots__ = ()
    _registry = {}
    _shutdown = False
    _index_iter = 0
    _dirty = False

    class _Info:
        __slots__ = ("weakref", "func", "args", "kwargs", "atexit", "index")

    def __init__(self, obj, func, /, *args, **kwargs):
        info = self._Info()
        info.weakref = ref(obj, self)
        info.func = func
        info.args = args
        info.kwargs = kwargs or None
        info.atexit = True
        finalize._index_iter += 1
        info.index = finalize._index_iter
        self._registry[self] = info
        finalize._dirty = True

    def __call__(self, _=None):
        """If alive then mark as dead and return func(*args, **kwargs);
        otherwise return None."""
        info = self._registry.pop(self, None)
        if info and not self._shutdown:
            return info.func(*info.args, **(info.kwargs or {}))
        return None

    def detach(self):
        """If alive then mark as dead and return (obj, func, args, kwargs);
        otherwise return None."""
        info = self._registry.get(self)
        obj = info and info.weakref()
        if obj is not None and self._registry.pop(self, None):
            return (obj, info.func, info.args, info.kwargs or {})
        return None

    def peek(self):
        """If alive then return (obj, func, args, kwargs);
        otherwise return None."""
        info = self._registry.get(self)
        obj = info and info.weakref()
        if obj is not None:
            return (obj, info.func, info.args, info.kwargs or {})
        return None

    @property
    def alive(self):
        """Whether finalizer is alive."""
        return self in self._registry

    @property
    def atexit(self):
        """Whether finalizer should be called at exit."""
        info = self._registry.get(self)
        return bool(info) and info.atexit

    @atexit.setter
    def atexit(self, value):
        info = self._registry.get(self)
        if info:
            info.atexit = bool(value)

    def __repr__(self):
        info = self._registry.get(self)
        obj = info and info.weakref()
        if obj is None:
            return f"<{type(self).__name__} object at {id(self):#x}; dead>"
        return (
            f"<{type(self).__name__} object at {id(self):#x}; "
            f"for '{type(obj).__name__}' at {id(obj):#x}>"
        )

    @classmethod
    def _select_for_exit(cls):
        L = [(f, i) for (f, i) in cls._registry.items() if i.atexit]
        L.sort(key=lambda item: item[1].index)
        return [f for (f, i) in L]

    @classmethod
    def _exitfunc(cls):
        try:
            while True:
                pending = cls._select_for_exit()
                if not pending:
                    break
                f = pending.pop()
                try:
                    f()
                except Exception:
                    pass
        finally:
            cls._shutdown = True
