"""
Issue #1411: module-scope name lookup must be sequential (global→builtins),
not function-scope pre-scanned locals.  A later assignment at module scope
must NOT retroactively shadow earlier reads of the same name.
"""

# Read a builtin before shadowing it — should find the builtin, not NameError.
x = float
print(x)           # <class 'float'>

float = 99         # shadow at module scope
print(float)       # 99

# Earlier reference not affected by later assignment.
y = int
int = "not_int"
print(y)           # <class 'int'>

# After deletion, builtins are visible again.
del float
print(float)       # <class 'float'>

# NameError for a name that genuinely doesn't exist.
try:
    print(totally_undefined_name_xyzzy)
except NameError as e:
    print("NameError:", e)

# Function scope: pre-scan still applies — reference before assignment raises
# UnboundLocalError (which is a subclass of NameError).
def check_function_scope():
    try:
        _ = local_later
    except UnboundLocalError:
        print("UnboundLocalError in function scope: ok")
    local_later = 1

check_function_scope()

# A definitely-bound name is read efficiently (register fast path); verify it
# still produces the right value.
z = 42
z = z + 1
print(z)           # 43

# Augmented assignment on an undefined name that is later assigned at module
# scope must raise NameError ("name 'x' is not defined"), not the wrong
# "local variable referenced before assignment" message.
try:
    aug_target += 1
except NameError as e:
    print("NameError aug:", e)
aug_target = 0
