# Parity fixture for issue #970: locals() at module scope returns the live
# module dict (same object as globals()), and mutations propagate back.

x = 1
y = 2

# locals() and globals() are the same object at module scope.
locs = locals()
print(locs is globals())      # True

# Assigning through locals() updates the real module variable.
locs["x"] = 99
print(x)                      # 99

# locals() contains the current module variables.
print("x" in locals())        # True
print("y" in locals())        # True
print(locals()["y"])          # 2

# globals() reflects the same state.
print(globals()["y"])         # 2

# Inside a function, locals() is a snapshot, not the module dict.
def fn():
    a = 10
    snap = locals()
    snap["a"] = 99
    print(a)                  # 10 (snapshot; mutation does not propagate)
    print(locals() is globals())  # False

fn()
