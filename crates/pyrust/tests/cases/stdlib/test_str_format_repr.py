# str.format !r conversion dispatches __repr__ on user-defined objects.
# Issue #783.


class MyObj:
    def __repr__(self):
        return "custom_repr"


class InheritedRepr(MyObj):
    """Subclass with no __repr__ of its own — inherits via MRO."""
    pass


class NoCustomRepr:
    """No __repr__ defined — falls back to built-in Value::repr()."""
    pass


# Basic dunder dispatch
print('{x!r}'.format(x=MyObj()))

# MRO-inherited __repr__
print('{x!r}'.format(x=InheritedRepr()))

# Object without custom __repr__: must not raise; output format is
# <module.ClassName object at 0x...> which varies by address, so we
# only assert the prefix is correct.
fallback = '{x!r}'.format(x=NoCustomRepr())
print(fallback.startswith('<'))
print('NoCustomRepr' in fallback)
print(fallback.endswith('>'))

# Built-in types: no behaviour change
print('{x!r}'.format(x=42))
print('{x!r}'.format(x="hello"))
print('{x!r}'.format(x=[1, 2, 3]))
print('{x!r}'.format(x=None))
print('{x!r}'.format(x=True))

# Positional !r
print('{0!r}'.format(MyObj()))

# __repr__ that returns non-string raises TypeError
class BadRepr:
    def __repr__(self):
        return 42


try:
    '{x!r}'.format(x=BadRepr())
    print("no error")
except TypeError:
    print("TypeError on non-string repr")

# repr() builtin is unaffected (control path)
print(repr(MyObj()))
