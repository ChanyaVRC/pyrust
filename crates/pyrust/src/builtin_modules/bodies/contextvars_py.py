# `contextvars` — Context Variables (PEP 567, issue #3184).
#
# Pure-Python implementation of CPython 3.12's `contextvars` module:
# `ContextVar`, `Token`, `Context` (Mapping), and `copy_context()`.

from types import GenericAlias as _GenericAlias

__all__ = ("Context", "ContextVar", "Token", "copy_context")

_NO_DEFAULT = object()


class _MissingType:
    def __repr__(self):
        return "<Token.MISSING>"


_MISSING = _MissingType()


class Token:
    """Opaque object returned by `ContextVar.set()`, used to restore a
    previous value via `ContextVar.reset(token)`."""

    MISSING = _MISSING

    __class_getitem__ = classmethod(_GenericAlias)

    def __new__(cls, *args, **kwargs):
        raise RuntimeError("Tokens can only be created by ContextVars")

    @classmethod
    def _create(cls, context, var, old_value):
        tok = object.__new__(cls)
        tok._context = context
        tok._var = var
        tok._old_value = old_value
        tok._used = False
        return tok

    @property
    def var(self):
        return self._var

    @property
    def old_value(self):
        return self._old_value

    def __repr__(self):
        used = " used" if self._used else ""
        return f"<Token{used} var={self._var!r} at {id(self):#x}>"


class ContextVar:
    """A context variable whose value is scoped to the current `Context`."""

    __class_getitem__ = classmethod(_GenericAlias)

    def __init__(self, name, /, *, default=_NO_DEFAULT):
        if not isinstance(name, str):
            raise TypeError("context variable name must be a str")
        self._name = name
        self._default = default

    @property
    def name(self):
        return self._name

    def get(self, default=_NO_DEFAULT, /):
        data = _current_context._data
        if self in data:
            return data[self]
        if default is not _NO_DEFAULT:
            return default
        if self._default is not _NO_DEFAULT:
            return self._default
        raise LookupError(self)

    def set(self, value, /):
        ctx = _current_context
        old_value = ctx._data.get(self, _MISSING)
        ctx._data[self] = value
        return Token._create(ctx, self, old_value)

    def reset(self, token, /):
        if not isinstance(token, Token):
            raise TypeError(f"expected an instance of Token, got {token!r}")
        if token._used:
            raise RuntimeError(f"{token!r} has already been used once")
        if token._var is not self:
            raise ValueError(f"{token!r} was created by a different ContextVar")
        if token._context is not _current_context:
            raise ValueError(f"{token!r} was created in a different Context")
        token._used = True
        if token._old_value is _MISSING:
            _current_context._data.pop(self, None)
        else:
            _current_context._data[self] = token._old_value

    def __repr__(self):
        if self._default is _NO_DEFAULT:
            return f"<ContextVar name={self._name!r} at {id(self):#x}>"
        return (
            f"<ContextVar name={self._name!r} "
            f"default={self._default!r} at {id(self):#x}>"
        )


class _ContextView:
    """Iterator view returned by `Context.keys()`, `values()`, `items()`."""

    def __init__(self, items):
        self._items = items
        self._index = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self._index >= len(self._items):
            raise StopIteration
        item = self._items[self._index]
        self._index += 1
        return item

    def __len__(self):
        return len(self._items)


def _check_contextvar_key(key):
    if not isinstance(key, ContextVar):
        raise TypeError(f"a ContextVar key was expected, got {key!r}")


class Context:
    """A mapping of `ContextVar`s to their values."""

    __hash__ = None

    def __init__(self, *args, **kwargs):
        if args or kwargs:
            raise TypeError("Context() does not accept any arguments")
        self._data = {}
        self._entered = False

    def __getitem__(self, var):
        _check_contextvar_key(var)
        return self._data[var]

    def __contains__(self, var):
        _check_contextvar_key(var)
        return var in self._data

    def get(self, var, default=None):
        _check_contextvar_key(var)
        return self._data.get(var, default)

    def __iter__(self):
        return iter(self._data)

    def __len__(self):
        return len(self._data)

    def keys(self):
        return _ContextView(list(self._data.keys()))

    def values(self):
        return _ContextView(list(self._data.values()))

    def items(self):
        return _ContextView(list(self._data.items()))

    def __eq__(self, other):
        if isinstance(other, Context):
            return self._data == other._data
        return NotImplemented

    def copy(self):
        new = Context()
        new._data = self._data.copy()
        return new

    def run(self, callable, /, *args, **kwargs):
        global _current_context
        if self._entered:
            raise RuntimeError(
                f"cannot enter context: {self!r} is already entered"
            )
        prev = _current_context
        self._entered = True
        _current_context = self
        try:
            return callable(*args, **kwargs)
        finally:
            _current_context = prev
            self._entered = False


_current_context = Context()


def copy_context():
    """Return a shallow copy of the current `Context`."""
    return _current_context.copy()
