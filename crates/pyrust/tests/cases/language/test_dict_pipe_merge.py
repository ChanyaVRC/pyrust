# PEP 584 (Python 3.9+): dict | dict and dict |= dict operators.

# Basic merge: non-overlapping keys
d = {"a": 1} | {"b": 2}
print(d)  # {'a': 1, 'b': 2}

# Right operand wins on key collision
d = {"a": 1, "b": 2} | {"b": 99, "c": 3}
print(d)  # {'a': 1, 'b': 99, 'c': 3}

# Empty dicts
print({} | {})          # {}
print({} | {"a": 1})   # {'a': 1}
print({"a": 1} | {})   # {'a': 1}

# dict |= dict: in-place update, right wins on collision
d = {"a": 1}
d2 = d
d |= {"b": 2}
print(d)        # {'a': 1, 'b': 2}
print(d is d2)  # True — same object

# dict |= dict: right wins on duplicate key
d = {"k": 1}
d |= {"k": 2}
print(d)  # {'k': 2}

# dict |= iterable of pairs (same semantics as dict.update)
d = {"a": 1}
d |= [("b", 2), ("c", 3)]
print(d)  # {'a': 1, 'b': 2, 'c': 3}

# dict | non-dict raises TypeError
try:
    {"a": 1} | 5
except TypeError as e:
    print(e)  # unsupported operand type(s) for |: 'dict' and 'int'

try:
    {"a": 1} | "hello"
except TypeError as e:
    print(e)  # unsupported operand type(s) for |: 'dict' and 'str'

try:
    {"a": 1} | [1, 2]
except TypeError as e:
    print(e)  # unsupported operand type(s) for |: 'dict' and 'list'
