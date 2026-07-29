# Tuple slicing must not deep-copy the whole source before slicing (#2114).
# Tuple Values now share an Rc-backed immutable payload, so retaining an owned
# source across bound conversion is an O(1) refcount bump. This fixture locks
# the observable result: a large-source / tiny-result slice has shared
# (shallow) element identity, byte-identical to CPython.

big = tuple(range(1000))

# Tiny / empty results from a large source.
print(big[100:100])
print(big[500:505])
print(len(big[:]))
print(big[-3:])
print(big[::250])
print(big[::-1][:5])

# Element identity is shared (shallow copy), independent of result size.
marker = object()
src = (marker, "a", "b", "c", "d")
print(src[0:1][0] is marker)
print(src[:][0] is marker)
print(src[::-1][-1] is marker)

# Nested mutable element is shared, not deep-copied.
inner = [1, 2]
nested = (inner, "x", "y")
sl = nested[0:1]
sl[0].append(3)
print(inner)
print(nested[0])

# Repeated slicing of the same large source stays correct (the hot-loop case).
acc = 0
for i in range(100):
    acc += sum(big[i : i + 3])
print(acc)
