# Regression test for issue #672: optimizer constant-fold must not use the
# pre-class value of a global written via a doubly-nested class body.
#
# PR #670 fixed the single-level case (class C: global x; x = 42).
# This fixture covers doubly-nested and triply-nested class bodies.

# --- double-nested: global write inside inner class ---
x = 0
class Outer:
    class Inner:
        global x
        x = 42

print(x)      # 42
print(x + 1)  # 43  (was 1 before fix: optimizer folded x as 0)

# --- triple-nested ---
y = 0
class A:
    class B:
        class C:
            global y
            y = 99

print(y)      # 99
print(y + 1)  # 100  (was 1 before fix)

# --- single-level regression guard (PR #670) ---
z = 0
class D:
    global z
    z = 7

print(z)      # 7
print(z + 1)  # 8

# --- class-method regression guard (issues #624/#629) ---
# A method's `global x` must NOT promote the class-body name to a cell var.
w = 0
class E:
    w = 50  # class attribute, should NOT be affected by method's global w

    def set_w(self):
        global w
        w = 77

print(E.w)    # 50  (class attribute, unchanged)
E().set_w()
print(w)      # 77
