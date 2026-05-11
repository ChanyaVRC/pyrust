class Container:
    def __init__(self, value):
        self._value = value

    @property
    def value(self):
        return self._value

    @value.setter
    def value(self, v):
        self._value = v

    @value.deleter
    def value(self):
        self._value = None

c = Container(42)
assert c.value == 42
del c.value
assert c.value is None

# deleter-only property (no setter)
class ReadOnly:
    def __init__(self):
        self._x = 10

    @property
    def x(self):
        return self._x

    @x.deleter
    def x(self):
        del self._x

r = ReadOnly()
assert r.x == 10
del r.x
try:
    _ = r.x
    assert False, "should have raised AttributeError"
except AttributeError:
    pass

# no-deleter property raises AttributeError on del
class NoDeleter:
    @property
    def y(self):
        return 1

nd = NoDeleter()
try:
    del nd.y
    assert False, "should have raised"
except AttributeError:
    pass

print("property deleter OK")
