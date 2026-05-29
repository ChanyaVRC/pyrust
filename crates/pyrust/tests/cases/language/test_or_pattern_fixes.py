# Parity fixture for OR pattern runtime fallback (#1764) and nested
# unreachable-check (#1765).

# --- #1764: runtime OR pattern fallback on non-sequence subjects ---

# A sequence arm in an OR pattern must fall through to the next alternative
# when the subject has no __len__ (e.g. int, float, None), rather than
# propagating the TypeError from len().

match 1:
    case (x,) | x:
        print("int fallthrough:", x)  # expect: int fallthrough: 1

match 1.5:
    case [x] | x:
        print("float fallthrough:", x)  # expect: float fallthrough: 1.5

match None:
    case [x] | x:
        print("none fallthrough:", x)  # expect: none fallthrough: None

# Sequence arm succeeds when subject is a list.
match [42]:
    case (x,) | x:
        print("list success:", x)  # expect: list success: 42

# str is excluded from sequence patterns by the isinstance check; OR
# fallback must still work.
match "hi":
    case [x] | x:
        print("str fallthrough:", x)  # expect: str fallthrough: hi

# --- #1765: compile-time unreachable check recurses into nested OR ---

# The error "name capture 'x' makes remaining patterns unreachable" must be
# emitted for (x | 1) | z — the inner OR's first alternative x is a bare
# capture that makes the rest unreachable.


def check(src, expected_fragment):
    try:
        compile(src, "<test>", "exec")
        print(f"ERROR: no SyntaxError raised for: {src!r}")
    except SyntaxError as e:
        if expected_fragment in e.msg:
            print("ok")
        else:
            print(f"ERROR: expected {expected_fragment!r} in msg, got {e.msg!r}")


check(
    "match 1:\n    case (x | 1) | z:\n        pass",
    "name capture 'x' makes remaining patterns unreachable",
)

check(
    "match 1:\n    case (_ | 1) | z:\n        pass",
    "wildcard makes remaining patterns unreachable",
)

# Valid: (x, 1) | (1, x) — both alternatives bind the same name x and
# neither is a bare capture in a non-last position.
match (1, 2):
    case (x, 1) | (1, x):
        print("structured or:", x)
    case _:
        print("no match")

print("done")
