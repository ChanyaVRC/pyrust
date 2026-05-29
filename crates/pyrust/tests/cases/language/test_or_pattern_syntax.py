# Parity fixture for OR pattern SyntaxError diagnostics (PEP 634, issue #1731).
# CPython 3.12 distinguishes between:
#   - A bare name capture in a non-last alternative making subsequent patterns
#     unreachable ("name capture 'X' makes remaining patterns unreachable")
#   - A wildcard in a non-last alternative ("wildcard makes remaining patterns
#     unreachable")
#   - Alternatives binding genuinely different name-sets
#     ("alternative patterns bind different names")

import sys


def check(src, expected_fragment):
    try:
        compile(src, "<test>", "exec")
        print(f"ERROR: no SyntaxError raised for: {src!r}")
    except SyntaxError as e:
        if expected_fragment in e.msg:
            print("ok")
        else:
            print(f"ERROR: expected {expected_fragment!r} in msg, got {e.msg!r}")


# Bare name capture in first position — "name capture 'X' makes remaining
# patterns unreachable".
check(
    "match 1:\n    case x | y:\n        pass",
    "name capture 'x' makes remaining patterns unreachable",
)

# Same but with a different leading name.
check(
    "match 1:\n    case y | x:\n        pass",
    "name capture 'y' makes remaining patterns unreachable",
)

# Bare name capture in middle position (not last).
check(
    "match 1:\n    case 1 | x | y:\n        pass",
    "name capture 'x' makes remaining patterns unreachable",
)

# Bare name capture before a structured pattern.
check(
    "match 1:\n    case x | (x,):\n        pass",
    "name capture 'x' makes remaining patterns unreachable",
)

# Wildcard in first position — "wildcard makes remaining patterns unreachable".
check(
    "match 1:\n    case _ | y:\n        pass",
    "wildcard makes remaining patterns unreachable",
)

# Wildcard in non-last position after a literal.
check(
    "match 1:\n    case _ | 1:\n        pass",
    "wildcard makes remaining patterns unreachable",
)

# Genuine name-set mismatch (not a bare capture problem) — "alternative
# patterns bind different names".
check(
    "match [1, 2]:\n    case (x, 1) | (1, y):\n        pass",
    "alternative patterns bind different names",
)

# Structured pattern first, bare capture second — name-sets differ so the
# name-set check fires before the unreachable check.
check(
    "match 1:\n    case (x, 1) | z:\n        pass",
    "alternative patterns bind different names",
)

# --- Valid patterns that must NOT raise SyntaxError ---

# Both alternatives bind the same name.
match [1, 2]:
    case [0, x] | [x, 0]:
        print("zero at edge:", x)
    case _:
        print("no zero")

# Wildcard as the LAST alternative is fine (doesn't make anything unreachable).
match 5:
    case 1 | _:
        print("one or wildcard")

# Two structured alternatives with identical bindings.
match (1, 2):
    case (0, x) | (x, 0):
        print("zero:", x)
    case _:
        print("no zero")

print("done")
