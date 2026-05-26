# Lambda parameter forms: defaults, *args, **kwargs, keyword-only.

# Default value
f = lambda x, y=1: x + y
assert f(1) == 2, f"f(1) = {f(1)}"
assert f(1, 2) == 3, f"f(1, 2) = {f(1, 2)}"
print("default:", f(1), f(1, 2))

# *args
g = lambda *args: args
assert g() == ()
assert g(1, 2, 3) == (1, 2, 3)
print("*args:", g(1, 2, 3))

# **kwargs
h = lambda **kw: kw
assert h() == {}
assert h(x=1, y=2) == {"x": 1, "y": 2}
print("**kwargs:", h(x=1))

# keyword-only after bare *
k = lambda *, x: x
assert k(x=99) == 99
print("keyword-only:", k(x=99))

# keyword-only with default after bare *
m = lambda *, x=7: x
assert m() == 7
assert m(x=3) == 3
print("keyword-only default:", m(), m(x=3))

# Full signature
j = lambda a, b=2, *args, c, d=4, **kw: (a, b, args, c, d, kw)
assert j(1, c=3) == (1, 2, (), 3, 4, {})
assert j(1, 10, 20, 30, c=3, d=5, e=6) == (1, 10, (20, 30), 3, 5, {"e": 6})
print("full:", j(1, c=3))

# Default captured from enclosing scope at definition time
val = 100
capture = lambda x=val: x
val = 999  # changing after definition should not affect default
assert capture() == 100
print("capture:", capture())

# Trailing comma after last positional before *args
t = lambda x, y=2,: x + y
assert t(1) == 3
print("trailing comma:", t(1))

# No params
nop = lambda: 42
assert nop() == 42
print("no params:", nop())

print("OK")
