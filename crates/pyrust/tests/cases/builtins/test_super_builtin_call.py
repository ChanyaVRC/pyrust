# Issue #1771: super().builtin_method() inside a subclass of a builtin type
# Issue #1772: callable() returns False for builtin classmethod wrappers

# --- Issue #1771: super_bound_builtin is callable ---

# dict subclass: super().update() with kwargs
class MyDict(dict):
    def update(self, *args, **kwargs):
        super().update(*args, **kwargs)

d = MyDict()
d.update(a=1)
d.update({'b': 2})
d.update([('c', 3)])
print(d)

# dict subclass: super().clear()
class MyDict2(dict):
    def clear(self):
        super().clear()

d2 = MyDict2({'x': 1, 'y': 2})
d2.clear()
print(d2)

# list subclass: super().append()
class MyList(list):
    def append(self, x):
        super().append(x)

l = MyList()
l.append(10)
l.append(20)
print(l)

# set subclass: super().add()
class MySet(set):
    def add(self, x):
        super().add(x)

s = MySet()
s.add(1)
s.add(2)
print(sorted(s))

# str subclass: super().upper()
class MyStr(str):
    def upper(self):
        return super().upper()

ms = MyStr("hello")
print(ms.upper())

# --- Issue #1772: callable() for builtin classmethod wrappers ---

print(callable(object.__subclasshook__))
print(callable(object.__init_subclass__))
print(callable(int.__init_subclass__))

# Regressions: already-working callable paths must still work
print(callable(len))
print(callable(print))
print(callable(str.upper))
print(callable(1))
print(callable(None))
print(callable([]))

# callable on the bound method of a subclass instance (not super_bound_builtin)
d3 = MyDict()
print(callable(d3.update))
