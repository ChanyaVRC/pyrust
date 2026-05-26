# Parity fixture for reversed() __reversed__ protocol (issue #1163).
# CPython 3.12 checks __reversed__ first, then __len__+__getitem__,
# then raises TypeError.

# 1. __reversed__ takes priority over __iter__
class WithBoth:
    def __iter__(self):
        return iter([1, 2, 3])
    def __reversed__(self):
        return iter([100, 200, 300])

print(list(reversed(WithBoth())))

# 2. __reversed__ only (no __iter__)
class RevOnly:
    def __reversed__(self):
        return iter([10, 20, 30])

print(list(reversed(RevOnly())))

# 3. __iter__ only -> TypeError
class IterOnly:
    def __iter__(self):
        return iter([1, 2, 3])

try:
    list(reversed(IterOnly()))
except TypeError as e:
    print(e)

# 4. __len__ + __getitem__ sequence protocol
class SeqProto:
    def __len__(self):
        return 3
    def __getitem__(self, i):
        return [10, 20, 30][i]

print(list(reversed(SeqProto())))

# 5. list regression
print(list(reversed([1, 2, 3])))

# 6. tuple regression
print(list(reversed((1, 2, 3))))

# 7. range regression
print(list(reversed(range(4))))

# 8. No protocol at all
class Empty:
    pass

try:
    list(reversed(Empty()))
except TypeError as e:
    print(e)

# 9. __getitem__ only (no __len__) -> "no len()"
class GetItemOnly:
    def __getitem__(self, i):
        if i >= 3:
            raise IndexError
        return i

try:
    list(reversed(GetItemOnly()))
except TypeError as e:
    print(e)

# 10. __len__ only (no __getitem__) -> not reversible
class LenOnly:
    def __len__(self):
        return 3

try:
    list(reversed(LenOnly()))
except TypeError as e:
    print(e)
