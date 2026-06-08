# Passing a tuple to a builtin must not deep-copy the source tuple (#2251).
# The VM's builtin-call argument path read each arg with a clone, and
# Value::clone of a heap tuple is an O(N) Vec deep-copy (unlike list/str/bytes,
# which share/rc-bump).  Read-only builtins (len/sum/min/max/any/all/sorted/…)
# therefore paid an O(N) copy per call.  The VM now lends a heap-tuple arg by
# move into the call buffer and restores the register afterwards, so the
# behaviour is observably unchanged.  This fixture locks the observable
# contract: results are correct, source identity is preserved, the source
# register survives the call, builtins that retain the value still copy it,
# and re-entrant calls during a key= callback see intact register state.

t = tuple(range(10))

# Read-only builtins return the right answers.
print(len(t), sum(t), min(t), max(t), any(t), all(t))
print(sorted(t, reverse=True))
print(sorted(t, key=lambda x: -x))
print(max(t, key=lambda x: -x))

# The source register survives the call: t is unchanged and still the same obj.
a = t
print(len(t), t)
print(t is a, len(t) == 10)

# Builtins that retain/own the value still get an independent, correct copy.
l = list(t)
print(l == list(range(10)), l is t)
it = iter(t)
print(next(it), next(it))

# Element identity is shared (shallow), independent of any arg handling.
marker = object()
src = (marker, "x", "y", "z", "w")
print(len(src), src[0] is marker)

# Empty and single-element tuples.
print(len(()), len((42,)), sum(()), bool(()), bool((0,)))

# Multiple heap-tuple args to one builtin.
u = tuple(range(10, 20))
print(list(zip(t, u)))
print(list(map(lambda x, y: x + y, t, u)))

# A tuple key in a dict (hashing reads the tuple).
d = {t: "found"}
print(d[t])

# Re-entrant builtin call inside a key= callback must see intact registers.
seen = []
def k(x):
    seen.append(len(t))
    return x
print(sorted(t, key=k))
print(seen[0], seen[-1])

# Hot-loop shape: repeated read-only calls on a large source stay correct.
big = tuple(range(1000))
acc = 0
for _ in range(1000):
    acc += len(big)
print(acc, sum(big), min(big), max(big))

# Error path: wrong argument count is unchanged.
try:
    len(t, t)
except TypeError as e:
    print(e)
