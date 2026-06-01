# Issue #1863: bound_method_dispatch_inner hands its positional-args buffer
# to the callee via std::mem::take instead of pos.drain(..).collect().
# Behaviour must be identical. The buffer is reused across calls, so this
# fixture calls each affected arm repeatedly (varying arg counts) to exercise
# the empty->grow->empty cycle, and covers every Vec-ownership arm:
# list, dict, set, str(no-kw + kw-merge), bytes.join, tuple, complex,
# range, generator, and the primitive-subclass backing path.

# list arm: varying arg counts across iterations
l = []
for i in range(5):
    l.append(i)
    l.insert(0, -i)
l.extend([100, 200])
print("list", l, l.index(100), l.count(0))

# dict arm
d = {}
for k in ("a", "b", "c"):
    d.update({k: len(k)})
print("dict", sorted(d.items()), d.get("a"), d.get("z", -1))

# set arm
s = set()
for i in range(4):
    s.add(i % 2)
s.update({5, 6})
print("set", sorted(s))

# str no-kwargs arm + kwargs-merge arm, repeated
acc = []
for i in range(3):
    acc.append("a,b,c".split(",", 1))
    acc.append("xyz".upper())
print("str", acc)
print("str_kw", "ABCABC".replace("A", "_", 1))
print("str_format", "{0}-{x}".format(7, x="q"))

# bytes.join arm
print("bytes_join", b",".join([b"1", b"2", b"3"]))

# tuple arm
print("tuple", (9, 8, 8, 7).index(8), (9, 8, 8, 7).count(8))

# complex arm
print("complex", (3 + 4j).conjugate())

# range arm: __len__, count, index
r = range(2, 20, 3)
print("range", len(r), r.count(8), r.index(11))

# generator arm
def g():
    yield 10
    yield 20
    yield 30
it = g()
print("gen", next(it), it.__next__())

# primitive subclass backing path (PyInstance -> backing dispatch)
class MyList(list):
    pass


ml = MyList([2, 1])
for i in range(3):
    ml.append(i)
ml.sort()
print("mylist", ml, ml.count(1))


class MyDict(dict):
    pass


# fromkeys classmethod via subclass-instance backing path
fk = MyDict().fromkeys(["p", "q", "r"], 0)
print("fromkeys", type(fk).__name__, sorted(fk.items()))

# error paths still raise the right classes
try:
    [1, 2, 3].index(99)
except ValueError as e:
    print("index_missing", type(e).__name__)

try:
    range(3).index(99)
except ValueError as e:
    print("range_index_missing", type(e).__name__)
