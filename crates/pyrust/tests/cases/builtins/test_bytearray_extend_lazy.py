# bytearray.extend must accept any iterable, including lazy iterators
# (map / filter / generator expressions / user generators), not just eagerly
# materialised sequences and `iter([...])` (issue #2532, sibling of #2522).
# Each element is still range-checked (range(0, 256)) and must be an int.

# --- generator expression ---
ba = bytearray(b"\x00")
ba.extend(x for x in [1, 2, 3])
print(ba)  # bytearray(b'\x00\x01\x02\x03')

# --- map object ---
ba = bytearray()
ba.extend(map(lambda x: x, [65, 66, 67]))
print(ba)  # bytearray(b'ABC')

# --- filter object ---
ba = bytearray()
ba.extend(filter(lambda x: x > 1, [1, 2, 3]))
print(ba)  # bytearray(b'\x02\x03')


# --- user generator function ---
def g():
    yield 10
    yield 20
    yield 30


ba = bytearray()
ba.extend(g())
print(list(ba))  # [10, 20, 30]

# --- range (lazy) ---
ba = bytearray()
ba.extend(range(3))
print(ba)  # bytearray(b'\x00\x01\x02')

# --- empty lazy iterator leaves the receiver unchanged ---
ba = bytearray(b"\xff")
ba.extend(x for x in [])
print(ba)  # bytearray(b'\xff')

# --- NativeIterFrame fast path (iter([...])) still works AND is exhausted ---
it = iter([4, 5, 6])
ba = bytearray()
ba.extend(it)
print(ba, list(it))  # bytearray(b'\x04\x05\x06') []

# --- eager containers still work (regression guard) ---
ba = bytearray()
ba.extend([7, 8])
ba.extend((9, 10))
ba.extend(b"\x0b")
ba.extend(bytearray(b"\x0c"))
print(ba)  # bytearray(b'\x07\x08\t\n\x0b\x0c')

# --- set materialises through its iteration protocol ---
ba = bytearray()
ba.extend({1, 2, 3})
print(sorted(ba))  # [1, 2, 3]


# --- bytearray subclass extends through its backing buffer ---
class BA(bytearray):
    pass


b = BA(b"\x01")
b.extend(map(lambda x: x, [2, 3]))
print(type(b).__name__, b)  # BA BA(b'\x01\x02\x03')

# --- out-of-range int yielded lazily raises ValueError ---
ba = bytearray()
try:
    ba.extend(x for x in [1, 300])
except ValueError as e:
    print("ValueError:", e)  # byte must be in range(0, 256)

# --- non-int element yielded lazily raises TypeError ---
ba = bytearray()
try:
    ba.extend(x for x in [1, "z"])
except TypeError as e:
    print("TypeError:", e)  # 'str' object cannot be interpreted as an integer

# --- non-iterable argument still raises the bytearray-specific TypeError ---
ba = bytearray()
try:
    ba.extend(42)
except TypeError as e:
    print("TypeError:", e)  # can't extend bytearray with int

# --- wrong argument count matches CPython's wording ---
try:
    bytearray().extend()
except TypeError as e:
    print(e)  # bytearray.extend() takes exactly one argument (0 given)
try:
    bytearray().extend(b"\x01", b"\x02")
except TypeError as e:
    print(e)  # bytearray.extend() takes exactly one argument (2 given)


# --- a generator that re-enters extend on the same generator raises the
#     CPython "generator already executing" ValueError ---
def selfconsume():
    yield 1
    out.extend(gen)
    yield 2


gen = selfconsume()
out = bytearray()
try:
    out.extend(gen)
    print(out)
except ValueError as e:
    print("ValueError:", e)  # generator already executing
