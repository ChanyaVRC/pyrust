# Classes: definitions, instances, methods, inheritance

# Basic class
class Counter:
    def __init__(self, start):
        self.value = start

    def inc(self, step=1):
        self.value = self.value + step
        return self.value


counter = Counter(10)
print("class-basic", counter.value, counter.inc(), counter.inc(4), counter.value)

# Inheritance
class BaseCounter:
    kind = "base"

    def __init__(self, start):
        self.value = start

    def total(self, extra=1):
        return self.value + extra


class DerivedCounter(BaseCounter):
    pass


derived = DerivedCounter(10)
print("class-inherit", derived.kind, derived.value, derived.total(), DerivedCounter.kind)


# Matrix multiplication operator (@/@=) with special methods
class MatValue:
    def __init__(self, value):
        self.value = value

    def __matmul__(self, other):
        return MatValue(self.value * 10 + other.value)

    def __rmatmul__(self, other):
        return MatValue(other.value * 100 + self.value)

    def __imatmul__(self, other):
        self.value = self.value + other.value
        return self


a = MatValue(2)
b = MatValue(3)
print("class-matmul", (a @ b).value)

x = MatValue(5)
x @= MatValue(7)
print("class-imatmul", x.value)


class LeftOnlyValue:
    def __init__(self, value):
        self.value = value


class RightMatmul:
    def __init__(self, value):
        self.value = value

    def __rmatmul__(self, other):
        return other.value * 1000 + self.value


print("class-rmatmul", LeftOnlyValue(4) @ RightMatmul(6))
