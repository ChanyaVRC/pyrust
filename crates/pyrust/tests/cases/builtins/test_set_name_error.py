# Parity fixture for PEP 487 __set_name__ error reporting (issue #2703).
#
# When __set_name__ raises during class creation, CPython 3.12 re-raises the
# original exception with a note appended via __notes__:
#   Error calling __set_name__ on 'D' instance 'd' in 'C'
# (3.12 does NOT wrap it in RuntimeError; the original ValueError propagates.)


class D:
    def __set_name__(self, owner, name):
        raise ValueError("boom")


try:

    class C:
        d = D()

except ValueError as e:
    print("type:", type(e).__name__)
    print("message:", str(e))
    print("notes:", getattr(e, "__notes__", None))


# Multiple descriptors: the FIRST failure is reported and propagated.
class E:
    def __set_name__(self, owner, name):
        raise RuntimeError(f"fail for {name}")


try:

    class F:
        a = E()
        b = E()

except RuntimeError as e:
    print("multi type:", type(e).__name__)
    print("multi message:", str(e))
    print("multi notes:", getattr(e, "__notes__", None))
