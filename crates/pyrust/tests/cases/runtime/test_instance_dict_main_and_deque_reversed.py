# Issue #2957: reversed(deque) mutation guards, instance __dict__ dict/reversed
# parity, and sys.modules['__main__'] registration and live namespace identity.
import copy
import operator
import sys
from collections import deque
from collections.abc import Mapping, MutableMapping

# --- 1. reversed(deque(...)) mutation guards ---------------------------------
mutators = [
    ("append", lambda d: d.append(9)),
    ("appendleft", lambda d: d.appendleft(9)),
    ("pop", lambda d: d.pop()),
    ("popleft", lambda d: d.popleft()),
    ("clear", lambda d: d.clear()),
    ("rotate", lambda d: d.rotate(1)),
    ("extend", lambda d: d.extend([9])),
    ("extendleft", lambda d: d.extendleft([9])),
    ("remove", lambda d: d.remove(2)),
    ("insert", lambda d: d.insert(1, 9)),
    ("delitem", lambda d: d.__delitem__(0)),
]

for name, mutate in mutators:
    dq = deque([1, 2, 3])
    it = reversed(dq)
    first = next(it)
    assert first == 3, (name, first)
    mutate(dq)
    try:
        next(it)
        raise AssertionError(f"reversed(deque) did not raise after {name}")
    except RuntimeError as exc:
        assert str(exc) == "deque mutated during iteration", (name, str(exc))

# Non-structural deque operations do not trigger the mutation guard in CPython.
for name, mutate in [
    ("reverse", lambda d: d.reverse()),
    ("setitem", lambda d: d.__setitem__(0, 99)),
]:
    dq = deque([1, 2, 3])
    it = reversed(dq)
    assert next(it) == 3
    mutate(dq)
    assert next(it) == 2

# Normal reverse walk on deque still completes cleanly.
assert list(reversed(deque([10, 20, 30]))) == [30, 20, 10]
assert list(reversed(deque())) == []


# --- 2. instance.__dict__ / vars(instance) dict & reversed parity ------------
class Sample:
    pass


obj = Sample()
obj.x = 1
obj.y = 2
obj.z = 3

assert isinstance(obj.__dict__, dict)
assert isinstance(vars(obj), dict)
assert isinstance(obj.__dict__, Mapping)
assert isinstance(obj.__dict__, MutableMapping)
assert type(obj.__dict__) is dict
assert type(vars(obj)) is dict
assert vars(obj) is obj.__dict__
assert id(vars(obj)) == id(obj.__dict__)

# Reversed iteration forms and iterator type name
rev_dict = reversed(obj.__dict__)
assert type(rev_dict).__name__ == "dict_reversekeyiterator"
assert operator.length_hint(rev_dict) == 3
assert list(rev_dict) == ["z", "y", "x"]
assert list(reversed(vars(obj))) == ["z", "y", "x"]
assert list(obj.__dict__.__reversed__()) == ["z", "y", "x"]

# Empty instance __dict__
empty_obj = Sample()
assert list(reversed(empty_obj.__dict__)) == []

# Value mutation (same size) is allowed during reverse walk
it_val = reversed(obj.__dict__)
assert next(it_val) == "z"
obj.x = 99
assert list(it_val) == ["y", "x"]

# Size growth during reverse walk raises RuntimeError and latches permanently
it_grow = reversed(obj.__dict__)
assert next(it_grow) == "z"
obj.w = 4
assert operator.length_hint(it_grow) == 0
try:
    next(it_grow)
    raise AssertionError("expected RuntimeError on size growth")
except RuntimeError as exc:
    assert str(exc) == "dictionary changed size during iteration"
del obj.w
try:
    next(it_grow)
    raise AssertionError("expected latched RuntimeError even after restoring size")
except RuntimeError as exc:
    assert str(exc) == "dictionary changed size during iteration"

# Size shrink during reverse walk also raises RuntimeError
it_shrink = reversed(obj.__dict__)
assert next(it_shrink) == "z"
del obj.y
try:
    next(it_shrink)
    raise AssertionError("expected RuntimeError on size shrink")
except RuntimeError as exc:
    assert str(exc) == "dictionary changed size during iteration"
obj.y = 2

# Independent reverse iterators and copy/reduce support
it_a = reversed(obj.__dict__)
it_b = reversed(obj.__dict__)
assert next(it_a) == "y"
it_a_copy = copy.copy(it_a)
assert list(it_a_copy) == ["z", "x"]
assert list(it_a) == ["z", "x"]
assert list(it_b) == ["y", "z", "x"]

# Dict merge operator with instance __dict__
assert (obj.__dict__ | {"extra": 5}) == {"x": 99, "z": 3, "y": 2, "extra": 5}
assert ({"extra": 5} | obj.__dict__) == {"extra": 5, "x": 99, "z": 3, "y": 2}


# --- 3. sys.modules['__main__'] registration & live namespace identity -------
top_before = 42

assert "__main__" in sys.modules
main_mod = sys.modules["__main__"]
assert main_mod.__name__ == "__main__"
assert main_mod.__dict__ is globals()
assert vars(main_mod) is globals()
assert sys._getframe().f_globals is globals()
assert sys._getframe().f_locals is globals()

import __main__

assert __main__ is main_mod
assert __main__.__dict__ is globals()

# Bidirectional live visibility between top-level globals and __main__ attrs
assert main_mod.top_before == 42
assert "top_before" in dir(main_mod)
top_before = 43
assert main_mod.top_before == 43

top_after = "live"
assert __main__.top_after == "live"

main_mod.injected_from_mod = 99
assert injected_from_mod == 99

from __main__ import injected_from_mod as imported_val

assert imported_val == 99

del main_mod.injected_from_mod
assert "injected_from_mod" not in globals()
assert not hasattr(main_mod, "injected_from_mod")

print("issue 2957 parity ok")
