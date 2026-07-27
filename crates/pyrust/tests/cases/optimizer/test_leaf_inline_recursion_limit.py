import sys


def leaf(a, b):
    return a + b


def probe(depth, use_leaf):
    if depth:
        return probe(depth - 1, use_leaf)
    if use_leaf:
        return leaf(1, 2)
    return 3


old_limit = sys.getrecursionlimit()
sys.setrecursionlimit(80)

boundary = None
for depth in range(1, 200):
    try:
        probe(depth, False)
    except RecursionError:
        boundary = depth
        break

print(boundary is not None)
try:
    # `boundary - 1` reaches the base frame successfully; entering one more
    # Python frame for leaf must cross the same recursion boundary.
    probe(boundary - 1, True)
except RecursionError:
    print("recursion")
else:
    print("missed")

sys.setrecursionlimit(old_limit)
print(leaf(20, 22))
