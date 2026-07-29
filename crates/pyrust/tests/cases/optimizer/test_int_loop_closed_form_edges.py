# The closed-form fold (PR #2902, issue #2886) replaces a constant-bound
# counted loop with `acc + delta` inside a fourth out-of-line copy, entered only
# behind `JumpIfIterNotIntRangeExact` — a guard that reads the *live* cursor and
# checks its exact `(start, stop, step)` plus an unmoved position.
#
# `test_int_loop_closed_form.py` pins the fold's arithmetic.  This file pins the
# edges around it: the shapes that must decline the fold and still be exact, the
# control flow that has to survive a body collapsing to two instructions, and
# the module namespace afterwards.

import builtins

namespace = globals()
native_range = range


def state(*names):
    return [(name, namespace[name]) for name in names if name in namespace]


# ── Negative steps, including the descending unit step ────────────────────────
unit_down = 0
for ud in range(10, 0, -1):
    unit_down += ud
print("unit down", unit_down, ud)

both_negative = 0
for bn in range(-3, -40, -4):
    both_negative += bn
print("both negative", both_negative, bn)

negative_span = 0
for ns in range(-9, -2):
    negative_span += 1
print("negative span", negative_span, ns)

down_one_trip = 0
for dot in range(7, 6, -1):
    down_one_trip += dot
print("down one trip", down_one_trip, dot)

print("down zero trip", [dzt for dzt in range(3, 9, -2)], "dzt" in namespace)
print("up zero trip", [uzt for uzt in range(9, 3, 2)], "uzt" in namespace)

# ── The accumulator promotes through the fold ────────────────────────────────
near_max = (1 << 63) - 6
for nm in range(11):
    near_max += 1
print("near max", near_max - (1 << 63), type(near_max).__name__, nm)

near_min = -(1 << 63) + 6
for nmin in range(11):
    near_min -= 1
print("near min", near_min + (1 << 63), type(near_min).__name__, nmin)

# A delta the fold itself cannot hold in a machine int: whether it folds or
# declines, the value must be the one the iterated adds reach.
wide_delta = 0
for wd in range(6):
    wide_delta += 1 << 62
print("wide delta", wide_delta == 6 * (1 << 62), type(wide_delta).__name__, wd)

wide_variable = 0
for wv in range((1 << 62) - 3, (1 << 62) + 3):
    wide_variable += wv
print("wide variable", wide_variable == sum(native_range((1 << 62) - 3, (1 << 62) + 3)), wv)

already_big = 1 << 70
for ab in range(100):
    already_big += 3
print("already big", already_big - (1 << 70), type(already_big).__name__, ab)

# ── `for … else` around a folded loop ────────────────────────────────────────
else_total = 0
for et in range(20):
    else_total += 2
else:
    print("else ran", else_total, et, namespace["else_total"])

zero_else = 0
for ze in range(0):
    zero_else += 1
else:
    print("zero-trip else ran", zero_else, "ze" in namespace)

# A `break` makes the body non-linear; the loop must run for real.
break_total = 0
for bt in range(50):
    break_total += 1
    if break_total == 7:
        break
else:
    print("unreachable")
print("break", break_total, bt)

# ── Shapes that must not fold, and must stay exact ───────────────────────────
# A range built into a variable first: the producing call is no longer adjacent
# to the header, so the argument trace has nothing to propose.
precomputed = range(12)
precomputed_total = 0
for pc in precomputed:
    precomputed_total += 1
print("precomputed", precomputed_total, pc)

# Reusing that same range object runs it again from the start.
precomputed_again = 0
for pc2 in precomputed:
    precomputed_again += 2
print("precomputed again", precomputed_again, pc2)

# A partly consumed cursor fails the "cursor still at start" half of the guard.
partial = iter(range(12))
print("partial first", next(partial), next(partial))
partial_total = 0
for pt in partial:
    partial_total += 1
print("partial", partial_total, pt)

# An exhausted cursor yields nothing and binds nothing new.
exhausted = iter(range(3))
print("drain", list(exhausted))
exhausted_total = 0
for ex in exhausted:
    exhausted_total += 1
print("exhausted", exhausted_total, "ex" in namespace)

# A register bound, rather than constant, is not a traced triple.
computed_stop = 5 + 4
computed_total = 0
for ct in range(computed_stop):
    computed_total += 3
print("computed stop", computed_total, ct)

# The loop variable rebound in the body is not a linear accumulation.
rebinding_total = 0
for rb in range(6):
    rebinding_total += rb
    rb = 100
print("rebinding", rebinding_total, rb)

# ── `range` shadowed by a lambda returning a genuine, differently-bounded range
# A guard that only pinned the *kind* of cursor would fold 1000 iterations here.
range = lambda n: builtins.range(5)
shadow_total = 0
for sh in range(1000):
    shadow_total += 1
print("lambda shadow", shadow_total, sh)

range = lambda n: builtins.range(n // 2, n, 3)
shadow_three = 0
for sh3 in range(20):
    shadow_three += sh3
print("lambda shadow three-arg", shadow_three, sh3)

# A shadow that is not a range at all takes the ordinary iterator path.
range = lambda n: [11, 22, 33]
shadow_list = 0
for sl in range(1000):
    shadow_list += sl
print("lambda shadow list", shadow_list, sl)

range = native_range
restored = 0
for rs in range(9):
    restored += 1
print("restored", restored, rs)

# ── Sequential folded loops sharing one loop-variable slot ───────────────────
slot_first = 0
for slot in range(10):
    slot_first += 1
print("slot first", slot_first, slot)

slot_second = 0
for slot in range(4, 25, 5):
    slot_second += slot
print("slot second", slot_second, slot)

slot_third = 0
for slot in range(0):
    slot_third += 1
print("slot third", slot_third, slot)

slot_fourth = 0
for slot in range(3, 0, -1):
    slot_fourth += slot
print("slot fourth", slot_fourth, slot)

# Two accumulators sharing the slot across a deopting middle loop.
mixed_a = 0
for shared_slot in range(7):
    mixed_a += 2
for shared_slot in [1, "two", 3]:
    mixed_a += 1
for shared_slot in range(5, 0, -2):
    mixed_a += shared_slot
print("shared slot", mixed_a, shared_slot)

# ── The module namespace after all of the above ──────────────────────────────
print("globals", state("ud", "nm", "et", "pc", "pt", "sh", "slot", "shared_slot"))
print("unbound", [name for name in ("dzt", "uzt", "ze", "ex") if name in namespace])
