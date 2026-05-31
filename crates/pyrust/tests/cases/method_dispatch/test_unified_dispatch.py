# Unified CallMethod / CallMethodExpanded dispatch parity (#431).
# Exercises every carve-out routed through the shared
# dispatch_builtin_container_method: list sort/index/count, dict views and
# get/pop/setdefault/contains, str format/format_map/maketrans, set
# add/discard/remove/contains, tuple index/count, __iter__, primitive-subclass
# backing, and the relevant error paths — on BOTH the no-kwargs and the
# kwargs/expanded opcode.


class E:
    def __init__(self, v):
        self.v = v

    def __eq__(self, o):
        return isinstance(o, E) and self.v == o.v

    def __hash__(self):
        return hash(self.v)


# --- list.sort: plain, reverse=, key=, key=+reverse= (expanded opcode) ---
a = [3, 1, 2, -5, 4]
a.sort()
print("sort_plain", a)

b = [3, 1, 2]
b.sort(reverse=True)
print("sort_reverse", b)

c = [3, 1, 2, -5, 4]
c.sort(key=lambda v: abs(v))
print("sort_key", c)

d = [3, 1, 2, -5, 4]
d.sort(key=abs, reverse=True)
print("sort_key_reverse", d)

# --- list.index / count: plain + user __eq__ ---
print("index_plain", [10, 20, 30].index(20))
print("count_plain", [1, 1, 2, 1].count(1))
print("index_start", [1, 2, 1, 2].index(2, 2))
print("index_user", [E(1), E(2), E(3)].index(E(2)))
print("count_user", [E(1), E(1), E(2)].count(E(1)))

# --- dict views are live ---
m = {"a": 1, "b": 2}
ks, vs, its = m.keys(), m.values(), m.items()
m["c"] = 3
print("keys", sorted(ks))
print("values", sorted(vs))
print("items", sorted(its))

# --- dict get/pop/setdefault/contains with user key (interpreter path) ---
dd = {E(1): "x"}
print("get", dd.get(E(1)), dd.get(E(9), "default"))
print("contains", E(1) in dd, E(9) in dd)
print("setdefault", dd.setdefault(E(2), "y"), dd.setdefault(E(1), "z"))
print("pop", dd.pop(E(1)), dd.pop(E(9), "none"))

# --- str templating ---
print("format", "{0}/{name}/{1}".format(7, 9, name="z"))
print("format_map", "{a}{b}".format_map({"a": 1, "b": 2}))
tbl = str.maketrans("abc", "xyz")
print("maketrans", "cabbage".translate(tbl))

# --- str ordinary methods (no kwargs + kwargs-merge path) ---
print("upper", "Hello".upper())
print("split", "a,b,c".split(","))
print("replace", "aaa".replace("a", "b"))

# --- set add/discard/remove/contains with user element ---
s = set()
s.add(E(1))
s.add(E(1))
s.add(E(2))
print("set_len", len(s))
print("set_contains", E(1) in s, E(9) in s)
s.discard(E(9))
s.remove(E(1))
print("set_after", len(s))

# --- tuple index/count ---
print("tuple_index", (1, 2, 3).index(3))
print("tuple_count", (1, 1, 2, 1).count(1))
print("tuple_index_user", (E(1), E(2), E(3)).index(E(3)))

# --- __iter__ on builtins ---
print("list_iter", list(iter([1, 2, 3])))
print("tuple_iter", list(iter((4, 5))))
print("dict_iter", sorted(iter({"x": 1, "y": 2})))
print("set_iter", sorted(iter({1, 2, 3})))

# --- primitive subclass backing (#976) ---
class MyList(list):
    pass


ml = MyList([3, 1, 2])
ml.append(0)
ml.sort()
print("mylist", ml, ml.index(2), ml.count(1))


class MyDict(dict):
    pass


md = MyDict()
md["k"] = "v"
print("mydict", md.get("k"), "k" in md)


class MySet(set):
    pass


ms = MySet()
ms.add(1)
ms.add(1)
print("myset", len(ms), 1 in ms)

# --- error paths preserved ---
# Note: the exact "unexpected keyword argument" wording for sort() diverges
# from CPython 3.12 (pre-existing, unrelated to #431); assert only the class.
try:
    [1, 2].sort(badkw=1)
except TypeError:
    print("sort_badkw", "TypeError")

try:
    [1, 2, 3].index(99)
except ValueError as e:
    print("index_missing", type(e).__name__)

try:
    "{a}".format_map({"a": 1}, {"b": 2})
except TypeError as e:
    print("format_map_args", type(e).__name__)

try:
    set().remove(1)
except KeyError as e:
    print("set_remove_missing", type(e).__name__)
