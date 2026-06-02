# Issue #1934: min / max / sorted accept int/float/str/bytes subclass
# elements (ordering inherited from the base type).
class F(float):
    pass


class I(int):
    pass


class S(str):
    pass


class B(bytes):
    pass


print(min(F(1.0), F(2.0)))
print(max(F(1.0), F(2.0)))
print(sorted([F(2.0), F(1.0)]))
print(min([I(1), I(2)]))
print(max([I(3), I(1), I(2)]))
print(min(S("a"), S("b")))
print(sorted([S("c"), S("a"), S("b")]))
print(min(B(b"a"), B(b"b")))

# Mixed subclass / base operands.
print(min(F(1.0), 2))
print(max(I(1), 5, 2.0))
print(sorted([I(3), 1, 2]))

# A user comparison override still wins over the inherited backing.
class ILt(int):
    def __lt__(self, other):
        # Reverse ordering.
        return int(self) > int(other)


print(sorted([ILt(1), ILt(3), ILt(2)]))
print(min(ILt(1), ILt(3)))
