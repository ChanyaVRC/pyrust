# __annotations__ is materialised lazily (#2256): an unannotated function does
# not carry an eagerly-allocated empty dict, but the observable behaviour must
# match CPython exactly — including object identity across repeated reads and
# mutation persistence.


# --- unannotated: empty dict, created lazily, stable identity ---
def f(a, b):
    return a + b


print(f.__annotations__)
print(f.__annotations__ is f.__annotations__)
a = f.__annotations__
a["z"] = 1
print(f.__annotations__)  # mutation persists → {'z': 1}


# --- annotated: populated at def time ---
def g(x: int, y: str = "h") -> bool:
    return True


print(g.__annotations__ == {"x": int, "y": str, "return": bool})


# --- assignment / deletion / None ---
def h():
    pass


h.__annotations__ = {"p": 1}
print(h.__annotations__)
del h.__annotations__
print(h.__annotations__)  # fresh empty dict
h.__annotations__ = None
print(h.__annotations__)  # empty dict (CPython coerces)


# --- closures and lambdas ---
def mk(i):
    def inner():
        return i

    return inner


print(mk(5).__annotations__)
print((lambda z: z).__annotations__)


# --- methods / staticmethod / classmethod ---
class C:
    def m(self, x: int) -> int:
        return x

    def plain(self):
        return 1

    @staticmethod
    def s(a: str):
        return a

    @classmethod
    def c(cls, b: float):
        return b


o = C()
print(o.m.__annotations__ == {"x": int, "return": int})
print(o.plain.__annotations__)
print(C.s.__annotations__ == {"a": str})
print(C.c.__annotations__ == {"b": float})

# mutating the function's annotations is visible through a bound method
C.m.__annotations__["extra"] = 1
print("extra" in o.m.__annotations__)
