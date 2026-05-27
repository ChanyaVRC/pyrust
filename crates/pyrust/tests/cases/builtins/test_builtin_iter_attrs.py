# Parity fixture for issue #1437: built-in iterable types must expose __iter__
# as an accessible attribute (hasattr / getattr), and calling the bound method
# must return a working iterator.

# --- hasattr checks ---
print(hasattr([], '__iter__'))           # True
print(hasattr({}, '__iter__'))           # True
print(hasattr("", '__iter__'))           # True
print(hasattr((1, 2), '__iter__'))       # True
print(hasattr(range(3), '__iter__'))     # True
print(hasattr(b"", '__iter__'))          # True
print(hasattr(frozenset(), '__iter__'))  # True

# --- hasattr on already-working dunders must not regress ---
print(hasattr([], '__len__'))            # True
print(hasattr({}, '__len__'))            # True

# --- __iter__ must not be exposed on non-iterable builtins ---
print(hasattr(1, '__iter__'))            # False
print(hasattr(1.0, '__iter__'))          # False

# --- calling the bound method returns a working iterator ---
it = [].__iter__()
print(next(it, 'done'))                  # done  (empty list)

it = [10, 20].__iter__()
print(next(it))                          # 10
print(next(it))                          # 20
print(next(it, 'done'))                  # done

it = "ab".__iter__()
print(next(it))                          # a
print(next(it))                          # b
print(next(it, 'done'))                  # done

it = (7, 8).__iter__()
print(next(it))                          # 7
print(next(it))                          # 8
print(next(it, 'done'))                  # done

it = b"\x01\x02".__iter__()
print(next(it))                          # 1
print(next(it))                          # 2
print(next(it, 'done'))                  # done

it = {1: 'a', 2: 'b'}.__iter__()
k1 = next(it)
k2 = next(it)
print(sorted([k1, k2]))                  # [1, 2]
print(next(it, 'done'))                  # done

it = {3, 4}.__iter__()
vals = sorted([next(it), next(it)])
print(vals)                              # [3, 4]
print(next(it, 'done'))                  # done

it = frozenset({5, 6}).__iter__()
vals = sorted([next(it), next(it)])
print(vals)                              # [5, 6]
print(next(it, 'done'))                  # done

it = range(2).__iter__()
print(next(it))                          # 0
print(next(it))                          # 1
print(next(it, 'done'))                  # done

# --- iterators produced by __iter__() must have __next__ ---
it = [].__iter__()
print(hasattr(it, '__next__'))           # True

it = "".__iter__()
print(hasattr(it, '__next__'))           # True
