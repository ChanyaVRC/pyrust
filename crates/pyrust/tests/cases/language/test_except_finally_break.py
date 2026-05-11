# Tests for break/continue/return inside except blocks with finally (issue #158).
# The finally block must always run even when an early exit bypasses the normal
# end of the except handler.

# ── break inside except with finally ────────────────────────────────────────

def break_in_except_finally():
    results = []
    for i in range(3):
        try:
            raise ValueError()
        except ValueError:
            results.append("except")
            break
        finally:
            results.append("finally")
    return results

print("break-except-finally", break_in_except_finally())

# ── continue inside except with finally ─────────────────────────────────────

def continue_in_except_finally():
    results = []
    for i in range(3):
        try:
            raise ValueError()
        except ValueError:
            results.append("except")
            continue
        finally:
            results.append("finally")
    return results

print("continue-except-finally", continue_in_except_finally())

# ── return inside except with finally ───────────────────────────────────────

def return_in_except_finally():
    results = []
    try:
        raise ValueError()
    except ValueError:
        results.append("except")
        return results
    finally:
        results.append("finally")

print("return-except-finally", return_in_except_finally())

# ── break inside try body (no except) with finally ──────────────────────────

def break_in_try_finally():
    results = []
    for i in range(3):
        try:
            results.append("try")
            break
        finally:
            results.append("finally")
    return results

print("break-try-finally", break_in_try_finally())

# ── continue inside try body (no except) with finally ───────────────────────

def continue_in_try_finally():
    results = []
    for i in range(2):
        try:
            results.append("try")
            continue
        finally:
            results.append("finally")
    return results

print("continue-try-finally", continue_in_try_finally())

# ── return inside try body (no except) with finally ─────────────────────────

def return_in_try_finally():
    results = []
    try:
        results.append("try")
        return results
    finally:
        results.append("finally")

print("return-try-finally", return_in_try_finally())

# ── nested: except inside try/finally inside loop ───────────────────────────

def nested_except_finally_break():
    results = []
    for i in range(3):
        try:
            try:
                raise ValueError()
            except ValueError:
                results.append("inner-except")
                break
        finally:
            results.append("outer-finally")
    return results

print("nested-except-finally-break", nested_except_finally_break())

# ── return from except clears active exception ──────────────────────────────

def return_clears_exception():
    def inner():
        try:
            raise ValueError("boom")
        except ValueError:
            return "caught"
        finally:
            pass
    result = inner()
    # After returning from inner(), no exception should be active
    try:
        raise RuntimeError("second")
    except RuntimeError as e:
        return result + "-" + str(e)

print("return-clears-exc", return_clears_exception())
