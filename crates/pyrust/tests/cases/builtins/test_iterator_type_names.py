# Iterator type names should match CPython 3.12
print(type(iter([1, 2, 3])).__name__)        # list_iterator
print(type(iter((1, 2, 3))).__name__)        # tuple_iterator
print(type(iter(range(5))).__name__)         # range_iterator
print(type(iter(lambda: 0, 1)).__name__)     # callable_iterator
print(type(iter({})).__name__)               # dict_keyiterator
print(type(iter({}.values())).__name__)      # dict_valueiterator
print(type(iter({}.items())).__name__)       # dict_itemiterator
print(type(iter(set())).__name__)            # set_iterator
print(type(iter(frozenset())).__name__)      # set_iterator (CPython uses set_iterator for frozenset too)
print(type(iter(b"abc")).__name__)           # bytes_iterator
print(type(iter("abc")).__name__)            # str_ascii_iterator
print(type(zip([], [])).__name__)            # zip
print(type(enumerate([])).__name__)          # enumerate
print(type(map(abs, [])).__name__)           # map
print(type(filter(None, [])).__name__)       # filter
print(type(reversed([])).__name__)           # list_reverseiterator


# Sequence-protocol (__getitem__) iterator
class SeqObj:
    def __getitem__(self, i):
        if i >= 3:
            raise IndexError
        return i


print(type(iter(SeqObj())).__name__)         # iterator
