# Pins the builtin conversion dunders `typing.Supports*` checks rely on
# (issue #3326): `float.__int__`/`__float__` slot wrappers and the
# `complex.__complex__` / `bytes.__bytes__` method descriptors (CPython 3.11+),
# including bound/unbound presentation, receiver guards, and arity errors.


def show(label, thunk):
    try:
        result = thunk()
    except Exception as exc:  # noqa: BLE001 - the error text is the point
        print(label, "-> raises", type(exc).__name__, exc)
        return
    if callable(result) and not isinstance(result, type):
        # Bound methods embed an address; keep only the stable prefix.
        print(label, "->", repr(result).split(" at ")[0])
    else:
        print(label, "->", repr(result))


for expr in [
    "float.__int__", "float.__float__", "complex.__complex__", "bytes.__bytes__",
    "type(float.__int__).__name__", "type(float.__float__).__name__",
    "type(complex.__complex__).__name__", "type(bytes.__bytes__).__name__",
    "type((1.5).__int__).__name__", "type((1j).__complex__).__name__",
    "type(b''.__bytes__).__name__",
    "(1.5).__int__", "(1j).__complex__", "b''.__bytes__",
    "(1.5).__int__()", "(-2.7).__int__()", "(1e30).__int__()", "(1.5).__float__()",
    "float.__int__(2.5)", "float.__float__(3.0)",
    "(1j).__complex__()", "complex.__complex__(2+3j)",
    "b'xy'.__bytes__()", "bytes.__bytes__(b'q')",
    "float('nan').__int__()", "float('inf').__int__()",
    "float.__int__('x')", "complex.__complex__(1)", "bytes.__bytes__('x')",
    "bytes.__bytes__(bytearray(b'x'))",
    "(1.5).__int__(1)", "(1j).__complex__(1)", "b''.__bytes__(1)",
    "'__int__' in dir(float)", "'__float__' in dir(float)",
    "'__complex__' in dir(complex)", "'__bytes__' in dir(bytes)",
    "'__int__' in float.__dict__", "'__float__' in float.__dict__",
    "'__complex__' in complex.__dict__", "'__bytes__' in bytes.__dict__",
    "hasattr(bool, '__float__')", "hasattr(int, '__complex__')",
    "hasattr(float, '__index__')", "hasattr(bytearray, '__bytes__')",
    "hasattr(str, '__bytes__')", "hasattr(complex, '__float__')",
    "int(2.9)", "float(1.25)", "complex(1j)", "bytes(b'ok')",
]:
    show(expr, lambda: eval(expr))


class F(float):
    pass


class C(complex):
    pass


class B(bytes):
    pass


print(type(F(1.5).__float__()).__name__, F(2.5).__int__())
print(type(C(1j).__complex__()).__name__, C(1j).__complex__())
print(type(B(b"x").__bytes__()).__name__, B(b"x").__bytes__())
