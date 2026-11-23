# Issue #2968:
#   1. Internal payload attributes (`_cursor`, `__builtin_data__`, `_items`,
#      `_buf`, ...) stay completely hidden from the Python-visible surface
#      (`vars()`, `__dict__`, `dir()`, `hasattr()`, `getattr()`, `setattr()`,
#      `delattr()`, `__dict__.clear()`).
#   2. `copy.deepcopy()` never exposes a half-built instance without valid
#      backing to an adversarial `__deepcopy__` inspecting `memo[id(orig)]`,
#      while preserving self-referential cycles across all container families.

import collections
import copy
import io
import itertools

# ── 1. Combinatoric iterators: no __dict__ on exact types, hidden _cursor ────

for name, make in (
    ("combinations", lambda: itertools.combinations([1, 2, 3], 2)),
    ("cwr", lambda: itertools.combinations_with_replacement([1, 2], 2)),
    ("permutations", lambda: itertools.permutations([1, 2, 3], 2)),
    ("product", lambda: itertools.product([1, 2], [3, 4])),
):
    it = make()
    for op in ("vars", "dict", "setattr"):
        try:
            if op == "vars":
                vars(it)
            elif op == "dict":
                _ = it.__dict__
            else:
                it.foo = 1
            res = "UNEXPECTED_OK"
        except Exception as exc:
            res = f"{type(exc).__name__}: {exc}"
        print(name, op, res)
    print(name, "cursor_hidden", hasattr(it, "_cursor"), "_cursor" in dir(it))


class CombSub(itertools.combinations):
    pass


class PermSub(itertools.permutations):
    pass


class ProdSub(itertools.product):
    pass


class CwrSub(itertools.combinations_with_replacement):
    pass


for label, it in (
    ("CombSub", CombSub([1, 2, 3], 2)),
    ("PermSub", PermSub([1, 2], 2)),
    ("ProdSub", ProdSub([1, 2], [3])),
    ("CwrSub", CwrSub([1, 2], 2)),
):
    print(label, "initial", vars(it), hasattr(it, "_cursor"), "_cursor" in dir(it))
    it.tag = "ok"
    # Assigning a user attribute named `_cursor` or clearing `__dict__` must
    # never touch or corrupt the opaque native cursor.
    it._cursor = "user_value"
    print(label, "user_cursor", it._cursor, sorted(vars(it)))
    it.__dict__.clear()
    print(label, "after_clear", vars(it), list(it))

# ── 2. Built-in subclasses & stdlib containers hide internal payloads ────────

for label, obj in (
    ("deque", collections.deque([1, 2], maxlen=5)),
    ("defaultdict", collections.defaultdict(list, a=1)),
):
    try:
        vars(obj)
        v_res = "OK"
    except Exception as exc:
        v_res = f"{type(exc).__name__}: {exc}"
    try:
        obj.foo = 1
        s_res = "OK"
    except Exception as exc:
        s_res = f"{type(exc).__name__}: {exc}"
    print(
        label,
        v_res,
        s_res,
        hasattr(obj, "__builtin_data__"),
        hasattr(obj, "_items"),
        "__builtin_data__" in dir(obj),
        "_items" in dir(obj),
    )

for label, obj in (
    ("OrderedDict", collections.OrderedDict(a=1)),
    ("Counter", collections.Counter("aab")),
    ("StringIO", io.StringIO("hello")),
    ("BytesIO", io.BytesIO(b"hello")),
):
    print(
        label,
        vars(obj),
        hasattr(obj, "__builtin_data__"),
        hasattr(obj, "_buf"),
        "__builtin_data__" in dir(obj),
        "_buf" in dir(obj),
    )


class LSub(list):
    pass


class DSub(dict):
    pass


class SSub(set):
    pass


class TSub(tuple):
    pass


class FSub(frozenset):
    pass


class DQSub(collections.deque):
    pass


class DDSub(collections.defaultdict):
    pass


class ODSub(collections.OrderedDict):
    pass


class CtSub(collections.Counter):
    pass


class BASub(bytearray):
    pass


for cls, args in (
    (LSub, ([1, 2],)),
    (DSub, ({"a": 1},)),
    (SSub, ([1, 2],)),
    (TSub, ([1, 2],)),
    (FSub, ([1, 2],)),
    (DQSub, ([1, 2],)),
    (DDSub, (list, {"a": 1})),
    (ODSub, ({"a": 1},)),
    (CtSub, ("aab",)),
    (BASub, (b"ab",)),
):
    o = cls(*args)
    print(
        cls.__name__,
        "clean",
        vars(o),
        hasattr(o, "__builtin_data__"),
        "__builtin_data__" in dir(o),
        hasattr(o, "_items"),
        "_items" in dir(o),
    )

# User writes to `__builtin_data__` / `_items` / `__dict__.clear()` cannot
# corrupt or graft the internal backing of any built-in subclass.
ls1 = LSub([10, 20])
ls2 = LSub([99])
ls1.__builtin_data__ = "not_a_list"
print("LSub tamper:", list(ls1), len(ls1), ls1.__builtin_data__, vars(ls1))
del ls1.__builtin_data__
print("LSub del:", list(ls1), vars(ls1))
ls1.tag = 1
ls1.__dict__.clear()
print("LSub clear dict:", list(ls1), vars(ls1))
ls1.__builtin_data__ = ls2
ls1.append(30)
print("LSub no graft:", list(ls1), list(ls2))

ds = DSub({"k": 1})
ds.__builtin_data__ = "tampered"
print("DSub tamper:", ds["k"], len(ds), ds.__builtin_data__)
ds.__dict__.clear()
print("DSub clear dict:", dict(ds), vars(ds))

ss = SSub([1, 2])
ss.__builtin_data__ = 123
print("SSub tamper:", sorted(ss), len(ss), ss.__builtin_data__)

dqs = DQSub([1, 2], maxlen=5)
dqs._items = "tampered"
dqs.tag = "hi"
print("DQSub tamper:", list(dqs), dqs.maxlen, dqs._items, dqs.tag)
dqs.__dict__.clear()
print("DQSub clear dict:", list(dqs), dqs.maxlen, vars(dqs))

dds = DDSub(list, a=1)
dds.__builtin_data__ = "tampered"
dds.tag = "hi"
print("DDSub tamper:", dds["a"], dds["missing"], dds.default_factory is list, dds.tag)
dds.__dict__.clear()
print("DDSub clear dict:", dds["a"], dds.default_factory is list, vars(dds))

ct = CtSub("aab")
ct.__builtin_data__ = "tampered"
print("CtSub tamper:", ct["a"], ct["b"], ct.__builtin_data__)

ba = BASub(b"xy")
ba.__builtin_data__ = "tampered"
print("BASub tamper:", bytes(ba), len(ba), ba.__builtin_data__)

sio = io.StringIO("abc")
sio._buf = "tampered"
print("StringIO tamper:", sio.getvalue(), sio._buf)

bio = io.BytesIO(b"abc")
bio._buf = b"tampered"
print("BytesIO tamper:", bio.getvalue(), bio._buf)

# ── 3. Adversarial Spy.__deepcopy__ inspecting memo[id(orig)] ────────────────


class Spy:
    def __init__(self, holder, label):
        self.holder = holder
        self.label = label

    def __deepcopy__(self, memo):
        seen = memo.get(id(self.holder[0]))
        if seen is None:
            print("spy", self.label, "memo=None")
        else:
            print("spy", self.label, type(seen).__name__, len(seen), list(seen))
        return "spied"


for label, make in (
    ("LSub", lambda s: LSub([s])),
    ("DSub", lambda s: DSub({"k": s})),
    ("DQSub", lambda s: DQSub([s])),
    ("deque", lambda s: collections.deque([s])),
    ("ODSub", lambda s: ODSub({"k": s})),
    ("OrderedDict", lambda s: collections.OrderedDict({"k": s})),
    ("DDSub", lambda s: DDSub(list, {"k": s})),
    ("defaultdict", lambda s: collections.defaultdict(list, {"k": s})),
    ("CtSub", lambda s: CtSub({"k": s})),
    ("Counter", lambda s: collections.Counter({"k": s})),
    ("SSub", lambda s: SSub([s])),
    ("TSub", lambda s: TSub([s])),
    ("FSub", lambda s: FSub([s])),
    ("combinations", lambda s: itertools.combinations([s], 1)),
    ("cwr", lambda s: itertools.combinations_with_replacement([s], 1)),
    ("permutations", lambda s: itertools.permutations([s], 1)),
    ("product", lambda s: itertools.product([s])),
    ("CombSub", lambda s: CombSub([s], 1)),
):
    holder = [None]
    obj = make(Spy(holder, label))
    holder[0] = obj
    copied = copy.deepcopy(obj)
    if hasattr(copied, "__next__"):
        print("spy_result", label, type(copied).__name__, list(copied))
    else:
        print("spy_result", label, type(copied).__name__, copied)

# ── 4. Self-referential cycles remain intact ─────────────────────────────────

sl = LSub()
sl.append(sl)
sl_copy = copy.deepcopy(sl)
print("cycle LSub:", type(sl_copy) is LSub, sl_copy is not sl, sl_copy[0] is sl_copy)

sd = DSub()
sd["self"] = sd
sd_copy = copy.deepcopy(sd)
print("cycle DSub:", type(sd_copy) is DSub, sd_copy is not sd, sd_copy["self"] is sd_copy)

sdq = collections.deque()
sdq.append(sdq)
sdq_copy = copy.deepcopy(sdq)
print("cycle deque:", type(sdq_copy) is collections.deque, sdq_copy is not sdq, sdq_copy[0] is sdq_copy)

sdqs = DQSub()
sdqs.append(sdqs)
sdqs_copy = copy.deepcopy(sdqs)
print("cycle DQSub:", type(sdqs_copy) is DQSub, sdqs_copy is not sdqs, sdqs_copy[0] is sdqs_copy)

sod = collections.OrderedDict()
sod["self"] = sod
sod_copy = copy.deepcopy(sod)
print("cycle OrderedDict:", type(sod_copy) is collections.OrderedDict, sod_copy is not sod, sod_copy["self"] is sod_copy)

sdd = collections.defaultdict(list)
sdd["self"] = sdd
sdd_copy = copy.deepcopy(sdd)
print("cycle defaultdict:", type(sdd_copy) is collections.defaultdict, sdd_copy is not sdd, sdd_copy["self"] is sdd_copy)

# Self-referential combinatoric iterator via a mutable pool container
box = []
cit = itertools.combinations([box], 1)
box.append(cit)
cit_copy = copy.deepcopy(cit)
cit_items = list(cit_copy)
print(
    "cycle combinations:",
    len(cit_items),
    type(cit_items[0][0][0]).__name__,
    cit_items[0][0][0] is cit_copy,
)

# Counter self-cycle raises RecursionError cleanly (no native stack overflow)
sct = collections.Counter()
sct["self"] = sct
try:
    copy.deepcopy(sct)
    print("cycle Counter: UNEXPECTED")
except RecursionError:
    print("cycle Counter: RecursionError OK")
