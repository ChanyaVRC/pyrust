# collections.UserDict / UserList / UserString — issue #1884.
#
# Thin wrapper base classes exposing a mutable `.data` attribute and
# delegating operations to it; designed to be subclassed.  The parity
# harness asserts byte-identical output against CPython 3.12.
#
# Reference: https://docs.python.org/3/library/collections.html

from collections import UserDict, UserList, UserString

# ---- UserDict ----
ud = UserDict({'a': 1})
ud['b'] = 2
print(ud, ud.data)
print(len(ud), ud['a'], 'a' in ud)
print(sorted(ud.keys()), ud.get('z', 9))
print(UserDict({'a': 1}) | {'b': 2})
print(UserDict(x=1, y=2))


class CountingDict(UserDict):
    def __missing__(self, key):
        return 0


cd = CountingDict({'a': 1})
print(cd['a'], cd['missing'])
print(type(cd).__name__)

# ---- UserList ----
ul = UserList([1, 2, 3])
ul.append(4)
ul.insert(0, 0)
print(ul, ul.data, len(ul), ul[2])
print(ul == [0, 1, 2, 3, 4])
print(ul + [5])
print(ul * 2)
print(ul[1:3], type(ul[1:3]).__name__)
ul.sort(reverse=True)
print(ul)
print(ul.count(2), ul.index(2))


class MyList(UserList):
    pass


ml = MyList([3, 1, 2])
ml.sort()
print(ml, type(ml).__name__)

# ---- UserString ----
us = UserString("Hello World")
print(us, str(us), len(us), us[0])
print(us.upper(), us.lower())
print(us.replace("o", "0"))
print(us.split(), us.startswith("Hello"))
print(us + "!", "say: " + us)
print(us * 2)
print(UserString("abc") == "abc", UserString("abc") == UserString("abc"))
print(UserString("5").rjust(4, "0"))
print(UserString("%d-%s") % (5, "x"))
print(UserString("  trim  ").strip())
print(type(us.upper()).__name__)


class MyStr(UserString):
    pass


print(type(MyStr("x").upper()).__name__)
