# Parity: valid annotation uses in class bodies are unaffected by the
# fix for issue #770 (annotation + global/nonlocal in class body -> SyntaxError).

# Annotation WITHOUT global/nonlocal is fine.
x = 0
class C:
    y: int = 5
    print(y)  # 5

# Global WITHOUT annotation is fine.
class D:
    global x
    x = 42
print(x)  # 42

# Unannotated assignment with global is fine.
y = 0
class E:
    global y
    y = 99
print(y)  # 99

# Subscript annotation with global is fine (only simple name annotations conflict).
arr = [0]
class F:
    global arr
    arr[0]: int = 7
print(arr[0])  # 7

# Annotated assignment with no global/nonlocal conflict is fine inside a function.
def h():
    a: int = 10
    return a
print(h())  # 10

# Bare annotation in class body — no attribute created, no conflict.
class WithBare:
    a: int = 1
    b: int        # bare annotation — no class attr

assert hasattr(WithBare, "a")
assert not hasattr(WithBare, "b")
print("OK")
