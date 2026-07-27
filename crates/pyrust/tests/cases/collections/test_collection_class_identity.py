# Native collection operations must use stable class identity, not the mutable
# Python-visible class name. Proper subclasses remain valid even after rename,
# while unrelated classes with a matching name must not acquire native storage
# semantics.

from collections import Counter as NativeCounter
from collections import deque as NativeDeque


class CounterChild(NativeCounter):
    pass


CounterChild.__name__ = "RenamedCounter"
counter_sum = NativeCounter(a=2) + CounterChild(a=3)
print("counter subclass:", counter_sum["a"])

# Counter is implemented in Python in CPython and its class name is writable.
# Renaming the canonical class must not change its native binary operations.
NativeCounter.__name__ = "RenamedNativeCounter"
counter_sum = NativeCounter(a=4) + NativeCounter(a=5)
print("counter canonical rename:", counter_sum["a"])


class Counter:
    pass


counter_lookalike = Counter()
counter_lookalike.__builtin_data__ = {"a": 100}
try:
    NativeCounter(a=1) + counter_lookalike
except TypeError:
    print("counter lookalike: rejected")
else:
    print("counter lookalike: accepted")


class DequeChild(NativeDeque):
    pass


DequeChild.__name__ = "RenamedDeque"
print("deque subclass equal:", NativeDeque([1, 2]) == DequeChild([1, 2]))
print("deque subclass add:", list(NativeDeque([1]) + DequeChild([2, 3])))


class deque:
    pass


deque_lookalike = deque()
print("deque lookalike equal:", NativeDeque() == deque_lookalike)
try:
    NativeDeque() + deque_lookalike
except TypeError:
    print("deque lookalike add: rejected")
else:
    print("deque lookalike add: accepted")
