# Parity test for `weakref` stdlib module (CPython 3.12, issue #3190).
import weakref


class Foo:
    def __init__(self, x=0):
        self.x = x


class EqFoo:
    def __init__(self, x):
        self.x = x

    def __eq__(self, other):
        return isinstance(other, EqFoo) and self.x == other.x

    def __hash__(self):
        return hash(self.x)


class UnhashableFoo:
    __hash__ = None


class SlottedNoWeakref:
    __slots__ = ("a",)

    def __init__(self, a):
        self.a = a


class SlottedWithWeakref:
    __slots__ = ("a", "__weakref__")

    def __init__(self, a):
        self.a = a


class SlottedChildOfWeakref(SlottedWithWeakref):
    __slots__ = ("b",)


class DictChildOfSlotted(SlottedNoWeakref):
    pass


# ── 1. Module exports & aliases ─────────────────────────────────────────────
print("ref is ReferenceType:", weakref.ref is weakref.ReferenceType)
print("ReferenceError is Exception:", issubclass(ReferenceError, Exception))
print("ProxyTypes:", weakref.ProxyTypes == (weakref.ProxyType, weakref.CallableProxyType))
for name in [
    "ref",
    "proxy",
    "getweakrefcount",
    "getweakrefs",
    "WeakKeyDictionary",
    "ReferenceType",
    "ProxyType",
    "CallableProxyType",
    "ProxyTypes",
    "WeakValueDictionary",
    "WeakSet",
    "WeakMethod",
    "finalize",
]:
    assert name in weakref.__all__, f"missing {name} in __all__"
print("all exports present: True")


# ── 2. Basic ref liveness, death on del / reassignment / scope exit ─────────
a = Foo(42)
r = weakref.ref(a)
print("alive deref:", r() is a, r().x)
print("repr alive:", repr(r).startswith("<weakref at 0x"), "; to 'Foo' at 0x" in repr(r))
print("callback None:", r.__callback__ is None)

# Deduplication for plain ref without callback
r2 = weakref.ref(a)
print("plain dedup:", r is r2, weakref.getweakrefcount(a))
print("getweakrefs len:", len(weakref.getweakrefs(a)), weakref.getweakrefs(a)[0] is r)

del a
print("dead after del:", r() is None)
print("repr dead:", repr(r).startswith("<weakref at 0x"), repr(r).endswith("; dead>"))

# Reassignment kills referent
b = Foo(99)
rb = weakref.ref(b)
b = None
print("dead after reassign:", rb() is None)


# Function-scope exit kills referent
def make_ref():
    local_obj = Foo(7)
    return weakref.ref(local_obj)


r_local = make_ref()
print("dead after scope exit:", r_local() is None)

# Temporary referent dies immediately
r_temp = weakref.ref(Foo(123))
print("dead temp:", r_temp() is None)


# ── 3. Callbacks: invocation, dead-inside-cb, LIFO order, dropped-ref skip ──
cb_log = []


def make_cb(tag):
    def cb(ref_obj):
        cb_log.append((tag, ref_obj() is None, ref_obj.__callback__ is None))

    return cb


c = Foo(1)
rc1 = weakref.ref(c, make_cb("first"))
rc2 = weakref.ref(c, make_cb("second"))
print("with-cb distinct:", rc1 is not rc2, weakref.getweakrefcount(c))
print("cb attr alive:", callable(rc1.__callback__))
del c
print("cb log LIFO:", cb_log)
print("cb attr dead:", rc1.__callback__ is None, rc2.__callback__ is None)

# Dropping the weakref BEFORE referent dies must NOT invoke callback
skipped = []
d = Foo(2)
rd = weakref.ref(d, lambda ref_obj: skipped.append("should_not_run"))
print("count before del ref:", weakref.getweakrefcount(d))
del rd
print("count after del ref:", weakref.getweakrefcount(d))
del d
print("skipped cb log:", skipped)


# ── 4. Equality & hash caching across death ─────────────────────────────────
e1 = EqFoo(10)
e2 = EqFoo(10)
e3 = EqFoo(20)
re1 = weakref.ref(e1)
re2 = weakref.ref(e2)
re3 = weakref.ref(e3)
print("eq alive:", re1 == re2, re1 != re2, re1 == re3, re1 != re3, re1 == 10, re1 != 10)
h_alive = hash(re1)
print("hash matches referent:", h_alive == hash(e1), hash(re1) == hash(re2))
del e1
print("hash preserved after death:", hash(re1) == h_alive)
print("eq dead vs alive:", re1 == re2, re1 != re2, re1 == re1, re1 != re1)

# Hash never computed while alive raises TypeError after death
e4 = EqFoo(30)
re4 = weakref.ref(e4)
del e4
try:
    hash(re4)
    print("ERROR: expected TypeError")
except TypeError as exc:
    print("hash dead uncached:", str(exc))

# Unhashable referent raises TypeError
u = UnhashableFoo()
ru = weakref.ref(u)
try:
    hash(ru)
    print("ERROR: expected TypeError")
except TypeError as exc:
    print("hash unhashable:", "unhashable" in str(exc))


# ── 5. Weakrefable vs non-weakrefable types & __slots__ ─────────────────────
# Non-weakrefable builtins
for val in [1, "abc", [1, 2], {"a": 1}, (1, 2), None, b"bytes", 3.14, 1 + 2j, range(5), object(), ValueError("x")]:
    try:
        weakref.ref(val)
        print("UNEXPECTED OK:", type(val).__name__)
    except TypeError as exc:
        assert "cannot create weak reference to" in str(exc), str(exc)
print("non-weakrefable builtins rejected: True")
print("getweakrefcount non-weakrefable:", weakref.getweakrefcount(42), weakref.getweakrefs("hi"))

# Weakrefable non-instance objects: class, function, set, frozenset, generator
def sample_fn():
    return 1


def sample_gen():
    yield 1


s_obj = {1, 2, 3}
fs_obj = frozenset({4, 5})
g_obj = sample_gen()
print("ref class:", weakref.ref(Foo)() is Foo)
print("ref func:", weakref.ref(sample_fn)() is sample_fn)
print("ref set:", weakref.ref(s_obj)() is s_obj)
print("ref frozenset:", weakref.ref(fs_obj)() is fs_obj)
print("ref gen:", weakref.ref(g_obj)() is g_obj)

# __slots__ without __weakref__ vs with __weakref__
s_no = SlottedNoWeakref(1)
try:
    weakref.ref(s_no)
    print("ERROR: slotted without __weakref__ should fail")
except TypeError as exc:
    print("slotted no weakref err:", str(exc))

s_yes = SlottedWithWeakref(2)
rs_yes = weakref.ref(s_yes)
print("slotted with weakref:", rs_yes() is s_yes, rs_yes().a)
del s_yes
print("slotted with weakref dead:", rs_yes() is None)

s_child = SlottedChildOfWeakref(3)
rs_child = weakref.ref(s_child)
print("slotted child inherits weakref:", rs_child() is s_child)

d_child = DictChildOfSlotted(4)
rd_child = weakref.ref(d_child)
print("dict child of slotted weakref:", rd_child() is d_child)


# ── 6. Subclassing weakref.ref ──────────────────────────────────────────────
class TaggedRef(weakref.ref):
    def __new__(cls, obj, cb=None, tag=None):
        self = super().__new__(cls, obj, cb)
        self.tag = tag
        return self

    def __init__(self, obj, cb=None, tag=None):
        super().__init__(obj, cb)


t_obj = Foo(55)
tr = TaggedRef(t_obj, tag="alpha")
print("subclass ref:", isinstance(tr, weakref.ref), tr() is t_obj, tr.tag)
del t_obj
print("subclass ref dead:", tr() is None, tr.tag)


# ── 7. weakref.proxy (ProxyType & CallableProxyType) ────────────────────────
class Box:
    def __init__(self, items):
        self.items = list(items)
        self.label = "box"

    def __len__(self):
        return len(self.items)

    def __getitem__(self, i):
        return self.items[i]

    def __setitem__(self, i, v):
        self.items[i] = v

    def __delitem__(self, i):
        del self.items[i]

    def __contains__(self, v):
        return v in self.items

    def __iter__(self):
        return iter(self.items)

    def __eq__(self, other):
        return isinstance(other, Box) and self.items == other.items

    def __bool__(self):
        return bool(self.items)

    def __str__(self):
        return f"Box({self.items})"


class Adder:
    def __init__(self, base):
        self.base = base

    def __call__(self, x):
        return self.base + x


box = Box([10, 20, 30])
p = weakref.proxy(box)
p_dup = weakref.proxy(box)
print("proxy type & dedup:", type(p) is weakref.ProxyType, p is p_dup)
print("proxy attr/len/item/str:", p.label, len(p), p[1], 20 in p, str(p), bool(p))
p[1] = 25
p.label = "updated"
print("proxy mutated:", box.items, box.label, list(p), p == Box([10, 25, 30]))
del p[0]
print("proxy delitem:", box.items)
print("proxy repr alive:", repr(p).startswith("<weakproxy at 0x"), " to Box at 0x" in repr(p))

# Proxy is unhashable
try:
    hash(p)
    print("ERROR: proxy should be unhashable")
except TypeError as exc:
    print("proxy hash err:", "unhashable" in str(exc))

# CallableProxyType
adder = Adder(100)
pc = weakref.proxy(adder)
print("callable proxy:", type(pc) is weakref.CallableProxyType, pc(23))

# Dead proxy raises ReferenceError
p_cb = []
box2 = Box([1])
p2 = weakref.proxy(box2, lambda prx: p_cb.append("proxy_dead"))
del box2
print("proxy cb fired:", p_cb)
print("proxy repr dead:", repr(p2).startswith("<weakproxy at 0x"), " to NoneType at 0x" in repr(p2))
try:
    _ = p2.label
    print("ERROR: expected ReferenceError")
except ReferenceError as exc:
    print("proxy dead getattr:", str(exc))
try:
    len(p2)
    print("ERROR: expected ReferenceError")
except ReferenceError as exc:
    print("proxy dead len:", str(exc))

del adder
try:
    pc(1)
    print("ERROR: expected ReferenceError")
except ReferenceError as exc:
    print("callable proxy dead:", str(exc))


# ── 8. WeakMethod ───────────────────────────────────────────────────────────
class Greeter:
    def __init__(self, prefix):
        self.prefix = prefix

    def greet(self, who):
        return f"{self.prefix} {who}"


wm_log = []
gr = Greeter("Hello")
wm = weakref.WeakMethod(gr.greet, lambda m: wm_log.append(m() is None))
print("WeakMethod alive:", wm()("world"), wm() == gr.greet)
del gr
print("WeakMethod dead:", wm() is None, wm_log)

try:
    weakref.WeakMethod(sample_fn)
    print("ERROR: expected TypeError for non-method")
except TypeError as exc:
    print("WeakMethod non-method err:", "bound method" in str(exc))


# ── 9. WeakKeyDictionary ────────────────────────────────────────────────────
wkd = weakref.WeakKeyDictionary()
k1 = Foo(1)
k2 = Foo(2)
wkd[k1] = "one"
wkd[k2] = "two"
print("wkd alive:", len(wkd), wkd[k1], wkd.get(k2), k1 in wkd, 42 in wkd)
print("wkd values:", sorted(wkd.values()))
del k1
print("wkd after del k1:", len(wkd), list(wkd.values()), k2 in wkd)
print("wkd pop:", wkd.pop(k2), len(wkd))


# ── 10. WeakValueDictionary ─────────────────────────────────────────────────
wvd = weakref.WeakValueDictionary()
v1 = Foo(101)
v2 = Foo(202)
wvd["a"] = v1
wvd["b"] = v2
print("wvd alive:", len(wvd), wvd["a"] is v1, "b" in wvd, wvd.get("missing", -1))
del v1
print("wvd after del v1:", len(wvd), "a" in wvd, list(wvd.keys()), [v.x for v in wvd.values()])
wvd_copy = wvd.copy()
print("wvd copy:", list(wvd_copy.keys()), wvd_copy["b"] is v2)
del v2
print("wvd empty:", len(wvd), len(wvd_copy))


# ── 11. WeakSet ─────────────────────────────────────────────────────────────
ws = weakref.WeakSet()
o1 = Foo(1)
o2 = Foo(2)
o3 = Foo(3)
ws.add(o1)
ws.add(o2)
print("ws alive:", len(ws), o1 in ws, o3 in ws, 99 in ws)
del o1
print("ws after del o1:", len(ws), [o.x for o in ws])
ws.update([o3])
print("ws union len:", len(ws | weakref.WeakSet([o2])), len(ws & weakref.WeakSet([o2])))
ws.discard(o2)
print("ws after discard:", len(ws), [o.x for o in ws])


# ── 12. finalize ────────────────────────────────────────────────────────────
fin_log = []


def on_finalize(tag, extra=0):
    fin_log.append((tag, extra))
    return f"done:{tag}:{extra}"


# Trigger on del
f_obj1 = Foo(1)
fin1 = weakref.finalize(f_obj1, on_finalize, "auto", extra=10)
peek1 = fin1.peek()
print("fin1 alive & peek:", fin1.alive, fin1.atexit, peek1[0] is f_obj1, peek1[1] is on_finalize, peek1[2], peek1[3])
del peek1
del f_obj1
print("fin1 after del:", fin1.alive, fin1.peek(), fin_log)
print("fin1 second call:", fin1())

# Trigger manually via __call__ (idempotent, does not fire again on del)
f_obj2 = Foo(2)
fin2 = weakref.finalize(f_obj2, on_finalize, "manual", extra=20)
print("fin2 manual call:", fin2(), fin2.alive, fin2())
del f_obj2
print("fin2 log unchanged after del:", fin_log)

# detach cancels finalizer
f_obj3 = Foo(3)
fin3 = weakref.finalize(f_obj3, on_finalize, "detached", extra=30)
fin3.atexit = False
print("fin3 atexit:", fin3.atexit)
detached = fin3.detach()
print("fin3 detach:", detached[0] is f_obj3, detached[2], detached[3], fin3.alive, fin3.detach())
del f_obj3
print("fin3 log after del:", fin_log)

# Anonymous finalize (return value discarded) still triggers on del!
f_obj4 = Foo(4)
weakref.finalize(f_obj4, on_finalize, "anon", extra=40)
del f_obj4
print("fin4 anon triggered:", fin_log)
