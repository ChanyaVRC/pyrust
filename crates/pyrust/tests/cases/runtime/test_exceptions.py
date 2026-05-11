# try/except/else/finally, raise, reraising, assert

# try/except/else/finally
try:
    raise ValueError("bad")
except ValueError as err:
    print("try-except", err, err.args[0])
else:
    print("try-except", "miss")
finally:
    print("try-finally", "done")

# Re-raising exceptions
def reraiser():
    try:
        raise RuntimeError("inner")
    except RuntimeError:
        raise


try:
    reraiser()
except RuntimeError as err:
    print("reraised", err)


# raise from
try:
    try:
        raise ValueError("inner-cause")
    except ValueError as inner:
        raise RuntimeError("outer") from inner
except RuntimeError as err:
    print("raise-from", err, err.__cause__)

try:
    raise RuntimeError("none-cause") from None
except RuntimeError as err:
    print("raise-from-none", err.__cause__ is None)

# try-else
def try_else(flag):
    try:
        if flag:
            return "body"
    except Exception:
        return "except"
    else:
        return "else"


print("try-else", try_else(False))

# assert
try:
    assert False, "oops"
except AssertionError as err:
    print("assert-fail", err)

assert True, "should not raise"
print("assert-ok")


# except tuple
try:
    raise ValueError("tuple-hit")
except (TypeError, ValueError) as err:
    print("except-tuple", err)

try:
    raise RuntimeError("nested-hit")
except (TypeError, ValueError, RuntimeError) as err:
    print("except-tuple-2", err)


# with: __exit__ suppresses exceptions when truthy
class SuppressCtx:
    def __enter__(self):
        print("with-enter", 1)
        return "token"

    def __exit__(self, exc_type, exc, tb):
        print("with-exit", exc_type is not None, exc is not None, tb is None)
        return True


with SuppressCtx() as token:
    print("with-body", token)
    raise ValueError("boom")
print("with-suppressed", 1)


# with: falsey __exit__ re-raises
class NoSuppressCtx:
    def __enter__(self):
        return None

    def __exit__(self, exc_type, exc, tb):
        print("with-nosuppress-exit", exc_type is not None)
        return False


try:
    with NoSuppressCtx():
        raise RuntimeError("again")
except RuntimeError as err:
    print("with-nosuppress", err)


# with: multiple context managers exit in reverse order
class TagCtx:
    def __init__(self, name):
        self.name = name

    def __enter__(self):
        print("with-tag-enter", self.name)
        return self

    def __exit__(self, exc_type, exc, tb):
        print("with-tag-exit", self.name)
        return False


with TagCtx("A"), TagCtx("B"):
    print("with-tag-body")


# with: enter failure should unwind already-entered managers
class BoomEnter:
    def __enter__(self):
        print("with-enter-fail-inner-enter")
        raise ValueError("enter-fail")

    def __exit__(self, exc_type, exc, tb):
        print("with-enter-fail-inner-exit")
        return False


class OuterSuppressOnEnterFail:
    def __enter__(self):
        print("with-enter-fail-outer-enter")
        return self

    def __exit__(self, exc_type, exc, tb):
        print("with-enter-fail-outer-exit", exc_type is not None)
        return True


with OuterSuppressOnEnterFail(), BoomEnter():
    print("with-enter-fail-body")
print("with-enter-fail-suppressed", 1)


class OuterNoSuppressOnEnterFail:
    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        print("with-enter-fail-outer-nosuppress", exc_type is not None)
        return False


try:
    with OuterNoSuppressOnEnterFail(), BoomEnter():
        print("with-enter-fail-body-2")
except ValueError as err:
    print("with-enter-fail-unsuppressed", err)

# Exception-as binding is deleted after handler exits — Issue #93
try:
    raise ValueError("test")
except ValueError as e:
    pass
try:
    print(e)
except NameError:
    print("except-as-deleted", "NameError")
