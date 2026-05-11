# Tests for pass_compact_consts: remove unreferenced constant pool entries
# after other optimization passes eliminate instructions that referenced them.
#
# The most direct trigger is a constant-condition branch:
#   if True: x = A         → only the True-branch survives in the instruction stream
#   else:    x = B         → the constant B is orphaned after dead-code elimination
#
# pass_compact_consts scans the surviving instructions, marks referenced constants,
# and removes entries that no instruction references.  Behaviour is identical to
# CPython since the operation is purely an internal representation change —
# runtime semantics are unaffected.

# Dead else branch — constant 99 should be eliminated from the pool.
if True:
    x = 1
else:
    x = 99          # dead: never reached, so 99 becomes an orphaned pool entry
print(x)            # 1

# Dead if branch — constant 0 should be eliminated.
if False:
    y = 0           # dead
else:
    y = 42
print(y)            # 42

# Constant folding creates an intermediate constant that may become orphaned
# after subsequent algebraic simplification.  e.g. x + 0 folds to Move(x, x)
# → trivial NOP removes it, leaving the constant 0 unreferenced.
def no_op_add(n):
    return n + 0    # algebraic simplify: Move; the 0 constant may become orphaned
print(no_op_add(7))     # 7
print(no_op_add(-3))    # -3

# Multiple dead branches — several constants orphaned at once.
flag = True
if flag:
    v = 100
else:
    v = 200         # dead; 200 orphaned
if flag:
    w = 300
else:
    w = 400         # dead; 400 orphaned
print(v + w)        # 400

# Nested constant conditions.
if True:
    if True:
        result = "ok"
    else:
        result = "inner-dead"   # dead; "inner-dead" orphaned
else:
    result = "outer-dead"       # dead; "outer-dead" orphaned
print(result)       # ok

# pow(x, 1) algebraic identity: x**1 → Move(x); the constant 1 used as exponent
# may become orphaned after simplification.
def identity_pow(n):
    return n ** 1
print(identity_pow(5))      # 5
print(identity_pow(-2))     # -2

# Constant folding a chain — intermediate constants become unreferenced.
a = 2 + 3       # folded to 5; the constants 2 and 3 may be orphaned
b = a * 1       # algebraic → Move; the constant 1 orphaned
print(b)        # 5
