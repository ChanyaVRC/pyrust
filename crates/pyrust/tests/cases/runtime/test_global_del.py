# Parity fixture: `global x; del x` inside a function removes x from module
# scope.  CPython 3.12 raises NameError on a subsequent module-level access.
# Regression for issue #531 (write-back loop was re-inserting the stale
# fastlocal register after the deletion).

# --- Basic case ---
x = 10
def f():
    global x
    del x
f()
try:
    print(x)
    print("no error")
except NameError as e:
    print("NameError")

# --- Re-assignment after deletion works ---
y = 5
def g():
    global y
    del y
g()
y = 99
print(y)   # 99

# --- Set then delete in the same function ---
z = 7
def h():
    global z
    z = 42
    del z
h()
try:
    print(z)
    print("no error")
except NameError:
    print("NameError")

# --- Deleting a global that was never assigned raises NameError at del ---
def del_undefined():
    global _never_assigned
    del _never_assigned
try:
    del_undefined()
    print("no error")
except NameError as e:
    print("NameError at del")

# --- Module-level del of an existing name ---
m = 123
del m
try:
    print(m)
    print("no error")
except NameError:
    print("NameError")

# --- Module-level del of a non-existent name raises NameError ---
try:
    del _no_such_name
    print("no error")
except NameError:
    print("NameError")
