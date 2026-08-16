import copy
import sys
from collections import deque


class Sequence:
    def __init__(self, values):
        self.values = list(values)

    def __getitem__(self, index):
        return self.values[index]

    def __len__(self):
        return len(self.values)


class UnsizedSequence:
    def __getitem__(self, index):
        if index < 2:
            return index + 1
        raise IndexError


METHODS = ("__reduce__", "__reduce_ex__", "__setstate__", "__length_hint__")


def surface(iterator):
    names = dir(iterator)
    return tuple((hasattr(iterator, name), name in names) for name in METHODS)


def reduction_summary(iterator, owner, extended=False):
    try:
        reduction = iterator.__reduce_ex__(4) if extended else iterator.__reduce__()
    except Exception as error:
        return (type(error).__name__, str(error))
    reducer, args = reduction[:2]
    return (
        getattr(reducer, "__name__", type(reducer).__name__),
        len(reduction),
        len(args),
        args[0] is owner if args else False,
        reduction[2] if len(reduction) == 3 else None,
        args == ((),),
    )


def owner_from_reduction(iterator):
    try:
        return iterator.__reduce__()[1][0]
    except Exception as error:
        return (type(error).__name__, str(error))


def no_resurrection(iterator):
    try:
        iterator.__setstate__(0)
        return list(iterator)
    except Exception as error:
        return (type(error).__name__, str(error))


def direct_keyword_errors(iterator):
    rows = []
    try:
        iterator.__reduce__(unused=1)
    except Exception as error:
        rows.append((type(error).__name__, str(error)))
    try:
        iterator.__reduce_ex__(protocol=4)
    except Exception as error:
        rows.append((type(error).__name__, str(error)))
    try:
        iterator.__setstate__(state=0)
    except Exception as error:
        rows.append((type(error).__name__, str(error)))
    try:
        iterator.__length_hint__(unused=1)
    except Exception as error:
        rows.append((type(error).__name__, str(error)))
    try:
        iterator.__getattribute__(name="missing")
    except Exception as error:
        rows.append((type(error).__name__, str(error)))
    return tuple(rows)


def reversed_descriptor_rejections(iterator):
    rows = []
    for method, args in (
        (reversed.__reduce__, ()),
        (reversed.__length_hint__, ()),
        (reversed.__setstate__, (0,)),
        (reversed.__getattribute__, ("__length_hint__",)),
    ):
        try:
            method(iterator, *args)
        except Exception as error:
            rows.append(type(error).__name__)
        else:
            rows.append("accepted")
    return tuple(rows)


print("--- surface and reduction shape ---")
owner = Sequence([10, 20, 30])
forward = iter(owner)
backward = reversed(owner)
print("forward surface", surface(forward))
print("reversed surface", surface(backward))
print("forward reduce", reduction_summary(forward, owner))
print("forward reduce-ex", reduction_summary(forward, owner, True))
print("reversed reduce", reduction_summary(backward, owner))
print("reversed reduce-ex", reduction_summary(backward, owner, True))
print("forward keyword errors", direct_keyword_errors(forward))
print("reversed keyword errors", direct_keyword_errors(backward))


def saved_getattribute_keyword_error(iterator):
    method = iterator.__getattribute__
    try:
        method(name="missing")
    except Exception as error:
        return (type(error).__name__, str(error))
    return ("accepted",)


class R(reversed):
    pass


def reduce_ex_keyword_errors(iterator):
    rows = []
    for saved in (False, True):
        try:
            if saved:
                method = iterator.__reduce_ex__
                method(protocol=4)
            else:
                iterator.__reduce_ex__(protocol=4)
        except Exception as error:
            rows.append((type(error).__name__, str(error)))
        else:
            rows.append(("accepted",))
    return tuple(rows)


print(
    "saved getattribute keyword errors",
    saved_getattribute_keyword_error(iter(Sequence([1]))),
    saved_getattribute_keyword_error(reversed(Sequence([1]))),
    saved_getattribute_keyword_error(reversed((1,))),
)
print(
    "reduce-ex keyword errors",
    tuple(
        (label, reduce_ex_keyword_errors(iterator))
        for label, iterator in (
            ("forward", iter(Sequence([1]))),
            ("legacy-reverse", reversed(Sequence([1]))),
            ("optimized-reverse", reversed((1,))),
            ("subclass", R((1,))),
        )
    ),
)
next(forward)
next(backward)
print("forward advanced", reduction_summary(forward, owner), forward.__length_hint__())
print("reversed advanced", reduction_summary(backward, owner), backward.__length_hint__())
print("unsized hint", iter(UnsizedSequence()).__length_hint__())


print("--- optimized reversed class surface ---")
for label, owner in (
    ("tuple", (1, 2, 3)),
    ("str", "abc"),
    ("bytes", b"abc"),
    ("bytearray", bytearray(b"abc")),
):
    optimized = reversed(owner)
    next(optimized)
    unbound_reduction = reversed.__reduce__(optimized)
    print(
        label,
        type(optimized).__name__,
        surface(optimized),
        reduction_summary(optimized, owner),
        reduction_summary(optimized, owner, True),
        unbound_reduction[0] is reversed,
        unbound_reduction[1][0] is owner,
        unbound_reduction[2],
        reversed.__length_hint__(optimized),
        reversed.__getattribute__(optimized, "__length_hint__")(),
    )
    reversed.__setstate__(optimized, 0)
    print(label, "setstate", list(optimized))


class ListReverseProvider:
    def __reversed__(self):
        return reversed([1, 2, 3])


class ReversedPassthroughSubclass(reversed):
    pass


list_reverse = reversed([1, 2, 3])
subclass_passthrough = ReversedPassthroughSubclass(ListReverseProvider())
print(
    "reversed descriptor negatives",
    type(list_reverse).__name__,
    reversed_descriptor_rejections(list_reverse),
    type(subclass_passthrough).__name__,
    reversed_descriptor_rejections(subclass_passthrough),
)


def reversed_wrong_receiver_arity_errors():
    rows = []
    for method, args in (
        (reversed.__reduce__, (1,)),
        (reversed.__reduce__, (1, 0)),
        (reversed.__length_hint__, (1,)),
        (reversed.__length_hint__, (1, 0)),
        (reversed.__setstate__, (1,)),
        (reversed.__setstate__, (1, 0, 0)),
        (reversed.__getattribute__, (1,)),
        (reversed.__getattribute__, (1, "x", "y")),
    ):
        try:
            method(*args)
        except Exception as error:
            rows.append((type(error).__name__, str(error)))
        else:
            rows.append(("accepted",))
    return tuple(rows)


print("reversed wrong receiver arity", reversed_wrong_receiver_arity_errors())


def descriptor_call_error(method, args=(), kwargs=None):
    try:
        method(*args, **({} if kwargs is None else kwargs))
    except Exception as error:
        return (type(error).__name__, str(error))
    return ("accepted",)


descriptor_sample = reversed((1, 2, 3))
print(
    "reversed descriptor no self",
    tuple(
        descriptor_call_error(method)
        for method in (
            reversed.__reduce__,
            reversed.__length_hint__,
            reversed.__setstate__,
            reversed.__getattribute__,
        )
    ),
)
print(
    "reversed descriptor correct receiver arity",
    descriptor_call_error(reversed.__reduce__, (descriptor_sample, 0)),
    descriptor_call_error(reversed.__length_hint__, (descriptor_sample, 0)),
    descriptor_call_error(reversed.__setstate__, (descriptor_sample,)),
    descriptor_call_error(reversed.__getattribute__, (descriptor_sample,)),
)
print(
    "reversed descriptor wrong receiver keyword",
    descriptor_call_error(reversed.__reduce__, (1,), {"unused": 1}),
    descriptor_call_error(reversed.__length_hint__, (1,), {"unused": 1}),
    descriptor_call_error(reversed.__setstate__, (1,), {"state": 0}),
    descriptor_call_error(reversed.__getattribute__, (1,), {"name": "x"}),
)


print("--- optimized reversed observed exhaustion ---")
for label, owner in (
    ("tuple", (1, 2)),
    ("str", "ab"),
    ("bytes", b"ab"),
    ("bytearray", bytearray(b"ab")),
):
    source = reversed(owner)
    list(source)
    shallow = copy.copy(source)
    deep = copy.deepcopy(source)
    print(
        "optimized exhausted",
        label,
        reduction_summary(source, owner),
        reduction_summary(shallow, owner),
        reduction_summary(deep, owner),
        shallow.__length_hint__(),
        deep.__length_hint__(),
        list(shallow),
        list(deep),
    )

cycle_holder = []
cycle_owner = (cycle_holder,)
cycle_source = reversed(cycle_owner)
cycle_holder.append(cycle_source)
cycle_deep = copy.deepcopy(cycle_source)
cycle_deep_owner = cycle_deep.__reduce__()[1][0]
cycle_linked = cycle_deep_owner[0][0]
cycle_linked_owner = cycle_linked.__reduce__()[1][0]
print(
    "optimized reversed owner cycle",
    cycle_linked is cycle_deep,
    cycle_linked_owner is cycle_deep_owner,
    cycle_deep.__reduce__()[2],
    cycle_linked.__reduce__()[2],
)


print("--- shallow shares and deepcopy detaches ---")
owner = Sequence([[1], [2], [3]])
forward = iter(owner)
next(forward)
forward_shallow = copy.copy(forward)
forward_deep = copy.deepcopy(forward)
owner.values[1].append(9)
print("forward copied", list(forward_shallow), list(forward_deep))

owner = Sequence([[1], [2], [3]])
backward = reversed(owner)
next(backward)
backward_shallow = copy.copy(backward)
backward_deep = copy.deepcopy(backward)
owner.values[1].append(9)
print("reversed copied", list(backward_shallow), list(backward_deep))


class Replacement:
    def __getitem__(self, index):
        if index == 0:
            return "replacement"
        raise IndexError

    def __len__(self):
        return 1


class RebindingSequence:
    def __getitem__(self, index):
        if index == 0:
            return "original"
        raise IndexError

    def __len__(self):
        return 1

    def __deepcopy__(self, memo):
        return Replacement()


print("slot rebound", list(copy.deepcopy(iter(RebindingSequence()))))
print("reverse slot rebound", list(copy.deepcopy(reversed(RebindingSequence()))))


class DynamicSequence(Sequence):
    pass


dynamic_owner = DynamicSequence(["original"])
dynamic_forward = iter(dynamic_owner)
dynamic_backward = reversed(dynamic_owner)
DynamicSequence.__iter__ = lambda self: iter(("dynamic-forward",))
DynamicSequence.__reversed__ = lambda self: iter(("dynamic-reversed",))
print("dynamic shallow", list(copy.copy(dynamic_forward)), list(copy.copy(dynamic_backward)))
print("dynamic deep", list(copy.deepcopy(dynamic_forward)), list(copy.deepcopy(dynamic_backward)))


print("--- reversed subclass custom reducers ---")


class RegularReduceReversed(reversed):
    calls = []

    def __reduce__(self):
        type(self).calls.append("reduce")
        return (list, (("regular-reduce", self.payload),))


class StaticReduceReversed(reversed):
    calls = []

    @staticmethod
    def __reduce__():
        StaticReduceReversed.calls.append("reduce")
        return (list, (("static-reduce",),))


class RegularReduceExReversed(reversed):
    calls = []

    def __reduce_ex__(self, protocol):
        type(self).calls.append(protocol)
        return (list, (("regular-reduce-ex", protocol, self.payload),))


class StaticReduceExReversed(reversed):
    calls = []

    @staticmethod
    def __reduce_ex__(protocol):
        StaticReduceExReversed.calls.append(protocol)
        return (list, (("static-reduce-ex", protocol),))


for cls in (
    RegularReduceReversed,
    StaticReduceReversed,
    RegularReduceExReversed,
    StaticReduceExReversed,
):
    source = cls(Sequence([1, 2]))
    source.payload = [[cls.__name__]]
    direct = source.__reduce_ex__(2)
    shallow = copy.copy(source)
    deep = copy.deepcopy(source)
    shallow_payload = shallow[-1] if isinstance(shallow, list) else None
    deep_payload = deep[-1] if isinstance(deep, list) else None
    print(
        "custom reducer",
        cls.__name__,
        tuple(cls.calls),
        direct[0] is list,
        type(shallow).__name__,
        type(deep).__name__,
        shallow_payload is source.payload,
        deep_payload is source.payload,
    )


print("--- reversed custom reduction reconstruction ---")


class FourReductionReversed(reversed):
    def __reduce_ex__(self, protocol):
        return (list, (), None, iter((self.payload, self.payload)))


four_source = FourReductionReversed((1,))
four_source.payload = [four_source]
four_shallow = copy.copy(four_source)
four_deep = copy.deepcopy(four_source)
print(
    "custom four reduction",
    len(four_shallow),
    four_shallow[0] is four_source.payload,
    four_shallow[0] is four_shallow[1],
    len(four_deep),
    four_deep[0] is four_deep[1],
    four_deep[0] is four_source.payload,
    four_deep[0][0] is four_deep,
)


class FiveReductionReversed(reversed):
    def __reduce_ex__(self, protocol):
        return (
            dict,
            (),
            None,
            None,
            iter((("left", self.payload), ("right", self.payload))),
        )


five_source = FiveReductionReversed((1,))
five_source.payload = [five_source]
five_shallow = copy.copy(five_source)
five_deep = copy.deepcopy(five_source)
print(
    "custom five reduction",
    five_shallow["left"] is five_source.payload,
    five_shallow["left"] is five_shallow["right"],
    five_deep["left"] is five_deep["right"],
    five_deep["left"] is five_source.payload,
    five_deep["left"][0] is five_deep,
)


class SlotReductionTarget:
    __slots__ = ("slot_value", "__dict__")


class SlotStateReversed(reversed):
    def __reduce_ex__(self, protocol):
        return (
            SlotReductionTarget,
            (),
            ({"dict_value": self.payload}, {"slot_value": self.payload}),
        )


slot_source = SlotStateReversed((1,))
slot_source.payload = [slot_source]
slot_shallow = copy.copy(slot_source)
slot_deep = copy.deepcopy(slot_source)
print(
    "custom slot state",
    slot_shallow.dict_value is slot_source.payload,
    slot_shallow.dict_value is slot_shallow.slot_value,
    slot_deep.dict_value is slot_deep.slot_value,
    slot_deep.dict_value is slot_source.payload,
    slot_deep.dict_value[0] is slot_deep,
)


class ReduceFallbackReversed(reversed):
    __reduce_ex__ = None

    def __reduce__(self):
        return (list, (("reduce-fallback", self.payload),))


fallback_source = ReduceFallbackReversed((1,))
fallback_source.payload = []
fallback_shallow = copy.copy(fallback_source)
fallback_deep = copy.deepcopy(fallback_source)
print(
    "custom reducer fallback",
    fallback_shallow[0],
    fallback_shallow[1] is fallback_source.payload,
    fallback_deep[0],
    fallback_deep[1] is fallback_source.payload,
)


class ReduceAttributeFallbackReversed(reversed):
    def __getattribute__(self, name):
        if name == "__reduce_ex__":
            raise AttributeError(name)
        return super(ReduceAttributeFallbackReversed, self).__getattribute__(name)

    def __reduce__(self):
        return (list, (("attribute-fallback", self.payload),))


attribute_fallback_source = ReduceAttributeFallbackReversed((1,))
attribute_fallback_source.payload = []
attribute_fallback_shallow = copy.copy(attribute_fallback_source)
attribute_fallback_deep = copy.deepcopy(attribute_fallback_source)
print(
    "custom reducer attribute fallback",
    attribute_fallback_shallow[0],
    attribute_fallback_shallow[1] is attribute_fallback_source.payload,
    attribute_fallback_deep[0],
    attribute_fallback_deep[1] is attribute_fallback_source.payload,
)


class ReversedSubclass(reversed):
    pass


owner = Sequence([[1], [2], [3]])
subclassed = ReversedSubclass(owner)
next(subclassed)
subclass_shallow = copy.copy(subclassed)
subclass_deep = copy.deepcopy(subclassed)
owner.values[1].append(9)
subclass_reduction = subclassed.__reduce__()
print(
    "reversed class dict",
    tuple(
        (name, name in reversed.__dict__)
        for name in (
            "__getattribute__",
            "__iter__",
            "__next__",
            "__length_hint__",
            "__reduce__",
            "__reduce_ex__",
            "__setstate__",
        )
    ),
)
print(
    "reversed inherited getstate",
    "__getstate__" in dir(reversed),
    "__getstate__" in dir(object),
)
print(
    "reversed subclass reduction",
    type(subclassed) is ReversedSubclass,
    subclass_reduction[0] is ReversedSubclass,
    subclass_reduction[1][0] is owner,
    subclass_reduction[2],
    subclassed.__reduce_ex__(4)[0] is ReversedSubclass,
)
print(
    "reversed subclass copies",
    type(subclass_shallow) is ReversedSubclass,
    type(subclass_deep) is ReversedSubclass,
    list(subclass_shallow),
    list(subclass_deep),
)

owner = Sequence([10, 20, 30])
subclassed = ReversedSubclass(owner)
next(subclassed)
print(
    "reversed unbound methods",
    reversed.__length_hint__(subclassed),
    reversed.__reduce__(subclassed)[0] is ReversedSubclass,
    reversed.__setstate__(subclassed, 0),
    list(subclassed),
)

optimized_subclass_owner = ([10], [20], [30])
optimized_subclass = ReversedSubclass(optimized_subclass_owner)
next(optimized_subclass)
optimized_subclass_shallow = copy.copy(optimized_subclass)
optimized_subclass_deep = copy.deepcopy(optimized_subclass)
optimized_subclass_owner[1].append(9)
optimized_subclass_reduction = optimized_subclass.__reduce__()
print(
    "optimized reversed subclass",
    type(optimized_subclass) is ReversedSubclass,
    optimized_subclass_reduction[0] is ReversedSubclass,
    optimized_subclass_reduction[1][0] is optimized_subclass_owner,
    optimized_subclass_reduction[2],
    optimized_subclass.__reduce_ex__(4)[0] is ReversedSubclass,
    reversed.__reduce__(optimized_subclass)[0] is ReversedSubclass,
    reversed.__length_hint__(optimized_subclass),
)
print(
    "optimized reversed subclass copies",
    type(optimized_subclass_shallow) is ReversedSubclass,
    type(optimized_subclass_deep) is ReversedSubclass,
    list(optimized_subclass_shallow),
    list(optimized_subclass_deep),
)
reversed.__setstate__(optimized_subclass, 0)
print("optimized reversed subclass setstate", list(optimized_subclass))


class SavedOptimizedReversed(reversed):
    pass


saved_optimized = SavedOptimizedReversed((10, 20, 30))
next(saved_optimized)
saved_optimized_reduce = saved_optimized.__reduce__
saved_optimized_hint = saved_optimized.__length_hint__
saved_optimized_setstate = saved_optimized.__setstate__
saved_optimized_getattribute = saved_optimized.__getattribute__
SavedOptimizedReversed.__reduce__ = lambda self: (list, (("new-reduce",),))
SavedOptimizedReversed.__length_hint__ = lambda self: 99
SavedOptimizedReversed.__setstate__ = lambda self, state: "new-setstate"


def saved_optimized_replacement_getattribute(self, name):
    if name == "marker":
        return "new-getattribute"
    return super(SavedOptimizedReversed, self).__getattribute__(name)


SavedOptimizedReversed.__getattribute__ = saved_optimized_replacement_getattribute
try:
    saved_optimized_marker = saved_optimized_getattribute("marker")
except Exception as error:
    saved_optimized_marker = type(error).__name__
print(
    "optimized reversed saved inherited",
    saved_optimized.__reduce__()[0] is list,
    saved_optimized_reduce()[0] is SavedOptimizedReversed,
    saved_optimized.__length_hint__(),
    saved_optimized_hint(),
    saved_optimized.__setstate__(0),
    saved_optimized_setstate(0),
    list(saved_optimized),
    saved_optimized.marker,
    saved_optimized_marker,
)


class CustomGetattributeReversed(reversed):
    def __getattribute__(self, name):
        if name == "marker":
            return "custom"
        return super().__getattribute__(name)


custom_getattribute = CustomGetattributeReversed(Sequence([1]))
try:
    unbound_marker = reversed.__getattribute__(custom_getattribute, "marker")
except Exception as error:
    unbound_marker = type(error).__name__
print("reversed base getattribute", custom_getattribute.marker, unbound_marker)


class SavedGetattributeReversed(reversed):
    pass


saved_getattribute_owner = SavedGetattributeReversed(Sequence([1]))
saved_getattribute = saved_getattribute_owner.__getattribute__


def replacement_getattribute(self, name):
    if name == "marker":
        return "replacement"
    return super(SavedGetattributeReversed, self).__getattribute__(name)


SavedGetattributeReversed.__getattribute__ = replacement_getattribute
try:
    saved_marker = saved_getattribute("marker")
except Exception as error:
    saved_marker = type(error).__name__
print("reversed saved getattribute", saved_getattribute_owner.marker, saved_marker)


print("--- memo and cycles ---")
owner = Sequence([1, 2])
source = iter(owner)
memo_first, memo_second = copy.deepcopy((source, source))
print("iterator memo", memo_first is memo_second)

owner = Sequence([1, 2])
first, second = copy.deepcopy((iter(owner), iter(owner)))
first_owner = owner_from_reduction(first)
second_owner = owner_from_reduction(second)
if isinstance(first_owner, tuple) or isinstance(second_owner, tuple):
    print("shared copied owner", first_owner, second_owner)
else:
    print("shared copied owner", first_owner is second_owner, first_owner is owner)

owner = Sequence([])
owner.values.append(owner)
cyclic = copy.deepcopy(iter(owner))
cyclic_owner = owner_from_reduction(cyclic)
if isinstance(cyclic_owner, tuple):
    print("owner cycle", cyclic_owner)
else:
    print(
        "owner cycle",
        cyclic_owner is owner,
        cyclic_owner.values[0] is cyclic_owner,
        next(cyclic) is cyclic_owner,
    )

owner = Sequence([1, 2])
outer = iter(owner)
owner.iterator = outer
copied_outer = copy.deepcopy(outer)
copied_owner = owner_from_reduction(copied_outer)
copied_inner = copied_owner.iterator
print(
    "iterator owner cycle",
    copied_inner is copied_outer,
    owner_from_reduction(copied_inner) is copied_owner,
    next(copied_inner),
    next(copied_outer),
)


print("--- setstate boundaries ---")


class DeletedLenSequence:
    def __getitem__(self, index):
        if index < 2:
            return index
        raise IndexError

    def __len__(self):
        return 2


deleted_len_owner = DeletedLenSequence()
deleted_len_reversed = reversed(deleted_len_owner)
del DeletedLenSequence.__len__
try:
    deleted_len_reversed.__setstate__(0)
except Exception as error:
    print("reversed setstate deleted len", type(error).__name__, str(error))
else:
    print("reversed setstate deleted len", "accepted")


def setstate_summary(reverse, state):
    iterator = reversed(Sequence([10, 20, 30])) if reverse else iter(Sequence([10, 20, 30]))
    try:
        result = iterator.__setstate__(state)
    except Exception as error:
        return (type(error).__name__, str(error))
    return (result is None, iterator.__length_hint__(), list(iterator))


for state in (-5, 1, 10):
    print("forward state", state, setstate_summary(False, state))
for state in (-5, 1, 10):
    print("reversed state", state, setstate_summary(True, state))
print("state float", setstate_summary(False, 1.0))
print("state huge", setstate_summary(False, 2**100))


class ReentrantLenSequence(Sequence):
    def __init__(self, values):
        super().__init__(values)
        self.iterator = None
        self.armed = False

    def __len__(self):
        if self.armed:
            self.armed = False
            list(self.iterator)
        return super().__len__()


owner = ReentrantLenSequence([10, 20, 30])
backward = reversed(owner)
owner.iterator = backward
owner.armed = True
print(
    "reversed state reentrant exhaustion",
    backward.__setstate__(0),
    reduction_summary(backward, owner),
    backward.__length_hint__(),
)

owner = Sequence([10])
forward = iter(owner)
next(forward)
print("forward yielded-last", reduction_summary(forward, owner), no_resurrection(forward))

owner = Sequence([10])
backward = reversed(owner)
next(backward)
before_restore = reduction_summary(backward, owner)
backward.__setstate__(0)
print("reversed yielded-last", before_restore, list(backward))

owner = Sequence([])
backward = reversed(owner)
print("reversed initially-empty", reduction_summary(backward, owner))


class OverflowSequence:
    calls = 0

    def __getitem__(self, index):
        type(self).calls += 1
        raise IndexError


overflow = iter(OverflowSequence())
overflow.__setstate__(sys.maxsize)
try:
    next(overflow)
except Exception as error:
    print("forward max", type(error).__name__, str(error), OverflowSequence.calls)


print("--- exhausted state stays released ---")
owner = Sequence([1, 2])
forward = iter(owner)
list(forward)
owner.values.append(3)
print("forward exhausted", reduction_summary(forward, owner), forward.__length_hint__())
print("forward no resurrection", no_resurrection(forward))

owner = Sequence([1, 2])
backward = reversed(owner)
list(backward)
owner.values.append(3)
print("reversed exhausted", reduction_summary(backward, owner), backward.__length_hint__())
print("reversed no resurrection", no_resurrection(backward))


class DeepcopyCounter(Sequence):
    calls = 0

    def __deepcopy__(self, memo):
        type(self).calls += 1
        return type(self)(copy.deepcopy(self.values, memo))


for label, make in (
    ("forward exhausted copy", lambda value: iter(value)),
    ("reversed exhausted copy", lambda value: reversed(value)),
):
    DeepcopyCounter.calls = 0
    owner = DeepcopyCounter([1])
    iterator = make(owner)
    list(iterator)
    shallow = copy.copy(iterator)
    copied = copy.deepcopy(iterator)
    print(
        label,
        DeepcopyCounter.calls,
        type(shallow).__name__,
        type(copied).__name__,
        shallow.__length_hint__(),
        copied.__length_hint__(),
        list(shallow),
        list(copied),
    )


print("--- deque regression rows ---")
for label, make in (
    ("deque", lambda value: iter(value)),
    ("deque-reversed", lambda value: reversed(value)),
):
    owner = deque([[1], [2], [3]])
    iterator = make(owner)
    next(iterator)
    shallow = copy.copy(iterator)
    deep = copy.deepcopy(iterator)
    shallow_owner = shallow.__reduce__()[1][0]
    deep_owner = deep.__reduce__()[1][0]
    owner[1].append(9)
    print(label, surface(iterator))
    print(label, "reduce", reduction_summary(iterator, owner))
    print(label, "reduce-ex", reduction_summary(iterator, owner, True))
    print(label, "owners", shallow_owner is owner, deep_owner is owner)
    print(label, "copied", list(shallow), list(deep))
