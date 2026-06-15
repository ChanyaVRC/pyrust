# Parity fixture for issue #2479.
#
# Unbound OrderedDict methods that OrderedDict *owns* (re-exposes as its own
# C method_descriptor, __objclass__ is OrderedDict) reject an unrelated
# receiver with:
#   TypeError: descriptor '<m>' for 'collections.OrderedDict' objects
#              doesn't apply to a '<X>' object
# pyrust previously validated only the receiver's value KIND (dict-like), so a
# plain dict silently passed and the call mutated it.
#
# Methods OrderedDict merely INHERITS from dict (objclass dict, e.g. `get`)
# keep accepting any dict receiver; a *user* Python subclass of dict does not
# re-own these and its own Python `def`s stay plain functions — neither
# enforces.

from collections import OrderedDict


class OD2(OrderedDict):
    pass


class MyDict(dict):
    def shout(self):
        return "HI"


def show(label, fn):
    try:
        result = fn()
        print(label, "->", result)
    except TypeError as e:
        print(label, "TypeError:", e)
    except KeyError as e:
        print(label, "KeyError:", e)


# --- owned methods reject a plain-dict receiver ---
for m in ("clear", "pop", "popitem", "update", "setdefault",
          "copy", "keys", "values", "items"):
    show(f"OrderedDict.{m}(plain)", lambda m=m: getattr(OrderedDict, m)({1: 2}))

show("OrderedDict.move_to_end(plain)",
     lambda: OrderedDict.move_to_end({1: 2}, 1))

# Owner is reported as collections.OrderedDict even when accessed on a subclass.
show("OD2.clear(plain)", lambda: OD2.clear({1: 2}))
show("OD2.move_to_end(plain)", lambda: OD2.move_to_end({1: 2}, 1))

# Empty call: "needs an argument" wording.
show("OrderedDict.clear()", lambda: OrderedDict.clear())
show("OrderedDict.move_to_end()", lambda: OrderedDict.move_to_end())

# --- correct receivers still work ---
od = OrderedDict(a=1, b=2)
show("OrderedDict.clear(od)", lambda: (OrderedDict.clear(od), dict(od))[1])

o2 = OD2(x=1)
show("OrderedDict.clear(OD2 instance)",
     lambda: (OrderedDict.clear(o2), dict(o2))[1])

od2 = OrderedDict(a=1, b=2)
show("OrderedDict.keys(od)", lambda: list(OrderedDict.keys(od2)))

# --- inherited (objclass dict) methods accept any dict receiver ---
show("OrderedDict.get(plain)", lambda: OrderedDict.get({1: 2}, 1))

# --- plain dict.clear stays unguarded ---
show("dict.clear(plain)", lambda: (dict.clear({1: 2}), "ok")[1])

# --- a user dict subclass does NOT enforce (Python function / inherited C) ---
print("MyDict.shout type:", type(MyDict.shout).__name__)
show("MyDict.shout(plain)", lambda: MyDict.shout({1: 2}))
show("MyDict.clear(plain)", lambda: (MyDict.clear({1: 2}), "ok")[1])

# --- descriptor introspection ---
print("clear type:", type(OrderedDict.clear).__name__)
print("move_to_end type:", type(OrderedDict.move_to_end).__name__)
print("clear name/qualname:",
      OrderedDict.clear.__name__, OrderedDict.clear.__qualname__)
print("clear objclass:", OrderedDict.clear.__objclass__.__name__)
print("move_to_end repr:", repr(OrderedDict.move_to_end))
