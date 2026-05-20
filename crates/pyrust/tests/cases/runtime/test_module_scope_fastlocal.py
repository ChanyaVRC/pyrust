# Parity fixture for issue #820: module-scope fastlocal registers.
#
# Verifies that module-scope names use fast register access (correctness check)
# and that globals() live-view is maintained when globals_accessed=true.

# 1. Basic module-scope assignment and read.
x = 42
print("basic", x)  # 42

# 2. Augmented assignment at module scope.
total = 0
total += 1
total += 2
print("aug-assign", total)  # 3

# 3. Tuple unpack at module scope.
a, b = 10, 20
print("unpack", a, b)  # 10 20

# 4. For-loop variable at module scope.
s = 0
for i in range(5):
    s += i
print("for-loop", s)  # 10

# 5. del at module scope removes the name.
z = 99
del z
try:
    print(z)
    print("del-fail")
except NameError:
    print("del-ok")  # NameError

# 6. After globals() is called, subsequent assignments update the dict.
y = 1
g = globals()
y = 2
print("globals-live", g["y"])  # 2

# 7. globals() dict write makes the name visible as a module global.
globals()["injected"] = 777
print("injected", injected)  # 777

# 8. Nested function reading module-scope variable sees the current value.
counter = 0
def inc():
    global counter
    counter += 1
inc()
inc()
print("nested-global", counter)  # 2
