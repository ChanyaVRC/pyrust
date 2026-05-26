# Parity fixture for repr() / str() of containers holding user-defined instances.
# Issue #1196: containers were calling Value::repr() on elements, which produces
# the default <Module.ClassName object at 0x...> form instead of dispatching
# __repr__ on the element.


class Item:
    def __init__(self, val):
        self.val = val

    def __repr__(self):
        return f"Item({self.val})"


a = Item(1)
b = Item(2)

# repr() on list, tuple, dict
print(repr([a, b]))  # [Item(1), Item(2)]
print(repr((a, b)))  # (Item(1), Item(2))
print(repr({"k": a}))  # {'k': Item(1)}
print(repr({1: a, 2: b}))  # {1: Item(1), 2: Item(2)}

# str() of a container calls repr() in CPython — must match
print(str([a, b]))  # [Item(1), Item(2)]

# print() of a container (goes through str path)
print([a, b])  # [Item(1), Item(2)]
print((a,))  # (Item(1),)
print({"k": a})  # {'k': Item(1)}

# f-string !r conversion on a container
print(f"{[a]!r}")  # [Item(1)]
print(f"{(a,)!r}")  # (Item(1),)

# Nested containers
print(repr([a, [b]]))  # [Item(1), [Item(2)]]
print(repr((a, (b,))))  # (Item(1), (Item(2),))
print(repr({"k": [a]}))  # {'k': [Item(1)]}

# Empty containers (no regression)
print(repr([]))  # []
print(repr(()))  # ()
print(repr({}))  # {}
print(repr(set()))  # set()

# Single-element tuple (must keep trailing comma)
print(repr((a,)))  # (Item(1),)


# __repr__ returns a non-string → TypeError
class BadRepr:
    def __repr__(self):
        return 42


try:
    repr([BadRepr()])
except TypeError as e:
    print(f"TypeError: {e}")  # TypeError: __repr__ returned non-string (type int)


# set with a hashable user instance
class Hashable:
    def __repr__(self):
        return "H"

    def __hash__(self):
        return 1

    def __eq__(self, other):
        return type(other) is Hashable


h = Hashable()
print(repr({h}))  # {H}


# Instance with no __repr__ defined falls back to default (address-based),
# which we can only test structurally.
class Plain:
    pass


p = Plain()
r = repr([p])
print(r.startswith("[<") and r.endswith(">]"))  # True


# Instance inside dict value
class Val:
    def __repr__(self):
        return "V"


print(repr({"a": Val()}))  # {'a': V}


# User instance as a dict key (via PyKey::Object) and set element
class HashKey:
    def __init__(self, n):
        self.n = n

    def __repr__(self):
        return f"K{self.n}"

    def __hash__(self):
        return self.n

    def __eq__(self, other):
        return type(other) is HashKey and self.n == other.n


k1 = HashKey(1)
k2 = HashKey(2)
d = {k1: "x", k2: "y"}
# Dict key order is insertion order; repr must show user repr for keys.
print(d[k1])  # x  (sanity check the key works)
# repr of keys must call __repr__
r = repr(d)
print("K1" in r and "K2" in r)  # True


# Exception instances inside containers: custom __repr__ must be dispatched
class MyExc(Exception):
    def __repr__(self):
        return "MyExc!"


e = MyExc("msg")
print(repr(e))  # MyExc!
print(repr([e]))  # [MyExc!]

# Exception without custom __repr__: built-in formatting must be preserved
class PlainExc(Exception):
    pass


p = PlainExc("msg")
print(repr(p))  # PlainExc('msg')
print(repr([p]))  # [PlainExc('msg')]


# frozenset with a hashable user instance — __repr__ must be dispatched
class FH:
    def __repr__(self):
        return "FH"

    def __hash__(self):
        return 1

    def __eq__(self, other):
        return type(other) is FH


fh = FH()
print(repr(frozenset({fh})))   # frozenset({FH})
print(str(frozenset({fh})))    # frozenset({FH})
print(frozenset({fh}))         # frozenset({FH})

# frozenset inside a list
print(repr([frozenset({fh})]))  # [frozenset({FH})]

# frozenset as a dict key
d_fs = {frozenset({fh}): "v"}
print(repr(d_fs))  # {frozenset({FH}): 'v'}

# tuple as a dict key where an element is a user instance
class TK:
    def __repr__(self):
        return "TK"

    def __hash__(self):
        return 42

    def __eq__(self, other):
        return type(other) is TK


tk = TK()
d_tk = {(tk, 1): "v"}
print(repr(d_tk))  # {(TK, 1): 'v'}

# PyKey::FrozenSet nested inside a PyKey::FrozenSet key (edge case)
# frozenset of frozensets — inner frozenset contains a user obj
inner_fs = frozenset({fh})
outer_d = {frozenset({inner_fs}): "w"}
print(repr(outer_d))  # {frozenset({frozenset({FH})}): 'w'}
