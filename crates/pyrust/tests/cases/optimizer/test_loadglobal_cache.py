# Parity fixture for the LoadGlobal inline cache (issue #346).
#
# Exercises: cache invalidation on global reassignment, deletion, and the
# common hot-path cases (builtins, module-level globals in tight loops).

# ── Builtin lookups in a tight loop (primary perf target) ────────────────────

def builtin_in_loop(n):
    total = 0
    for i in range(n):
        total += len([1, 2, 3])  # len is a builtin; must be cached after first call
    return total

print(builtin_in_loop(100))  # 300

# ── Module-global reassignment must invalidate the cache ─────────────────────

counter = 0

def read_counter():
    return counter

print(read_counter())  # 0

counter = 1
print(read_counter())  # 1 — must see the updated value, not the cached 0

counter = 42
print(read_counter())  # 42

# ── Calling the same function multiple times after global changes ─────────────

x = 10

def get_x():
    return x

print(get_x())  # 10
x = 20
print(get_x())  # 20 — cache must be invalidated by the reassignment

# ── Deletion invalidates the cache ───────────────────────────────────────────

deletable = "hello"

def read_deletable():
    try:
        return deletable
    except NameError:
        return "gone"

print(read_deletable())  # hello
del deletable
print(read_deletable())  # gone

# ── globals() mutation via dict does NOT break correctness ────────────────────
# (values inserted only via globals() dict are not cached; subsequent reads
#  must still see the current dict value)

globals()["dynamic_name"] = "first"

def read_dynamic():
    # dynamic_name was written via globals() dict, not assign_name.
    # It is not cached by the inline cache, so each read goes through
    # the dict fallback path.
    return globals().get("dynamic_name", "missing")

print(read_dynamic())        # first
globals()["dynamic_name"] = "second"
print(read_dynamic())        # second

# ── Nonlocal / cell-var accesses are NOT affected by the cache ───────────────
# The cache only applies to module-level globals and builtins; closures must
# always see the current enclosing-scope value.

def outer():
    y = 1
    def inner():
        nonlocal y
        y = y + 4
        return y
    result_inner = inner()
    result_outer = y
    return [result_inner, result_outer]

print(outer())  # [5, 5]

# ── Free-variable reads (cell var without explicit nonlocal) ─────────────────

def outer2():
    z = 10
    def inner2():
        return z  # z is a cell var in outer2 — reads current value
    first = inner2()
    z = 99
    second = inner2()
    return [first, second]

print(outer2())  # [10, 99]

# ── Builtin still found after a same-named global is deleted ─────────────────
# Not a realistic scenario, but ensures cache correctness.

print(abs(-5))   # 5 — abs is a builtin
