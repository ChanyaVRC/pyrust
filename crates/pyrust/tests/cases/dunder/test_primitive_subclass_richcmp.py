# Issue #2847: an explicit primitive base rich-comparison slot must compare
# the primitive backing carried by subclass instances.  It must not return
# NotImplemented merely because the visible class name is the subclass, and
# it must not redispatch a subclass override while executing the base slot.


def call(label, owner, method, left, right):
    try:
        print(label, getattr(owner, method)(left, right))
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


def call_args(label, owner, method, args, kwargs):
    try:
        print(label, getattr(owner, method)(*args, **kwargs))
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


def call_bound(label, receiver, method, other):
    try:
        print(label, getattr(receiver, method)(other))
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


class I(int):
    pass


class F(float):
    pass


class S(str):
    pass


class B(bytes):
    pass


class T(tuple):
    pass


cases = [
    ("int", int, I(1), I(2), 1, 2, 2.0),
    ("float", float, F(1.0), F(2.0), 1.0, 2.0, "2"),
    ("str", str, S("a"), S("b"), "a", "b", b"b"),
    ("bytes", bytes, B(b"a"), B(b"b"), b"a", b"b", "b"),
    ("tuple", tuple, T((1,)), T((2,)), (1,), (2,), [2]),
]


# All six slots with two subclass operands, plus equality's true case.
for name, owner, sub_left, sub_right, base_left, base_right, wrong in cases:
    for method in ("__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__"):
        call(name + ".sub-sub." + method, owner, method, sub_left, sub_right)
    call(name + ".sub-sub.same-eq", owner, "__eq__", sub_left, sub_left)
    call_bound(name + ".bound.lt", sub_left, "__lt__", sub_right)
    call_bound(name + ".bound.eq", sub_left, "__eq__", sub_left)

    # Either operand may be the subclass.  The explicitly selected base slot
    # still owns the operation in both directions.
    call(name + ".sub-base.lt", owner, "__lt__", sub_left, base_right)
    call(name + ".base-sub.lt", owner, "__lt__", base_left, sub_right)
    call(name + ".base-sub.eq", owner, "__eq__", base_left, sub_left)

    # Incompatible operands remain the base slot's NotImplemented result.
    call(name + ".wrong.eq", owner, "__eq__", sub_left, wrong)
    call(name + ".wrong.lt", owner, "__lt__", sub_left, wrong)

    # The ordinary inherited operator path was already correct and must stay
    # correct while the explicit base-slot path is repaired.
    print(name + ".inherited", sub_left < sub_right, sub_left == sub_left)


# Numeric-tower direction remains owned by the selected base slot.
call("int.bool.eq", int, "__eq__", I(1), True)
call("int.float-sub.lt", int, "__lt__", I(1), F(2.0))
call("float.int-sub.lt", float, "__lt__", F(1.0), I(2))


# Routing float richcmp through the shared wrapper keeps its positional-only
# receiver, receiver-first error precedence, arity, and keyword contracts.
call_args("float.call.none", float, "__lt__", [], {})
call_args("float.call.self-only", float, "__lt__", [F(1.0)], {})
call_args("float.call.extra", float, "__lt__", [F(1.0), F(2.0), F(3.0)], {})
call_args("float.call.keyword", float, "__lt__", [F(1.0)], {"value": F(2.0)})
call_args("float.call.wrong-self", float, "__lt__", ["x", F(2.0)], {})
call_args("float.call.wrong-extra", float, "__lt__", ["x", F(2.0), F(3.0)], {})
call_args("float.call.wrong-keyword", float, "__lt__", ["x"], {"value": F(2.0)})
call_args("float.call.keyword-only", float, "__lt__", [], {"value": F(2.0)})


class SiblingS(str):
    pass


call("str.distinct-subclasses.lt", str, "__lt__", S("a"), SiblingS("b"))


# Calling str.__lt__ explicitly must not bounce through either subclass slot.
explicit_events = []


class LoudStr(str):
    def __lt__(self, other):
        explicit_events.append("lt")
        return False

    def __gt__(self, other):
        explicit_events.append("gt")
        return True


call("str.explicit-base", str, "__lt__", LoudStr("a"), LoudStr("b"))
print("str.explicit-events", explicit_events)


# Copying object.__lt__ into the subclass is an explicit override, despite the
# callable value looking like the canonical sentinel.  Provenance, not the
# callable's name, decides whether bound dispatch may use primitive backing.
class ObjectSlotStr(str):
    __lt__ = object.__lt__


call_bound(
    "str.copied-object-slot",
    ObjectSlotStr("a"),
    "__lt__",
    ObjectSlotStr("b"),
)


# The motivating case: a user override may delegate to the explicit base slot.
class ReverseStr(str):
    def __lt__(self, other):
        return str.__gt__(self, other)


reverse_values = [ReverseStr("a"), ReverseStr("b")]
print("delegate.direct", reverse_values[0] < reverse_values[1])
print("delegate.sorted", [str(value) for value in sorted(reverse_values)])
print("delegate.min", str(min(reverse_values)))
print("delegate.max", str(max(reverse_values)))


# A genuine user class has no primitive backing and keeps its own dispatch.
plain_events = []


class Plain:
    def __init__(self, value):
        self.value = value

    def __lt__(self, other):
        plain_events.append((self.value, other.value))
        return self.value < other.value


print("plain", Plain(1) < Plain(2), plain_events)


# `__builtin_data__` is an implementation detail represented as an instance
# attribute in PyRust.  A plain user class must not become a primitive operand
# merely by forging that public-looking name; canonical primitive ancestry is
# required before an explicit base slot may consume the stored backing.
class ForgedBacking:
    pass


for name, owner, _, _, base_left, base_right, _ in cases:
    forged = ForgedBacking()
    forged.__builtin_data__ = base_right
    call(name + ".forged-other.lt", owner, "__lt__", base_left, forged)
