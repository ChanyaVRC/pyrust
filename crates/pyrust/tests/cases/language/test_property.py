class Temperature:
    def __init__(self, celsius):
        self._celsius = celsius

    @property
    def celsius(self):
        return self._celsius

    @celsius.setter
    def celsius(self, value):
        if value < -273.15:
            raise ValueError("below absolute zero")
        self._celsius = value

    @property
    def fahrenheit(self):
        return self._celsius * 9 / 5 + 32

t = Temperature(100)
assert t.celsius == 100
assert t.fahrenheit == 212.0
t.celsius = 0
assert t.celsius == 0
assert t.fahrenheit == 32.0

try:
    t.celsius = -300
    assert False, "should have raised"
except ValueError:
    pass

# read-only property
class Rect:
    def __init__(self, w, h):
        self.w = w
        self.h = h

    @property
    def area(self):
        return self.w * self.h

r = Rect(3, 4)
assert r.area == 12
try:
    r.area = 10
    assert False, "should have raised AttributeError"
except AttributeError as e:
    # CPython 3.12 names the property and owner class (issue #1845).
    assert str(e) == "property 'area' of 'Rect' object has no setter", str(e)
# A getter-only property is a data descriptor: the failed assignment must NOT
# fall through to a silent instance-dict write.
assert "area" not in r.__dict__

# Deleting a getter-only property raises with the matching message.
try:
    del r.area
    assert False, "should have raised AttributeError"
except AttributeError as e:
    assert str(e) == "property 'area' of 'Rect' object has no deleter", str(e)


# property() built without the decorator carries the attribute name too.
class Named:
    def _get(self):
        return 1

    y = property(_get)


n = Named()
try:
    n.y = 9
    assert False, "should have raised AttributeError"
except AttributeError as e:
    assert str(e) == "property 'y' of 'Named' object has no setter", str(e)

print("property OK")
