# Tests for pass_not_invert: UnaryOp(Not) + JumpIf* → JumpIf*(inverted)
#
# Without the optimization, `if not x:` emits:
#   UnaryOp(r, Not, x)
#   JumpIfFalse(r, body_exit)
# With the optimization, this becomes one instruction:
#   JumpIfTrue(x, body_exit)
#
# Correctness of the optimization is verified by observing the same output
# as CPython for all the patterns below.

# Basic if not
def check_if_not(x):
    if not x:
        return "falsy"
    return "truthy"

print(check_if_not(0))      # falsy
print(check_if_not(1))      # truthy
print(check_if_not(""))     # falsy
print(check_if_not("hi"))   # truthy
print(check_if_not([]))     # falsy
print(check_if_not([0]))    # truthy

# while not
def count_until_true(lst):
    i = 0
    while not lst[i]:
        i += 1
    return i

print(count_until_true([0, 0, 1, 2]))  # 2

# Chained not not (double negation)
def double_not(x):
    return not not x

print(double_not(0))    # False
print(double_not(42))   # True

# not in condition with side effects on both branches
results = []
for v in [True, False, True, False]:
    if not v:
        results.append(0)
    else:
        results.append(1)
print(results)          # [1, 0, 1, 0]

# not with comparison
def classify(n):
    if not (n > 0):
        return "non-positive"
    return "positive"

print(classify(-1))     # non-positive
print(classify(0))      # non-positive
print(classify(1))      # positive

# Nested if not
def nested(a, b):
    if not a:
        if not b:
            return "both falsy"
        return "only a falsy"
    if not b:
        return "only b falsy"
    return "both truthy"

print(nested(0, 0))     # both falsy
print(nested(0, 1))     # only a falsy
print(nested(1, 0))     # only b falsy
print(nested(1, 1))     # both truthy
