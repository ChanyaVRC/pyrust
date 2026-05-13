class H:
    def __init__(self, v):
        self.v = v
    def __hash__(self):
        return hash(self.v)
    def __eq__(self, other):
        return isinstance(other, H) and self.v == other.v


# Distinct instances that compare equal should collapse in dict/set.
d = {H(1): "a"}
print(d.get(H(1)))                  # "a"
print(len({H(1), H(1)}))            # 1
print(H(1) in {H(1)})               # True
print({H(1): 1, H(2): 2}[H(1)])     # 1

# `in` on a dict with a custom-hashable key.
print(H(1) in d)                    # True
print(H(2) in d)                    # False

# dict.pop and dict.setdefault must also honor __eq__.
e = {H(1): "x"}
print(e.pop(H(1)))                  # x
print(len(e))                       # 0

f = {}
print(f.setdefault(H(7), "default"))  # default
print(f.setdefault(H(7), "other"))    # default — already present
print(len(f))                          # 1

# set.add / discard / remove via __eq__.
s = set()
s.add(H(3))
s.add(H(3))
print(len(s))                       # 1
s.discard(H(3))
print(len(s))                       # 0

# Mixing primitive and instance keys in the same dict.
m = {H(1): "h", 1: "primitive", "k": "string"}
print(m[H(1)])                      # h
print(m[1])                         # primitive
print(m["k"])                       # string

# Without __hash__, default object hash (identity) is used:
# distinct instances are distinct keys.
class Plain:
    pass

a = Plain()
b = Plain()
g = {a: "first"}
print(g.get(a))                     # first
print(g.get(b))                     # None — different identity

# pop() on a missing custom-key raises KeyError (not RuntimeError).
class H:
    def __init__(self, v): self.v = v
    def __hash__(self): return hash(self.v)
    def __eq__(self, other): return isinstance(other, H) and self.v == other.v

d = {H(1): "a"}
try:
    d.pop(H(2))
    print("pop-missing-custom", "FAIL")
except KeyError:
    print("pop-missing-custom", "KeyError")

# pop with default — no exception.
print("pop-default-custom", d.pop(H(2), "fallback"))

# Same fix should cover the plain-key case for symmetry — verify
# pop() on a missing primitive key also raises KeyError.
try:
    {1: "a"}.pop(99)
    print("pop-missing-plain", "FAIL")
except KeyError:
    print("pop-missing-plain", "KeyError")
