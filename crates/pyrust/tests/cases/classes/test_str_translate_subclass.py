# str.translate() must accept builtin-subclass mapping values, matching
# CPython 3.12: the inherited int/str/None backing of a `class MyInt(int)` /
# `class MyStr(str)` instance is used, rather than the value being rejected as
# a non-int/str/None replacement (issue #2651).


class MyInt(int):
    pass


class MyStr(str):
    pass


# int subclass replacement (ordinal -> chr).
print("abc".translate({ord("a"): MyInt(ord("X"))}))  # Xbc

# str subclass replacement (single and multi char).
print("abc".translate({ord("a"): MyStr("X")}))  # Xbc
print("abc".translate({ord("a"): MyStr("XY")}))  # XYbc

# Ordinal-keyed mapping whose key is itself an int subclass.
print("abc".translate({MyInt(ord("a")): MyStr("Z")}))  # Zbc

# None deletes the character (NoneType cannot be subclassed in CPython, so a
# plain None is the only valid "delete" sentinel).
print("abc".translate({ord("a"): None}))  # bc

# Out-of-range int subclass value raises ValueError, not TypeError.
try:
    "a".translate({ord("a"): MyInt(0x110000)})
except ValueError as e:
    print("ValueError:", e)

# A subclass value that is neither int nor str still raises TypeError.
class MyList(list):
    pass


try:
    "a".translate({ord("a"): MyList()})
except TypeError as e:
    print("TypeError:", e)

# Sanity: the plain str.maketrans byte-range table path is unaffected.
print("hello".translate(str.maketrans("el", "ip")))  # hippo
print("hello".translate(str.maketrans("el", "ip", "o")))  # hipp
