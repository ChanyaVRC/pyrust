# Test that a class method reading a free variable whose name matches a
# class attribute does not strip the class attribute (issue #695).

# --- Main repro: flat class, method reads free var matching class attr ---
y = 99  # module global

class B:
    y = 22  # class attribute

    def method(self):
        return y  # reads free variable 'y' (module global, not class attr)

print(B.y)          # 22  — class attribute must survive
print(B().method())  # 99  — method sees module global

# --- Regression: class attribute accessible when no freevar collision ---
class C:
    x = 5

    def m(self):
        pass

print(C.x)  # 5

# --- Regression: module global read in method when no name collision ---
z = 77

class D:
    def m(self):
        return z

print(D().m())  # 77

# --- Multiple attributes: only the colliding name is affected ---
a = 10

class E:
    a = 1
    b = 2

    def method(self):
        return a  # reads module global 'a'

print(E.a)  # 1
print(E.b)  # 2
print(E().method())  # 10

# --- Method with its own local shadows neither class attr nor global ---
val = 50

class F:
    val = 7

    def method(self):
        val = 99  # local to method
        return val

print(F.val)          # 7
print(F().method())   # 99
