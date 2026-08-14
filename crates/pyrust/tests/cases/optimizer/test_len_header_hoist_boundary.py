"""A len-header copy caches its bound only for a proven-stable region."""

# Canonical sequence plus an int-only body: this is the hoisted fast path.
values = [3, 1, 4, 1, 5]
index = 0
total = 0
while index < len(values):
    total += values[index]
    index += 1
print("stable", total, index)

# Zero-trip entry must leave the body state untouched.
empty = []
index = 0
marker = 17
while index < len(empty):
    marker = 99
    index += 1
print("empty", marker, index)

# A body call that grows the list is not eligible for the cached-bound copy;
# the original per-iteration len call observes both appended elements.
growing = [1, 2]
index = 0
seen = []
while index < len(growing):
    seen.append(growing[index])
    if index < 2:
        growing.append(index + 10)
    index += 1
print("growing", seen, index, growing)

# Even an otherwise admissible Move cannot replace the sequence register under
# a cached bound. The next original header must call len(7) and raise.
rebound = [1, 2]
replacement = 7
index = 0
try:
    while index < len(rebound):
        rebound = replacement
        index += 1
except TypeError as error:
    print("rebound", type(error).__name__, index, rebound)

# A protocol receiver always takes the original call path, including the final
# false header check.
events = []


class Sized:
    def __init__(self, size):
        self.size = size

    def __len__(self):
        events.append("len")
        return self.size


sized = Sized(3)
index = 0
while index < len(sized):
    index += 1
print("protocol", index, events)
