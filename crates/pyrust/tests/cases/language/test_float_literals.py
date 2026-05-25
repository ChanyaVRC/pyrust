# Float literal syntax: trailing-dot, leading-dot, and exponent forms.
# CPython 3.12 accepts all of these; earlier versions agree on this subset.

# ── trailing-dot floats ────────────────────────────────────────────────────
x = 1.
print(x, type(x).__name__)   # 1.0 float

y = 42.
print(y, type(y).__name__)   # 42.0 float

# ── leading-dot floats ─────────────────────────────────────────────────────
a = .5
print(a, type(a).__name__)   # 0.5 float

b = .25
print(b, type(b).__name__)   # 0.25 float

# ── trailing-dot with exponent ─────────────────────────────────────────────
c = 1.e5
print(c, type(c).__name__)   # 100000.0 float

d = 1.E+2
print(d, type(d).__name__)   # 100.0 float

e = 2.e-1
print(e, type(e).__name__)   # 0.2 float

# ── leading-dot with exponent ─────────────────────────────────────────────
f = .5e-3
print(f, type(f).__name__)   # 0.0005 float

g = .5E2
print(g, type(g).__name__)   # 50.0 float

# ── standard float (regression: must still work) ───────────────────────────
h = 1.5
print(h, type(h).__name__)   # 1.5 float

i = 3.14
print(i, type(i).__name__)   # 3.14 float

# ── imaginary suffix on leading-dot and trailing-dot ─────────────────────
j1 = .5j
print(j1, type(j1).__name__)   # 0.5j complex

j2 = 1.j
print(j2, type(j2).__name__)   # 1j complex

# ── arithmetic using trailing/leading dot literals ─────────────────────────
print(1. + .5)   # 1.5
print(.5 * 2.)   # 1.0
print(1.e2 / .5e1)   # 20.0

# ── attribute access on int variable (must not be broken) ─────────────────
n = 255
print(n.bit_length())   # 8
