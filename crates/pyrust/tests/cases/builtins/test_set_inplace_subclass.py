# Issue #1006: in-place set operators (|=, &=, -=, ^=) on a set subclass must
# preserve the subclass type.  CPython 3.12 set.__ior__ etc. mutate self and
# return self, so the variable keeps its original type after the augmented
# assignment.

class MySet(set):
    pass


# |= union
s = MySet({1, 2})
s |= {3}
print(type(s).__name__)   # MySet
print(sorted(s))          # [1, 2, 3]

# &= intersection
s = MySet({1, 2, 3})
s &= {1, 2}
print(type(s).__name__)   # MySet
print(sorted(s))          # [1, 2]

# -= difference
s = MySet({1, 2, 3})
s -= {1}
print(type(s).__name__)   # MySet
print(sorted(s))          # [2, 3]

# ^= symmetric difference
s = MySet({1, 2, 3})
s ^= {2, 4}
print(type(s).__name__)   # MySet
print(sorted(s))          # [1, 3, 4]

# RHS is also a set subclass
a = MySet({1, 2, 3})
b = MySet({3, 4})
a |= b
print(type(a).__name__)   # MySet
print(sorted(a))          # [1, 2, 3, 4]

# RHS is a frozenset — LHS type is still preserved
a = MySet({1, 2, 3})
a |= frozenset({4})
print(type(a).__name__)   # MySet
print(sorted(a))          # [1, 2, 3, 4]

a = MySet({1, 2, 3})
a &= frozenset({2, 3, 4})
print(type(a).__name__)   # MySet
print(sorted(a))          # [2, 3]

a = MySet({1, 2, 3})
a -= frozenset({1})
print(type(a).__name__)   # MySet
print(sorted(a))          # [2, 3]

a = MySet({1, 2, 3})
a ^= frozenset({2, 4})
print(type(a).__name__)   # MySet
print(sorted(a))          # [1, 3, 4]

# Wrong RHS type raises TypeError (same as plain set)
try:
    a = MySet({1, 2})
    a |= [3]
except TypeError:
    print("TypeError for list RHS")

# Plain set regression — type stays set
t = {1, 2}
t |= {3}
print(type(t).__name__)   # set
print(sorted(t))          # [1, 2, 3]

# User-defined __ior__ override is still dispatched
class MySet2(set):
    def __ior__(self, other):
        print("custom __ior__ called")
        set.update(self, other)
        return self

s2 = MySet2({1})
s2 |= {2}
print(type(s2).__name__)  # MySet2
print(sorted(s2))         # [1, 2]
