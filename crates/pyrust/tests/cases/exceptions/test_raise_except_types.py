# Parity fixture for issues #1083 and #1084:
# - raise <non-exception> must raise TypeError, not RuntimeError
# - raise X from <non-exception> must raise TypeError
# - except <non-exception-class>: must raise TypeError, not RuntimeError


def check_raise_non_exception():
    def r_int():
        raise 42

    def r_str():
        raise "not an exception"

    def r_int_class():
        raise int  # int is a class, but not an exception class

    for fn, label in [
        (r_int, "raise 42"),
        (r_str, 'raise "str"'),
        (r_int_class, "raise int"),
    ]:
        try:
            fn()
        except TypeError as e:
            print(f"{label} -> TypeError: {e}")
        except Exception as e:
            print(f"{label} -> WRONG {type(e).__name__}: {e}")


def check_raise_from_cause():
    def r_from_int():
        raise RuntimeError("r") from 42

    def r_from_str():
        raise RuntimeError("r") from "bad"

    for fn, label in [
        (r_from_int, "raise from 42"),
        (r_from_str, "raise from str"),
    ]:
        try:
            fn()
        except TypeError as e:
            print(f"{label} -> TypeError: {e}")
        except Exception as e:
            print(f"{label} -> WRONG {type(e).__name__}: {e}")

    # None is a valid cause: suppresses context display
    try:
        raise ValueError("v") from None
    except ValueError as e:
        print("raise from None -> ValueError:", e)

    # A real exception is a valid cause
    try:
        raise ValueError("v") from TypeError("t")
    except ValueError as e:
        print("raise from exc -> ValueError:", e)


def check_except_non_class():
    def e_int():
        try:
            raise ValueError("x")
        except 42:
            pass

    def e_tuple_bad():
        # CPython validates the entire tuple before matching, so this raises
        # TypeError even though ValueError would have matched the first element.
        try:
            raise ValueError("x")
        except (ValueError, 42):
            pass

    for fn, label in [
        (e_int, "except 42"),
        (e_tuple_bad, "except (ValueError, 42)"),
    ]:
        try:
            fn()
        except TypeError as e:
            print(f"{label} -> TypeError: {e}")
        except Exception as e:
            print(f"{label} -> WRONG {type(e).__name__}: {e}")

    # Valid single-class except still works
    try:
        raise ValueError("ok")
    except ValueError as e:
        print("except ValueError -> ok:", e)

    # Valid tuple except still works
    try:
        raise TypeError("ok")
    except (ValueError, TypeError) as e:
        print("except (ValueError, TypeError) -> ok:", e)


check_raise_non_exception()
check_raise_from_cause()
check_except_non_class()
