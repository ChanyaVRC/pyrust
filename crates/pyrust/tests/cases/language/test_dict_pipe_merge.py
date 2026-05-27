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

# dict | dict subclass: right operand is a dict subclass, result is plain dict
class SubDict(dict):
    pass
sd = SubDict({"b": 2})
result = {"a": 1} | sd
print(result)                    # {'a': 1, 'b': 2}
print(type(result).__name__)     # dict

# dict subclass |= dict: in-place update preserves identity and type
sd2 = SubDict({"a": 1})
sd2_alias = sd2
sd2 |= {"b": 2}
print(dict(sd2))                 # {'a': 1, 'b': 2}
print(sd2 is sd2_alias)          # True
print(type(sd2).__name__)        # SubDict
