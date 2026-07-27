from functools import total_ordering


class Base:
    def __lt__(self, other):
        return True


class Child(Base):
    pass


value = Child()


def probe(obj):
    return obj.__ge__(obj)


print("before:", probe(value) is NotImplemented)
total_ordering(Base)
print("installed:", "__ge__" in Base.__dict__)
print("after:", probe(value))
