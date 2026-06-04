# Exercises the zero-cost exception-handler table (CPython 3.11 model): the
# compiler emits a per-pc handler table and strips the runtime block-setup
# instructions, so the VM reconstructs the active handler from the raising pc.
# Every control-flow shape below has to resolve to the same handler the dynamic
# block stack would have, so this is the parity gate for the rework.

out = []

# Raise inside a handler dispatches to the enclosing finally, not back to the
# inner except (the inner try region has already been left at the handler's pc).
def reraise_to_finally():
    try:
        try:
            raise ValueError("inner")
        except ValueError:
            raise KeyError("from-handler")
        finally:
            out.append("inner-finally")
    except KeyError as e:
        out.append(("outer-caught", str(e)))


reraise_to_finally()

# break / continue / return must run finally and then leave the protected region
# (so a later raise sees the *outer* handler, or none).
def loop_control():
    for i in range(4):
        try:
            if i == 0:
                continue
            if i == 2:
                break
            out.append(("body", i))
        finally:
            out.append(("fin", i))


loop_control()


def return_through_finally():
    for i in range(3):
        try:
            return i
        finally:
            out.append(("retfin", i))


out.append(("returned", return_through_finally()))

# Deeply nested try; the innermost raise walks out to the matching type.
def nested_three():
    try:
        try:
            try:
                raise IndexError("deep")
            except KeyError:
                out.append("wrong-1")
        except TypeError:
            out.append("wrong-2")
    except IndexError as e:
        out.append(("nested-caught", str(e)))


nested_three()

# with-statement whose __exit__ suppresses; control resumes after the with.
class Suppress:
    def __enter__(self):
        return self

    def __exit__(self, *a):
        return True


with Suppress():
    raise RuntimeError("swallowed-by-exit")
out.append("after-suppress")


# with-statement whose __exit__ does NOT suppress; exception propagates to the
# surrounding handler.
class Propagate:
    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


try:
    with Propagate():
        raise RuntimeError("propagated")
except RuntimeError as e:
    out.append(("with-propagated", str(e)))


# Generator that catches a thrown exception *at the yield point* (the resume pc
# is past the Yield, so the handler must be keyed off the Yield's pc).
def catching_gen():
    try:
        yield 1
        out.append("unreachable")
    except RuntimeError:
        out.append("gen-caught")
        yield 2
    finally:
        out.append("gen-finally")


g = catching_gen()
out.append(("g-first", next(g)))
out.append(("g-thrown", g.throw(RuntimeError("boom"))))

# bare raise re-raises the active exception.
def bare_reraise():
    try:
        try:
            raise ValueError("orig")
        except ValueError:
            raise
    except ValueError as e:
        out.append(("bare", str(e)))


bare_reraise()

# Exception raised in a finally replaces the in-flight one (context chaining).
def finally_raises():
    try:
        try:
            raise ValueError("first")
        finally:
            raise TypeError("second")
    except TypeError as e:
        out.append(("finally-ctx", type(e.__context__).__name__, str(e)))


finally_raises()

for item in out:
    print(item)
