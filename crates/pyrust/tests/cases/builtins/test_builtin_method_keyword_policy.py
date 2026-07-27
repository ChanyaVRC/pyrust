"""Keyword ownership, rejection precedence, and side-effect ordering.

Each concrete builtin type owns whether a method accepts keywords. Rejection
must happen before positional arity checks or method-body argument conversion.
"""

events = []


class Probe:
    def __index__(self):
        events.append("index")
        return 1

    def __hash__(self):
        events.append("hash")
        return 1

    def __iter__(self):
        events.append("iter")
        return iter([1])

    def __bool__(self):
        events.append("bool")
        return True

    def __eq__(self, other):
        events.append("eq")
        return False


def show(label, fn):
    events.clear()
    try:
        value = fn()
        print(label, "OK", repr(value), events)
    except Exception as exc:
        print(label, type(exc).__name__, str(exc), events)


# Rejection wins before protocol conversion for all nine audited type owners.
show("list reject", lambda: [].insert(Probe(), 0, bad=1))
show("tuple reject", lambda: (0,).index(Probe(), bad=1))
show("dict reject", lambda: {}.get(Probe(), bad=1))
show("set reject", lambda: set().add(Probe(), bad=1))
show("frozenset reject", lambda: frozenset().union(Probe(), bad=1))
show("str reject", lambda: "x".center(Probe(), bad=1))
show("bytes reject", lambda: b"x".center(Probe(), bad=1))
show("bytearray reject", lambda: bytearray().extend(Probe(), bad=1))
show("slice reject", lambda: slice(None).indices(Probe(), bad=1))

# Keyword rejection also precedes missing/excess positional diagnostics.
show("list missing+kw", lambda: [].append(bad=1))
show("tuple missing+kw", lambda: ().count(bad=1))
show("dict missing+kw", lambda: {}.get(bad=1))
show("set missing+kw", lambda: set().add(bad=1))
show("frozenset missing+kw", lambda: frozenset().isdisjoint(bad=1))
show("str missing+kw", lambda: "x".center(bad=1))
show("bytes missing+kw", lambda: b"x".center(bad=1))
show("bytearray missing+kw", lambda: bytearray().append(bad=1))
show("slice missing+kw", lambda: slice(None).indices(bad=1))
show("list excess+kw", lambda: [].append(1, 2, bad=1))

# list.sort has two typed keyword-only slots and validates them before truth().
show("sort bad before bool", lambda: [2, 1].sort(reverse=Probe(), bad=1))
show(
    "sort keyword overflow",
    lambda: [2, 1].sort(key=None, reverse=False, bad=1),
)

# Static/class helpers are positional-only and must not discard named args.
show("str.maketrans kw", lambda: str.maketrans({"a": "b"}, bad=1))
show("bytes.fromhex kw", lambda: bytes.fromhex("61", bad=1))
show("bytes.maketrans kw", lambda: bytes.maketrans(b"a", b"b", bad=1))
show("bytearray.fromhex kw", lambda: bytearray.fromhex("61", bad=1))
show(
    "bytearray.maketrans kw",
    lambda: bytearray.maketrans(b"a", b"b", bad=1),
)


class MyStr(str):
    pass


# A str subclass uses the same rejection and accepted-keyword binder.
show("str subclass reject", lambda: MyStr("x").center(3, bad=1))
show("str subclass split", lambda: MyStr("a b c").split(maxsplit=1))
show("str subclass expandtabs", lambda: MyStr("a\tb").expandtabs(tabsize=4))


class MyBytes(bytes):
    pass


# Keyword slots accepting bytes-like values mirror their positional forms.
show(
    "bytes split bytearray kw",
    lambda: b"a,b".split(sep=bytearray(b",")),
)
show(
    "bytes translate subclass kw",
    lambda: b"abc".translate(None, delete=MyBytes(b"b")),
)
show(
    "bytearray split subclass kw",
    lambda: bytearray(b"a,b").split(sep=MyBytes(b",")),
)
show(
    "bytearray translate delete",
    lambda: bytearray(b"abc").translate(None, delete=bytearray(b"b")),
)

# Unknown translate keywords are rejected before inspecting the table.
show(
    "bytearray translate bad before table",
    lambda: bytearray(b"abc").translate(Probe(), bad=b"b"),
)
