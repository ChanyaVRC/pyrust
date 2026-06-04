# Compile-time SyntaxError for comprehension misuse:
#   * walrus rebinding a comprehension iteration variable (issue #2139)
#   * yield inside a comprehension / generator expression (issue #2143)


def check(src, mode="exec"):
    try:
        compile(src, "<test>", mode)
        print("no error")
    except SyntaxError as e:
        print("SyntaxError:", e.msg)


# Walrus rebinding the iteration variable (#2139).
check("[(i := i + 1) for i in range(3)]")
check("{(k := k) for k in range(3)}")
check("{(v := v): 0 for v in range(3)}")
check("[i for i in range(3) if (i := i + 1)]")
check("[[(i := 1) for j in range(2)] for i in range(2)]")

# yield inside a comprehension (#2143).
check("[(yield i) for i in range(3)]")
check("{(yield i) for i in range(3)}")
check("{(yield i): 0 for i in range(3)}")
check("list((yield i) for i in range(3))")
check("[(yield from x) for i in range(3)]")

# Valid neighbors: walrus targeting a non-iteration name, and a walrus inside a
# nested lambda (its own scope) are both fine.
print([(x := 1) for i in range(3)])
print([(y := i) for i in range(3)])
print([(lambda: (i := 1))() for i in range(2)])
print([i for i in range(3)])
print({k for k in range(3)})
print({v: v * 2 for v in range(3)})
