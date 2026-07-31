# `id()` must agree with `is` (#2956).
#
# pyrust stores floats (and complex) as NaN-boxed immediates, so the exact
# integer `id()` returns is an implementation detail that cannot match
# CPython's addresses.  What both implementations must agree on is the
# *relation* between `is` and `id`:
#
#   a is b      =>  id(a) == id(b)
#   a is not b  =>  id(a) != id(b)     (while both objects are alive)
#
# so every check below prints a relation, never a raw id.


def consistent(a, b):
    return (a is b) == (id(a) == id(b))


nan1 = float("nan")
nan2 = float("nan")
f1 = 1.5
f2 = 2.5
f1_alias = f1
zero = 0.0
neg_zero = -0.0
c1 = complex(1.0, 2.0)
c2 = complex(3.0, 4.0)
lst_a = [1]
lst_b = [2]
lst_alias = lst_a


class Obj:
    pass


inst = Obj()
inst_alias = inst

pairs = [
    (nan1, nan1),
    (nan1, nan2),
    (f1, f1),
    (f1, f1_alias),
    (f1, f2),
    (zero, neg_zero),
    (zero, 0),
    (zero, False),
    (c1, c1),
    (c1, c2),
    (c1, 1.0),
    (1, 1.0),
    (1, True),
    (0, False),
    (None, 0),
    (None, zero),
    (None, ...),
    (..., NotImplemented),
    (lst_a, lst_alias),
    (lst_a, lst_b),
    (inst, inst_alias),
    (inst, lst_a),
    ("abc", "abc"),
    (b"xy", b"xy"),
]
print("consistent", all(consistent(a, b) for a, b in pairs))

# Distinct live objects get distinct ids.  Every pair below is `is`-distinct
# under CPython *and* pyrust.
print("nan-distinct", id(nan1) != id(nan2))
print("float-distinct", id(f1) != id(f2))
print("signed-zero-distinct", id(zero) != id(neg_zero))
print("complex-distinct", id(c1) != id(c2))
print("cross-type-distinct", len({id(o) for o in (0, 0.0, False, None, ..., NotImplemented)}) == 6)
print("int-float-distinct", id(1) != id(1.0), id(1) != id(True))
print("heap-distinct", id(lst_a) != id(lst_b), id(inst) != id(lst_a))

# Stability: the same object keeps its id.
print("stable", id(f1) == id(f1), id(nan1) == id(nan1), id(c1) == id(c1), id(inst) == id(inst))

# id() is a non-negative int, and is usable as a dict key.
objs = [nan1, nan2, f1, f2, zero, neg_zero, c1, c2, 0, 1, True, False, None, ..., NotImplemented, lst_a, lst_b, inst, "abc"]
print("int-type", all(type(id(o)) is int for o in objs))
print("non-negative", all(id(o) >= 0 for o in objs))
print("id-keyed", len({id(o): None for o in objs}) == len(objs))

# A direct one-argument `id(x)` and an `id` reached through a variable, a
# `map()` or a module attribute take different call paths through the
# interpreter; they must not disagree.
import builtins

alias = id
by_attr = getattr(builtins, "id")
direct = [id(o) for o in objs]
print(
    "paths-agree",
    list(map(id, objs)) == [alias(o) for o in objs] == [by_attr(o) for o in objs] == direct,
)

# A float carries no type tag — its box *is* the double — so an id taken from
# its raw bit pattern shares a number line with every heap object's id.  The
# smallest subnormal is `1` in bits and `5e-324 * n` is exactly the subnormal
# whose bits are `n`, so this reached a live object's id in one step.
heap_objs = [{}, [1], (1, 2, 3), {1}, b"xy", Obj(), "a long string kept off the inline path"]
print("subnormal-vs-heap", all(id(5e-324 * id(o)) != id(o) for o in heap_objs))
print("subnormal-vs-first-alloc", all(id(5e-324 * k) not in {id(o) for o in heap_objs} for k in range(1, 64)))

# Systematic sweep: many simultaneously-live distinct objects, all ids
# distinct.  `keep` holds every object alive, so no id can be recycled.
keep = []
for k in range(1, 200):
    keep.append(5e-324 * k)  # subnormal floats: raw bits 1..199
    keep.append(float(k))
    keep.append(-float(k))
    keep.append(complex(k, 0.0))
    keep.append(complex(0.0, k))
    keep.append(k)
    keep.append(-k)
    keep.append(k * 10**30)
    keep.append(str(k))
    keep.append("pad" * 12 + str(k))
    keep.append([k])
    keep.append((k,))
    keep.append({k})
    keep.append({k: k})
    keep.append(bytes([k % 256]))
    keep.append(Obj())
keep.extend([None, ..., NotImplemented, True, False, float("nan"), float("nan"), 0.0, -0.0])
print("sweep-size", len(keep))
print("sweep-distinct-ids", len({id(o) for o in keep}) == len(keep))
print("sweep-consistent", all(consistent(keep[i], keep[i + 1]) for i in range(len(keep) - 1)))
