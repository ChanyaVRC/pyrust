# Parity fixture for issue #629:
# A `global x` declaration in a method of a *nested* class must not affect
# how the outer class body stores the same name.  The outer class body's
# `x = ...` should produce a class attribute (RecordClassStore), while the
# nested method's `global x; x = ...` should write directly to the module
# global.

# --- basic case: outer class attr vs nested method global ---
x = "module_x"
class Outer:
    x = "outer_x"   # should become Outer.x (class attribute)
    class Inner:
        def m(self):
            global x   # routes to module, not Outer
            x = "changed"

print(Outer.x)   # outer_x
print(x)         # module_x (unmodified before m is called)

# --- calling Inner.m writes to module global, not Outer.x ---
Outer.Inner.m(None)
print(x)         # changed  (module x modified by m)
print(Outer.x)   # outer_x  (class attr unchanged)

# --- regression: simple class with no nested class still works ---
y = "module_y"
class Simple:
    y = "simple_y"

print(Simple.y)   # simple_y
print(y)          # module_y

# --- regression: direct method global (unrelated to nested class) ---
z = "module_z"
class Direct:
    def m(self):
        global z
        z = "direct_z"

Direct().m()
print(z)         # direct_z

# --- three levels deep: Outer -> Mid -> Inner ---
w = "module_w"
class Outer3:
    w = "outer3_w"
    class Mid:
        w = "mid_w"
        class Inner3:
            def m(self):
                global w
                w = "inner3_w"

print(Outer3.w)      # outer3_w
print(Outer3.Mid.w)  # mid_w
print(w)             # module_w (unmodified)
Outer3.Mid.Inner3.m(None)
print(w)             # inner3_w (module w modified by m)
print(Outer3.w)      # outer3_w (unchanged)
print(Outer3.Mid.w)  # mid_w    (unchanged)
