# Parity fixture: str.format dispatches __str__ on user instances (issue #776).
#
# Before the fix, '{}'.format(obj) and '{!s}'.format(obj) both called
# Value::to_py_str(), which falls through to the default repr instead of
# dispatching __str__.

class WithStr:
    def __str__(self):
        return "custom_str"

class WithRepr:
    def __repr__(self):
        return "custom_repr"

class WithBoth:
    def __str__(self):
        return "both_str"
    def __repr__(self):
        return "both_repr"

class WithNone:
    pass

w = WithStr()
r = WithRepr()
b = WithBoth()
n = WithNone()

# Empty format spec: should call __str__ (like str(x))
print('{}'.format(w))       # custom_str
print('{}'.format(r))       # custom_repr (falls back to __repr__)
print('{}'.format(b))       # both_str (__str__ wins)

# !s conversion: should call __str__ (like str(x))
print('{!s}'.format(w))     # custom_str
print('{!s}'.format(r))     # custom_repr (falls back to __repr__)
print('{!s}'.format(b))     # both_str (__str__ wins)

# Objects with neither __str__ nor __repr__: default repr form
result_empty = '{}'.format(n)
result_s = '{!s}'.format(n)
# Both should contain the class name; we just check the type rather than
# the address, which differs between runs.
print("WithNone" in result_empty)   # True
print("WithNone" in result_s)       # True

# Built-in types are unaffected
print('{}'.format(42))              # 42
print('{}'.format(3.14))            # 3.14
print('{}'.format([1, 2, 3]))       # [1, 2, 3]
print('{!s}'.format(42))            # 42
print('{!s}'.format(True))          # True

# Inherited __str__ via MRO
class Base:
    def __str__(self):
        return "base_str"

class Child(Base):
    pass

c = Child()
print('{}'.format(c))               # base_str
print('{!s}'.format(c))             # base_str
