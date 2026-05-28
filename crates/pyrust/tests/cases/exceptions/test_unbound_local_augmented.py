# Issue #1644: augmented assignment on an unset local raises UnboundLocalError
# (not NameError) with the CPython 3.12 message.

# Case 1: x += 1 where x is a local assigned later
def f1():
    x += 1
    x = 0

try:
    f1()
except UnboundLocalError as e:
    print(type(e).__name__)
    print(isinstance(e, NameError))
    print(isinstance(e, UnboundLocalError))
    print(str(e))
except NameError as e:
    print("WRONG NameError:", str(e))

# Case 2: UnboundLocalError is a subclass of NameError; except NameError catches it
def f2():
    x += 1
    x = 0

try:
    f2()
except NameError as e:
    print("caught_as_NameError:", type(e).__name__)

# Case 3: other aug assign ops raise UnboundLocalError with the correct name
def f3():
    z -= 1
    z = 0

try:
    f3()
except UnboundLocalError as e:
    print("z" in str(e))
    print(type(e).__name__)

# Case 4: already-bound local must still work correctly
def f4():
    x = 10
    x += 5
    return x

print(f4())

# Case 5: module-scope augmented assignment on an undefined name raises NameError
g = {"__builtins__": __builtins__}
try:
    exec("y += 1", g)
except NameError as e:
    print("module NameError:", type(e).__name__)
except UnboundLocalError as e:
    print("WRONG UnboundLocalError at module scope:", str(e))
