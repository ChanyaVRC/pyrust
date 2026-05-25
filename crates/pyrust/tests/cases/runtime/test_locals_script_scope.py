# Parity fixture for issue #648: locals()/globals() at module/script scope.
#
# The noalias fix (RegSlice replacing &mut [Value] in run_bytecode_inner_impl)
# ensures that reading VmFrameView::regs_ptr through merge_frame_view_into_dict
# while the dispatch loop runs is not aliasing UB.
#
# We probe specific keys rather than printing the full dict to avoid dependence
# on CPython boilerplate names (__builtins__, __name__, __doc__, etc.) that
# pyrust may populate differently.

# --- locals() at module/script scope ---

x = 1
y = "hello"
locs = locals()
print("x in locals:", "x" in locs)
print("y in locals:", "y" in locs)
print("locals['x']:", locs["x"])
print("locals['y']:", locs["y"])

# Acceptance criterion from issue #648.
assert locs["x"] == 1

# --- globals() from inside a nested function ---

outer = 42

def nested_reads_globals():
    g = globals()
    print("outer in globals:", "outer" in g)
    print("globals['outer']:", g["outer"])

nested_reads_globals()

# --- locals() inside a module-level generator ---

def module_gen():
    ga = 10
    gb = 20
    locs = locals()
    yield sorted(locs.items())

for items in module_gen():
    print("gen locals:", items)

# --- locals() at module scope reflects current fastlocal values ---

z = 99
w = z + 1
locs2 = locals()
print("z in locs2:", "z" in locs2)
print("w in locs2:", "w" in locs2)
print("locs2['z']:", locs2["z"])
print("locs2['w']:", locs2["w"])
