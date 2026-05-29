# collections.abc — __instancecheck__, __subclasshook__, __subclasscheck__ dispatch.
#
# Verifies that ABC classes expose real, callable dunder methods that:
#   1. Are accessible via hasattr/getattr on ABC classes.
#   2. Are directly callable (Iterable.__instancecheck__(x)).
#   3. Enable issubclass() structural subtyping for user-defined classes
#      (fixes #1799).
#   4. Produce the same results as isinstance() / issubclass().

from collections.abc import (
    Callable, Container, Hashable, Iterable, Iterator,
    Sized, Sequence, Mapping, MutableMapping,
)

# ── hasattr checks ────────────────────────────────────────────────────────────

print(hasattr(Iterable, '__instancecheck__'))    # True
print(hasattr(Iterable, '__subclasshook__'))     # True
print(hasattr(Iterable, '__subclasscheck__'))    # True
print(hasattr(Hashable, '__instancecheck__'))    # True
print(hasattr(Callable, '__instancecheck__'))    # True
print(hasattr(Sequence, '__instancecheck__'))    # True

# ── callable() checks ─────────────────────────────────────────────────────────

print(callable(Iterable.__instancecheck__))    # True
print(callable(Iterable.__subclasshook__))     # True
print(callable(Iterable.__subclasscheck__))    # True

# ── User-defined classes for testing ─────────────────────────────────────────

class WithIter:
    def __iter__(self): return iter([])

class WithLen:
    def __len__(self): return 0

class WithCall:
    def __call__(self): pass

class WithIterAndNext:
    def __iter__(self): return self
    def __next__(self): raise StopIteration

class Empty:
    pass

# ── Iterable.__instancecheck__ — direct call ──────────────────────────────────

print(Iterable.__instancecheck__(WithIter()))   # True
print(Iterable.__instancecheck__(42))           # False
print(Iterable.__instancecheck__([]))           # True
print(Iterable.__instancecheck__({}))           # True

# ── Iterable.__subclasshook__ — direct call ───────────────────────────────────

print(Iterable.__subclasshook__(WithIter))      # True
print(Iterable.__subclasshook__(Empty))         # False
print(Iterable.__subclasshook__(int))           # False (int has no __iter__)

# ── Mapping.__subclasshook__ returns NotImplemented (no structural hook) ──────

print(Mapping.__subclasshook__(dict))           # NotImplemented
print(Mapping.__subclasshook__(WithIter))       # NotImplemented

# ── Mapping.__instancecheck__ still works via MRO fallback ───────────────────

print(Mapping.__instancecheck__({}))            # True (dict is Mapping via extra_bases)
print(Mapping.__instancecheck__([]))            # False

# ── issubclass with user-defined classes (fixes #1799) ───────────────────────

print(issubclass(WithIter, Iterable))          # True
print(issubclass(WithLen, Sized))              # True
print(issubclass(WithCall, Callable))          # True
print(issubclass(WithIterAndNext, Iterator))   # True
print(issubclass(Empty, Iterable))             # False
print(issubclass(int, Iterable))               # False (primitive, no __iter__)
print(issubclass(list, Iterable))              # True (registered via extra_bases)

# ── issubclass with __subclasscheck__ matches isinstance ─────────────────────

class Foo:
    def __iter__(self): return iter([])
    def __len__(self): return 0

print(isinstance(Foo(), Iterable))    # True
print(issubclass(Foo, Iterable))      # True
print(isinstance(Foo(), Sized))       # True
print(issubclass(Foo, Sized))         # True

# ── Hashable edge cases via __subclasscheck__ ─────────────────────────────────

class WithHash:
    def __hash__(self): return 42

class WithHashNone:
    __hash__ = None

print(issubclass(WithHash, Hashable))     # True
print(issubclass(WithHashNone, Hashable)) # False (__hash__ = None)
print(issubclass(Empty, Hashable))        # True (inherits object.__hash__)
