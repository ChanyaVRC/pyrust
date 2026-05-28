# Parity fixture for OR pattern name-binding validation (PEP 634).
# CPython 3.12 raises SyntaxError at compile time when the alternatives of a
# `p1 | p2` pattern bind different sets of names.

# --- Valid: same binding in each alternative ---

match [1, 2]:
    case [0, x] | [x, 0]:
        print(f"zero at edge: {x}")
    case _:
        print("no zero")

match (1, 2):
    case (0, x) | (x, 0):
        print(f"zero: {x}")
    case _:
        print("no zero")

# OR of two literals — no bindings in either arm, valid.
match 5:
    case 1 | 5:
        print("one or five")
    case _:
        print("other")

print("all valid patterns ok")

# --- Invalid: SyntaxError at compile time ---

import sys

code1 = "match [1,2]:\n    case [a, *rest] | 'hello':\n        print(a)\n"
try:
    compile(code1, "<test>", "exec")
    print("ERROR: should have raised SyntaxError")
except SyntaxError as e:
    print("SyntaxError1:", "alternative patterns" in str(e))

code2 = "match 42:\n    case (x, y) | z:\n        print(z)\n"
try:
    compile(code2, "<test>", "exec")
    print("ERROR: should have raised SyntaxError")
except SyntaxError as e:
    print("SyntaxError2:", "alternative patterns" in str(e))
