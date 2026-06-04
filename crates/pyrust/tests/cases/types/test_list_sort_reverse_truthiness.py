# list.sort(reverse=...) applies bool(reverse) to ANY object, matching
# sorted() and CPython 3.12 (issue #2126).  Previously only bool/int/float
# truthiness was honoured, so a truthy list/str/object silently failed to
# reverse.


def s(rev):
    lst = [3, 1, 2]
    lst.sort(reverse=rev)
    return lst


# Truthy non-numeric values reverse.
print(s([1]))        # [3, 2, 1]
print(s("x"))        # [3, 2, 1]
print(s(object()))   # [3, 2, 1]
print(s((0,)))       # [3, 2, 1]  (non-empty tuple is truthy)

# Falsy values do not reverse.
print(s([]))         # [1, 2, 3]
print(s(""))         # [1, 2, 3]
print(s(0))          # [1, 2, 3]
print(s(None))       # [1, 2, 3]
print(s(False))      # [1, 2, 3]

# Numeric truthiness still works.
print(s(1))          # [3, 2, 1]
print(s(True))       # [3, 2, 1]
print(s(0.0))        # [1, 2, 3]
print(s(2.5))        # [3, 2, 1]


# A user __bool__ on the reverse argument is honoured (interpreter dispatch).
class Truthy:
    def __bool__(self):
        return True


class Falsy:
    def __bool__(self):
        return False


print(s(Truthy()))   # [3, 2, 1]
print(s(Falsy()))    # [1, 2, 3]


# __len__ fallback when no __bool__.
class HasLen:
    def __init__(self, n):
        self.n = n

    def __len__(self):
        return self.n


print(s(HasLen(3)))  # [3, 2, 1]
print(s(HasLen(0)))  # [1, 2, 3]

# sorted() behaviour is unchanged (was already correct).
print(sorted([3, 1, 2], reverse=[1]))  # [3, 2, 1]
print(sorted([3, 1, 2], reverse=[]))   # [1, 2, 3]

# reverse with a key function.
lst = ["bb", "a", "ccc"]
lst.sort(key=len, reverse="yes")
print(lst)  # ['ccc', 'bb', 'a']
