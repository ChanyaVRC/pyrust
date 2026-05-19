x = "global"


def f():
    x: int
    try:
        return x
    except UnboundLocalError:
        return "UnboundLocalError"


print(f())  # UnboundLocalError (not 'global')


# With value: works normally
def g():
    y: str = "hello"
    return y


print(g())  # hello


# Module-scope bare annotation does NOT make x a local;
# the global x is still accessible.
x: int
print(x)  # global


# Module-scope annotation for an undefined name: the name stays absent.
z: int
try:
    print(z)
except NameError:
    print("NameError: z not defined")


# UnboundLocalError is a subclass of NameError (CPython hierarchy).
def h():
    v: int
    try:
        return v
    except NameError:
        return "NameError caught"


print(h())  # NameError caught


# Class-scope bare annotation does NOT add the name to vars(C).
class C:
    x: int


print("x" in vars(C))  # False


# Attribute annotation in a method is a no-op (not a simple-name target).
class D:
    def __init__(self):
        self.x: int  # no store, no local declaration


d = D()
try:
    print(d.x)
except AttributeError:
    print("AttributeError for attr annotation")
