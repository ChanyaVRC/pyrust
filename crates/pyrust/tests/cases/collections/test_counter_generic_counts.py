from collections import Counter


events = []


def value_of(other):
    if hasattr(other, "value"):
        return other.value
    return other


def name_of(other):
    if hasattr(other, "name"):
        return other.name
    return str(other)


class Count:
    def __init__(self, value, name):
        self.value = value
        self.name = name

    def __repr__(self):
        return f"Count({self.value})"

    def __add__(self, other):
        events.append(f"add:{self.name}:{name_of(other)}")
        return Count(self.value + value_of(other), f"({self.name}+{name_of(other)})")

    def __radd__(self, other):
        events.append(f"radd:{self.name}:{name_of(other)}")
        return Count(value_of(other) + self.value, f"({name_of(other)}+{self.name})")

    def __sub__(self, other):
        events.append(f"sub:{self.name}:{name_of(other)}")
        return Count(self.value - value_of(other), f"({self.name}-{name_of(other)})")

    def __rsub__(self, other):
        events.append(f"rsub:{self.name}:{name_of(other)}")
        return Count(value_of(other) - self.value, f"({name_of(other)}-{self.name})")

    def __eq__(self, other):
        events.append(f"eq:{self.name}:{name_of(other)}")
        return self.value == value_of(other)

    def __lt__(self, other):
        events.append(f"lt:{self.name}:{name_of(other)}")
        return self.value < value_of(other)

    def __le__(self, other):
        events.append(f"le:{self.name}:{name_of(other)}")
        return self.value <= value_of(other)

    def __gt__(self, other):
        events.append(f"gt:{self.name}:{name_of(other)}")
        return self.value > value_of(other)

    def __ge__(self, other):
        events.append(f"ge:{self.name}:{name_of(other)}")
        return self.value >= value_of(other)


# Mapping update and subtract have intentionally different operand order.
c = Counter()
c["x"] = Count(10, "current")
events.clear()
c.update({"x": Count(3, "incoming")})
print("update-order", events, c["x"].value)

c["x"] = Count(10, "current")
events.clear()
c.subtract({"x": Count(3, "incoming")})
print("subtract-order", events, c["x"].value)

# total() is Python sum(self.values()), including its left-to-right protocol.
c = Counter()
c["a"] = Count(2, "a")
c["b"] = Count(3, "b")
events.clear()
total = c.total()
print("total-generic", events, total.value)

# Arbitrary precision and floats must not be narrowed to i64.
huge = 2 ** 100
c = Counter({"huge": huge, "one": 1})
print("big-total", c.total() == huge + 1)
c.update({"huge": huge})
print("big-update", c["huge"] == huge * 2)
c.subtract({"huge": huge * 3})
print("big-subtract", c["huge"] == -huge)

c = Counter({"x": 1.5, "y": 2})
print("float-total", c.total())
print("float-add", (c + Counter({"x": 0.5}))["x"])
print("float-and", (c & Counter({"x": 2.0}))["x"])

# Binary operators evaluate the count protocol and the positive filter.
left = Counter()
left["x"] = Count(4, "left")
right = Counter()
right["x"] = Count(3, "right")
events.clear()
added = left + right
print("generic-add", events, added["x"].value)

events.clear()
intersected = left & right
print("generic-and", events, intersected["x"].value)

# Missing entries compare as zero; unary operations return a base Counter.
missing = Counter()
missing["x"] = Count(-2, "negative")
events.clear()
print("missing-le", missing <= Counter(), events)

events.clear()
positive = +Counter({"a": Count(2, "positive"), "z": 0})
print("unary-plus", positive["a"].value, "z" in positive, events)

events.clear()
negative = -Counter({"a": Count(-2, "negative"), "z": 0})
print("unary-minus", negative["a"].value, "z" in negative, events)


class CounterChild(Counter):
    pass


print("unary-base", type(+CounterChild(a=1)) is Counter)
print("binary-base", type(CounterChild(a=1) + CounterChild(b=1)) is Counter)

left_plain = Counter(a=1)
same_with_zero = Counter(a=1, zero=0)
print(
    "missing-comparisons",
    left_plain == same_with_zero,
    left_plain != same_with_zero,
    left_plain <= same_with_zero,
    left_plain < same_with_zero,
    left_plain >= same_with_zero,
    left_plain > same_with_zero,
)
print(
    "ordered-comparisons",
    Counter(a=1) < Counter(a=2),
    Counter(a=2) > Counter(a=1),
)
print(
    "dict-equality-fallback",
    Counter() == {},
    Counter(a=1) == {"a": 1},
    Counter(a=0) == {},
)

# In-place operators follow their Python-level mapping protocol and also work
# with a plain dict RHS.
c = Counter(a=2)
c += {"a": 1, "b": 2}
print("inplace-dict", sorted(c.items()))
c += c
print("inplace-self", sorted(c.items()))

# In-place operations commit completed arithmetic before a later failure.
class AddBoom:
    def __radd__(self, other):
        raise RuntimeError("add boom")


c = Counter({"a": 1})
rhs = Counter()
rhs["a"] = 2
rhs["bad"] = AddBoom()
try:
    c += rhs
except RuntimeError:
    print("inplace-partial", c["a"], "bad" in c)


# _keep_positive computes the full deletion list before deleting any key.
class CompareBoom:
    def __gt__(self, other):
        raise RuntimeError("compare boom")


c = Counter()
c["zero"] = 0
c["bad"] = CompareBoom()
try:
    c += Counter({"x": 1})
except RuntimeError:
    print("cleanup-atomic", "zero" in c, c["x"])

# Iterator creation/early break is lazy; value changes are allowed, size
# changes are diagnosed on the next advance.
c = Counter({i: i for i in range(200)})
it = iter(c)
print("iter-first", next(it))
c[1] = 999
print("iter-value-change", next(it))
c[300] = 1
try:
    next(it)
except RuntimeError as error:
    print("iter-size-change", str(error))

# repr reuses most_common ordering, falls back only for TypeError, and lets
# unrelated exceptions escape.
print("repr-typeerror", repr(Counter({"first": "x", "second": 1})))


class ReprBoom:
    def __lt__(self, other):
        raise ValueError("repr comparison")


c = Counter()
c["a"] = ReprBoom()
c["b"] = ReprBoom()
try:
    repr(c)
except ValueError as error:
    print("repr-valueerror", str(error))

# Counter-specific dict overrides.
c = Counter()
del c["missing"]
print("del-missing", c["missing"])
try:
    Counter.fromkeys("ab")
except NotImplementedError as error:
    print("fromkeys", str(error))
