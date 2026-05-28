# dict.fromkeys classmethod subclass dispatch — issue #1563
# When called on a dict subclass, fromkeys must return an instance of the
# subclass, not a plain dict.

class MyDict(dict):
    pass

# Basic subclass dispatch — return type should be MyDict
result = MyDict.fromkeys(["a", "b"])
print(type(result).__name__)
print(result)

# Explicit value
result2 = MyDict.fromkeys(["x"], 42)
print(type(result2).__name__)
print(result2)

# Plain dict.fromkeys still returns a plain dict
plain = dict.fromkeys(["a", "b"])
print(type(plain).__name__)
print(plain)

# Duplicate keys: first-occurrence order, no duplication
result3 = MyDict.fromkeys(["a", "b", "a"])
print(type(result3).__name__)
print(result3)

# Empty iterable
result4 = MyDict.fromkeys([])
print(type(result4).__name__)
print(result4)

# Generator iterable
result5 = MyDict.fromkeys(x for x in ["a", "b"])
print(type(result5).__name__)
print(result5)

# Deep subclass (grandchild of dict)
class DeepDict(MyDict):
    pass

result6 = DeepDict.fromkeys(["x", "y"])
print(type(result6).__name__)
print(result6)

# Error cases use subclass name in messages
try:
    MyDict.fromkeys(["a"], value=0)
except TypeError as e:
    print(type(e).__name__, e)

try:
    MyDict.fromkeys()
except TypeError as e:
    print(type(e).__name__, e)

try:
    MyDict.fromkeys([1], 2, 3)
except TypeError as e:
    print(type(e).__name__, e)

# Unhashable key raises TypeError
try:
    MyDict.fromkeys([[1, 2]])
except TypeError as e:
    print(type(e).__name__, e)

# Instance call: MyDict().fromkeys(...) should also return MyDict (CPython classmethod semantics)
result7 = MyDict().fromkeys(["a", "b"])
print(type(result7).__name__)
print(result7)

# Instance call with explicit value
result8 = MyDict().fromkeys(["x"], 7)
print(type(result8).__name__)
print(result8)

# Plain dict instance call still returns dict
result9 = {}.fromkeys(["a", "b"])
print(type(result9).__name__)
print(result9)
