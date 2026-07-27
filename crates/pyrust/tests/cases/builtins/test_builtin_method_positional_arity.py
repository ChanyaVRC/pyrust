def classify(label, operation):
    try:
        operation()
    except Exception as error:
        print(label, type(error).__name__)
    else:
        print(label, "accepted")


# Mutable sequence methods must not silently discard extra operands.
classify("list.append", lambda: [].append(1, 2))
classify("list.clear", lambda: [].clear(1))
classify("list.copy", lambda: [].copy(1))
classify("list.extend", lambda: [].extend([], []))
classify("list.insert", lambda: [].insert(0, 1, 2))
classify("list.pop", lambda: [1].pop(0, 1))
classify("list.remove", lambda: [1].remove(1, 2))
classify("list.reverse", lambda: [].reverse(1))
classify("list.sort", lambda: [].sort(1))

# View/fixed-arity mapping methods have exact positional signatures.
classify("dict.keys", lambda: {}.keys(1))
classify("dict.values", lambda: {}.values(1))
classify("dict.items", lambda: {}.items(1))
classify("dict.popitem", lambda: {1: 2}.popitem(1))
classify("dict.clear", lambda: {}.clear(1))
classify("dict.copy", lambda: {}.copy(1))
classify("dict.get", lambda: {}.get("key", None, "extra"))
classify("dict.pop", lambda: {"key": 1}.pop("key", None, "extra"))
classify("dict.setdefault", lambda: {}.setdefault("key", None, "extra"))
classify("dict.update", lambda: {}.update({}, {}))

# Variadic union/update families remain variadic; only fixed-arity set
# operations appear here.
classify("set.add", lambda: set().add(1, 2))
classify("set.remove", lambda: {1}.remove(1, 2))
classify("set.discard", lambda: {1}.discard(1, 2))
classify("set.pop", lambda: {1}.pop(1))
classify("set.clear", lambda: {1}.clear(1))
classify("set.copy", lambda: {1}.copy(1))
classify(
    "set.symmetric_difference",
    lambda: {1}.symmetric_difference({2}, {3}),
)
classify(
    "set.symmetric_difference_update",
    lambda: {1}.symmetric_difference_update({2}, {3}),
)
classify("set.issubset", lambda: {1}.issubset({1}, {2}))
classify("set.issuperset", lambda: {1}.issuperset({1}, {2}))
classify("set.isdisjoint", lambda: {1}.isdisjoint({2}, {3}))
classify(
    "frozenset.symmetric_difference",
    lambda: frozenset({1}).symmetric_difference({2}, {3}),
)
classify(
    "frozenset.issubset",
    lambda: frozenset({1}).issubset({1}, {2}),
)
classify("frozenset.copy", lambda: frozenset({1}).copy(1))
classify(
    "frozenset.issuperset",
    lambda: frozenset({1}).issuperset({1}, {2}),
)
classify(
    "frozenset.isdisjoint",
    lambda: frozenset({1}).isdisjoint({2}, {3}),
)

# These families are intentionally variadic.
classify("set.union variadic", lambda: {1}.union({2}, {3}, {4}))
classify("set.update variadic", lambda: set().update({1}, {2}, {3}))
classify(
    "frozenset.union variadic",
    lambda: frozenset({1}).union({2}, {3}, {4}),
)

# Shared list/tuple sequence helpers must enforce their public signatures.
classify("list.count", lambda: [].count(1, 2))
classify("list.index", lambda: [1].index(1, 0, 1, 2))
classify("tuple.count", lambda: ().count(1, 2))
classify("tuple.index", lambda: (1,).index(1, 0, 1, 2))
classify("tuple.__getnewargs__", lambda: (1,).__getnewargs__(1))

# No-argument text/bytes operations currently share large algorithm routers;
# the router still owns rejecting operands before selecting an algorithm.
classify("str.upper", lambda: "a".upper(1))
classify("str.isalpha", lambda: "a".isalpha(1))
classify("str.title", lambda: "a".title(1))
classify("str.split", lambda: "a".split(None, -1, "extra"))
classify("str.strip", lambda: "a".strip(None, "extra"))
classify("str.find", lambda: "a".find("a", 0, 1, 2))
classify("str.partition", lambda: "a".partition("a", "extra"))
classify("str.join", lambda: ",".join([], []))
classify("str.center", lambda: "a".center(1, " ", "extra"))
classify("str.replace", lambda: "a".replace("a", "b", 1, "extra"))
classify("str.encode", lambda: "a".encode("utf-8", "strict", "extra"))
classify("str.splitlines", lambda: "a".splitlines(False, "extra"))
classify("str.translate", lambda: "a".translate({}, {}))
classify("str.format_map", lambda: "{x}".format_map({"x": 1}, {}))
classify("str.maketrans", lambda: str.maketrans("a", "b", "c", "extra"))
classify("bytes.upper", lambda: b"a".upper(1))
classify("bytes.isalpha", lambda: b"a".isalpha(1))
classify("bytes.title", lambda: b"a".title(1))
classify("bytes.strip", lambda: b"a".strip(None, b"extra"))
classify("bytes.find", lambda: b"a".find(b"a", 0, 1, 2))
classify("bytes.partition", lambda: b"a".partition(b"a", b"extra"))
classify("bytes.join", lambda: b",".join([], []))
classify("bytes.center", lambda: b"a".center(1, b" ", b"extra"))
classify("bytes.replace", lambda: b"a".replace(b"a", b"b", 1, b"extra"))
classify("bytes.decode", lambda: b"a".decode("utf-8", "strict", "extra"))
classify("bytes.splitlines", lambda: b"a".splitlines(False, "extra"))
classify("bytes.translate", lambda: b"a".translate(None, b"", b"extra"))
classify("bytes.fromhex", lambda: bytes.fromhex("00", "extra"))
classify("bytes.maketrans", lambda: bytes.maketrans(b"a", b"b", b"extra"))
classify("bytearray.upper", lambda: bytearray(b"a").upper(1))
classify("bytearray.isalpha", lambda: bytearray(b"a").isalpha(1))
classify("bytearray.title", lambda: bytearray(b"a").title(1))
classify(
    "bytearray.strip",
    lambda: bytearray(b"a").strip(None, bytearray(b"extra")),
)
classify("bytearray.append", lambda: bytearray().append(1, 2))
classify("bytearray.extend", lambda: bytearray().extend([], []))
classify("bytearray.insert", lambda: bytearray().insert(0, 1, 2))
classify("bytearray.pop", lambda: bytearray(b"a").pop(0, 1))
classify("bytearray.remove", lambda: bytearray(b"a").remove(97, 98))
classify("bytearray.reverse", lambda: bytearray().reverse(1))
classify("bytearray.clear", lambda: bytearray().clear(1))
classify("bytearray.copy", lambda: bytearray().copy(1))
classify("bytearray.join", lambda: bytearray(b",").join([], []))
classify("bytearray.translate", lambda: bytearray(b"a").translate(None, b"", b"extra"))
classify("bytearray.fromhex", lambda: bytearray.fromhex("00", "extra"))
classify(
    "bytearray.maketrans",
    lambda: bytearray.maketrans(b"a", b"b", b"extra"),
)
classify("slice.indices", lambda: slice(None).indices(1, 2))


# Excess operands must win before argument conversion, hashing, or iteration.
side_effects = []


class Probe:
    def __index__(self):
        side_effects.append("index")
        return 0

    def __hash__(self):
        side_effects.append("hash")
        return 1

    def __iter__(self):
        side_effects.append("iter")
        return iter(())


classify("list.insert side-effect guard", lambda: [].insert(Probe(), 1, 2))
classify("list.extend side-effect guard", lambda: [].extend(Probe(), []))
classify("dict.get side-effect guard", lambda: {}.get(Probe(), None, "extra"))
classify("set.add side-effect guard", lambda: set().add(Probe(), "extra"))
classify(
    "bytearray.extend side-effect guard",
    lambda: bytearray().extend(Probe(), []),
)
print("extra-arg side effects", side_effects)


# Keep established diagnostics stable while moving validation ahead of bodies.
def show_message(label, operation):
    try:
        operation()
    except Exception as error:
        print(label, str(error))


show_message("message list.sort", lambda: [].sort(1))
show_message("message dict.update", lambda: {}.update({}, {}))
show_message("message str.partition", lambda: "a".partition("a", "extra"))
show_message("message str.expandtabs", lambda: "a".expandtabs(1, 2))
show_message("message str.center", lambda: "a".center(1, " ", "extra"))
show_message("message bytes.translate", lambda: b"a".translate(None, b"", b"extra"))
show_message("message slice.indices", lambda: slice(None).indices(1, 2))
