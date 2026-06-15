# Parity fixture for issue #2475.
#
# Follow-up to #2474: the *unbound* call form of clear() during iteration must
# match the bound form's wording. CPython 3.12 decides the wording from the
# RECEIVER's actual type, not the class the method was looked up on:
#   - OrderedDict.clear(od)  -> "OrderedDict changed size during iteration"
#   - dict.clear(od)         -> "OrderedDict changed size during iteration"
#                               (od is an OrderedDict)
#   - Subclass.clear(od)     -> "OrderedDict changed size during iteration"
#   - dict.clear(d)          -> "dictionary changed size during iteration"
#                               (d is a plain dict)
# Non-clear unbound mutations (__delitem__/__setitem__) still say "mutated".
#
# #2474 hooked the bound-method paths but not the unbound function-call
# dispatcher, so pyrust said "mutated" for every unbound clear().

from collections import OrderedDict


class ODSub(OrderedDict):
    pass


def run(label, factory, mutate):
    d = factory(a=1, b=2, c=3)
    try:
        for _ in d:
            mutate(d)
    except RuntimeError as e:
        print(label, "->", e)
    else:
        print(label, "-> (no error)")


def run_view(label, factory, view, mutate):
    d = factory(a=1, b=2, c=3)
    try:
        for _ in getattr(d, view)():
            mutate(d)
    except RuntimeError as e:
        print(label, "->", e)
    else:
        print(label, "-> (no error)")


# Unbound clear() — direct iteration.
run("OrderedDict.clear(od)", OrderedDict, lambda d: OrderedDict.clear(d))
run("dict.clear(od)", OrderedDict, lambda d: dict.clear(d))
run("ODSub.clear(odsub)", ODSub, lambda d: ODSub.clear(d))
run("OrderedDict.clear(odsub)", ODSub, lambda d: OrderedDict.clear(d))
run("dict.clear(odsub)", ODSub, lambda d: dict.clear(d))
run("dict.clear(plain)", dict, lambda d: dict.clear(d))

# Unbound clear() — view iteration.
for view in ("keys", "values", "items"):
    run_view(f"OrderedDict.clear(od) [{view}]", OrderedDict, view,
             lambda d: OrderedDict.clear(d))
    run_view(f"dict.clear(plain) [{view}]", dict, view,
             lambda d: dict.clear(d))

# Unbound non-clear mutations still say "mutated".
run("OrderedDict.__delitem__(od, k)", OrderedDict,
    lambda d: OrderedDict.__delitem__(d, next(iter(d))))
run("OrderedDict.pop(od, k)", OrderedDict,
    lambda d: OrderedDict.pop(d, next(iter(d))))
run("OrderedDict.__setitem__(od, k)", OrderedDict,
    lambda d: OrderedDict.__setitem__(d, "z", 9))
