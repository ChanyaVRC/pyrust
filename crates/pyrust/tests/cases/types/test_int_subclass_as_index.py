# Issue #1929: int/bool subclasses accepted as integers in index / length /
# repetition / range / chr / hex / bin / oct / to_bytes contexts.
class I(int):
    pass


i = I(2)

# Subscript: list / str / tuple / bytes
print([10, 20, 30][i])
print("abcde"[i])
print((1, 2, 3, 4)[i])
print(b"abc"[i])

# Negative index from a subclass.
print([1, 2, 3, 4, 5][I(-1)])

# range / repetition
print(list(range(i)))
print([0] * i)
print("ab" * i)

# Number-base formatters honour the inherited int.
print(hex(i), bin(i), oct(i))

# chr from an int subclass.
print(chr(I(65)))
print(chr(I(0x10FFFF)) == "\U0010ffff")

# to_bytes length from an int subclass (positional and keyword).
print((255).to_bytes(I(2), "big"))
print((255).to_bytes(length=I(2), byteorder="big"))

# An int subclass with a custom __index__ still indexes by its *value*
# (CPython treats the object as the int it already is, not via __index__).
class IIdx(int):
    def __index__(self):
        return 7


print([0, 1, 2, 3, 4, 5, 6, 7, 8, 9][IIdx(3)])
print(list(range(IIdx(3))))
print("ab" * IIdx(3))
print(IIdx(3).__index__())

# A plain object with __index__ (no int backing) keeps working (#1908).
class Idx:
    def __index__(self):
        return 2


print([10, 20, 30][Idx()])
print(list(range(Idx())))
print(hex(Idx()))
print((255).to_bytes(Idx(), "big"))
