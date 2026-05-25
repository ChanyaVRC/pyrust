# Comprehension scope isolation: loop variables must not leak into the enclosing scope.
# CPython 3 isolates each comprehension in its own implicit function scope.


# ── List comprehension ─────────────────────────────────────────────────────────

def list_comp_leaks():
    result = [x * 2 for x in range(5)]
    try:
        _ = x  # noqa: F821
        return "LEAKED"
    except NameError:
        return "OK"

print("list-scope", list_comp_leaks())

# Result is still correct
print("list-result", [x * 2 for x in range(5)])

# Nested list comp: neither loop variable leaks
def list_nested_leaks():
    _ = [a + b for a in range(3) for b in range(3)]
    leaked = []
    try:
        leaked.append(a)  # noqa: F821
    except NameError:
        pass
    try:
        leaked.append(b)  # noqa: F821
    except NameError:
        pass
    return "OK" if not leaked else "LEAKED"

print("list-nested-scope", list_nested_leaks())

# List comp with if: loop var still doesn't leak
def list_cond_leaks():
    _ = [x for x in range(10) if x % 2 == 0]
    try:
        _ = x  # noqa: F821
        return "LEAKED"
    except NameError:
        return "OK"

print("list-cond-scope", list_cond_leaks())


# ── Set comprehension ──────────────────────────────────────────────────────────

def set_comp_leaks():
    _ = {x * 2 for x in range(5)}
    try:
        _ = x  # noqa: F821
        return "LEAKED"
    except NameError:
        return "OK"

print("set-scope", set_comp_leaks())


# ── Dict comprehension ─────────────────────────────────────────────────────────

def dict_comp_leaks():
    _ = {k: v for k, v in enumerate(range(3))}
    leaked = []
    try:
        leaked.append(k)  # noqa: F821
    except NameError:
        pass
    try:
        leaked.append(v)  # noqa: F821
    except NameError:
        pass
    return "OK" if not leaked else "LEAKED"

print("dict-scope", dict_comp_leaks())


# ── Generator expression ───────────────────────────────────────────────────────

def genexp_leaks():
    _ = list(x for x in range(5))
    try:
        _ = x  # noqa: F821
        return "LEAKED"
    except NameError:
        return "OK"

print("genexp-scope", genexp_leaks())


# ── Outermost iterable evaluated in enclosing scope ───────────────────────────

# The outermost iterable must be evaluated before the implicit function runs,
# so it can reference variables that only exist in the enclosing scope.

def outer_iter_scope():
    items = [10, 20, 30]
    result = [x for x in items]
    return result

print("outer-iter", outer_iter_scope())

# If the enclosing scope variable is modified after the comp starts, the
# outermost iterable already captured its value.
def outer_iter_capture():
    n = 3
    result = [i for i in range(n)]
    n = 999  # modifying n after comp is no-op (range was already called)
    return result

print("outer-iter-capture", outer_iter_capture())


# ── Outer variables readable from inside the comprehension ────────────────────

def outer_var_read():
    multiplier = 7
    result = [x * multiplier for x in range(4)]
    return result

print("outer-var", outer_var_read())

def outer_var_set_comp():
    offset = 100
    result = {x + offset for x in range(3)}
    return sorted(result)

print("outer-var-set", outer_var_set_comp())

def outer_var_dict_comp():
    base = 5
    result = {k: k + base for k in range(3)}
    return result

print("outer-var-dict", outer_var_dict_comp())


# ── Module-level comprehension scope ──────────────────────────────────────────

# At module level, loop variables also do not leak (Python 3 behaviour).
_sentinel = "original"
_list_result = [_sentinel for _sentinel in range(3)]
# _sentinel should still be "original" here
print("module-scope", _sentinel)
print("module-list", _list_result)
