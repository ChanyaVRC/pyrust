# Parity fixture: `in` operator with a custom __iter__/__next__ class
# that raises StopIteration as a real exception instance.
#
# The __iter__ path in eval_binary (In) must handle PyError::Raised
# StopIteration the same way the __getitem__ path does.  Without the fix
# the `in` operator propagated the exception instead of returning False.

class Seq:
    def __init__(self, items):
        self._items = items
        self._idx = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self._idx >= len(self._items):
            raise StopIteration
        v = self._items[self._idx]
        self._idx += 1
        return v


s1 = Seq([1, 2, 3])
print(2 in s1)     # True

s2 = Seq([1, 2, 3])
print(5 in s2)     # False

# Re-entering after exhaustion should still work correctly.
s3 = Seq([10])
print(10 in s3)    # True
s3_again = Seq([10])
print(99 in s3_again)  # False

# Membership in an empty sequence
empty = Seq([])
print(1 in empty)  # False

# StopIteration raised via StopIteration() (with parens) — same behaviour.
class SeqParen:
    def __init__(self, items):
        self._items = items
        self._idx = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self._idx >= len(self._items):
            raise StopIteration()
        v = self._items[self._idx]
        self._idx += 1
        return v


sp = SeqParen([7, 8, 9])
print(8 in sp)    # True
sp2 = SeqParen([7, 8, 9])
print(6 in sp2)   # False
