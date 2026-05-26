# Issue #1144: local variable access after del raises UnboundLocalError (not NameError);
# module-scope access of undefined/deleted name raises NameError with correct message.

# Case 1: deleted local inside a function -> UnboundLocalError
def foo():
    x = 1
    del x
    print(x)

try:
    foo()
except UnboundLocalError as e:
    print(type(e).__name__)
    print("x" in str(e))
except NameError as e:
    print("WRONG NameError:", str(e))

# Case 2: referenced before assignment inside a function -> UnboundLocalError
def bar():
    print(y)
    y = 2

try:
    bar()
except UnboundLocalError as e:
    print(type(e).__name__)
except NameError as e:
    print("WRONG NameError:", str(e))

# Case 3: undefined name at module scope -> NameError
try:
    print(undefined_var)
except NameError as e:
    print(type(e).__name__)
    print("undefined_var" in str(e))

# Case 4: deleted name at module scope -> NameError (not UnboundLocalError)
module_var = 1
del module_var
try:
    print(module_var)
except NameError as e:
    print(type(e).__name__)
    print("module_var" in str(e))
except UnboundLocalError as e:
    print("WRONG UnboundLocalError:", str(e))

# Case 5: UnboundLocalError is a subclass of NameError; except NameError catches it
def qux():
    a = 1
    del a
    print(a)

try:
    qux()
except NameError as e:
    print("caught_as_NameError:", type(e).__name__)

# Case 6: error message contains the variable name
def greet():
    print(message)
    message = "hello"

try:
    greet()
except UnboundLocalError as e:
    print("message" in str(e))
