# Parity fixture for the int-loop versioning pass's mid-loop side exits.
#
# A `for` over a canonical list/tuple and a canonical sequence subscript both
# produce values whose type is only known per iteration, so the specialized
# out-of-line copy guards them and side-exits into the *original* loop when a
# guard fails.  The side exit flushes every deferred SyncModuleGlobal first, so
# the live globals dict and any subsequent raise must be indistinguishable from
# the unversioned loop.

namespace = globals()

# ── for over a canonical list ─────────────────────────────────────────────────
list_total = 0
for list_item in [1, 2, 3, 4]:
    list_total += list_item
print(list_item, list_total)
print(namespace["list_item"], namespace["list_total"])

# Deopt on the very first element, then again in the middle: the loop must keep
# running on the original path and produce CPython's result.
mixed_seen = []
for mixed_item in ["a", 1, 2.5, 3, None, 4]:
    mixed_seen.append(repr(mixed_item))
print(mixed_seen)
print(namespace["mixed_item"])

# Ints, one non-int, then ints again: the loop re-enters the fast copy through
# the entry guards after the side exit, and the shared cursor must not skip or
# repeat an element.
resume_seen = []
resume_total = 0
for resume_item in [1, 2, "stop", 3, 4]:
    resume_seen.append(resume_item)
    resume_total += 1
print(resume_seen, resume_total)

# `bool` is not an exact int, so True/False side-exit like any other element.
bool_seen = []
for bool_item in [True, 1, False, 2]:
    bool_seen.append(bool_item)
print(bool_seen)

# Every iteration must be observable through the live namespace.
observed = []
for observed_item in [10, 20, 30]:
    observed_total = observed_item
    observed.append((namespace["observed_item"], namespace["observed_total"]))
print(observed)

# ── for over a canonical tuple ────────────────────────────────────────────────
tuple_total = 0
for tuple_item in (5, 6, 7):
    tuple_total += tuple_item
print(tuple_item, tuple_total)

# ── empty / single-element / zero-trip ────────────────────────────────────────
empty_ran = False
for empty_item in []:
    empty_ran = True
print(empty_ran, "empty_item" in namespace)

single_total = 0
for single_item in [42]:
    single_total += single_item
print(single_item, single_total)

# ── canonical sequence subscript ──────────────────────────────────────────────
subscript_source = [1, 2, 3, 4, 5]
subscript_total = 0
for subscript_index in range(5):
    subscript_total += subscript_source[subscript_index]
print(subscript_index, subscript_total)
print(namespace["subscript_index"], namespace["subscript_total"])

# A non-int element deopts at the subscript, not at the loop variable.
element_source = [1, "two", 3]
element_seen = []
for element_index in range(3):
    element_seen.append(element_source[element_index])
print(element_seen)

# A negative index is not a fast-path index and stays on the original path.
negative_source = [1, 2, 3]
negative_total = 0
for negative_index in range(3):
    negative_total += negative_source[-1]
print(negative_total)

# A dict subscript is not a canonical sequence read.
mapping = {0: 100, 1: 200, 2: 300}
mapping_total = 0
for mapping_index in range(3):
    mapping_total += mapping[mapping_index]
print(mapping_total)


# A list subclass overriding __getitem__ must keep running its Python code.
class Doubling(list):
    def __getitem__(self, index):
        return 2 * list.__getitem__(self, index)


doubling_source = Doubling([1, 2, 3])
doubling_total = 0
for doubling_index in range(3):
    doubling_total += doubling_source[doubling_index]
print(doubling_total)

# ── an IndexError raised mid-loop ─────────────────────────────────────────────
# The raise happens on the original path, so it reports the subscript's own
# line and the globals dict holds exactly what per-iteration synchronization
# would have published.  The expected line is recorded as an offset from a
# marker raise so the check survives edits elsewhere in this file.
try:
    raise RuntimeError("line marker")
except RuntimeError as marker:
    marker_line = marker.__traceback__.tb_lineno

raise_source = [1, 2, 3]
raise_index = 0
raise_total = 0
try:
    for raise_index in range(6):
        raise_total += raise_source[raise_index]
except IndexError as error:
    frame = error.__traceback__
    while frame.tb_next is not None:
        frame = frame.tb_next
    print("IndexError:", error)
    print("raised at marker line +", frame.tb_lineno - marker_line)
    print("globals:", namespace["raise_index"], namespace["raise_total"])

# The same shape inside a `while`, where the loop variable is an ordinary
# guarded int rather than an iterator target.
while_source = [1, 2, 3, 4]
while_index = 0
while_total = 0
while while_index < 4:
    while_total += while_source[while_index]
    while_index += 1
print(while_index, while_total)

while_stop = 6
while_over_index = 0
while_over_total = 0
try:
    while while_over_index < while_stop:
        while_over_total += while_source[while_over_index]
        while_over_index += 1
except IndexError as error:
    print("while IndexError:", error)
    print("globals:", namespace["while_over_index"], namespace["while_over_total"])

# A tuple is a canonical sequence too.
tuple_source = (1, 2, 3)
tuple_sub_total = 0
for tuple_sub_index in range(3):
    tuple_sub_total += tuple_source[tuple_sub_index]
print(tuple_sub_total)

# A TypeError raised by the body after a side exit must still reach the
# surrounding handler, which the guard and side-exit edges have to carry into
# the zero-cost exception table.
handler_seen = []
try:
    for handler_item in [1, 2, "boom", 4]:
        handler_seen.append(handler_item)
        handler_sum = handler_item + 1
except TypeError as error:
    print("TypeError:", error)
print(handler_seen, namespace["handler_sum"])

# Mutating the sequence mid-loop changes both its length and its element types.
grow_source = [1, 2]
grow_index = 0
grow_seen = []
while grow_index < 4:
    grow_seen.append(grow_source[grow_index])
    if grow_index == 1:
        grow_source.append("appended")
        grow_source.append(9)
    grow_index += 1
print(grow_seen, grow_source)
