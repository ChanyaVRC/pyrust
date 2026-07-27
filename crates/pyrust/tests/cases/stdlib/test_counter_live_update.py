# Counter.update/subtract mutate their live dict backing one source item at a
# time. Cover partial failures, self-aliasing, and user-key hash/equality.

from collections import Counter


class UpdateBoom(Exception):
    pass


def raising_elements():
    yield "x"
    yield "y"
    raise UpdateBoom("iteration")


c = Counter(a=1)
try:
    c.update(raising_elements())
except UpdateBoom:
    print("iter-partial", sorted(c.items()))


c = Counter(z=1)
try:
    c.update({"a": 2, "bad": "not-a-count"})
except TypeError:
    print("mapping-partial", sorted(c.items()))


c = Counter(a=2, b=3)
c.update(c)
print("self-update", sorted(c.items()))
c.subtract(c)
print("self-subtract", sorted(c.items()))


# An empty Counter delegates mapping update to dict.update and copies arbitrary
# values verbatim instead of attempting arithmetic.
c = Counter()
c.update({"raw": "value"})
print("empty-preserve", c["raw"])


events = []


class Stored:
    def __init__(self, name, hash_value):
        self.name = name
        self.hash_value = hash_value

    def __hash__(self):
        events.append("stored-hash")
        return self.hash_value

    def __eq__(self, other):
        events.append(f"{self.name}-eq")
        if other.name == "boom":
            raise UpdateBoom("equality")
        return self.hash_value == other.hash_value


class Probe:
    def __init__(self, name, hash_value):
        self.name = name
        self.hash_value = hash_value

    def __hash__(self):
        events.append(f"{self.name}-hash")
        return self.hash_value

    def __eq__(self, other):
        events.append(f"{self.name}-probe-eq")
        return False


first = Stored("first", 11)
second = Stored("second", 22)
c = Counter({first: 10, second: 20})
source = {Probe("match", 11): 3, Probe("boom", 22): 4}
events.clear()
try:
    c.update(source)
except UpdateBoom:
    print(
        "eq-partial",
        c[first],
        c[second],
        next(key for key in c if key.name == "first") is first,
    )
print("dispatch", events)
