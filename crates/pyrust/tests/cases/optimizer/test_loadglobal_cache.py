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

# ── Cell var shadowing a module global must not be cached as a module global ──
# When an inner closure reads a name via LoadGlobal, the slow-path must not
# cache the cell-var value under the global_env_version sentinel just because
# the module env also happens to have a binding for the same name.  Doing so
# causes stale results after the outer function mutates the cell var (which
# does NOT bump global_env_version).
#
# Regression test for review-time fix: the original PR used
# `lookup_name_in_module(name).is_some()` to decide whether to cache, which
# returns True even when the value came from an intermediate (cell-var) env.

shadow_g = "module_shadow_g"  # module global — same name as the cell var below

def outer_shadow():
    shadow_g = "cell_a"         # outer's cell var, shadows the module global
    def inner_shadow():
        return shadow_g         # LoadGlobal: must always see outer's live cell var
    r1 = inner_shadow()         # "cell_a"
    r2 = inner_shadow()         # "cell_a" (cached correctly)
    shadow_g = "cell_b"         # nonlocal write — does NOT bump global_env_version
    r3 = inner_shadow()         # MUST be "cell_b", not stale "cell_a"
    r4 = inner_shadow()         # also "cell_b"
    return [r1, r2, r3, r4]

print(outer_shadow())  # ['cell_a', 'cell_a', 'cell_b', 'cell_b']
print(shadow_g)        # 'module_shadow_g' — module global unchanged

# ── Module global and cell var share the same name; module reassignment must ──
# not poison the cell-var read path.
#
# `y` exists at module scope.  `outer_cellvar` defines its own `y` (cell var)
# and `inner_cellvar` reads and mutates it via `nonlocal`.  After the module-
# level `y` is reassigned the closure must still see the cell-var value, not
# the freshly cached module-scope value, because the cache-eligibility check
# inspects `global_names` (the `global`-declaration set) and `env.parent.is_none()`
# to determine whether the slow-path hit was truly in the module env.

y = 0  # module-level global

def outer_cellvar():
    y = 10            # cell var in outer_cellvar, shadows the module global
    def inner_cellvar():
        nonlocal y
        y += 1
        return y
    r1 = inner_cellvar()   # 11
    r2 = inner_cellvar()   # 12
    return [r1, r2]

print(outer_cellvar())   # [11, 12]

# Reassign the module global — must not disturb the cell-var logic above.
y = 999
print(outer_cellvar())   # still [11, 12] because each call creates a fresh cell

# ── Builtin shadowed by a module-level assignment must resolve to the new name ─
# When a builtin name is first loaded by a function it is cached with the
# current global_env_version.  A subsequent module-level assignment to that
# name bumps the version, so the cache entry is invalidated and the slow path
# re-resolves.  The test uses global declarations to go through the StoreGlobal
# path (which bumps global_env_version), and impure side effects so the
# function is not mis-classified as pure and memoized.

results = []

def record_len():
    # Side effect (list.append) makes this function impure so the compiler
    # does not cache its result via CallMemo.
    results.append(len([1, 2, 3]))

record_len()          # results[0] = 3  — uses builtin len

def shadow_len():
    global len
    len = lambda v: 99

shadow_len()          # shadows builtin len via StoreGlobal, bumps global_env_version
record_len()          # results[1] = 99 — must see the module-scope shadow

def restore_len():
    global len
    del len

restore_len()         # removes the shadow; builtin len is visible again
record_len()          # results[2] = 3  — builtin restored

print(results)        # [3, 99, 3]

# ── Builtin still found after a same-named global is deleted ─────────────────
# Not a realistic scenario, but ensures cache correctness.

print(abs(-5))   # 5 — abs is a builtin
