# Parity tests for exec(), eval(), and compile() builtins.
# CPython reference: Python 3.12

# --- exec: basic statement execution ---
exec("x = 2")
print(x)  # 2

# --- exec: multiline string ---
exec("""
y = 10
z = y * 3
""")
print(y)   # 10
print(z)   # 30

# --- exec: def statement makes function callable ---
exec("def add(a, b): return a + b")
print(add(3, 4))  # 7

# --- eval: expression returns value ---
print(eval("1 + 2"))      # 3
print(eval("'hello'"))    # hello
print(eval("[1, 2, 3]"))  # [1, 2, 3]

# --- eval with explicit globals dict ---
print(eval("x + 1", {"x": 5}))  # 6

# --- exec with explicit globals dict ---
ns = {}
exec("a = 42", ns)
print(ns["a"])  # 42

# --- exec with explicit globals and locals ---
# CPython: assignments go to locals; globals is read-only.
g = {"base": 10}
l = {}
exec("result = base + 5", g, l)
print(l["result"])      # 15 (written to locals)
print("result" not in g or g["result"] is None)  # True (globals unchanged)

# --- exec with None globals falls back to module scope ---
exec("module_var = 999", None)
print(module_var)  # 999

# --- SyntaxError for invalid code ---
try:
    exec("def :")
except SyntaxError:
    print("SyntaxError")  # SyntaxError

# --- SyntaxError for assignment in eval ---
try:
    eval("x = 1")
except SyntaxError:
    print("SyntaxError")  # SyntaxError

# --- TypeError for too few arguments ---
try:
    exec()
except TypeError:
    print("TypeError")  # TypeError

# --- compile + exec ---
code_exec = compile("compiled_x = 55", "<string>", "exec")
exec(code_exec)
print(compiled_x)  # 55

# --- compile + eval ---
code_eval = compile("3 + 4", "<string>", "eval")
print(eval(code_eval))  # 7

# --- compile: invalid mode raises ValueError ---
try:
    compile("x=1", "<s>", "badmode")
except ValueError:
    print("ValueError")  # ValueError

# --- runtime error inside exec propagates with correct type ---
try:
    exec("1 / 0")
except ZeroDivisionError:
    print("ZeroDivisionError from exec")  # ZeroDivisionError from exec

# --- runtime error inside eval propagates with correct type ---
try:
    eval("1 / 0")
except ZeroDivisionError:
    print("ZeroDivisionError from eval")  # ZeroDivisionError from eval
