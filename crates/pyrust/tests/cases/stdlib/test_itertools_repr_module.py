# itertools iterator objects must repr with the `itertools.` module prefix
# (`<itertools.islice object at 0x..>`), not the `__main__` default, and
# `type(x).__module__` must be "itertools" (issue #2098).

import itertools as it


def norm_addr(s):
    # Replace the trailing `0x...` hex address with a fixed placeholder so the
    # output is stable across runs / interpreters.
    i = s.find("0x")
    if i == -1:
        return s
    j = i + 2
    while j < len(s) and s[j] in "0123456789abcdefABCDEF":
        j += 1
    return s[:i] + "0xADDR" + s[j:]


objs = [
    ("islice", it.islice([1, 2, 3], 2)),
    ("cycle", it.cycle([1])),
    ("accumulate", it.accumulate([1, 2])),
    ("takewhile", it.takewhile(lambda x: x < 2, [1, 2, 3])),
    ("dropwhile", it.dropwhile(lambda x: x < 2, [1, 2, 3])),
    ("starmap", it.starmap(pow, [(2, 3)])),
    ("compress", it.compress("AB", [1, 0])),
    ("product", it.product([1], [2])),
    ("permutations", it.permutations([1, 2])),
    ("combinations", it.combinations([1, 2, 3], 2)),
    ("combinations_with_replacement", it.combinations_with_replacement([1, 2], 2)),
    ("groupby", it.groupby("aabb")),
]

for name, obj in objs:
    r = norm_addr(repr(obj))
    assert r == "<itertools.%s object at 0xADDR>" % name, r
    assert type(obj).__module__ == "itertools", type(obj).__module__

print("itertools repr module ok")
