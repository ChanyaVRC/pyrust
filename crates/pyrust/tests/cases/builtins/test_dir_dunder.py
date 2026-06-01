# dir(obj) must honour a user-defined __dir__ (issue #1941).
# CPython 3.12: dir(obj) == sorted(type(obj).__dir__(obj)); it accepts any
# iterable, sorts via the elements' own comparison, and does not dedup.


# List result is sorted.
class Dir:
    def __dir__(self):
        return ['c', 'a', 'b']


print(dir(Dir()))


# Any iterable (iterator) is accepted.
class Dir2:
    def __dir__(self):
        return iter(['x', 'y'])


print(dir(Dir2()))


# Tuple result.
class DirTuple:
    def __dir__(self):
        return ('z', 'y')


print(dir(DirTuple()))


# Generator result.
class DirGen:
    def __dir__(self):
        yield 'm'
        yield 'n'
        yield 'm'


print(dir(DirGen()))


# Duplicates are NOT removed (unlike the default object path).
class DirDup:
    def __dir__(self):
        return ['b', 'a', 'a']


print(dir(DirDup()))


# Non-str elements are returned as-is when mutually comparable.
class DirInts:
    def __dir__(self):
        return [3, 1, 2]


print(dir(DirInts()))


# An inherited __dir__ is found through the MRO.
class Base:
    def __dir__(self):
        return ['inherited', 'attr']


class Child(Base):
    pass


print(dir(Child()))


# Non-iterable result raises TypeError.
class DirBad:
    def __dir__(self):
        return 5


try:
    dir(DirBad())
except TypeError as e:
    print('non-iterable:', e)


# Mutually-incomparable elements raise during the sort.
class DirMixed:
    def __dir__(self):
        return ['b', 1]


try:
    dir(DirMixed())
except TypeError as e:
    print('mixed:', e)


# A plain class without an override keeps the default attribute list.
class Plain:
    def __init__(self):
        self.x = 1


inst = Plain()
print('x' in dir(inst))
print('__class__' in dir(inst))
