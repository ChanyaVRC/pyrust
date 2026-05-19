class Foo:
    x = 1
    def bar(self): pass

d = Foo.__dict__

# Key existence
print('x' in d)
print('bar' in d)

# Value access
print(d['x'])

# Assigning the whole __dict__ attribute raises AttributeError
try:
    Foo.__dict__ = {}
except AttributeError as e:
    print('AttributeError:', e)

# Deleting __dict__ also raises AttributeError
try:
    del Foo.__dict__
except AttributeError as e:
    print('AttributeError:', e)

# Subclass: __dict__ includes attrs from the subclass body only
class Bar(Foo):
    y = 2

print('y' in Bar.__dict__)
# x is on Foo, not Bar
print('x' in Bar.__dict__)

# Class with no explicit attrs still has __module__
class Empty:
    pass

print('__module__' in Empty.__dict__)
