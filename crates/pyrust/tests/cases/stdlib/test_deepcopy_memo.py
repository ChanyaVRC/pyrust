# copy module — parity fixture for deepcopy memoisation (#1997).  deepcopy
# must use a memo keyed by id() so that cyclic structures terminate (no native
# stack overflow) and shared references stay shared.

import copy

# ── self-referential list: cycle preserved, no crash ─────────────────────────

a = [1, 2]
a.append(a)
b = copy.deepcopy(a)
assert len(b) == 3
assert b[2] is b          # cycle re-pointed at the copy, not the original
assert b is not a
assert b[2] is not a

# ── self-referential dict ────────────────────────────────────────────────────

d = {}
d["self"] = d
cd = copy.deepcopy(d)
assert cd["self"] is cd
assert len(cd) == 1

# ── mixed list/dict cycle ────────────────────────────────────────────────────

x = [1]
y = {"k": x}
x.append(y)
cx = copy.deepcopy(x)
assert cx[0] == 1
assert cx[1]["k"] is cx

# ── shared references preserved (not duplicated) ─────────────────────────────

inner = [1, 2]
shared = copy.deepcopy([inner, inner])
assert shared[0] is shared[1]      # both entries still the same object
assert shared[0] is not inner

p = [1]
q = copy.deepcopy([p, p])
assert q[0] is q[1]

# shared inside a dict
sd = copy.deepcopy({"a": inner, "b": inner})
assert sd["a"] is sd["b"]

# shared inside a tuple
st = copy.deepcopy((inner, inner))
assert st[0] is st[1]

# ── self-referential instance graph ──────────────────────────────────────────

class Node:
    def __init__(self):
        self.next = None
        self.data = [0]


n = Node()
n.next = n
cn = copy.deepcopy(n)
assert cn.next is cn               # instance cycle preserved
assert cn is not n
assert cn.data is not n.data       # owned attr still deep-copied

# shared instance refs in a list
o = Node()
co = copy.deepcopy([o, o])
assert co[0] is co[1]
assert co[0] is not o

# ── mutating the deep copy never touches the original ────────────────────────

orig = [[1, 2], [3, 4]]
cp = copy.deepcopy(orig)
cp[0][0] = 99
assert orig[0][0] == 1
assert cp[0][0] == 99

print("deepcopy memo ok")
