from functools import reduce


events = []


def finite_source():
    for value in range(5):
        events.append("yield-" + str(value))
        yield value


def fail(left, right):
    events.append("reduce")
    raise RuntimeError("stop")


try:
    reduce(fail, finite_source())
except RuntimeError as error:
    print(str(error), events)


def infinite_source():
    value = 0
    while True:
        yield value
        value += 1


try:
    reduce(fail, infinite_source())
except RuntimeError as error:
    print("infinite", str(error))

print(reduce(lambda left, right: left + right, [1, 2, 3], 10))

try:
    reduce(lambda left, right: left + right, [])
except TypeError as error:
    print("empty", str(error))
