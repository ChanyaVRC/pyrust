# PEP 572: walrus operator inside a comprehension assigns to the nearest
# enclosing scope that is not itself a comprehension.

# ── Basic: walrus in list-comp inside a function ──────────────────────────────

def f_basic():
    last = None
    _ = [last := x for x in [1, 2, 3]]
    return last

print("basic:", f_basic())  # 3

# ── Module-level walrus via comprehension ─────────────────────────────────────

_mod_last = None
_ = [_mod_last := x for x in [10, 20, 30]]
print("module:", _mod_last)  # 30

# ── Module-level variable not mutated by comp inside a function ───────────────

_outer = "outer"

def f_no_global_bleed():
    last = None
    _ = [last := x for x in [1, 2, 3]]
    return last

_ = f_no_global_bleed()
print("outer unchanged:", _outer)  # outer

# ── Walrus inside a genexp ────────────────────────────────────────────────────

def f_genexp():
    last = None
    _ = list(last := x for x in [1, 2, 3])
    return last

print("genexp:", f_genexp())  # 3

# ── Walrus inside a set-comp ──────────────────────────────────────────────────

def f_setcomp():
    last = None
    _ = {last := x for x in [1, 2, 3]}
    return last

print("setcomp:", f_setcomp())  # 3

# ── Walrus inside a dict-comp (in value position) ─────────────────────────────

def f_dictcomp():
    last = None
    _ = {x: (last := x * 2) for x in [1, 2, 3]}
    return last

print("dictcomp:", f_dictcomp())  # 6

# ── Walrus in the filter condition ────────────────────────────────────────────

def f_cond():
    last = None
    _ = [x for x in [1, 2, 3, 4, 5] if (last := x) > 2]
    return last

print("cond:", f_cond())  # 5

# ── Nested comp: walrus in inner comp targets the enclosing function ──────────

def f_nested():
    last = None
    _ = [[last := x for x in row] for row in [[1, 2], [3, 4]]]
    return last

print("nested:", f_nested())  # 4

# ── Triple nesting ────────────────────────────────────────────────────────────

def f_triple():
    last = None
    matrix = [[[1, 2], [3, 4]], [[5, 6], [7, 8]]]
    _ = [[[last := x for x in row] for row in mat] for mat in matrix]
    return last

print("triple:", f_triple())  # 8

# ── Walrus in nested comp with condition ─────────────────────────────────────

def f_nested_cond():
    last = None
    _ = [[last := x for x in row if x > 1] for row in [[1, 2], [3, 4]]]
    return last

print("nested_cond:", f_nested_cond())  # 4
