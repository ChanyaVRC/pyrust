# Issue #2119: list / bytearray in-place dunders __iadd__ / __imul__ are
# exposed as bound method-wrappers, mutating the receiver in place and
# returning it (the in-place complement of #1909's __add__ / __mul__).


def show(expr, fn):
    try:
        print(repr(fn()))
    except Exception as ex:
        print(expr, "->", type(ex).__name__, ex)


# --- list __iadd__ / __imul__: mutate in place, return self -----------------
l = [1, 2]
r = l.__iadd__([3])
print(r, r is l)

l2 = [1, 2, 3]
print(l2.__imul__(-1))

l3 = [1, 2]
print(l3.__imul__(2))

print([1, 2].__imul__(True))
print([1, 2].__imul__(False))

# --- bytearray __iadd__ / __imul__ ------------------------------------------
ba = bytearray(b"x")
print(ba.__iadd__(b"y"))

ba2 = bytearray(b"ab")
print(ba2.__imul__(2))

# --- hasattr / dir consistency ----------------------------------------------
print(hasattr([], "__iadd__"), hasattr([], "__imul__"))
print(hasattr(bytearray(), "__iadd__"), hasattr(bytearray(), "__imul__"))
print("__iadd__" in dir(list), "__imul__" in dir(bytearray))
# immutable sequences expose no in-place dunders
print(hasattr((), "__iadd__"), hasattr("", "__imul__"), hasattr(b"", "__iadd__"))

# --- unbound type-level form ------------------------------------------------
print(list.__iadd__([1], [2]))
print(bytearray.__iadd__(bytearray(b"a"), b"b"))

# --- __index__ count for __imul__ -------------------------------------------
class I:
    def __index__(self):
        return 2


print([1].__imul__(I()))

# --- error parity (matches the dunder, not the operator) --------------------
show("[1].__iadd__(5)", lambda: [1].__iadd__(5))
show("[1].__imul__(2.5)", lambda: [1].__imul__(2.5))
show("bytearray(b'a').__iadd__(5)", lambda: bytearray(b"a").__iadd__(5))
show("[1].__iadd__()", lambda: [1].__iadd__())
show("[1].__imul__()", lambda: [1].__imul__())
