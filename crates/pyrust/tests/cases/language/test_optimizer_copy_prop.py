# Exercises patterns that copy propagation should handle correctly.

# ── basic elimination: Move(r1, r0) + use(r1) → use(r0) ──────────────────
def add(a, b):
    return a + b

assert add(3, 4) == 7

# ── alias killed by overwrite ─────────────────────────────────────────────
def alias_overwrite():
    x = 10
    y = x      # y → x
    x = 99     # kills y → x alias
    return y   # must return 10, not 99

assert alias_overwrite() == 10

# ── dict update receiver must not be substituted ──────────────────────────
def merge(**kw):
    return kw

a = {'x': 1, 'y': 2}
b = {'z': 3}
result = merge(**a, **b)
assert result == {'x': 1, 'y': 2, 'z': 3}, f"got {result}"

# ── BinOp writes its dst, killing stale aliases ───────────────────────────
def binop_kills_alias():
    r = 5
    s = r       # s → r
    r = r + 10  # BinOp writes r, kills s → r
    return s    # must return 5, not 15

assert binop_kills_alias() == 5

# ── augmented assignment across globals ───────────────────────────────────
counter = 0
counter += 1
assert counter == 1
counter += 100
assert counter == 101

# ── chained copies ────────────────────────────────────────────────────────
def chain():
    a = 42
    b = a   # b → a
    c = b   # c → a (canonical)
    return c

assert chain() == 42

# ── list mutation via aliased receiver must not redirect ──────────────────
def list_alias():
    lst = [1, 2, 3]
    tmp = lst      # alias
    lst.append(4)  # appends to lst, not redirected to some other allocation
    return lst

assert list_alias() == [1, 2, 3, 4]

print("copy prop OK")
