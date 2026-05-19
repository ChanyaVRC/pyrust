# Parity fixture for issue #624:
# A `global x` declaration in a *method* must not affect how the *class body*
# stores the same name.  The class body's `x = ...` should produce a class
# attribute (RecordClassStore), while the method's `global x; x = ...` should
# write directly to the module global.

# --- basic case: class attr vs method-level global ---
x = 0
class C:
    def method(self):
        global x   # method-level global only
    x = 42         # class attribute, not a module write
print(C.x)   # 42
print(x)     # 0

# --- calling the method modifies the module global ---
y = "before"
class D:
    y = "class_y"
    def method(self):
        global y
        y = "after"
print(D.y)   # class_y  (class attr unaffected)
print(y)     # before   (module y unmodified until method is called)
D.method(None)
print(y)     # after    (method's global write now visible)
print(D.y)   # class_y  (class attr still unaffected)

# --- method with global in an if block inside the class body ---
z = 0
class E:
    if True:
        z = "class_z"
    def method(self):
        global z
        z = "method_z"
print(E.z)   # class_z
print(z)     # 0

# --- plain class attribute: no method involved (regression guard) ---
class F:
    a = 1
    b = "hello"
print(F.a, F.b)   # 1 hello

# --- multiple attributes, only one shadowed by method global ---
w = "module_w"
class G:
    w = "class_w"
    v = "class_v"
    def method(self):
        global w
        w = "method_w"
print(G.w, G.v)   # class_w class_v
print(w)           # module_w
