# Parity fixture: str.translate() with arbitrary mapping objects (#1488)
# CPython 3.12 calls table[codepoint] per character; KeyError / IndexError
# (both LookupError subclasses) keep the character unchanged; None deletes;
# int replaces with chr(n); str replaces with that string.


# --- Custom mapping via __getitem__ ---

class MyMapping:
    def __getitem__(self, key):
        if key == ord('a'):
            return ord('b')
        raise KeyError(key)

print("abc".translate(MyMapping()))  # bbc


# --- IndexError treated same as KeyError (keep char) ---

class IndexMapping:
    def __getitem__(self, key):
        if key == ord('x'):
            return ord('y')
        raise IndexError(key)

print("axb".translate(IndexMapping()))  # ayb


# --- LookupError base class keeps char ---

class LookupMapping:
    def __getitem__(self, key):
        if key == ord('b'):
            return None  # delete
        raise LookupError(key)

print("abc".translate(LookupMapping()))  # ac


# --- User-defined LookupError subclass keeps char ---

class MyLookup(LookupError):
    pass

class MyLookupMapping:
    def __getitem__(self, key):
        raise MyLookup("custom")

print("abc".translate(MyLookupMapping()))  # abc


# --- Multi-character string replacement ---

class StrMapping:
    def __getitem__(self, key):
        if key == ord('a'):
            return "XY"
        raise KeyError(key)

print("ab".translate(StrMapping()))  # XYb


# --- None deletes the character ---

print("axb".translate({ord('x'): None}))  # ab


# --- Dict fast path (regression: maketrans output) ---

t = str.maketrans("abc", "xyz")
print("abcdef".translate(t))  # xyzdef


# --- Non-LookupError propagates ---

class BadMapping:
    def __getitem__(self, key):
        raise ValueError("bad")

try:
    "abc".translate(BadMapping())
except ValueError as e:
    print(f"ValueError: {e}")  # ValueError: bad


# --- Non-subscriptable raises TypeError ---

try:
    "abc".translate(42)
except TypeError as e:
    print(e)  # 'int' object is not subscriptable


# --- Out-of-range int raises ValueError ---

class OutOfRange:
    def __getitem__(self, key):
        return 0x200000

try:
    "a".translate(OutOfRange())
except ValueError as e:
    print(e)  # character mapping must be in range(0x110000)


# --- Wrong return type raises TypeError ---

class WrongType:
    def __getitem__(self, key):
        return [1, 2]

try:
    "a".translate(WrongType())
except TypeError as e:
    print(e)  # character mapping must return integer, None or str


# --- Empty string returns empty string ---

class AnyMapping:
    def __getitem__(self, key):
        return ord('z')

print(repr("".translate(AnyMapping())))  # ''


# --- Bool return treated as int (subclass of int) ---

class BoolMapping:
    def __getitem__(self, key):
        return True  # chr(1) = SOH

result = "a".translate(BoolMapping())
print(ord(result))  # 1


# --- int subclass return is accepted ---

class MyInt(int):
    pass

class IntSubclassMapping:
    def __getitem__(self, key):
        return MyInt(65)  # ord('A')

result = "a".translate(IntSubclassMapping())
print(repr(result))  # 'A'


# --- str subclass return is accepted ---

class MyStr(str):
    pass

class StrSubclassMapping:
    def __getitem__(self, key):
        if key == ord('a'):
            return MyStr("XY")
        raise KeyError(key)

result = "ab".translate(StrSubclassMapping())
print(repr(result))  # 'XYb'


# --- BigInt (> i64::MAX) return raises ValueError, not TypeError ---

class BigIntOutOfRange:
    def __getitem__(self, key):
        return 2 ** 63  # Too large for any Unicode codepoint

try:
    "a".translate(BigIntOutOfRange())
except ValueError as e:
    print(f"ValueError: {e}")  # ValueError: character mapping must be in range(0x110000)
