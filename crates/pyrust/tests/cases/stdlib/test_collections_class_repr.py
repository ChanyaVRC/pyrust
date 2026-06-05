# Parity fixture for issue #2228.
#
# The public collections classes must carry __module__ == "collections" so the
# type repr renders "<class 'collections.Counter'>" etc., matching CPython.
# namedtuple-created classes keep the *caller's* module (here __main__), not
# "collections".

from collections import (
    Counter,
    defaultdict,
    OrderedDict,
    deque,
    ChainMap,
    UserDict,
    UserList,
    UserString,
    namedtuple,
)

for cls in [
    Counter,
    defaultdict,
    OrderedDict,
    deque,
    ChainMap,
    UserDict,
    UserList,
    UserString,
]:
    print(repr(cls), cls.__module__)

# namedtuple takes the defining module of the caller, not collections.
NT = namedtuple('NT', 'a b')
print(repr(NT), NT.__module__)

# Instances and isinstance/type relationships are unchanged.
print(repr(Counter(a=1)))
print(repr(OrderedDict(a=1)))
print(repr(deque([1, 2])))
print(isinstance(Counter(), dict), isinstance(OrderedDict(), dict))
print(Counter.__name__, OrderedDict.__name__, deque.__name__)
