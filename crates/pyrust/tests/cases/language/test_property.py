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
except AttributeError:
    pass

print("property OK")
