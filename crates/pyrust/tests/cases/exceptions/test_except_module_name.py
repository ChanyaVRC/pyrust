# PEP 3110: the `except E as var` binding is deleted when the handler exits,
# including when the handler body exits early via break/continue/return.
# Issue #1241.

# Exercise the second module-binding representation too: once globals() has
# escaped, an except binding is mirrored into its live dictionary and PEP 3110
# cleanup must remove both the fast-local slot and that dictionary entry.
globals()

# ── Module scope, normal exit ────────────────────────────────────────────────

# After a handler exits normally, the variable is gone.
try:
    raise ValueError("x")
except ValueError as e:
    pass

try:
    _ = e
except NameError as ne:
    print("module normal exit NameError:", str(ne))

# ── Module scope, break ──────────────────────────────────────────────────────

for _i in range(1):
    try:
        raise ValueError("brk")
    except ValueError as e:
        break  # early exit

try:
    _ = e
except NameError as ne:
    print("module break NameError:", str(ne))

# ── Module scope, continue ───────────────────────────────────────────────────

for _i in range(1):
    try:
        raise ValueError("cnt")
    except ValueError as e:
        continue  # early exit

try:
    _ = e
except NameError as ne:
    print("module continue NameError:", str(ne))

# ── Function scope, break ────────────────────────────────────────────────────

def fn_break():
    for _i in range(1):
        try:
            raise ValueError("brk")
        except ValueError as e:
            break  # early exit
    try:
        _ = e
    except UnboundLocalError as ue:
        return "UnboundLocalError: " + str(ue)
    except NameError as ne:
        return "NameError: " + str(ne)

print("fn break:", fn_break())

# ── Function scope, continue ─────────────────────────────────────────────────

def fn_continue():
    for _i in range(1):
        try:
            raise ValueError("cnt")
        except ValueError as e:
            continue  # early exit
    try:
        _ = e
    except UnboundLocalError as ue:
        return "UnboundLocalError: " + str(ue)
    except NameError as ne:
        return "NameError: " + str(ne)

print("fn continue:", fn_continue())

# ── Function scope, return from within handler ────────────────────────────────

def fn_return():
    try:
        raise ValueError("ret")
    except ValueError as e:
        return "returned from handler with e = " + str(e)

print("fn return:", fn_return())

# ── Variable is accessible inside the handler body before early exit ──────────

def fn_access_before_break():
    captured = None
    for _i in range(1):
        try:
            raise ValueError("abc")
        except ValueError as e:
            captured = str(e)
            break
    return captured

print("fn access before break:", fn_access_before_break())

# ── No exception raised: handler never runs, variable stays unbound ───────────

for _i in range(1):
    try:
        pass  # no exception
    except ValueError as e:
        break

try:
    _ = e
except NameError as ne:
    print("module no-exc NameError:", str(ne))
