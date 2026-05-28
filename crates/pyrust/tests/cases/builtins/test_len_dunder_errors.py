# Parity fixture for issue #1555: __len__ stub error message format.
# Verifies that wrong-type and no-argument errors match CPython 3.12 exactly.

# --- no-argument errors ---
try:
    list.__len__()
except TypeError as e:
    print(f"TypeError: {e}")

try:
    tuple.__len__()
except TypeError as e:
    print(f"TypeError: {e}")

try:
    dict.__len__()
except TypeError as e:
    print(f"TypeError: {e}")

try:
    set.__len__()
except TypeError as e:
    print(f"TypeError: {e}")

try:
    bytes.__len__()
except TypeError as e:
    print(f"TypeError: {e}")

try:
    frozenset.__len__()
except TypeError as e:
    print(f"TypeError: {e}")

try:
    str.__len__()
except TypeError as e:
    print(f"TypeError: {e}")

# --- wrong-type errors ---
try:
    list.__len__(42)
except TypeError as e:
    print(f"TypeError: {e}")

try:
    tuple.__len__(42)
except TypeError as e:
    print(f"TypeError: {e}")

try:
    dict.__len__(42)
except TypeError as e:
    print(f"TypeError: {e}")

try:
    set.__len__(42)
except TypeError as e:
    print(f"TypeError: {e}")

try:
    bytes.__len__(42)
except TypeError as e:
    print(f"TypeError: {e}")

try:
    frozenset.__len__(42)
except TypeError as e:
    print(f"TypeError: {e}")

try:
    str.__len__(42)
except TypeError as e:
    print(f"TypeError: {e}")

# --- happy path: correct types work ---
print(list.__len__([1, 2, 3]))
print(tuple.__len__((1, 2)))
print(dict.__len__({"a": 1}))
print(set.__len__({1, 2, 3, 4}))
print(bytes.__len__(b"hello"))
print(frozenset.__len__(frozenset({1, 2})))
print(str.__len__("hello"))
