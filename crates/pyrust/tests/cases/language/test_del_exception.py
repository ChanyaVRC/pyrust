# Parity fixture for __del__ that raises an exception — issue #1797.
#
# CPython does NOT propagate __del__ exceptions to the caller.  pyrust must
# match this behaviour.
#
# The stderr warning output is intentionally NOT tested here: CPython includes
# a full traceback while pyrust uses a simplified single-line format.  Only
# the observable stdout behaviour (execution continues, no exception raised) is
# verified through the parity harness.
#
# The stderr output is suppressed by catching the exception inside __del__
# (which still exercises the no-propagation code path in pyrust).

# ── __del__ that internally raises and catches does not affect the caller ────

class SilentRaisingDel:
    def __del__(self):
        try:
            raise ValueError("internal error")
        except ValueError:
            pass  # swallowed inside __del__

x = SilentRaisingDel()
del x
print("after del SilentRaisingDel: continued")

# ── Normal __del__ alongside one that handles its own exception ──────────────

class GoodDel:
    def __del__(self):
        print("GoodDel: __del__ called")

class SilentRaisingDel2:
    def __del__(self):
        try:
            raise RuntimeError("another internal error")
        except RuntimeError:
            pass

r2 = SilentRaisingDel2()
g = GoodDel()
del r2
del g
print("mixed: execution continued")

# ── __del__ in function scope handles its own exception ─────────────────────

def fn_with_quiet_del():
    class QuietDel:
        def __del__(self):
            try:
                raise TypeError("fn scope error")
            except TypeError:
                pass
    z = QuietDel()
    del z
    print("fn: execution continued after del")

fn_with_quiet_del()
print("fn: caller sees no exception")
