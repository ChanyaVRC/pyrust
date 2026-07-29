# Heap tuples (>=4 elements) share an Rc backing so Value::clone / Insn::Move is
# an O(1) refcount bump instead of an O(N) Vec deep-copy (#2268).  Sharing is
# sound because tuples are immutable.  This fixture locks the observable
# contract: aliasing identity, element identity, equality/hashing, ordering, and
# all the construction/indexing/iteration surface stay byte-for-byte unchanged.

# --- Aliasing identity: b = a shares the backing, so a is b and id() match. ---
t = tuple(range(4))
a = t
print(a is t)            # True
print(id(a) == id(t))    # True
b = t
c = b
print(a is c, b is c)    # True True

# A distinct construction with equal contents is a different object.
d = tuple(range(4))
print(t == d, t is d)    # True False
print(hash(t) == hash(d))# True

# --- Element access reads through the shared backing correctly. ---
print(t[0], t[1], t[2], t[3])
print(t[-1], t[-4])
print(t[1:3], t[::-1])

# --- Element identity is preserved across aliasing clones (shallow share). ---
marker = object()
e = (marker, "x", "y", "z")
f = e
print(f[0] is marker)    # True
print(e[0] is f[0])      # True

# --- Tuple of tuples: inner heap tuple aliased in two slots is one object. ---
inner = tuple(range(10, 14))
outer = (inner, inner)
print(outer[0] is outer[1])      # True
print(outer[0] is inner)         # True

# --- Equality, ordering, membership, concatenation, repetition, unpacking. ---
print((1, 2, 3, 4) < (1, 2, 3, 5))   # True
print((1, 2, 3, 4) == (1, 2, 3, 4))  # True
print(3 in t, 99 in t)               # True False
print(t + (4, 5))                    # (0, 1, 2, 3, 4, 5)
print((0, 1) * 3)                    # (0, 1, 0, 1, 0, 1)
w, x, y, z = t
print(w, x, y, z)                    # 0 1 2 3

# --- Hashing / dict + set membership uses the shared backing. ---
m = {t: "hit"}
print(m[tuple(range(4))])            # hit
s = {t, d, (9, 9, 9, 9)}
print(len(s))                        # 2

# --- Iteration, list/tuple round-trips, len, repr. ---
print([v for v in t])                # [0, 1, 2, 3]
print(list(t), tuple(list(t)) == t)  # [0, 1, 2, 3] True
print(len(t), len(()), len((1, 2, 3, 4, 5)))
print(repr(t), str(t))

# --- Reassigning a name to a fresh tuple does not disturb the alias. ---
g = tuple(range(4))
h = g
g = (100, 200, 300, 400)
print(h, g, h is g)                  # (0, 1, 2, 3) (100, 200, 300, 400) False

# --- Many aliases then drop: backing stays valid (no use-after-free). ---
base = tuple(range(100))
refs = [base for _ in range(50)]
print(refs[0] is base, len(refs[49]), refs[49] is refs[0])
refs = None
print(len(base), base[0], base[99])  # backing survived the dropped aliases

# --- nested mutable inside immutable tuple still aliases correctly. ---
lst = [1, 2, 3]
nt = (lst, lst, "a", "b")
nt2 = nt
nt[0].append(4)
print(nt2[0], nt[0] is nt2[1])       # [1, 2, 3, 4] True
