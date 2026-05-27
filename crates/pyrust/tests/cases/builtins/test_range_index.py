class MyIndex:
    def __index__(self):
        return 5


class BigIndex:
    def __index__(self):
        return 10


class BadIndex:
    def __index__(self):
        return "not an int"


class NoIndex:
    pass


# Single argument
print(repr(range(MyIndex())))

# Two arguments
print(repr(range(MyIndex(), BigIndex())))

# Three arguments
print(repr(range(0, 10, MyIndex())))

# start and step via __index__
print(repr(range(MyIndex(), 20, MyIndex())))

# list() over __index__-based range
print(list(range(MyIndex())))

# Bool is an int subtype: must still work
print(repr(range(True, 5, True)))

# int and bool literals: regression guard
print(repr(range(3)))
print(repr(range(1, 4)))
print(repr(range(0, 10, 2)))

# __index__ returning a non-int raises TypeError
try:
    range(BadIndex())
except TypeError as e:
    print(type(e).__name__, e)

# No __index__ method raises TypeError
try:
    range(NoIndex())
except TypeError as e:
    print(type(e).__name__, e)

# str has no __index__
try:
    range("abc")
except TypeError as e:
    print(type(e).__name__, e)

# None has no __index__
try:
    range(None)
except TypeError as e:
    print(type(e).__name__, e)

# step=0 is still ValueError even with __index__
class Zero:
    def __index__(self):
        return 0

try:
    range(0, 10, Zero())
except ValueError as e:
    print(type(e).__name__, e)
