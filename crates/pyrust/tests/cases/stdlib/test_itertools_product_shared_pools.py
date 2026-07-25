from itertools import product


events = []


def source():
    events.append("start")
    yield 1
    yield 2
    events.append("end")


# A repeated input is materialised exactly once, before the first result.
iterator = product(source(), repeat=3)
print(events)
print(next(iterator))
print(len(list(iterator)), events)

# Repeated dimensions retain the identity of mutable pool elements.
token = []
row = next(product([token], repeat=4))
print([item is token for item in row])
print(row[0] is row[1] is row[2] is row[3])


def untouched():
    events.append("unexpected")
    yield 99


# A zero-fold product neither validates nor consumes its inputs.
before = list(events)
print(list(product(untouched(), repeat=0)), events == before)
print(list(product(1, repeat=0)))

# With no positional pools, even a huge repeat has zero dimensions.
print(next(product(repeat=1_000_000_000)))
