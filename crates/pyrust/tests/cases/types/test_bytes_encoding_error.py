# Check that bytes() reports the encoding argument type error before the
# source-not-a-string error (CPython 3.12 parity, issue #789).

# encoding is not a str — error mentions the actual type
try:
    bytes(1, 2, 3)
except TypeError as e:
    print(str(e))

# encoding is int, source also wrong — still reports encoding first
try:
    bytes(1, 2)
except TypeError as e:
    print(str(e))

# encoding is float
try:
    bytes(1, 2.5)
except TypeError as e:
    print(str(e))

# encoding is a str but source is not — "encoding without a string argument"
try:
    bytes(1, 'utf-8')
except TypeError as e:
    print(str(e))

# bytes source is not a str (only str triggers encoding-based conversion)
try:
    bytes(b'x', 'utf-8')
except TypeError as e:
    print(str(e))

# encoding is str, errors is not str
try:
    bytes('hello', 'utf-8', 3)
except TypeError as e:
    print(str(e))

# happy path — must still work
print(bytes('hello', 'utf-8'))
print(bytes('hello', 'ascii'))
