# *rest in match/case sequence patterns always binds a list
match (1, 2, 3):
    case [first, *rest]:
        print(type(rest).__name__)  # list
        print(rest)                 # [2, 3]

match (1, 2, 3):
    case [*rest]:
        print(type(rest).__name__)  # list
        print(rest)                 # [1, 2, 3]

match (1, 2):
    case [a, b, *rest]:
        print(type(rest).__name__)  # list
        print(rest)                 # []

# Subject is already a list — should still work
match [4, 5, 6]:
    case [x, *rest]:
        print(type(rest).__name__)  # list
        print(rest)                 # [5, 6]

# Nested: rest from tuple, rest from list
match (10, 20, 30, 40):
    case [a, *mid, b]:
        print(type(mid).__name__)   # list
        print(mid)                  # [20, 30]
