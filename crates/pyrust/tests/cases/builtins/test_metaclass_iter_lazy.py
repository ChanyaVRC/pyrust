# A class object's iteration protocol belongs to its metaclass.  The returned
# iterator must remain lazy; `for ... break` must not drain it first.

seen = []


def source():
    value = 0
    while True:
        seen.append(value)
        yield value
        value += 1


class IterableMeta(type):
    def __iter__(cls):
        return source()


class Values(metaclass=IterableMeta):
    pass


for value in Values:
    print("for", value)
    break
print("seen-after-break", seen)

iterator = iter(Values)
print("iter-next", next(iterator), next(iterator))
print("seen-after-next", seen)


class BadMeta(type):
    def __iter__(cls):
        return [1, 2]


class Bad(metaclass=BadMeta):
    pass


try:
    iter(Bad)
except TypeError as error:
    print("bad", str(error))
