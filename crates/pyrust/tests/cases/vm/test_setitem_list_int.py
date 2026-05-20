# Parity fixture for the SetItem list[int] fast path (#868).
# Covers: positive index, negative index, out-of-bounds (both directions),
# empty-list assignment, slice assignment (slow path), dict assignment (slow
# path), and user __setitem__ dispatch (slow path).

# -- positive index --
xs = [1, 2, 3]
xs[0] = 99
print(xs)  # [99, 2, 3]

xs = [1, 2, 3]
xs[2] = 77
print(xs)  # [1, 2, 77]

# -- negative index (normalisation) --
xs = [1, 2, 3]
xs[-1] = 99
print(xs)  # [1, 2, 99]

xs = list(range(10))
xs[-len(xs)] = 999
print(xs)  # [999, 1, 2, 3, 4, 5, 6, 7, 8, 9]

# -- out-of-bounds: positive --
xs = [1, 2, 3]
try:
    xs[5] = 0
except IndexError as e:
    print(f'IndexError: {e}')

# -- out-of-bounds: negative --
xs = [1, 2, 3]
try:
    xs[-5] = 0
except IndexError as e:
    print(f'IndexError: {e}')

# -- empty list --
xs = []
try:
    xs[0] = 0
except IndexError as e:
    print(f'IndexError: {e}')

# -- slice assignment (must still work through slow path) --
xs = [1, 2, 3]
xs[0:2] = [7, 8]
print(xs)  # [7, 8, 3]

# -- dict assignment (slow path) --
d = {'a': 1}
d['b'] = 2
print(d)  # {'a': 1, 'b': 2}

# -- user __setitem__ dispatch (slow path) --
class Recorder:
    def __setitem__(self, key, val):
        print(f'set({key!r}, {val!r})')

r = Recorder()
r[0] = 42
r['x'] = 'hello'

# -- correctness under mutation loop (the original repro) --
def insertion_sort(xs):
    for i in range(1, len(xs)):
        key = xs[i]
        j = i - 1
        while j >= 0 and xs[j] > key:
            xs[j + 1] = xs[j]
            j -= 1
        xs[j + 1] = key
    return xs

print(insertion_sort([3, 1, 4, 1, 5, 9, 2, 6]))
