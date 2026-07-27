# str/bytes/bytearray.splitlines(keepends) accepts any Python object and
# converts it through __bool__ / __len__.  Exercise bound, unbound type-method,
# and builtin-subclass backing dispatch, including validation-before-conversion.

events = []


class BoolFlag:
    def __init__(self, label, answer):
        self.label = label
        self.answer = answer

    def __bool__(self):
        events.append("bool:" + self.label)
        return self.answer


class LenFlag:
    def __init__(self, label, length):
        self.label = label
        self.length = length

    def __len__(self):
        events.append("len:" + self.label)
        return self.length


class Boom:
    def __init__(self, label):
        self.label = label

    def __bool__(self):
        events.append("boom:" + self.label)
        raise RuntimeError("splitlines truth boom:" + self.label)


class Text(str):
    pass


class Data(bytes):
    pass


class Buffer(bytearray):
    pass


def show(label, thunk):
    try:
        print(label, thunk())
    except Exception as exc:
        print(label, "ERROR", type(exc).__name__, str(exc))


def validation_error(label, thunk):
    before = len(events)
    try:
        thunk()
    except Exception as exc:
        print(label, type(exc).__name__, len(events) == before)
    else:
        print(label, "missing")


# Exact receivers, bound calls.  Include positional and keyword binding.
show(
    "str-bound-pos",
    lambda: "a\nb".splitlines(BoolFlag("str-bound-pos", False)),
)
show(
    "bytes-bound-kw",
    lambda: b"a\nb".splitlines(keepends=LenFlag("bytes-bound-kw", 1)),
)
show(
    "bytearray-bound-pos",
    lambda: bytearray(b"a\nb").splitlines(
        BoolFlag("bytearray-bound-pos", True)
    ),
)

# Unbound type-method calls.
show(
    "str-unbound-kw",
    lambda: str.splitlines("a\nb", keepends=LenFlag("str-unbound-kw", 0)),
)
show(
    "bytes-unbound-pos",
    lambda: bytes.splitlines(
        b"a\nb",
        BoolFlag("bytes-unbound-pos", True),
    ),
)
show(
    "bytearray-unbound-kw",
    lambda: bytearray.splitlines(
        bytearray(b"a\nb"),
        keepends=LenFlag("bytearray-unbound-kw", 1),
    ),
)

# Builtin-subclass instances must dispatch through their primitive backing.
show(
    "str-subclass",
    lambda: Text("a\nb").splitlines(BoolFlag("str-subclass", True)),
)
show(
    "bytes-subclass",
    lambda: Data(b"a\nb").splitlines(LenFlag("bytes-subclass", 0)),
)
show(
    "bytearray-subclass",
    lambda: Buffer(b"a\nb").splitlines(BoolFlag("bytearray-subclass", False)),
)

# Truth-conversion exceptions propagate for each concrete type, distributed
# across the three dispatch shapes.
show("str-boom", lambda: "a\nb".splitlines(Boom("str")))
show("bytes-boom", lambda: bytes.splitlines(b"a\nb", Boom("bytes")))
show(
    "bytearray-boom",
    lambda: Buffer(b"a\nb").splitlines(keepends=Boom("bytearray")),
)

# Signature errors win before keepends is truth-tested.  Cover duplicate
# positional+keyword, excess positional args, and an invalid keyword across
# the three type families and dispatch shapes.
validation_error(
    "str-duplicate",
    lambda: "a\nb".splitlines(Boom("bad-str-pos"), keepends=Boom("bad-str-kw")),
)
validation_error(
    "bytes-arity",
    lambda: bytes.splitlines(
        b"a\nb",
        Boom("bad-bytes-1"),
        Boom("bad-bytes-2"),
    ),
)
validation_error(
    "bytearray-invalid-kw",
    lambda: Buffer(b"a\nb").splitlines(wrong=Boom("bad-bytearray-kw")),
)

print("events", events)
