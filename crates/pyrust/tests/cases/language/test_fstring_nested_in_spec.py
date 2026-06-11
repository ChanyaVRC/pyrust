# Regression for issue #2356: an f-string literal nested inside a format
# spec's replacement field (e.g. f"{v:{f'{w}.{p}'}f}") must lex correctly.
# Previously the lexer's nested-spec expression collector only tracked
# paren/bracket depth, so a `}` inside the nested string literal terminated
# the field early and raised "single '}' in f-string".
#
# CPython 3.12 (PEP 701) also allows reusing the same quote in nested
# f-strings; both same- and different-quote nestings are covered here.

w = 10
p = 3
v = 3.14159
s = "x"

# --- Headline case from the issue: nested f-string in spec, different quotes ---
assert f"{v:{f'{w}.{p}'}f}" == "     3.142"

# --- Plain dynamic spec (no nesting) still works ---
assert f"{v:{w}.{p}f}" == "     3.142"
assert f"{s:{'>'}{w}}" == "         x"

# --- Two nested f-strings in one spec ---
assert f"{v:{f'{w}'}.{f'{p}'}f}" == "     3.142"
n = 1
assert f"{v:{f'{w}'}.{f'{n}'}e}" == "   3.1e+00"

# --- Same-quote reuse (PEP 701, 3.12) ---
assert f"{w:{f"{w}"}d}" == "        10"

# --- Nested f-string carrying a conversion inside the spec ---
assert f"{v:{f'{w!r}'}}" == "   3.14159"

# --- Conversion (!s) applied to the nested field itself ---
assert f"{v:>{f'{p}'!s}}" == "3.14159"

# --- Deep nesting: f-string inside f-string inside the spec, mixed quotes ---
assert f"{v:{f'.{f"{p}"}'}f}" == "3.142"

# --- Nested precision built from an f-string ---
assert f"{v:.{f'{p}'}f}" == "3.142"

# --- A literal-brace-bearing string literal inside the field ---
assert f"{ '''a}b''' }" == "a}b"

print("fstring nested in spec OK")

# Dict/set literals as the nested replacement field (brace depth inside the
# nested expression must not terminate the field).
w = 5
print(f"{w:>{ {'a':3}['a'] }}")
print(f"{w:>{ {1,2,3}.__len__() }}")
print(f"{w:>{ {'}': 6}['}'] }}")
print(f"{w:>{ {'k': {'n': 4}}['k']['n'] }}")

# `!=` inside the nested field is the operator, not a conversion flag.
print(f"{w:{1 if w!=5 else 2}}")
print(f"{w:>{ {'a': 3 if w!=0 else 4}['a'] }}")
