"""Issue #2767: float / complex method_descriptors accept no keyword
arguments and must raise ``TypeError: <type>.<method>() takes no keyword
arguments`` (CPython 3.12 parity) rather than silently dropping the
unexpected keyword.

Covers both call forms:
  * bound       — ``(3.14).conjugate(x=1)``
  * unbound     — ``float.conjugate(3.14, x=1)``
and the builtin-subclass receiver path (``class F(float)``).
"""


def show(label, fn):
    try:
        fn()
        print(label, "-> NO ERROR")
    except TypeError as e:
        print(label, "->", e)


# --- float: bound calls ------------------------------------------------------
show("float.conjugate bound", lambda: (3.14).conjugate(x=1))
show("float.is_integer bound", lambda: (3.14).is_integer(x=1))
show("float.hex bound", lambda: (3.14).hex(x=1))
show("float.as_integer_ratio bound", lambda: (3.14).as_integer_ratio(x=1))

# --- float: unbound (type-level) calls ---------------------------------------
show("float.conjugate unbound", lambda: float.conjugate(3.14, x=1))
show("float.is_integer unbound", lambda: float.is_integer(3.14, x=1))
show("float.hex unbound", lambda: float.hex(3.14, x=1))
show("float.as_integer_ratio unbound",
     lambda: float.as_integer_ratio(3.14, x=1))

# --- complex: bound + unbound ------------------------------------------------
show("complex.conjugate bound", lambda: (1 + 2j).conjugate(x=1))
show("complex.conjugate unbound", lambda: complex.conjugate(1j, k=1))

# --- builtin-subclass instances reject with the base type's wording ----------


class MyFloat(float):
    pass


show("MyFloat().hex", lambda: MyFloat(3.14).hex(x=1))
show("MyFloat().is_integer", lambda: MyFloat(3.14).is_integer(x=1))


# --- happy paths keep working ------------------------------------------------
print("conjugate:", (3.14).conjugate())
print("is_integer:", (3.14).is_integer())
print("hex:", (3.14).hex())
print("as_integer_ratio:", (2.5).as_integer_ratio())
print("complex.conjugate:", (1 + 2j).conjugate())
print("float.conjugate unbound:", float.conjugate(3.14))
print("complex.conjugate unbound:", complex.conjugate(1j))
print("MyFloat().is_integer:", MyFloat(4.0).is_integer())


# --- positional-arg error wording is unchanged (sanity) ----------------------
show("float.conjugate extra positional", lambda: (3.14).conjugate(99))
