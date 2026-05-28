# Variadic slow-call path (*args, no **kwargs) should raise TypeError with the
# correct CPython 3.12 wording when an unexpected keyword argument is passed.

def f(*args): pass

try:
    f(unknown=1)
except TypeError as e:
    print(e)
except Exception as e:
    print(type(e).__name__ + ":", e)

# **kwargs absorbs all keywords — no error expected.
def g(**kwargs): pass

g(x=1)
print("g ok")

# Non-variadic function with unexpected keyword also raises TypeError.
def h(a, b): pass

try:
    h(1, 2, c=3)
except TypeError as e:
    print(e)
except Exception as e:
    print(type(e).__name__ + ":", e)
