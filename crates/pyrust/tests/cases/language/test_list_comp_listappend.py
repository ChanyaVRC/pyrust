# Issue #1862: list comprehensions emit Insn::ListAppend directly.
# Exercises the same edge cases as the SetAdd fixture (#1861) to confirm the
# opcode rewrite is behavior-preserving and never hijacks a real .append().

# basic + condition
print([x * x for x in range(10)])
print([x for x in range(20) if x % 3 == 0])

# multiple for clauses with a filter
print([(i, j) for i in range(3) for j in range(3) if i != j])

# nested comprehensions (inner and outer each have their own .acc)
print([[y for y in range(x)] for x in range(4)])
print([[a * b for b in range(2)] for a in range(3)])

# element expression that legitimately calls a real .append() on a user object
# must NOT be intercepted (target is `out`, not the reserved `.acc`).
out = []
res = [out.append(i) for i in range(3)]
print(out, res)


class C:
    def __init__(self):
        self.items = []

    def append(self, v):
        self.items.append(v * 10)
        return None


c = C()
_ = [c.append(i) for i in range(3)]
print(c.items)

# comp over a generator
print([x + 1 for x in (n for n in range(5) if n)])

# empty comp
print([x for x in []])

# walrus in comp leaks the name to the enclosing scope
data = [1, 2, 3, 4]
filtered = [y for x in data if (y := x * 2) > 4]
print(filtered, y)

# side-effect / evaluation ordering preserved
log = []


def f(v):
    log.append(("f", v))
    return v


def keep(v):
    log.append(("keep", v))
    return v % 2 == 0


print([f(x) for x in range(4) if keep(x)])
print(log)

# element is a list literal -> ListAppend receives a list value
print([[i] for i in range(3)])
