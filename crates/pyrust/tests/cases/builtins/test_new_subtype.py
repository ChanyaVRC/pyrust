# Issue #2623: primitive T.__new__(U, ...) must verify issubclass(U, T),
# raising TypeError otherwise (CPython 3.12 tp_new subtype guard).


def show(label, fn):
    try:
        print(label, "->", repr(fn()))
    except TypeError as e:
        print(label, "-> TypeError:", e)


# Non-subtype cls is rejected for every primitive with a dedicated __new__.
show("bool.__new__(int, 5)", lambda: bool.__new__(int, 5))
show("str.__new__(int)", lambda: str.__new__(int))
show("float.__new__(int)", lambda: float.__new__(int))
show("tuple.__new__(list)", lambda: tuple.__new__(list))
show("int.__new__(float)", lambda: int.__new__(float))
show("bytes.__new__(int)", lambda: bytes.__new__(int))
show("frozenset.__new__(int)", lambda: frozenset.__new__(int))

# int.__new__(bool): bool IS a subtype of int, but allocation is unsafe.
show("int.__new__(bool)", lambda: int.__new__(bool))
show("int.__new__(bool, 5)", lambda: int.__new__(bool, 5))

# Identity (cls is the base type) is always allowed.
show("int.__new__(int, 5)", lambda: int.__new__(int, 5))
show("str.__new__(str, 'hi')", lambda: str.__new__(str, "hi"))
show("tuple.__new__(tuple, [1, 2])", lambda: tuple.__new__(tuple, [1, 2]))
show("float.__new__(float, 1.5)", lambda: float.__new__(float, 1.5))


# Genuine subclasses are accepted, and the result is typed as the subclass.
class MyInt(int):
    pass


class MyStr(str):
    pass


class MyTuple(tuple):
    pass


class MyFloat(float):
    pass


m_int = int.__new__(MyInt, 7)
print("int.__new__(MyInt, 7) ->", repr(m_int), type(m_int).__name__)

m_str = str.__new__(MyStr, "hi")
print("str.__new__(MyStr, 'hi') ->", repr(m_str), type(m_str).__name__)

m_tuple = tuple.__new__(MyTuple, [1, 2])
print("tuple.__new__(MyTuple, [1, 2]) ->", repr(m_tuple), type(m_tuple).__name__)

m_float = float.__new__(MyFloat, 1.5)
print("float.__new__(MyFloat, 1.5) ->", repr(m_float), type(m_float).__name__)

# The error message names the actual cls argument, including user subclasses.
show("str.__new__(MyInt, 'x')", lambda: str.__new__(MyInt, "x"))
