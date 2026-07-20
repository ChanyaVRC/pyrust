# Built-in name resolution is cached against a "structure version" that a hot
# value reassignment does NOT bump, so a module-scope loop that reassigns a
# global while calling built-ins stays fast. This must not change any
# observable semantics: rebinding, deleting, `global`-shadowing, and
# globals()-mutating a built-in name must all still take effect immediately.

# builtin call in a module-scope loop that reassigns a global every iteration
n = 0
for i in range(300):
    n += len(str(i))
print("loop", n)

# rebind a builtin at module scope, then restore via del
print(len("abc"), abs(-5))
saved_len = len
len = lambda x: -99  # noqa: E731
print(len("abc"), len([1, 2, 3]))
del len
print(len("abc"))  # back to the real builtin

# rebind inside a loop body that also reassigns a global
total = 0
ordf = ord
ord = lambda c: 1000  # noqa: E731
for _ in range(5):
    total += ord("a")
print("rebound", total)
del ord
total = 0
for _ in range(5):
    total += ord("a")  # real builtin -> 97
print("restored", total)

# shadow a builtin from inside a function via `global`, then unshadow
def shadow():
    global hex
    hex = lambda x: "shadowed"  # noqa: E731

def unshadow():
    global hex
    del hex

print(hex(255))
shadow()
print(hex(255))
unshadow()
print(hex(255))

# globals()-dict mutation of a builtin name takes effect
print(oct(8))
globals()["oct"] = lambda x: "custom-oct"
print(oct(8))
del globals()["oct"]
print(oct(8))

# a value-changing global is still read fresh across calls
g = 1
def read_g():
    return g
print(read_g())
g = 2
print(read_g())

# builtin calls inside a function (no global thrash there) stay correct
def compute():
    s = 0
    for i in range(300):
        s += len(str(i)) + ord("z")
    return s
print("infn", compute())

# interleave two builtins and a reassigned global in one hot loop
acc = 0
for i in range(200):
    acc += len(str(i)) * ord("a") - abs(-i)
print("mixed", acc)
