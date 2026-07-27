# General descriptor protocol: __get__ / __set__ / __delete__ dispatch,
# data vs non-data priority (CPython Data Model §3.3.2).

# --- Data descriptor: __get__ is called, __set__ raises ---
class ReadOnly:
    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        return 42
    def __set__(self, obj, value):
        raise AttributeError("read-only!")

class Foo:
    x = ReadOnly()

f = Foo()
print(f.x)       # 42 — __get__ called
try:
    f.x = 99
except AttributeError as e:
    print(e)     # read-only!

# Class access: __get__ called with obj=None → returns self
print(type(Foo.x).__name__)  # ReadOnly

# --- Data descriptor with all three dunders ---
class FullDesc:
    def __init__(self):
        self._vals = {}
    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        return self._vals.get(id(obj), "unset")
    def __set__(self, obj, value):
        self._vals[id(obj)] = value
    def __delete__(self, obj):
        self._vals.pop(id(obj), None)

class C:
    v = FullDesc()

c = C()
print(c.v)          # unset — __get__
c.v = "hello"       # __set__
print(c.v)          # hello — __get__
del c.v             # __delete__
print(c.v)          # unset — __get__ again

# --- Non-data descriptor: instance assignment shadows it ---
class NonData:
    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        return "from_descriptor"

class Bar:
    y = NonData()

b = Bar()
print(b.y)           # from_descriptor — __get__ (no instance entry yet)
b.y = "from_instance"  # writes to instance dict (no __set__ to intercept)
print(b.y)           # from_instance — instance dict wins over non-data descriptor

# --- Data descriptor priority over instance dict ---
class DataPriority:
    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        return "data_wins"
    def __set__(self, obj, value):
        pass  # absorb writes silently

class Baz:
    x = DataPriority()

bz = Baz()
bz.x = "instance_value"  # __set__ absorbs
print(bz.x)              # data_wins — data descriptor shadows instance dict

# --- Descriptor with __delete__ only (still a data descriptor) ---
class DelOnly:
    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        return "del_only"
    def __delete__(self, obj):
        print("deleted")

class D:
    z = DelOnly()

d = D()
print(d.z)      # del_only — __get__
del d.z         # deleted — __delete__
# Assignment must raise AttributeError: __set__ (data descriptor blocks write)
try:
    d.z = "x"
except AttributeError as e:
    print(f"blocked: {e}")  # blocked: __set__

# --- Class-level access: __get__(None, cls) must be called ---
# CPython calls __get__(None, owner) when accessed on the class directly.
class ClassAccess:
    def __get__(self, obj, objtype=None):
        if obj is None:
            return f"class:{objtype.__name__}"
        return f"instance:{type(obj).__name__}"

class E:
    ca = ClassAccess()

e = E()
print(E.ca)   # class:E  — __get__(None, E) called
print(e.ca)   # instance:E — __get__(e, E) called

# Repeated reads from one bytecode site must continue invoking a custom
# descriptor. An attribute inline cache may retain ordinary class values and
# methods, but it must not turn a dynamic __get__ result into the raw descriptor.
class CountingDescriptor:
    def __init__(self):
        self.calls = 0
    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        self.calls += 1
        return self.calls

counting = CountingDescriptor()
class CacheOwner:
    value = counting

cache_owner = CacheOwner()
def read_cached_site(obj):
    return obj.value

print(read_cached_site(cache_owner))
print(read_cached_site(cache_owner))
print(read_cached_site(cache_owner))
print(counting.calls)
