# Tests for the legacy sequence-iter protocol (#394): a class that
# defines `__getitem__` but no `__iter__` should still be iterable.
# CPython's `iter(obj)` falls back to calling `obj[0]`, `obj[1]`, ...
# until `IndexError` (or `StopIteration`) is raised.


# Basic for-loop.
class Seq:
    def __init__(self):
        self.items = ["a", "b", "c"]
    def __getitem__(self, i):
        return self.items[i]

result = []
for x in Seq():
    result.append(x)
print(result)                # ['a', 'b', 'c']


# list / tuple / set / frozenset constructors.
print(list(Seq()))           # ['a', 'b', 'c']
print(tuple(Seq()))          # ('a', 'b', 'c')
print(sorted(set(Seq())))    # ['a', 'b', 'c']
print(sorted(frozenset(Seq())))  # ['a', 'b', 'c']


# iter() / next() builtins.
it = iter(Seq())
print(next(it))              # a
print(next(it))              # b
print(next(it))              # c
try:
    next(it)
except StopIteration:
    print("StopIteration")   # StopIteration


# `in` operator (legacy __contains__ fallback).
print("a" in Seq())          # True
print("b" in Seq())          # True
print("z" in Seq())          # False


# Empty sequence.
class Empty:
    def __getitem__(self, i):
        raise IndexError

print(list(Empty()))         # []
print(tuple(Empty()))        # ()
print("x" in Empty())        # False


# StopIteration also terminates (CPython compat).
class StopAtTwo:
    def __getitem__(self, i):
        if i >= 2:
            raise StopIteration
        return i * 10

print(list(StopAtTwo()))     # [0, 10]


# __iter__ wins when both are present.
class Both:
    def __iter__(self):
        return iter([1, 2])
    def __getitem__(self, i):
        raise RuntimeError("should not be called")

print(list(Both()))          # [1, 2]


# Arbitrary indices: returns each call's return value.
class Arith:
    def __getitem__(self, i):
        if i >= 4:
            raise IndexError
        return i * i

print(list(Arith()))         # [0, 1, 4, 9]


# Non-IndexError exceptions propagate.
class BadSeq:
    def __getitem__(self, i):
        if i == 1:
            raise ValueError("boom")
        return i

try:
    list(BadSeq())
except ValueError as e:
    print("ValueError:", e)  # ValueError: boom


# Nested in comprehensions.
print([x * 2 for x in Seq()])  # ['aa', 'bb', 'cc']


# ─── Lazy semantics (#416 Copilot review) ───────────────────────────────
# next(iter(obj)) consumes only index 0 — later __getitem__ calls that
# would raise are never made.
class LazyOne:
    def __init__(self):
        self.called = []
    def __getitem__(self, i):
        self.called.append(i)
        if i == 0:
            return "first"
        raise RuntimeError("should not be called for i > 0")

lz = LazyOne()
it = iter(lz)
print(next(it))             # first
print(lz.called)            # [0]


# `break` in a for-loop stops further __getitem__ calls.
lz2 = LazyOne()
for x in lz2:
    print(x)                # first
    break
print(lz2.called)           # [0]


# `in` short-circuits on first match without invoking a later
# __getitem__ index that would raise.
class SeqWithBomb:
    def __getitem__(self, i):
        if i == 0:
            return "found"
        if i == 1:
            raise RuntimeError("bomb")
        raise IndexError

print("found" in SeqWithBomb())   # True


# Subclasses of IndexError / StopIteration terminate iteration.
class MyIE(IndexError):
    pass

class SubclassTerm:
    def __getitem__(self, i):
        if i < 3:
            return i
        raise MyIE

print(list(SubclassTerm()))   # [0, 1, 2]


# A user-defined class merely named "IndexError" (not a real subclass)
# does NOT terminate iteration — its raise propagates.  This guards
# against the previous name-match terminator.
class FakeIndexError(Exception):
    pass

class FakeNameSeq:
    def __getitem__(self, i):
        if i == 1:
            raise FakeIndexError("not a real IndexError")
        return i

try:
    list(FakeNameSeq())
except FakeIndexError as e:
    print("FakeIndexError propagates:", e)   # propagates
