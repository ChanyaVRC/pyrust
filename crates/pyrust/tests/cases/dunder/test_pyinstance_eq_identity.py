"""Parity fixture: ``Value::PartialEq::eq`` for ``PyInstance`` is identity-only
(umbrella issue #434, final acceptance criterion).

The low-layer ``PartialEq`` impl on ``Value`` must compare two ``PyInstance``
values by object identity (``Rc::ptr_eq``) only.  All user-observable equality
flows through the interpreter, which dispatches ``__eq__``; the bypass path is
used internally (e.g. ``PyKey`` hashing, dict/set membership) where identity is
the right semantic.

Two behaviours are locked in:

1. Direct ``a == b`` on user instances dispatches ``__eq__`` through the
   interpreter — a custom ``__eq__`` is honoured even for *distinct* objects,
   proving the identity-only ``PartialEq`` bypass is NOT what answers ``==``.
2. Without ``__eq__``, distinct instances compare unequal and the same instance
   compares equal — the identity-only fallback.  Internal containers that use
   the bypass (``dict``/``set`` membership) must agree with that identity rule.
"""


# --- direct == dispatches __eq__, even for distinct objects ------------------

class Always:
    def __eq__(self, other):
        return True


a1 = Always()
a2 = Always()
print(a1 == a2)   # True  — dispatches __eq__, not identity
print(a1 == a1)   # True
print(a1 is a2)   # False — distinct objects


class Never:
    def __eq__(self, other):
        return False
    __hash__ = object.__hash__


n1 = Never()
print(n1 == n1)        # False — __eq__ wins over identity short-circuit
print([n1] == [n1])    # False — element __eq__ in container


# --- no __eq__: identity-only semantics --------------------------------------

class Plain:
    pass


p1 = Plain()
p2 = Plain()
print(p1 == p2)   # False — distinct objects, default identity
print(p1 == p1)   # True  — same object


# --- internal containers (bypass path) honour identity -----------------------
# dict/set use the identity-only PartialEq for default-hashable instances.

d = {}
d[p1] = "p1"
d[p2] = "p2"
print(len(d))          # 2 — distinct identities, distinct keys
print(d[p1])           # p1
print(d.get(p2))       # p2
print(d.get(Plain()))  # None — a fresh object is not in the dict

s = {p1, p2}
print(p1 in s)          # True
print(p2 in s)          # True
print(Plain() in s)     # False
print(len(s))           # 2


# --- __eq__ returning NotImplemented falls back to identity ------------------

class NI:
    def __eq__(self, other):
        return NotImplemented
    __hash__ = object.__hash__


x = NI()
y = NI()
print(x == x)   # True  — identity fallback
print(x == y)   # False — distinct identities
