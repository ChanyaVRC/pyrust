# Parity fixture: 'x in str' raises TypeError when x is not a string.
# CPython 3.12 reference: "'in <string>' requires string as left operand, not <type>"

try:
    x = 1 in "hello"
except TypeError as e:
    print(type(e).__name__, str(e))

try:
    x = [] in "hello"
except TypeError as e:
    print(type(e).__name__, str(e))

try:
    x = None in "hello"
except TypeError as e:
    print(type(e).__name__, str(e))

try:
    x = b"x" in "hello"
except TypeError as e:
    print(type(e).__name__, str(e))

# Happy path: string in string should work normally.
print("sub" in "hello")
print("he" in "hello")
print("xyz" in "hello")
