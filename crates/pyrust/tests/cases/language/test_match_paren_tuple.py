# Parity test for parenthesised patterns in match/case (PEP 634).
# (a, b) → sequence pattern; (x) → grouping; () → empty sequence.

# --- Two-element sequence pattern ---
match (1, 2):
    case (a, b):
        print(a, b)
# 1 2

# --- Single-element with trailing comma (still a sequence, not a grouping) ---
match (42,):
    case (y,):
        print(y)
# 42

# --- Empty sequence pattern ---
match ():
    case ():
        print("empty")
# empty

# --- Grouping (no comma → plain pattern, not sequence) ---
match 10:
    case (z):
        print(z)
# 10

# --- Nested: tuple-in-tuple ---
match (1, (2, 3)):
    case (a, (b, c)):
        print(a, b, c)
# 1 2 3

# --- Mix: paren sequence vs bracket sequence (brackets tried first, no match, falls to parens) ---
match (1, 2):
    case [x, y]:
        print("bracket", x, y)
    case (x, y):
        print("paren", x, y)
# paren 1 2

# --- Bracket sequence still works (regression guard) ---
match [7, 8]:
    case [p, q]:
        print(p, q)
# 7 8

# --- Paren pattern falling through to next arm ---
matched = None
match (5, 6):
    case (1, 2):
        matched = "wrong"
    case (a, b):
        matched = (a, b)
print(matched)
# (5, 6)

# --- Guard on paren sequence ---
match (3, 4):
    case (x, y) if x + y == 7:
        print("sum7")
# sum7

# --- Three-element paren sequence ---
match (10, 20, 30):
    case (p, q, r):
        print(p, q, r)
# 10 20 30

# --- Paren sequence matching a list (sequence patterns match both) ---
match [1, 2]:
    case (a, b):
        print("list matched paren pattern", a, b)
# list matched paren pattern 1 2
