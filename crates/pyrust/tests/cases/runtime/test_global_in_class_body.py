# Exercises `global` declarations inside a class body.
# Names declared `global` in a class body must write through to the module
# global, not into the class attribute dict (issue #618).

# --- Basic: global assignment writes to module global ---
x = 0
class C:
    global x
    x = 42

print(x)     # 42
try:
    print(C.x)
    print("WRONG: x in class dict")
except AttributeError:
    print("x not in class dict")  # x not in class dict


# --- Non-global names in the same class still become class attributes ---
y = 0
class D:
    global y
    y = 99
    z = 77   # not declared global

print(y)    # 99
print(D.z)  # 77
try:
    print(D.y)
    print("WRONG: y in class dict")
except AttributeError:
    print("y not in class dict")  # y not in class dict


# --- Reading a global name inside the class body uses module value ---
counter = 10
class E:
    global counter
    snapshot = counter   # reads module global (10) into class attr
    counter = 99         # writes 99 to module global

print(counter)     # 99
print(E.snapshot)  # 10


# --- No global declaration: existing class-body behaviour unchanged ---
a = 1
class F:
    a = 2    # class attribute, not a global write

print(a)    # 1  (module global unchanged)
print(F.a)  # 2


# --- global inside a class nested in a function: targets module global ---
outer_val = 0

def outer_fn():
    outer_val = 100   # function-local, shadows module global
    class Inner:
        global outer_val  # targets MODULE global, not the function local
        outer_val = 200

outer_fn()
print(outer_val)  # 200  (module global was updated)


# --- Multiple globals declared in one class body ---
p = 0
q = 0
class G:
    global p, q
    p = 11
    q = 22
    r = 33   # class attribute

print(p)    # 11
print(q)    # 22
print(G.r)  # 33
