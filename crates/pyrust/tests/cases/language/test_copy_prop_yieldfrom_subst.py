# Regression test for issue #1521: pass_copy_prop had no YieldFrom arm, so
# iter_reg and sent_reg were not substituted when copy aliases existed.
#
# These cases are designed so that the compiler emits a Move before YieldFrom
# (creating a copy alias), and the optimizer must substitute the alias in the
# YieldFrom operands.

# ── 1. Basic yield-from with aliased iterator ─────────────────────────────────

def sub():
    yield 1
    yield 2
    return 'done'

def wrapper_alias_iter():
    it = sub()      # iter assigned to a local
    alias = it      # copy alias: alias → it
    result = yield from alias  # iter_reg should be substituted to it
    yield result

g = wrapper_alias_iter()
print(next(g))   # 1
print(next(g))   # 2
try:
    print(next(g))  # 'done'
except StopIteration:
    pass

# ── 2. yield-from with send values propagated correctly ───────────────────────

def sub2():
    x = yield 10
    y = yield 20
    return (x, y)

def wrapper_send():
    it = sub2()
    it2 = it    # alias: it2 → it
    result = yield from it2
    yield result

g2 = wrapper_send()
print(next(g2))      # 10
print(g2.send(100))  # 20
try:
    val = g2.send(200)  # StopIteration with (100, 200)
    print(val)           # (100, 200)
except StopIteration as e:
    print(e.value)

# ── 3. Alias of a generator expression (iter_reg is a temp alias) ─────────────

def sub3():
    for i in range(3):
        yield i

def wrapper_genexpr():
    src = sub3()
    copy = src  # alias
    total = 0
    for v in copy:
        total += v
    yield total

print(list(wrapper_genexpr()))  # [3]

# ── 4. Two-level nesting: sent values reach the innermost generator ────────────

def inner():
    a = yield 'a'
    b = yield 'b'
    return [a, b]

def outer():
    gen = inner()
    alias = gen
    r = yield from alias
    yield r

g4 = outer()
print(next(g4))       # 'a'
print(g4.send('A'))   # 'b'
try:
    v4 = g4.send('B')   # ['A', 'B']
    print(v4)
except StopIteration as e:
    print(e.value)

print("copy_prop YieldFrom substitution OK")
