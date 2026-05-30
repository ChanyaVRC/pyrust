# Parity tests for __builtins__ injection by eval() and exec().
# CPython 3.12 injects __builtins__ into a caller-supplied globals dict
# the first time eval() or exec() is called with it (PyEval_EvalCode).
# Issue #1775.

# eval() injects __builtins__ into an empty globals dict
g = {}
eval("1+1", g)
print("__builtins__ in g:", "__builtins__" in g)   # True

# exec() injects __builtins__ into an empty globals dict
g2 = {}
exec("x = 1", g2)
print("__builtins__ in g2:", "__builtins__" in g2)  # True

# eval() does not overwrite an existing __builtins__
g3 = {"__builtins__": {}}
eval("1+1", g3)
print("g3 __builtins__ unchanged:", g3["__builtins__"] == {})  # True

# exec() does not overwrite an existing __builtins__
g4 = {"__builtins__": {}}
exec("x = 1", g4)
print("g4 __builtins__ unchanged:", g4["__builtins__"] == {})  # True

# Injected __builtins__ makes builtins accessible inside eval'd code
result = eval("len([1,2,3])", {})
print("len inside eval:", result)  # 3

# Injected __builtins__ makes builtins accessible inside exec'd code
ns = {}
exec("out = len([10, 20, 30, 40])", ns)
print("len inside exec:", ns["out"])  # 4

# __builtins__ remains after repeated calls with the same dict
g5 = {}
eval("1", g5)
eval("2", g5)
print("__builtins__ still present:", "__builtins__" in g5)  # True
