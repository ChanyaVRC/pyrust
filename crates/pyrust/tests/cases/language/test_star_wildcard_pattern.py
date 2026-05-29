# PEP 634: `*_` in a sequence pattern is a non-binding wildcard.
# The name `_` must NOT be bound after the match arm executes.

# --- bracket sequence: [*_] ---

match [1, 2, 3]:
    case [*_]:
        print("matched list")

# _ must not be in the local namespace
import sys
print("_ bound after [*_]:", "_" in dir())

# --- bracket sequence: [x, *_] binds x but not _ ---

match [1, 2, 3]:
    case [x, *_]:
        print("x =", x)

print("_ bound after [x, *_]:", "_" in dir())

# --- *rest still binds when name is not _ ---

match [4, 5, 6]:
    case [*rest]:
        print("rest =", rest)

# --- paren tuple sequence: (*_,) ---

match (10, 20):
    case (*_,):
        print("matched tuple")

print("_ bound after (*_,):", "_" in dir())

# --- paren tuple: (a, *_) ---

match (7, 8, 9):
    case (a, *_):
        print("a =", a)

print("_ bound after (a, *_):", "_" in dir())

# --- OR pattern: [*_] | [*_] is legal (both bind empty set) ---

match [99]:
    case [*_] | [*_]:
        print("or star wildcard matched")

# --- prefix + suffix with *_ in middle ---

match [1, 2, 3, 4]:
    case [first, *_, last]:
        print("first =", first, "last =", last)

print("_ bound after [first, *_, last]:", "_" in dir())

# --- empty slice case: [a, *_, b] on two-element list ---

match [10, 20]:
    case [a2, *_, b2]:
        print("a2 =", a2, "b2 =", b2)
