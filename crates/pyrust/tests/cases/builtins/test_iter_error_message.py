# Parity fixture: iter() error message when __iter__ returns a non-iterator.
# CPython 3.12: TypeError: iter() returned non-iterator of type '<typename>'


# Case 1: __iter__ returns a plain int (no __next__).
class BadIterInt:
    def __iter__(self):
        return 42


try:
    iter(BadIterInt())
except TypeError as e:
    print(str(e))

# Case 2: __iter__ returns a string (no __next__).
class BadIterStr:
    def __iter__(self):
        return "hello"


try:
    iter(BadIterStr())
except TypeError as e:
    print(str(e))

# Case 3: __iter__ returns None (no __next__).
class BadIterNone:
    def __iter__(self):
        return None


try:
    iter(BadIterNone())
except TypeError as e:
    print(str(e))

# Case 4: __iter__ returns a custom class instance without __next__ -- uses the class name.
class NotAnIter:
    pass


class BadIterCustom:
    def __iter__(self):
        return NotAnIter()


try:
    iter(BadIterCustom())
except TypeError as e:
    print(str(e))

# Case 5: Good path -- __iter__ returns an object that has __next__.
class GoodIter:
    def __next__(self):
        raise StopIteration


class GoodIterable:
    def __iter__(self):
        return GoodIter()


it = iter(GoodIterable())
print(type(it).__name__)

# Case 6: iter([]) -- no error (list has a valid built-in iterator).
it2 = iter([])
print(type(it2).__name__)

# Case 7: iter(42) -- non-iterable, different error (must stay unchanged).
try:
    iter(42)
except TypeError as e:
    print(str(e))

# Case 8: for-loop with __iter__ returning non-iterator triggers same error.
try:
    for _ in BadIterInt():
        pass
except TypeError as e:
    print(str(e))
