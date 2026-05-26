# str.format() field subscript accessor {field[key]} — issue #1169.
# CPython 3.12 passes keys as int when the subscript is a non-negative integer
# string, and as str for all other keys (non-numeric, or negative like "-1").

# --- str subscript ---
print("{0[0]}".format("hello"))     # 'h'
print("{0[1]}".format("hello"))     # 'e'

try:
    "{0[-1]}".format("hello")       # '-1' is passed as str -> TypeError
except TypeError as e:
    print(type(e).__name__, "str[-1]")

try:
    "{0[10]}".format("hello")       # out of range -> IndexError
except IndexError as e:
    print(type(e).__name__, "str[10]")

# --- bytes subscript ---
print("{0[0]}".format(b"hi"))       # 104 (int value of 'h')
print("{0[1]}".format(b"hi"))       # 105

try:
    "{0[5]}".format(b"hi")          # out of range -> IndexError
except IndexError as e:
    print(type(e).__name__, "bytes[5]")

# --- user object with __getitem__ ---
class Mapping:
    def __getitem__(self, key):
        return f"got:{type(key).__name__}:{key}"

# Non-numeric key -> passed as str
print("{0[foo]}".format(Mapping()))        # got:str:foo
# Non-negative integer key -> passed as int
print("{0[42]}".format(Mapping()))         # got:int:42
print("{0[0]}".format(Mapping()))          # got:int:0
# Negative-looking key -> passed as str
print("{0[-1]}".format(Mapping()))         # got:str:-1

# --- keyword field with subscript ---
print("{d[x]}".format(d={"x": "val"}))    # val

# --- list and tuple (existing, must not regress) ---
print("{0[0]}".format([10, 20, 30]))       # 10
print("{0[2]}".format((7, 8, 9)))          # 9

try:
    "{0[5]}".format([1, 2])                # IndexError
except IndexError as e:
    print(type(e).__name__, "list[5]")

# --- dict with int key ---
print("{0[0]}".format({0: "zero"}))        # zero

# --- regression: attribute access still works ---
class Obj:
    name = "Bob"

print("{0.name}".format(Obj()))            # Bob

# --- regression: basic positional and keyword ---
print("{}".format(42))                    # 42
print("{0}".format("hello"))              # hello
print("{name}".format(name="Alice"))      # Alice
