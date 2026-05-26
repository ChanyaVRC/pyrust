# Issue #1126: vars() with no args should return current-frame locals,
# not module globals.
#
# Keys are tested individually rather than printing the whole dict to
# avoid ordering / boilerplate-key (__builtins__, __name__, ...) drift.

module_var = 42


def foo():
    a = 1
    b = 2
    v = vars()
    print("a in vars():", "a" in v)
    print("b in vars():", "b" in v)
    print("module_var not in vars():", "module_var" not in v)
    print("vars()['a']:", v["a"])
    print("vars()['b']:", v["b"])


foo()

# vars() at module scope returns module globals (same as globals()).
mv = vars()
print("module_var in module vars():", "module_var" in mv)
print("type of module vars():", type(mv).__name__)

# vars(obj) returns obj.__dict__
class MyClass:
    def __init__(self):
        self.x = 1
        self.y = 2


obj = MyClass()
d = vars(obj)
print("vars(obj) keys:", sorted(d.keys()))

# vars(obj) when obj has no __dict__ raises TypeError
try:
    vars(42)
    print("vars-int-error: FAIL")
except TypeError:
    print("vars-int-error: TypeError")

# vars() inside a nested function sees its own locals
def outer():
    outer_var = "outer"

    def inner():
        inner_var = "inner"
        v = vars()
        print("inner_var in inner vars():", "inner_var" in v)
        print("outer_var not in inner vars():", "outer_var" not in v)

    inner()
    v = vars()
    print("outer_var in outer vars():", "outer_var" in v)


outer()
