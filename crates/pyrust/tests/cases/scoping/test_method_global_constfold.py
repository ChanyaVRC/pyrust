# Parity fixture for issue #671:
# pass_const_fold must not retain stale constant-fold entries for module-level
# fastlocal registers across a call that may write to those globals via
# the assign_name write-through in vm_frame_views.

# --- basic case: method writes global, then arithmetic must see new value ---
x = 0

class C:
    def m(self):
        global x
        x = 42

C().m()
print(x)       # 42
print(x + 1)   # 43  (was 1 before fix — optimizer folded x as 0)
print(x == 42) # True

# --- __init__ variant: global written during class instantiation ---
y = 10

class Foo:
    def __init__(self):
        global y
        y = 99

Foo()
print(y)       # 99
print(y + 1)   # 100  (was 11 before fix)

# --- doubly-nested class method writes global ---
z = 7

class Outer:
    class Inner:
        def mutate(self):
            global z
            z = 100

Outer.Inner().mutate()
print(z + 1)   # 101  (was 8 before fix)

# --- regression guard: class attribute write via class-body assignment
#     (class-body global declaration, PR #618) must not regress ---
w = 10

class Bar:
    global w
    w = 99  # direct class-body global write

print(w)       # 99
print(w + 1)   # 100

# --- copy-prop: named-local alias must not survive across call boundary ---
# When a named local is assigned from another named local (x = y), the
# copy-propagation pass may record "x aliases y".  If a subsequent call
# writes to x via assign_name write-through, the alias is stale and must
# be evicted before propagating x to any downstream instruction.
p = 5
q = p          # q aliases p in copy-prop

class Alias:
    def update(self):
        global q
        q = 100

Alias().update()
print(q)       # 100  (was 5 before fix — copy-prop substituted q with p)
print(q + 1)   # 101
