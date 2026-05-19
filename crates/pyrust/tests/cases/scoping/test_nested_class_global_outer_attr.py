# Parity fixture for issue #679:
# A `global x` declaration directly in a nested class body (not in a method)
# must not cause the enclosing outer class's attribute of the same name to
# disappear.  The outer class's `x = 50` must produce a class attribute
# (RecordClassStore), while `global x; x = 42` in the inner class body writes
# directly to the module global, bypassing the outer class namespace.

# --- main repro ---
x = 10
class Outer:
    x = 50

    class Inner:
        global x
        x = 42

print(x)        # 42 (module global written by Inner)
print(Outer.x)  # 50 (outer class attr unchanged)

# --- regression: basic class attribute still works ---
class Simple:
    val = 5

print(Simple.val)   # 5

# --- regression: flat class global still writes module global (#618) ---
g = 1
class FlatGlobal:
    global g
    g = 99

print(g)   # 99

# --- multiple attributes, only one matches the nested class's global ---
a = 10
b = 20
class Outer2:
    a = 100
    b = 200
    class Inner2:
        global a
        a = 999

print(a)         # 999 (global overwritten)
print(b)         # 20  (global untouched)
print(Outer2.a)  # 100 (class attr untouched)
print(Outer2.b)  # 200 (class attr untouched)

# --- global access remains correct after nested class construction ---
z = 0
class Wrapper:
    z = -1
    class Sub:
        global z
        z = 77

print(z)         # 77
print(Wrapper.z) # -1
