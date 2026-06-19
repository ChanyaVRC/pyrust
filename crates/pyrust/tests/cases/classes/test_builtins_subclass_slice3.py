# Issue #2386 (slice 3): builtin-subclass backing extraction routed through the
# centralised builtin_data_backing helper in builtins.rs. Exercise a sampling of
# the touched methods plus the scalar-subclass builtins that share the backing
# extraction path.

class MyInt(int):
    pass


class MyStr(str):
    pass


class MyFloat(float):
    pass


# hash() on scalar subclasses matches the backing primitive's hash.
print(hash(MyInt(42)) == hash(42))
print(hash(MyStr("hi")) == hash("hi"))

# abs() on a float subclass returns the backing magnitude.
print(abs(MyFloat(-1.5)))

# divmod() on int subclasses.
print(divmod(MyInt(7), MyInt(3)))

# round() on a float subclass.
print(round(MyFloat(1.6)))

# Native subscript descriptors on container subclasses (the converted sites).
class MyList(list):
    pass


class MyTuple(tuple):
    pass


class MyBytes(bytes):
    pass


print(list.__getitem__(MyList([10, 20, 30]), 1))
print(list.__getitem__(MyList([10, 20, 30]), slice(0, 2)))
print(tuple.__getitem__(MyTuple((1, 2, 3)), 2))
print(bytes.__getitem__(MyBytes(b"abc"), 0))

# object.__format__ delegation to the backing primitive formatter.
print(format(MyInt(255), "x"))
print(MyInt(255).__format__("x"))

# Wrong-receiver-type descriptor errors are preserved.
try:
    list.__getitem__(5, 0)
except TypeError as e:
    print(e)
try:
    bytes.__getitem__("nope", 0)
except TypeError as e:
    print(e)
