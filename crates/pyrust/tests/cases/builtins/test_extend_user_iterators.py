# list.extend / bytearray.extend must accept user-defined iterator and iterable
# objects (the `__iter__`/`__next__` protocol and the legacy `__getitem__`
# protocol), not just built-in lazy iterators (issue #2534).


# --- user iterator object (__iter__ returns self, __next__ yields) ---
class Counter:
    def __init__(self, n):
        self.n = n
        self.i = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self.i >= self.n:
            raise StopIteration
        v = self.i
        self.i += 1
        return v


a = []
a.extend(Counter(3))
print(a)  # [0, 1, 2]

b = bytearray()
b.extend(Counter(3))
print(b)  # bytearray(b'\x00\x01\x02')


# --- iterable whose __iter__ returns a separate iterator ---
class Range3:
    def __iter__(self):
        return iter([10, 20, 30])


a = []
a.extend(Range3())
print(a)  # [10, 20, 30]

b = bytearray()
b.extend(Range3())
print(list(b))  # [10, 20, 30]


# --- legacy sequence protocol (__getitem__ with IndexError sentinel) ---
class Seq:
    def __getitem__(self, i):
        if i >= 3:
            raise IndexError
        return i * 5


a = [99]
a.extend(Seq())
print(a)  # [99, 0, 5, 10]

b = bytearray()
b.extend(Seq())
print(list(b))  # [0, 5, 10]


# --- regression guard: a plain str still extends char-by-char ---
a = []
a.extend("abc")
print(a)  # ['a', 'b', 'c']


# --- a non-iterable object raises the right TypeError per container ---
class Plain:
    pass


try:
    [].extend(Plain())
except TypeError as e:
    print("list:", e)  # 'Plain' object is not iterable

try:
    bytearray().extend(Plain())
except TypeError as e:
    print("bytearray:", e)  # can't extend bytearray with Plain


# --- user iterator yielding an out-of-range / non-int byte raises for bytearray ---
class BadVals:
    def __iter__(self):
        return iter([1, 300])


try:
    bytearray().extend(BadVals())
except ValueError as e:
    print("ValueError:", e)  # byte must be in range(0, 256)


class StrVals:
    def __iter__(self):
        return iter([1, "z"])


try:
    bytearray().extend(StrVals())
except TypeError as e:
    print("TypeError:", e)  # 'str' object cannot be interpreted as an integer


# --- subclass of a NON-iterable builtin (int/float/complex) is not iterable:
# bytearray.extend must report `can't extend bytearray with <type>`, not the
# `'int' object is not iterable` a blind collect would surface (review #2554).
class IntSub(int):
    pass


try:
    bytearray().extend(IntSub(5))
except TypeError as e:
    print("intsub:", e)  # can't extend bytearray with IntSub


class FloatSub(float):
    pass


try:
    bytearray().extend(FloatSub(1.0))
except TypeError as e:
    print("floatsub:", e)  # can't extend bytearray with FloatSub


# --- subclass of an ITERABLE builtin still extends through its buffer ---
class ListSub(list):
    pass


ba = bytearray()
ba.extend(ListSub([65, 66]))
print(ba)  # bytearray(b'AB')
