# Regression for issue #371: dict/set key dedup must preserve bool-vs-int
# identity (CPython behaviour).  `True == 1` and `hash(True) == hash(1)`,
# so the second insert updates the value but the *original* key is kept.
print({True: 1, 1: 2})        # {True: 2}
print({1: 1, True: 2})        # {1: 2}
print({True: 1, 1: 2}[1])     # 2
print({True: 1, 1: 2}[True])  # 2
print(len({True: 1, 1: 2}))   # 1

# False vs 0 — same rule.
print({False: 1, 0: 2})       # {False: 2}
print({0: 1, False: 2})       # {0: 2}
print(len({False: 0, 0: 1})) # 1

# Bool and int with different truthy mappings must NOT dedup.
print(len({True: 1, 0: 2}))   # 2
print(len({False: 1, 1: 2})) # 2

# Sets dedup across bool/int the same way; CPython keeps the first-inserted key.
s = {True, 1, False, 0}
print(len(s))                 # 2

s2 = {1, True}
print(len(s2))                # 1

# `in` membership crosses bool/int.
d = {True: "a"}
print(1 in d)                 # True
print(True in d)              # True
print(False in d)             # False

s3 = {0}
print(False in s3)            # True
print(True in s3)             # False
