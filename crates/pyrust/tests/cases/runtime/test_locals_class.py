# Parity fixture for issue #487: locals() inside a class body should
# return the partially-built class attrs dict, not the module globals.

# Basic case: names assigned before locals() are present.
class C:
    x = 1
    y = 2
    locs = locals()
    print('x' in locs)   # True
    print('y' in locs)   # True
    print(locs['x'])      # 1
    print(locs['y'])      # 2

# globals() inside a class body still returns module-level globals (a dict).
class D:
    g = globals()
    print(type(g).__name__)   # dict

# Nested class: locals() returns the *inner* class namespace, not the outer.
class Outer:
    a = 1
    class Inner:
        b = 2
        print('b' in locals())   # True
        print('a' in locals())   # False  (outer class attrs not included)
