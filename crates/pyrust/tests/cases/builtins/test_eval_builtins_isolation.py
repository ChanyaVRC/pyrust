# Parity fixture for issue #1810: exec/eval with an explicit __builtins__ dict
# should restrict the available builtins to that dict only.
#
# CPython 3.12 behaviour: if globals["__builtins__"] is a dict, builtins are
# looked up exclusively in that dict.  The hardcoded Rust builtin table must
# not be consulted.

# Case 1: empty __builtins__ dict -> NameError for any builtin
try:
    exec("x = len([])", {"__builtins__": {}})
    print("FAIL: should have raised NameError")
except NameError as e:
    print("ok: empty __builtins__:", e)

# Case 2: __builtins__ dict containing only len -> len works, print does not
g = {"__builtins__": {"len": len}}
exec("x = len([])", g)
print("ok: len via custom __builtins__, x =", g["x"])

# Case 3: builtin absent from __builtins__ dict -> NameError
try:
    exec("y = str(1)", {"__builtins__": {"len": len}})
    print("FAIL: should have raised NameError for str")
except NameError as e:
    print("ok: str absent from __builtins__:", e)

# Case 4: no explicit globals -> builtins fully available (no regression)
result = eval("len([1, 2, 3])")
print("ok: eval without globals:", result)

# Case 5: exec without globals -> builtins fully available (no regression)
exec("_z = len([1, 2])")
print("ok: exec without globals")

# Case 6: empty globals dict (no __builtins__ key) -> injected builtins work
g2 = {}
exec("w = len([])", g2)
print("ok: exec with empty globals dict, w =", g2["w"])

# Case 7: __builtins__ dict with print builtin available
output_container = []
g3 = {"__builtins__": {"len": len, "range": range, "list": list}}
exec("result = list(range(3))", g3)
print("ok: list/range via custom __builtins__:", g3["result"])
