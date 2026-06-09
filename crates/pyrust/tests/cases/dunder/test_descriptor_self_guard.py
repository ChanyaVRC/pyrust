# Issue #2266: the descriptor receiver-validation guards for type-qualified
# builtin methods (unbound descriptor calls such as `str.upper()` /
# `str.__len__()`) are routed through the shared `descriptor_needs_arg!` /
# `descriptor_requires!` macros in pyrust-core.  CPython 3.12 picks the wording
# by descriptor kind, so the macros expose two variants each:
#
#   method_descriptor  (str.upper, list.append, int.conjugate, object.__sizeof__):
#     no self    -> "unbound method <type>.<m>() needs an argument"
#     wrong type -> "descriptor '<m>' for '<type>' objects doesn't apply to a '<X>' object"
#
#   slot wrapper  (str.__len__, list.__add__, object.__repr__, comparison ops):
#     no self    -> "descriptor '<m>' of '<type>' object needs an argument"
#     wrong type -> "descriptor '<m>' requires a '<type>' object but received a '<X>'"
#
# Every assertion below was verified byte-for-byte against python3.12.


def show(label, fn):
    try:
        fn()
    except TypeError as e:
        print(label, str(e))


# === method_descriptors: "unbound method <type>.<m>() needs an argument" =====
show("str.upper", lambda: str.upper())
show("str.split", lambda: str.split())
show("bytes.hex", lambda: bytes.hex())
show("list.append", lambda: list.append())
show("list.pop", lambda: list.pop())
show("dict.get", lambda: dict.get())
show("set.add", lambda: set.add())
show("int.bit_length", lambda: int.bit_length())
show("int.conjugate", lambda: int.conjugate())
show("int.to_bytes", lambda: int.to_bytes())
show("float.is_integer", lambda: float.is_integer())
# method_descriptor dunders
show("object.__sizeof__", lambda: object.__sizeof__())
show("object.__dir__", lambda: object.__dir__())
show("object.__reduce__", lambda: object.__reduce__())
show("object.__reduce_ex__", lambda: object.__reduce_ex__())
show("object.__format__", lambda: object.__format__())
show("float.__trunc__", lambda: float.__trunc__())
show("float.__floor__", lambda: float.__floor__())
show("float.__ceil__", lambda: float.__ceil__())
show("str.format", lambda: str.format())
# __getitem__ is per-type: dict/list are method_descriptors (str/tuple/bytes
# are slot wrappers, asserted below).
show("dict.__getitem__", lambda: dict.__getitem__())
show("list.__getitem__", lambda: list.__getitem__())
# __contains__ is also per-type: dict/set/frozenset are method_descriptors
# (str/list/tuple/bytes are slot wrappers, asserted below).
show("dict.__contains__", lambda: dict.__contains__())
show("set.__contains__", lambda: set.__contains__())
show("frozenset.__contains__", lambda: frozenset.__contains__())


# --- method_descriptor wrong receiver: "... doesn't apply to a '<X>' object" -
show("str.upper(int)", lambda: str.upper(5))
show("str.split(int)", lambda: str.split(5))
show("bytes.hex(int)", lambda: bytes.hex(5))
show("list.append(int)", lambda: list.append(5))
show("dict.get(int)", lambda: dict.get(5))
show("set.add(int)", lambda: set.add(5))
show("int.bit_length(str)", lambda: int.bit_length("x"))
show("float.is_integer(str)", lambda: float.is_integer("x"))
show("str.format(int)", lambda: str.format(5))
show("dict.__getitem__(int)", lambda: dict.__getitem__(5, 0))
show("list.__getitem__(int)", lambda: list.__getitem__(5, 0))
show("dict.__contains__(int)", lambda: dict.__contains__(5, "k"))
show("set.__contains__(int)", lambda: set.__contains__(5, "k"))


# === slot wrappers: "descriptor '<m>' of '<type>' object needs an argument" ==
show("str.__len__", lambda: str.__len__())
show("list.__len__", lambda: list.__len__())
show("tuple.__len__", lambda: tuple.__len__())
show("dict.__len__", lambda: dict.__len__())
show("set.__len__", lambda: set.__len__())
show("bytes.__len__", lambda: bytes.__len__())
show("frozenset.__len__", lambda: frozenset.__len__())
show("str.__add__", lambda: str.__add__())
show("str.__contains__", lambda: str.__contains__())
show("list.__contains__", lambda: list.__contains__())
show("tuple.__contains__", lambda: tuple.__contains__())
show("bytes.__contains__", lambda: bytes.__contains__())
show("list.__add__", lambda: list.__add__())
show("int.__add__", lambda: int.__add__())
show("int.__lt__", lambda: int.__lt__())
show("str.__getitem__", lambda: str.__getitem__())
show("object.__repr__", lambda: object.__repr__())
show("object.__str__", lambda: object.__str__())
show("object.__eq__", lambda: object.__eq__())
show("object.__hash__", lambda: object.__hash__())
show("object.__init__", lambda: object.__init__())
show("object.__getattribute__", lambda: object.__getattribute__())
show("object.__setattr__", lambda: object.__setattr__())
show("object.__delattr__", lambda: object.__delattr__())


# --- slot wrapper wrong receiver: "... requires a '<type>' object but received" -
show("str.__len__(int)", lambda: str.__len__(5))
show("list.__len__(int)", lambda: list.__len__(5))
show("tuple.__len__(int)", lambda: tuple.__len__(5))
show("dict.__len__(int)", lambda: dict.__len__(5))
show("bytes.__len__(int)", lambda: bytes.__len__(5))
show("str.__len__(list)", lambda: str.__len__([1, 2, 3]))
show("dict.__len__(str)", lambda: dict.__len__("ab"))
show("str.__contains__(int)", lambda: str.__contains__(5, "x"))
show("str.__getitem__(int)", lambda: str.__getitem__(5, 0))
