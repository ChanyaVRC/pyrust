# A per-call-site inline cache resolves a plain (dot-free) built-in callee once
# and dispatches straight through the cached fn pointer on later calls. It must
# stay invisible: the cache is guarded on the callee name, so a rebound builtin,
# a polymorphic call site, or a shadowing local all re-resolve correctly.

# repeated calls at one site (the hot cache-hit path)
n = 0
for i in range(500):
    n += len(str(i)) + ord("a")
print(n)

# a variety of dot-free builtins through the same machinery
print(len("hello"), ord("A"), chr(66), abs(-7), max(3, 1, 2), min([5, 2, 8]))
print(sum([1, 2, 3, 4]), sorted([3, 1, 2]), list(range(4)), tuple("ab"))
print(int("42"), int("ff", 16), float("2.5"), str(99), bool(0), hex(255), bin(5))
print(round(3.14159, 2), pow(2, 8), divmod(17, 5), abs(-2.5))

# overloaded builtins (arg-type dispatch) at one repeated site
for v in ["A", b"B"]:
    print(ord(v))

# polymorphic call site: the callee changes between iterations
for f in (len, str, abs):
    if f is len:
        print(f("abcd"))
    elif f is str:
        print(f(123))
    else:
        print(f(-9))

# a call site that alternates between two different builtins
for i in range(6):
    g = len if i % 2 == 0 else hex
    print(g("xy") if i % 2 == 0 else g(i))

# rebinding a builtin name to a user function
saved = len
def len(x):
    return -1
print(len("ignored"), saved("four"))
del len
print(len("four"))

# shadowing a builtin with a local inside a function
def use_local_len(seq):
    len = lambda s: 100  # noqa: E731
    return len(seq)
print(use_local_len("anything"), len("real"))

# dotted names (type methods) must still dispatch via the full path
print("hello".upper(), [3, 1, 2, 1].count(1), {"a": 1}.get("a"), "a,b".split(","))

# builtins returning callables, called immediately
print(list(map(abs, [-1, -2, -3])), list(filter(bool, [0, 1, 2, 0])))
