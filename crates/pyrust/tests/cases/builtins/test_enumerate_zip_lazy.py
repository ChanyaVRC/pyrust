# Parity fixture for issue #1408: enumerate() and zip() must be lazy iterators.
#
# Demonstrates that no items are consumed from the source at construction time;
# all consumption happens during iteration.  Non-ASCII characters are avoided
# to satisfy the parity harness constraint.

# -- enumerate: lazy proof via side-effect tracking --------------------------

consumed = []

def gen_a():
    for i in range(3):
        consumed.append(i)
        yield i

e = enumerate(gen_a())
print(consumed)          # [] -- nothing consumed yet

for idx, val in e:
    pass
print(consumed)          # [0, 1, 2] -- all consumed after iteration

# -- enumerate with start parameter ------------------------------------------

print(list(enumerate(['a', 'b', 'c'], start=1)))   # [(1, 'a'), (2, 'b'), (3, 'c')]
print(list(enumerate(['a', 'b', 'c'], 5)))          # [(5, 'a'), (6, 'b'), (7, 'c')]
print(list(enumerate([])))                           # []
print(list(enumerate([], start=99)))                 # []

# -- enumerate: partial iteration leaves source partially consumed ------------

side = []
def gen_b():
    for i in range(5):
        side.append(i)
        yield i

e2 = enumerate(gen_b())
print(side)              # []
next(e2)                 # consume one
print(len(side) == 1)    # True

# -- zip: lazy proof with infinite generator ---------------------------------

def inf():
    i = 0
    while True:
        yield i
        i += 1

z = zip(inf(), [1, 2, 3])
print(list(z))           # [(0, 1), (1, 2), (2, 3)]

# -- zip stops at shortest iterable ------------------------------------------

print(list(zip([1, 2, 3], [4, 5])))       # [(1, 4), (2, 5)]
print(list(zip([1, 2], [3, 4, 5])))       # [(1, 3), (2, 4)]
print(list(zip([1, 2, 3], [], [7, 8, 9])))  # []

# -- zip with no arguments ---------------------------------------------------

print(list(zip()))       # []

# -- zip: three iterables ----------------------------------------------------

print(list(zip([1, 2], [3, 4], [5, 6])))  # [(1, 3, 5), (2, 4, 6)]

# -- zip: lazy proof via side-effect tracking --------------------------------

produced = []
def gen_c():
    for i in range(4):
        produced.append(i)
        yield i

z2 = zip(gen_c(), [0, 1])
print(produced)          # []
print(next(z2))          # (0, 0)
print(len(produced) == 1)  # True
print(next(z2))          # (1, 1)
print(len(produced) == 2)  # True

# -- zip: strict mode ---------------------------------------------------------

print(list(zip([1, 2], [3, 4], strict=True)))   # [(1, 3), (2, 4)]

try:
    list(zip([1, 2, 3], [4, 5], strict=True))
except ValueError as e:
    print(e)    # zip() argument 2 is shorter than argument 1

try:
    list(zip([1, 2], [3, 4, 5], strict=True))
except ValueError as e:
    print(e)    # zip() argument 2 is longer than argument 1

# -- enumerate chained with zip ----------------------------------------------

print(list(zip(enumerate([10, 20]), ['x', 'y'])))  # [((0, 10), 'x'), ((1, 20), 'y')]
