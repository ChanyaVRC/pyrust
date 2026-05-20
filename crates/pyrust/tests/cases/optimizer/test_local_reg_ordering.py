# Regression test for issue #837: inner_local was a HashSet, so register
# numbers for function-scope locals were assigned in random order.  After the
# fix (IndexSet preserving insertion/declaration order), consecutive locals get
# consecutive register numbers, which is required for range-based optimizer
# passes such as pass_loadnone_merge.
#
# This fixture verifies observable correctness: the function must produce the
# right result regardless of internal register numbering.

def many_none_locals():
    a = None
    b = None
    c = None
    d = None
    e = None
    a = 1
    b = 2
    return a + b

print(many_none_locals())  # 3


def locals_with_params(x, y):
    a = None
    b = None
    c = None
    a = x + 1
    b = y + 2
    return a + b

print(locals_with_params(10, 20))  # 33


def locals_in_branches(flag):
    a = None
    b = None
    if flag:
        a = 1
    else:
        a = 2
    b = a * 3
    return b

print(locals_in_branches(True))   # 3
print(locals_in_branches(False))  # 6
