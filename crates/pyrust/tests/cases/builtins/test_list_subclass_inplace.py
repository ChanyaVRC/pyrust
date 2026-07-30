# Issue #2986: `+=` / `*=` on a `list` subclass must mutate the shared backing
# and return the SAME object, exactly as the inherited `list.__iadd__` /
# `list.__imul__` do — not rebind the target to a fresh plain `list`, which
# silently drops every alias and the subclass type.


class LSub(list):
    pass


# --- the reported repro: aliases see the update, identity is preserved ---
p = LSub([1])
q = p
p += [9]
print(p, q, p is q, type(p).__name__)

p = LSub([1, 2])
q = p
p *= 3
print(p, q, p is q, type(p).__name__)


# --- `+=` accepts any iterable, like list.extend ---
for rhs in [(2, 3), "ab", range(2), {"k": 0}, {5}, frozenset({7}), b"\x01", map(str, [8])]:
    p = LSub([1])
    q = p
    p += rhs
    print(p, p is q, type(p).__name__)


p = LSub([1])
q = p
p += (i for i in range(2))
print(p, p is q, type(p).__name__)


# --- `*=` boundary counts ---
for n in [3, 1, 0, -1, True, False]:
    p = LSub([1, 2])
    q = p
    p *= n
    print(n, p, p is q, type(p).__name__)


# --- `*=` resolves the count through __index__, still in place ---
class Idx:
    def __index__(self):
        return 2


p = LSub([1])
q = p
p *= Idx()
print(p, p is q, type(p).__name__)


# --- empty operands ---
p = LSub([])
q = p
p += []
print(p, p is q, type(p).__name__)

p = LSub([1])
q = p
p += []
print(p, p is q, type(p).__name__)


# --- self-concatenation reads a snapshot, never a live iterator ---
p = LSub([1, 2])
q = p
p += p
print(p, p is q, type(p).__name__)


# --- subclass state survives the in-place update ---
class LState(list):
    def __init__(self, items, tag):
        super().__init__(items)
        self.tag = tag


p = LState([1], "keep")
q = p
p += [2]
print(list(p), p.tag, p is q, type(p).__name__)


# --- non-name targets are updated in place too ---
class Holder:
    pass


h = Holder()
h.v = LSub([1])
alias = h.v
h.v += [2]
print(h.v, h.v is alias, type(h.v).__name__)

box = [LSub([1])]
alias = box[0]
box[0] += [2]
print(box[0], box[0] is alias, type(box[0]).__name__)

d = {"k": LSub([1])}
alias = d["k"]
d["k"] += [2]
print(d["k"], d["k"] is alias, type(d["k"]).__name__)


# --- multi-level subclass ---
class L2(LSub):
    pass


p = L2([1])
q = p
p += [2]
print(p, p is q, type(p).__name__)


# --- a user __iadd__ / __imul__ override still wins ---
class LIAdd(list):
    def __iadd__(self, other):
        self.append("iadd")
        return self


p = LIAdd([1])
q = p
p += [9]
print(p, p is q, type(p).__name__)


class LIMul(list):
    def __imul__(self, n):
        self.append(("imul", n))
        return self


p = LIMul([1])
q = p
p *= 3
print(p, p is q, type(p).__name__)


# --- defining only __add__ does NOT displace the inherited __iadd__ ---
class LAdd(list):
    def __add__(self, other):
        return "add-called"


p = LAdd([1])
q = p
p += [9]
print(p, p is q, type(p).__name__)
print(LAdd([1]) + [9])


# --- an override returning NotImplemented falls back to plain binary `+`,
#     which builds a NEW plain list (identity and subclass type are dropped) ---
class LNI(list):
    def __iadd__(self, other):
        return NotImplemented


p = LNI([1])
q = p
p += [9]
print(p, q, p is q, type(p).__name__)


# --- CPython's C-level extend ignores a subclass `extend` override ---
class LExt(list):
    def extend(self, other):
        raise AssertionError("list.__iadd__ must not call the Python-level extend")


p = LExt([1])
q = p
p += [9]
print(p, p is q, type(p).__name__)


# --- ...but it does honour a subclass __iter__ when reading the RHS ---
class LIter(list):
    def __iter__(self):
        return iter(["override"])


p = LSub([0])
q = p
p += LIter([1, 2])
print(p, p is q, type(p).__name__)

p = LIter([0])
q = p
p += [1]
print(list.__repr__(p), p is q, type(p).__name__)

p = LIter([1, 2])
q = p
p += p
print(list.__repr__(p), p is q, type(p).__name__)


# --- a lazy RHS that raises leaves the receiver bound to the same object ---
def bad():
    yield 1
    raise ValueError("boom")


p = LSub([0])
q = p
try:
    p += bad()
except ValueError as e:
    print("raised", e)
print(p is q, type(p).__name__)


# --- error paths keep CPython's exception class and wording ---
for rhs in [5, None, object()]:
    p = LSub([1])
    try:
        p += rhs
    except TypeError as e:
        print(type(e).__name__, e)

p = LSub([1])
try:
    p *= 10**30
except OverflowError as e:
    print(type(e).__name__, e)

# A count that fits an index-sized integer but cannot be allocated raises
# MemoryError -- the repeat must range-check before copying rather than let the
# allocator abort the process.
p = LSub([1])
q = p
try:
    p *= 2**62
except MemoryError as e:
    print(type(e).__name__, p is q, type(p).__name__)

p = [1]
try:
    p *= 2**62
except MemoryError as e:
    print(type(e).__name__, p)


# --- plain lists keep their existing behaviour ---
p = [1]
q = p
p += [2]
print(p, p is q)

p = [1, 2]
q = p
p *= 2
print(p, p is q)

p = [1, 2]
q = p
p += p
print(p, p is q)
