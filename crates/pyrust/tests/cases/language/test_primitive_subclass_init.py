# Parity fixture for issue #988: super().__init__() in a dict/list/set subclass
# must not raise AttributeError.  The primitive __init__ is a no-op that
# accepts the same args as the constructor.

# ── dict subclass ─────────────────────────────────────────────────────────────

class MyDict(dict):
    def __init__(self):
        super().__init__()

d = MyDict()
print(type(d).__name__)           # MyDict
print(isinstance(d, dict))        # True
d['k'] = 'v'
print(d['k'])                     # v

# Subclass that stores a tag alongside the inherited dict behaviour.
class TaggedDict(dict):
    def __init__(self, tag, mapping=()):
        super().__init__()
        self.tag = tag
        if mapping:
            self.update(mapping)

td = TaggedDict("env", {"a": 1, "b": 2})
print(td.tag)                     # env
print(td["a"])                    # 1
print(sorted(td.keys()))          # ['a', 'b']

# ── list subclass ─────────────────────────────────────────────────────────────

class MyList(list):
    def __init__(self):
        super().__init__()

l = MyList()
print(type(l).__name__)           # MyList
print(isinstance(l, list))        # True
l.append(7)
print(l[0])                       # 7

# Subclass that initialises by extending after the super().__init__() call.
class PrefixedList(list):
    def __init__(self, prefix, items=()):
        super().__init__()
        self.prefix = prefix
        self.extend(items)

pl = PrefixedList("p", [1, 2, 3])
print(pl.prefix)                  # p
print(pl[0], pl[1], pl[2])        # 1 2 3
print(len(pl))                    # 3

# ── set subclass ──────────────────────────────────────────────────────────────

class MySet(set):
    def __init__(self):
        super().__init__()

s = MySet()
print(type(s).__name__)           # MySet
print(isinstance(s, set))         # True
s.add(42)
# Verify the element was added via len (avoids the pre-existing `in` dispatch
# limitation for set subclasses, tracked separately in issue #989).
print(len(s))                     # 1

# Subclass that populates the set after super().__init__().
class UniqueList(set):
    def __init__(self, items=()):
        super().__init__()
        self.update(items)

ul = UniqueList([1, 2, 2, 3])
print(len(ul))                    # 3

# ── multiple levels of inheritance ────────────────────────────────────────────

class ExtDict(MyDict):
    def __init__(self):
        super().__init__()

ed = ExtDict()
print(type(ed).__name__)          # ExtDict
print(isinstance(ed, dict))       # True
ed['x'] = 99
print(ed['x'])                    # 99

# ── super().__init__() with variadic *args pass-through ───────────────────────

class MyList2(list):
    def __init__(self, *args):
        super().__init__(*args)

l2 = MyList2([10, 20, 30])
print(type(l2).__name__)          # MyList2
