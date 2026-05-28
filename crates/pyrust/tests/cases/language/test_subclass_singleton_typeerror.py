# Issue #1453: subclassing NoneType, ellipsis, NotImplementedType, or bool
# must raise TypeError: type 'X' is not an acceptable base type.
# CPython 3.12 rejects these via the Py_TPFLAGS_BASETYPE flag mechanism.

for t in [type(None), type(...), type(NotImplemented), bool]:
    try:
        class Foo(t): pass
        print(f"FAIL: subclass of {t.__name__} should have raised")
    except TypeError as e:
        print(f"OK: {e}")

# Confirm that the error is TypeError and not something else
try:
    class Bad(type(None)): pass
    print("FAIL: no error raised")
except TypeError:
    print("OK: TypeError raised for NoneType")
except Exception as e:
    print(f"FAIL: wrong exception type {type(e).__name__}: {e}")

# Confirm subclassing with multiple bases also catches the bad one
try:
    class Multi(int, type(None)): pass
    print("FAIL: no error raised for multi-base with NoneType")
except TypeError as e:
    print(f"OK: multi-base caught: {e}")

# Confirm that the dynamic type() constructor also rejects non-subclassable bases
for t in [type(None), type(...), type(NotImplemented), bool]:
    try:
        x = type("Foo", (t,), {})
        print(f"FAIL: type() with {t.__name__} base should have raised")
    except TypeError as e:
        print(f"OK type(): {e}")
