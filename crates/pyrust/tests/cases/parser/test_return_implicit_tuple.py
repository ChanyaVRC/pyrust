# Implicit tuple in `return` and `yield` without parentheses.
# CPython 3.12 treats `return a, b` as `return (a, b)` and
# `yield a, b` as `yield (a, b)`.


def pair():
    return 1, 2


def triple():
    return "a", "b", "c"


def single_trailing():
    # trailing comma -> single-element tuple
    return 42,


def single_no_comma():
    # plain expression, no comma -> unaffected
    return 99


# --- return ---
print(pair())
print(triple())
print(single_trailing())
print(single_no_comma())

# unpacking from implicit-tuple return
x, y = pair()
print(x, y)


# --- yield ---
def gen_pairs():
    yield 1, 2
    yield "x", "y"


def gen_single():
    yield 10
    yield 20


def gen_trailing():
    yield 1,


print(list(gen_pairs()))
print(list(gen_single()))
print(list(gen_trailing()))
