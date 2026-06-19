# int.from_bytes accepts any bytes-like object (bytes, bytearray) or any
# iterable of ints in 0..=255, on both the class-method and bound-instance
# forms (issue #2624).

# bytes (the always-supported case)
print(int.from_bytes(b"\x01\x00", "big"))
print(int.from_bytes(b"\x01\x00", "little"))

# bytearray
print(int.from_bytes(bytearray(b"\x01\x00"), "big"))
print(int.from_bytes(bytearray(b"\x01\x00"), "little"))

# iterables of ints
print(int.from_bytes([1, 0], "big"))
print(int.from_bytes((255, 1), "little"))
print(int.from_bytes(range(3), "big"))
print(int.from_bytes((x for x in [1, 2]), "big"))
# dict iterates its keys
print(int.from_bytes({1: "a", 0: "b"}, "big"))

# user-defined iterable
class MyIter:
    def __iter__(self):
        return iter([1, 0])
print(int.from_bytes(MyIter(), "big"))

# bytes subclass
class MyBytes(bytes):
    pass
print(int.from_bytes(MyBytes(b"\x01\x00"), "big"))

# empty sources -> 0
print(int.from_bytes([], "big"))
print(int.from_bytes(bytearray(), "big"))
print(int.from_bytes(range(0), "big"))

# signed handling
print(int.from_bytes(bytearray(b"\xff\x00"), "big", signed=True))
print(int.from_bytes([255, 255], "big", signed=True))
print(int.from_bytes([255, 255], "little", signed=True))

# big value promotes to bigint
print(int.from_bytes(bytearray(b"\xff" * 16), "big"))

# keyword forms
print(int.from_bytes(bytes=bytearray(b"\x01\x00"), byteorder="big"))

# bound-instance form (classmethod called on an int instance)
print((0).from_bytes(bytearray(b"\x01\x00"), "big"))
print((0).from_bytes([1, 0], "big"))

# --- error paths -----------------------------------------------------------

# str source: rejected with the buffer-protocol message (not iterated)
try:
    int.from_bytes("ab", "big")
except TypeError as e:
    print("str:", e)

# bare int / bool source: rejected (not a length count, unlike bytes())
try:
    int.from_bytes(42, "big")
except TypeError as e:
    print("int:", e)
try:
    int.from_bytes(True, "big")
except TypeError as e:
    print("bool:", e)

# element out of range
try:
    int.from_bytes([1, 256], "big")
except ValueError as e:
    print("range-hi:", e)
try:
    int.from_bytes([1, -1], "big")
except ValueError as e:
    print("range-lo:", e)

# non-int element
try:
    int.from_bytes([1, 2.5], "big")
except TypeError as e:
    print("float-elem:", e)

# list containing a str element (per-element message, not buffer message)
try:
    int.from_bytes(["a"], "big")
except TypeError as e:
    print("str-elem:", e)

# bad byteorder
try:
    int.from_bytes(bytearray(b"\x01"), "middle")
except ValueError as e:
    print("byteorder:", e)
