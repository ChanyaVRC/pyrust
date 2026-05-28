# Parity fixture for str.format() and str.format_map() field accessors on
# built-in types (issue #1031).  CPython 3.12 dispatches the same getattr
# mechanism for all object types; pyrust previously restricted .attr access to
# PyInstance values, causing AttributeError on int, float, complex, etc.

# --- complex attributes ---
print('{0.real}'.format(3+2j))
print('{0.imag}'.format(3+2j))

# --- int numeric-tower attributes ---
print('{0.numerator}'.format(5))
print('{0.denominator}'.format(5))
print('{0.real}'.format(5))
print('{0.imag}'.format(5))

# --- float attributes ---
print('{0.real}'.format(1.5))   # float.real is the float itself
print('{0.imag}'.format(1.5))   # float.imag is 0.0

# --- bool (int subclass) attributes ---
print('{0.real}'.format(True))      # returns int 1, not bool
print('{0.numerator}'.format(True)) # returns int 1, not bool

# --- format_map has the same fix ---
print('{x.real}'.format_map({'x': 3+2j}))
print('{x.real}'.format_map({'x': 1.5}))

# --- PyInstance attribute access must still work ---
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

p = Point(10, 20)
print('{0.x}'.format(p))
print('{0.y}'.format(p))

# --- chained accessors ---
class Wrapper:
    def __init__(self, inner):
        self.inner = inner

w = Wrapper(Point(7, 8))
print('{0.inner.x}'.format(w))

# --- bad attribute raises AttributeError ---
try:
    '{0.nosuchattr}'.format(5)
except AttributeError as e:
    print(f'AttributeError: {e}')

try:
    '{0.nosuchattr}'.format(3+2j)
except AttributeError as e:
    print(f'AttributeError: {e}')

try:
    '{x.nosuchattr}'.format_map({'x': 1.5})
except AttributeError as e:
    print(f'AttributeError: {e}')
