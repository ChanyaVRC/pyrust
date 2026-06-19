# Guards the dispatch boundary that issue #434's rename enforces:
# the bypass methods (Value::truthy_raw / Value::repr_raw) must NOT be the
# ones reached for user instances. bool()/if and repr() have to dispatch the
# user's __bool__ / __len__ / __repr__.


class AlwaysFalse:
    def __bool__(self):
        return False


class AlwaysTrue:
    def __bool__(self):
        return True


class LenZero:
    def __len__(self):
        return 0


class LenThree:
    def __len__(self):
        return 3


class CustomRepr:
    def __repr__(self):
        return "<custom repr>"


# __bool__ dispatch through truthiness checks.
print(bool(AlwaysFalse()))
print(bool(AlwaysTrue()))
print("if-false" if AlwaysFalse() else "else-false")
print("if-true" if AlwaysTrue() else "else-true")
print(not AlwaysFalse())

# __len__ drives truthiness when __bool__ is absent.
print(bool(LenZero()))
print(bool(LenThree()))
print("empty" if LenZero() else "nonempty")

# __repr__ dispatch.
print(repr(CustomRepr()))
print([CustomRepr()])
print((CustomRepr(),))
print(f"{CustomRepr()!r}")

# Primitives still render structurally (the bypass path is correct here).
print(repr([1, 2, 3]))
print(repr("hi"))
print(repr({"a": 1}))
print(bool([]))
print(bool([0]))
print(bool(""))
print(bool("x"))
