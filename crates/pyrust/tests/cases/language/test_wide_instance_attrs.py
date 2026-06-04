# Hybrid Vec/hash instance attribute storage (#2162): get/set/del/__dict__ and
# insertion order must be identical for small (below threshold), transition
# (the 17th attribute), and wide instances.
class C:
    pass


def make(n):
    o = C()
    for i in range(n):
        setattr(o, f"a{i}", i * 10)
    return o


for n in (3, 16, 17, 32, 50, 100):
    o = make(n)
    print(n, all(getattr(o, f"a{i}") == i * 10 for i in range(n)))
    print(list(vars(o).keys()) == [f"a{i}" for i in range(n)])
    print(list(o.__dict__.keys()) == [f"a{i}" for i in range(n)])

# Overwrite keeps insertion position even when wide.
o = make(20)
o.a5 = 999
print(o.a5, list(vars(o).keys()) == [f"a{i}" for i in range(20)])

# Delete then re-add appends at the end (CPython __dict__ semantics).
o = make(20)
del o.a5
print("a5" in vars(o))
o.a5 = 1234
print(list(vars(o).keys())[-1], o.a5)

# Deleting across the threshold leaves remaining attrs readable and ordered.
o = make(18)
for i in range(5):
    delattr(o, f"a{i}")
print(hasattr(o, "a0"), o.a17)
print(list(vars(o).keys()) == [f"a{i}" for i in range(5, 18)])

# __dict__ wholesale replacement on a wide instance.
o = make(20)
o.__dict__ = {"x": 1, "y": 2}
print(o.x, o.y, list(vars(o).keys()))

# Warm repeated access on a wide instance (inline-cache path).
o = make(40)
total = 0
for _ in range(1000):
    total += o.a39
print(total)
