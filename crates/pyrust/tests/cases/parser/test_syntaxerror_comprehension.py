# Compile-time SyntaxError for comprehension misuse:
#   * walrus anywhere in a comprehension iterable (PEP 572)
#   * walrus rebinding a comprehension iteration variable (issue #2139)
#   * yield inside a comprehension / generator expression (issue #2143)


def check(src, mode="exec"):
    try:
        compile(src, "<test>", mode)
        print("no error")
    except SyntaxError as e:
        print("SyntaxError:", e.msg)


# Assignment expressions are prohibited anywhere lexically inside every
# comprehension iterable, across all comprehension and expression scope forms.
check("[i for i in (seen := ())]")
check("{i for i in (seen := ())}")
check("{i: i for i in (seen := ())}")
check("(i for i in (seen := ()))")
check("[j for i in () for j in (seen := ())]")
check("[i for i in (lambda: (seen := ()))()]")
check("[i for i in (lambda value=(seen := ()): ())()]")
check("[i for i in [seen := j for j in ()]]")
check("[i for i in f'{(seen := ())}']")
# Iterable-walrus diagnostics take precedence over body and async-context
# diagnostics in CPython.
check("[(yield i) for i in (seen := ())]")
check("[i async for i in (seen := ())]")

# Walrus rebinding the iteration variable (#2139).
check("[(i := i + 1) for i in range(3)]")
check("{(k := k) for k in range(3)}")
check("{(v := v): 0 for v in range(3)}")
check("[i for i in range(3) if (i := i + 1)]")
check("[[(i := 1) for j in range(2)] for i in range(2)]")
# A lambda default is evaluated in the comprehension scope, so it cannot
# rebind that comprehension's iteration variable either.
check("[(lambda value=(i := 3): value)() for i in range(2)]")

# yield inside a comprehension (#2143).
check("[(yield i) for i in range(3)]")
check("{(yield i) for i in range(3)}")
check("{(yield i): 0 for i in range(3)}")
check("list((yield i) for i in range(3))")
check("[(yield from x) for i in range(3)]")
# The outermost iterable belongs to the enclosing scope, but later iterables
# execute inside the implicit comprehension function and cannot yield.
check("[j for i in range(3) for j in (yield i)]")
check("(j for i in range(3) for j in (yield from [i]))")

# Valid neighbors: walrus targeting a non-iteration name, and a walrus inside a
# nested lambda (its own scope) are both fine.
print([(x := 1) for i in range(3)])
print([(y := i) for i in range(3)])
print([(lambda: (i := 1))() for i in range(2)])
print([i for i in range(3)])
print({k for k in range(3)})
print({v: v * 2 for v in range(3)})
