# Parity fixture: str.format field accessors consult the class MRO (issue #775).
#
# Before the fix, '{obj.attr}'.format(obj=instance) only looked at the instance
# dict and never fell through to the class or its bases.

class Animal:
    species = "mammal"

class Dog(Animal):
    legs = 4

d = Dog()

# Class-level attribute (declared on Dog itself)
print('{0.legs}'.format(d))

# Inherited class attribute (from Animal via MRO)
print('{0.species}'.format(d))

# Instance attribute shadows class attribute
class Shadowed:
    x = 10

s = Shadowed()
s.x = 99
print('{0.x}'.format(s))

# AttributeError when attr is genuinely absent
try:
    print('{0.missing}'.format(d))
except AttributeError:
    print("AttributeError raised")

# format_map path: same MRO lookup applies
class Config:
    default_timeout = 30

c = Config()
print('{c.default_timeout}'.format(c=c))

# Chained accessors: first accessor returns a class attr, second attr on that
class Inner:
    value = 7

class Outer:
    inner = Inner()

o = Outer()
print('{0.inner.value}'.format(o))
