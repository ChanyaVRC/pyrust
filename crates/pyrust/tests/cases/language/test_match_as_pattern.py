# Tests for `case pattern as name:` (PEP 634 §7 AS patterns).
# The AS pattern binds the entire matched subject to `name`, not just the
# matched portion.  The inner pattern can be any pattern variant.

# ── Sequence as ───────────────────────────────────────────────────────────────
match [1, 2, 3]:
    case [1, *rest] as whole:
        print(whole)   # [1, 2, 3]
        print(rest)    # [2, 3]

# ── Class pattern as ──────────────────────────────────────────────────────────
match "hello":
    case str() as s:
        print(s)  # hello

# ── Capture as (two names, same value) ───────────────────────────────────────
match 42:
    case x as y:
        print(x, y)  # 42 42

# ── Wildcard as ───────────────────────────────────────────────────────────────
match 42:
    case _ as n:
        print(n)  # 42

# ── Literal as ────────────────────────────────────────────────────────────────
match 99:
    case 99 as n:
        print(n)  # 99

# ── Or-pattern as ─────────────────────────────────────────────────────────────
match "exit":
    case "quit" | "exit" as cmd:
        print(cmd)  # exit

# ── AS pattern: inner match fails, falls through to next arm ─────────────────
match [10, 20]:
    case [1, *rest] as whole:
        print("wrong arm")
    case [a, b] as pair:
        print(a, b, pair)  # 10 20 [10, 20]

# ── Nested: AS inside a sequence element ──────────────────────────────────────
match [True, 7]:
    case [True as flag, v]:
        print(flag, v)  # True 7

# ── Parenthesised sequence as ────────────────────────────────────────────────
match (3, 4):
    case (a, b) as t:
        print(a, b, t)  # 3 4 (3, 4)

# ── Guard with AS pattern ─────────────────────────────────────────────────────
match [5, 6]:
    case [x, y] as seq if x < y:
        print(seq)  # [5, 6]

# ── No match in AS arm ────────────────────────────────────────────────────────
match 0:
    case 1 as n:
        print("no")
    case _ as m:
        print(m)  # 0
