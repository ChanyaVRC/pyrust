# Parity fixture for issue #460: PyError::Class carries class identity
# directly so the VM handler can skip the env-name lookup.
#
# This fixture exercises the exception paths that were updated to use the
# pre-resolved ExcClasses cache: AssertionError (RaiseAssert opcode),
# RuntimeError (PyError::Runtime arm in vm_try!), and RecursionError
# (call-depth guards in calls.rs and vm.rs).  It also exercises the
# generator/iterator StopIteration / GeneratorExit paths that use the
# class_name_is() helper to detect those exceptions across both the
# Named and Class variants.

# ── AssertionError ────────────────────────────────────────────────────────────
try:
    assert False, "something went wrong"
except AssertionError as e:
    print(type(e).__name__)          # AssertionError
    print("something went wrong" in str(e.args))  # True

try:
    assert False
except AssertionError:
    print("bare assert caught")

# ── ValueError / TypeError raised via PyError::Named (existing path) ──────────
try:
    int("not a number")
except ValueError:
    print("ValueError caught")

try:
    1 + "a"
except TypeError:
    print("TypeError caught")

# ── RuntimeError (PyError::Runtime arm) ───────────────────────────────────────
# The VM converts PyError::Runtime → RuntimeError via the exc_classes cache.
try:
    # Accessing an unbound name triggers a NameError via PyError::Named, but
    # we need a RuntimeError specifically.  Trigger one via a raw raise.
    raise RuntimeError("runtime test")
except RuntimeError as e:
    print(type(e).__name__)          # RuntimeError
    print("runtime test" in str(e.args))  # True

# ── RecursionError ────────────────────────────────────────────────────────────
def inf():
    return inf()

try:
    inf()
except RecursionError:
    print("RecursionError caught")

# ── StopIteration across generator boundary ───────────────────────────────────
def countdown():
    yield 3
    yield 2
    yield 1

print(list(countdown()))   # [3, 2, 1]

# Generator exhaustion via __next__ raises StopIteration which the for-loop /
# list() / iter() machinery must intercept via class_name_is("StopIteration").
class FiniteIter:
    def __init__(self, n):
        self._n = n

    def __iter__(self):
        return self

    def __next__(self):
        if self._n <= 0:
            raise StopIteration
        self._n -= 1
        return self._n + 1

print(list(FiniteIter(3)))  # [3, 2, 1]

# ── GeneratorExit ─────────────────────────────────────────────────────────────
def gen_with_cleanup():
    try:
        yield 10
        yield 20
    except GeneratorExit:
        print("GeneratorExit handled")
        return

g = gen_with_cleanup()
print(next(g))   # 10
g.close()        # triggers GeneratorExit; generator handles it

# ── Exception identity is correct ─────────────────────────────────────────────
# Verify that exceptions raised via the fast path still carry the correct
# class (not just the right name string).
try:
    raise ValueError("identity check")
except ValueError as e:
    print(isinstance(e, ValueError))   # True
    print(isinstance(e, Exception))    # True
    print(isinstance(e, TypeError))    # False

try:
    assert False, "id2"
except AssertionError as e:
    print(isinstance(e, AssertionError))  # True
    print(isinstance(e, Exception))       # True
