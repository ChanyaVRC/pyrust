# copy module — parity fixture for the __getstate__ / __setstate__ copy
# protocol (#2131).  copy.copy / copy.deepcopy must consult __getstate__ to
# capture state and __setstate__ to restore it, instead of cloning __dict__
# verbatim.

import copy

# ── __getstate__ + __setstate__: deepcopy drives state through the hooks ──────

class St:
    def __init__(self):
        self.a = 1
        self.b = 2

    def __getstate__(self):
        return {"a": self.a * 10}

    def __setstate__(self, state):
        self.a = state["a"]
        self.restored = True


c = copy.deepcopy(St())
# __getstate__ returned {"a": 10}; __setstate__ set a + restored; b dropped.
assert c.a == 10
assert c.restored is True
assert not hasattr(c, "b")

# Shallow copy honours the same protocol.
sc = copy.copy(St())
assert sc.a == 10
assert sc.restored is True
assert not hasattr(sc, "b")

# ── only __getstate__: default __setstate__ does __dict__.update ──────────────

class OnlyGet:
    def __init__(self):
        self.a = 1
        self.b = 2

    def __getstate__(self):
        return {"a": self.a * 100}


g = copy.deepcopy(OnlyGet())
assert g.a == 100
assert not hasattr(g, "b")

# ── only __setstate__: default __getstate__ returns the __dict__ ──────────────

class OnlySet:
    def __init__(self):
        self.a = 1
        self.b = 2

    def __setstate__(self, state):
        self.a = state["a"]
        self.loaded = True


s = copy.deepcopy(OnlySet())
assert s.a == 1
assert s.loaded is True
assert not hasattr(s, "b")

# ── non-dict state object (tuple) round-trips through the hooks ───────────────

class TupleState:
    def __init__(self):
        self.a = 5

    def __getstate__(self):
        return (self.a,)

    def __setstate__(self, state):
        self.a = state[0] * 2


ts = copy.deepcopy(TupleState())
assert ts.a == 10

# ── 2-tuple state (state, slotstate) with default __setstate__ ───────────────

class TwoTuple:
    def __init__(self):
        self.a = 1

    def __getstate__(self):
        return ({"a": 5}, {"b": 9})


tt = copy.deepcopy(TwoTuple())
assert tt.a == 5
assert tt.b == 9

# ── deepcopy through __getstate__ deep-copies the captured state ──────────────

class Holder:
    def __init__(self, items):
        self.items = items


h = Holder([1, 2, 3])
hc = copy.deepcopy(h)
hc.items.append(4)
assert h.items == [1, 2, 3]      # original untouched — state was deep-copied
assert hc.items == [1, 2, 3, 4]

# ── plain instance (no hooks) still copies __dict__ as before ─────────────────

class Plain:
    def __init__(self):
        self.x = 7
        self.y = 8


p = copy.deepcopy(Plain())
assert p.x == 7 and p.y == 8
ps = copy.copy(Plain())
assert ps.x == 7 and ps.y == 8

print("copy protocol ok")
