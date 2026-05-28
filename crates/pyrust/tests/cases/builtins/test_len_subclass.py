# Parity fixture for issue #1448:
# len() on container subclasses must dispatch __len__ via MRO, not read
# the backing data length directly.

# --- Subclasses with __len__ overrides ---

class MyList(list):
    def __len__(self):
        return 99

x = MyList([1, 2, 3])
print(len(x))  # 99

class MyDict(dict):
    def __len__(self):
        return 42

d = MyDict({'a': 1})
print(len(d))  # 42

class MyTuple(tuple):
    def __len__(self):
        return 7

t = MyTuple((1, 2))
print(len(t))  # 7

# --- Subclass without __len__ override: falls back to backing data ---

class PlainList(list):
    pass

print(len(PlainList([1, 2, 3])))  # 3

# --- Plain builtin: no regression ---

print(len([1, 2, 3]))  # 3
print(len({'a': 1, 'b': 2}))  # 2
print(len((10, 20)))  # 2

# --- __len__ returning 0: bool() should see it as falsy ---

class ZeroLen(list):
    def __len__(self):
        return 0

print(bool(ZeroLen([1, 2, 3])))  # False

# --- __len__ returning negative: ValueError ---

class NegLen(list):
    def __len__(self):
        return -1

try:
    len(NegLen([1]))
except ValueError as e:
    print(type(e).__name__, str(e))

# --- __len__ returning non-int: TypeError ---

class BadLen(list):
    def __len__(self):
        return "oops"

try:
    len(BadLen([1]))
except TypeError as e:
    print(type(e).__name__)
