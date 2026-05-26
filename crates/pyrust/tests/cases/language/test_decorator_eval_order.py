# Decorator expressions must be evaluated top-to-bottom (issue #1280).
# Application order is bottom-to-top (innermost first): fn = d1(d2(d3(fn))).

# --- Evaluation order ---

calls = []

def track(n):
    calls.append(n)
    return lambda f: f

@track(1)
@track(2)
@track(3)
def fn():
    pass

print(calls)  # [1, 2, 3]

# --- Application order ---

def wrap(n):
    def decorator(f):
        def wrapper():
            return f"d{n}({f()})"
        return wrapper
    return decorator

@wrap(1)
@wrap(2)
@wrap(3)
def hello():
    return "hello"

print(hello())  # d1(d2(d3(hello)))

# --- Single decorator (no change) ---

def identity(f):
    print("applied")
    return f

@identity
def solo():
    pass

# --- Class decorator evaluation order ---

cls_calls = []

def cls_track(n):
    cls_calls.append(n)
    return lambda c: c

@cls_track(1)
@cls_track(2)
class MyClass:
    pass

print(cls_calls)  # [1, 2]

# --- Decorator with side effects: evaluation must precede application ---

side_effects = []

def side(tag):
    side_effects.append(("eval", tag))
    def deco(f):
        side_effects.append(("apply", tag))
        return f
    return deco

@side("A")
@side("B")
def g():
    pass

print(side_effects)  # [('eval', 'A'), ('eval', 'B'), ('apply', 'B'), ('apply', 'A')]
