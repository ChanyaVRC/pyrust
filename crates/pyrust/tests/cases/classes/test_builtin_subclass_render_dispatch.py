# Issue #2386 (slice 2): the builtin-subclass backing helper now also drives the
# render/format/subscript/printf consumer paths. This fixture pins their CPython
# parity: str()/repr()/format() rendering of subclass instances, slice subscript
# delegation, item assignment through the backing, printf int/float coercion,
# str.translate mapping to a subclass value, and `in` membership.


class MyList(list):
    pass


class MyDict(dict):
    pass


class MySet(set):
    pass


class MyInt(int):
    pass


class MyFloat(float):
    pass


class MyStr(str):
    pass


class MyBytes(bytes):
    pass


# str() / repr() delegate to the backing when no user __str__/__repr__.
print(str(MyList([1, 2, 3])))
print(repr(MyList([1, 2, 3])))
print(str(MyDict({"a": 1})))
print(repr(MyDict({"a": 1})))
print(repr(MySet({1})))
print(str(MyInt(42)), repr(MyInt(42)))
print(str(MyStr("hi")), repr(MyStr("hi")))
print(str(MyBytes(b"ab")), repr(MyBytes(b"ab")))

# format(): empty spec routes through str(self); scalar specs apply to backing.
print(format(MyInt(255), "x"))
print(format(MyFloat(1.5), ".2f"))
print(format(MyList([1, 2]), ""))

# slice subscript delegates to the backing sequence.
print(MyList([0, 1, 2, 3, 4])[1:4])
print(MyList([0, 1, 2, 3, 4])[::2])

# item assignment on a dict/list subclass mutates the backing.
d = MyDict()
d["k"] = 9
print(d["k"], type(d).__name__)
li = MyList([0, 0, 0])
li[1] = 7
print(li, type(li).__name__)

# membership on a subclass with no user __contains__.
print(2 in MyList([1, 2, 3]))
print("a" in MyDict({"a": 1}))

# printf int/float coercion from subclass backing.
print("%d/%x" % (MyInt(10), MyInt(255)))
print("%.1f" % MyFloat(2.5))
print("%d" % MyInt(7))

# bytes printf %b / %s with a bytes subclass argument.
print(b"%b-%s" % (MyBytes(b"xy"), MyBytes(b"zz")))
