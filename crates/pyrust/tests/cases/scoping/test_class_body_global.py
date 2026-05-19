# Parity fixture for issue #618 (optimizer regression):
# When a class body declares `global x` and assigns `x = <val>`, the module-
# level name `x` must reflect the new value in ALL subsequent operations, not
# just in `print(x)` calls.  A bug in the optimizer's constant-fold pass was
# treating the module-level register for `x` as still holding the pre-class
# value, producing wrong results for arithmetic and comparisons.

# --- Basic: arithmetic and comparison after global write ---
x = 0
class C:
    global x
    x = 42

print(x)        # 42
print(x + 1)    # 43  (was: 1 due to optimizer constant-fold bug)
print(x == 42)  # True  (was: False)
print(x > 0)    # True  (was: False)
print(x * 2)    # 84   (was: 0)


# --- Chained reads ---
y = 0
class D:
    global y
    y = 10

result = y + y * 2  # 10 + 20 = 30
print(result)       # 30  (was: 0)


# --- Read-modify-write pattern ---
counter = 10
class E:
    global counter
    counter = counter + 1  # reads module global (10), writes 11

print(counter)      # 11
print(counter + 1)  # 12  (was: 1 due to constant-fold of original 10)


# --- Variable assigned via class and then used in condition ---
flag = False
class SetFlag:
    global flag
    flag = True

if flag:
    print("flag is True")   # flag is True
else:
    print("flag is False")  # wrong


# --- Multiple globals; check both see updated values ---
p = 0
q = 0
class G:
    global p, q
    p = 11
    q = 22

print(p + q)   # 33  (was: 0 due to both being constant-folded)


# --- Regression guard: non-global names still become class attributes ---
a = 1
class H:
    global a
    a = 99
    b = 77    # class attribute

print(a)      # 99
print(H.b)    # 77
print(a + 1)  # 100  (was: 2 due to pre-class value of a=1)
