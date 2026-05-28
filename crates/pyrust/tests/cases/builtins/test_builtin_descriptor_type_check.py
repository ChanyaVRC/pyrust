# Parity test for issue #1500: builtin method descriptors raise TypeError
# when accessed on an instance whose class is not a subclass of the
# descriptor's defining type.
#
# CPython's method_descriptor.__get__ checks PyObject_TypeCheck(obj, descr->d_type)
# and raises TypeError: descriptor 'M' for 'T' objects doesn't apply to a 'U' object.

# --- list.append on unrelated class ---
class Foo:
    pass

Foo.append = list.append
try:
    Foo().append
    print("FAIL: list.append should raise TypeError")
except TypeError as e:
    print("TypeError:", e)

# --- dict.keys on unrelated class ---
class Bar:
    pass

Bar.keys = dict.keys
try:
    Bar().keys
    print("FAIL: dict.keys should raise TypeError")
except TypeError as e:
    print("TypeError:", e)

# --- str.upper on unrelated class ---
class Baz:
    pass

Baz.upper = str.upper
try:
    Baz().upper
    print("FAIL: str.upper should raise TypeError")
except TypeError as e:
    print("TypeError:", e)

# --- set.add on unrelated class ---
class Qux:
    pass

Qux.add = set.add
try:
    Qux().add
    print("FAIL: set.add should raise TypeError")
except TypeError as e:
    print("TypeError:", e)

# --- tuple.count on unrelated class ---
class Quux:
    pass

Quux.count = tuple.count
try:
    Quux().count
    print("FAIL: tuple.count should raise TypeError")
except TypeError as e:
    print("TypeError:", e)

# --- Subclass of the defining type: must NOT raise ---
class MyList(list):
    pass

ml = MyList()
try:
    m = ml.append
    print("OK: MyList().append is accessible")
except TypeError as e:
    print("FAIL: MyList().append raised TypeError:", e)

class MyDict(dict):
    pass

md = MyDict()
try:
    m = md.keys
    print("OK: MyDict().keys is accessible")
except TypeError as e:
    print("FAIL: MyDict().keys raised TypeError:", e)

class MySet(set):
    pass

ms = MySet()
try:
    m = ms.add
    print("OK: MySet().add is accessible")
except TypeError as e:
    print("FAIL: MySet().add raised TypeError:", e)

# --- Direct class-level access must NOT raise ---
try:
    x = list.append
    print("OK: list.append (class) is accessible")
except TypeError as e:
    print("FAIL: list.append (class) raised TypeError:", e)

# --- Normal instance access must NOT raise ---
try:
    x = [].append
    print("OK: [].append is accessible")
except TypeError as e:
    print("FAIL: [].append raised TypeError:", e)

try:
    x = {}.keys
    print("OK: {}.keys is accessible")
except TypeError as e:
    print("FAIL: {}.keys raised TypeError:", e)

try:
    x = "hello".upper
    print("OK: 'hello'.upper is accessible")
except TypeError as e:
    print("FAIL: 'hello'.upper raised TypeError:", e)
