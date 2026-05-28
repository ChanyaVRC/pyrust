"""
TypeError messages for set/frozenset binary operators with non-set RHS.

CPython 3.12 raises:
  TypeError: unsupported operand type(s) for OP: 'TYPE' and 'int'

where TYPE is the actual class name (including subclass names).

Covers:
- Plain set: |, &, -, ^
- Plain set in-place: |=, &=, -=, ^=
- set subclass: binary and in-place
- frozenset: binary ops
- frozenset subclass: binary ops
- Regression: set op set still works
"""


class MySet(set):
    pass


class MyFrozenSet(frozenset):
    pass


# ── Plain set binary ops ──────────────────────────────────────────────────────

try:
    {1, 2} | 42
except TypeError as e:
    print(e)

try:
    {1, 2} & 42
except TypeError as e:
    print(e)

try:
    {1, 2} - 42
except TypeError as e:
    print(e)

try:
    {1, 2} ^ 42
except TypeError as e:
    print(e)

# ── set subclass binary ops ───────────────────────────────────────────────────

s = MySet({1, 2})

try:
    s | 42
except TypeError as e:
    print(e)

try:
    s & 42
except TypeError as e:
    print(e)

try:
    s - 42
except TypeError as e:
    print(e)

try:
    s ^ 42
except TypeError as e:
    print(e)

# ── Plain set in-place ops ────────────────────────────────────────────────────

s = {1, 2}
try:
    s |= 42
except TypeError as e:
    print(e)

s = {1, 2}
try:
    s &= 42
except TypeError as e:
    print(e)

s = {1, 2}
try:
    s -= 42
except TypeError as e:
    print(e)

s = {1, 2}
try:
    s ^= 42
except TypeError as e:
    print(e)

# ── set subclass in-place ops ─────────────────────────────────────────────────

s = MySet({1, 2})
try:
    s |= 42
except TypeError as e:
    print(e)

s = MySet({1, 2})
try:
    s &= 42
except TypeError as e:
    print(e)

s = MySet({1, 2})
try:
    s -= 42
except TypeError as e:
    print(e)

s = MySet({1, 2})
try:
    s ^= 42
except TypeError as e:
    print(e)

# ── frozenset binary ops ──────────────────────────────────────────────────────

try:
    frozenset({1, 2}) | 42
except TypeError as e:
    print(e)

try:
    frozenset({1, 2}) & 42
except TypeError as e:
    print(e)

try:
    frozenset({1, 2}) - 42
except TypeError as e:
    print(e)

try:
    frozenset({1, 2}) ^ 42
except TypeError as e:
    print(e)

# ── frozenset subclass binary ops ────────────────────────────────────────────

try:
    MyFrozenSet({1}) | 42
except TypeError as e:
    print(e)

# ── Regression: set op set still works ───────────────────────────────────────

print(sorted({1, 2} | {3}))
print(sorted({1, 2} & {1}))
print(sorted({1, 2} - {1}))
print(sorted({1, 2} ^ {1, 3}))
print(sorted(MySet({1, 2}) | {3}))
print(sorted(MySet({1, 2}) & {1}))
print(sorted(MySet({1, 2}) - {1}))
print(sorted(MySet({1, 2}) ^ {1, 3}))
