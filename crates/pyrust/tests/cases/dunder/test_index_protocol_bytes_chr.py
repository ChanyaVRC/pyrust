# Parity fixture for issue #1908: chr(), bytes()/bytearray() (elements & count
# forms), and bytearray indexing honor the __index__ protocol like CPython 3.12.
#
# Tests cover:
#   - chr() via __index__, bool, range/overflow, non-int return, __int__-only
#     reject, float reject, raising
#   - bytes()/bytearray() element building via __index__ (list/tuple/iterable)
#   - bytes()/bytearray() count form via __index__
#   - bytearray getitem/setitem (single index + slice bounds) via __index__
#   - error classes / messages match CPython exactly


class Idx:
    def __init__(self, v):
        self.v = v

    def __index__(self):
        return self.v


class IntOnly:
    def __int__(self):
        return 65


class BadIdx:
    def __index__(self):
        return "x"


class Raiser:
    def __index__(self):
        raise ValueError("boom")


def t(label, fn):
    try:
        print(label, "->", repr(fn()))
    except BaseException as e:
        print(label, "->", type(e).__name__ + ":", e)


# --- chr() ---
t("chr(Idx(65))", lambda: chr(Idx(65)))
t("chr(True)", lambda: chr(True))
t("chr(Idx(0))", lambda: chr(Idx(0)))
t("chr(65.0)", lambda: chr(65.0))
t("chr(IntOnly())", lambda: chr(IntOnly()))
t("chr(BadIdx())", lambda: chr(BadIdx()))
t("chr(Idx(-1))", lambda: chr(Idx(-1)))
t("chr(Idx(0x110000))", lambda: chr(Idx(0x110000)))
t("chr(Idx(2**100))", lambda: chr(Idx(2**100)))
t("chr(Raiser())", lambda: chr(Raiser()))
t("chr('a')", lambda: chr("a"))

# --- bytes()/bytearray() element form ---
t("bytes([Idx(65), Idx(66)])", lambda: bytes([Idx(65), Idx(66)]))
t("bytes((Idx(1), Idx(2)))", lambda: bytes((Idx(1), Idx(2))))
t("bytearray([Idx(65)])", lambda: bytes(bytearray([Idx(65)])))
t("bytes([True, False])", lambda: bytes([True, False]))
t("bytes([Idx(256)])", lambda: bytes([Idx(256)]))
t("bytes([Idx(-1)])", lambda: bytes([Idx(-1)]))
t("bytes([65.0])", lambda: bytes([65.0]))
t("bytes([IntOnly()])", lambda: bytes([IntOnly()]))
t("bytes([BadIdx()])", lambda: bytes([BadIdx()]))
t("bytes([Raiser()])", lambda: bytes([Raiser()]))
t("bytes(range(3))", lambda: bytes(range(3)))

# --- bytes()/bytearray() count form ---
t("bytes(Idx(3))", lambda: bytes(Idx(3)))
t("bytearray(Idx(2))", lambda: bytes(bytearray(Idx(2))))
t("bytes(Idx(0))", lambda: bytes(Idx(0)))
t("bytes(Idx(-1))", lambda: bytes(Idx(-1)))
t("bytes(IntOnly())", lambda: bytes(IntOnly()))


# __bytes__ takes priority over __index__ count form
class Both:
    def __index__(self):
        return 2

    def __bytes__(self):
        return b"XY"


t("bytes(Both())", lambda: bytes(Both()))
t("bytearray(Both())", lambda: bytes(bytearray(Both())))

# --- bytearray getitem ---
ba = bytearray(b"abc")
t("ba[Idx(0)]", lambda: ba[Idx(0)])
t("ba[Idx(-1)]", lambda: ba[Idx(-1)])
t("ba[Idx(10)]", lambda: ba[Idx(10)])
t("ba[Raiser()]", lambda: ba[Raiser()])
t("ba[1.0]", lambda: ba[1.0])
t("ba['x']", lambda: ba["x"])
t("ba[(1,)]", lambda: ba[(1,)])
t("ba[Idx(1):Idx(3)]", lambda: bytes(bytearray(b"abcde")[Idx(1):Idx(3)]))
t("ba[1:3]", lambda: bytes(bytearray(b"abcde")[1:3]))


# --- bytearray setitem ---
def setidx(i, v):
    b = bytearray(b"abc")
    b[i] = v
    return bytes(b)


t("ba[Idx(0)]=Idx(88)", lambda: setidx(Idx(0), Idx(88)))
t("ba[Idx(0)]=True", lambda: setidx(Idx(0), True))
t("ba[Idx(0)]=Idx(256)", lambda: setidx(Idx(0), Idx(256)))
t("ba[Idx(0)]=Idx(2**100)", lambda: setidx(Idx(0), Idx(2**100)))
t("ba[Idx(0)]=BadIdx()", lambda: setidx(Idx(0), BadIdx()))
t("ba[Idx(0)]=1.5", lambda: setidx(Idx(0), 1.5))
t("ba[Idx(0)]=IntOnly()", lambda: setidx(Idx(0), IntOnly()))
t("ba[Idx(10)]=5", lambda: setidx(Idx(10), 5))
t("ba[1.0]=65", lambda: setidx(1.0, 65))
t("ba['x']=65", lambda: setidx("x", 65))


def setslice():
    b = bytearray(b"abcde")
    b[Idx(1):Idx(3)] = b"XY"
    return bytes(b)


t("ba[Idx(1):Idx(3)]=b'XY'", setslice)
