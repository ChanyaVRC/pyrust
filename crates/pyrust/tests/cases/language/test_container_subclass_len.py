"""
Issue #1434: __len__ stubs for list/tuple/dict/set should work on subclasses.

When a container subclass instance (MyList, MyDict, MyTuple, MySet) is passed
to the corresponding __len__ stub, the backing primitive must be extracted from
__builtin_data__ and its length returned.  This is already done by the built-in
len() function; the __len__ stubs were missing the PyInstance arm.

CPython 3.12 reference output is shown in the prints below.
"""


class MyList(list):
    pass


class MyDict(dict):
    pass


class MyTuple(tuple):
    pass


class MySet(set):
    pass


# len() via __len__ stub — non-empty
print(len(MyList([1, 2])))         # 2
print(len(MyDict({'a': 1})))       # 1
print(len(MyTuple((1, 2, 3))))     # 3
print(len(MySet({1, 2})))          # 2

# bool() via __len__ stub — empty (falsy)
print(bool(MyList([])))            # False
print(bool(MyDict({})))            # False
print(bool(MyTuple(())))           # False
print(bool(MySet()))               # False

# bool() via __len__ stub — non-empty (truthy)
print(bool(MyList([1])))           # True
print(bool(MyDict({'a': 1})))      # True
print(bool(MyTuple((1,))))         # True
print(bool(MySet({1})))            # True

# Direct dunder call — list
print(list.__len__(MyList([1, 2, 3])))   # 3
print(list.__len__(MyList([])))           # 0

# Direct dunder call — dict
print(dict.__len__(MyDict({'x': 1, 'y': 2})))  # 2
print(dict.__len__(MyDict({})))                  # 0

# Direct dunder call — tuple
print(tuple.__len__(MyTuple((10, 20))))   # 2
print(tuple.__len__(MyTuple(())))         # 0

# Direct dunder call — set
print(set.__len__(MySet({1, 2, 3})))   # 3
print(set.__len__(MySet()))             # 0
