# Per-site `CallMethod` cache for built-in receivers (#3379).
#
# Each helper below has exactly one `obj.method(...)` call site, executed many
# times so the site warms up, then re-executed with a different receiver shape
# or after a class mutation.  The cache must never change observable behavior:
# subclass overrides, `__getattribute__`, instance shadows, wrong arity, and
# exception propagation all match CPython 3.12.
from collections import Counter, OrderedDict, defaultdict, deque


def append_site(obj, x):
    return obj.append(x)


def get_site(obj, k):
    return obj.get(k)


def upper_site(obj):
    return obj.upper()


def pop_site(obj):
    return obj.pop()


def popleft_site(obj):
    return obj.popleft()


def add_site(obj, x):
    return obj.add(x)


def index_site(obj, x):
    return obj.index(x)


def warm(fn, make, *args, n=40):
    out = None
    for _ in range(n):
        out = fn(make(), *args)
    return out


# --- exact containers warm, then polymorphic receivers -----------------------
lst = []
for i in range(50):
    append_site(lst, i)
print("list warm", len(lst), lst[:3], lst[-1])

dq = deque()
for i in range(50):
    append_site(dq, i)
print("deque after list site", len(dq), dq[0], dq[-1])


class MyList(list):
    def append(self, x):
        super().append(x * 10)
        return "overridden"


ml = MyList()
print("subclass override at warm list site", append_site(ml, 3), list(ml))


class PlainSub(list):
    pass


ps = PlainSub()
print("plain subclass at warm site", append_site(ps, 7), list(ps), type(ps).__name__)


class UserAppender:
    def __init__(self):
        self.items = []

    def append(self, x):
        self.items.append(-x)
        return "user"


ua = UserAppender()
print("user class at warm site", append_site(ua, 4), ua.items)
print("list again after megamorphic", append_site(lst, 99), lst[-1])

# --- deque site warm, then subclass overrides and class mutation -------------
dq2 = deque()
for i in range(30):
    append_site(dq2, i)
    popleft_site(dq2)
append_site(dq2, "tail")
print("deque warm", list(dq2))


class MyDeque(deque):
    def popleft(self):
        return ("custom", super().popleft())


md = MyDeque([1, 2, 3])
print("deque subclass override", popleft_site(md), list(md))

md2 = MyDeque([4, 5])
for _ in range(2):
    print("deque subclass repeat", popleft_site(md2))


class Mutable(deque):
    pass


mut = Mutable(range(100))
for _ in range(20):
    popleft_site(mut)
Mutable.popleft = lambda self: "patched"
print("after class patch", popleft_site(mut))
del Mutable.popleft
print("after class unpatch", popleft_site(mut))


class Base(deque):
    pass


class Child(Base):
    pass


ch = Child(range(50))
for _ in range(20):
    popleft_site(ch)
Base.popleft = lambda self: "base-patched"
print("after ancestor patch", popleft_site(ch))
del Base.popleft
print("after ancestor unpatch", popleft_site(ch))


class Shadowable(deque):
    pass


sh = Shadowable(range(50))
for _ in range(20):
    popleft_site(sh)
sh.popleft = lambda: "instance-shadow"
print("instance shadow", popleft_site(sh))
del sh.popleft
print("shadow removed", popleft_site(sh))


class Tracing(deque):
    def __getattribute__(self, name):
        if name == "popleft":
            return lambda: "via __getattribute__"
        return super().__getattribute__(name)


tr = Tracing([1])
print("custom __getattribute__", popleft_site(tr))

# --- wrong arity and exceptions on warm sites --------------------------------
def append_noargs(obj):
    return obj.append()


def append_two(obj):
    return obj.append(1, 2)


# (`[].append()` underflow raises the wrong exception type in the list method
# body itself, independent of call-site caching, so only deque covers it.)
for make, fns in ((list, (append_two,)), (deque, (append_noargs, append_two))):
    for _ in range(30):
        append_site(make(), 0)
    for fn in fns:
        for _ in range(3):
            try:
                fn(make())
            except TypeError as e:
                # deque's arity wording is a separate pre-existing gap; the
                # cache must only preserve the exception type and the cold/warm
                # consistency, so print the message for list only.
                detail = e if make is list else "<message>"
                print(type(make()).__name__, fn.__name__, "TypeError:", detail)

empty_dq = deque()
for i in range(25):
    empty_dq.append(i)
    popleft_site(empty_dq)
for _ in range(3):
    try:
        popleft_site(empty_dq)
    except IndexError as e:
        print("empty deque:", e)

for _ in range(30):
    pop_site([1])
for _ in range(2):
    try:
        pop_site([])
    except IndexError as e:
        print("empty list:", e)

for _ in range(30):
    index_site([1, 2, 3], 2)
try:
    index_site([1, 2, 3], 9)
except ValueError as e:
    print("list.index:", e)
print("tuple at list index site", index_site((5, 6, 7), 7))
print("str at index site", index_site("hello", "l"))

# --- dict / defaultdict / OrderedDict / Counter ------------------------------
d = {1: "one"}
for _ in range(40):
    get_site(d, 1)
print("dict get", get_site(d, 1), get_site(d, 2))
dd = defaultdict(list)
print("defaultdict at dict get site", get_site(dd, "missing"), dict(dd))
od = OrderedDict(a=1)
print("OrderedDict at dict get site", get_site(od, "a"))
c = Counter("aab")
print("Counter at dict get site", get_site(c, "a"), get_site(c, "z"))


class MyDict(dict):
    def get(self, k, default=None):
        return ("mydict", k)


print("dict subclass override", get_site(MyDict(), 5))

groups = defaultdict(list)
for i in range(200):
    groups[i % 7].append(i)
print("defaultdict append", sorted((k, len(v)) for k, v in groups.items()))

cnt = Counter()
for w in "the quick brown the lazy the end".split():
    cnt.update([w])
print("counter", cnt.most_common(2))

# --- str / bytes / set / tuple -----------------------------------------------
for _ in range(40):
    upper_site("abc")
print("str upper", upper_site("hello"), upper_site(b"bytes"), upper_site(bytearray(b"ba")))


class MyStr(str):
    def upper(self):
        return "MyStr.upper"


print("str subclass override", upper_site(MyStr("x")))

s = set()
for i in range(40):
    add_site(s, i)
add_site(s, "k")
add_site(s, (1, 2))
add_site(s, 2.5)
print("set add", len(s), "k" in s, (1, 2) in s, 2.5 in s)
try:
    add_site(s, [1])
except TypeError as e:
    print("set unhashable:", e)


class MySet(set):
    def add(self, x):
        super().add(("wrapped", x))


ms = MySet()
add_site(ms, 1)
print("set subclass override", ms)

# --- dunder names stay on protocol dispatch ----------------------------------
def len_site(obj):
    return obj.__len__()


for _ in range(30):
    len_site([1, 2])
print("dunder site", len_site([1, 2, 3]), len_site(deque([1])), len_site("abcd"))

# --- native methods that run Python code keep tracebacks correct -------------
def gen():
    yield 1
    raise RuntimeError("boom in generator")


def extend_site(obj, it):
    return obj.extend(it)


for _ in range(30):
    extend_site(deque(), [1, 2])
try:
    extend_site(deque(), gen())
except RuntimeError as e:
    import traceback

    frames = traceback.extract_tb(e.__traceback__)
    print("traceback functions", [f.name for f in frames])
    print("traceback lines", [f.line for f in frames])


class Eq:
    def __eq__(self, other):
        raise ValueError("eq raised")


def count_site(obj, x):
    return obj.count(x)


for _ in range(30):
    count_site(deque([1, 2, 1]), 1)
print("deque count", count_site(deque([1, 2, 1]), 1))
try:
    count_site(deque([Eq()]), 1)
except ValueError as e:
    print("deque count propagates:", e)

# --- unbound method objects and bound-method values still work ---------------
bound = deque([1, 2]).popleft
print("bound value", bound())
print("unbound descriptor", deque.popleft(deque([9, 8])))
print("list unbound", list.append, list.append.__qualname__)
