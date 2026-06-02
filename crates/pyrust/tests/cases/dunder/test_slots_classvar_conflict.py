# Issue #1971: a __slots__ name that is also a class variable is an error.
try:
    class E:
        __slots__ = ("x",)
        x = 1
except ValueError as e:
    print(e)

# Conflict reported for whichever slot collides (not just the first).
try:
    class H:
        __slots__ = ("p", "q")
        q = 2
except ValueError as e:
    print(e)

# A method whose name is in __slots__ also conflicts.
try:
    class M:
        __slots__ = ("f",)

        def f(self):
            return 1
except ValueError as e:
    print(e)

# Non-conflicting __slots__ still works.
class F:
    __slots__ = ("a", "b")


f = F()
f.a = 1
f.b = 2
print(f.a, f.b)


# Methods that are NOT in __slots__ are fine alongside slots.
class S:
    __slots__ = ("m",)

    def f(self):
        return 1


s = S()
s.m = 3
print(s.m, s.f())

# __dict__ in __slots__ alongside a __dict__ class var is NOT a conflict.
class G:
    __slots__ = ("__dict__",)
    __dict__ = 1


print("G ok")
