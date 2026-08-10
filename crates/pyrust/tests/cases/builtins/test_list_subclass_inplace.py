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


# --- explicit inherited in-place dunders mutate and return the subclass ---
class SSub(set):
    pass


class DSub(dict):
    pass


class BSub(bytearray):
    pass


p = LSub([1])
r = p.__iadd__([2])
print("list bound iadd", r is p, type(r).__name__, list(p))

p = LSub([1, 2])
r = p.__imul__(2)
print("list bound imul", r is p, type(r).__name__, list(p))

p = LSub([1])
r = list.__iadd__(p, [2])
print("list unbound iadd", r is p, type(r).__name__, list(p))

p = LSub([1, 2])
r = list.__imul__(p, 2)
print("list unbound imul", r is p, type(r).__name__, list(p))

p = LSub([1])
method = p.__iadd__
r = method([2])
print("list captured bound iadd", r is p, type(r).__name__, list(p))


def cached_iadd(receiver):
    return receiver.__iadd__([2])


p = LSub([1])
cached_iadd(p)
p = LSub([1])
r = cached_iadd(p)
print("list cached bound iadd", r is p, type(r).__name__, list(p))


class LSuper(list):
    def inherited_iadd(self, other):
        return super().__iadd__(other)


p = LSuper([1])
r = p.inherited_iadd([2])
print("list super iadd", r is p, type(r).__name__, list(p))

p = SSub({1, 2})
r = p.__ior__({2, 3})
print("set bound ior", r is p, type(r).__name__, sorted(p))

p = SSub({1, 2})
r = set.__ior__(p, {2, 3})
print("set unbound ior", r is p, type(r).__name__, sorted(p))

p = SSub({1, 2})
r = p.__iand__({2, 3})
print("set bound iand", r is p, type(r).__name__, sorted(p))

p = SSub({1, 2})
r = set.__iand__(p, {2, 3})
print("set unbound iand", r is p, type(r).__name__, sorted(p))

p = SSub({1, 2})
r = p.__isub__({2})
print("set bound isub", r is p, type(r).__name__, sorted(p))

p = SSub({1, 2})
r = set.__isub__(p, {2})
print("set unbound isub", r is p, type(r).__name__, sorted(p))

p = SSub({1, 2})
r = p.__ixor__({2, 3})
print("set bound ixor", r is p, type(r).__name__, sorted(p))

p = SSub({1, 2})
r = set.__ixor__(p, {2, 3})
print("set unbound ixor", r is p, type(r).__name__, sorted(p))

p = DSub({"a": 1})
r = p.__ior__({"b": 2})
print("dict bound ior", r is p, type(r).__name__, sorted(p.items()))

p = DSub({"a": 1})
r = dict.__ior__(p, {"b": 2})
print("dict unbound ior", r is p, type(r).__name__, sorted(p.items()))

p = BSub(b"a")
r = p.__iadd__(b"b")
print("bytearray bound iadd", r is p, type(r).__name__, list(p))

p = BSub(b"ab")
r = p.__imul__(2)
print("bytearray bound imul", r is p, type(r).__name__, list(p))

p = BSub(b"a")
r = bytearray.__iadd__(p, b"b")
print("bytearray unbound iadd", r is p, type(r).__name__, list(p))

p = BSub(b"ab")
r = bytearray.__imul__(p, 2)
print("bytearray unbound imul", r is p, type(r).__name__, list(p))


# --- explicit-call controls stay distinct from inherited in-place wrappers ---
class ExplicitOverride(list):
    def __iadd__(self, other):
        return "override-result"


p = ExplicitOverride([1])
r = p.__iadd__([2])
print("explicit override", r, list(p), type(p).__name__)


class ExplicitNotImplemented(list):
    def __iadd__(self, other):
        return NotImplemented


p = ExplicitNotImplemented([1])
r = p.__iadd__([2])
print("explicit NotImplemented", r is NotImplemented, list(p), type(p).__name__)

p = LSub([1])
r = p.__add__([2])
print("binary bound", r is p, type(r).__name__, r, list(p))

p = LSub([1])
r = list.__add__(p, [2])
print("binary unbound", r is p, type(r).__name__, r, list(p))

for owner, method, receiver, rhs in [
    (list, "__iadd__", DSub(), []),
    (set, "__ior__", DSub(), set()),
    (dict, "__ior__", LSub(), {}),
    (bytearray, "__iadd__", LSub(), b""),
]:
    try:
        getattr(owner, method)(receiver, rhs)
    except TypeError as e:
        print("invalid receiver", owner.__name__, method, type(e).__name__)

p = [1]
r = list.__iadd__(p, [2])
print("exact list", r is p, type(r).__name__, p)

p = {1}
r = set.__ior__(p, {2})
print("exact set", r is p, type(r).__name__, sorted(p))

p = {"a": 1}
r = dict.__ior__(p, {"b": 2})
print("exact dict", r is p, type(r).__name__, sorted(p.items()))

p = bytearray(b"a")
r = bytearray.__iadd__(p, b"b")
print("exact bytearray", r is p, type(r).__name__, list(p))


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
