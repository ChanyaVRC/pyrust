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
# so every check below prints a relation, never a raw implementation-specific
# id.

import sys


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
dict_a = {"key": "value"}


class Obj:
    pass


inst = Obj()
inst_alias = inst
other_inst = Obj()
inst_dict_a = vars(inst)
inst_dict_b = vars(inst)
other_inst_dict = vars(other_inst)
small_range = range(10)
small_range_alias = small_range
equal_small_range = range(10)
big_range = range(10**30)
big_range_alias = big_range
equal_big_range = range(10**30)

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
    (small_range, small_range_alias),
    (small_range, equal_small_range),
    (big_range, big_range_alias),
    (big_range, equal_big_range),
    (inst_dict_a, inst_dict_b),
    (inst_dict_a, other_inst_dict),
    (inst_dict_a, inst),
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
print(
    "range-identity",
    id(small_range) == id(small_range_alias),
    id(small_range) != id(equal_small_range),
    id(big_range) == id(big_range_alias),
    id(big_range) != id(equal_big_range),
)
print(
    "instance-dict-identity",
    id(inst_dict_a) == id(inst_dict_b),
    id(inst_dict_a) != id(other_inst_dict),
    id(inst_dict_a) != id(inst),
)

# Stability: the same object keeps its id.
print("stable", id(f1) == id(f1), id(nan1) == id(nan1), id(c1) == id(c1), id(inst) == id(inst))

# id() is a non-negative int, and is usable as a dict key.
objs = [
    nan1,
    nan2,
    f1,
    f2,
    zero,
    neg_zero,
    c1,
    c2,
    0,
    1,
    True,
    False,
    None,
    ...,
    NotImplemented,
    lst_a,
    lst_b,
    dict_a,
    inst,
    inst_dict_a,
    other_inst_dict,
    small_range,
    equal_small_range,
    big_range,
    equal_big_range,
    "abc",
    len,
]
print("int-type", all(type(id(o)) is int for o in objs))
print("non-negative", all(id(o) >= 0 for o in objs))
# This row spans inline primitives/string/float, a counter-backed list, an
# allocation-backed dict and user object, plus built-in function/object forms.
print("bounded", all(id(o) <= sys.maxsize for o in objs))
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

# Mixing the float bits with a bijective u64 finalizer still leaves every u64
# reachable.  This is the exact finite float obtained by inverting the old
# finalizer for `id(None)`; the two typed identities must receive distinct ids.
old_fmix_collision = float.fromhex("-0x1.77077a6d0dbbbp+947")
print(
    "fmix-inverse-vs-none",
    old_fmix_collision is not None,
    id(old_fmix_collision) != id(None),
    consistent(old_fmix_collision, None),
)

# The old complex id folded 128 component bits into 64.  These two component
# pairs produced the same fold even though `is` compares them as distinct.
old_fold_a = complex(0.0, 0.0)
old_fold_b = complex(1.0, float.fromhex("0x1.1ebef62fc0279p-625"))
print(
    "complex-fold-distinct",
    old_fold_a is not old_fold_b,
    id(old_fold_a) != id(old_fold_b),
    consistent(old_fold_a, old_fold_b),
)

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
keep.extend(
    [
        None,
        ...,
        NotImplemented,
        True,
        False,
        float("nan"),
        float("nan"),
        0.0,
        -0.0,
        old_fmix_collision,
        old_fold_a,
        old_fold_b,
        inst,
        inst_dict_a,
        other_inst,
        other_inst_dict,
        small_range,
        equal_small_range,
        big_range,
        equal_big_range,
    ]
)
print("sweep-size", len(keep))
print("sweep-distinct-ids", len({id(o) for o in keep}) == len(keep))
print("sweep-consistent", all(consistent(keep[i], keep[i + 1]) for i in range(len(keep) - 1)))
