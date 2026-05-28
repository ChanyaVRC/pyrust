# Parity fixture for issue #1723:
# Variadic functions (*args/**kwargs) with multiple missing required arguments
# should report all missing args in one TypeError, not just the first one.

# 1. *args with 2 missing keyword-only args
def f(*args, a, b): pass
try: f()
except TypeError as e: print(e)

# 2. *args with 3 missing keyword-only args
def g(*args, a, b, c): pass
try: g()
except TypeError as e: print(e)

# 3. *args + positional + 2 missing keyword-only (only missing kwonly)
def h(*args, a, b): pass
try: h(1, 2, 3)
except TypeError as e: print(e)

# 4. 1 missing keyword-only (already worked; regression guard)
def j(*args, a): pass
try: j()
except TypeError as e: print(e)

# 5. Missing keyword-only with one supplied
def k(*args, a, b): pass
try: k(a=1)
except TypeError as e: print(e)

# 6. Missing positional with *args present
def m(x, y, *args): pass
try: m()
except TypeError as e: print(e)

# 7. Missing positional takes priority over missing kwonly
def n(x, *args, a, b): pass
try: n()
except TypeError as e: print(e)

# 8. **kwargs with 2 missing keyword-only
def p(*, a, b, **kwargs): pass
try: p()
except TypeError as e: print(e)

# 9. qualname for method in class
class Foo:
    def bar(*args, x, y): pass
try: Foo.bar()
except TypeError as e: print(e)

# 10. **kwargs with missing positionals
def q(x, y, **kwargs): pass
try: q()
except TypeError as e: print(e)
