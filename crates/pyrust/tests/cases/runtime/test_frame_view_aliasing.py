# Regression test for issue #547: vm_frame_views soundness.
#
# Each test exercises a path where the raw pointer in a VmFrameView is
# accessed while a suspended frame's &mut [Value] lives on the call stack.
# These are the aliasing patterns identified in #547.

# --- locals() from a function frame (innermost frame view read) ---

def fn_with_locals():
    a = 1
    b = 2
    locs = locals()
    print(sorted(locs.items()))

fn_with_locals()

# --- globals() from a function frame (script frame view read via pointer) ---

g_val = 42

def fn_reads_globals():
    g = globals()
    print('g_val' in g)
    print(g['g_val'])

fn_reads_globals()

# --- locals() from a generator frame (generator frame view read) ---

def gen_with_locals():
    x = 10
    locs = locals()
    yield sorted(locs.items())

for item in gen_with_locals():
    print(item)

# --- StoreGlobal write-through: nested function writes a global,
#     which must update the script frame's suspended register (env.rs path) ---

counter = 0

def increment_global():
    global counter
    counter += 1

increment_global()
increment_global()
print(counter)

# --- locals() inside a class body (class frame view read) ---
# Both CPython and pyrust return the persistent live class namespace.  This
# fixture keeps the original basic membership checks; mutation and identity
# coverage lives in test_frame_locals_class.py.

class C:
    x = 1
    y = 2
    locs = locals()
    print('x' in locs)
    print('y' in locs)
    print(locs['x'])
    print(locs['y'])

# --- Nested: globals() called from inside a nested generator ---

outer_val = 99

def make_gen():
    def inner_gen():
        g = globals()
        yield 'outer_val' in g
    return inner_gen()

for v in make_gen():
    print(v)
