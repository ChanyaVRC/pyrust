# Issue #3006: dict-view intersection follows CPython's
# `_PyDictView_Intersect` scan order.  The scanned side's element object must
# survive when equal keys have distinguishable representations.


class SetSubclass(set):
    pass


class IteratingSet(set):
    def __iter__(self):
        return iter([True])


class IteratingFrozenSet(frozenset):
    def __iter__(self):
        return iter([True])


class RaisingSet(set):
    def __iter__(self):
        raise RuntimeError("set subclass iter")


class RaisingFrozenSet(frozenset):
    def __iter__(self):
        raise RuntimeError("frozenset subclass iter")


class Key:
    def __init__(self, group, tag):
        self.group = group
        self.tag = tag

    def __hash__(self):
        return self.group

    def __eq__(self, other):
        events.append((self.tag, other.tag))
        return isinstance(other, Key) and self.group == other.group

    def __repr__(self):
        return "Key(%s)" % self.tag


def who(result):
    element = next(iter(result))
    if isinstance(element, Key):
        return element.tag
    if isinstance(element, tuple):
        return "(%s,%s)" % (type(element[0]).__name__, type(element[1]).__name__)
    return type(element).__name__


def fingerprint(result):
    return sorted(type(element).__name__ + ":" + repr(element) for element in result)


keys = {1: "v"}.keys()
larger_keys = {1: "v", 9: "v"}.keys()
float_keys = {1.0: "v"}.keys()
larger_float_keys = {1.0: "v", 9.0: "v"}.keys()
items = {1: 2}.items()
float_items = {1.0: 2.0}.items()


# The issue's eleven representative operand shapes.  Only the first case was
# already correct on the base revision.
print("A", who(keys & {1.0}))
print("B", who({1.0} & keys))
print("C", who(keys & frozenset({1.0})))
print("D", who(keys & [1.0]))
print("E", who(keys & float_keys))
print("F", who(float_keys & keys))
print("G", who(larger_keys & {1.0}))
print("H", who({1} & {True: 0}.keys()))
print("I", who({(1.0, 2.0)} & items))
print("J", who(items & [(1.0, 2.0)]))
print("K", who(items & float_items))


# An exact set is special only when it is at least as large as the view.  A
# frozenset, set subclass, or arbitrary iterable always takes the manual path.
print("exact larger right", who(keys & {1.0, 9.0}))
print("exact larger left", who({1.0, 9.0} & keys))
print("exact smaller right", who(larger_keys & {1.0}))
print("exact smaller left", who({1.0} & larger_keys))
print("frozen left", who(frozenset({1.0}) & keys))
print("subclass right", who(keys & SetSubclass({1.0})))
print("subclass left", who(SetSubclass({1.0}) & keys))
print("list left", who([1.0] & keys))


# Set and frozenset subclasses use the manual iterable path too.  Their
# overridden iterator therefore controls both the retained object and any
# exception, rather than the operation reading their backing table directly.
print("iter set right", who(keys & IteratingSet({1.0})))
print("iter set left", who(IteratingSet({1.0}) & keys))
print("iter frozen right", who(keys & IteratingFrozenSet({1.0})))
print("iter frozen left", who(IteratingFrozenSet({1.0}) & keys))

for label, left, right in (
    ("raising set right", keys, RaisingSet({1.0})),
    ("raising set left", RaisingSet({1.0}), keys),
    ("raising frozen right", keys, RaisingFrozenSet({1.0})),
    ("raising frozen left", RaisingFrozenSet({1.0}), keys),
):
    try:
        left & right
        print(label, "no error")
    except RuntimeError as error:
        print(label, str(error))


# View/view intersections scan the smaller view and let the right side win a
# size tie.
print("views tie", who(keys & float_keys), who(float_keys & keys))
print(
    "views different size",
    who(keys & larger_float_keys),
    who(larger_float_keys & keys),
    who(larger_keys & float_keys),
    who(float_keys & larger_keys),
)


# Items views share the same scan rule, including exact-set and iterable
# boundaries.
print("items exact right", who(items & {(1.0, 2.0)}))
print("items exact left", who({(1.0, 2.0)} & items))
print("items list right", who(items & [(1.0, 2.0)]))
print("items list left", who([(1.0, 2.0)] & items))
print("items views tie", who(items & float_items), who(float_items & items))


# The other dict-view set operators already use source-order semantics and are
# outside this fix.
pair_keys = {1: "v", 2: "v"}.keys()
print("or view left", fingerprint(keys | {1.0, 9.0}))
print("or view right", fingerprint({1.0, 9.0} | keys))
print("sub view left", fingerprint(pair_keys - {1.0, 3.0}))
print("sub view right", fingerprint({1.0, 3.0} - pair_keys))
print("xor view left", fingerprint(pair_keys ^ {1.0, 3.0}))
print("xor view right", fingerprint({1.0, 3.0} ^ pair_keys))

in_place = keys
in_place &= {1.0}
print("in place", type(in_place).__name__, who(in_place))


# Exercise the user-`__eq__` branch.  The event pairs show which stored probe
# owns equality, while the result tag shows which scanned object survives.
events = []
view_key = Key(7, "view")
set_key = Key(7, "set")
view = {view_key: None}.keys()
other_set = {set_key}
events.clear()
result = view & other_set
print("user view exact", who(result), events)

events.clear()
result = other_set & view
print("user exact view", who(result), events)

list_key = Key(7, "list")
events.clear()
result = view & [list_key]
print("user view list", who(result), events)

right_view_key = Key(7, "right-view")
right_view = {right_view_key: None}.keys()
events.clear()
result = view & right_view
print("user views tie", who(result), events)

larger_view_key = Key(7, "larger-view")
larger_view = {larger_view_key: None, Key(11, "extra"): None}.keys()
small_set_key = Key(7, "small-set")
small_set = {small_set_key}
events.clear()
result = larger_view & small_set
print("user exact smaller", who(result), events)


class RaisingKey:
    def __init__(self, tag):
        self.tag = tag

    def __hash__(self):
        return 23

    def __eq__(self, other):
        raise RuntimeError(self.tag)


# The manual iterable path scans the iterable and probes the view, so the
# view's stored key owns the failing equality call.
try:
    {RaisingKey("view error"): None}.keys() & [RaisingKey("list error")]
    print("raising no error")
except RuntimeError as error:
    print("raising", str(error))
