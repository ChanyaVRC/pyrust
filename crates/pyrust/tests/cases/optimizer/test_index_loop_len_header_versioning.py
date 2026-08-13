"""`while i < len(seq):` keeps its per-iteration length, however the loop runs.

The guarded copy of this shape reads the length natively instead of calling
`len`, which is only sound while the read stays *inside* the loop: the bound has
to move when the body resizes the sequence, and the whole copy has to step aside
when the name no longer resolves to the built-in or the argument is not a
canonical sequence.
"""

import builtins

# ── The original path observes mutations in its body ─────────────────────────

growing = [1, 2, 3]
index = 0
visited = []
while index < len(growing):
    visited.append(growing[index])
    if len(growing) < 6:
        growing.append(growing[index] * 10)
    index += 1
print("grow", visited, index, growing)

shrinking = [1, 2, 3, 4, 5, 6, 7, 8]
index = 0
total = 0
while index < len(shrinking):
    total += shrinking[index]
    del shrinking[0]
    index += 1
print("shrink", total, index, shrinking)

emptied = [1, 2, 3, 4]
index = 0
steps = 0
while index < len(emptied):
    steps += 1
    emptied.clear()
    index += 1
print("clear", steps, index, emptied)

# The mutating calls and deletion above deliberately keep those loops on the
# original path.
# Here a guarded subscript first reaches the copy, then side-exits on an int
# subclass. Its reflected add runs outside the copy, grows the bound, and the
# next entry must read the new length rather than reuse the original two.
def external_growth():
    holder = [[1, 2]]
    bounds = holder[0]

    class GrowOnAdd(int):
        def __radd__(self, other):
            holder[0].extend([3, 4])
            return other + int(self)

    values = [GrowOnAdd(10), 11, 12, 13]
    index = 0
    total = 0
    while index < len(bounds):
        total += values[index]
        index += 1
    return total, index, bounds


print("external-grow", external_growth())

# ── A cursor that outruns the sequence still raises on the original path ──────

items = [1, 2, 3, 4, 5]
index = 0
total = 0
try:
    while index < len(items):
        ahead = index + 2
        total += items[ahead]
        index += 1
except IndexError as error:
    print("IndexError", error)
print("after-raise", total, index, ahead)

# ── Non-canonical arguments run the real call ─────────────────────────────────

events = []


class ProtocolSequence:
    def __init__(self, size):
        self.size = size

    def __len__(self):
        events.append("len")
        return self.size

    def __getitem__(self, index):
        events.append(("get", index))
        return index + 10


sequence = ProtocolSequence(3)
index = 0
seen = []
while index < len(sequence):
    seen.append(sequence[index])
    index += 1
print("protocol", seen, events)

mapping = {"a": 1, "b": 2, "c": 3}
index = 0
steps = 0
while index < len(mapping):
    steps += 1
    index += 1
print("mapping", steps, index)

number = 7
index = 0
try:
    while index < len(number):
        index += 1
except TypeError as error:
    print("TypeError", error)

# ── A rebound `len` is observed by value, not by name ─────────────────────────

numbers = [1, 2, 3, 4, 5]

index = 0
total = 0
while index < len(numbers):
    total += numbers[index]
    index += 1
print("builtin-len", total, index)

globals()["len"] = lambda sequence: 2
index = 0
total = 0
while index < len(numbers):
    total += numbers[index]
    index += 1
print("globals-rebind", total, index)
del globals()["len"]

original_len = builtins.len
builtins.len = lambda sequence: 1
index = 0
total = 0
while index < len(numbers):
    total += numbers[index]
    index += 1
builtins.len = original_len
print("builtins-rebind", total, index)


def shadowed():
    def len(value):
        return 0

    items = [1, 2]
    index = 0
    seen = []
    while index < len(items):
        seen.append(items[index])
        index += 1
    return seen


print("shadowed", shadowed())

# ── Other canonical sequence kinds ────────────────────────────────────────────

text = "hello"
index = 0
letters = ""
while index < len(text):
    letters += text[index]
    index += 1
print("str", letters, index)

astral = "a😀bé"
index = 0
steps = 0
while index < len(astral):
    steps += 1
    index += 1
print("astral-str-bound", steps, index)

raw = b"abc"
index = 0
total = 0
while index < len(raw):
    total += raw[index]
    index += 1
print("bytes", total, index)

# bytearray is not a canonical fast-path receiver. This pins the original/deopt
# path's real-len bound.
buffer = bytearray(b"abc")
index = 0
steps = 0
while index < len(buffer):
    steps += 1
    index += 1
print("bytearray-bound", steps, index)

pair = (7, 8, 9)
index = 0
total = 0
while index < len(pair):
    total += pair[index]
    index += 1
print("tuple", total, index)

empty = []
index = 0
steps = 0
while index < len(empty):
    steps += 1
    index += 1
print("empty", steps, index)

# ── The sequence register itself may be rebound mid-loop ──────────────────────

swapped = [1, 2, 3, 4, 5, 6, 7, 8]
index = 0
total = 0
while index < len(swapped):
    total += swapped[index]
    if index == 1:
        swapped = [9, 9, 9]
    index += 1
print("rebind-sequence", total, index, swapped)

# ── The live namespace is current at every exit ───────────────────────────────

namespace = globals()
values = [1, 2, 3]
index = 0
total = 0
while index < len(values):
    total += values[index]
    index += 1
print("namespace", namespace["total"], namespace["index"])
