# Parity fixture for issue #1448:
# len() on a container subclass must call the user-defined __len__ override
# rather than falling back directly to the backing primitive's length.

# --- Subclass overrides ---

class MyList(list):
    def __len__(self):
        return 99

x = MyList([1, 2, 3])
print(len(x))   # 99

class MyDict(dict):
    def __len__(self):
        return 42

d = MyDict({'a': 1})
print(len(d))   # 42

class MyTuple(tuple):
    def __len__(self):
        return 7

t = MyTuple((1, 2))
print(len(t))   # 7

class MyStr(str):
    def __len__(self):
        return 5

s = MyStr("hi")
print(len(s))   # 5

# --- Plain subclass (no __len__ override) still falls back to backing data ---

class PlainList(list):
    pass

pl = PlainList([1, 2, 3])
print(len(pl))  # 3

class PlainDict(dict):
    pass

pd = PlainDict({'a': 1, 'b': 2})
print(len(pd))  # 2

# --- Plain non-subclass containers are unaffected ---

print(len([1, 2, 3]))       # 3
print(len({'a': 1}))        # 1
print(len((1, 2, 3, 4)))    # 4
print(len("hello"))         # 5

# --- __len__ returning non-int raises TypeError ---

class BadLen:
    def __len__(self):
        return "not an int"

try:
    len(BadLen())
except TypeError as e:
    print(type(e).__name__)  # TypeError

# --- __len__ returning negative raises ValueError ---

class NegLen:
    def __len__(self):
        return -1

try:
    len(NegLen())
except ValueError as e:
    print(type(e).__name__)  # ValueError
