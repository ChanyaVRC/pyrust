# Compile-time SyntaxError for duplicate parameters and repeated call keywords
# (issue #2140), and a parameter declared global/nonlocal (issue #2141).
# Each invalid form is compiled via compile()/exec() so the SyntaxError is
# catchable; valid neighbors must still compile and run.


def check(src, mode="exec"):
    try:
        compile(src, "<test>", mode)
        print("no error")
    except SyntaxError as e:
        print("SyntaxError:", e.msg if hasattr(e, "msg") else e)


# Duplicate argument names (#2140).
check("def f(a, a): pass")
check("def g(a, b, a): pass")
check("def h(x, *, x): pass")
check("def k(*args, args): pass")
check("lam = lambda a, a: a")

# Repeated explicit keyword argument in a call (#2140).
check("f(x=1, x=2)")
check("g(a=1, b=2, a=3)")

# Parameter declared global / nonlocal (#2141).
check("def f(x):\n global x")
check("def g(x):\n nonlocal x")
check("def h(*args):\n global args")
check("def k(**kw):\n nonlocal kw")

# Valid neighbors still work.
def ok(a, b, *args, c=3, **kw):
    return (a, b, args, c, kw)


print(ok(1, 2, 9, c=4, d=5))


def call_ok(**k):
    return k


print(call_ok(x=1, y=2))

y = 10


def uses_global():
    global y
    y = 20


uses_global()
print(y)


def outer():
    z = 1

    def inner():
        nonlocal z
        z = 2

    inner()
    return z


print(outer())
