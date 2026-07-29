# value_to_pykey primitive fast path: str/int/bool/None keys build their PyKey
# directly.  This must be behavior-identical to the general path, including the
# CPython cross-type key equivalences (True == 1 == 1.0, False == 0) and the
# mixing of fast-path primitive keys with slow-path keys (tuple/float/instance)
# in the same dict/set.

# Cross-type numeric key collapse (fast-path Bool/Int vs slow-path Float).
print({True: "a", 1: "b"})  # True and 1 are the same key -> {True: 'b'}
print({1: "a", True: "b"})  # -> {1: 'b'}
print({False: "a", 0: "b", 0.0: "c"})  # all one key -> {False: 'c'}
print({1: "x", 1.0: "y"})  # int/float collapse -> {1: 'y'}

# None key alongside other primitive keys.
print({None: 1, "None": 2, 0: 3})
print(None in {None: 1})
print(1 in {True: 9})  # True key, look up 1

# str keys (SSO and longer), lookups.
d = {"a": 1, "bb": 2, "ccc": 3, "a_longer_key_beyond_sso": 4}
print(d["a"], d["bb"], d["ccc"], d["a_longer_key_beyond_sso"])
print("bb" in d, "zz" in d)

# Mixed fast (int/str/bool) + slow (tuple/float/frozenset) keys in one dict.
m = {1: "int", "s": "str", True: "bool2", (1, 2): "tuple", 3.5: "float", frozenset({7}): "fs"}
print(m[1], m["s"], m[(1, 2)], m[3.5], m[frozenset({7})])
print(len(m))

# Sets with primitive members (value_to_pykey drives SetAdd too).
s = {1, 2, 2, "a", "a", True, None, False}
print(sorted(repr(x) for x in s))
print(1 in s, "a" in s, None in s, 3 in s)

# dict/set comprehensions (per-element value_to_pykey).
print({k: k * k for k in range(5)})
print(sorted({x % 3 for x in range(10)}))

# bool/int dedup in set: {1, True} -> one element.
print(len({1, True}), len({0, False}), len({1, True, 1.0}))


# Unhashable key still raises the precise TypeError (fast path must not swallow).
def show_err(fn):
    try:
        fn()
    except Exception as e:
        print(type(e).__name__ + ":", e)


show_err(lambda: {[1, 2]: 1})
show_err(lambda: {(1, [2]): 1})  # nested unhashable -> names 'list'
